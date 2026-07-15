//! **W11 — 프로덕션 진입점 증인 (P-5).**
//!
//! ## 왜 이 증인이 따로 있어야 하는가
//!
//! 구조적 불변식(*"`Entry`가 유일한 통로다"*)만으로는 **한 가지**를 못 잡는다:
//! *"헬퍼는 초록인데 **프로덕션이 그 헬퍼를 안 쓴다**."* `recover_graves_from`을 **직접** 부르는
//! 증인은 그 함수가 옳다는 것만 증명할 뿐, **`PassGuard::begin`이 그것을 부른다**는 것은 증명하지
//! 않는다. ⇒ 이 증인은 **`run_once_at_for_test`를 spawn해 `PassGuard::begin` → `recover_graves`**
//! 라는 **진짜 프로덕션 경로**를 탄다.
//!
//! ## seam — 9번째 훅 `pre_recover_grave`
//!
//! 그 구간(`begin` 안 · `collect_referenced` **이전**)에서 발화하는 훅이 **하나도 없었다** ⇒ 결정적
//! 배리어를 꽂을 방법이 없었다. 훅은 **`grave_sha` 필터와 `file_type` 검사 뒤, `blob_intact` 판정 앞**
//! ⇒ **무덤 항목 하나당 정확히 한 번**, **remove/rename 어느 분기로 갈 항목이든 예외 없이** 발화한다.
//! 프로덕션에서는 항상 `None` ⇒ no-op ⇒ **관측 행동 변화 0**(`pins.rs`의 `Hooks` doc이 논증을 소유한다).
//!
//! ## 무대 — 두 분기를 **둘 다** 덮는다
//!
//! * **R 계급**(2개) — 정본 blob **부재** ⇒ `blob_intact = false` ⇒ **rename 분기**
//!   (`Entry::rename_durable_to`).
//! * **K 계급**(2개) — 정본 blob **무손상** ⇒ `blob_intact = true` ⇒ **remove 분기**(`Entry::remove`).
//!
//! 첫 발화에서 park하고, **파킹된 것을 뺀 무덤 3개를 삭제**한 뒤 재개한다 ⇒ 그 셋은 `remove`/`rename`
//! 에서 ENOENT를 맞고 **`Gone` ⇒ skip**된다. 오늘의 코드라면 **`?`가 패스를 죽인다**(범인 표 ⑦).
//!
//! ⚠ **파킹된 무덤이 R인지 K인지는 readdir 순서가 정한다** ⇒ 기대값을 **관측된 계급으로부터 계산**한다.
//! 그것이 정직한 결정성이다 — 순서를 가정하지 않고, **무엇이 파킹됐는지 훅에게 물어서** 판정한다.

use super::*;

/// 무덤 하나의 계급.
const R_SEEDS: [&[u8]; 2] = [b"w11-rename-branch-0", b"w11-rename-branch-1"];
const K_SEEDS: [&[u8]; 2] = [b"w11-remove-branch-0", b"w11-remove-branch-1"];

/// **모든 발화 sha를 기록**하고 **첫 발화에서만** park한다.
/// 규율: **`send(도착)` ≺ `park`**(뒤집으면 신호가 영영 오지 않는다) · 해제는 `notify_one()`.
fn record_all_park_first(
    tx: UnboundedSender<String>,
    gate: Arc<Notify>,
    seen: Arc<Mutex<Vec<String>>>,
) -> AsyncHook {
    on_first(
        move |sha| seen.lock().unwrap().push(sha.to_owned()),
        move |sha| {
            let (tx, gate) = (tx.clone(), gate.clone());
            Box::pin(async move {
                tx.send(sha).expect("pre_recover_grave 도착 신호");
                gate.notified().await;
            })
        },
    )
}

/// **W11 — 프로덕션 진입점(`PassGuard::begin` → `recover_graves`)이 사라진 무덤을 건너뛰고 완주한다.**
///
/// ⓐ 무덤 4개(R 2 · K 2) · ⓑ 패스 spawn · ⓒ **첫 `pre_recover_grave` 도착 await**(무덤 루프에 서 있다)
/// ⓓ park 중 **파킹된 것을 뺀 무덤 3개 삭제** · ⓔ 해제
/// ⓕ 그 셋의 `remove`/`rename_durable_to`가 ENOENT → **`Gone` ⇒ skip** → 패스 **완주**.
#[tokio::test]
async fn recover_graves_production_seam_survives_vanished_graves() {
    let (tx, mut rx) = unbounded_channel::<String>();
    let gate = Arc::new(Notify::new());
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let hooks = Hooks {
        pre_recover_grave: Some(record_all_park_first(tx, gate.clone(), seen.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);
    let objects = root.join(".objects");
    tokio::fs::create_dir_all(&objects).await.unwrap();

    // ── ⓐ 무대 ────────────────────────────────────────────────────────────────────────────
    // R 계급 — 무덤만 있고 **정본 blob은 없다** ⇒ rename 분기. 내용은 **자기 sha와 정합**해야 한다
    //          (복원된 blob을 엔트리 루프가 재검증한다 — 어긋나면 격리되어 무대가 오염된다).
    let mut r_shas = Vec::new();
    for seed in R_SEEDS {
        let sha = hex_sha(seed);
        tokio::fs::write(s.layout().grave_path(&sha), seed)
            .await
            .unwrap();
        r_shas.push(sha);
    }
    // K 계급 — 무덤 **과** 무손상 정본이 **둘 다** 있다 ⇒ remove 분기.
    let mut k_shas = Vec::new();
    for seed in K_SEEDS {
        let sha = hex_sha(seed);
        tokio::fs::write(s.layout().grave_path(&sha), seed)
            .await
            .unwrap();
        tokio::fs::write(s.blob_path(&sha), seed).await.unwrap(); // 정본 **무손상**
        k_shas.push(sha);
    }
    let all: Vec<String> = r_shas.iter().chain(k_shas.iter()).cloned().collect();
    assert_eq!(grave_names(&root).await.len(), 4, "무대: 무덤 4개");

    // ── ⓑ spawn ───────────────────────────────────────────────────────────────────────────
    let s2 = s.clone();
    let t0 = SystemTime::now();
    let mut gc: PassHandle =
        tokio::spawn(async move { reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE).await });

    // ── ⓒ 도착 await — **무덤 루프 안**에 서 있다(`begin`을 지났다는 직접 증거) ──────────────
    let parked = arrived(&mut rx).await;
    assert!(
        all.contains(&parked),
        "파킹된 sha는 심은 무덤 중 하나다: {parked}"
    );
    probe_still_waiting(&mut gc).await;

    // ⚠ **파킹된 것의 계급이 기대값을 정한다**(readdir 순서를 가정하지 않는다).
    let parked_is_k = k_shas.contains(&parked);

    // ── ⓓ park 중 **나머지 무덤 3개 삭제** — 스냅샷 이후 소멸 ─────────────────────────────
    for sha in all.iter().filter(|s| **s != parked) {
        let g = s.layout().grave_path(sha);
        tokio::fs::remove_file(&g).await.unwrap_or_else(|e| {
            panic!("자기검증: 아직 처리되지 않은 무덤({sha})은 park 시점에 디스크에 있어야 한다: {e:?}")
        });
    }
    assert_eq!(
        grave_names(&root).await.len(),
        1,
        "자기검증: 파킹된 무덤 하나만 남았다"
    );

    // ── ⓔ 해제 → ⓕ 완주 ──────────────────────────────────────────────────────────────────
    gate.notify_one();
    let stats = finish_pass(gc).await;

    // ── 자기검증 ④ — 훅 발화 sha 집합 == 심은 무덤 **전부** ────────────────────────────────
    //    (FS-독립: `file_type()`이 소멸한 무덤에도 **캐시된 `Ok`** 를 주므로 `Gone`으로 빠지지 않는다
    //     ⇒ 삭제된 셋도 훅을 발화시킨다 ⇒ 루프가 **끝까지 돌았다**는 증거다.)
    let mut fired = seen.lock().unwrap().clone();
    fired.sort();
    let mut want = all.clone();
    want.sort();
    assert_eq!(
        fired, want,
        "`pre_recover_grave`는 무덤 항목 **하나당 정확히 한 번** 발화한다(두 분기 모두)"
    );

    // ── 사후 디스크 상태 ──────────────────────────────────────────────────────────────────
    assert!(
        grave_names(&root).await.is_empty(),
        "무덤은 하나도 남지 않는다(처리됐거나 사라졌다)"
    );
    for sha in &k_shas {
        assert!(
            tokio::fs::try_exists(s.blob_path(sha)).await.unwrap(),
            "K 계급의 정본은 무손상이었다 ⇒ 살아남는다"
        );
    }
    // 파킹된 R은 **복원**된다. 파킹되지 않은 R은 무덤이 사라졌으므로 정본도 없다.
    for sha in &r_shas {
        let restored = tokio::fs::try_exists(s.blob_path(sha)).await.unwrap();
        assert_eq!(
            restored,
            *sha == parked,
            "R 계급은 **파킹된 것만** 정본으로 복원된다(나머지는 무덤이 사라졌다): sha={sha}"
        );
    }

    // ── 원장 · 전수 stats ─────────────────────────────────────────────────────────────────
    assert!(
        tokio::fs::try_exists(s.layout().gc_pending_path())
            .await
            .unwrap(),
        "완주한 패스는 `.gc-pending.json`을 발행한다(중단된 루프는 여기 도달하지 못한다)"
    );
    // 엔트리 루프가 보는 blob = K 2개 + (파킹된 R이 복원됐다면 1개). 전부 **최초 관측** ⇒ tombstone만.
    let expect_pending = k_shas.len() + usize::from(!parked_is_k);
    assert_eq!(
        stats,
        ReconcileStats {
            referenced: 0,              // 포인터를 만들지 않았다 — **구조적** 0
            gc_deleted: 0,              // tombstone을 심지 않았다 ⇒ 최초 관측에 그친다
            gc_pending: expect_pending, // K 2개 + 복원된 R(파킹된 것이 R일 때만)
            temps_deleted: 0,
            quarantined: 0, // 무덤 내용이 자기 sha와 정합하므로 격리 없음
        },
        "사라진 무덤 3개는 **건너뛰고** 파킹된 하나는 정상 처리된다 (parked_is_k={parked_is_k})"
    );
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  **W13-G (재작성) — remove 분기의 소멸 창은 *프로덕션만*의 증거다.**
//
//  구 통합 무대(`tests/reconcile_vanishing_entries.rs::phase_g_...`)는 **동시성 랑데부**였다:
//  `run_once`를 spawn하고 `.gc-grave-*` 개수가 줄기를 **busy-spin**으로 기다린 뒤 남은 무덤을 외부에서
//  지웠다. 그 조율은 신뢰성이 없어 green.sha에서 **5/5 결정적으로 RED**였다(`K_KEEP의 무덤이 남아 있다`
//  · line 372) — 프로덕션은 옳은데(무덤 24개를 홀로 돌리면 `grave_count → 0`) **무대가 K_KEEP을 처리하기
//  전에 단언에 도달**했고, 어떤 계측을 붙여도(stderr·파일) 타이밍이 밀려 **초록으로 뒤집히는** 하이젠버그였다.
//
//  ⇒ 여기서는 **9번째 훅 `pre_recover_grave`** 로 **결정적 park**를 걸어 랑데부를 대체한다(SPIN_BUDGET 없음).
//  훅은 `PassGuard::begin → recover_graves`라는 **진짜 프로덕션 경로**에서, **무덤 항목 하나당 정확히 한 번,
//  두 분기(rename·remove) 이전**에 발화한다(prod = `None` ⇒ no-op). **첫 발화에서 park**하면 프로덕션은
//  **grave[0]의 파일 연산 직전**에 서고, 스냅샷은 이미 고정되어 **모든 무덤이 아직 디스크에 있다** ⇒
//  그 park 창에서 우리가 지우는 것은 **readdir 순서와 무관하게 100% 결정적**이다.
// ══════════════════════════════════════════════════════════════════════════════════════════

/// **W13-G.** 세 계급을 **한 무대**에서 — 전부 **날조 불가능한 관측치**로 — 판정한다.
///
/// * **`K_KILL`(remove 분기의 *소멸 창*).** 무손상 정본 + **쓰레기** 무덤. park 중 **우리가 그 무덤을
///   지운다** ⇒ 재개하면 프로덕션은 `blob_intact = true`(정본은 안 건드렸다) → **remove 분기** → 지워진
///   무덤에 `e.remove()` → **`Seen::Gone` → `else { continue }`**. 훅이 그 sha로 **발화했다는 것**이
///   *"프로덕션이 그 무덤 항목에 도달했다"*의 직접 증거다 ⇒ `e.remove()?`를 raw `?`로 되돌리는 뮤턴트는
///   ENOENT에서 패스를 죽여 **RED**(remove 분기 소멸 창의 결정적 커버리지).
/// * **`K_KEEP`(remove 분기의 *선택*·*완주* · **M-REMOVE-NOOP 킬**).** 무손상 정본 + 쓰레기 무덤. 무덤을
///   **우리는 절대 건드리지 않는다** ⇒ 사라졌다면 **프로덕션의 remove 분기가 지운 것**이다. 무덤 루프를
///   no-op으로 만드는 뮤턴트는 **여기서 무덤이 남아** RED가 된다(테스트가 만든 상태가 아니라 프로덕션만이
///   만들 수 있는 산출물).
/// * **`R`(rename 분기).** 정본 **부재** ⇒ `blob_intact = false` → `rename(무덤 → 정본)`. **survivor**(안
///   지운 R)는 정본으로 **복원**되고(정본 blob 등장), park 중 지운 R은 **escaped**(정본·무덤 둘 다 부재)로
///   rename 분기의 소멸 창을 덮는다.
/// * **K 무덤 내용 = *쓰레기*(정본 sha와 다르다).** remove 분기는 무덤을 *지우기만* 하므로 정본은 **바이트
///   그대로**다. rename으로 잘못 가는 뮤턴트는 쓰레기를 정본에 덮어써 ⇒ **바이트 동일성**과 `quarantined==0`이
///   **둘 다 RED**가 된다(분기 *선택*을 핀한다).
#[tokio::test]
async fn phase_g_recover_graves_survives_vanishing_graves() {
    const R: usize = 3;
    /// park 중 **우리가** 무덤을 지운다 ⇒ remove 분기의 **소멸 창**.
    const K_KILL: usize = 2;
    /// **절대** 건드리지 않는다 ⇒ **프로덕션만이** 그 무덤을 없앨 수 있다(선택 + 완주 + M-REMOVE-NOOP 킬).
    const K_KEEP: usize = 2;

    /// K 무덤의 내용 — **정본과 다르다**(sha 불일치). rename으로 잘못 가면 정본이 오염된다.
    fn garbage(sha: &str) -> Vec<u8> {
        format!("w13g-GARBAGE-must-never-reach-a-blob-{sha}").into_bytes()
    }
    async fn on_disk(p: &std::path::Path) -> bool {
        tokio::fs::try_exists(p).await.unwrap()
    }

    let (tx, mut rx) = unbounded_channel::<String>();
    let gate = Arc::new(Notify::new());
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let hooks = Hooks {
        pre_recover_grave: Some(record_all_park_first(tx, gate.clone(), seen.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);
    let objects = root.join(".objects");
    tokio::fs::create_dir_all(&objects).await.unwrap();

    // ── 무대 ───────────────────────────────────────────────────────────────────────────────
    // R — 무덤만(정본 **부재**) ⇒ rename 분기. 무덤 내용은 **자기 sha와 정합**해야 한다(복원된 정본을
    //     엔트리 루프가 재검증한다 — 어긋나면 격리되어 무대가 오염된다).
    let mut r_shas = Vec::new();
    for i in 0..R {
        let seed = format!("w13g-rename-{i}");
        let sha = hex_sha(seed.as_bytes());
        tokio::fs::write(s.layout().grave_path(&sha), seed.as_bytes())
            .await
            .unwrap();
        r_shas.push(sha);
    }
    // K_KILL · K_KEEP — 무손상 정본 **과** 쓰레기 무덤이 둘 다 있다 ⇒ blob_intact = true ⇒ remove 분기.
    //   `k_seed[sha]`는 정본의 **기대 바이트**다(사후 바이트-동일성 단언에 쓴다).
    let mut k_kill = Vec::new();
    let mut k_keep = Vec::new();
    let mut k_seed: HashMap<String, Vec<u8>> = HashMap::new();
    for (dst, tag, n) in [
        (&mut k_kill, "kill", K_KILL),
        (&mut k_keep, "keep", K_KEEP),
    ] {
        for i in 0..n {
            let seed = format!("w13g-remove-{tag}-{i}").into_bytes();
            let sha = hex_sha(&seed);
            tokio::fs::write(s.blob_path(&sha), &seed).await.unwrap(); // 정본 = **무손상**
            tokio::fs::write(s.layout().grave_path(&sha), garbage(&sha))
                .await
                .unwrap(); // 무덤 = **쓰레기**
            k_seed.insert(sha.clone(), seed);
            dst.push(sha);
        }
    }
    let all: Vec<String> = r_shas
        .iter()
        .chain(k_kill.iter())
        .chain(k_keep.iter())
        .cloned()
        .collect();
    assert_eq!(
        grave_names(&root).await.len(),
        R + K_KILL + K_KEEP,
        "무대 자기검증: 무덤 전부"
    );

    // ── spawn → 도착 await(무덤 루프 안 · `begin`을 지났다) → 아직 대기 확인 ────────────────
    let s2 = s.clone();
    let t0 = SystemTime::now();
    let mut gc: PassHandle =
        tokio::spawn(async move { reconcile::run_once_at_for_test(&s2, t0, GRACE, SETTLE).await });
    let parked = arrived(&mut rx).await;
    assert!(
        all.contains(&parked),
        "파킹된 sha는 심은 무덤 중 하나다: {parked}"
    );
    probe_still_waiting(&mut gc).await;

    // ── park 중 소멸(결정적) ─────────────────────────────────────────────────────────────────
    // 프로덕션은 grave[0] 처리 **직전**에 서 있다 ⇒ 스냅샷은 고정 · **모든 무덤이 아직 디스크에 있다**.
    //  · K_KILL 무덤 **전부** 삭제 ⇒ 재개 시 프로덕션 `e.remove()`가 `Gone`(remove 분기 소멸 창).
    //  · R 무덤은 **survivor 하나만 남기고**(= rename 복원 증거) 나머지 삭제 ⇒ rename 분기 소멸 창(escaped).
    //  · ⚠⚠ **K_KEEP 무덤은 *절대* 건드리지 않는다** — 그것을 없앨 수 있는 건 **프로덕션뿐**이다.
    let r_survivor = r_shas[0].clone();
    let mut killed_k = 0usize;
    for sha in &k_kill {
        tokio::fs::remove_file(s.layout().grave_path(sha))
            .await
            .unwrap_or_else(|e| {
                panic!("자기검증: park 시점 K_KILL 무덤은 디스크에 있어야 한다: {sha}: {e:?}")
            });
        killed_k += 1; // 프로덕션은 파킹돼 미처리 ⇒ 우리가 반드시 이긴다 ⇒ 그 무덤의 `e.remove()`는 `Gone`을 본다
    }
    let mut escaped_r = 0usize;
    for sha in r_shas.iter().filter(|x| **x != r_survivor) {
        tokio::fs::remove_file(s.layout().grave_path(sha))
            .await
            .unwrap_or_else(|e| {
                panic!("자기검증: park 시점 R 무덤은 디스크에 있어야 한다: {sha}: {e:?}")
            });
        escaped_r += 1;
    }
    assert!(killed_k >= 1, "무대 자기검증: killed_k>=1 (K_KILL={K_KILL})");
    assert!(escaped_r >= 1, "무대 자기검증: escaped_r>=1 (R={R})");

    // ── 해제 → 완주 ──────────────────────────────────────────────────────────────────────────
    gate.notify_one();
    let stats = finish_pass(gc).await;

    // ── 자기검증 — 훅이 무덤 **전부**에 발화했다(루프가 끝까지 돌았다) ────────────────────────
    //    (FS-독립: `file_type()`이 소멸한 무덤에도 **캐시된 `Ok`** 를 주므로 삭제된 무덤도 훅을 발화시킨다
    //     ⇒ 프로덕션이 그 항목에 **도달했다**는 증거다.)
    let mut fired = seen.lock().unwrap().clone();
    fired.sort();
    let mut want = all.clone();
    want.sort();
    assert_eq!(
        fired, want,
        "`pre_recover_grave`는 무덤 항목 하나당 정확히 한 번 발화한다(세 계급 전부)"
    );

    // ── 사후 디스크 — 날조 불가능한 프로덕션 증거 ────────────────────────────────────────────
    // ★ **K_KEEP — M-REMOVE-NOOP 킬.** 우리는 건드리지 않았다 ⇒ 사라졌다면 **프로덕션 remove 분기가 지운 것**.
    for sha in &k_keep {
        assert!(
            !on_disk(&s.layout().grave_path(sha)).await,
            "K_KEEP의 무덤이 남아 있다 — 우리는 건드리지 않았으므로 **프로덕션이 remove 분기를 타지 않았다**. \
             sha={sha}"
        );
    }
    // ★ **바이트 동일성** — remove 분기는 무덤을 *지우기만* 한다 ⇒ 정본은 **그대로**다. rename으로 잘못
    //   가는 뮤턴트는 쓰레기 무덤을 정본에 덮어써 ⇒ 여기서 RED(그리고 `quarantined`도).
    for sha in k_kill.iter().chain(k_keep.iter()) {
        let got = tokio::fs::read(s.blob_path(sha))
            .await
            .unwrap_or_else(|e| panic!("K의 정본은 살아남는다. sha={sha}: {e}"));
        assert_eq!(
            &got, &k_seed[sha],
            "K의 정본이 **오염**됐다 — 쓰레기 무덤이 정본을 덮어썼다 = remove 분기가 아니라 \
             **rename 분기**를 탔다. sha={sha}"
        );
    }
    // ★ **R** — survivor는 rename 분기로 **복원**(정본 blob 등장) · 지운 R은 **escaped**(정본·무덤 둘 다 부재).
    assert!(
        on_disk(&s.blob_path(&r_survivor)).await,
        "R survivor는 rename 분기로 정본이 복원된다: {r_survivor}"
    );
    for sha in r_shas.iter().filter(|x| **x != r_survivor) {
        assert!(
            !on_disk(&s.blob_path(sha)).await && !on_disk(&s.layout().grave_path(sha)).await,
            "park 중 지운 R은 escaped다(정본·무덤 둘 다 부재): {sha}"
        );
    }
    // 무덤은 하나도 남지 않는다. ⚠ 이 단언의 힘은 **K_KEEP 덕분**이다 — R·K_KILL만이면 우리가 전부 지웠으므로
    // **항상 참**(= 공허)이었다.
    assert!(
        grave_names(&root).await.is_empty(),
        "무덤은 하나도 남지 않는다(처리됐거나 · park 중 지워졌다)"
    );

    // ── 원장 · 전수 stats ────────────────────────────────────────────────────────────────────
    assert!(
        tokio::fs::try_exists(s.layout().gc_pending_path())
            .await
            .unwrap(),
        "완주한 패스는 `.gc-pending.json`을 발행한다(중단된 루프는 여기 도달하지 못한다)"
    );
    // 엔트리 루프가 보는 blob = K_KILL 정본 + K_KEEP 정본 + 복원된 R survivor(1). 전부 **최초 관측** ⇒ tombstone만.
    let expect_pending = K_KILL + K_KEEP + 1;
    assert_eq!(
        stats,
        ReconcileStats {
            referenced: 0,              // 포인터를 만들지 않았다 — **구조적** 0
            gc_deleted: 0,              // tombstone을 심지 않았다 ⇒ 최초 관측에 그친다
            gc_pending: expect_pending, // K 정본 전부 + 복원된 R survivor
            temps_deleted: 0,
            quarantined: 0, // 무덤 내용이 정본에 **닿지 않았다** ⇒ 격리 0
        },
        "W13-G 항등식: 원장 = K 정본 전부 + 복원된 R survivor. killed_k={killed_k} escaped_r={escaped_r}"
    );
}
