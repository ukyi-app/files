//! **원(un-minimised) repro — F-14 릴리스 게이트 R-2.**
//!
//! 진단이 적은 원 repro는 이것이다: ***"40개 버퍼드 put이 reconcile과 경합한다."***
//! `tests/adversarial.rs::concurrent_nested_puts_with_reconcile_loop_preserve_all`이 **바로 그
//! 안무**를 돌지만, 그 테스트는 red 판본에서 `let _ = reconcile::run_once(..)`로 **결과를 버렸기
//! 때문에** 버그를 매 실행 밟으면서도 **초록**이었다. 이 파일은 그 안무를 **관측하는** 판본이다.
//!
//! ## 이 파일이 지키는 계약(R-2)
//!
//! | | |
//! |---|---|
//! | **버리지 않는다** | `run_once`의 `Err`를 **센다**(패닉하지 않고 계속 돈다 — 전수 계수) |
//! | **반복 증인** | reconcile 패스가 **put이 in-flight인 동안** 실제로 몇 번 완주(시도)했는지 계수 → 단언 |
//! | **레이스 증인** | **소멸 레이스가 실제로 일어났음**을 계수 → 단언 |
//! | **symptomToken** | red에서 실패 메시지에 **`PASS ABORTED`** |
//!
//! ## 레이스 증인은 왜 "관측자"인가 — 그리고 그 한계
//!
//! 통합 테스트는 `pins::Hooks`(crate-private)에도 `reconcile::Vanished`(reconcile 서브트리 전용)에도
//! **닿을 수 없다**. `ReconcileStats`는 **필드 추가가 금지**돼 있고(`layout_tree.rs`의 전수
//! `assert_eq!` 3곳), 소멸 경로는 **tracing 이벤트도 내지 않는다**(green 소스 실측: `absence.rs` ·
//! `entry.rs`에 `tracing::` 0건). ⇒ **reconciler 자신의 소멸 계수기는 밖에서 읽을 방법이 없다.**
//!
//! 그래서 레이스 증인은 **관측자 태스크**다. 그 관측자는 reconcile의 항목 루프와 **같은 모양**이다:
//!
//! 1. `.objects`를 `read_dir`로 **스냅샷**한다 (= `Entry::snapshot` / red의 `read_dir` 루프),
//! 2. 스냅샷 **순서대로** 항목당 작업을 한다 — blob 이름이면 `read`(= Blob 분기의 무결성 읽기),
//!    `.tmp-` 이면 `metadata`(= **Temp 분기의 첫 stat, 즉 오늘의 범인 `e.metadata().await?`**),
//! 3. 그 `metadata`가 **`NotFound`**를 내면 = **스냅샷에 있던 항목이 사라졌다** = **소멸 레이스**.
//!
//! `.objects` 안의 `.tmp-<uniq>`를 **지우는** 주체는 `write_atomic`/`stage`의 **rename**뿐이다
//! (grace 1h ⇒ reconcile의 temp 회수는 발화하지 않는다) ⇒ 이 `NotFound` 하나하나가 **rename이
//! 스냅샷과 경합했다는 물증**이다. 관측자의 항목당 작업량은 reconciler의 것과 **같으므로**(blob read
//! 포함) 관측자의 창은 reconciler의 창보다 **넓지 않다** ⇒ 관측된 소멸 수는 reconciler가 노출된
//! 레이스의 **하한**이다.
//!
//! ### 자기공격: *"그 소멸이 put이 아니라 **reconcile 자신**의 것이면?"*
//!
//! `.objects` 안에 `.tmp-`를 **만드는** 주체는 정확히 **둘**이다(`layout.rs` 실측: `.objects` 직속
//! 경로를 target으로 받는 `write_atomic` 호출부는 이 둘뿐):
//!
//! * **put** — `write_atomic(blob_path(sha))` ⇒ **put 1회당 정확히 1개**(내용이 유니크하므로 dedup 없음),
//! * **reconcile 자신** — `write_atomic(gc_pending_path())`(`.objects/.gc-pending.json`) ⇒ **완주한
//!   패스 1회당 최대 1개**(루프 **뒤**에 딱 한 번 호출된다).
//!
//! ⇒ 한 실행에서 태어난 **reconcile발 temp의 상계는 `passes`**다. 그래서 레이스 증인은
//! **`vanished_during_pass − passes`**(= **put이 만든 temp의 소멸 수의 하한**)에 걸린다 —
//! 관측된 소멸이 **전부** reconcile 자신의 gc-pending temp였다는 세계에서도 이 단언은 살아남지 못한다.
//!
//! **red에서는 여기에 직접 증거가 하나 더 붙는다**: reconciler 자신이 낸 `Err`의 `kind`가
//! **`NotFound`(ENOENT)** 라는 것 — 그것은 관측자가 아니라 **패스 자신이** 소멸을 밟았다는 물증이며,
//! 실패 메시지에 원문으로 찍힌다.
//!
//! ⚠ **green에서 못 하는 것(정직한 한계)**: green은 `Err`가 0이므로 *"reconciler **자신의**
//! 스냅샷에 있던 그 항목이 사라졌다"*를 **패스 내부에서** 증명할 수 없다(위의 봉인 3중 때문에
//! 관측 가능한 표면이 없다). green이 증명하는 것은 **① 레이스가 살아 있었다**(관측자) ∧
//! **② 그 레이스가 도는 동안 reconcile 패스가 in-flight였다**(같은 계수에 pass-in-flight 조건을
//! 걸었다) ∧ **③ 그런데도 패스는 한 번도 중단되지 않았다**(`Err == 0`)이다. red↔green은 put 경로·
//! 스냅샷 구조가 **동일**하고 **에러 처리만 다르므로**, red가 직접 증명한 "패스가 소멸을 밟는다"는
//! green에서도 성립한다 — 그것을 흡수하는 것이 픽스다.

use files::store::{reconcile, Store};
use std::io::ErrorKind;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod common;
use common::hex_sha;

/// 진단이 적은 규모 — **동시 put 40개**.
const PUT_WORKERS: usize = 40;
/// 워커당 라운드. 폭풍이 **충분히 오래** 지속돼야 reconcile 루프와 겹치는 창이 생긴다.
const ROUNDS: usize = 25;
const TOTAL_PUTS: usize = PUT_WORKERS * ROUNDS;

/// grace 1h — 갓 기록된 blob도 `.tmp-` 잔재도 **회수되지 않는다**(원 repro와 동일).
/// ⇒ `.objects`에서 무언가가 **사라진다면** 그것은 **동시 put의 rename**뿐이다.
const GC_GRACE: Duration = Duration::from_secs(3600);
const SETTLE: Duration = Duration::from_secs(30);

/// **반복 증인의 바닥** — put이 in-flight인 동안 완주(시도)한 패스 수.
/// (실측 20회: green 16~24 · red 33~42 ⇒ 3배 이상의 여유.)
const MIN_OVERLAPPED_PASSES: usize = 5;
/// **레이스 증인의 바닥** — reconcile 패스가 in-flight인 동안 관측된 소멸 중 **put이 만든 temp의
/// 소멸 수의 하한**(= `vanished_during_pass − passes`; 위 §자기공격 참조).
/// (실측 20회: green ≥283 · red ≥261 ⇒ 26배 이상의 여유.)
const MIN_PUT_TEMP_VANISHES: usize = 10;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn original_repro_concurrent_puts_do_not_abort_the_reconcile_pass() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let objects = root.join(".objects");
    // 무대를 **먼저** 세운다: `.objects`가 없으면 `run_once`는 즉시 `Ok(default)`로 조기 반환한다
    // ⇒ 그런 패스는 **공허**하다(스냅샷도 안 뜬다). 반복 증인이 그것을 세면 안 된다.
    std::fs::create_dir_all(&objects).unwrap();
    let s = Arc::new(Store::new(root.clone()));

    let stop = Arc::new(AtomicBool::new(false));
    let puts_in_flight = Arc::new(AtomicUsize::new(0));
    // 현재 reconcile 패스가 `run_once` **안**에 있는가(0 또는 1).
    let in_pass = Arc::new(AtomicUsize::new(0));

    let passes = Arc::new(AtomicUsize::new(0));
    let overlapped = Arc::new(AtomicUsize::new(0));
    let errs = Arc::new(AtomicUsize::new(0));
    let errs_notfound = Arc::new(AtomicUsize::new(0));
    let first_err: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let scans = Arc::new(AtomicUsize::new(0));
    let temps_seen = Arc::new(AtomicUsize::new(0));
    let vanished = Arc::new(AtomicUsize::new(0));
    let vanished_during_pass = Arc::new(AtomicUsize::new(0));

    // ── ① reconcile 루프 — 결과를 **버리지 않는다** ────────────────────────────────────────
    // ⚠ 여기서 패닉하지 않는다(`unwrap`/`expect` 금지). red에서 첫 `Err`에 패닉하면 **표본이 1개**로
    //    끝나고 반복·레이스 증인이 죽는다. **전수 계수**하고 마지막에 단언한다.
    let rec = {
        let s2 = (*s).clone(); // ⚠ 같은 Store를 공유해야 한다(핀 등록부가 in-process) — D-3
        let stop = stop.clone();
        let in_flight = puts_in_flight.clone();
        let in_pass = in_pass.clone();
        let passes = passes.clone();
        let overlapped = overlapped.clone();
        let errs = errs.clone();
        let errs_notfound = errs_notfound.clone();
        let first_err = first_err.clone();
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let before = in_flight.load(Ordering::SeqCst);
                in_pass.fetch_add(1, Ordering::SeqCst);
                let r = reconcile::run_once(&s2, GC_GRACE, SETTLE).await;
                in_pass.fetch_sub(1, Ordering::SeqCst);
                let after = in_flight.load(Ordering::SeqCst);

                passes.fetch_add(1, Ordering::Relaxed);
                // **보수적 계수**: 패스의 시작·끝 **양 끝에서** put이 하나도 안 보이면 세지 않는다
                // (중간에 겹쳤더라도 세지 않는다 ⇒ 과소계수).
                if before > 0 || after > 0 {
                    overlapped.fetch_add(1, Ordering::Relaxed);
                }
                if let Err(e) = r {
                    errs.fetch_add(1, Ordering::Relaxed);
                    if e.kind() == ErrorKind::NotFound {
                        errs_notfound.fetch_add(1, Ordering::Relaxed);
                    }
                    let mut slot = first_err.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(format!("kind={:?} err={e:?}", e.kind()));
                    }
                }
                tokio::task::yield_now().await;
            }
        })
    };

    // ── ② 소멸 관측자 — reconcile 항목 루프와 **같은 모양**(스냅샷 → 항목당 stat) ──────────
    let obs = {
        let stop = stop.clone();
        let in_pass = in_pass.clone();
        let objects = objects.clone();
        let scans = scans.clone();
        let temps_seen = temps_seen.clone();
        let vanished = vanished.clone();
        let vanished_during_pass = vanished_during_pass.clone();
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                // 1) 스냅샷(= reconciler의 `read_dir` 한 걸음).
                let mut names: Vec<String> = Vec::new();
                let mut rd = match tokio::fs::read_dir(&objects).await {
                    Ok(rd) => rd,
                    Err(_) => break,
                };
                while let Ok(Some(e)) = rd.next_entry().await {
                    names.push(e.file_name().to_string_lossy().into_owned());
                }
                scans.fetch_add(1, Ordering::Relaxed);

                // 2) 스냅샷 **순서대로** 항목당 작업 — reconciler와 같은 무게(창을 넓히지 않는다).
                for name in &names {
                    if name.starts_with(".tmp-") {
                        temps_seen.fetch_add(1, Ordering::Relaxed);
                        // reconcile 패스가 **지금 in-flight인가**를 stat **직전에** 표본한다.
                        let during = in_pass.load(Ordering::SeqCst) > 0;
                        // 3) 오늘의 범인과 **같은 syscall**: Temp 분기의 첫 stat.
                        match tokio::fs::metadata(objects.join(name)).await {
                            Err(e) if e.kind() == ErrorKind::NotFound => {
                                // **소멸 레이스**: 스냅샷에 있던 `.tmp-`가 stat 전에 사라졌다.
                                // 그것을 치우는 주체는 **동시 put의 rename**뿐이다(grace 1h).
                                vanished.fetch_add(1, Ordering::Relaxed);
                                if during {
                                    vanished_during_pass.fetch_add(1, Ordering::Relaxed);
                                }
                            }
                            _ => {}
                        }
                    } else if name.len() == 64 && name.chars().all(|c| c.is_ascii_hexdigit()) {
                        // Blob 분기의 무결성 읽기와 **같은 무게** — 관측자의 창이 reconciler의 창보다
                        // **좁지 않도록**(그래야 관측 소멸 수가 하한이 된다).
                        let _ = tokio::fs::read(objects.join(name)).await;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
    };

    // ── ③ put 폭풍 — 40 워커 × ROUNDS 라운드. 내용은 **전부 유니크**(dedup되면 `.tmp-`가 안 생긴다) ──
    let mut hs = Vec::with_capacity(PUT_WORKERS);
    for w in 0..PUT_WORKERS {
        let s = s.clone();
        let in_flight = puts_in_flight.clone();
        hs.push(tokio::spawn(async move {
            for r in 0..ROUNDS {
                let key = format!("dir/sub/w{w}-r{r}.bin");
                // 유니크 내용 ⇒ `pin.blob_intact`가 거짓 ⇒ `write_atomic`이 **반드시**
                // `.objects/.tmp-<uniq>`를 만들고 `<sha>`로 rename한다 = **소멸의 원천**.
                let mut body = format!("f14-original-repro-w{w}-r{r}-").into_bytes();
                body.resize(200, b'.');
                in_flight.fetch_add(1, Ordering::SeqCst);
                let out = s.put("b", &key, "application/octet-stream", "u", body).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
                out.unwrap();
            }
        }));
    }
    for h in hs {
        h.await.unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    rec.await.unwrap();
    obs.await.unwrap();

    let (passes, overlapped) = (
        passes.load(Ordering::SeqCst),
        overlapped.load(Ordering::SeqCst),
    );
    let (errs, errs_notfound) = (
        errs.load(Ordering::SeqCst),
        errs_notfound.load(Ordering::SeqCst),
    );
    let (scans, temps_seen) = (scans.load(Ordering::SeqCst), temps_seen.load(Ordering::SeqCst));
    let (vanished, vanished_during_pass) = (
        vanished.load(Ordering::SeqCst),
        vanished_during_pass.load(Ordering::SeqCst),
    );
    let first_err = first_err.lock().unwrap().clone().unwrap_or_default();

    // **put이 만든 temp의 소멸 수의 하한.** reconcile 자신이 만든 `.tmp-`(gc-pending)의 상계는
    // `passes`다(완주 패스당 최대 1개) ⇒ 그만큼을 **통째로 깎아** 남는 것만 센다.
    let put_temp_vanishes = vanished_during_pass.saturating_sub(passes);

    // 실행 원문(요약 금지) — `--nocapture`로 보인다.
    eprintln!(
        "REPRO WITNESS puts={TOTAL_PUTS} passes={passes} overlapped_passes={overlapped} \
         scans={scans} temps_seen={temps_seen} vanished={vanished} \
         vanished_during_pass={vanished_during_pass} put_temp_vanishes={put_temp_vanishes} \
         pass_errs={errs} pass_errs_notfound={errs_notfound} first_err=[{first_err}]"
    );

    // ── 자기검증 ① 반복 증인 — 루프가 **정말로 돌았다** ────────────────────────────────────
    // 이것이 없으면 `errs == 0`은 **아무것도 증명하지 않는다**(패스를 한 번도 안 돌렸어도 0이다).
    assert!(
        overlapped >= MIN_OVERLAPPED_PASSES,
        "반복 증인이 공허하다 — put이 in-flight인 동안 완주한 reconcile 패스가 {overlapped}회뿐이다\
         (요구: ≥{MIN_OVERLAPPED_PASSES}). 총 패스={passes}. 루프가 돌지 않았다면 `pass_errs == 0`은 \
         버그의 부재를 증명하지 않는다."
    );

    // ── 자기검증 ② 레이스 증인 — **소멸 레이스가 실제로 일어났다** ────────────────────────
    // `.objects`의 `.tmp-`를 치우는 주체는 rename뿐이고(grace 1h ⇒ temp 회수 무발화), `.objects`에
    // `.tmp-`를 만드는 주체는 **put(put당 1개)** 과 **reconcile의 gc-pending(완주 패스당 ≤1개)** 둘뿐이다.
    // ⇒ `vanished_during_pass − passes`가 0보다 크면 **put이 만든 temp가 스냅샷 이후 사라졌고,
    //    그때 reconcile 패스가 in-flight였다**는 것이 **산술적으로 강제된다**(§자기공격).
    assert!(
        put_temp_vanishes >= MIN_PUT_TEMP_VANISHES,
        "레이스 증인이 공허하다 — put이 만든 temp의 소멸(하한)이 {put_temp_vanishes}건뿐이다\
         (요구: ≥{MIN_PUT_TEMP_VANISHES}). 패스 in-flight 중 소멸={vanished_during_pass} · \
         전체 소멸={vanished} · 패스={passes}(= reconcile 자신의 gc-pending temp 상계) · \
         관측된 `.tmp-`={temps_seen} · 스캔={scans}. 소멸이 일어나지 않았다면 이 테스트는 \
         **버그를 밟지도 않은 것**이고 `pass_errs == 0`은 공허하다. \
         재현율을 높여라(put 수 · 라운드 · 루프 횟수)."
    );

    // ── 관측 행동(단일 플립) — 사라진 항목은 **그 항목만** 건너뛰고 패스는 완주해야 한다 ────
    assert_eq!(
        errs, 0,
        "PASS ABORTED — 동시 put과 경합하는 reconcile 패스가 `Err`로 중단됐다({errs}/{passes} 패스, \
         그중 NotFound={errs_notfound}). 스냅샷 이후 사라진 `.objects` 항목(동시 `write_atomic`이 \
         `.tmp-<uniq>` → `<sha>`로 rename해 치운 그 항목)은 **그 항목만 건너뛰고** 패스는 완주해야 \
         한다(F-14). 첫 에러: {first_err} — `NotFound`(ENOENT)가 곧 소멸의 물증이다: 패스 자신이 \
         스냅샷에 잡아 둔 항목을 stat하다 밟았다. 관측자가 독립적으로 센 소멸도 {vanished}건이다\
         (그중 패스 in-flight 중 {vanished_during_pass}건 · put이 만든 temp의 소멸 하한 \
         {put_temp_vanishes}건)."
    );

    // ── 원 repro의 나머지 절반 — 40×{ROUNDS}개 중첩 키가 **전부 생존**하고 정합해야 한다 ────
    let listed = s.list("b").await.unwrap();
    assert_eq!(listed.len(), TOTAL_PUTS, "중첩 키가 reconcile에서 유실됨");
    for (k, _) in &listed {
        let (m, b) = s.get_bytes("b", k).await.unwrap();
        assert_eq!(hex_sha(&b), m.sha256, "메타-데이터 desync: {k}");
    }
}
