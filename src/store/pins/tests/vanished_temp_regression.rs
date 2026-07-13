//! **F-14 회귀 증인 — Temp 분기** (`reconcile-vanished-entry-aborts-pass`).
//!
//! ## 왜 형제(`vanished_entry_regression`)만으로는 부족한가 — **이 모듈의 존재 이유**
//!
//! 형제 증인은 **Blob 분기**(`reconcile.rs`의 `let content = tokio::fs::read(&p).await?;`)를 때린다.
//! 그러나 이 버그의 **프론트매터 증상**은 **Temp 분기**다:
//!
//! > *동시 `put_stream`이 `.tmp-<uniq>`를 최종 blob 이름으로 rename하면, 사라진 경로에 대한 stat/read가
//! > ENOENT를 하드 `io::Error`로 전파해 패스 전체가 `Err`로 중단된다.*
//!
//! ⇒ 범인은 Temp 분기의 **`let mtime = e.metadata().await?…`**(나이 판정 **전에** stat한다)다.
//! **Blob 분기만 고치는 픽스**는 형제 증인을 초록으로 만들면서 **프로덕션이 실제로 밟는 Temp 경로는
//! 그대로 망가진 채** 남길 수 있다 — 그러면 기계 배리어(RED→GREEN 락)가 **헛돈다**. 이 증인이 그
//! 구멍을 막는다: **두 분기는 각자의 증인을 갖는다.**
//!
//! ## seam — 8번째 훅 `pre_entry`
//!
//! 기존 7개 훅 중 **Temp 분기에 창을 여는 것은 하나도 없다**: `during_collect`는 스냅샷 **이전**,
//! `post_observe`는 **put 경로**, `pre_grave`/`post_grave`는 **Blob 분기 전용**(무덤은 blob만 만든다).
//! ⇒ `Hooks`에 **`pre_entry`**(항목 루프의 **첫 FS 접촉 직전** · 인자는 **항목 이름**)를 열었다.
//! 프로덕션에서는 **항상 `None`** ⇒ 즉시 반환 ⇒ **관측 행동 변화 0**(근거: `pins.rs`의 `Hooks` doc).
//!
//! ## 결정성 (이 증인의 전부다 — 읽어라)
//!
//! park을 **이름으로 표적화**한다: `pre_entry`가 **바로 그 temp 이름**(`.tmp-f14-temp-victim`)으로
//! 발화할 때만 park하고, **다른 항목의 발화는 그냥 통과**시킨다. ⇒ 그 temp가 스냅샷의 **몇 번째**든
//! park은 **그 항목의 첫 FS 접촉 직전**에 정확히 걸린다 → **readdir 순서와 무관하게 100% 결정적**이다
//! (형제 증인이 *"첫 발화에서 park + victim ≥ 2개"*로 얻은 결정성을, 여기서는 **표적 이름**으로 얻는다).
//!
//! 심는 temp는 **grace 이내(recent)** 다 ⇒ **오늘의 코드는 그것을 보존한다**(삭제 대상이 아니다).
//! 즉 이 증인이 만드는 유일한 차이는 **"우리가 park 중에 그것을 지웠다"** 하나뿐이다 — 그 뒤에 오는
//! `e.metadata()`가 **ENOENT** → `?` → **패스 전체 중단**.
//!
//! ## 동반 blob이 load-bearing인 이유
//!
//! 항목이 temp **하나뿐**이면 훅의 **통과 경로**(이름이 표적이 아닐 때 park하지 않는다)가 **한 번도
//! 실행되지 않는다** → 표적화가 실제로 동작하는지 아무것도 증명하지 못한다. 미참조 blob 하나를 함께
//! 심어 **두 항목**을 만들고, 대조군에서 **`pre_entry`가 두 이름 모두에서 발화했음**을 단언한다
//! (= 훅이 특정 분기에만 배선된 것이 아니다 · 표적 park이 다른 항목을 막지 않는다).
//! 이 blob은 **tombstone이 없으므로**(최초 관측) 회수되지 않는다 — `gc_deleted == 0`이 **구조적**이다.

use super::*;
use std::collections::HashMap;

/// 사라질 temp의 uniq 조각(`layout::temp_blob_path`의 인자).
const VICTIM_UNIQ: &str = "f14-temp-victim";

/// 그 temp의 **온디스크 이름** — **raw 리터럴**이다(ADR-0001: 테스트의 기대값을 `layout` 상수로
/// 경유시키면 **동어반복**이 되어 `.tmp-` 접두사 드리프트를 한 곳에서도 못 잡는다).
/// `stage()`가 이 리터럴이 `layout`이 실제로 만든 이름과 **같음을 단언**한다 → 리터럴이 썩으면 시끄럽다.
const VICTIM_NAME: &str = ".tmp-f14-temp-victim";

/// **표적 이름에서만** 신호 + park한다. 다른 이름은 **기록만 하고 통과**한다.
///
/// 규율(형제와 동일): **`send(도착)` ≺ `park`**(뒤집으면 신호가 영영 오지 않는다) · 해제는
/// `notify_one()`(permit을 저장한다 ⇒ lost wakeup 불가 — `notify_waiters()`는 쓰면 안 된다).
/// `armed` 플래그가 **필요 없다**: 표적 이름의 항목은 스냅샷에 **정확히 하나**뿐이므로 두 번 park될 수 없다.
fn park_at_name(
    target: &'static str,
    tx: UnboundedSender<String>,
    gate: Arc<Notify>,
    seen: Arc<Mutex<Vec<String>>>,
) -> AsyncHook {
    Arc::new(move |name: &str| {
        let (tx, gate, seen, name) = (tx.clone(), gate.clone(), seen.clone(), name.to_owned());
        Box::pin(async move {
            seen.lock().unwrap().push(name.clone());
            if name != target {
                return; // **통과** — 표적이 아닌 항목은 막지 않는다(⇒ readdir 순서 무관)
            }
            tx.send(name).expect("pre_entry 도착 신호");
            gate.notified().await;
        })
    })
}

/// 패스 완료를 **결과째로** 받는다(형제의 `settle_pass`와 같은 명제 — 형제 모듈은 `tests`의 **자식**
/// 이므로 그 private 헬퍼를 형제가 재사용할 수 없다). 언랩 깊이는 `finish_pass`와 같되(타임아웃 ·
/// `JoinError` — 패닉을 `Err`로 **오독하지 않는다**) **`io::Result`는 삼키지 않고 그대로 돌려준다**:
/// 이 증인의 명제는 그 `Err`의 **내용**이기 때문이다(`PASS ABORTED` + `kind == NotFound`).
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

/// 증인과 대조군이 **똑같이** 쓰는 무대. 차이는 오직 **park 중에 temp를 지우느냐**다.
///
/// 포인터를 하나도 만들지 않는다 → `referenced == 0`이 **구조적**이다.
/// tombstone도 하나도 심지 않는다 → 동반 blob은 **최초 관측**에 그친다 → `gc_deleted == 0`도 **구조적**이다.
/// 반환하는 `t0`는 **temp를 심은 직후**의 시각이다 ⇒ `age ≈ 0 ≪ GRACE` ⇒ 그 temp는 **보존 대상**이다.
async fn stage(
    tx: UnboundedSender<String>,
    gate: Arc<Notify>,
    seen: Arc<Mutex<Vec<String>>>,
) -> (Store, tempfile::TempDir, String, SystemTime) {
    let hooks = Hooks {
        pre_entry: Some(park_at_name(VICTIM_NAME, tx, gate, seen)),
        ..Hooks::default()
    };
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let s = Store::with_hooks(root.clone(), hooks);
    tokio::fs::create_dir_all(root.join(".objects"))
        .await
        .unwrap();

    // 동반 blob — 훅의 **통과 경로**를 실제로 태운다(모듈 doc "동반 blob이 load-bearing인 이유").
    let companion = plant_orphan_blob(&s, b"f14-temp-companion").await;

    // 희생될 temp — **grace 이내**다 ⇒ 오늘의 코드는 **보존**한다(삭제 대상이 아니다).
    let victim = s.layout().temp_blob_path(VICTIM_UNIQ);
    atomic::write_atomic(&victim, b"in flight").await.unwrap();
    assert_eq!(
        victim.file_name().unwrap().to_string_lossy(),
        VICTIM_NAME,
        "raw 리터럴 `VICTIM_NAME`이 layout이 실제로 짓는 온디스크 이름과 어긋났다 — \
         `.tmp-` 접두사가 바뀌었다면 이 증인의 park 표적이 영영 발화하지 않는다"
    );

    let t0 = SystemTime::now();
    (s, d, companion, t0)
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  플립 증인 (RED) — 스냅샷 이후 사라진 **temp**는 건너뛰어야 한다. 패스를 중단시켜서는 안 된다.
// ══════════════════════════════════════════════════════════════════════════════════════════

/// **F-14 회귀 증인 — Temp 분기.** 동시 `put_stream`/`write_atomic`의 `.tmp-<uniq>` → `<sha>` rename과
/// **같은 인과**(스냅샷 이후 temp 소멸)를 훅으로 **결정화**한다.
///
/// ⓐ 동반 blob 1개 + **grace 이내** temp 1개를 심는다(오늘의 코드는 temp를 **보존**한다)
/// ⓑ 패스 spawn → ⓒ **그 temp 이름의 `pre_entry` 도착 await**(스냅샷 확정 · 첫 FS 접촉 **이전**)
/// ⓓ park 중에 **그 temp를 삭제**(= 동시 rename이 한 일) → ⓔ 해제
/// ⓕ 루프의 `e.metadata()`가 **ENOENT** → `?` → **패스 전체가 Err**. 올바른 행동은 **skip 후 완주**다
///   → 버그가 살아 있는 동안 **RED**. 그게 목적이다(red-capture).
#[tokio::test]
async fn reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot() {
    let (tx, mut rx) = unbounded_channel::<String>();
    let gate = Arc::new(Notify::new());
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let (s, _d, companion, t0) = stage(tx, gate.clone(), seen.clone()).await;

    // ⓑ
    let s2 = s.clone();
    let mut gc: PassHandle =
        tokio::spawn(async move { reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE).await });

    // ⓒ **도착 await** — 여기서 기다리지 않으면 삭제가 read_dir **이전에** 일어나 시나리오가 성립하지 않는다.
    // 자기검증 ①: 훅이 **실제로 발화했고**(안 왔으면 `arrived`가 타임아웃으로 **실패**한다 — "조용한 초록"
    // 은 불가능하다) 그 이름이 **바로 그 temp**다(= 루프가 이 항목의 FS 접촉 **직전**에 서 있다).
    let parked = arrived(&mut rx).await;
    assert_eq!(
        parked, VICTIM_NAME,
        "자기검증 ①: park은 **표적 temp의 이름**에서 걸려야 한다 — parked={parked}"
    );
    // 패스가 **훅 안에 머물러 있다** ⇒ 아래 삭제는 스냅샷 **이후** · 그 항목의 stat **이전**에 일어난다.
    probe_still_waiting(&mut gc).await;

    // ⓓ **스냅샷 이후 소멸.** 자기검증 ②: 삭제가 **성공해야** 한다 = 그 temp는 park 시점에 **아직
    // 디스크에 있었고**(= 미처리) **지운 것은 우리다**. 실패하면 시나리오 자체가 성립하지 않는다.
    let victim = s.layout().temp_blob_path(VICTIM_UNIQ);
    tokio::fs::remove_file(&victim).await.unwrap_or_else(|e| {
        panic!(
            "자기검증 ②: victim({VICTIM_NAME})은 park 시점에 **아직 디스크에 있어야** 한다 \
             (= 루프가 그 항목을 아직 처리하지 않았다). 없다면 시나리오가 성립하지 않는다: {e:?}"
        )
    });
    assert!(
        !tokio::fs::try_exists(&victim).await.unwrap(),
        "자기검증 ②': 삭제 후 그 temp는 **부재**여야 한다"
    );

    // ⓔ 해제
    gate.notify_one();

    // ⓕ **올바른 행동**: 사라진 temp는 건너뛰고 패스는 완주한다.
    let stats = settle_pass(gc).await.unwrap_or_else(|e| {
        assert_eq!(
            e.kind(),
            std::io::ErrorKind::NotFound,
            "PASS ABORTED — 패스가 Err로 중단됐는데 그 kind가 **NotFound가 아니다**. \
             이 증인이 겨누는 것은 ENOENT 전파다 — 다른 io 에러라면 시나리오가 오염된 것이다: {e:?}"
        );
        panic!(
            "PASS ABORTED — 스냅샷 이후 사라진 **temp**({VICTIM_NAME})를 만난 reconcile 패스가 \
             그 항목을 **건너뛰지 않고** 패스 **전체**를 Err로 중단시켰다. \
             범인 `?`는 Temp 분기의 `let mtime = e.metadata().await?…`(나이 판정 **전에** stat한다). \
             이것이 프론트매터가 적은 **바로 그 증상**이다: 동시 `put_stream`이 `.tmp-<uniq>`를 최종 blob \
             이름으로 rename하면 스냅샷에 잡힌 temp가 사라진다 → 패스 전체 중단. \
             err={e:?} kind={:?}",
            e.kind()
        )
    });

    // 완주의 **온디스크** 증거: 패스 끝의 pending 지속화가 실제로 일어났다(중단된 루프는 **도달하지 못한다**).
    let pending = read_pending(&s).await;
    assert_eq!(
        pending.keys().cloned().collect::<Vec<_>>(),
        vec![companion.clone()],
        "동반 blob은 **최초 관측**으로 tombstone만 얻는다(회수되지 않는다)"
    );
    assert!(
        tokio::fs::try_exists(s.blob_path(&companion))
            .await
            .unwrap(),
        "동반 blob은 살아남는다 — 사라진 것은 temp 하나뿐이다"
    );
    assert_eq!(
        stats,
        ReconcileStats {
            referenced: 0,    // 포인터를 만들지 않았다 — **구조적으로** 0
            gc_deleted: 0,    // 동반 blob은 tombstone 최초 관측 단계다 — 회수 없음
            gc_pending: 1,    // 그 tombstone 하나
            temps_deleted: 0, // temp는 grace 안이었고, 게다가 **사라졌다** → 삭제 대상 아님
            quarantined: 0,   // 비트로트 없음
        },
        "사라진 temp는 **건너뛰고** 나머지는 정상 처리된다"
    );
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  대조군 (GREEN) — 빨간 원인이 **하니스가 아니라 temp 소멸 하나**임을 고정한다.
// ══════════════════════════════════════════════════════════════════════════════════════════

/// **대조군.** 위 증인과 **똑같은 랑데부**(같은 훅 · 같은 park · 같은 주입 시각 · 같은 무대)를 돌리되
/// **삭제만 하지 않는다**. 이것이 초록이면 위 증인의 RED는 park·`pre_entry`·시각·`Store::with_hooks`
/// 때문이 **아니라** 오직 **스냅샷 이후 temp 소멸** 때문이다 ⇒ 신호가 tight하다.
/// **red 트리에서도 green 트리에서도 초록**이다(characterization).
///
/// 덤으로 **seam의 계약**을 못박는다: `pre_entry`는 예약 이름을 제외한 **모든** 항목에서 발화하며
/// (동반 blob + temp = **정확히 둘**), 표적 park이 **다른 항목을 막지 않는다**.
#[tokio::test]
async fn reconcile_pass_control_without_a_vanishing_temp_is_green() {
    let (tx, mut rx) = unbounded_channel::<String>();
    let gate = Arc::new(Notify::new());
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let (s, _d, companion, t0) = stage(tx, gate.clone(), seen.clone()).await;

    let s2 = s.clone();
    let mut gc: PassHandle =
        tokio::spawn(async move { reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE).await });

    let parked = arrived(&mut rx).await;
    assert_eq!(parked, VICTIM_NAME, "park은 표적 temp의 이름에서 걸린다");
    probe_still_waiting(&mut gc).await;

    // **삭제하지 않는다** — 증인과의 차이는 이것뿐이다.
    gate.notify_one();

    let stats = finish_pass(gc).await;

    let victim = s.layout().temp_blob_path(VICTIM_UNIQ);
    assert!(
        tokio::fs::try_exists(&victim).await.unwrap(),
        "grace 이내의 temp는 **보존**된다(활성 스트리밍 보호) — 오늘의 코드도 그렇다"
    );
    assert_eq!(
        read_pending(&s).await.keys().cloned().collect::<Vec<_>>(),
        vec![companion.clone()]
    );
    assert_eq!(
        stats,
        ReconcileStats {
            referenced: 0,
            gc_deleted: 0,
            gc_pending: 1,
            temps_deleted: 0, // grace 안의 temp는 보존된다
            quarantined: 0,
        },
        "아무것도 사라지지 않으면 같은 랑데부에서 패스는 **Ok**다"
    );

    // seam의 계약: 예약 이름을 뺀 **모든** 항목에서 발화한다 · 표적 park이 다른 항목을 막지 않는다.
    let mut fired = seen.lock().unwrap().clone();
    fired.sort();
    let mut want = vec![companion, VICTIM_NAME.to_owned()];
    want.sort();
    assert_eq!(
        fired, want,
        "`pre_entry`는 항목 루프의 **모든** 비예약 항목에서 정확히 한 번씩 발화한다"
    );
}
