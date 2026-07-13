//! blob 핀 등록부 — reconcile GC ↔ dedup put 경합의 seam.
//!
//! ## 불변식
//! P1 `pin()`은 절대 블록하지 않는다(상호배제 0 — put은 GC를 기다리지 않는다).
//! P2 **보호 술어는 `landed` 하나뿐이다.**
//!      landed(sha) = 이 패스 동안 **커밋 rename이 Ok를 반환한** sha (sticky)
//!      live(sha)   = 지금 존재하는 핀 = **결말이 아직 확정되지 않은** put → **대기 조건**이지 보호가 아니다
//!    GC 보호 술어: restore ⇔ landed(sha)   ← 코호트 대기가 끝난 **뒤에만** 평가된다
//! P3 **커밋은 취소 불가다.** `PinGuard`는 커밋 클로저가 **소유**하며, Drop은 rename·마킹·fsync가
//!    모두 끝난 뒤 그 클로저 안에서 실행된다 → "핀이 죽었는데 rename이 나중에 착지"는 **불가능**.
//!    (tokio: 시작된 `spawn_blocking` 태스크는 abort 불가 — 퓨처를 드롭해도 클로저는 끝까지 실행된다.)
//!    ⇒ **핀의 죽음 = 그 put의 종료 결과(terminal outcome) 확정**이며, 결과는 landed에 이미 반영돼 있다.
//! P3′ **키 락도 같은 클로저가 소유한다**(B8: 같은 bucket/key 쓰기 직렬화). 무취소 커밋과 취소 가능한
//!    가드는 **공존할 수 없다** — 가드가 호출자 퓨처에 남으면 `upload_timeout`이 그것을 풀어버리고,
//!    같은 키의 재시도·delete가 **먼저 끝난 뒤** 낡은 rename이 깨어나 그 결과를 덮어쓴다(포인터 회귀·
//!    삭제된 키의 부활). ⇒ **두 가드는 같은 `spawn_blocking` 클로저 안에서, 핀 → 키 락 순으로 죽는다**
//!    → 키 락이 풀리는 순간 이 put은 이미 terminal이다. (증인: T-S1)
//! P4 보호 판정 API는 **`Graved::settle(self)` 하나뿐**이다. `Graved`는 **`PassGuard::grave()`의
//!    blob→무덤 rename이 성공했을 때만** 태어나고(private 필드·같은 모듈 외 생성자 0·derive 0),
//!    자기 `sha`와 **무덤 시점 코호트**를 품는다 → 판정이 **그 전이·그 sha에** 바인딩된다.
//!    `BlobPins`에 sha로 조회하는 **공개 술어는 존재하지 않는다**(`protected()` 없음).
//!    ⇒ `reconcile.rs`는 **훅과 `grave()`만** 볼 수 있고, **보호 상태를 읽을 수단이 아예 없다**
//!      → 사전확인 뮤턴트는 `reconcile.rs`에서 **표현 불가**다.
//! P5 `pass_live` 플래그는 `PassGuard`(Drop 보유)가 **fallible op 이전에** 획득한다 → `?` 누수 0.
//! P6 핀에는 **단조 증가 id**가 붙는다. 무덤 rename **직후** 그 sha의 live id를 스냅샷한 것이 **코호트**다.
//!    코호트는 **고정·유한**하며, 무덤 **이후**에 생긴 핀은 코호트에 들어오지 않는다
//!    (그 put은 `blob_path`에서 ENOENT를 보고 바이트를 재기록한다 → **자급자족**).
//! P7 **대기는 유한하며 fail-CLOSED다.** 멈춘 파일시스템 연산은 코호트 멤버를 영원히 살려 둘 수 있고
//!    `upload_timeout`은 **호출자 퓨처를 드롭할 뿐** blocking 클로저를 죽이지 못한다
//!    → **`upload_timeout`은 대기의 상계가 아니다**. `settle()`은 셋 중 먼저 오는 것에서 깨어난다:
//!      (a) `landed(sha)` 확정 → 즉시 복원(대기 0) · (b) 코호트 드레인 → `landed`로 판정 ·
//!      (c) `settle_timeout` 소진 → **fail-CLOSED**: 무덤을 정본으로 복원 · tombstone 유지 ·
//!          `gc_deleted` 무증가 · `tracing::error!`.
//!    `settled: Notify`는 **핀 drop**과 **`landed` 삽입** 양쪽에서 울린다 → (a)가 즉시 발화한다.
//!
//! ## ⚠ B-2에서의 위치 — **배선 완료**
//! GC 삭제 분기가 `hooks().pre_grave()` → `grave()` → `settle()`을 부른다(`reconcile.rs`).
//! 핀과 `landed`는 이제 **읽힌다**. B-1이 달아 둔 dead-code 허용 속성 넷은 **배선과 함께 사라졌다** —
//! 남은 하나(`PassGuard::recovered`)는 B-3(관측성 배선)이 제거한다.
//! **비트로트 격리 분기는 핀·무덤을 거치지 않는다**(D-4 — F-25로 분리).

use super::atomic;
use super::locks::KeyGuard;
use super::Store;
use crate::layout::Layout;
use futures::future::BoxFuture;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OwnedMutexGuard;

// ── 결정적 배리어 ────────────────────────────────────────────────────────────
// 배리어는 **프로덕션 코드와 같은 경로**를 지난다. `BlobPins`가 소유하고(prod = 전부 None),
// put 경로와 GC 경로가 **같은 등록부의 같은 훅**을 본다 → `#[cfg(test)]` 코드 경로 분기가 없다.

type AsyncHook = Arc<dyn Fn(&str) -> BoxFuture<'static, ()> + Send + Sync>;
type SyncHook = Arc<dyn Fn(&str) + Send + Sync>;
type FailHook = Arc<dyn Fn(&str) -> std::io::Result<()> + Send + Sync>;

/// 필드는 정확히 7개다. 늘리지 마라.
#[derive(Clone, Default)]
pub(crate) struct Hooks {
    post_observe: Option<AsyncHook>,
    during_collect: Option<AsyncHook>,
    pre_grave: Option<AsyncHook>,
    post_grave: Option<AsyncHook>,
    in_commit_pre_rename: Option<SyncHook>,
    in_commit_post_landed: Option<SyncHook>,
    restore_io: Option<FailHook>,
}

impl Hooks {
    pub(crate) async fn post_observe(&self, sha: &str) {
        if let Some(h) = &self.post_observe {
            h(sha).await;
        }
    }
    pub(crate) async fn during_collect(&self, sha: &str) {
        if let Some(h) = &self.during_collect {
            h(sha).await;
        }
    }
    pub(crate) async fn pre_grave(&self, sha: &str) {
        if let Some(h) = &self.pre_grave {
            h(sha).await;
        }
    }
    pub(crate) async fn post_grave(&self, sha: &str) {
        if let Some(h) = &self.post_grave {
            h(sha).await;
        }
    }
    pub(crate) fn in_commit_pre_rename(&self, sha: &str) {
        if let Some(h) = &self.in_commit_pre_rename {
            h(sha);
        }
    }
    pub(crate) fn in_commit_post_landed(&self, sha: &str) {
        if let Some(h) = &self.in_commit_post_landed {
            h(sha);
        }
    }
    pub(crate) fn restore_io(&self, sha: &str) -> std::io::Result<()> {
        match &self.restore_io {
            Some(h) => h(sha),
            None => Ok(()),
        }
    }
}

// ── 등록부 ───────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub(crate) struct BlobPins {
    /// 동기 Mutex — 임계구역이 await를 걸치지 않는다(P1).
    inner: Arc<Mutex<Inner>>,
    /// **두 곳에서** 울린다: ① `PinGuard::drop`(코호트 드레인 진행) ② `landed` 삽입(보호 확정 → 즉시 깨움).
    settled: Arc<tokio::sync::Notify>,
    /// 프로세스 내 라이브 패스 ≤ 1.
    pass_lock: Arc<tokio::sync::Mutex<()>>,
    /// 결정적 배리어. prod = 전부 None.
    hooks: Hooks,
}

#[derive(Default)]
struct Inner {
    /// 단조 증가 핀 id (P6).
    next_id: u64,
    /// sha → 살아있는 핀 id 집합.
    live: HashMap<String, HashSet<u64>>,
    /// 커밋 rename이 Ok를 반환한 sha (sticky, 패스 스코프).
    landed: HashSet<String>,
    pass_live: bool,
}
// ※ `armed` 맵도, `touched := armed 스냅샷` 시드도 **없다**.

impl BlobPins {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 배리어 주입 생성자(테스트 전용). prod은 `new()`만 쓴다.
    #[cfg(test)]
    pub(crate) fn with_hooks(hooks: Hooks) -> Self {
        Self {
            hooks,
            ..Self::default()
        }
    }

    /// 배리어 전용 접근자.
    pub(crate) fn hooks(&self) -> &Hooks {
        &self.hooks
    }

    /// blob을 **보기 전에** 잡는다. 동기·무대기. 새 id를 발급한다.
    pub(crate) fn pin(&self, sha: &str) -> PinGuard {
        let mut g = self.inner.lock().unwrap();
        g.next_id += 1;
        let id = g.next_id;
        g.live.entry(sha.to_owned()).or_default().insert(id);
        PinGuard {
            pins: self.clone(),
            sha: sha.to_owned(),
            id,
        }
    }

    // ── 아래는 **private**이다(`pub(crate)` 아님) → `reconcile.rs`는 술어를 부를 수조차 없다. ──

    fn enter_pass(&self) {
        let mut g = self.inner.lock().unwrap();
        g.pass_live = true;
        g.landed.clear();
    }

    fn exit_pass(&self) {
        let mut g = self.inner.lock().unwrap();
        g.pass_live = false;
        g.landed.clear();
    }

    /// 무덤 rename **직후** 호출된다. 그 시점의 live id 집합 = **코호트**.
    fn cohort_at_grave(&self, sha: &str) -> HashSet<u64> {
        self.inner
            .lock()
            .unwrap()
            .live
            .get(sha)
            .cloned()
            .unwrap_or_default()
    }

    /// **유한 대기.** 셋 중 **먼저 오는 것**에서 깨어난다 — 무한 대기가 표현 불가하다.
    /// `landed`가 **이미** true면 첫 검사에서 즉시 `Landed`(await 0회) — 코호트를 기다리지 않는다.
    /// 코호트가 비어 있으면 첫 검사에서 즉시 `Drained`(await 0회) — 정상 GC의 fast path.
    async fn await_settlement(
        &self,
        sha: &str,
        cohort: &HashSet<u64>,
        budget: Duration,
    ) -> Settlement {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let notified = self.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable(); // **검사 이전에** 등록 → lost wakeup 불가
            {
                // 동기 Mutex는 await를 **절대 걸치지 않는다**(P1 불변 유지)
                let g = self.inner.lock().unwrap();
                // ① 보호 **확정**. 나머지 코호트의 결말은 판정을 바꿀 수 없다(landed는 sticky·단일 술어)
                //    → 더 기다리는 것은 순손해다(그 객체가 그동안 404다).
                if g.landed.contains(sha) {
                    return Settlement::Landed;
                }
                // ② 코호트 전원 종료 = 모든 멤버의 종료 결과 확정 → landed가 정확히 반영돼 있다(P3)
                if g.live.get(sha).is_none_or(|ids| ids.is_disjoint(cohort)) {
                    return Settlement::Drained;
                }
            }
            // ③ **유한.** 예산이 끊기면 fail-CLOSED로 빠진다 — 멈춘 핀은 GC를 정지시킬 수 없다.
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Settlement::TimedOut;
            }
        }
    }

    /// **유일한 보호 술어.** 코호트 결말이 확정된 뒤에만 읽힌다.
    fn landed(&self, sha: &str) -> bool {
        self.inner.lock().unwrap().landed.contains(sha)
    }
}

/// 대기가 **왜** 끝났는가. `pins.rs` private — `reconcile.rs`는 이 타입을 볼 수 없다(P4 봉인).
enum Settlement {
    Landed,
    Drained,
    TimedOut,
}

// ── 핀 ───────────────────────────────────────────────────────────────────────

pub(crate) struct PinGuard {
    pins: BlobPins,
    sha: String,
    id: u64,
}

impl PinGuard {
    /// 관측은 핀을 통해서만(순서 = 타입). sha가 핀에서 나오므로 "핀은 A, 검사는 B" 뮤턴트도 표현 불가.
    pub(crate) async fn blob_intact(&self, layout: &Layout) -> bool {
        let ok = matches!(
            tokio::fs::read(layout.blob_path(&self.sha)).await,
            Ok(b) if hex::encode(Sha256::digest(&b)) == self.sha
        );
        self.pins.hooks.post_observe(&self.sha).await; // 결정적 배리어
        ok
    }

    /// **커밋 = 핀과 키 락을 함께 소비하는 무취소 연산.**
    /// 단일 blocking 클로저가 **두 가드를 모두 소유**한다 → 호출자 취소(upload_timeout·disconnect)가
    /// in-flight rename에서 **핀을 떼어낼 수도**(P3) **같은 키의 직렬화를 풀 수도**(B8) 없다.
    /// 키 락이 호출자 퓨처에 남아 있으면: 취소가 락을 풀고 → 같은 키의 재시도·delete가 락을 얻어
    /// **먼저 끝나고** → 뒤늦게 깨어난 낡은 rename이 **더 새 포인터를 덮어쓰거나 삭제된 키를
    /// 되살린다**. 그래서 락은 **커밋으로 이전된다**(증인: T-S1).
    ///
    /// # ⚠ 재시작-필요 복구 계약 (S-2)
    ///
    /// 커밋 클로저는 `PinGuard`와 `KeyGuard`를 **함께 소유**하며 rename·fsync가 끝난 뒤에야 놓는다.
    /// 시작된 `spawn_blocking`은 **취소할 수 없으므로**, **파일시스템 연산이 반환하지 않으면 그
    /// `bucket/key`는 syscall이 반환하거나 프로세스가 재시작될 때까지 쓰기 불가**가 된다.
    ///
    /// **이것은 의도된 교환이다** — 가드를 먼저 놓으면 detach된 낡은 커밋이 더 새로운 포인터를
    /// 덮어쓰거나 **성공적으로 삭제된 키를 되살린다**(무결성 손상). **가용성을 잃는 편이 낫다.**
    ///
    /// 근거(무엇을 사고 무엇을 파는가):
    /// - 멈춘 fs는 **병리적 상황**이고, 그 경우 이 스토어는 **이미 사실상 죽은 상태**다
    ///   — `reconcile`도 같은 fs를 읽는다(P7의 `settle_timeout`이 그 사실을 이미 모델링한다).
    /// - 홈랩 **단일 replica + RWO PVC**라 blast radius가 **그 키 하나**다.
    /// - **잠김(가용성) < 되살아나기(무결성)** — 삭제된 키가 부활하는 것은 **조용한 데이터 손상**이다.
    ///
    /// **침묵하지 않는다**: 같은 키의 대기자(PUT 재시도·타임아웃 없는 DELETE)는 `KeyLocks::lock`이
    /// `LOCK_WARN_AFTER`를 넘기는 순간 `tracing::error!`를 낸다 — **행동은 불변**(계속 기다린다).
    /// 증인: **T-S2**. 잠김 **없이** 되살아나기를 막는 설계(키-바인드 펜싱 / 버전화된 포인터 발행)는
    /// 이번 범위 밖 — **F-30**.
    pub(crate) async fn commit_pointer(
        self,
        key: KeyGuard,
        target: PathBuf,
        bytes: Vec<u8>,
    ) -> std::io::Result<()> {
        tokio::task::spawn_blocking(move || {
            let r = self.commit_blocking(&target, &bytes);
            // ⚠ **드롭 순서 고정**(획득 역순 = LIFO): ① 핀 ② 키 락. 암묵적 스코프 규칙에 맡기지 않는다.
            // 핀이 **먼저** 죽어야 키 락이 풀리는 순간 이 put은 **이미 terminal**이다(P3: 핀의 죽음
            // = 종료 결과 확정) → 같은 키의 다음 writer는 **결말이 확정된 세계**에서 시작한다.
            // 반대로 두면 다음 writer가 이전 put의 **살아있는 핀**과 겹친다(GC 코호트가 커진다).
            drop(self); // ① 핀: live[sha]에서 id 제거 + settle 깨움
            key.release(); // ② 키 락: 그제서야 같은 bucket/key의 다음 writer가 깨어난다
            r
        })
        .await
        .expect("join")
    }

    /// 커밋의 **동기 본체**. 두 가드는 이것이 반환한 **뒤에** 드롭된다 — `?`가 가드를 조기 해제할 수 없다.
    fn commit_blocking(&self, target: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let staged = atomic::stage_blocking(target, bytes)?; // ← 여기까지의 실패 = **흔적 0**
        self.pins.hooks.in_commit_pre_rename(&self.sha); // 동기 훅
        staged.commit_blocking(|| {
            // ← rename Ok 직후에만
            let landed_now = {
                let mut g = self.pins.inner.lock().unwrap();
                // **착지 흔적**. insert의 반환값 = 이번에 처음 들어갔는가(전이에서만 깨운다)
                g.pass_live && g.landed.insert(self.sha.clone())
            }; // ← 락을 **먼저 놓고** 깨운다(PinGuard::drop과 같은 규율)
               // 보호가 **확정**됐다 → 코호트 대기 중인 settle()을 **즉시** 깨운다.
               // notify_waiters()는 동기·논블로킹이고 런타임 컨텍스트를 요구하지 않는다.
            if landed_now {
                self.pins.settled.notify_waiters();
            }
            // ⚠ notify_waiters()가 이 훅보다 **먼저** 호출된다 — 순서를 뒤집지 마라(B-2 T-P4b-2).
            self.pins.hooks.in_commit_post_landed(&self.sha); // rename **이후**
        })
    }
}

impl Drop for PinGuard {
    /// **핀의 죽음 = 이 put의 종료 결과 확정**(P3). `landed`는 건드리지 않는다.
    fn drop(&mut self) {
        {
            let mut g = self.pins.inner.lock().unwrap();
            if let Some(ids) = g.live.get_mut(&self.sha) {
                ids.remove(&self.id);
                if ids.is_empty() {
                    g.live.remove(&self.sha);
                }
            }
        } // ← 락을 **먼저 놓고** 깨운다
        self.pins.settled.notify_waiters(); // 동기·논블로킹 → blocking 클로저 안에서도 안전
    }
}

// ── 패스 ─────────────────────────────────────────────────────────────────────

pub(crate) struct PassGuard {
    pins: BlobPins,
    _pass: OwnedMutexGuard<()>,
    layout: Layout,
    refs: HashSet<String>,
    recovered: usize,
    /// 호출자가 **명시**한다. 기본값을 숨기지 않는다 — 이것이 대기의 **유일한 상계**다.
    settle_timeout: Duration,
}

impl PassGuard {
    /// **패스 순서의 유일한 소유자.** P5: 플래그를 든 가드를 **fallible op 이전에** 만든다.
    pub(crate) async fn begin(store: &Store, settle_timeout: Duration) -> std::io::Result<Self> {
        let _pass = store.pins().pass_lock.clone().lock_owned().await;
        let mut me = Self {
            pins: store.pins().clone(),
            _pass,
            layout: store.layout().clone(),
            refs: HashSet::new(),
            recovered: 0,
            settle_timeout,
        };
        me.pins.enter_pass(); // pass_live = true; landed.clear()
        // ↓ 이 아래 모든 `?`는 me(Drop 보유)를 통과한다 → pass_live/landed 누수 불가
        let recovered = super::reconcile::recover_graves(&me.layout).await?; // collect **이전**
        me.recovered = recovered;
        let refs = super::reconcile::collect_referenced(&me.layout, me.pins.hooks()).await?;
        me.refs = refs;
        Ok(me)
    }

    pub(crate) fn referenced(&self) -> &HashSet<String> {
        &self.refs
    }

    #[allow(dead_code)] // B-3에서 제거 — 그때 관측성(tracing)이 이것을 소비한다
    pub(crate) fn recovered(&self) -> usize {
        self.recovered
    }

    pub(crate) fn pins(&self) -> &BlobPins {
        &self.pins
    }

    /// blob → 무덤 rename + fsync. **성공했을 때만** `Graved`를 낳는다 — `Graved`의 유일한 생성자다.
    pub(crate) async fn grave<'p>(&'p self, sha: &str) -> std::io::Result<Graved<'p>> {
        atomic::rename_durable(
            &self.layout.blob_path(sha),
            &self.layout.grave_path(sha),
            &self.layout.objects_dir(),
        )
        .await?; // ← 여기가 실패하면 Graved는 없다
        // 무덤 이름이 **자리잡은 뒤에** 코호트를 뜬다(P6). 이 rename 이후에 pin한 put은
        // blob_path에서 ENOENT를 보므로 **자급자족**이다 → 구조적으로 코호트 밖.
        let cohort = self.pins.cohort_at_grave(sha);
        self.pins.hooks.post_grave(sha).await;
        Ok(Graved {
            pass: self,
            sha: sha.into(),
            cohort,
        })
    }
}

impl Drop for PassGuard {
    /// 디스크 무접촉 — 플래그와 패스-스코프 상태만 되돌린다.
    fn drop(&mut self) {
        self.pins.exit_pass();
    }
}

// ── 무덤 ─────────────────────────────────────────────────────────────────────

/// **무덤 rename 이후에만 존재할 수 있는 증거.** 파괴적 Drop 없음(흘리면 무덤 잔존 → 다음 패스 복구).
/// 필드 전부 private · **`pins.rs` 밖에 생성자 없음** · `Default`/`Clone`/`Copy` 유도 금지.
#[must_use = "Graved를 흘리면 무덤이 남는다 — settle하라"]
pub(crate) struct Graved<'p> {
    pass: &'p PassGuard,
    sha: String,
    /// 무덤 rename 시점에 살아있던 핀 id들 — **고정·유한 집합**.
    cohort: HashSet<u64>,
}

/// `Restored`/`Deferred`는 **디스크 전이가 동일**하다(무덤 → 정본). 갈라지는 것은 **왜**뿐이다:
/// `Restored` = 보호가 **확정**됐다(landed) · `Deferred` = 결말을 **알아내지 못했다**(타임아웃 → fail-CLOSED).
/// 변이를 나누는 이유는 **정직성**이다 — 타임아웃 복원을 "landed commit"으로 로깅하면 거짓말이다.
pub(crate) enum Settled {
    Restored,
    Reaped,
    Deferred,
}

impl Graved<'_> {
    /// **보호 판정의 유일한 API.** 자기 자신을 **소비**한다 → 판정은 이 무덤 전이·이 sha에 바인딩된다.
    /// **유한·fail-CLOSED**(P7): 멈춘 핀 하나가 GC를 영구 정지시킬 수 없다.
    pub(crate) async fn settle(self) -> std::io::Result<Settled> {
        let began = tokio::time::Instant::now();

        // ① **결말을 기다린다 — 단, 유한하게.** 무덤은 그동안 안전하게 보존된다(파괴 연산은 ③에서만).
        let outcome = self
            .pass
            .pins
            .await_settlement(&self.sha, &self.cohort, self.pass.settle_timeout)
            .await;

        let grave_path = self.pass.layout.grave_path(&self.sha);
        let blob_path = self.pass.layout.blob_path(&self.sha);
        let objects_dir = self.pass.layout.objects_dir();

        // ② **판정.** 보호 술어는 여전히 `landed` 하나뿐이다(P2).
        let (protect, verdict) = match outcome {
            // 보호 확정. 코호트 잔여 멤버의 결말은 판정을 바꿀 수 없다(landed는 sticky·단일 술어).
            Settlement::Landed => (true, Settled::Restored),
            // 결말을 **알고 나서** 판정한다.
            Settlement::Drained => match self.pass.pins.landed(&self.sha) {
                true => (true, Settled::Restored),
                // 실패·취소·ENOSPC put: 오늘과 동일하게 회수한다.
                false => (false, Settled::Reaped),
            },
            // **fail-CLOSED.** 결말을 알아내지 **못했다** → 보호 여부를 알 수 없다 → 보존을 택한다.
            // 무덤을 정본으로 되돌리고 tombstone은 유지 → 다음 패스가 새 스냅샷으로 재판정한다.
            // `gc_deleted`는 증가하지 않는다(회수하지 않았으므로).
            Settlement::TimedOut => {
                tracing::error!(
                    sha = %self.sha,
                    cohort_size = self.cohort.len(),
                    waited_ms = began.elapsed().as_millis() as u64,
                    "gc settle timed out — grave restored, reclamation deferred"
                );
                (true, Settled::Deferred)
            }
        };

        // ③ **파괴/복원은 판정 이후에만.** 어느 분기든 `?`로 탈출해도 무덤이 남을 뿐이다
        //    → 다음 패스의 `recover_graves`가 복원한다(fail-CLOSED by construction).
        if protect {
            self.pass.pins.hooks.restore_io(&self.sha)?; // fault injection
            atomic::rename_durable(&grave_path, &blob_path, &objects_dir).await?; // 되돌리기
            Ok(verdict) // Restored | Deferred
        } else {
            tokio::fs::remove_file(&grave_path).await?; // **무덤 이름만** 지운다
            atomic::fsync_dir(&objects_dir).await?;
            Ok(Settled::Reaped)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;
    use crate::store::reconcile;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 넉넉한 예산 — B-1에서는 무덤이 만들어지지 않으므로 발화하지 않는다.
    const SETTLE: Duration = Duration::from_secs(30);

    fn hex_sha(b: &[u8]) -> String {
        hex::encode(Sha256::digest(b))
    }

    async fn store_with_objects() -> (Store, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path().to_path_buf());
        tokio::fs::create_dir_all(d.path().join(".objects"))
            .await
            .unwrap();
        (s, d)
    }

    fn landed_has(s: &Store, sha: &str) -> bool {
        s.pins().inner.lock().unwrap().landed.contains(sha)
    }

    fn live_ids(s: &Store, sha: &str) -> HashSet<u64> {
        s.pins()
            .inner
            .lock()
            .unwrap()
            .live
            .get(sha)
            .cloned()
            .unwrap_or_default()
    }

    /// P1 — `pin()`도 `put()`도 **패스 보유 중에 블록하지 않는다**(상호배제 0).
    /// 블록되면 hang 대신 타임아웃으로 실패한다(`locks.rs` 관행).
    #[tokio::test]
    async fn pin_and_put_do_not_block_while_pass_is_live() {
        let (s, d) = store_with_objects().await;
        let pass = PassGuard::begin(&s, SETTLE).await.unwrap();

        let sha = "a".repeat(64);
        let pin = tokio::time::timeout(Duration::from_secs(5), async { s.pins().pin(&sha) })
            .await
            .expect("pin()은 패스 보유 중에도 블록하면 안 됨");
        assert!(live_ids(&s, &sha).contains(&pin.id));
        drop(pin);

        // put 전체(관측 → 바이트 → 커밋)도 패스를 기다리지 않는다
        let m = tokio::time::timeout(
            Duration::from_secs(5),
            s.put("b", "k", "text/plain", "u", b"live-pass".to_vec()),
        )
        .await
        .expect("put()은 패스 보유 중에도 블록하면 안 됨")
        .unwrap();
        assert!(tokio::fs::try_exists(s.blob_path(&m.sha256)).await.unwrap());
        drop(pass);
        drop(d);
    }

    /// 커밋 성공 → `landed ∋ sha` ∧ `live[sha]` 비어 있음(핀은 커밋 클로저 안에서 죽는다).
    #[tokio::test]
    async fn commit_pointer_lands_and_releases_pin() {
        let (s, d) = store_with_objects().await;
        let pass = PassGuard::begin(&s, SETTLE).await.unwrap();

        let sha = hex_sha(b"payload");
        let pin = s.pins().pin(&sha);
        let key = s.locks.lock("b", "k").await; // 커밋으로 **이전**된다(P3′)
        pin.commit_pointer(key, d.path().join("b").join("k.meta.json"), b"{}".to_vec())
            .await
            .unwrap();

        assert!(landed_has(&s, &sha), "커밋 rename이 Ok → 착지 흔적");
        assert!(live_ids(&s, &sha).is_empty(), "핀은 커밋이 끝나면 죽는다");
        drop(pass);
        // 패스가 끝나면 landed는 비워진다(패스 스코프)
        assert!(!landed_has(&s, &sha));
    }

    /// **stage 실패**(타깃 부모가 **파일**) → rename에 도달조차 못 한다 → `landed` **무흔적**.
    #[tokio::test]
    async fn stage_failure_leaves_no_landed_trace() {
        let (s, d) = store_with_objects().await;
        // 부모 자리에 파일 → mkdir_p/create가 ENOTDIR로 실패
        tokio::fs::write(d.path().join("b"), b"i am a file").await.unwrap();
        let pass = PassGuard::begin(&s, SETTLE).await.unwrap();

        let sha = hex_sha(b"never-staged");
        let pin = s.pins().pin(&sha);
        let key = s.locks.lock("b", "k").await;
        let r = pin
            .commit_pointer(key, d.path().join("b").join("k.meta.json"), b"{}".to_vec())
            .await;
        assert!(r.is_err(), "stage는 실패해야 한다");
        assert!(!landed_has(&s, &sha), "stage 실패 → 흔적 0");
        assert!(live_ids(&s, &sha).is_empty(), "실패해도 핀은 죽는다");
        drop(pass);
    }

    /// **흔적의 위치**: 흔적은 "커밋을 **시도**했다"가 아니라 "**착지했다**(rename이 Ok)"에만 생긴다.
    /// 뮤턴트 킬: `Staged::commit_blocking`의 `on_landed()`를 rename **앞**으로 옮기면
    /// 실패한 커밋도 흔적을 남겨 이 단언이 깨진다. **park 0 · spawn 0**(랑데부 없음).
    #[tokio::test]
    async fn landed_trace_only_when_rename_returns_ok() {
        let (s, d) = store_with_objects().await;
        // 커밋 타깃 자리에 **디렉터리** → stage는 성공하고 rename만 결정적으로 실패(EISDIR/ENOTEMPTY)
        let blocked = d.path().join("b").join("k.meta.json");
        tokio::fs::create_dir_all(&blocked).await.unwrap();
        let pass = PassGuard::begin(&s, SETTLE).await.unwrap();

        let sha = hex_sha(b"rename-fails");
        let pin = s.pins().pin(&sha);
        let key = s.locks.lock("b", "k").await;
        let r = pin.commit_pointer(key, blocked, b"{}".to_vec()).await;
        assert!(r.is_err(), "rename은 실패해야 한다");
        assert!(
            !landed_has(&s, &sha),
            "rename이 Err → 흔적 0 (on_landed는 rename Ok 이후에만 불린다)"
        );

        // 대조군: 같은 sha의 성공 커밋은 흔적을 남긴다 → 위 단언이 동어반복이 아님을 보인다
        // (앞 커밋이 실패해도 키 락은 그 클로저 안에서 풀렸다 → 이 lock()은 블록되지 않는다)
        let pin2 = s.pins().pin(&sha);
        let key2 = s.locks.lock("b", "ok").await;
        pin2.commit_pointer(key2, d.path().join("b").join("ok.meta.json"), b"{}".to_vec())
            .await
            .unwrap();
        assert!(landed_has(&s, &sha), "rename이 Ok → 흔적 1");
        drop(pass);
    }

    /// P6 — 핀 id 단조성: 같은 sha를 두 번 pin하면 **서로 다른 id** 2개가 live에 들어가고,
    /// 하나를 drop해도 나머지는 남는다(코호트 판정의 전제).
    #[tokio::test]
    async fn pin_ids_are_monotonic_and_independent() {
        let (s, _d) = store_with_objects().await;
        let sha = "c".repeat(64);
        let p1 = s.pins().pin(&sha);
        let p2 = s.pins().pin(&sha);
        assert_ne!(p1.id, p2.id, "핀 id는 단조 증가 — 충돌 불가");
        assert_eq!(live_ids(&s, &sha).len(), 2);

        let id2 = p2.id;
        drop(p1);
        let live = live_ids(&s, &sha);
        assert_eq!(live.len(), 1, "하나를 drop해도 나머지 핀은 남는다");
        assert!(live.contains(&id2));

        drop(p2);
        assert!(live_ids(&s, &sha).is_empty());
    }

    /// **D-3** — 데이터 루트 하나당 Store는 정확히 하나. `clone()`은 등록부를 공유하지만
    /// 같은 root의 `Store::new` 2개는 **공유하지 않는다**(= 버그 부활 해저드). 테스트로 못박는다.
    #[test]
    fn store_clone_shares_pin_registry_but_new_does_not() {
        let root = PathBuf::from("/data");
        let a = Store::new(root.clone());
        let b = a.clone();
        assert!(
            Arc::ptr_eq(&a.pins().inner, &b.pins().inner),
            "Store::clone()은 핀 등록부를 공유해야 한다"
        );
        let c = Store::new(root);
        assert!(
            !Arc::ptr_eq(&a.pins().inner, &c.pins().inner),
            "같은 root의 Store::new 2개는 등록부가 갈라진다 — reconcile이 다른 Store의 put을 못 본다(D-3)"
        );
    }

    /// 배리어가 **프로덕션 경로**를 지난다: put 하나가 `post_observe`(관측 후) ·
    /// `in_commit_pre_rename` · `in_commit_post_landed`(커밋 클로저 안)를 전부 발화시킨다.
    #[tokio::test]
    async fn hooks_fire_on_production_put_path() {
        let observed = Arc::new(AtomicUsize::new(0));
        let pre = Arc::new(AtomicUsize::new(0));
        let post = Arc::new(AtomicUsize::new(0));
        let (o, p, q) = (observed.clone(), pre.clone(), post.clone());

        let hooks = Hooks {
            post_observe: Some(Arc::new(move |_sha: &str| {
                let o = o.clone();
                Box::pin(async move {
                    o.fetch_add(1, Ordering::SeqCst);
                })
            })),
            in_commit_pre_rename: Some(Arc::new(move |_sha: &str| {
                p.fetch_add(1, Ordering::SeqCst);
            })),
            in_commit_post_landed: Some(Arc::new(move |_sha: &str| {
                q.fetch_add(1, Ordering::SeqCst);
            })),
            ..Hooks::default()
        };

        let d = tempfile::tempdir().unwrap();
        let s = Store::with_hooks(d.path().to_path_buf(), hooks);
        s.put("b", "k", "text/plain", "u", b"hooked".to_vec())
            .await
            .unwrap();

        assert_eq!(observed.load(Ordering::SeqCst), 1, "blob_intact 후 post_observe");
        assert_eq!(pre.load(Ordering::SeqCst), 1, "커밋 클로저 안 in_commit_pre_rename");
        assert_eq!(post.load(Ordering::SeqCst), 1, "on_landed 안 in_commit_post_landed");
    }

    /// **T-S1 — 무취소 커밋은 키 락을 **함께** 들고 죽는다**(B8: 같은 bucket/key 직렬화).
    ///
    /// 안무(랑데부):
    ///  ① A(put)를 spawn → 커밋 클로저의 `in_commit_pre_rename`에서 **park**
    ///     (이 순간 핀 **과** 키 락은 **클로저**의 소유다) → **도착 신호**를 await.
    ///  ② A의 **바깥 퓨처를 abort** → `JoinError::is_cancelled()`를 **await로 확인**
    ///     (abort ≠ 취소 완료). blocking 클로저는 여전히 park 중이다 — tokio는 그것을 죽이지 못한다.
    ///  ③ 같은 bucket/key로 **delete B**를 spawn → `timeout(200ms)`가 **pending**임을 관측
    ///     (= B가 커밋이 쥔 키 락에 막혔다).
    ///  ④ park 해제 → A의 클로저가 rename·fsync·핀drop·키락drop을 **완주**.
    ///  ⑤ **그제서야** B가 진행되어 **이긴다**: 순서 = [A:landed, B:deleted] ∧ 포인터 **부재**.
    ///
    /// 뮤턴트(키 가드를 **호출자 퓨처에** 남김 = S-1 이전 코드):
    ///  ② 취소가 락을 **풀어버린다** → ③의 timeout이 **완료**(B가 먼저 끝난다) → 첫 단언 RED.
    ///  그리고 뒤늦게 깨어난 A의 rename이 **삭제된 키를 되살린다** → 순서·부재 단언도 RED.
    #[tokio::test]
    async fn commit_holds_key_lock_until_rename_lands() {
        // 도착 신호(비동기 수신) · park 해제(동기 대기 — 커밋 클로저는 blocking 스레드에 있다)
        let (arrived_tx, mut arrived_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let seq: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let seq_hook = seq.clone();

        let hooks = Hooks {
            // 커밋 클로저 **안**, rename **직전**. 두 가드 모두 클로저가 들고 있다.
            in_commit_pre_rename: Some(Arc::new(move |_sha: &str| {
                arrived_tx.send(()).expect("도착 신호");
                release_rx.lock().unwrap().recv().expect("park 해제 신호");
            })),
            // rename이 Ok를 반환한 **직후** → A가 실제로 착지했음의 증거(공허한 통과 방지).
            in_commit_post_landed: Some(Arc::new(move |_sha: &str| {
                seq_hook.lock().unwrap().push("A:landed");
            })),
            ..Hooks::default()
        };

        let d = tempfile::tempdir().unwrap();
        let s = Store::with_hooks(d.path().to_path_buf(), hooks);
        let meta = s.meta_for("b", "k").unwrap();

        // ① A: put — 커밋 직전에서 park한다.
        let a = tokio::spawn({
            let s = s.clone();
            async move { s.put("b", "k", "text/plain", "u", b"A".to_vec()).await }
        });
        tokio::time::timeout(Duration::from_secs(5), arrived_rx.recv())
            .await
            .expect("A는 커밋 클로저(rename 직전)에 도달해야 한다")
            .expect("도착 신호 채널");

        // ② abort → **취소 완료까지 await**한다.
        a.abort();
        let joined = tokio::time::timeout(Duration::from_secs(5), a)
            .await
            .expect("취소는 완료되어야 한다(블로킹 클로저를 기다리지 않는다)");
        assert!(
            joined.expect_err("A의 퓨처는 취소된다").is_cancelled(),
            "abort → JoinError::is_cancelled (퓨처는 죽었다)"
        );
        // ※ 그러나 blocking 클로저는 **살아 있다** — 지금도 park 중이며 두 가드를 쥐고 있다.

        // ③ B: 같은 bucket/key delete. 커밋이 키 락을 쥐고 있으므로 **블록돼야 한다**.
        let mut b = tokio::spawn({
            let (s, seq) = (s.clone(), seq.clone());
            async move {
                let r = s.delete("b", "k").await;
                seq.lock().unwrap().push("B:deleted");
                r
            }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut b)
                .await
                .is_err(),
            "B는 무취소 커밋이 키 락을 놓을 때까지 블록돼야 한다 \
             — 가드가 호출자 퓨처에 남으면 여기서 B가 먼저 끝난다(RED)"
        );

        // ④ park 해제 → A의 클로저가 rename·fsync·핀drop·키락drop을 완주한다.
        release_tx.send(()).expect("park 해제");

        // ⑤ 그제서야 B가 진행되어 이긴다. 핸들은 **보유했다가** await하고 두 겹을 모두 unwrap한다.
        tokio::time::timeout(Duration::from_secs(5), b)
            .await
            .expect("B는 커밋이 끝나면 진행된다")
            .expect("B 태스크는 패닉/취소되지 않는다")
            .expect("delete는 성공한다");

        assert_eq!(
            *seq.lock().unwrap(),
            vec!["A:landed", "B:deleted"],
            "무취소 커밋이 먼저 **완주**하고(rename Ok), 그 다음에야 같은 키의 B가 실행된다"
        );
        assert!(
            !tokio::fs::try_exists(&meta).await.unwrap(),
            "B(delete)가 **이긴다** — 낡은 커밋이 삭제된 키를 되살리면 안 된다"
        );
        drop(d);
    }

    // ── T-S2 전용: tracing 캡처 (테스트 전용 · **새 의존성 0**) ───────────────────────────────
    // 저장소에 기존 캡처 관행은 **없다**(`tracing_subscriber`는 `main.rs`의 prod 초기화 전용).
    // 그래서 `tracing`(이미 dep)의 `Subscriber`를 **직접** 구현한다 — layer 스택·registry 불필요.
    // **스레드-로컬** default(`set_default`)라 다른 테스트로 새지 않는다. `#[tokio::test]`의 기본
    // current_thread 런타임은 `tokio::spawn`한 태스크도 **같은 스레드**에서 폴링하므로,
    // delete 태스크가 `KeyLocks::lock` 안에서 내는 이벤트까지 이 구독자가 잡는다.

    struct CaptureSubscriber(Arc<Mutex<Vec<String>>>);

    struct FieldVisitor(String);
    impl tracing::field::Visit for FieldVisitor {
        fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
            self.0.push_str(&format!(" {}={}", f.name(), v));
        }
        fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
            self.0.push_str(&format!(" {}={:?}", f.name(), v));
        }
    }

    impl tracing::Subscriber for CaptureSubscriber {
        /// ERROR·WARN·INFO만 잡는다(DEBUG/TRACE는 의존성 소음). `Level`의 Ord는
        /// `ERROR < WARN < INFO < DEBUG < TRACE`다.
        fn enabled(&self, m: &tracing::Metadata<'_>) -> bool {
            *m.level() <= tracing::Level::INFO
        }
        fn new_span(&self, _a: &tracing::span::Attributes<'_>) -> tracing::Id {
            tracing::Id::from_u64(1)
        }
        fn record(&self, _i: &tracing::Id, _v: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _i: &tracing::Id, _f: &tracing::Id) {}
        /// ⚠ **레벨을 접두사로 기록한다.** B-2의 증인들은 `settle()`의 **ERROR**
        /// (`"gc settle timed out"`)와 GC 루프의 **INFO**(`"GC restored: landed commit"`)를
        /// **구별해서 센다** — 레벨을 버리면 두 뮤턴트(`Deferred`↔`Restored` 혼동)가 살아남는다.
        fn event(&self, e: &tracing::Event<'_>) {
            let mut v = FieldVisitor(format!("{} ", e.metadata().level()));
            e.record(&mut v);
            self.0.lock().unwrap().push(v.0);
        }
        fn enter(&self, _i: &tracing::Id) {}
        fn exit(&self, _i: &tracing::Id) {}
    }

    /// 캡처된 이벤트 중 `level` ∧ `needle`을 **동시에** 만족하는 것의 개수.
    fn count_events(logs: &Arc<Mutex<Vec<String>>>, level: &str, needle: &str) -> usize {
        logs.lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with(level) && l.contains(needle))
            .count()
    }

    /// `needle`을 담은 ERROR 이벤트가 나타날 때까지 유계 폴링. 예산이 끊기면 **실패**한다
    /// → 경고를 지우는 뮤턴트가 여기서 RED가 된다("잠긴 키가 침묵한다").
    async fn wait_for_error_log(
        logs: &Arc<Mutex<Vec<String>>>,
        needle: &str,
        budget: Duration,
    ) -> Vec<String> {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let hits: Vec<String> = logs
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.starts_with("ERROR") && l.contains(needle))
                .cloned()
                .collect();
            if !hits.is_empty() {
                return hits;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "예산 안에 `{needle}` ERROR가 발화하지 않았다 — 잠긴 키가 **침묵한다**(관측성 부재)"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// **T-S2 — 재시작-필요 복구 계약**(S-2 인간 판정: 잠김 < 되살아나기).
    ///
    /// 이 테스트는 **선택한 유계 행동을 증명**한다 — 버그가 아니라 **교환**임을 못박는다:
    /// 멈춘 fs 위의 무취소 커밋은 그 `bucket/key`를 **쓰기 불가**로 만들고(가용성 손실),
    /// **그 사실을 시끄럽게 로그하며**(관측성), 커밋이 끝나면 **삭제가 이긴다**(무결성 보존).
    ///
    /// 안무(랑데부):
    ///  ① A(put)를 spawn → `in_commit_pre_rename`에서 **park**(무취소 클로저가 **핀 + 키 락** 보유)
    ///     → **도착 신호**를 await.
    ///  ② A의 바깥 퓨처를 **abort** → `JoinError::is_cancelled()`로 **취소 완료를 await**.
    ///     ※ blocking 클로저는 **죽지 않았다** — 지금이 "fs가 반환하지 않는" 상황의 결정적 모형이다.
    ///  ③ 같은 키의 **delete**(타임아웃 **없음** — `objects.rs`)를 spawn → `timeout(200ms)`가
    ///     **pending**임을 관측 = **그 키는 쓰기 불가다**.
    ///  ④ 경고 임계(`with_hooks_and_lock_warn`으로 100ms 주입)를 넘겨 `tracing::error!`가
    ///     **실제로 발화**함을 캡처해 단언 — 잠긴 `bucket`·`key`를 지목하는지까지.
    ///  ⑤ **행동 불변**: 경고 후에도 B는 **여전히 대기**한다(에러 반환 0 · 상계 0).
    ///  ⑥ park 해제 → A 완주.
    ///  ⑦ **그제서야** B가 진행되어 **이긴다**: 순서 = [A:landed, B:deleted] ∧ 포인터 **부재**.
    ///
    /// 뮤턴트:
    ///  - **경고 제거**(로그 한 줄 삭제) → ④의 `wait_for_error_log`가 예산을 소진 → **RED**(관측성 부재).
    ///  - **가드를 타임아웃으로 놓기**(= S-1 부활: 키 가드를 커밋 클로저로 옮기지 않음) → ②의 취소가
    ///    락을 풀어버려 ③의 `timeout`이 **완료**(B가 먼저 끝난다) → **RED**. 그리고 뒤늦게 깨어난 A의
    ///    rename이 **삭제된 키를 되살려** ⑦의 부재 단언도 **RED**. ⇒ 잠김을 없애면 **되살아난다**.
    #[tokio::test]
    async fn wedged_commit_keeps_key_unwritable_and_says_so_loudly() {
        /// 프로덕션의 `LOCK_WARN_AFTER`(30s)를 기다릴 수 없으므로 짧게 **주입**한다.
        /// 프로덕션 경로는 불변 — `Store::new`는 여전히 `KeyLocks::new()`를 쓴다.
        const WARN: Duration = Duration::from_millis(100);

        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let _sub = tracing::subscriber::set_default(CaptureSubscriber(logs.clone()));

        let (arrived_tx, mut arrived_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let seq: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
        let seq_hook = seq.clone();

        let hooks = Hooks {
            // 커밋 클로저 **안**, rename **직전** — 두 가드 모두 클로저의 것이다.
            // 이 park이 "반환하지 않는 파일시스템 연산"의 결정적 대역이다.
            in_commit_pre_rename: Some(Arc::new(move |_sha: &str| {
                arrived_tx.send(()).expect("도착 신호");
                release_rx.lock().unwrap().recv().expect("park 해제 신호");
            })),
            in_commit_post_landed: Some(Arc::new(move |_sha: &str| {
                seq_hook.lock().unwrap().push("A:landed");
            })),
            ..Hooks::default()
        };

        let d = tempfile::tempdir().unwrap();
        let s = Store::with_hooks_and_lock_warn(d.path().to_path_buf(), hooks, WARN);
        let meta = s.meta_for("wedged", "stalled-key").unwrap();

        // ① A: put — 커밋 직전에서 park.
        let a = tokio::spawn({
            let s = s.clone();
            async move {
                s.put("wedged", "stalled-key", "text/plain", "u", b"A".to_vec())
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), arrived_rx.recv())
            .await
            .expect("A는 커밋 클로저(rename 직전)에 도달해야 한다")
            .expect("도착 신호 채널");

        // ② abort → **취소 완료까지 await**(abort ≠ 취소 완료).
        a.abort();
        let joined = tokio::time::timeout(Duration::from_secs(5), a)
            .await
            .expect("취소는 완료되어야 한다(블로킹 클로저를 기다리지 않는다)");
        assert!(
            joined.expect_err("A의 퓨처는 취소된다").is_cancelled(),
            "abort → JoinError::is_cancelled (퓨처는 죽었다 — 그러나 클로저는 살아 있다)"
        );

        // ③ B: 같은 키의 delete(**타임아웃 없음**) → 쓰기 불가.
        let mut b = tokio::spawn({
            let (s, seq) = (s.clone(), seq.clone());
            async move {
                let r = s.delete("wedged", "stalled-key").await;
                seq.lock().unwrap().push("B:deleted");
                r
            }
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut b)
                .await
                .is_err(),
            "무취소 커밋이 키 락을 쥐고 있다 → 그 키는 **쓰기 불가**다(계약의 대가)"
        );

        // ④ **관측성** — 그 상황은 침묵하지 않는다.
        let warned = wait_for_error_log(
            &logs,
            "key lock held beyond threshold",
            Duration::from_secs(5),
        )
        .await;
        assert!(
            warned
                .iter()
                .any(|l| l.contains("bucket=wedged") && l.contains("key=stalled-key")),
            "경고는 **어느 키가** 잠겼는지 지목해야 한다 — 잡힌 ERROR: {warned:?}"
        );

        // ⑤ **행동 불변**: 로그는 로그일 뿐이다. B는 여전히 무한정 기다린다.
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut b)
                .await
                .is_err(),
            "경고 후에도 B는 **계속 기다린다** — 에러를 반환하지도, 상계를 갖지도 않는다"
        );
        assert!(
            seq.lock().unwrap().is_empty(),
            "아직 아무도 완주하지 않았다(경고가 B를 풀어주면 안 된다)"
        );

        // ⑥ park 해제 → A의 클로저가 rename·fsync·핀drop·키락drop을 완주.
        release_tx.send(()).expect("park 해제");

        // ⑦ 그제서야 B가 진행되어 **이긴다**. 핸들을 보유했다가 await + 두 겹 unwrap.
        tokio::time::timeout(Duration::from_secs(5), b)
            .await
            .expect("B는 커밋이 끝나면 진행된다")
            .expect("B 태스크는 패닉/취소되지 않는다")
            .expect("delete는 성공한다");
        assert_eq!(
            *seq.lock().unwrap(),
            vec!["A:landed", "B:deleted"],
            "무취소 커밋이 먼저 완주하고, 그 다음에야 같은 키의 delete가 실행된다"
        );
        assert!(
            !tokio::fs::try_exists(&meta).await.unwrap(),
            "delete가 **이긴다** — 가드를 타임아웃으로 놓았다면 여기서 낡은 커밋이 삭제된 키를 되살린다"
        );
        drop(d);
    }

    /// **T-C1 — 두 번째 플립 회귀 가드**(ENOSPC 무한연기의 기계 증인).
    /// 커밋 rename이 결정적으로 실패한 put(`Err(Internal)`)은 blob을 **보호하지 않는다** →
    /// 만료·미참조 blob은 오늘과 동일하게 회수된다(`gc_deleted == 1`).
    ///
    /// ⚠ 한계(정직하게): put은 reconcile **시작 전에** 완주(Err)하고 그 핀은 **이미 죽어 있다**
    /// → **park 0 · spawn 0**이며, **겹치는(overlapping) 실패 put**은 전혀 재현하지 못한다.
    /// 그 창의 증인은 T-C3(B-2)다. 이 테스트는 `landed` 흔적의 **위치**만 지킨다.
    ///
    /// **시각은 주입한다**(S-3 다리). tombstone의 기준 시각과 reconcile이 보는 `now`가 **같은 `T0`**이므로
    /// 만료가 **결정적으로** 성립한다 — `gc_grace = 0` 우회(*"0보다 오래됐으면 만료"*)가 필요 없고,
    /// **실제 grace가 걸린** 2단계 tombstone 경로를 그대로 지난다. **단언은 B-1과 동일하다.**
    #[tokio::test]
    async fn failed_commit_does_not_protect_blob_from_gc() {
        /// 실제 grace — 0 우회가 아니다.
        const GRACE: Duration = Duration::from_secs(3600);

        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::new(root.clone());

        // 1) blob을 만들고 참조를 지운다 → 미참조 blob이 디스크에 남는다
        let content = b"tc1-payload".to_vec();
        let sha = hex_sha(&content);
        s.put("b", "v", "text/plain", "u", content.clone())
            .await
            .unwrap();
        s.delete("b", "v").await.unwrap();

        // 2) 만료 tombstone: `T0`에서 볼 때 **grace를 넘긴** 과거에 최초 관측된 것으로 심는다.
        //    `T0 - 2·GRACE`가 첫 관측 → `T0`에서 경과 = 2·GRACE > GRACE → **만료**.
        let t0 = SystemTime::now();
        let first_seen = (t0 - 2 * GRACE)
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut pending = serde_json::Map::new();
        pending.insert(sha.clone(), serde_json::json!(first_seen));
        tokio::fs::write(
            root.join(".objects").join(".gc-pending.json"),
            serde_json::to_vec(&pending).unwrap(),
        )
        .await
        .unwrap();

        // 3) 커밋 rename을 결정적으로 실패시킨다: 포인터 자리에 디렉터리
        tokio::fs::create_dir_all(root.join("b").join("k.meta.json"))
            .await
            .unwrap();
        let r = s.put("b", "k", "text/plain", "u", content).await;
        assert!(
            matches!(r, Err(AppError::Internal(_))),
            "커밋 rename 실패 → Internal"
        );
        assert!(!landed_has(&s, &sha), "착지하지 못한 put은 흔적을 남기지 않는다");

        // 4) 실패한 put은 blob을 보호하지 않는다 → 회수된다. **주입형 시각 `T0`**로 판정한다.
        let stats = tokio::time::timeout(
            Duration::from_secs(5),
            reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE),
        )
        .await
        .expect("reconcile 패스는 유한 시간에 끝난다")
        .unwrap();
        assert_eq!(stats.gc_deleted, 1, "실패한 커밋은 blob을 보호하지 않는다");
        assert!(!tokio::fs::try_exists(s.blob_path(&sha)).await.unwrap());
    }

    /// **S-3 다리 스모크 — B-2의 배리어 안무가 *구성 가능함*을 증명한다.**
    ///
    /// 이 테스트가 존재하는 유일한 이유: **한 증인 안에서** ⓐ `Hooks`를 **짓고**(7개 필드는 `pins.rs`
    /// private → **이 모듈에서만** 리터럴 가능) ⓑ **주입형 시각**의 reconciler를 **돌린다**
    /// (`run_once_at`은 `reconcile.rs` private → **`run_once_at_for_test` 다리로만** 도달 가능).
    /// 이 둘이 갈라져 있으면 T-B1·T-B2·T-B4·T-C2·T-C3·T-P4a·T-P4b-1·T-P4b-2는 **쓸 수 없다.**
    ///
    /// **park 0 · spawn 0** — 랑데부는 B-2의 증인들이 한다. 여기서 증명하는 것은 **seam뿐**이다.
    /// 안무: 참조된 객체 R(포인터 살아 있음) + 미참조 blob X를 심고 **시계를 손으로 밀어** 두 패스를 돈다.
    ///  · 패스 1 `T0`             → X **최초 관측**(tombstone) → `gc_deleted == 0`
    ///  · 패스 2 `T0 + GRACE + 1s` → X **만료** → 회수 → `gc_deleted == 1` (**sleep 0**)
    /// 그 사이 `during_collect` 훅은 **프로덕션 경로**(`collect_referenced`)에서 R의 sha를 패스마다 본다.
    #[tokio::test]
    async fn barrier_hooks_and_injected_clock_compose_in_one_witness() {
        const GRACE: Duration = Duration::from_secs(100);

        // ⓐ 훅 — `pins.rs` private 필드를 리터럴로 짓는다(이 모듈 밖에서는 불가능하다).
        let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = collected.clone();
        let hooks = Hooks {
            during_collect: Some(Arc::new(move |sha: &str| {
                let (sink, sha) = (sink.clone(), sha.to_owned());
                Box::pin(async move {
                    sink.lock().unwrap().push(sha);
                })
            })),
            ..Hooks::default()
        };

        let d = tempfile::tempdir().unwrap();
        let s = Store::with_hooks(d.path().to_path_buf(), hooks);

        // R: 참조된 객체. 포인터가 **최소 1개** 있어야 `during_collect`가 발화한다(B-2 T-B1의 디코이 D와 같은 역할).
        let r = s
            .put("b", "kept.bin", "text/plain", "u", b"R".to_vec())
            .await
            .unwrap();
        // X: 미참조 blob(포인터 0) → GC 후보.
        let x_content = b"X-orphan".to_vec();
        let x_sha = hex_sha(&x_content);
        atomic::write_atomic(&s.blob_path(&x_sha), &x_content)
            .await
            .unwrap();

        let t0 = SystemTime::now();

        // ⓑ 패스 1 — 주입형 시각 `T0`. X는 **최초 관측**일 뿐 회수되지 않는다.
        let p1 = tokio::time::timeout(
            Duration::from_secs(5),
            reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE),
        )
        .await
        .expect("패스 1은 유한 시간에 끝난다")
        .unwrap();
        assert_eq!(p1.referenced, 1, "포인터는 R 하나 — 참조 스냅샷 누수 0");
        assert_eq!(p1.gc_deleted, 0, "grace 안 — 최초 관측만 기록된다");
        assert!(tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());

        // ⓒ 패스 2 — **시계를 grace 너머로 민다**(sleep 0 · 벽시계 무의존). X 만료 → 회수.
        let p2 = tokio::time::timeout(
            Duration::from_secs(5),
            reconcile::run_once_at_for_test(&s, t0 + GRACE + Duration::from_secs(1), GRACE, SETTLE),
        )
        .await
        .expect("패스 2는 유한 시간에 끝난다")
        .unwrap();
        assert_eq!(p2.referenced, 1, "여전히 R 하나");
        assert_eq!(p2.gc_deleted, 1, "주입형 시각이 tombstone을 만료시킨다 — sleep 없이");
        assert!(!tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());

        // ⓓ R은 두 패스 모두 생존하고, 훅은 **프로덕션 경로**에서 매 패스 R을 정확히 1회 봤다.
        assert!(tokio::fs::try_exists(s.blob_path(&r.sha256)).await.unwrap());
        assert_eq!(
            *collected.lock().unwrap(),
            vec![r.sha256.clone(), r.sha256],
            "during_collect(훅) × run_once_at(주입형 시각) — **한 테스트 안에서** 둘 다 쓸 수 있다"
        );
    }

    // ══════════════════════════════════════════════════════════════════════════════════════
    //  B-2 배리어 증인 — 공용 유틸
    //
    //  ⚠ §4 삭제 분기 자기검증: **모든** 배리어 증인은 두 단언을 **함께** 갖는다.
    //     ① `stats.referenced`의 **정확값**(`>=` 금지) — 테스트의 put이 만든 포인터가
    //        참조 스냅샷에 새어 들면 값이 1 커진다 → **참조됨 분기 누수**를 시끄럽게 잡는다.
    //     ② `post_grave` 훅의 `graved == vec![X_sha]` — `grave()`는 blob→무덤 rename이
    //        **성공한 뒤에만** 이 훅을 부른다 ⇒ **삭제 분기에 실제로 들어갔다**의 직접 증거.
    //     복원 증인의 기대값은 `gc_deleted == 0`인데 **참조됨 분기로 샌 경우에도 0**이다
    //     — 두 세계를 가르는 것은 오직 `referenced`와 `graved`다.
    //
    //  ⚠ §5 랑데부 규율: 개시 ≠ 완료. spawn ≠ 폴링됨 · abort ≠ 취소 완료 ·
    //     send ≠ 수신됨 · timeout Err ≠ 안쪽 완료 · async 호출 ≠ 폴링된 퓨처.
    // ══════════════════════════════════════════════════════════════════════════════════════

    use crate::store::reconcile::ReconcileStats;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
    use tokio::sync::Notify;
    use tokio::time::timeout;

    // ── 랑데부 프리미티브 — **위험한 반복구는 여기 한 곳에만 산다** ─────────────────────────
    //
    // 반복되던 셋은 전부 **틀리기 쉬운** 모양이었다:
    //  · 도착 await   — 두 겹(타임아웃 · 채널 종료)을 **둘 다** 언랩해야 한다.
    //  · 패스 완료    — **3중** 언랩(타임아웃 · `JoinError` · `io::Result`). 하나라도 빠지면
    //                   **핸들이 패닉을 삼킨다**(함정 5) → 증인이 조용히 공허해진다.
    //  · 대기 프로브  — 반드시 **`&mut`**. 값으로 넘기면 `Err`일 때 `JoinHandle`이 드롭돼
    //                   **GC가 detach된다**(함정 6) → 이후 단언이 경합한다.
    // 복사-붙여넣기 실수 하나가 **한 증인만 조용히 약화**시킬 수 있었다. 이제 그럴 수 없다.
    //
    // ⚠ **의미 파라미터가 0이다**(불리언·모드·예산 인자 없음). 각 헬퍼의 명제는 **이름에 고정**돼
    //    있고 호출부가 넘길 수 있는 것은 *무엇을* 기다리는가(채널·핸들)뿐이다 ⇒ 증인을 **차등적으로
    //    약화시킬 수단이 존재하지 않는다**. 예산을 인자로 받는 순간 그 수단이 생긴다 — **받지 않는다.**
    // ⚠ 증인별 **ⓐ→ⓔ 단계 순서는 인라인으로 남는다** — 그 순서가 곧 각 증인의 **명제**이기 때문이다.
    //    factor되는 것은 *어떻게 기다리는가*(위험한 기계)뿐이고, *무엇을 언제 기다리는가*가 아니다.

    /// 스폰된 GC 패스 핸들. `run_once_at_for_test`의 반환형에 고정된다.
    type PassHandle = tokio::task::JoinHandle<std::io::Result<ReconcileStats>>;

    /// 훅 **도착 신호** 하나를 유계 대기로 수신한다. 두 겹을 **모두** 언랩한다.
    async fn arrived(rx: &mut UnboundedReceiver<String>) -> String {
        timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("훅 도착 신호가 예산 안에 오지 않았다 — 랑데부가 성립하지 않았다")
            .expect("도착 채널")
    }

    /// GC 패스의 **정상 완료**. 3중 언랩 — 패닉도 `io::Error`도 삼키지 않는다.
    async fn finish_pass(gc: PassHandle) -> ReconcileStats {
        timeout(Duration::from_secs(5), gc)
            .await
            .expect("패스는 유한 시간에 끝난다")
            .expect("GC 태스크는 패닉하지 않는다")
            .expect("패스는 Ok다")
    }

    /// GC 패스가 **예산을 태우지 않고** 끝난다 — `SETTLE_LONG`(30s)의 **1/15**.
    ///
    /// ⚠ `finish_pass`(5s)와 **다른 명제**다. 여기서 **2초는 단언 그 자체**다(T-P4b-1 ⑤ ·
    ///   T-P4b-2 ⑤): `landed` 확정이 코호트 대기를 **건너뛰었다**는 것을 시간으로 잰다.
    ///   5초로 늘리면 분리도가 15× → 6×로 **약해진다** ⇒ 예산을 인자로 받는 대신 **함수를 갈라
    ///   둔다**. 이름이 곧 명제이므로 복사-붙여넣기 실수는 **읽는 순간 보인다**.
    async fn finish_pass_promptly(gc: PassHandle) -> ReconcileStats {
        timeout(Duration::from_secs(2), gc)
            .await
            .expect("`landed`가 확정된 패스는 코호트 예산(30s)을 태우지 않는다 — **15× 분리**")
            .expect("GC 태스크는 패닉하지 않는다")
            .expect("패스는 Ok다")
    }

    /// GC 패스가 **`Err`로 끝난다**(B7: `io::Error` 무가공 전파). 언랩 깊이는 `finish_pass`와
    /// 같다 — 패닉을 `Err`로 **오독하지 않는다**(`JoinError`를 먼저 언랩한다).
    async fn finish_pass_err(gc: PassHandle) -> std::io::Error {
        timeout(Duration::from_secs(5), gc)
            .await
            .expect("패스는 유한 시간에 끝난다")
            .expect("GC 태스크는 패닉하지 않는다")
            .expect_err("패스는 **Err**여야 한다 — `io::Error`를 삼키면 여기서 Ok가 된다")
    }

    /// 패스가 **아직 대기 중**임을 관측한다.
    /// **`&mut`만 받는다** ⇒ 호출부가 핸들을 잃지 않는다 → **detach가 표현 불가**다(함정 6).
    async fn probe_still_waiting(gc: &mut PassHandle) {
        assert!(
            timeout(Duration::from_millis(200), gc).await.is_err(),
            "패스는 settle 대기에 **머물러 있어야** 한다 — 여기서 완료되면 대기가 사라진 것이다"
        );
    }

    /// 실제 grace(0 우회가 아니다) — 2단계 tombstone 경로를 그대로 지난다.
    const GRACE: Duration = Duration::from_secs(3600);

    /// 만료 tombstone: `t0`에서 볼 때 grace를 **압도적으로** 넘긴 과거에 최초 관측된 것으로 심는다
    /// (`t0 - 2·GRACE` → `t0`에서 경과 = 2·GRACE > GRACE → **결정적 만료**).
    async fn seed_expired_tombstones(root: &Path, t0: SystemTime, shas: &[&str]) {
        let first_seen = (t0 - 2 * GRACE)
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut pending = serde_json::Map::new();
        for sha in shas {
            pending.insert((*sha).to_owned(), serde_json::json!(first_seen));
        }
        atomic::write_atomic(
            &root.join(".objects").join(".gc-pending.json"),
            &serde_json::to_vec(&pending).unwrap(),
        )
        .await
        .unwrap();
    }

    /// tombstone이 **아직 만료되지 않은** 주입 시각. 복구 패스를 이 시각으로 돌리는 이유(정직하게):
    /// 같은 `now`로 돌리면 그 패스가 복원 **직후** X를 **정당하게 다시 파묻고 reap**한다(X는 진짜
    /// 가비지다) → *"복구됐다"*가 `gc_deleted`로부터의 **간접 추론**으로 약해진다. 되돌리면
    /// **복원 그 자체를 직접 관측**한다. `run_once_at`의 `now`는 **이미 주입형 인자**다.
    fn before_expiry(t0: SystemTime) -> SystemTime {
        t0 - 2 * GRACE + Duration::from_secs(1)
    }

    /// 포인터가 **하나도 없는** blob을 디스크에 직접 심는다 → `referenced == 0`이 **구조적**이다.
    async fn plant_orphan_blob(s: &Store, content: &[u8]) -> String {
        let sha = hex_sha(content);
        atomic::write_atomic(&s.blob_path(&sha), content).await.unwrap();
        sha
    }

    /// `.objects` 직속 무덤 이름 전부(정렬). 정상 패스는 무덤을 **남기지 않는다**.
    ///
    /// ⚠ **여기(발견)는 layout을 경유하지만, 단언(핀)은 raw 리터럴이어야 한다.** ADR-0001:
    /// *"온디스크 이름 리터럴은 **테스트에서는 raw 문자열로 유지**한다. layout 상수를 경유시키면
    /// **동어반복**이 되어 회귀 감지력을 잃는다."* — 그래서 `grave_name_of(sha)`(= `layout::grave_name`의
    /// 순수 passthrough)는 **삭제했다**: 그것으로 만든 기대값은 `GRAVE_PREFIX`가 바뀌어도 **같이
    /// 바뀌므로** 접두사 드리프트를 **한 곳에서도 잡지 못했다**. 기대값은 `expected_grave_name()`이
    /// 짓는다 — 그 안에 온디스크 바이트가 **raw로** 박혀 있다.
    async fn grave_names(root: &Path) -> Vec<String> {
        let mut v = Vec::new();
        let mut rd = tokio::fs::read_dir(root.join(".objects")).await.unwrap();
        while let Some(e) = rd.next_entry().await.unwrap() {
            let n = e.file_name().to_string_lossy().to_string();
            if crate::layout::grave_sha(&n).is_some() {
                v.push(n);
            }
        }
        v.sort();
        v
    }

    /// **온디스크 무덤 이름의 raw 핀** — `layout::grave_name`을 **경유하지 않는다**(동어반복 금지).
    /// 접두사를 `.tomb-`로 바꾸는 뮤턴트는 무덤 생명주기 증인 전부(T-B5①·③·③′·④)에서 **RED**가
    /// 된다. 이름이 그 sha를 **품는다**는 것도 함께 핀한다.
    fn expected_grave_name(sha: &str) -> String {
        format!(".gc-grave-{sha}")
    }

    fn count_of(sink: &Arc<Mutex<Vec<String>>>, sha: &str) -> usize {
        sink.lock().unwrap().iter().filter(|s| *s == sha).count()
    }

    /// `post_grave` — **기록 전용**(§4 자기검증).
    fn grave_recorder(sink: Arc<Mutex<Vec<String>>>) -> AsyncHook {
        Arc::new(move |sha: &str| {
            let (sink, sha) = (sink.clone(), sha.to_owned());
            Box::pin(async move {
                sink.lock().unwrap().push(sha);
            })
        })
    }

    /// `post_grave` — **기록 + 도착 신호**(park 없음 — 통과한다).
    fn grave_recorder_signal(
        sink: Arc<Mutex<Vec<String>>>,
        tx: UnboundedSender<String>,
    ) -> AsyncHook {
        Arc::new(move |sha: &str| {
            let (sink, tx, sha) = (sink.clone(), tx.clone(), sha.to_owned());
            Box::pin(async move {
                sink.lock().unwrap().push(sha.clone());
                tx.send(sha).expect("post_grave 도착 신호");
            })
        })
    }

    /// `post_grave` — **기록 + 도착 신호 + park**(T-B5① 취소 증인 전용).
    fn grave_recorder_park(
        sink: Arc<Mutex<Vec<String>>>,
        tx: UnboundedSender<String>,
        gate: Arc<Notify>,
    ) -> AsyncHook {
        Arc::new(move |sha: &str| {
            let (sink, tx, gate, sha) = (sink.clone(), tx.clone(), gate.clone(), sha.to_owned());
            Box::pin(async move {
                sink.lock().unwrap().push(sha.clone());
                tx.send(sha).expect("post_grave 도착 신호");
                gate.notified().await; // 해제는 **abort**다(퓨처째로 드롭된다)
            })
        })
    }

    /// **async 훅 park** — 규율 1: **`send(도착)` ≺ `park`**(뒤집으면 신호가 영영 오지 않는다).
    /// 해제는 **`notify_one()`** — 대기자가 없어도 permit을 저장한다 ⇒ **lost wakeup 불가**.
    /// ⚠ `notify_waiters()`는 permit을 저장하지 **않는다** — 테스트 훅에 쓰면 유실된다.
    /// ⚠ `oneshot`도 못 쓴다 — `Receiver::await`가 `self`를 소비해 `Fn` 훅에 들어가지 않는다.
    fn async_park(tx: UnboundedSender<String>, gate: Arc<Notify>) -> AsyncHook {
        Arc::new(move |sha: &str| {
            let (tx, gate, sha) = (tx.clone(), gate.clone(), sha.to_owned());
            Box::pin(async move {
                tx.send(sha).expect("도착 신호");
                gate.notified().await;
            })
        })
    }

    /// `target` sha에 **한해서만** 도착 신호 + park. 무관한 put은 **통과**한다
    /// (T-B4의 데드락 부재 sanity가 이것에 의존한다 — 훅은 전역이다).
    fn async_park_for(
        target: String,
        tx: UnboundedSender<String>,
        gate: Arc<Notify>,
    ) -> AsyncHook {
        Arc::new(move |sha: &str| {
            let (target, tx, gate, sha) = (target.clone(), tx.clone(), gate.clone(), sha.to_owned());
            Box::pin(async move {
                if sha != target {
                    return;
                }
                tx.send(sha).expect("도착 신호");
                gate.notified().await;
            })
        })
    }

    /// **sync 훅 park**(커밋 blocking 클로저 **안**) — §5.3 park 함정.
    /// `std::sync::mpsc::recv(&self)`라 `Fn` 제약을 만족하고, **블로킹이 옳다**(blocking 스레드다).
    /// `Err(RecvError)`(= sender drop)가 **곧 해제 신호**다 ⇒ teardown이 park을 반드시 푼다.
    fn sync_park(tx: UnboundedSender<String>, rx: std::sync::mpsc::Receiver<()>) -> SyncHook {
        let rx = Mutex::new(rx);
        Arc::new(move |sha: &str| {
            tx.send(sha.to_owned()).expect("도착 신호");
            let _ = rx.lock().unwrap().recv(); // 동기 호출 — 퓨처가 아니다(함정 10 무관)
        })
    }

    // ── T-B1 ─────────────────────────────────────────────────────────────────────────────

    /// **T-B1 — put이 참조 수집(`collect_referenced`) *도중*에 완료된다.**
    ///
    /// 랑데부: `during_collect` = 도착(`collect_reached`) → `Notify` park.
    /// ⓐ GC **spawn** → ⓑ **도착 await**(패스가 `collect_referenced` **안**에 있음이 확정 —
    /// 여기서 안 기다리면 putter의 포인터가 `SeedRoot`의 루트 readdir **이전에** 착지해 `refs`에
    /// 샌다) → ⓒ putter를 **spawn하지 않고 완주까지 await**(완주 = 도착 · **핀 drop 확정**,
    /// 보조정리 L) → ⓓ 해제 → ⓔ `finish_pass(gc)`.
    ///
    /// **뮤턴트 M1** — `enter_pass()`를 `collect_referenced` 뒤로 → put 착지 시 `pass_live=false`
    /// → 흔적 0 ∧ refs에도 없음 → **Reap** → `get_bytes` 404 → **RED**.
    /// **equivalent(정직하게)** — `PassGuard::drop`의 `landed.clear()` 제거는 관측 동일(GREEN):
    /// 다음 패스가 시작 시 clear한다.
    #[tokio::test]
    async fn put_landing_during_reference_collection_is_protected() {
        let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (collect_tx, mut collect_rx) = unbounded_channel::<String>();
        let gate = Arc::new(Notify::new());

        let hooks = Hooks {
            during_collect: Some(async_park(collect_tx, gate.clone())),
            post_grave: Some(grave_recorder(graved.clone())),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);

        // 디코이 D — 포인터가 **살아 있다**. 장식이 아니라 **필수**다: `during_collect`는 포인터를
        // 1개 낼 때마다 발화하므로, 포인터가 하나도 없으면 훅이 **영영 발화하지 않아** 랑데부가 걸린다.
        let dec = s
            .put("b", "decoy.bin", "text/plain", "u", b"tb1-decoy".to_vec())
            .await
            .unwrap();
        // X — 만료·미참조 blob(포인터 0).
        let x_content = b"tb1-victim".to_vec();
        let x_sha = plant_orphan_blob(&s, &x_content).await;
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;

        // ⓐ GC spawn
        let s2 = s.clone();
        let gc =
            tokio::spawn(
                async move { reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE).await },
            );
        // ⓑ **도착 await** — spawn만 하고 넘어가는 지점 0개.
        //    (`during_collect`는 디코이 포인터에서 발화한다.)
        assert_eq!(arrived(&mut collect_rx).await, dec.sha256);

        // ⓒ **그 park 동안** putter — **완주까지 await**한다(spawn 지점이 아니다).
        //    패스 시작 시 **존재하지 않던 버킷** `fresh` → `SeedRoot` 스냅샷이 구조적으로 못 본다.
        let put = s
            .put("fresh", "v.bin", "text/plain", "u", x_content.clone())
            .await;
        assert!(put.is_ok(), "dedup put은 성공한다(실패하면 landed가 안 서서 **엉뚱한 이유로** RED다)");

        // ⓓ 해제 → ⓔ GC 완주(`finish_pass`가 `JoinError`와 `io::Result`를 **둘 다** 언랩한다).
        gate.notify_one();
        let stats = finish_pass(gc).await;

        // **삭제 분기 자기검증 ①** — 전수 `assert_eq!`. `referenced == 1` = **디코이 하나뿐**.
        // putter의 포인터가 스냅샷에 새어 들었다면 **2**가 된다 → 참조됨 분기 누수를 시끄럽게 잡는다.
        // `gc_pending == 1`은 X의 tombstone이 **복원 뒤에도 유지**됨(D-2)을 함께 못박는다.
        assert_eq!(
            stats,
            ReconcileStats {
                referenced: 1,
                gc_deleted: 0,
                gc_pending: 1,
                temps_deleted: 0,
                quarantined: 0,
            }
        );
        // **삭제 분기 자기검증 ②** — 무덤이 **실제로 파였다**(= `Graved`가 태어났다).
        assert_eq!(*graved.lock().unwrap(), vec![x_sha.clone()]);

        let (_, got) = s
            .get_bytes("fresh", "v.bin")
            .await
            .expect("참조 수집 창 안에서 착지한 dedup put은 유실되면 안 된다");
        assert_eq!(got, x_content);
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
    }

    // ── T-B2 ─────────────────────────────────────────────────────────────────────────────

    /// **T-B2 — "사전확인 ↔ 무덤 rename" 창의 결정적 증인**(2차 방어선).
    ///
    /// GC를 **모델링된 사전확인 지점**(`pre_grave`)에서 park한다. **그 park 동안** putter가
    /// **비로소 시작**해 X를 dedup 관측(`blob_intact == true` — 무덤은 아직 없다)하고 **완전히
    /// 착지**한다(핀 drop · 포인터 on-disk). 그 다음 GC 재개 → `grave()` → `settle()`.
    /// 무덤 시점 **코호트는 비어 있는데도**(핀이 이미 죽었다) `landed ∋ sha` → **Restore 필수**.
    ///
    /// **뮤턴트** ① `landed` 삽입 삭제 → 코호트도 비고 `landed`도 비었다 → Reap → 404 → RED.
    /// ② `pins.rs`에 lock-and-peek 사전확인 추가(= 새 API 추가라 **컴파일된다** — 봉인은 모듈
    /// 경계지 타입 마법이 아니다) → `pre_grave` 시점엔 putter가 **시작조차 안 했다** → 미보호 판정
    /// → Reap → 그 사이 putter가 dedup으로 착지 → **포인터 + blob 부재** → 404 → RED.
    #[tokio::test]
    async fn put_landing_between_pre_grave_and_grave_is_protected() {
        let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (pre_tx, mut pre_rx) = unbounded_channel::<String>();
        let gate = Arc::new(Notify::new());

        let hooks = Hooks {
            pre_grave: Some(async_park(pre_tx, gate.clone())),
            post_grave: Some(grave_recorder(graved.clone())),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);

        let x_content = b"tb2-victim".to_vec();
        let x_sha = plant_orphan_blob(&s, &x_content).await;
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;

        // ⓐ GC spawn → ⓑ **도착 await**. 이 신호가 *"패스가 실제로 시작했다"*를 **기계로** 못박는다
        //    — 없으면 putter가 `fresh` 버킷을 `SeedRoot`의 루트 readdir **이전에** 만들어
        //    포인터가 `refs`에 샌다(= 참조됨 분기 누수 재발).
        let s2 = s.clone();
        let gc =
            tokio::spawn(
                async move { reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE).await },
            );
        assert_eq!(arrived(&mut pre_rx).await, x_sha);
        // 사전조건: `pre_grave`는 rename **이전**이다 → 무덤은 아직 없고 정본이 살아 있다.
        assert!(tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());
        assert!(grave_names(&root).await.is_empty());

        // ⓒ *그제서야* putter — **완주까지 await**(핀 drop · 포인터 on-disk 확정, 보조정리 L)
        assert!(s
            .put("fresh", "v.bin", "text/plain", "u", x_content.clone())
            .await
            .is_ok());

        // ⓓ 해제 → ⓔ GC 완주
        gate.notify_one();
        let stats = finish_pass(gc).await;

        // **삭제 분기 자기검증** — 패스 시작 시 포인터가 **하나도 없다**(X의 포인터는 애초에 없다).
        // putter의 포인터가 스냅샷에 새면 **1**이 된다.
        assert_eq!(stats.referenced, 0);
        assert_eq!(stats.gc_deleted, 0);
        assert_eq!(*graved.lock().unwrap(), vec![x_sha.clone()]);

        let (_, got) = s
            .get_bytes("fresh", "v.bin")
            .await
            .expect("사전확인 ↔ 무덤 창에서 착지한 dedup put은 유실되면 안 된다");
        assert_eq!(got, x_content, "바이트까지 동일해야 한다");
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
    }

    // ── T-B4 ─────────────────────────────────────────────────────────────────────────────

    /// **T-B4 — 관측 후·커밋 전 park (코호트 대기 킬)** + **데드락 부재 sanity**.
    ///
    /// putter를 `post_observe`에서 park(intact=true · 핀 live · **미커밋**) → 무덤 시점 코호트 =
    /// {그 핀} → `settle()`이 **대기에 들어간다**(⓭ pending 프로브가 그것을 **관측**한다) →
    /// putter 해제 → 착지 → 핀 drop → settle 깨어남 → `landed ∋ sha` → **Restore 필수**.
    ///
    /// ⚠ ⓓ의 `graved_reached` await가 **M4를 죽이는 힘의 원천**이다: 더 일찍 해제하면 put이
    /// 무덤 **이전에** 착지해 `landed`가 서고, **M4 뮤턴트도 Restore로 살아남는다**.
    ///
    /// **뮤턴트 M4** — 코호트 대기 제거(`settle`이 즉시 `landed`만 본다): 판정 시점에 putter는
    /// 아직 park 중 → `landed` 비었음 → **Reap** → 해제된 putter가 dedup으로 커밋(바이트 재기록
    /// 없음) → **포인터 + blob 부재** → 404 → **RED**.
    /// **equivalent(정직하게)** — M4′(코호트를 `settle` **진입 시점**에 스냅샷)은 관측 동일(GREEN):
    /// 늦게 뜨면 **더 많이 기다릴 뿐** 안전 측이다. 무덤 **직후**로 고정하는 이유는 **성능**이지
    /// 안전이 아니다.
    #[tokio::test]
    async fn put_parked_after_observe_forces_cohort_wait_then_restore() {
        let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (obs_tx, mut obs_rx) = unbounded_channel::<String>();
        let (grv_tx, mut grv_rx) = unbounded_channel::<String>();
        let gate = Arc::new(Notify::new());

        let x_content = b"tb4-victim".to_vec();
        let x_sha = hex_sha(&x_content); // 훅에 sha 필터를 걸려면 먼저 알아야 한다

        let hooks = Hooks {
            // ⚠ **X의 put만** park한다 — 훅은 전역이므로 필터가 없으면 데드락 부재 sanity의
            //    "무관한 put"까지 park돼 버린다.
            post_observe: Some(async_park_for(x_sha.clone(), obs_tx, gate.clone())),
            post_grave: Some(grave_recorder_signal(graved.clone(), grv_tx)),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);
        atomic::write_atomic(&s.blob_path(&x_sha), &x_content)
            .await
            .unwrap();
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;

        // ⓐ put spawn → ⓑ **`observed` await**. 없으면 put이 아직 폴링되지 않아 **핀이 없을 수
        //    있고** → 무덤 시점 코호트가 **비어** → `Drained` → `landed` 없음 → **Reap** →
        //    **엉뚱한 이유로 RED**가 된다.
        let (s2, xc) = (s.clone(), x_content.clone());
        let put = tokio::spawn(async move { s2.put("b", "dedup", "text/plain", "u", xc).await });
        assert_eq!(arrived(&mut obs_rx).await, x_sha);
        assert!(!live_ids(&s, &x_sha).is_empty(), "핀은 살아 있다(커밋 이전)");

        // ⓒ GC spawn → ⓓ **`graved_reached` await**(코호트가 {그 핀}으로 **확정된 뒤**임을 못박는다)
        let s3 = s.clone();
        let mut gc =
            tokio::spawn(
                async move { reconcile::run_once_at_for_test(&s3, t0, GRACE, SETTLE).await },
            );
        assert_eq!(arrived(&mut grv_rx).await, x_sha);

        // ⓓ′ **대기 진입 프로브** — `settle()`은 코호트가 드레인될 때까지 **대기**해야 한다
        //     (대기 제거 뮤턴트는 여기서 완료돼 RED가 된다).
        probe_still_waiting(&mut gc).await;

        // **데드락 부재 sanity** — settle이 대기하는 동안에도 **다른 키의 put은 정상 완주**한다
        // (GC→put 단방향 대기 · put은 `pass_lock`을 잡지 않는다). spawn이 아니라 **완주 await**다.
        let other = timeout(
            Duration::from_secs(5),
            s.put("b", "unrelated", "text/plain", "u", b"tb4-other".to_vec()),
        )
        .await
        .expect("무관한 put이 settle 대기에 막히면 데드락이다");
        assert!(other.is_ok());

        // ⓔ putter 해제 → **완주 await**(핀 drop 확정 — 보조정리 L). `JoinError` 언랩 + `Ok` 단언.
        gate.notify_one();
        let r = timeout(Duration::from_secs(5), put)
            .await
            .expect("해제된 put은 완주한다")
            .expect("put 태스크는 패닉하지 않는다");
        assert!(r.is_ok());

        // ⓕ GC 완주 — 코호트가 드레인되면 settle이 깨어난다.
        let stats = finish_pass(gc).await;

        // **삭제 분기 자기검증** — putter는 `post_observe`(= 커밋 **이전**)에 park돼 있었으므로
        // `collect_referenced` 시점에 포인터가 디스크에 **없다**.
        assert_eq!(stats.referenced, 0);
        assert_eq!(stats.gc_deleted, 0);
        assert_eq!(*graved.lock().unwrap(), vec![x_sha.clone()]);

        let (_, got) = s
            .get_bytes("b", "dedup")
            .await
            .expect("코호트 드레인 후 landed → **Restore 필수**");
        assert_eq!(got, x_content);
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
    }

    // ── T-C2 ─────────────────────────────────────────────────────────────────────────────

    /// **T-C2 — 커밋 도중 호출자 취소**(crash 렌즈 FATAL의 결정적 증인).
    ///
    /// put을 spawn → `in_commit_pre_rename`(blocking 클로저 **내부** 동기 훅)에서 park →
    /// **바깥 퓨처를 abort**(= `upload_timeout`/disconnect 시뮬레이션) → **⚠ 그 취소가 *완료*될
    /// 때까지 await**하고 `JoinError::is_cancelled()`를 단언 → *그 다음에* GC 패스.
    /// 무덤 시점 코호트 = {그 핀} — 가드는 **클로저 소유**이므로 취소가 **완료된 뒤에도 살아
    /// 있다**(뮤턴트에서는 **죽어 있다**) → settle 대기 → 훅 해제 → 클로저가 rename·마킹·fsync·
    /// drop 완주 → `landed ∋ sha` → **Restore**.
    ///
    /// ⚠ **ⓑ와 ⓓ는 서로 다른 것을 증명한다 — 둘 다 없으면 이 테스트는 아무것도 봉인하지 못한다.**
    /// ⓑ = *"blocking 클로저가 **시작**됐다"*(도착 전에 abort하면 클로저가 시작조차 않을 수 있다).
    /// ⓓ = *"호출자 취소가 **완료**됐다"* — 없으면 caller-owned 뮤턴트에서 가드가 **아직 살아 있어**
    /// GC가 그것을 코호트로 잡고 **복원해 버린다** → **뮤턴트가 경합으로 생존**한다.
    /// ⚠ ⓓ는 blocking 클로저의 *종료*를 뜻하지 **않는다** — 그것은 **detach된 채** park에 살아 있다.
    /// **이 비대칭이 T-C2의 명제 그 자체다.**
    ///
    /// **teardown(정직하게)**: await할 핸들이 **구조적으로 없다** — abort가 커밋 클로저를 detach
    /// 시킨다(그것이 이 테스트의 명제다). **대리 관측**: GC의 Restore + 포인터 실재 + blob 존재가
    /// *"클로저가 rename·`landed` 삽입까지 완주했다"*를 증명한다. **잔여**: 착지 **이후**(fsync·핀
    /// drop)의 패닉만 미관측.
    #[tokio::test]
    async fn caller_cancellation_mid_commit_still_protects_the_blob() {
        let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (pre_tx, mut pre_rx) = unbounded_channel::<String>();
        let (grv_tx, mut grv_rx) = unbounded_channel::<String>();
        let (tx_a, rx_a) = std::sync::mpsc::channel::<()>();

        let hooks = Hooks {
            in_commit_pre_rename: Some(sync_park(pre_tx, rx_a)),
            post_grave: Some(grave_recorder_signal(graved.clone(), grv_tx)),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);

        let x_content = b"tc2-victim".to_vec();
        let x_sha = plant_orphan_blob(&s, &x_content).await;
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;

        // ⓐ put spawn → ⓑ **도착 await**. 확정 사실: dedup 관측 완료 ∧ stage 성공 ∧
        //    **blocking 클로저가 시작됐다** ∧ 핀 live ∧ `landed` 무흔적 ∧ 포인터 부재.
        let (s2, xc) = (s.clone(), x_content.clone());
        let mut put =
            tokio::spawn(async move { s2.put("b", "cancelled", "text/plain", "u", xc).await });
        assert_eq!(arrived(&mut pre_rx).await, x_sha);
        assert!(!landed_has(&s, &x_sha));
        assert!(!live_ids(&s, &x_sha).is_empty());

        // ⓒ abort → ⓓ **⚠ 취소 *완료*까지 await**(abort는 스케줄만 한다).
        //    `is_cancelled()` 단언은 **패닉 탐지기도 겸한다**(패닉이면 `is_panic()`이라 RED).
        put.abort();
        let e = timeout(Duration::from_secs(2), &mut put)
            .await
            .expect("abort는 완료되어야 한다 — 바깥 퓨처는 안쪽 blocking JoinHandle을 **드롭(detach)**하고 즉시 취소로 완료된다")
            .expect_err("바깥 퓨처는 취소된다");
        assert!(e.is_cancelled(), "abort → JoinError::is_cancelled");
        // ※ 그러나 blocking 클로저는 **살아 있다** — 지금도 park 중이며 핀을 쥐고 있다.

        // ⓔ *그제서야* GC spawn → ⓕ `graved_reached` await → ⓖ **대기 진입 프로브**
        let s3 = s.clone();
        let mut gc =
            tokio::spawn(
                async move { reconcile::run_once_at_for_test(&s3, t0, GRACE, SETTLE).await },
            );
        assert_eq!(arrived(&mut grv_rx).await, x_sha);
        // 취소된 put의 핀은 **여전히 살아 있다**(클로저 소유) → settle은 대기해야 한다
        // (caller-owned 뮤턴트에서는 여기가 완료돼 RED가 된다).
        probe_still_waiting(&mut gc).await;

        // ⓗ `park_A` 해제(sender drop = `Err(RecvError)` = 해제) → ⓘ GC 완주.
        //    ⚠ 해제 직후에는 **아무것도 단언하지 않는다**(함정 6: send 반환 ≠ 클로저 재개).
        //    **GC 완주가 그 관측이다** — 클로저가 완주하면 settle이 깨어난다.
        drop(tx_a);
        let stats = finish_pass(gc).await;

        // **삭제 분기 자기검증** — put은 `park_A`(= rename **이전**)에 있었으므로 포인터가 없다.
        assert_eq!(stats.referenced, 0);
        assert_eq!(stats.gc_deleted, 0);
        assert_eq!(*graved.lock().unwrap(), vec![x_sha.clone()]);

        let (_, got) = s
            .get_bytes("b", "cancelled")
            .await
            .expect("무취소 커밋은 **취소 이후에도 착지한다** → 복원 필수");
        assert_eq!(got, x_content);
        assert!(tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
    }

    // ── T-C3 ─────────────────────────────────────────────────────────────────────────────

    /// **T-C3 — 겹치는 실패 put의 결정적 증인**(`live`가 보호 술어가 **아님**을 못박는다).
    ///
    /// ⚠ **이 테스트가 가장 위험했다**: 2′의 도착 신호가 없으면 put이 폴링되기 전에 GC가 무덤을 파
    /// **빈 코호트**를 캡처 → `Drained` → `landed` 없음 → **Reap** → `gc_deleted == 1` — 이것은
    /// **6번이 기대하는 바로 그 값이다.** 즉 테스트는 **GREEN인데 시나리오는 한 번도 재현되지
    /// 않고**, 뮤턴트도 살아남는다. **이 도착 신호가 T-C3의 킬 파워 전부를 지탱한다.**
    ///
    /// **뮤턴트**(개정 전 복원 — `restore ⇔ live ∨ landed`, 코호트 대기 없음) → 판정 시점에 그 핀은
    /// **park된 채 live** → **Restore** → `gc_deleted == 0` → **RED**(4의 pending 단언도 함께 깨진다
    /// — 독립 RED 신호 2개).
    #[tokio::test]
    async fn overlapping_failed_put_does_not_protect_the_blob() {
        let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (pre_tx, mut pre_rx) = unbounded_channel::<String>();
        let (grv_tx, mut grv_rx) = unbounded_channel::<String>();
        let (tx_a, rx_a) = std::sync::mpsc::channel::<()>();

        let hooks = Hooks {
            in_commit_pre_rename: Some(sync_park(pre_tx, rx_a)),
            post_grave: Some(grave_recorder_signal(graved.clone(), grv_tx)),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);

        // 1) 만료·미참조 blob X
        let x_content = b"tc3-victim".to_vec();
        let x_sha = plant_orphan_blob(&s, &x_content).await;
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;
        // 커밋 rename을 **결정적으로 EISDIR 실패**시킨다(포인터 자리에 디렉터리 — T-C1과 같은 기법)
        tokio::fs::create_dir_all(root.join("b").join("poisoned.meta.json"))
            .await
            .unwrap();

        // 2) put spawn → 2′) **도착 await**(⚠ 없으면 조용히 GREEN)
        let (s2, xc) = (s.clone(), x_content.clone());
        let put = tokio::spawn(async move { s2.put("b", "poisoned", "text/plain", "u", xc).await });
        assert_eq!(arrived(&mut pre_rx).await, x_sha);
        // 확정 사실: **핀 live · 미착지 · 포인터 부재**
        assert!(!live_ids(&s, &x_sha).is_empty(), "핀은 살아 있다");
        assert!(!landed_has(&s, &x_sha), "아직 착지하지 않았다");

        // 3) GC spawn → 무덤 rename → 코호트 = {그 핀} → settle **대기**
        let s3 = s.clone();
        let mut gc =
            tokio::spawn(
                async move { reconcile::run_once_at_for_test(&s3, t0, GRACE, SETTLE).await },
            );
        // 4) **대기 진입의 증인** — 도착 신호를 await한 **뒤** pending 프로브.
        //    settle은 코호트(= 살아 있는 **실패** put의 핀)를 기다린다.
        assert_eq!(arrived(&mut grv_rx).await, x_sha);
        probe_still_waiting(&mut gc).await;

        // 5) 해제 → rename이 **EISDIR로 실패** → `on_landed`는 **절대 호출되지 않는다**
        drop(tx_a);
        // 5′) **put 완주 await** — 이것만이 *"핀 drop · 코호트 드레인 · landed 무흔적"*을 **관측**으로
        //     만든다(보조정리 L). `JoinError`를 언랩한다 → 패닉이면 **즉시 RED**(패닉으로 인한
        //     `landed` 무흔적을 EISDIR 때문이라고 **오독**하지 않는다).
        let r = timeout(Duration::from_secs(5), put)
            .await
            .expect("해제된 put은 완주한다")
            .expect("put 태스크는 패닉하지 않는다");
        assert!(
            matches!(r, Err(AppError::Internal(_))),
            "EISDIR → 커밋 rename 실패 → Err(Internal). `Ok`면 셋업이 깨진 것이고 시나리오가 재현되지 않았다"
        );
        assert!(!landed_has(&s, &x_sha), "rename이 Err → 흔적 0");

        // 6) settle이 깨어나 판정 → `landed(X) == false` → **Reap**
        let stats = finish_pass(gc).await;

        // **삭제 분기 자기검증** — 그 rename은 끝내 실패하므로 포인터는 **한 번도 존재하지 않는다**.
        assert_eq!(stats.referenced, 0);
        assert_eq!(*graved.lock().unwrap(), vec![x_sha.clone()]);
        // **주 단언**
        assert_eq!(
            stats.gc_deleted, 1,
            "실패한(겹치는) put은 blob을 보호하지 않는다 — `live`는 **대기 조건**이지 보호가 아니다"
        );
        assert!(!tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
        assert!(s.get_bytes("b", "poisoned").await.is_err(), "포인터 무흔적 → 404");
    }

    // ── T-P4a ────────────────────────────────────────────────────────────────────────────

    /// **T-P4a — 포인터 rename *이전*에 영원히 멈춘 핀**(P-4 봉인의 fail-CLOSED 증인).
    ///
    /// T-C3와 형제지만 정반대를 친다: T-C3의 핀은 **결국 죽는다**(EISDIR) → 결말 확정.
    /// **T-P4a의 핀은 죽지 않는다** → 결말이 **영원히 불명** → `settle_timeout` 소진 →
    /// **fail-CLOSED 복원** + tombstone 유지 + `gc_deleted` 무증가 + `tracing::error!`.
    ///
    /// **뮤턴트** ① 무한 대기(`await_settlement` → 코호트 드레인만 기다림) → 4단계의
    /// `timeout(5s, …)`가 `Err` → **패닉 = RED**(park를 절대 해제하지 않으므로 **탈출구가 없다**).
    /// ② fail-OPEN(타임아웃 시 Reap) → 무덤을 지운다 → 단언 ①이 **RED**.
    #[tokio::test]
    async fn stuck_pin_defers_reclamation_but_never_stalls_the_pass() {
        /// 관측 가능하게 짧은 예산. **주입형 인자**이므로 프로덕션 경로는 불변이다.
        const SETTLE_SHORT: Duration = Duration::from_millis(200);

        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let _sub = tracing::subscriber::set_default(CaptureSubscriber(logs.clone()));

        let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (pre_tx, mut pre_rx) = unbounded_channel::<String>();
        let (tx, rx) = std::sync::mpsc::channel::<()>(); // **본문이 도는 동안 절대 풀리지 않는다**

        let hooks = Hooks {
            in_commit_pre_rename: Some(sync_park(pre_tx, rx)),
            post_grave: Some(grave_recorder(graved.clone())),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);

        // 1) 만료·미참조 blob X
        let x_content = b"tp4a-victim".to_vec();
        let x_sha = plant_orphan_blob(&s, &x_content).await;
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;

        // 3) put spawn — **핸들을 보유한다**(`let _ =` 금지) → 3′) **도착 await**(spawn ≠ polled)
        let (s2, xc) = (s.clone(), x_content.clone());
        let put = tokio::spawn(async move { s2.put("b", "stuck", "text/plain", "u", xc).await });
        assert_eq!(arrived(&mut pre_rx).await, x_sha);

        // 4) 패스 1 — 코호트 = {영원히 멈춘 핀} → 200ms 소진 → **fail-CLOSED 복원**(`Deferred`)
        let p1 = timeout(
            Duration::from_secs(5),
            reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE_SHORT),
        )
        .await
        .expect("⚠ 멈춘 핀 하나가 패스를 **영구 정지**시키면 안 된다(무한 대기 뮤턴트가 여기서 죽는다)")
        .expect("패스는 Ok다");

        // 단언 ① (유실 0) — fail-OPEN 뮤턴트가 여기서 죽는다
        assert_eq!(
            tokio::fs::read(s.blob_path(&x_sha)).await.unwrap(),
            x_content,
            "결말을 모르면 **보존**한다(fail-CLOSED) — 무덤은 정본으로 되돌아간다"
        );
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
        // 단언 ② (무회수)
        assert_eq!(p1.gc_deleted, 0);
        // 단언 ②′ (삭제 분기 자기검증) — put은 rename **이전**에 영원히 park돼 있다 → 포인터 0
        assert_eq!(p1.referenced, 0);
        assert_eq!(*graved.lock().unwrap(), vec![x_sha.clone()]);
        // 단언 ⑤ (관측 가능한 에러) — 패스마다 정확히 1건
        assert_eq!(count_events(&logs, "ERROR", "gc settle timed out"), 1);

        // 단언 ③ (GC가 영구 정지하지 않는다 — `pass_lock`이 풀린다). 핀은 **아직도** park 중이다.
        let p2 = timeout(
            Duration::from_secs(5),
            reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE_SHORT),
        )
        .await
        .expect("후속 패스도 완주해야 한다 — 멈춘 핀은 **이후 패스를 막지 못한다**")
        .expect("패스는 Ok다");
        assert_eq!(p2.gc_deleted, 0);
        assert_eq!(p2.referenced, 0);
        assert!(tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());
        assert_eq!(count_of(&graved, &x_sha), 2, "매 패스가 무덤을 **다시** 판다");
        assert_eq!(count_events(&logs, "ERROR", "gc settle timed out"), 2);

        // 단언 ④ (격리 — **다른 blob은 오늘과 똑같이 회수된다**). Y: 핀 없는 만료·미참조 blob.
        let y_content = b"tp4a-bystander".to_vec();
        let y_sha = plant_orphan_blob(&s, &y_content).await;
        seed_expired_tombstones(&root, t0, &[&x_sha, &y_sha]).await;
        let p3 = timeout(
            Duration::from_secs(5),
            reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE_SHORT),
        )
        .await
        .expect("패스는 완주한다")
        .expect("패스는 Ok다");
        assert_eq!(
            p3.gc_deleted, 1,
            "멈춘 핀 **하나**가 다른 blob들의 회수를 막으면 안 된다 — 봉인의 목표는 **격리**다"
        );
        assert_eq!(p3.referenced, 0);
        assert!(!tokio::fs::try_exists(s.blob_path(&y_sha)).await.unwrap(), "Y는 회수됐다");
        assert!(tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap(), "X는 여전히 존재한다");
        assert_eq!(count_of(&graved, &x_sha), 3);
        assert_eq!(count_of(&graved, &y_sha), 1);
        assert_eq!(count_events(&logs, "ERROR", "gc settle timed out"), 3);

        // 단언 ⑤ 계속 — 필드(레벨 ERROR · sha · cohort_size=1 · waited_ms)
        let errs: Vec<String> = logs
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with("ERROR") && l.contains("gc settle timed out"))
            .cloned()
            .collect();
        assert!(
            errs.iter().all(|l| l.contains(&format!("sha={x_sha}"))
                && l.contains("cohort_size=1")
                && l.contains("waited_ms=")),
            "타임아웃 에러는 어느 sha·코호트 크기·대기 시간인지 지목해야 한다: {errs:?}"
        );

        // 5) **⚠ teardown**(함정 9 — park 이후에도 코드는 돈다). 단언 ①~⑤가 **전부 끝난 뒤**에만.
        //    먼저 해제하면 핀이 drop되고 포인터가 착지해 *"영원히 멈춘 핀"* 시나리오 자체가 사라진다.
        drop(tx); // ① **명시적** 해제
        let r = timeout(Duration::from_secs(5), put)
            .await
            .expect("put must finish after park release") // ② 유한 대기
            .expect("put task must not panic"); // ③ JoinError 언랩
        assert!(
            r.is_ok(),
            "④ 안쪽 결과까지 단언 — X의 정본은 fail-CLOSED 복원으로 디스크에 있고 \
             `b/stuck.meta.json` 자리는 비어 있다 ⇒ 재개된 rename은 **성공**한다"
        );
    }

    // ── T-P4b-1 ──────────────────────────────────────────────────────────────────────────

    /// **T-P4b-1 — 무덤 시점에 `landed`가 이미 true(핀은 live) → 대기 0 · 즉시 복원.**
    ///
    /// `settle_timeout = 30s`가 이 테스트의 **핵심 장치**다: 픽스는 그 30초를 **한 번도 건드리지
    /// 않아야** 한다. 코호트는 **드레인되지 않은 채**(put이 `in_commit_post_landed`에 갇혀 있다)
    /// 복원이 일어나야 한다 — 더 기다리는 것은 **순손해**다(그 창 내내 **실재하는 포인터가 404**).
    ///
    /// **뮤턴트** ① `await_settlement`의 **검사 ①**(landed 즉시복원) 삭제 → 코호트 = {park된 핀}
    /// → 영영 드레인되지 않는다 → **30s 예산을 전부 태운다** → 단언 ⑤가 `Err`(RED) ∧ 단언 ④의 두
    /// 문자열이 정확히 **뒤바뀜**(RED). ② `landed` 삽입 삭제 → 같은 경로로 `TimedOut` → RED ×2.
    ///
    /// ⚠ **정직하게**: `notify_waiters()` 제거 뮤턴트는 **여기서 GREEN이다**(settle이 **첫
    /// 검사에서** landed를 본다 — 깨울 필요가 없다). **그것이 T-P4b-2가 존재하는 이유다.**
    #[tokio::test]
    async fn already_landed_at_grave_time_restores_without_waiting_for_the_cohort() {
        const SETTLE_LONG: Duration = Duration::from_secs(30);

        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let _sub = tracing::subscriber::set_default(CaptureSubscriber(logs.clone()));

        let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (gc_tx, mut gc_rx) = unbounded_channel::<String>();
        let (landed_tx, mut landed_rx) = unbounded_channel::<String>();
        let (tx_put, rx_put) = std::sync::mpsc::channel::<()>();
        let gate = Arc::new(Notify::new());

        let hooks = Hooks {
            pre_grave: Some(async_park(gc_tx, gate.clone())),
            in_commit_post_landed: Some(sync_park(landed_tx, rx_put)),
            post_grave: Some(grave_recorder(graved.clone())),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);

        // 1) 만료·미참조 blob X. **포인터는 0개다.**
        let x_content = b"tp4b1-victim".to_vec();
        let x_sha = plant_orphan_blob(&s, &x_content).await;
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;
        let pointer = root.join("b").join("landed_then_stuck.meta.json");

        // 3) **reconcile을 먼저 spawn** → `collect_referenced`(포인터 0개) → `pre_grave`에서 park
        let s2 = s.clone();
        let gc = tokio::spawn(async move {
            reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE_LONG).await
        });
        assert_eq!(arrived(&mut gc_rx).await, x_sha);
        // **사전조건** ⇒ `collect_referenced`는 포인터를 볼 수 **없었다**(참조됨 분기 누수 구조적 배제)
        assert!(tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap(), "무덤은 아직 없다");
        assert!(!tokio::fs::try_exists(&pointer).await.unwrap(), "포인터는 아직 없다");

        // 4) **그 park 동안** put spawn(핸들 **보유**) → dedup → rename **Ok** → `landed` 삽입 +
        //    `notify_waiters()`(⚠ 대기자 **0명** — settle은 아직 시작조차 안 했다) →
        //    `in_commit_post_landed`에서 park(fsync 직전 · **핀은 여전히 live**)
        let (s3, xc) = (s.clone(), x_content.clone());
        let put = tokio::spawn(async move {
            s3.put("b", "landed_then_stuck", "text/plain", "u", xc).await
        });
        assert_eq!(arrived(&mut landed_rx).await, x_sha);
        assert!(landed_has(&s, &x_sha), "커밋 rename이 Ok → 착지 흔적");
        assert!(!live_ids(&s, &x_sha).is_empty(), "핀은 **여전히 live**(클로저 소유)");
        assert!(tokio::fs::try_exists(&pointer).await.unwrap(), "포인터가 VFS에 실재한다");

        // 5) `gc_park` 해제 → `grave()` → 코호트 = {**살아 있는** 핀} → `settle()`의 **첫 검사**에서
        //    `landed ∋ sha` → **await 0회 · 즉시 복원**
        gate.notify_one();
        // 6) **단언 ⑤(시간 기반, 보조)** — `finish_pass_promptly`의 2초 창은 5단계 **이후에만**
        //    돈다 → settle 구간만 잰다. 예산 30s의 **1/15**다.
        let stats = finish_pass_promptly(gc).await;

        // 단언 ① (삭제 분기 자기검증) — 포인터는 **3단계 이후에** 착지했으므로 스냅샷에 없다
        assert_eq!(stats.referenced, 0);
        assert_eq!(*graved.lock().unwrap(), vec![x_sha.clone()]);
        // 단언 ② (핀이 live인데도 즉시 복원 — **이 테스트의 요지**)
        assert!(
            !live_ids(&s, &x_sha).is_empty(),
            "단언 시점에 put은 여전히 park돼 있다 ⇒ **코호트는 드레인되지 않았다**"
        );
        let (_, got) = s.get_bytes("b", "landed_then_stuck").await.expect("즉시 복원");
        assert_eq!(got, x_content);
        assert!(tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
        // 단언 ③ (무회수)
        assert_eq!(stats.gc_deleted, 0);
        // 단언 ④ (시간 무관, **주 단언**) — 타임아웃을 **안 태웠다**
        assert_eq!(count_events(&logs, "INFO", "GC restored: landed commit"), 1);
        assert_eq!(count_events(&logs, "ERROR", "gc settle timed out"), 0);

        // 7) **⚠ teardown** — 반드시 단언 이후. 먼저 해제하면 핀이 drop돼 코호트가 드레인되고,
        //    *"핀이 live인데도 즉시 복원됐다"*(단언 ②)는 **이 테스트의 요지가 사라진다.**
        drop(tx_put);
        let r = timeout(Duration::from_secs(5), put)
            .await
            .expect("put must finish after park release")
            .expect("put task must not panic");
        assert!(r.is_ok());
    }

    // ── T-P4b-2 ──────────────────────────────────────────────────────────────────────────

    /// **T-P4b-2 — 대기 *도중*에 착지 → `landed` 삽입의 `notify_waiters()`가 대기를 깨운다.**
    ///
    /// **핵심 장치**: `park_B`(`in_commit_post_landed`)가 **핀을 착지 이후에도 살려 둠**으로써
    /// **`PinGuard::drop`의 알림이라는 대체 기상 수단을 제거한다.** ⇒ settlement를 깨울 수 있는
    /// 것은 **`landed` 삽입의 `notify_waiters()` 하나뿐**이다(그 외에는 30s 타임아웃뿐).
    ///
    /// **뮤턴트** ① `notify_waiters()` 제거 → settlement는 6단계 **이전에 이미** `notified`에
    /// park했다(단언 ②가 그것을 못박는다) → 깨울 것이 **아무것도 없다** → **30s 예산 소진** →
    /// 단언 ⑤ `Err`(RED) ∧ 단언 ④의 두 문자열이 뒤바뀜(RED). ② 코호트 대기 제거 → 판정 시
    /// `landed` 비었음 → **Reap** → 해제된 put이 dedup으로 착지 → 404 → 단언 ③ RED.
    ///
    /// ⚠ **정직하게 — `notify_waiters()` 제거는 안전성 결함이 아니라 지연(latency) 결함이다.**
    /// 알림이 없어도 settlement는 **결국** 깨어나고 **어느 쪽이든 복원한다**(`Landed` 또는
    /// fail-CLOSED `Deferred` — **디스크 전이가 같다**) ⇒ 유실 0 · 판정 동일. 바뀌는 것은
    /// **실재하는 포인터가 404를 내는 창의 길이**뿐이다. **이 증인은 그 창을 관측 가능하게 만든다.**
    #[tokio::test]
    async fn landing_during_settle_wait_is_woken_by_the_landed_notification() {
        const SETTLE_LONG: Duration = Duration::from_secs(30);

        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let _sub = tracing::subscriber::set_default(CaptureSubscriber(logs.clone()));

        let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (gc_tx, mut gc_rx) = unbounded_channel::<String>();
        let (pre_tx, mut pre_rx) = unbounded_channel::<String>();
        let (post_tx, mut post_rx) = unbounded_channel::<String>();
        let (grv_tx, mut grv_rx) = unbounded_channel::<String>();
        let (tx_a, rx_a) = std::sync::mpsc::channel::<()>(); // 6단계에서 해제
        let (tx_b, rx_b) = std::sync::mpsc::channel::<()>(); // **teardown에서만** 해제
        let gate = Arc::new(Notify::new());

        // **전부 기존 훅이다** — `Hooks` 필드 7개 불변 · 프로덕션 훅 0개 추가.
        let hooks = Hooks {
            pre_grave: Some(async_park(gc_tx, gate.clone())),
            in_commit_pre_rename: Some(sync_park(pre_tx, rx_a)),
            in_commit_post_landed: Some(sync_park(post_tx, rx_b)),
            post_grave: Some(grave_recorder_signal(graved.clone(), grv_tx)),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);

        // 1) 만료·미참조 blob X. **포인터는 0개다.**
        let x_content = b"tp4b2-victim".to_vec();
        let x_sha = plant_orphan_blob(&s, &x_content).await;
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;
        let pointer = root.join("b").join("settle_wakeup.meta.json");

        // 3) **reconcile을 먼저 spawn** → `pre_grave`에서 park → **`gc_arrived` await**
        let s2 = s.clone();
        let mut gc = tokio::spawn(async move {
            reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE_LONG).await
        });
        assert_eq!(arrived(&mut gc_rx).await, x_sha);
        assert!(tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap(), "무덤은 **아직 없다**");
        assert!(!tokio::fs::try_exists(&pointer).await.unwrap(), "포인터는 **아직 없다**");

        // 4) **그 park 동안** put spawn(핸들 **보유**) → dedup 관측 → stage → `park_A`(rename 직전)
        //    **⇒ `pre_rename_reached` await — ⚠ 이 await가 봉인 그 자체다.**
        let (s3, xc) = (s.clone(), x_content.clone());
        let put =
            tokio::spawn(async move { s3.put("b", "settle_wakeup", "text/plain", "u", xc).await });
        assert_eq!(arrived(&mut pre_rx).await, x_sha);
        // **확정 사실**: 핀 live ∧ 미착지 ∧ 포인터 부재 — 5단계 논증의 두 전제가 여기서 선다.
        assert!(!live_ids(&s, &x_sha).is_empty());
        assert!(!landed_has(&s, &x_sha));

        // 5) `gc_park` 해제 → `grave()` → 코호트 = {그 핀} → `settle()` → 검사 ① landed **false** ·
        //    검사 ② 코호트 **미드레인** → **`notified.await`로 진입한다(= 대기 중)**
        gate.notify_one();
        assert_eq!(arrived(&mut grv_rx).await, x_sha);
        // **단언 ② (대기 진입)** — `await_settlement`의 루프 몸통은 **동기**다(Mutex 검사뿐).
        // 유일한 await 지점이 `timeout_at(deadline, notified)`이고, 이 순간 세 종료 조건이 **전부
        // 거짓**이다(landed 비었음 ∵ put은 `park_A` · 코호트 살아 있음 ∵ 4단계의 도착 신호 · 30s
        // 예산 남음) ⇒ **200ms 동안 반환하지 않았다는 사실 자체가 "settle이 그 await에 있다"**는 뜻.
        probe_still_waiting(&mut gc).await;

        // 6) **그제서야** `park_A` 해제 → rename **Ok** → `landed` 삽입 + `notify_waiters()` →
        //    `park_B`에서 park — ⚠ **핀은 drop되지 않는다**(대체 기상 수단 제거).
        drop(tx_a);
        // **⇒ `post_landed_reached` await** — *"착지했고, 핀은 아직 살아 있으며, 그 상태로 갇혔다"*가
        //    **논증이 아니라 관측**이 된다(함정 6: 해제 send의 반환은 재개가 아니다).
        assert_eq!(arrived(&mut post_rx).await, x_sha);
        assert!(landed_has(&s, &x_sha), "rename Ok → 착지 흔적");
        assert!(!live_ids(&s, &x_sha).is_empty(), "핀은 **착지 이후에도** 살아 있다");

        // 7) settlement가 **깨어나** 검사 ①에서 `landed ∋ sha` → 즉시 복원.
        //    **단언 ⑤(시간 기반, 보조)** — `finish_pass_promptly`의 2초 창은 6단계 **이후에만** 돈다.
        //    깨울 수 있는 것은 `landed` 삽입의 `notify_waiters()` **하나뿐**이다(핀은 살아 있다).
        let stats = finish_pass_promptly(gc).await;

        // 단언 ① (삭제 분기 자기검증)
        assert_eq!(stats.referenced, 0);
        assert_eq!(*graved.lock().unwrap(), vec![x_sha.clone()]);
        // 단언 ③ (핀이 **아직도** live인 채로 복원됐다)
        assert!(!live_ids(&s, &x_sha).is_empty(), "코호트는 **여전히** 드레인되지 않았다");
        let (_, got) = s.get_bytes("b", "settle_wakeup").await.expect("복원 필수");
        assert_eq!(got, x_content);
        assert!(tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
        assert_eq!(stats.gc_deleted, 0);
        // 단언 ④ (시간 무관, **주 단언**)
        assert_eq!(count_events(&logs, "INFO", "GC restored: landed commit"), 1);
        assert_eq!(count_events(&logs, "ERROR", "gc settle timed out"), 0);

        // 8) **⚠ teardown** — 반드시 단언 이후. 먼저 해제하면 코호트가 드레인되어 **대체 기상 수단이
        //    되살아나고**, `notify_waiters()` 제거 뮤턴트가 **살아남는다**.
        drop(tx_b); // `tx_a`는 6단계에서 이미 해제됐다
        let r = timeout(Duration::from_secs(5), put)
            .await
            .expect("put must finish after park release")
            .expect("put task must not panic");
        assert!(r.is_ok());
    }

    // ── T-B5 ① 취소 ──────────────────────────────────────────────────────────────────────

    /// **T-B5 ① — 패스가 무덤과 `settle()` 사이에서 취소된다** → 무덤 잔존 → 다음 패스가 복구.
    ///
    /// ⚠ **랑데부(도착)**: 도착을 기다리지 않고 abort하면 **무덤이 아직 안 파여 있다** →
    /// `.gc-grave-<sha>`가 0개 → **엉뚱한 이유로 RED**(`recover_graves` 삭제 뮤턴트도 살아남는다).
    /// ⚠ **랑데부(취소 완료)**: `abort()`는 스케줄만 한다. 곧바로 새 `run_once`를 시작하면 아직
    /// 드롭되지 않은 `PassGuard`가 **`pass_lock`을 쥐고 있어** 새 패스가 `lock_owned()`에서 막힌다.
    /// ⇒ **취소 완료 = `PassGuard` drop = `pass_lock` 해제**가 **관측**이 되어야 한다.
    #[tokio::test]
    async fn pass_cancelled_after_grave_leaves_it_for_the_next_pass_to_recover() {
        let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (grv_tx, mut grv_rx) = unbounded_channel::<String>();
        let gate = Arc::new(Notify::new()); // **해제하지 않는다** — abort가 곧 해제다

        let hooks = Hooks {
            post_grave: Some(grave_recorder_park(graved.clone(), grv_tx, gate)),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);

        let x_content = b"tb5-cancel".to_vec();
        let x_sha = plant_orphan_blob(&s, &x_content).await;
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;

        let s2 = s.clone();
        let mut gc =
            tokio::spawn(
                async move { reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE).await },
            );
        // **도착 await** — 무덤이 **실제로 파인 뒤**임을 못박는다.
        assert_eq!(arrived(&mut grv_rx).await, x_sha);

        // abort → **⚠ 취소 완료 await**(없으면 새 `run_once`가 `pass_lock`에서 hang한다)
        gc.abort();
        let e = timeout(Duration::from_secs(2), &mut gc)
            .await
            .expect("abort must complete")
            .expect_err("pass must be cancelled");
        assert!(e.is_cancelled());
        // (함정 3 확인: abort 시점에 in-flight `spawn_blocking`은 **없다** — `grave()`의 rename은
        //  `post_grave` **이전에** 이미 반환했다.)

        // 무덤이 **정확히 1개** ∧ 정본 **부재** ∧ `graved` 관측
        assert_eq!(grave_names(&root).await, vec![expected_grave_name(&x_sha)]);
        assert!(!tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());
        assert_eq!(*graved.lock().unwrap(), vec![x_sha.clone()]);

        // 새 패스 → `PassGuard::begin`의 `recover_graves`가 무덤을 정본으로 되돌린다.
        // `now`를 만료 **이전**으로 되돌려 **복원 그 자체를 직접 관측**한다(간접 추론 금지).
        let stats = timeout(
            Duration::from_secs(5),
            reconcile::run_once_at_for_test(&s, before_expiry(t0), GRACE, SETTLE),
        )
        .await
        .expect("복구 패스는 유한 시간에 끝난다(취소 완료를 안 기다리면 여기서 `pass_lock`에 막힌다)")
        .expect("패스는 Ok다");

        assert_eq!(stats.gc_deleted, 0);
        assert_eq!(stats.referenced, 0);
        assert_eq!(
            tokio::fs::read(s.blob_path(&x_sha)).await.unwrap(),
            x_content,
            "`recover_graves`가 무덤을 정본으로 되돌린다 — 유실 0"
        );
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
    }

    // ── T-B5 ② 크래시/재시작 ─────────────────────────────────────────────────────────────

    /// **T-B5 ② — 크래시/재시작**: 무덤이 심어진 root에 **새 `Store`**를 만들어 `run_once`.
    ///
    /// **함정 4 ("확인했고 없음")**: `drop(store)`는 **디스크에 아무 효과도 없다**
    /// (`PassGuard::drop`은 디스크 무접촉). ②는 그 드롭의 효과에 **의존하지 않는다** — 전제는
    /// **디스크에 놓인 무덤**뿐이고, 재시작 시뮬레이션의 동력은 **새 `Store::new`가 새(빈) 핀
    /// 등록부를 만든다**는 사실이다(**D-3의 해저드를 의도적으로 쓴다**).
    #[tokio::test]
    async fn grave_planted_by_a_crashed_process_is_recovered_on_restart() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let content = b"tb5-restart".to_vec();
        let sha = hex_sha(&content);

        // 크래시 잔재를 **디스크에 심는다**: 무덤만 있고 정본은 **없다**. 포인터는 **살아 있다**.
        {
            let dead = Store::new(root.clone());
            atomic::write_atomic(&dead.layout().grave_path(&sha), &content)
                .await
                .unwrap();
            let meta = crate::meta::ObjectMeta {
                content_type: "text/plain".into(),
                size: content.len() as u64,
                sha256: sha.clone(),
                created_at: "2026-01-01T00:00:00Z".into(),
                uploaded_by: "u".into(),
            };
            atomic::write_atomic(
                &root.join("b").join("k.meta.json"),
                &serde_json::to_vec(&meta).unwrap(),
            )
            .await
            .unwrap();
            drop(dead); // 디스크 무접촉 — 전제는 무덤뿐이다
        }

        // **새 `Store`**(빈 핀 등록부) → run_once
        let s = Store::new(root.clone());
        let stats = timeout(
            Duration::from_secs(5),
            reconcile::run_once(&s, GRACE, SETTLE),
        )
        .await
        .expect("복구 패스는 유한 시간에 끝난다")
        .expect("패스는 Ok다");

        assert_eq!(stats.referenced, 1, "복구된 정본은 포인터로 참조된다");
        assert_eq!(stats.gc_deleted, 0);
        let (_, got) = s.get_bytes("b", "k").await.expect("무덤이 복원되어 서빙 가능해야 한다");
        assert_eq!(got, content);
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
    }

    // ── T-B5 ③ 복원 실패 ─────────────────────────────────────────────────────────────────

    /// **T-B5 ③ — 복원 `rename`이 실패한다**(EIO 주입) → `io::Error` **무가공 전파**(B7) ∧
    /// 무덤 **잔존** ∧ **unlink 0회** → 다음 패스가 복구 → **유실 0**.
    ///
    /// **spawn 0 · park 0** — 순차 await만으로 보호 상태(`landed`)를 만든다: `PassGuard`를 손에 든
    /// 채 dedup put을 **완주**시키면(보조정리 L: 완주 = 핀 사망 + 흔적 확정) `landed ∋ sha`가 서고
    /// 코호트는 비어 있다 → `settle()`이 **첫 검사에서 `Landed`** → 복원 분기 → 주입된 EIO.
    #[tokio::test]
    async fn restore_failure_keeps_the_grave_and_never_unlinks_it() {
        let injected = Arc::new(AtomicUsize::new(0));
        let inj = injected.clone();
        let hooks = Hooks {
            restore_io: Some(Arc::new(move |_sha: &str| {
                inj.fetch_add(1, Ordering::SeqCst);
                Err(std::io::Error::other("injected EIO"))
            })),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);

        let x_content = b"tb5-restore-fail".to_vec();
        let x_sha = plant_orphan_blob(&s, &x_content).await;
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;

        let pass = PassGuard::begin(&s, SETTLE).await.expect("begin");
        // **삭제 분기 자기검증**(stats가 없는 경로 → `referenced()`로 같은 규율을 건다)
        assert!(pass.referenced().is_empty(), "포인터가 하나도 없다");

        // 패스 **안에서** dedup put이 완주한다 → `landed ∋ x_sha` ∧ 핀은 **이미 죽었다**
        s.put("b", "k", "text/plain", "u", x_content.clone())
            .await
            .expect("dedup put");
        assert!(landed_has(&s, &x_sha));

        // ⚠ `grave()`를 **await**한다 — `Graved`는 rename이 성공했을 때만 태어난다.
        let g = pass.grave(&x_sha).await.expect("grave rename must succeed");
        // `Settled`는 `Debug`를 유도하지 않는다(프로덕션 타입 diff 0) → `match`로 언랩한다.
        let e = match g.settle().await {
            Ok(_) => panic!("주입된 EIO는 **무가공**으로 전파되어야 한다(합성 금지 — B7)"),
            Err(e) => e,
        };
        assert_eq!(e.kind(), std::io::ErrorKind::Other);
        assert_eq!(injected.load(Ordering::SeqCst), 1);

        // 무덤 **잔존** ∧ 정본 **부재** = **unlink 0회**(파괴 연산은 판정 이후에만 · 여기선 미실행)
        assert_eq!(grave_names(&root).await, vec![expected_grave_name(&x_sha)]);
        assert!(!tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());
        drop(pass); // 명시적 — `pass_lock`을 쥔 채면 다음 패스가 hang한다(함정 4)

        // 다음 패스가 복구한다. `restore_io`는 `settle()`에만 있고, 복구된 정본은 **참조됨**이므로
        // 삭제 분기에 들어가지 않는다 → 주입 카운터는 **1로 고정**된다.
        let stats = timeout(
            Duration::from_secs(5),
            reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE),
        )
        .await
        .expect("복구 패스는 유한 시간에 끝난다")
        .expect("패스는 Ok다");
        assert_eq!(stats.gc_deleted, 0);
        assert_eq!(stats.referenced, 1);
        assert_eq!(injected.load(Ordering::SeqCst), 1, "복구 패스는 settle에 도달하지 않는다");
        let (_, got) = s.get_bytes("b", "k").await.expect("다음 패스가 복구한다 — 유실 0");
        assert_eq!(got, x_content);
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
    }

    // ── T-B5 ③′ 복원 실패 — **reconcile 레벨** ────────────────────────────────────────────

    /// **T-B5 ③′ — 복원 `rename`이 실패할 때 `run_once`가 `Err`를 반환한다**(B7의 **reconcile 레벨**
    /// 증인).
    ///
    /// T-B5③은 `PassGuard::begin` + `grave()` + `settle()`를 **직접** 불러 그 자리에서 `Err`를
    /// 단언한다 — `settle()`이 `io::Error`를 **낳는다**는 것은 못박지만, **`reconcile.rs`가 그것을
    /// 삼키지 않는다**는 것은 못박지 못한다. 그 절반이 무방비였다:
    /// `run_once_at`의 `.settle().await?`를 **`Err(_) => continue`로 바꾸는 뮤턴트가 전 스위트를
    /// 통과했다.** 이 증인이 그 구멍을 막는다.
    ///
    /// 명세 §2의 계약: *"이 개정은 `io::Error`를 **하나도 새로 만들지 않고 하나도 삼키지 않는다**."*
    /// ⇒ **주입된 EIO는 `run_once`의 반환값까지 무가공으로 올라와야 한다**(`ErrorKind`·메시지 보존).
    ///
    /// 안무(랑데부): ⓐ GC spawn → ⓑ `pre_grave` **도착 await**(패스가 `begin()`을 지나 **삭제
    /// 분기 안**에 있음이 확정 — 여기서 안 기다리면 putter의 포인터가 참조 스냅샷에 샌다) →
    /// ⓒ **그 park 동안** dedup put을 **완주까지 await**(보조정리 L: 완주 = `landed ∋ sha` ∧ 핀
    /// 사망 ⇒ 코호트는 빈다 → `settle()`이 **첫 검사에서 `Landed`**) → ⓓ 해제 → ⓔ **`run_once`가
    /// `Err`**.
    ///
    /// **뮤턴트** ① `.settle().await?` → `Err(_) => continue`(에러 삼킴) → `run_once`가 **Ok**를
    /// 반환한다 → ⓔ의 `finish_pass_err`가 **RED**. ② EIO를 합성/래핑(`ErrorKind` 변조) →
    /// `kind()`·메시지 단언이 **RED**. ③ 판정 **이전에** 파괴 연산(unlink)을 옮김 → 무덤이 사라져
    /// 무덤 잔존 단언이 **RED**.
    ///
    /// **§삭제 분기 자기검증**: `run_once`가 `Err`라 `stats`가 **없다** ⇒ `post_grave`가 수집한
    /// `graved == vec![x_sha]`가 *"삭제 분기에 실제로 들어갔다"*의 직접 증거다(`grave()`는 blob→무덤
    /// rename이 **성공한 뒤에만** 이 훅을 부른다). 복구 패스의 `referenced == 1`이 그것을 보강한다.
    #[tokio::test]
    async fn restore_failure_makes_the_reconcile_pass_return_the_raw_io_error() {
        let injected = Arc::new(AtomicUsize::new(0));
        let inj = injected.clone();
        let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (gc_tx, mut gc_rx) = unbounded_channel::<String>();
        let gate = Arc::new(Notify::new());

        let hooks = Hooks {
            pre_grave: Some(async_park(gc_tx, gate.clone())),
            post_grave: Some(grave_recorder(graved.clone())),
            restore_io: Some(Arc::new(move |_sha: &str| {
                inj.fetch_add(1, Ordering::SeqCst);
                Err(std::io::Error::other("injected EIO"))
            })),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);

        // 1) 만료·미참조 blob X. **포인터는 0개다.**
        let x_content = b"tb5-reconcile-restore-fail".to_vec();
        let x_sha = plant_orphan_blob(&s, &x_content).await;
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;

        // ⓐ GC spawn → ⓑ **`pre_grave` 도착 await**
        let s2 = s.clone();
        let gc =
            tokio::spawn(
                async move { reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE).await },
            );
        assert_eq!(arrived(&mut gc_rx).await, x_sha);
        // 사전조건: `pre_grave`는 rename **이전**이다 → 무덤은 아직 없고 정본이 살아 있다.
        assert!(tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());
        assert!(grave_names(&root).await.is_empty());

        // ⓒ **그 park 동안** dedup put이 **완주**한다 → `landed ∋ x_sha` ∧ 핀은 **이미 죽었다**
        //    ⇒ 무덤 시점 코호트가 비어 `settle()`은 **첫 검사에서 `Landed`** → **복원 분기**로 간다.
        s.put("b", "k", "text/plain", "u", x_content.clone())
            .await
            .expect("dedup put");
        assert!(landed_has(&s, &x_sha));

        // ⓓ 해제 → `grave()` → `settle()` → 복원 분기 → **주입된 EIO**
        gate.notify_one();

        // ⓔ **주 단언**: `run_once`가 **`Err`**다 — `reconcile.rs`는 `io::Error`를 **삼키지 않는다**.
        let e = finish_pass_err(gc).await;
        assert_eq!(
            e.kind(),
            std::io::ErrorKind::Other,
            "주입된 `io::Error`는 **무가공**으로 전파되어야 한다(합성·래핑 금지 — B7)"
        );
        assert!(
            e.to_string().contains("injected EIO"),
            "메시지까지 무가공이어야 한다 — 잡힌 에러: {e}"
        );
        assert_eq!(injected.load(Ordering::SeqCst), 1);

        // **삭제 분기 자기검증** — `Err`라 `stats`가 없다 ⇒ `post_grave`가 그 증거다.
        assert_eq!(*graved.lock().unwrap(), vec![x_sha.clone()]);

        // **무덤 잔존 ∧ 정본 부재 = `remove_file` 0회.** 파괴 연산은 판정 **이후에만** 일어나고,
        // 이 경로(복원)에서는 **아예 실행되지 않는다**.
        assert_eq!(grave_names(&root).await, vec![expected_grave_name(&x_sha)]);
        assert!(!tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());

        // 다음 패스가 복구한다 → **유실 0**. 복구된 정본은 (ⓒ의 put이 남긴 포인터로) **참조됨**이므로
        // 삭제 분기에 들어가지 않는다 → `settle` 미도달 → 주입 카운터는 **1로 고정**된다.
        let stats = timeout(
            Duration::from_secs(5),
            reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE),
        )
        .await
        .expect("복구 패스는 유한 시간에 끝난다")
        .expect("패스는 Ok다");
        assert_eq!(stats.referenced, 1, "ⓒ의 put이 남긴 포인터 하나");
        assert_eq!(stats.gc_deleted, 0);
        assert_eq!(injected.load(Ordering::SeqCst), 1, "복구 패스는 settle에 도달하지 않는다");
        let (_, got) = s.get_bytes("b", "k").await.expect("다음 패스가 복구한다 — 유실 0");
        assert_eq!(got, x_content);
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
    }

    // ── T-B5 ④ `Graved` 누수 ─────────────────────────────────────────────────────────────

    /// **T-B5 ④ — `Graved`를 흘린다(fail-CLOSED by construction).**
    ///
    /// ⚠ **`let _ = pass.grave(..)`는 *아무 일도 하지 않는다*** — `grave`는 `async fn`이므로
    /// **폴링되지 않은 퓨처를 드롭**할 뿐이고 **blob→무덤 rename이 아예 일어나지 않는다**
    /// (`#[must_use]`조차 `let _ =`가 삼킨다 — **컴파일러는 침묵한다**). 그러면 다음 패스는 **원래의
    /// 멀쩡한 blob**을 발견하고, **`recover_graves`가 통째로 깨져 있어도 테스트가 GREEN이다.**
    /// ⇒ **3·4단계가 이 증인의 킬 파워 전부다.**
    ///
    /// **뮤턴트** ① `recover_graves` 삭제 → 복구 패스가 무덤을 되돌리지 못한다 → blob 부재 ∧ 무덤
    /// 잔존 → **RED ×2**. ② `Graved`에 파괴적 Drop 추가(= fail-OPEN) → **5단계 재확인 단언 RED**
    /// (무덤이 사라졌다) ∧ 복구할 것이 없어 **영구 유실**. ③ rename 없이 `Graved`를 낳는다 →
    /// **4단계 RED**(무덤 0개 ∧ blob 존재 ∧ `graved`가 비어 있다).
    #[tokio::test]
    async fn leaked_graved_token_leaves_a_grave_that_the_next_pass_recovers() {
        let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let hooks = Hooks {
            post_grave: Some(grave_recorder(graved.clone())),
            ..Hooks::default()
        };
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let s = Store::with_hooks(root.clone(), hooks);

        // 1) 만료·미참조 blob X. **동시 put 0 · spawn 0 · park 0.**
        let x_content = b"tb5-leak".to_vec();
        let x_sha = plant_orphan_blob(&s, &x_content).await;
        let t0 = SystemTime::now();
        seed_expired_tombstones(&root, t0, &[&x_sha]).await;

        // 2) 패스 등록
        let pass = PassGuard::begin(&s, SETTLE).await.expect("begin");
        assert!(pass.referenced().is_empty(), "포인터가 하나도 없다");

        // 3) ⚠ **`grave()`를 await한다**(§5.1-9 · 함정 10)
        let g = pass.grave(&x_sha).await.expect("grave rename must succeed");

        // 4) ⚠ **복구 *이전* 디스크 상태를 단언한다** — 이 네 줄이 P-8이 없앴던 바로 그 관측이다.
        //    개정 전에는 넷 다 **거짓**이었고(무덤 0개 · blob 존재) **아무도 그것을 묻지 않았다.**
        let names = grave_names(&root).await;
        assert_eq!(names, vec![expected_grave_name(&x_sha)]);
        // ⚠ **온디스크 바이트의 raw 핀**(ADR-0001). 위 `assert_eq!`의 기대값도 raw지만, 이 저장소의
        //    무덤 접두사는 **여기서 문자 그대로** 못박힌다 — `GRAVE_PREFIX`를 `.tomb-`로 바꾸는
        //    드리프트는 layout을 경유하는 어떤 단언으로도 잡히지 않는다(동어반복).
        assert!(
            names[0].starts_with(".gc-grave-"),
            "무덤의 온디스크 접두사는 `.gc-grave-`다 — 잡힌 이름: {names:?}"
        );
        assert!(names[0].contains(&x_sha), "무덤 이름은 그 sha를 품는다");
        assert!(!tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap(), "정본은 무덤으로 갔다");
        assert_eq!(*graved.lock().unwrap(), vec![x_sha.clone()]);

        // 5) **누수**: `settle()`을 부르지 않고 버린다. `Graved`에는 **파괴적 Drop이 없다** ⇒ 디스크 불변.
        drop(g);
        assert_eq!(
            grave_names(&root).await,
            vec![expected_grave_name(&x_sha)],
            "`Graved`의 Drop이 무덤을 지우면 **fail-OPEN**이다 — 복구할 것이 사라진다"
        );
        assert!(!tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap());
        drop(pass); // ⚠ **명시적**(함정 4) — 살아 있으면 다음 `run_once`가 `pass_lock`에서 hang한다

        // 복구 패스 — `now`를 만료 **이전**으로 되돌려 **복원 그 자체를 직접 관측**한다.
        let stats = timeout(
            Duration::from_secs(5),
            reconcile::run_once_at_for_test(&s, before_expiry(t0), GRACE, SETTLE),
        )
        .await
        .expect("복구 패스는 유한 시간에 끝난다")
        .expect("패스는 Ok다");

        assert_eq!(stats.gc_deleted, 0);
        assert_eq!(stats.referenced, 0);
        assert_eq!(
            tokio::fs::read(s.blob_path(&x_sha)).await.unwrap(),
            x_content,
            "`recover_graves`가 무덤을 정본으로 되돌린다"
        );
        assert!(grave_names(&root).await.is_empty(), "무덤 잔재 0");
    }
}
