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
