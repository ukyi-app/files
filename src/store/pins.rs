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
//! ## ⚠ B-1에서의 위치
//! `Graved`·`settle`·`grave`는 **아직 아무도 부르지 않는다**(GC 삭제/격리 분기는 기존 그대로).
//! 핀과 `landed`는 **기록되지만 읽히지 않는다**. 아래 다섯 항목에 붙은 **dead-code 허용 속성**이
//! 그 사실의 표지이며, **배선과 함께 사라진다** — 넷은 B-2(GC 삭제 분기가 `grave`/`settle`을 부른다),
//! `PassGuard::recovered`의 하나는 B-3(관측성 배선)이 제거한다. **그 제거가 곧 배선의 증거다.**

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
    #[allow(dead_code)] // B-2에서 제거 — 그때 GC 루프에 배선된다
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

    #[allow(dead_code)] // B-2에서 제거 — GC 루프가 pre_grave 훅을 잡을 때 배선된다
    pub(crate) fn pins(&self) -> &BlobPins {
        &self.pins
    }

    /// blob → 무덤 rename + fsync. **성공했을 때만** `Graved`를 낳는다 — `Graved`의 유일한 생성자다.
    #[allow(dead_code)] // B-2에서 제거 — 그때 GC 삭제 분기가 이것으로 교체된다
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

#[allow(dead_code)] // B-2에서 제거 — 그때 GC 삭제 분기가 이것을 쓴다
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
        fn enabled(&self, _m: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _a: &tracing::span::Attributes<'_>) -> tracing::Id {
            tracing::Id::from_u64(1)
        }
        fn record(&self, _i: &tracing::Id, _v: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _i: &tracing::Id, _f: &tracing::Id) {}
        fn event(&self, e: &tracing::Event<'_>) {
            if *e.metadata().level() != tracing::Level::ERROR {
                return;
            }
            let mut v = FieldVisitor(String::new());
            e.record(&mut v);
            self.0.lock().unwrap().push(v.0);
        }
        fn enter(&self, _i: &tracing::Id) {}
        fn exit(&self, _i: &tracing::Id) {}
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
                .filter(|l| l.contains(needle))
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
}
