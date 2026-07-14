//! **W13 — 통합 증인 (Phase E / G / T).** `tests/`의 독립 바이너리 · **프로덕션 공개 API만** 쓴다.
//!
//! ## W13-0. 왜 `tests/`여야 하는가 — **`cfg(test)` 없이 lib를 링크한다**
//!
//! 이 바이너리는 **프로덕션 빌드의 lib**를 링크한다(`cfg!(test) == false`) ⇒ **모든 조건부 뮤턴트의
//! *프로덕션 팔*을 탄다**:
//! * `if cfg!(test) { 올바름 } else { legacy }` (**M19** — `Err(e) if e.kind()==NotFound && cfg!(test)`)
//! * **훅-존재 가드** — `with_hooks`가 `#[cfg(test)]`이므로 **여기서는 훅을 심을 수 없다**
//!   ⇒ 훅이 있을 때만 옳게 도는 구현은 여기서 **legacy 팔**로 떨어진다
//! * `#[path]`/`include!`/스캔 밖 파일 — **컴파일된 산출물의 행동**을 보므로 미끼가 무의미하다
//!
//! **남는 편향 술어는 정확히 하나**(`cfg!(debug_assertions)`/프로파일/env) ⇒ **보상 통제 = `--release`
//! 실행**(B-1 · acceptance의 `--release` 두 줄).
//!
//! ## W13-2. 랑데부 — 훅 없이 "스냅샷 이후"를 만든다
//!
//! 프로덕션이 **스스로 만드는 온디스크 관측치**를 신호로 쓴다:
//! * `.corrupt/<name>` 등장 ⇒ **엔트리 루프가 이미 돌고 있다**
//! * `.gc-grave-*` 개수 감소 ⇒ **복구 루프가 돌고 있다**
//!
//! **절차**: 카나리아·victim을 **spawn 전에** 심고 → 패스 spawn → 관측치까지 유계 busy-spin →
//! **아직 디스크에 있는 victim을 지운다** → join → `Ok(stats)` + **사후-디스크 항등식** +
//! **전수 `assert_eq!`** + **자기검증 하한(`MIN_STEPS_*`)**.
//!
//! > ⚠⚠ **회계는 *우리 `remove_file`의 반환값*이 아니라 *패스 종료 후 디스크 상태*로 한다**
//! > (실측이 강제했다 — 초안대로 짠 Phase E는 `--release`에서 **~6% RED**였다: 우리의 `unlink`와
//! > 패스의 격리 `rename`이 **둘 다 성공할 수 있다**). **예외는 unlink vs unlink 짝**(Phase T)뿐이며
//! > 거기서는 **정확히 하나만 성공**하므로 반환값을 그대로 써도 된다.
//!
//! > ⚠⚠ **심는 순서가 load-bearing이다**(tmpfs·ext4는 **삽입 순서**로 돌려준다) ⇒
//! > **카나리아 ≺ victim**(E) · **카나리아 ≺ ballast ≺ temp**(T). 그래서 **`MIN_STEPS_*`가 반드시 함께
//! > 있어야 한다** — 창을 못 밟으면 **RED로 소리친다**(조용한 초록이 불가능하다).
//!
//! ## W13-3. 결정성
//!
//! `Err`를 볼 수 있는 `?`는 전수 **양성**이다: `try_exists(.objects)`(지우지 않는다) ·
//! `read_dir`/`next_entry`(항목 변경은 getdents 에러가 아니다) · `collect_referenced`(**포인터 0개**
//! ⇒ **F-31 도달 불가** ⇒ W13의 GREEN은 **FS 무관**) · `mkdir_p_durable`/`fsync_dir`/`write_atomic`
//! (`.corrupt`/`.objects`를 지우지 않는다) · **루프-후 가드**(`.objects`가 **살아 있다** ⇒ `Ok(dir)`) ·
//! `try_exists(blob_path)`(부재 = `Ok(false)`) · **소멸 항목**(= 정확히 픽스가 흡수하는 것).
//!
//! **가드의 영향: 없다.** 세 페이즈는 **`.objects`를 지우지 않는다**(항목만 지운다) ⇒ 가드는
//! `metadata` = `Ok(dir)`를 보고 **통과**한다.

use files::layout::Layout;
use files::store::reconcile::{self, ReconcileStats};

mod common;
use common::{f14_store, hex_sha};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

const SETTLE: Duration = Duration::from_secs(30);
/// 아무것도 만료시키지 않는 grace(Phase E/G).
const KEEP: Duration = Duration::from_secs(3600);
/// 랑데부 예산 — 넘으면 **창을 못 밟았다**는 뜻이고, `MIN_STEPS_*`가 그것을 RED로 만든다.
const SPIN_BUDGET: Duration = Duration::from_secs(10);

/// 내용이 이름의 sha와 **정합**한 blob(격리되지 않는다).
fn plant_intact(l: &Layout, seed: &[u8]) -> String {
    let sha = hex_sha(seed);
    std::fs::write(l.blob_path(&sha), seed).unwrap();
    sha
}

/// 이름 = `sha(seed)` · 내용 = **다른 바이트** ⇒ **비트로트** ⇒ 격리 대상(`.corrupt/<sha>` 등장).
fn plant_rotten(l: &Layout, seed: &[u8], bytes: &[u8]) -> String {
    let sha = hex_sha(seed);
    std::fs::write(l.blob_path(&sha), bytes).unwrap();
    sha
}

/// `.corrupt`에 무엇이든 나타날 때까지 유계 대기 = **엔트리 루프가 돌고 있다**.
async fn spin_until_entry_loop_running(l: &Layout) {
    let deadline = Instant::now() + SPIN_BUDGET;
    while Instant::now() < deadline {
        if std::fs::read_dir(l.corrupt_dir())
            .map(|rd| rd.count() > 0)
            .unwrap_or(false)
        {
            return;
        }
        tokio::task::yield_now().await;
    }
}

/// `.objects` 직속 무덤 개수. ⚠ 접두사는 **raw 리터럴**이다(ADR-0001 — layout 상수를 경유시키면
/// 동어반복이 되어 접두사 드리프트를 한 곳에서도 못 잡는다).
fn grave_count(l: &Layout) -> usize {
    std::fs::read_dir(l.objects_dir())
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.file_name().to_string_lossy().starts_with(".gc-grave-"))
                .count()
        })
        .unwrap_or(0)
}

fn grave_path(l: &Layout, sha: &str) -> std::path::PathBuf {
    l.objects_dir().join(format!(".gc-grave-{sha}"))
}

fn exists(p: &Path) -> bool {
    std::fs::exists(p).unwrap_or(false)
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  **Phase E — 엔트리 루프.** 스냅샷 이후 blob·temp가 사라져도 패스는 완주한다.
// ══════════════════════════════════════════════════════════════════════════════════════════

/// **W13-E.** `BALLAST` 48 × 256 KiB(루프를 늘려 창을 연다) · `CANARY` 4(비트로트 = **루프 진입 신호**) ·
/// `VICTIM_BLOB` 16(비트로트) · `VICTIM_TEMP` 8 · `ROUNDS_E` 6 · **포인터 0개**.
///
/// **항등식**: `quarantined == CANARY + (VICTIM_BLOB − ‖escaped‖)` where
/// `escaped = { v ∈ VICTIM_BLOB : .corrupt/v **부재** }`.
/// ⚠ **우리 `unlink`의 반환값을 쓰지 않는다** — 우리의 unlink와 패스의 격리 `rename`은 **둘 다 성공할
/// 수 있다**(실측: `--release`에서 ~6% RED였다). **사후 디스크 상태만이 날조 불가능한 회계다.**
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_e_entry_loop_survives_vanishing_entries() {
    const BALLAST: usize = 48;
    const BALLAST_BYTES: usize = 256 * 1024;
    const CANARY: usize = 4;
    const VICTIM_BLOB: usize = 16;
    const VICTIM_TEMP: usize = 8;
    const ROUNDS_E: usize = 6;
    const MIN_STEPS_E: usize = 6;

    let mut stepped_total = 0usize;

    for round in 0..ROUNDS_E {
        let (_d, s, l) = f14_store();

        // ⚠ **심는 순서**: 카나리아 ≺ ballast ≺ victim (삽입 순서 FS에서 카나리아가 먼저 처리된다).
        let mut canaries = Vec::new();
        for i in 0..CANARY {
            canaries.push(plant_rotten(
                &l,
                format!("e-canary-{round}-{i}").as_bytes(),
                format!("e-rot-{i}").as_bytes(),
            ));
        }
        // ballast — **내용 정합**(격리되지 않는다) · 크다(루프를 늦춘다 ⇒ 창이 넓어진다).
        // ⚠ 이름은 반드시 **내용의 sha**여야 한다 — 아니면 비트로트로 격리되어 무대가 오염된다.
        let mut ballast = Vec::new();
        for i in 0..BALLAST {
            let mut body = format!("e-ballast-{round}-{i}").into_bytes();
            body.resize(BALLAST_BYTES, b'.');
            let sha = hex_sha(&body);
            std::fs::write(l.blob_path(&sha), &body).unwrap();
            ballast.push(sha);
        }
        let mut victims = Vec::new();
        for i in 0..VICTIM_BLOB {
            victims.push(plant_rotten(
                &l,
                format!("e-victim-{round}-{i}").as_bytes(),
                format!("e-rot-v-{i}").as_bytes(),
            ));
        }
        let mut victim_temps = Vec::new();
        for i in 0..VICTIM_TEMP {
            let p = l.temp_blob_path(&format!("e-temp-{round}-{i}"));
            std::fs::write(&p, b"in flight").unwrap();
            victim_temps.push(p);
        }

        // ── spawn → 랑데부 → 소멸 ─────────────────────────────────────────────────────────
        let s2 = s.clone();
        let pass = tokio::spawn(async move { reconcile::run_once(&s2, KEEP, SETTLE).await });

        spin_until_entry_loop_running(&l).await;
        for v in &victims {
            let _ = std::fs::remove_file(l.blob_path(v)); // 아직 있으면 지운다(= 동시 rename이 하는 일)
        }
        for t in &victim_temps {
            let _ = std::fs::remove_file(t); // Temp 분기의 `metadata()` 창
        }

        let stats = pass
            .await
            .expect("패스 태스크는 패닉하지 않는다")
            .expect("PASS ABORTED — 스냅샷 이후 사라진 항목이 패스를 중단시켰다(Phase E)");

        // ── 사후-디스크 회계(날조 0) ──────────────────────────────────────────────────────
        let escaped = victims
            .iter()
            .filter(|v| !exists(&l.corrupt_dir().join(v)))
            .count();
        stepped_total += escaped;

        // 카나리아는 **항상** 격리된다(루프 진입 신호 그 자체)
        for c in &canaries {
            assert!(
                exists(&l.corrupt_dir().join(c)),
                "카나리아가 격리되지 않았다 — 랑데부 신호가 깨졌다. round={round}"
            );
        }
        // ballast는 전부 살아남아 **최초 관측** tombstone을 얻는다
        for b in &ballast {
            assert!(exists(&l.blob_path(b)), "ballast는 건드리지 않는다");
        }
        // ★ 전수 `assert_eq!` — `ReconcileStats`에 필드가 늘면 여기서 깨진다(P10)
        assert_eq!(
            stats,
            ReconcileStats {
                referenced: 0, // 커밋 포인터 **0개** ⇒ 구조적
                gc_deleted: 0, // grace 1h · 최초 관측 ⇒ 회수 없음
                gc_pending: BALLAST,
                temps_deleted: 0, // temp는 grace 이내였고, 게다가 **사라졌다**
                quarantined: CANARY + (VICTIM_BLOB - escaped),
            },
            "Phase E 항등식: 격리 = 카나리아 + (소멸하지 않은 victim). round={round} escaped={escaped}"
        );
        // 원장이 **파싱된다** = 루프가 끝까지 돌고 `write_atomic`에 도달했다
        let raw = std::fs::read(l.gc_pending_path()).expect(".gc-pending.json은 발행된다");
        let pending: std::collections::HashMap<String, u64> =
            serde_json::from_slice(&raw).expect("유효한 JSON");
        assert_eq!(pending.len(), BALLAST, "원장에는 ballast만 남는다");
    }

    // ★ **자기검증 하한** — 창을 못 밟았으면 이 증인은 아무것도 증명하지 않는다. **조용히 넘어가지 않는다.**
    assert!(
        stepped_total >= MIN_STEPS_E,
        "Phase E가 소멸 창을 충분히 밟지 못했다(stepped={stepped_total} < {MIN_STEPS_E}) — \
         랑데부가 깨졌다는 뜻이다(심는 순서? readdir 순서?). 초록으로 넘기지 않는다"
    );
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  **Phase G — `recover_graves`.** 두 분기(rename · remove)를 **둘 다** 덮는다.
// ══════════════════════════════════════════════════════════════════════════════════════════

/// **W13-G.** `R = 12`(정본 **부재** ⇒ **rename 분기**) + `K = 12`(정본 **무손상** ⇒ **remove 분기**).
/// **트리거**: `.gc-grave-*`가 `R+K` → 그 미만으로 떨어지는 순간(= 복구 루프가 **이미 돌고 있다**).
/// **랑데부 도달을 단언한다** — 못 밟았으면 이 증인은 아무것도 증명하지 않는다.
///
/// **항등식**: `gc_pending == K + (R − ‖escaped_R‖)` where
/// `escaped_R = { r ∈ R : 정본 **부재** ∧ 무덤 **부재** }` (**날조 0** — 패스가 처리했다면 정본이 **있다**).
///
/// ## ⚠⚠ 두-분기 커버리지의 회계 — **r1/R-4가 고친 세 겹의 공허함**
///
/// **① 옛 `stepped_K`(= 정본 **존재** ∧ 무덤 **부재**)로 remove-분기 커버리지를 주장했다.** 그런데 K의
/// 정본은 아무도 지우지 않고(항상 존재) 남은 무덤은 **테스트가 스스로 전부 지웠다**(항상 부재) ⇒
/// **`stepped_K ≡ K`가 프로덕션이 무엇을 했든 참** ⇒ **공허**. **② `grave_count == 0`**도 같은 이유로
/// **항상 참**이었다(우리가 전부 지웠다 · 새 무덤은 `gc_deleted == 0`이라 태어나지 않는다).
/// **③ K의 무덤 내용을 정본과 **똑같이** 심었다** ⇒ **rename 분기로 잘못 가는 뮤턴트가 바이트-동일한 정본을
/// 만들어** 디스크·stats 어디에도 흔적이 없다 ⇒ **분기 *선택*을 아무것도 핀하지 못했다.**
///
/// ## 정본 회계 — **K를 두 하위계급으로 쪼갠다**(하나의 무대가 세 가지를 한꺼번에 증명한다)
///
/// * **`K_KILL`(8) — remove 분기의 *소멸 창*.** 랑데부 후 **우리가 무덤을 지운다**.
///   무덤 삭제는 **unlink vs unlink** ⇒ 프로덕션의 `e.remove()`와 우리의 `remove_file`은 **정확히 하나만
///   이긴다** ⇒ **`killed_K` = 우리가 이긴 수 = 패스가 그 무덤을 *지우지 못한* 수**. 패스는 스냅샷의 **모든**
///   항목을 방문하고(완주를 `await`한다) · `file_type()`은 **캐시된 `d_type`**을 돌려주며(Gone 팔 미발화) ·
///   K의 정본은 **무손상**이다 ⇒ 그 항목은 반드시 **`blob_intact` → `e.remove()` → `NotFound` →
///   `Seen::Gone`**을 탔다. ⇒ **`killed_K ≥ 1`이 remove-분기 소멸 창의 *날조 불가능한* 증거다**(Phase T의 논리).
/// * **`K_KEEP`(4) — remove 분기의 *선택*과 *완주*.** 무덤을 **우리는 절대 건드리지 않는다** ⇒ 그 무덤을
///   없앨 수 있는 것은 **프로덕션뿐이다** ⇒ *"`K_KEEP`의 무덤이 사라졌다"*는 **날조 불가능한 프로덕션 증거**다
///   (무덤 루프를 no-op으로 만드는 뮤턴트는 여기서 죽는다).
/// * **K의 무덤 내용 = *쓰레기*(정본과 다르다).** remove 분기는 무덤을 **지우기만** 하므로 정본은
///   **바이트 그대로** 남는다. **rename으로 잘못 가는 뮤턴트는 쓰레기를 정본에 덮어써서** ⇒ 엔트리 루프가
///   **격리**한다 ⇒ `read(정본) == seed` **바이트 동일성**과 `quarantined == 0`이 **둘 다 RED**가 된다.
/// * **R(rename 분기) = unlink vs rename** — **둘 다 성공할 수 있다** ⇒ `killed_R`은 커버리지 조건으로
///   **쓰지 않는다**(Phase E의 그 함정 그대로). rename 분기는 **사후 디스크**의 `escaped_R`로 센다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_g_recover_graves_survives_vanishing_graves() {
    const R: usize = 12;
    /// 우리가 무덤을 지운다 ⇒ **소멸 창**(`killed_K`).
    const K_KILL: usize = 8;
    /// 우리가 **절대** 건드리지 않는다 ⇒ **프로덕션만이** 그 무덤을 없앨 수 있다(분기 선택 + 완주).
    const K_KEEP: usize = 4;
    const K: usize = K_KILL + K_KEEP;
    const ROUNDS_G: usize = 3;
    const MIN_STEPS_G: usize = 4;

    /// K 무덤의 내용 — **정본과 다르다**(sha 불일치). rename으로 잘못 가면 정본이 오염된다.
    fn k_garbage(round: usize, i: usize) -> Vec<u8> {
        format!("g-GARBAGE-must-never-reach-a-blob-{round}-{i}").into_bytes()
    }

    let mut escaped_total = 0usize;
    let mut killed_k_total = 0usize;
    let mut killed_r_total = 0usize;

    for round in 0..ROUNDS_G {
        let (_d, s, l) = f14_store();

        // R 계급 — 무덤만 있다(정본 **부재**) ⇒ `blob_intact = false` ⇒ **rename 분기**.
        // 무덤 내용은 **자기 sha와 정합**해야 한다(복원된 정본을 엔트리 루프가 재검증한다).
        let mut r_shas = Vec::new();
        for i in 0..R {
            let seed = format!("g-rename-{round}-{i}");
            let sha = hex_sha(seed.as_bytes());
            std::fs::write(grave_path(&l, &sha), seed.as_bytes()).unwrap();
            r_shas.push(sha);
        }
        // K 계급 — 무덤 **과** 무손상 정본이 둘 다 있다 ⇒ `blob_intact = true` ⇒ **remove 분기**.
        // ⚠ **무덤 내용 = 쓰레기** ⇒ rename으로 잘못 가는 뮤턴트는 정본을 오염시켜 **격리**된다.
        //   `k_seeds[i]`는 정본의 **기대 바이트**다(사후 바이트-동일성 단언에 쓴다).
        let mut k_shas = Vec::new();
        let mut k_seeds = Vec::new();
        for i in 0..K {
            let seed = format!("g-remove-{round}-{i}");
            let sha = plant_intact(&l, seed.as_bytes()); // 정본 = seed (내용 정합)
            std::fs::write(grave_path(&l, &sha), k_garbage(round, i)).unwrap(); // 무덤 = **쓰레기**
            k_shas.push(sha);
            k_seeds.push(seed.into_bytes());
        }
        // K를 두 하위계급으로 나눈다(심는 순서는 위에서 이미 끝났다 — 분할은 **인덱스**로만 한다).
        let k_kill = &k_shas[..K_KILL];
        let k_keep = &k_shas[K_KILL..];
        assert_eq!(k_keep.len(), K_KEEP, "무대 자기검증: K_KEEP");
        assert_eq!(grave_count(&l), R + K, "무대 자기검증: 무덤 {}개", R + K);

        // ── spawn → 랑데부(무덤 수 감소) → 남은 무덤 전부 삭제 ────────────────────────────
        let s2 = s.clone();
        let pass = tokio::spawn(async move { reconcile::run_once(&s2, KEEP, SETTLE).await });

        // ★ **랑데부 도달을 단언한다**(r1/R-4) — 예산을 태워 먹고 그냥 진행하면, 복구 루프가 아직
        //   시작도 안 한 무대에서 무덤을 지우는 것이 되어 창을 못 밟는다. **조용히 넘어가지 않는다.**
        let deadline = Instant::now() + SPIN_BUDGET;
        let mut rendezvous = false;
        while Instant::now() < deadline {
            if grave_count(&l) < R + K {
                rendezvous = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            rendezvous,
            "랑데부 실패 — {SPIN_BUDGET:?} 안에 무덤 수가 {}에서 줄지 않았다(복구 루프가 돌지 않았다). \
             round={round}",
            R + K
        );

        // ★ **외부 삭제 성공을 계급별로 따로 센다**(r1/R-4). K는 **unlink vs unlink** ⇒ `killed_k`가
        //   remove-분기 소멸 창의 **독립 관측치**다(위 doc 참조). R은 unlink vs rename ⇒ 커버리지에 쓰지 않는다.
        // ⚠⚠ **`k_keep`의 무덤은 절대 건드리지 않는다** — 그것을 없앨 수 있는 건 **프로덕션뿐**이다.
        let mut killed_r = 0usize;
        for sha in &r_shas {
            if std::fs::remove_file(grave_path(&l, sha)).is_ok() {
                killed_r += 1;
            }
        }
        let mut killed_k = 0usize;
        for sha in k_kill {
            if std::fs::remove_file(grave_path(&l, sha)).is_ok() {
                killed_k += 1; // **우리가** 이겼다 ⇒ 패스의 `e.remove()`는 `Gone`을 봤다
            }
        }
        killed_r_total += killed_r;
        killed_k_total += killed_k;

        let stats = pass
            .await
            .expect("패스 태스크는 패닉하지 않는다")
            .expect("PASS ABORTED — 스냅샷 이후 사라진 **무덤**이 패스를 중단시켰다(Phase G)");

        // ── 사후-디스크 회계 ──────────────────────────────────────────────────────────────
        // escaped_R: 패스가 rename하지 **못했다** ⇒ 정본이 **없다** ∧ 무덤도 없다(우리가 지웠다).
        //            패스가 처리했다면 정본이 **있다** ⇒ 두 세계는 **디스크로 구별된다**(날조 0).
        let escaped_r = r_shas
            .iter()
            .filter(|sha| !exists(&l.blob_path(sha)) && !exists(&grave_path(&l, sha)))
            .count();
        escaped_total += escaped_r;

        // ★ **`K_KEEP` — 날조 불가능한 프로덕션 증거.** 우리는 이 무덤을 **건드리지 않았다** ⇒ 사라졌다면
        //   **패스의 remove 분기가 지운 것**이다. 무덤 루프를 no-op으로 만드는 뮤턴트는 **여기서 죽는다**.
        for sha in k_keep {
            assert!(
                !exists(&grave_path(&l, sha)),
                "K_KEEP의 무덤이 남아 있다 — 우리는 건드리지 않았으므로 **패스가 remove 분기를 타지 않았다**. \
                 round={round} sha={sha}"
            );
        }
        // ★ **바이트 동일성** — remove 분기는 무덤을 *지우기만* 한다 ⇒ 정본은 **그대로**다.
        //   rename으로 잘못 가는 뮤턴트는 **쓰레기 무덤을 정본에 덮어쓴다** ⇒ 여기서 RED(그리고 quarantined도).
        for (sha, seed) in k_shas.iter().zip(k_seeds.iter()) {
            let got = std::fs::read(l.blob_path(sha))
                .unwrap_or_else(|e| panic!("K의 정본은 살아남는다. round={round} sha={sha}: {e}"));
            assert_eq!(
                &got, seed,
                "K의 정본이 **오염**됐다 — 무덤(쓰레기)이 정본을 덮어썼다 = remove 분기가 아니라 \
                 **rename 분기**를 탔다. round={round} sha={sha}"
            );
        }
        // 무덤은 하나도 남지 않는다. ⚠ 이 단언의 힘은 **`K_KEEP` 덕분에** 생긴다 — R·K_KILL만 있었다면
        // 우리가 전부 지웠으므로 **항상 참**(= 공허)이었다(r1/R-4 ②).
        assert_eq!(
            grave_count(&l),
            0,
            "무덤은 하나도 남지 않는다. round={round}"
        );
        // ★ 전수 `assert_eq!`
        assert_eq!(
            stats,
            ReconcileStats {
                referenced: 0,
                gc_deleted: 0,
                gc_pending: K + (R - escaped_r), // 살아남은 정본 = K 전부 + 복원된 R
                temps_deleted: 0,
                quarantined: 0, // 무덤 내용이 자기 sha와 정합 ⇒ 격리 0
            },
            "Phase G 항등식: 원장 = K + 복원된 R. round={round} escaped_r={escaped_r}"
        );
    }

    // ★ **두 분기를 모두 덮었음을 증인이 스스로 증명한다** — **날조 불가능한 관측치로만**(r1/R-4).
    assert!(
        escaped_total >= 1,
        "Phase G가 **rename 분기**의 소멸 창을 한 번도 밟지 못했다(escaped_R={escaped_total}) — \
         패스가 전부 처리해 버렸다는 뜻이다(랑데부가 너무 늦다). 초록으로 넘기지 않는다"
    );
    // ⚠ `killed_K`(우리 unlink의 승리 수)만이 remove 분기의 증거다 — **정본/무덤의 사후 존재 여부가
    //   아니다**(그것은 프로덕션이 무엇을 했든 참이라 **공허했다** — r1/R-4).
    assert!(
        killed_k_total >= 1,
        "Phase G가 **remove 분기**의 소멸 창을 한 번도 밟지 못했다(killed_K={killed_k_total}, \
         killed_R={killed_r_total}) ⇒ 패스의 `e.remove()`가 `Seen::Gone`을 한 번도 보지 못했다 ⇒ \
         이 증인은 **remove 분기를 raw `?`로 되돌리는 뮤턴트와 구별되지 않는다**. 초록으로 넘기지 않는다"
    );
    assert!(
        escaped_total + killed_k_total >= MIN_STEPS_G,
        "Phase G의 총 스텝이 부족하다({} < {MIN_STEPS_G}) — 랑데부가 깨졌다(escaped_R={escaped_total} · \
         killed_K={killed_k_total})",
        escaped_total + killed_k_total
    );
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  **Phase T — temp 삭제.** `temps_deleted`는 **우리가 지운 것**만 센다(`Mut-Count` 킬).
// ══════════════════════════════════════════════════════════════════════════════════════════

/// **W13-T.** `gc_grace = 0` · **CANARY(4) ≺ BALLAST_T(32) ≺ TEMPS(16)** · `ROUNDS_T = 3` ·
/// **벽시계 슬립 0**(mtime **백데이트**).
///
/// **항등식**: `temps_deleted == TEMPS − stepped_t`.
/// ⚠ 여기서만 **우리 unlink의 반환값**을 회계에 쓴다 — 이 짝은 **unlink vs unlink**이므로 **정확히
/// 하나만 성공한다**(Phase E의 unlink-vs-rename과 다르다). ⇒ `stepped_t`는 **독립 관측치**이고
/// 항등식은 **동어반복이 아니다**: 사라진 temp까지 세는 뮤턴트(**Mut-Count**)는
/// `temps_deleted == TEMPS`를 보고해 **RED**가 된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_t_temp_deletion_counts_only_what_we_deleted() {
    const CANARY: usize = 4;
    const BALLAST_T: usize = 32;
    const BALLAST_BYTES: usize = 256 * 1024;
    const TEMPS: usize = 16;
    const ROUNDS_T: usize = 3;
    const MIN_STEPS_T: usize = 1;

    /// `gc_grace = 0`에서 만료시키려면 `age.as_secs() > 0`이라야 한다 ⇒ mtime을 **과거로 민다**
    /// (벽시계 슬립 0 — `std::fs::File::set_times`는 **일반 파일**에 쓸 수 있다).
    fn backdate(p: &Path) {
        let f = std::fs::File::options().write(true).open(p).unwrap();
        let past = SystemTime::now() - Duration::from_secs(3600);
        f.set_times(std::fs::FileTimes::new().set_modified(past))
            .unwrap();
    }

    let mut stepped_total = 0usize;

    for round in 0..ROUNDS_T {
        let (_d, s, l) = f14_store();

        // ⚠ **심는 순서**: 카나리아 ≺ ballast ≺ temp
        let mut canaries = Vec::new();
        for i in 0..CANARY {
            canaries.push(plant_rotten(
                &l,
                format!("t-canary-{round}-{i}").as_bytes(),
                format!("t-rot-{i}").as_bytes(),
            ));
        }
        let mut ballast = Vec::new();
        for i in 0..BALLAST_T {
            let mut body = format!("t-ballast-{round}-{i}").into_bytes();
            body.resize(BALLAST_BYTES, b'.');
            let sha = hex_sha(&body);
            std::fs::write(l.blob_path(&sha), &body).unwrap(); // 내용 **정합**
            ballast.push(sha);
        }
        let mut temps = Vec::new();
        for i in 0..TEMPS {
            let p = l.temp_blob_path(&format!("t-temp-{round}-{i}"));
            std::fs::write(&p, b"stale in-flight").unwrap();
            backdate(&p); // ★ **만료된 temp** — 패스가 지울 대상이다
            temps.push(p);
        }

        // ── spawn → 랑데부 → 소멸 ─────────────────────────────────────────────────────────
        let s2 = s.clone();
        let pass =
            tokio::spawn(async move { reconcile::run_once(&s2, Duration::ZERO, SETTLE).await });

        spin_until_entry_loop_running(&l).await;

        // ★ **unlink vs unlink** — 정확히 하나만 성공한다 ⇒ 반환값이 곧 독립 관측치다.
        let mut stepped_t = 0usize;
        for t in &temps {
            if std::fs::remove_file(t).is_ok() {
                stepped_t += 1; // **우리가** 지웠다 ⇒ 패스는 그것을 세면 안 된다
            }
        }
        stepped_total += stepped_t;

        let stats = pass
            .await
            .expect("패스 태스크는 패닉하지 않는다")
            .expect("PASS ABORTED — 스냅샷 이후 사라진 temp가 패스를 중단시켰다(Phase T)");

        for t in &temps {
            assert!(!exists(t), "temp는 어느 쪽이든 사라진다");
        }
        for c in &canaries {
            assert!(exists(&l.corrupt_dir().join(c)), "카나리아는 격리된다");
        }
        // ★ 전수 `assert_eq!` — **`Mut-Count`의 킬 지점**
        assert_eq!(
            stats,
            ReconcileStats {
                referenced: 0,
                gc_deleted: 0,
                gc_pending: BALLAST_T,
                temps_deleted: TEMPS - stepped_t, // ★ **우리가 지운 것은 세지 않는다**
                quarantined: CANARY,
            },
            "Phase T 항등식: temps_deleted = TEMPS − (우리가 지운 수). round={round} stepped_t={stepped_t}"
        );
    }

    // ★ 자기검증 하한 — 창을 못 밟았으면 `Mut-Count`를 죽이지 못한다.
    assert!(
        stepped_total >= MIN_STEPS_T,
        "Phase T가 temp 소멸 창을 한 번도 밟지 못했다(stepped={stepped_total}) ⇒ \
         `temps_deleted == TEMPS`인 뮤턴트와 구별되지 않는다. 초록으로 넘기지 않는다"
    );
}
