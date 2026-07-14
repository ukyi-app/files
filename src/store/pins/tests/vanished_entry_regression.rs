//! **F-14 회귀 증인 — `reconcile-vanished-entry-aborts-pass`.**
//!
//! `run_once_at`은 `.objects` 직속 항목을 `Vec<DirEntry>`로 **스냅샷**한 뒤 항목별로 **다시 stat/read**
//! 한다. 그 사이 동시 쓰기가 `.tmp-<uniq>`를 rename해 **항목이 사라지면**, 사라진 경로에 대한 stat/read가
//! 내는 `NotFound`를 *"이 항목은 이제 없다 = 건너뛴다"*로 해석하지 않고 **`?`로 전파**해 **패스 전체를
//! `Err`로 중단**시킨다. 이 모듈의 증인은 **올바른 행동을 단언한다**(= 사라진 항목은 skip · 패스는 완주)
//! → 버그가 살아 있는 동안 **RED**다. 그게 목적이다(red-capture).
//!
//! ## 범인 (진단으로 확정한 `?` 목록 — 픽스가 **셋 다** 덮어야 한다)
//!
//! * `reconcile.rs:199` — Temp 분기 `let mtime = e.metadata().await?…`
//!   (나이 판정 **전에** stat한다 → 사라진 temp는 여기서 ENOENT)
//! * `reconcile.rs:208` — Blob 분기 `let content = tokio::fs::read(&p).await?;`
//! * `reconcile.rs:192` — `let ft = e.file_type().await?;` — **잠복 범인**. APFS는 `readdir`의 `d_type`을
//!   `DirEntry`에 캐시하므로 stat을 걸지 않는다 → **로컬에서는 무죄**다. 그러나 `d_type`이 `DT_UNKNOWN`인
//!   FS(일부 Linux FS·overlayfs 등)에서는 이것이 **stat으로 내려가** 같은 ENOENT 중단을 낸다.
//!   ⇒ **이 증인이 여기서는 그것을 잡지 못한다**. 픽스는 세 곳을 **함께** 고쳐야 한다.
//!
//! ## blast radius — `put_stream` 한정이 **아니다**
//!
//! `atomic::write_atomic`(`atomic.rs:53,59`)이 **모든** blob 기록에 `.objects/.tmp-<uniq>` 생성 + rename을
//! 쓴다 ⇒ **버퍼드 `put`도 같은 레이스를 만든다**. "스트리밍 업로드 중에만 나는 버그"가 아니라 **쓰기
//! 트래픽이 있는 한 상시**다. (진단 단계에서 버퍼드 `put` 40개 + reconcile 루프만으로 재현했다.)
//!
//! ## 왜 지금까지 조용했는가
//!
//! `tests/adversarial.rs:91`(`concurrent_nested_puts_with_reconcile_loop_preserve_all`)이 이 시나리오를
//! **매 실행 밟고 있으면서** `let _ = reconcile::run_once(..).await;`로 **결과를 버린다** → 패스가 `Err`로
//! 중단돼도 테스트는 초록이다. **그 `let _ =`를 고치는 것은 픽스 증분의 몫이다** — red.sha(이 커밋)에서
//! 고치면 characterization이 빨개져 게이트의 락(= "회귀 증인만 RED")이 성립하지 않는다.
//!
//! ## 결정성 (이 증인의 전부다 — 읽어라)
//!
//! 엔트리 루프 **안에서** 발화하는 훅은 `pre_grave` 단 하나다(`during_collect`는 스냅샷 **이전**,
//! `post_observe`는 **put 경로**에서 발화한다 → 둘 다 창을 열지 못한다). `pre_grave`는 **스냅샷 순서대로**
//! 발화하므로 **첫 발화에서 park**하면 스냅샷은 이미 떠 있고, 파킹된 항목 **뒤**의 항목은 전부 **미처리**다.
//! ⇒ gravable blob을 **2개 이상** 심어 두고 **파킹된 sha가 아닌** 다른 gravable blob을 지우면 그 항목은
//! **반드시 미처리** → Blob 분기의 `read()`가 ENOENT → 패스 중단. **readdir 순서와 무관하게 100% 결정적**이다.
//! (gravable이 1개뿐이면 그것이 스냅샷의 **마지막** 항목일 수 있다 → victim 0 → 초록. 2개 이상이 **load-bearing**.)
//!
//! ## 이 모듈이 `pins::tests`의 **자식**인 이유
//!
//! ① `Hooks`의 9개 필드는 `pins` private이다 → 훅을 리터럴로 짓는 증인은 **`pins`의 자손**이어야 한다
//!    (필드 계수와 "왜 두 번째 플립이 아닌가"의 논증은 `pins.rs`의 `Hooks` doc이 소유한다).
//! ② 랑데부 프리미티브(`arrived`/`finish_pass`/`probe_still_waiting`…)는 `pins::tests` private이다 →
//!    **형제 모듈은 재사용할 수 없다**(그래서 진단 산출물은 그것들을 복사했었다 — 위험한 반복구의 복제).
//! `tests`의 **자식**이면 ①②를 **둘 다** 만족한다: 기존 테스트의 가시성을 넓히지도, 훅을 늘리지도,
//! 위험한 기계를 복사하지도 않는다.

use super::*;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

/// gravable orphan 수. **≥2가 결정성의 근거**다(모듈 doc "결정성" 참조). 3개를 심어 여유를 둔다.
const ORPHANS: usize = 3;
/// 활성 스트리밍 잔재. grace 안이라 **삭제 대상이 아니지만**, Temp 분기는 나이 판정 **전에**
/// `e.metadata()`를 부른다 → 사라지면 ENOENT다. 이것들이 `reconcile.rs:199`를 함께 핀한다.
const TEMPS: usize = 2;

/// **첫 `pre_grave` 도착에서만** 신호 + park한다. 두 번째부터는 통과한다 —
/// `async_park`(무조건 park)를 쓰면 park을 한 번 풀어 준 뒤 **다음 항목에서 다시 park해 hang**하고,
/// `async_park_for(target)`은 **어느 sha가 첫 발화인지 미리 알 수 없어** 쓸 수 없다.
///
/// 규율: **`send(도착)` ≺ `park`**(뒤집으면 신호가 영영 오지 않는다) · 해제는 `notify_one()`
/// (permit을 저장한다 ⇒ lost wakeup 불가 — `notify_waiters()`는 쓰면 안 된다).
fn park_at_first_grave(tx: UnboundedSender<String>, gate: Arc<Notify>) -> AsyncHook {
    let armed = Arc::new(AtomicBool::new(true));
    Arc::new(move |sha: &str| {
        let (tx, gate, sha, armed) = (tx.clone(), gate.clone(), sha.to_owned(), armed.clone());
        Box::pin(async move {
            if !armed.swap(false, Ordering::SeqCst) {
                return; // 두 번째부터는 통과 — 무한 park 방지
            }
            tx.send(sha).expect("pre_grave 도착 신호");
            gate.notified().await;
        })
    })
}

/// 패스 완료를 **결과째로** 받는다. 언랩 깊이는 `finish_pass`와 같되(타임아웃 · `JoinError` —
/// 패닉을 `Err`로 **오독하지 않는다**) **`io::Result`는 삼키지 않고 그대로 돌려준다**:
/// 이 증인의 명제는 그 `Err`의 **내용**이기 때문이다(실패 메시지가 `PASS ABORTED`와
/// `kind=NotFound`를 **둘 다** 말해야 한다 → `finish_pass`의 `.expect("패스는 Ok다")`로는 말할 수 없다).
/// 대조군은 `finish_pass`를 **그대로** 쓴다.
async fn settle_pass(gc: PassHandle) -> std::io::Result<ReconcileStats> {
    timeout(Duration::from_secs(5), gc)
        .await
        .expect("패스는 유한 시간에 끝난다")
        .expect("GC 태스크는 패닉하지 않는다")
}

/// 패스 끝에 재기록되는 `.gc-pending.json`. **버그 상태에서는 `?`가 루프를 탈출하므로 이 write가
/// 영영 실행되지 않는다** ⇒ 이 파일의 내용은 *"루프가 끝까지 돌았다"*의 온디스크 증거다.
async fn read_pending(s: &Store) -> HashMap<String, u64> {
    let raw = tokio::fs::read(s.layout().gc_pending_path())
        .await
        .expect(".gc-pending.json은 패스 끝에 재기록된다");
    serde_json::from_slice(&raw).expect(".gc-pending.json은 유효한 JSON이다")
}

/// 증인과 대조군이 **똑같이** 쓰는 무대. 차이는 오직 **park 중에 항목을 지우느냐**다.
///
/// 포인터를 하나도 만들지 않는다(blob을 디스크에 직접 심는다) → `referenced == 0`이 **구조적**이다.
/// tombstone은 `t0 - 2·GRACE`에 심어 `t0` 주입 패스에서 **결정적으로 만료**시킨다(= `pre_grave` 발화).
async fn stage(
    tx: UnboundedSender<String>,
    gate: Arc<Notify>,
) -> (Store, tempfile::TempDir, Vec<String>, SystemTime) {
    let hooks = Hooks {
        pre_grave: Some(park_at_first_grave(tx, gate)),
        ..Hooks::default()
    };
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let s = Store::with_hooks(root.clone(), hooks);
    tokio::fs::create_dir_all(root.join(".objects"))
        .await
        .unwrap();

    let mut shas = Vec::new();
    for i in 0..ORPHANS {
        shas.push(plant_orphan_blob(&s, format!("f14-orphan-{i}").as_bytes()).await);
    }
    for i in 0..TEMPS {
        atomic::write_atomic(
            &s.layout().temp_blob_path(&format!("f14-{i}")),
            b"in flight",
        )
        .await
        .unwrap();
    }
    let t0 = SystemTime::now();
    let refs: Vec<&str> = shas.iter().map(String::as_str).collect();
    seed_expired_tombstones(&root, t0, &refs).await;
    (s, d, shas, t0)
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  플립 증인 (RED) — 스냅샷 이후 사라진 항목은 **건너뛰어야** 한다. 패스를 중단시켜서는 안 된다.
// ══════════════════════════════════════════════════════════════════════════════════════════

/// **F-14 회귀 증인.** 동시 쓰기의 `.tmp-<uniq>` → `<sha>` rename과 **같은 인과**(스냅샷 이후 항목 소멸)를
/// 훅으로 **결정화**한다.
///
/// ⓐ gravable orphan 3개 + temp 2개를 심고 만료 tombstone을 주입한다
/// ⓑ 패스 spawn → ⓒ **첫 `pre_grave` 도착 await**(스냅샷 확정 · 루프 진입 확정) → **아직 대기 중임을 관측**
/// ⓓ park 중에 **파킹된 sha가 아닌** 항목들을 지운다(= 동시 rename이 한 일)
/// ⓔ 해제 → ⓕ 패스는 **완주해야 한다**. 현재는 `Err(NotFound)`로 **중단**된다 → 여기서 RED.
#[tokio::test]
async fn reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot() {
    let (tx, mut rx) = unbounded_channel::<String>();
    let gate = Arc::new(Notify::new());
    let (s, _d, shas, t0) = stage(tx, gate.clone()).await;

    // ⓑ
    let s2 = s.clone();
    let mut gc: PassHandle =
        tokio::spawn(async move { reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE).await });

    // ⓒ **도착 await** — 여기서 기다리지 않으면 삭제가 read_dir **이전에** 일어나 시나리오가 성립하지 않는다.
    let parked = arrived(&mut rx).await;
    // 자기검증 ①: `pre_grave`가 **실제로 발화했고**, 파킹된 것은 우리가 심은 orphan이다(= GC 분기 진입).
    assert!(
        shas.contains(&parked),
        "자기검증 ①: 파킹된 sha는 우리가 심은 orphan 중 하나여야 한다 — 실제로 삭제(grave) 분기에 진입했다. \
         parked={parked} planted={shas:?}"
    );
    // 패스가 **훅 안에 머물러 있다** ⇒ 아래 삭제는 스냅샷 **이후** · victim 처리 **이전**에 일어난다.
    probe_still_waiting(&mut gc).await;

    // ⓓ **스냅샷 이후 소멸.** 파킹된 blob만 남기고 나머지를 지운다.
    let victims: Vec<&String> = shas.iter().filter(|x| **x != parked).collect();
    for sha in &victims {
        // 자기검증 ②: **여기서 지울 수 있다는 것 자체가** 그 항목이 **미처리**라는 증거다 —
        // 이미 처리됐다면 gravable orphan은 grave→reap으로 **이미 사라졌을** 것이므로 ENOENT로 실패한다.
        tokio::fs::remove_file(s.blob_path(sha)).await.unwrap_or_else(|e| {
            panic!(
                "자기검증 ②: victim({sha})은 park 시점에 **아직 디스크에 있어야** 한다 \
                 (= 루프가 그 항목에 도달하지 않았다 = 미처리). 없다면 시나리오가 성립하지 않는다: {e:?}"
            )
        });
    }
    for i in 0..TEMPS {
        tokio::fs::remove_file(s.layout().temp_blob_path(&format!("f14-{i}")))
            .await
            .unwrap();
    }

    // ⓔ 해제
    gate.notify_one();

    // ⓕ **올바른 행동**: 사라진 항목은 건너뛰고 패스는 완주한다.
    let stats = settle_pass(gc).await.unwrap_or_else(|e| {
        panic!(
            "PASS ABORTED — 스냅샷 이후 사라진 항목(victims={victims:?})을 만난 reconcile 패스가 \
             그 항목을 **건너뛰지 않고** 패스 **전체**를 Err로 중단시켰다. \
             동시 쓰기(`atomic::write_atomic`의 `.tmp-<uniq>` → rename)가 있는 한 이것은 상시 발생한다. \
             err={e:?} kind={:?} (NotFound = ENOENT: 범인 `?`는 reconcile.rs:199(Temp `metadata`) / \
             :208(Blob `read`) — :192(`file_type`)는 DT_UNKNOWN FS에서의 잠복 범인)",
            e.kind()
        )
    });

    // 완주의 **온디스크** 증거: 패스 끝의 pending 지속화가 실제로 일어났고, 사라진 blob의 tombstone은
    // 그 정리(try_exists)에서 제거됐다. (중단된 루프는 이 write에 **도달하지 못한다**.)
    assert!(
        read_pending(&s).await.is_empty(),
        "사라진 blob의 tombstone은 패스 끝 정리에서 제거된다 — 남았다면 루프가 중단된 것이다"
    );
    assert_eq!(
        stats,
        ReconcileStats {
            referenced: 0,    // 포인터를 만들지 않았다 — **구조적으로** 0
            gc_deleted: 1,    // 파킹된 orphan 1개만 회수된다(나머지는 **사라졌으므로 skip**)
            gc_pending: 0,    // 사라진 blob의 tombstone은 정리된다
            temps_deleted: 0, // temp는 grace 안이었고, 게다가 사라졌다 → 삭제 대상 아님
            quarantined: 0,   // 비트로트 없음
        },
        "사라진 항목은 **건너뛰고** 나머지는 정상 처리된다"
    );
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  대조군 (GREEN) — 빨간 원인이 **하니스가 아니라 항목 소멸 하나**임을 고정한다.
// ══════════════════════════════════════════════════════════════════════════════════════════

/// **대조군.** 위 증인과 **똑같은 랑데부**(같은 훅 · 같은 park · 같은 주입 시각 · 같은 무대)를 돌리되
/// **삭제만 하지 않는다**. 이것이 초록이면 위 증인의 RED는 park·훅·시각·`Store::with_hooks` 때문이
/// **아니라** 오직 **스냅샷 이후 항목 소멸** 때문이다 ⇒ 신호가 tight하다.
/// 픽스 이후에도 **초록으로 남는다**(정상 경로 무회귀).
#[tokio::test]
async fn reconcile_pass_control_without_vanishing_entries_is_green() {
    let (tx, mut rx) = unbounded_channel::<String>();
    let gate = Arc::new(Notify::new());
    let (s, _d, shas, t0) = stage(tx, gate.clone()).await;

    let s2 = s.clone();
    let mut gc: PassHandle =
        tokio::spawn(async move { reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE).await });

    let parked = arrived(&mut rx).await;
    assert!(
        shas.contains(&parked),
        "파킹된 sha는 우리가 심은 orphan 중 하나다"
    );
    probe_still_waiting(&mut gc).await;

    // **삭제하지 않는다** — 증인과의 차이는 이것뿐이다.
    gate.notify_one();

    let stats = finish_pass(gc).await;
    assert!(read_pending(&s).await.is_empty());
    assert_eq!(
        stats,
        ReconcileStats {
            referenced: 0,
            gc_deleted: ORPHANS, // 미참조·만료 orphan은 **전부** 회수된다
            gc_pending: 0,
            temps_deleted: 0, // grace 안의 temp는 보존된다
            quarantined: 0,
        },
        "아무것도 사라지지 않으면 같은 랑데부에서 패스는 **Ok**다"
    );
}
