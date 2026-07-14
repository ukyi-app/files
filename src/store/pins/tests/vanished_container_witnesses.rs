//! **F-14 봉인 증인 — 컨테이너 파괴 · 무덤 rename · 댕글링 심링크.**
//!
//! 회귀 증인 2개(`vanished_entry_regression` · `vanished_temp_regression`)가 **플립**을 핀한다면,
//! 이 모듈은 **플립하지 않는 것**을 핀한다 — 즉 **두 번째 플립이 없음**을.
//!
//! * **W10** — `.objects` **자체**가 파괴되면(재생성 없음) 항목별 `NotFound`가 **항목 부재로 위장**된다
//!   (실측: `lstat(missingdir/child)` = ENOENT). 루프는 전 항목을 skip하고 **완주**하지만, **루프-후
//!   컨테이너 가드**가 오늘과 같은 `Err(NotFound/2)`를 낸다 ∧ `.objects`는 **부활하지 않는다**
//!   ∧ 원장은 **발행되지 않는다**. 가드를 `write_atomic` **뒤로** 옮기는 뮤턴트(M-GUARD-AFTER)는
//!   `write_atomic`의 첫 줄 `mkdir_p_durable(parent)`가 `.objects`를 **되살리므로** 여기서 RED다.
//! * **W-GRAVE-CD-A** — 파괴가 **무덤 rename 시점**에 착지하는 세계. `grave()`의 `SourceGone` 채널은
//!   **blocking**(`rename_checked_blocking`)이고 `Entry::seen`의 채널은 **async**다 ⇒ **두 채널은 서로를
//!   못 덮는다**. 무대에 **비예약 항목을 정확히 하나만** 두어(⚠ load-bearing) `grave()`가 집계를 올리지
//!   않으면 **가드가 발화하지 않게** 만든다 ⇒ **M-NOBUMP-BLOCKING의 유일한 킬러**다.
//! * **W3** — **댕글링 blob 심링크**: 항목은 **있다**(`symlink_metadata` = `Ok`) ⇒ **skip 금지** ⇒
//!   오늘의 `Err(NotFound)`를 **바이트 보존**한다(P-1 봉인). 확인을 `metadata`(follow)로 바꾸는
//!   뮤턴트(M-FOLLOW)는 여기서 RED다.

use super::*;
use crate::store::reconcile::ReconcileStats;

/// `.objects`를 **통째로 파괴**한다(동시 rename이 아니라 **컨테이너 소멸**이다).
/// 파괴는 훅 **안에서 완주까지 await**한다 — spawn 0 ⇒ *"spawn ≠ 폴링됨"* 함정이 없다.
async fn destroy_objects(root: &std::path::Path) {
    tokio::fs::remove_dir_all(root.join(".objects"))
        .await
        .expect(".objects 파괴");
}

/// 첫 발화에서 `.objects`를 파괴한다. **모든** 발화를 계수한다(루프 완주의 증거).
fn destroy_objects_at_first(root: std::path::PathBuf, fired: Arc<AtomicUsize>) -> AsyncHook {
    on_first(
        move |_name| {
            fired.fetch_add(1, Ordering::SeqCst);
        },
        move |_name| {
            let root = root.clone();
            Box::pin(async move { destroy_objects(&root).await })
        },
    )
}

/// 위와 같되 **발화한 이름을 전부 기록**한다(W10-G의 자기검증 — *"루프가 끝까지 돌았다"*).
fn destroy_objects_at_first_recording(
    root: std::path::PathBuf,
    seen: Arc<Mutex<Vec<String>>>,
) -> AsyncHook {
    on_first(
        move |name| seen.lock().unwrap().push(name.to_owned()),
        move |_name| {
            let root = root.clone();
            Box::pin(async move { destroy_objects(&root).await })
        },
    )
}

/// 첫 발화에서 `.objects`를 파괴하고 **곧바로 빈 디렉터리로 재생성**한다(적대적 ABA).
fn destroy_and_recreate_objects_at_first(
    root: std::path::PathBuf,
    fired: Arc<AtomicUsize>,
) -> AsyncHook {
    on_first(
        move |_name| {
            fired.fetch_add(1, Ordering::SeqCst);
        },
        move |_name| {
            let root = root.clone();
            Box::pin(async move {
                destroy_objects(&root).await;
                tokio::fs::create_dir(root.join(".objects"))
                    .await
                    .expect(".objects 재생성(빈 dir)");
            })
        },
    )
}

/// **W-GRAVE-CD-A 전용** — `pre_grave`(무덤 rename **직전**)에서 **무대를 자기검증하고** `.objects`를
/// 통째로 파괴한다. 발화 sha를 전부 기록한다(③ `pre_grave` 정확히 1회 · ① `sha == VICTIM`).
///
/// **self-verify ②는 훅 *안*에서만 관측 가능하다**(park 시점의 디스크는 여기서밖에 못 본다):
/// `.objects/<sha>` **존재**(= rename이 아직 안 일어났다 ⇒ 우리는 정말로 rename **직전**에 서 있다)
/// ∧ `.gc-grave-*` **0개**(= 무덤이 아직 태어나지 않았다) → `remove_dir_all().unwrap()` →
/// 직후 `.objects` **부재**(= 파괴가 실제로 일어났다).
/// 훅은 패스 퓨처 **안에서** await되므로(spawn 0) 여기서의 panic은 테스트를 **시끄럽게** 죽인다.
fn verify_stage_then_destroy_objects_at_first(
    root: std::path::PathBuf,
    fired: Arc<Mutex<Vec<String>>>,
) -> AsyncHook {
    on_first(
        move |sha| fired.lock().unwrap().push(sha.to_owned()),
        move |sha| {
            let root = root.clone();
            Box::pin(async move {
                let objects = root.join(".objects");
                assert!(
                    tokio::fs::try_exists(objects.join(&sha)).await.unwrap(),
                    "자기검증 ②: `pre_grave`는 rename **직전**이다 ⇒ 정본이 아직 디스크에 있어야 한다"
                );
                assert!(
                    grave_names(&root).await.is_empty(),
                    "자기검증 ②: 무덤은 아직 하나도 태어나지 않았다"
                );
                destroy_objects(&root).await;
                assert!(
                    !tokio::fs::try_exists(&objects).await.unwrap(),
                    "자기검증 ②: 파괴가 실제로 일어났다"
                );
            })
        },
    )
}

/// `pre_grave`의 인자는 **sha**다 — 첫 발화에서 **바로 그 sha의 정본 blob**을 지운다(컨테이너는 산다).
fn vanish_that_blob_at_first(
    objects: std::path::PathBuf,
    victim: Arc<Mutex<Option<String>>>,
) -> AsyncHook {
    on_first(
        |_name| {},
        move |sha| {
            let (objects, victim) = (objects.clone(), victim.clone());
            Box::pin(async move {
                *victim.lock().unwrap() = Some(sha.clone()); // ⚠ 가드는 await를 넘지 않는다
                tokio::fs::remove_file(objects.join(&sha))
                    .await
                    .expect("자기검증: 파킹된 정본은 아직 디스크에 있어야 한다(= 미처리)");
            })
        },
    )
}

/// **W10 — `.objects` 파괴(재생성 없음) · 항목 접촉 잔존.**
///
/// ⓐ orphan blob 3개 · ⓑ **첫 `pre_entry`에서 `.objects` 파괴** · ⓒ 남은 항목의 `read()`가 ENOENT →
/// 부재 확인(`symlink_metadata` = ENOENT) → skip + **집계 bump** · ⓓ 루프 완주(훅 **3회** 발화) →
/// **가드**가 `metadata(.objects)` = ENOENT → **`Err(NotFound/2)` 무가공**.
///
/// **자기무효화 검사가 무대에 봉인돼 있다**: 격리 분기의 `mkdir_p_durable(.corrupt)`는 `read()`의
/// `Present` 팔 뒤에 있는데 파괴 이후 **어떤 `read()`도 성공할 수 없다** ⇒ 원리적으로 도달 불가.
/// 비트로트 blob·`.corrupt`·동시 put을 **무대에 두지 않는다**(벨트+멜빵).
#[tokio::test]
async fn objects_container_destroyed_mid_pass_still_fails_the_pass_and_publishes_nothing() {
    let fired = Arc::new(AtomicUsize::new(0));
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let hooks = Hooks {
        pre_entry: Some(destroy_objects_at_first(root.clone(), fired.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);
    tokio::fs::create_dir_all(root.join(".objects"))
        .await
        .unwrap();
    for i in 0..3 {
        plant_orphan_blob(&s, format!("w10-{i}").as_bytes()).await;
    }

    let err = reconcile::run_once(&s, GRACE, SETTLE)
        .await
        .expect_err("컨테이너가 죽은 패스는 **오늘과 같이** Err여야 한다 — 가드가 그것을 낸다");

    // ① 오늘과 **같은 kind ∧ 같은 errno**(무가공 전파 — 합성 에러가 아니다).
    //    ⚠ 형제 W10-TEMP·W-GRAVE-CD-A와 **동일 단언 3종**이다 — errno를 빼면 가드가 `NotFound`를
    //    **합성**해도(예: `Error::from(ErrorKind::NotFound)`, raw_os_error = `None`) 초록이 된다.
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::NotFound,
        "`.objects` 부재는 ENOENT/2다 — 오늘과 같은 kind. err={err:?}"
    );
    assert_eq!(
        err.raw_os_error(),
        Some(2),
        "가드는 `metadata`의 에러를 **무가공**으로 전파한다(errno까지). err={err:?}"
    );
    // ② 루프는 **완주했다**(오늘은 첫 항목에서 죽는다 → 1회) — 항목 소멸이 항목 skip으로 흡수됐다
    assert_eq!(
        fired.load(Ordering::SeqCst),
        3,
        "사라진 항목은 skip되고 루프는 **끝까지 돈다**(오늘의 red.sha는 여기서 1이다)"
    );
    // ③ **부활 0** — 가드가 `write_atomic`(→ `mkdir_p_durable`)보다 **먼저** 돌았다는 온디스크 증거.
    //    가드를 뒤로 옮기면(M-GUARD-AFTER) `.objects`가 되살아나고 이 단언이 RED다.
    assert!(
        !tokio::fs::try_exists(root.join(".objects")).await.unwrap(),
        "파괴된 컨테이너가 **부활**했다 — 가드가 `write_atomic` 뒤로 밀렸다"
    );
    // ④ **원장 미발행**
    assert!(
        !tokio::fs::try_exists(s.layout().gc_pending_path())
            .await
            .unwrap(),
        "`.gc-pending.json`이 발행됐다 — 패스가 조용히 완주했다"
    );
}

/// **W-GRAVE-CD-A — 파괴가 `grave()`의 rename에 착지한다**(blocking 채널).
///
/// 무대: **비예약 항목이 정확히 하나**(정합한 orphan blob) + 만료 tombstone.
/// ⚠ 둘 이상이면 남은 항목이 `Entry::seen`(**async** 채널)에서 스스로 집계를 올려 **`grave()`가 집계를
/// 올리지 않아도 가드가 발화한다** ⇒ M-NOBUMP-BLOCKING이 살아남는다. **하나가 load-bearing이다.**
///
/// `pre_grave`(rename **직전**)에서 `.objects`를 파괴한다 ⇒ `std::fs::rename`이 ENOENT →
/// `rename_checked_blocking`이 **소스를 확인**(`symlink_metadata` = ENOENT) → `Renamed::SourceGone`
/// (+ **blocking bump**) → `GraveOutcome::SourceGone` → `continue` → 루프 종료 → **가드** → `Err`.
#[tokio::test]
async fn container_destroyed_at_the_grave_rename_fails_the_pass_without_publishing_or_resurrecting()
{
    let pre_grave_fired: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let pre_entry_fired: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let hooks = Hooks {
        // ⑤ 엔트리 루프의 **발화 집합**을 기록한다 — bump 후보가 누구였는지를 증인이 스스로 센다.
        pre_entry: Some(recorder(pre_entry_fired.clone())),
        // ⚠ 파괴는 `pre_entry`가 아니라 `pre_grave`다 — **rename 직전**에 착지시킨다.
        pre_grave: Some(verify_stage_then_destroy_objects_at_first(
            root.clone(),
            pre_grave_fired.clone(),
        )),
        // ④ `post_grave`는 rename이 **`Ok`였을 때만** 불린다 ⇒ 0회 = `Renamed::Done`이 아니었다.
        post_grave: Some(recorder(graved.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);
    tokio::fs::create_dir_all(root.join(".objects"))
        .await
        .unwrap();
    let sha = plant_orphan_blob(&s, b"w-grave-cd-a").await; // 내용 **정합**(비트로트 아님)
    let t0 = SystemTime::now();
    seed_expired_tombstones(&root, t0, &[&sha]).await; // `.gc-pending.json`은 **Reserved** ⇒ FS 접촉 0

    let err = reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE)
        .await
        .expect_err("무덤 rename이 ENOENT인 세계도 **Err**로 끝난다(가드)");

    // ── self-verify ①③ — park에 도달했다 ∧ 그 sha는 **우리가 심은 그것**이다 ∧ **정확히 1회** ────
    //    (②는 훅 안에서 이미 단언됐다: 정본 존재 · 무덤 0개 · 파괴 완수.)
    assert_eq!(
        *pre_grave_fired.lock().unwrap(),
        vec![sha.clone()],
        "①③ 무덤 분기에 **정확히 한 번** 진입했고, 그 sha는 심어 둔 정본이다"
    );

    // ── 단언 — **무가공**(kind ∧ errno). 가드는 `metadata`의 에러를 그대로 전파한다 ─────────────
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::NotFound,
        "`.objects` 부재는 ENOENT다 — 오늘과 같은 kind. err={err:?}"
    );
    assert_eq!(
        err.raw_os_error(),
        Some(2),
        "errno까지 **무가공**이다(합성 에러가 아니다 — 가드가 `metadata`의 것을 그대로 낸다). err={err:?}"
    );

    // ── 단언 — **부활 0** ∧ **원장 미발행** ∧ **`.corrupt` 부재** ────────────────────────────────
    //    `.objects` 미부활이 `grave()`의 **blocking** 채널이 집계를 올렸다는 유일한 증거다:
    //    `rename_checked_blocking`의 `bump()`를 지우면(**M-NOBUMP-BLOCKING**) `vanished == 0`
    //    ⇒ 가드 미발화 ⇒ `write_atomic`의 첫 줄 `mkdir_p_durable`이 `.objects`를 **되살리고**
    //    `{}` 원장을 발행하며 패스는 **`Ok`** 가 된다 ⇒ 아래 셋이 한꺼번에 RED다.
    assert!(
        !tokio::fs::try_exists(root.join(".objects")).await.unwrap(),
        "컨테이너가 부활했다 — blocking 채널의 bump가 사라졌거나 가드가 밀렸다"
    );
    assert!(
        !tokio::fs::try_exists(s.layout().gc_pending_path())
            .await
            .unwrap(),
        "원장이 발행됐다 — 패스가 조용히 완주했다"
    );
    assert!(
        !tokio::fs::try_exists(s.layout().corrupt_dir())
            .await
            .unwrap(),
        "`.corrupt`가 생겼다 — 격리 분기는 이 무대에서 **원리적으로 도달 불가**다(내용 정합 ∧ 파괴 후 \
         어떤 `read()`도 성공할 수 없다). 생겼다면 무대가 오염된 것이다"
    );

    // ── self-verify ④ — **`post_grave` 0회** ⇒ `Renamed::Done`이 아니었다 ⇒ `Graved`도 `settle()`도
    //    태어나지 않았다. (⚠ *"`.gc-grave-<sha>`가 없다"*는 **공허하다** — `.objects`를 통째로
    //    지웠으므로 그 안의 무엇이든 무조건 부재다. 무덤이 안 태어났음의 증거는 **훅의 침묵**이다.)
    assert!(
        graved.lock().unwrap().is_empty(),
        "`post_grave`가 발화했다 = rename이 `Ok`였다 = 무덤이 태어났다 — `SourceGone`이 아니었다"
    );

    // ── self-verify ⑤ — **`pre_entry` 발화 집합 == {sha}** ⇒ 비예약 항목은 그 하나뿐이었다
    //    ⇒ `Entry::seen`(**async** 채널)의 bump 후보가 **없었다** ⇒ 이 패스에서 집계를 올릴 수 있었던
    //    것은 `grave()`의 **blocking** 채널뿐이다. **§C-A의 "정확히 하나" 규율을 증인이 스스로 검증한다.**
    //    (이것이 없으면 무대에 항목이 하나 더 새어 들어와도 증인이 조용히 GREEN이 되고,
    //     M-NOBUMP-BLOCKING이 살아남는다.)
    assert_eq!(
        *pre_entry_fired.lock().unwrap(),
        vec![sha.clone()],
        "⑤ 비예약 항목은 **정확히 하나**였다(`.gc-pending.json`은 Reserved ⇒ `pre_entry` 이전에 \
         continue ⇒ FS 접촉 0 ⇒ bump 후보가 아니다) ⇒ 집계의 bump 후보는 `grave()`뿐이었다"
    );
}

/// **W3 — 댕글링 blob 심링크는 오늘과 같이 패스를 죽인다**(P-1 봉인 · 플립 아님).
///
/// `tokio::fs::read`는 open이므로 **심링크를 추종**한다 → ENOENT. 그러나 **항목은 있다**
/// (`symlink_metadata` = `Ok(symlink)`) ⇒ `entry_is_absent` = `None` ⇒ **원본 에러 무가공 전파**.
/// 확인을 `metadata`(follow)로 바꾸는 뮤턴트(M-FOLLOW)는 이것을 `Absent`로 **위조**해 skip하고
/// 패스가 `Ok`로 완주한다 ⇒ RED. **park 0 · spawn 0 · 훅 0.**
#[cfg(unix)]
#[tokio::test]
async fn a_dangling_blob_symlink_still_aborts_the_pass_exactly_like_today() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let s = Store::new(root.clone());
    let objects = root.join(".objects");
    tokio::fs::create_dir_all(&objects).await.unwrap();

    let sha = "a".repeat(64); // 이름은 Blob으로 분류된다(64자 ASCII hex)
    let link = objects.join(&sha);
    std::os::unix::fs::symlink(objects.join("nope-target"), &link).unwrap();

    let err = reconcile::run_once(&s, GRACE, SETTLE)
        .await
        .expect_err("댕글링 심링크는 **항목이 있다** ⇒ skip 금지 ⇒ 오늘과 같은 Err");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::NotFound,
        "read(추종)의 ENOENT가 **무가공**으로 전파된다. err={err:?}"
    );
    // 링크는 그대로다(아무 것도 지우지 않았다) ∧ 패스가 중단됐으므로 원장도 없다
    assert!(
        tokio::fs::symlink_metadata(&link).await.is_ok(),
        "댕글링 심링크는 건드리지 않는다"
    );
    assert!(
        !tokio::fs::try_exists(s.layout().gc_pending_path())
            .await
            .unwrap(),
        "패스가 중단됐다면 `.gc-pending.json`은 발행되지 않는다(오늘과 동일)"
    );
}

/// **W10-TEMP — 같은 파괴를 *Temp 클래스만*으로.** (계수의 **클래스 전수화**.)
///
/// W10의 무대는 blob 3개다 ⇒ 계수가 **Blob 팔**(`e.read()`)에서만 선다. `Entry::seen`의 async 채널이
/// Temp 팔(`e.metadata()`)에서도 계수를 올린다는 것은 **행동으로 따로 핀해야 한다** — r14 적대적 반증이
/// *"W10(blob 무대)만으로는 계수 누락 뮤턴트가 살아남는다"*를 실측으로 보였다.
/// **단언은 W10과 동일한 3종**(무가공 `Err` · 부활 0 · 원장 0)이다.
#[tokio::test]
async fn temp_only_container_destruction_still_fails_the_pass_and_publishes_nothing() {
    const TEMPS: usize = 3;

    let fired = Arc::new(AtomicUsize::new(0));
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let hooks = Hooks {
        pre_entry: Some(destroy_objects_at_first(root.clone(), fired.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);
    tokio::fs::create_dir_all(root.join(".objects"))
        .await
        .unwrap();

    // ⚠ **blob 0개** — 이 무대의 유일한 계수 채널은 Temp 팔(`e.metadata()`)이다.
    for i in 0..TEMPS {
        atomic::write_atomic(
            &s.layout().temp_blob_path(&format!("w10t-{i}")),
            b"in flight",
        )
        .await
        .unwrap();
    }

    let err = reconcile::run_once(&s, GRACE, SETTLE)
        .await
        .expect_err("컨테이너가 죽은 패스는 **오늘과 같이** Err여야 한다 — 가드가 그것을 낸다");

    // ① 무가공 — kind·errno 둘 다
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "err={err:?}");
    assert_eq!(
        err.raw_os_error(),
        Some(2),
        "가드는 `metadata`의 에러를 **무가공**으로 전파한다(합성 에러가 아니다). err={err:?}"
    );
    // ② 루프 완주 — Temp 팔의 `metadata()` ENOENT가 **skip**으로 흡수됐다
    assert_eq!(
        fired.load(Ordering::SeqCst),
        TEMPS,
        "Temp 항목의 소멸도 skip으로 흡수되고 루프는 **끝까지** 돈다"
    );
    // ③ 부활 0 — Temp 팔이 집계를 올렸다는 유일한 증거(안 올리면 가드 미발화 ⇒ write_atomic이 부활시킨다)
    assert!(
        !tokio::fs::try_exists(root.join(".objects")).await.unwrap(),
        "컨테이너가 부활했다 — **Temp 팔의 bump가 사라졌다**(M-NOBUMP-ASYNC)"
    );
    // ④ 원장 미발행
    assert!(
        !tokio::fs::try_exists(s.layout().gc_pending_path())
            .await
            .unwrap(),
        "원장이 발행됐다 — 패스가 조용히 완주했다"
    );
}

/// **W10-G — 그 `Err`는 *항목 연산*이 낼 수 없다. 가드가 냈다.** (green-only · 가드 경로 self-verify.)
///
/// `pre_entry`가 발화한 **이름의 집합**이 심은 항목 **전부**와 같다 ⇒ 루프가 **끝까지 돌았다**
/// ⇒ 파괴 이후의 모든 항목 연산은 **skip됐다**(하나라도 `?`로 죽었으면 뒤의 훅이 발화하지 못한다)
/// ⇒ 남은 `Err`의 출처는 **루프-후 가드뿐**이다. (오늘의 red.sha에서는 발화가 **1회**다 ⇒ green-only.)
#[tokio::test]
async fn container_guard_fires_after_the_loop_runs_to_completion() {
    const ORPHANS: usize = 3;

    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let hooks = Hooks {
        pre_entry: Some(destroy_objects_at_first_recording(
            root.clone(),
            seen.clone(),
        )),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);
    tokio::fs::create_dir_all(root.join(".objects"))
        .await
        .unwrap();

    let mut planted = Vec::new();
    for i in 0..ORPHANS {
        planted.push(plant_orphan_blob(&s, format!("w10g-{i}").as_bytes()).await);
    }
    // ⑤ 창을 실제로 밟을 수 있는 무대인가(자기검증 — 항목 수 ≥ 3)
    assert_eq!(planted.len(), ORPHANS, "무대 자기검증: 심은 항목 수");

    let err = reconcile::run_once(&s, GRACE, SETTLE)
        .await
        .expect_err("파괴된 컨테이너에서 패스는 Err로 끝난다");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "err={err:?}");

    // ④ **발화 집합 == 심은 항목 전부** — 루프가 완주했다는 구조적 증거.
    let mut fired = seen.lock().unwrap().clone();
    fired.sort();
    let mut want = planted.clone();
    want.sort();
    assert_eq!(
        fired, want,
        "`pre_entry`가 **모든** 항목에서 발화했다 ⇒ 파괴 이후 어떤 항목 연산도 패스를 죽이지 않았다 \
         ⇒ 이 `Err`는 **항목 연산이 낼 수 없다** ⇒ 루프-후 가드가 냈다"
    );
    // 그리고 가드가 `write_atomic`보다 **앞**이라는 온디스크 증거
    assert!(
        !tokio::fs::try_exists(root.join(".objects")).await.unwrap(),
        "가드가 `write_atomic` 뒤로 밀리면 `.objects`가 부활한다(M-GUARD-AFTER)"
    );
}

/// **W10b — 꼬리 파괴는 *오늘도* 조용한 `Ok`다. 그것을 보존한다.** (특성화 · **게이트를 핀한다**.)
///
/// ⚠⚠ **`vanished > 0` 게이트를 지우는 뮤턴트(M-GUARD-ALWAYS)를 죽이는 *유일한* 증인이다.**
/// 무조건 가드는 **오늘 `Ok`인 이 패스를 `Err`로 뒤집는다** = **두 번째 관측 플립**.
///
/// 무대: **`Other` 클래스 항목 하나뿐**(63자 hex ⇒ blob도 temp도 예약도 아니다 ⇒ **분기 본문이 비어
/// 있다 = FS 접촉 0**). 그 항목의 `pre_entry`에서 `.objects`를 파괴한다 ⇒ **소멸 판정이 0건**이다
/// (`file_type()`은 d_type 캐시라 `Ok` · 분기 본문이 없어 그 뒤로 아무 syscall도 없다)
/// ⇒ **가드는 발화하지 않는다** ⇒ 루프 뒤 `write_atomic`이 `.objects`를 **되살리고** `{}` 원장을
/// 발행하며 패스는 **`Ok`** 다 — **red.sha에서 실측한 T1 그대로**(F-42: 오늘의 버그를 보존한다).
#[tokio::test]
async fn tail_destruction_without_any_vanished_entry_stays_ok_like_today() {
    let fired = Arc::new(AtomicUsize::new(0));
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let objects = root.join(".objects");
    let hooks = Hooks {
        pre_entry: Some(destroy_objects_at_first(root.clone(), fired.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);
    tokio::fs::create_dir_all(&objects).await.unwrap();

    // **63자** hex — `is_sha_name`(64자)이 아니고 `.tmp-`도 예약도 아니다 ⇒ `ObjectsEntry::Other`
    // ⇒ `match class`의 본문이 **비어 있다** ⇒ `file_type()` 이후 **FS 접촉이 0**이다.
    let other = "a".repeat(63);
    assert!(
        matches!(
            crate::layout::classify_objects_entry(&other),
            crate::layout::ObjectsEntry::Other
        ),
        "무대 자기검증: 63자 이름은 `Other`다(분기 본문 없음 ⇒ 소멸 판정 후보가 아니다)"
    );
    tokio::fs::write(objects.join(&other), b"tail")
        .await
        .unwrap();

    let stats = reconcile::run_once(&s, GRACE, SETTLE)
        .await
        .expect("꼬리 파괴는 **오늘도** 조용한 `Ok`다 — 무조건 가드는 이것을 `Err`로 뒤집는다");

    assert_eq!(fired.load(Ordering::SeqCst), 1, "항목 하나 ⇒ 훅 1회");
    // ① 오늘과 같은 조용한 `Ok` + 전수 stats
    assert_eq!(
        stats,
        ReconcileStats::default(),
        "FS 접촉이 0이었으므로 아무 카운터도 오르지 않는다"
    );
    // ② `write_atomic` → `mkdir_p_durable`이 컨테이너를 **되살린다**(오늘의 행동 — 보존한다)
    assert!(
        tokio::fs::try_exists(&objects).await.unwrap(),
        "오늘은 `write_atomic`이 `.objects`를 되살린다 — D안이 그것을 **보존**한다(F-42)"
    );
    // ③ 원장은 `{}`로 발행된다(심어 둔 것이 없으므로 빈 맵)
    assert_eq!(
        tokio::fs::read_to_string(s.layout().gc_pending_path())
            .await
            .unwrap(),
        "{}",
        "오늘의 꼬리 파괴는 `{{}}` 원장을 발행한다 — 그것도 보존한다"
    );
}

/// **W6 — park 중 *정본 blob*이 사라져도 패스는 완주한다**(컨테이너는 살아 있다 · green-only).
///
/// `pre_grave`가 sha와 함께 발화한다 ⇒ **바로 그 정본**을 지운다 ⇒ `grave()`의 rename이 ENOENT →
/// 소스 확인(부재) → **`GraveOutcome::SourceGone`** → `continue`(무덤은 **태어나지 않는다**) → 나머지는
/// 정상 회수. 오늘의 코드는 여기서 `?`로 **패스 전체를 죽인다**.
#[tokio::test]
async fn grave_source_vanished_during_park_lets_the_pass_finish() {
    const ORPHANS: usize = 3;

    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let objects = root.join(".objects");
    let victim: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let hooks = Hooks {
        pre_grave: Some(vanish_that_blob_at_first(objects.clone(), victim.clone())),
        post_grave: Some(recorder(graved.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);
    tokio::fs::create_dir_all(&objects).await.unwrap();

    let mut planted = Vec::new();
    for i in 0..ORPHANS {
        planted.push(plant_orphan_blob(&s, format!("w6-{i}").as_bytes()).await);
    }
    let t0 = SystemTime::now();
    let refs: Vec<&str> = planted.iter().map(String::as_str).collect();
    seed_expired_tombstones(&root, t0, &refs).await; // 셋 다 **회수 대상**

    let stats = reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE)
        .await
        .expect("PASS ABORTED — 회수할 정본이 사라진 항목 하나가 패스를 죽이면 안 된다");

    // 자기검증: 창을 실제로 밟았다(훅이 발화했고, 그 정본은 park 시점에 **아직 있었다**)
    let v = victim
        .lock()
        .unwrap()
        .clone()
        .expect("pre_grave가 발화했다");
    assert!(planted.contains(&v), "victim은 심은 orphan 중 하나다");
    assert!(
        !tokio::fs::try_exists(s.blob_path(&v)).await.unwrap(),
        "victim의 정본은 사라졌다"
    );
    // `post_grave`는 rename이 **성공했을 때만** 불린다 ⇒ victim은 그 목록에 **없다**
    // = `GraveOutcome::SourceGone`이었다는 직접 증거(`Graved`도 `settle()`도 태어나지 않았다).
    let mut moved = graved.lock().unwrap().clone();
    moved.sort();
    let mut want: Vec<String> = planted.iter().filter(|s| **s != v).cloned().collect();
    want.sort();
    assert_eq!(
        moved, want,
        "사라진 정본은 **무덤을 낳지 않는다**(`post_grave` 미발화) — 나머지만 회수된다"
    );

    assert_eq!(
        stats,
        ReconcileStats {
            referenced: 0,
            gc_deleted: ORPHANS - 1, // 사라진 하나를 뺀 전부가 회수됐다
            gc_pending: 0,           // 전부 사라졌거나 회수됐다 ⇒ 원장이 비워진다
            temps_deleted: 0,
            quarantined: 0,
        },
        "사라진 정본은 **건너뛰고** 나머지는 정상 회수된다"
    );
    assert!(
        grave_names(&root).await.is_empty(),
        "무덤 잔재 0 — 태어난 무덤은 전부 정산됐다"
    );
}

/// **W6b — 무덤 rename은 성공했고 *그 뒤의* fsync가 EACCES다 → raw `Err` ∧ 무덤은 디스크에 남는다.**
///
/// **M6의 프로덕션 얼굴이다**: `rename_durable_source_checked`가 rename `Ok` 이후의 fsync 실패를
/// `SourceGone`으로 **위조**하면 `settle()`이 스킵되고 무덤이 조용히 남는다(그리고 패스는 `Ok`가 된다).
/// 여기서는 **`Err(PermissionDenied)`가 무가공으로 나와야** 한다.
///
/// ⚠ **root면 권한 검사가 우회된다** ⇒ 전제가 사라진다 ⇒ **사유를 출력하고 skip**(조용한 GREEN 금지).
#[cfg(unix)]
#[tokio::test]
async fn grave_rename_ok_then_fsync_eacces_propagates_raw() {
    use std::os::unix::fs::PermissionsExt;

    // ── root 프로브: 0o300(w+x, **no read**) 디렉터리를 열 수 있으면 EACCES를 만들 수 없다 ──────
    let probe_d = tempfile::tempdir().unwrap();
    let probe = probe_d.path().join("nord");
    std::fs::create_dir(&probe).unwrap();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o300)).unwrap();
    let can_open = std::fs::File::open(&probe).is_ok();
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o700)).unwrap();
    if can_open {
        eprintln!(
            "SKIP grave_rename_ok_then_fsync_eacces_propagates_raw: 0o300 디렉터리를 열 수 있다 \
             (root로 도는 중인가?) ⇒ fsync EACCES 전제가 사라진다. \
             **조용한 GREEN이 아니라 명시적 skip이다.**"
        );
        return;
    }

    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let objects = root.join(".objects");
    let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // `pre_grave`(rename **직전**)에서 `.objects`를 **no-read**로 만든다.
    // rename은 w+x만 필요하므로 **성공**하고, 그 뒤 `fsync_dir_blocking`의 `File::open(dir)`가 **EACCES**다.
    let chmod = {
        let objects = objects.clone();
        on_first(
            |_sha| {},
            move |_sha| {
                let objects = objects.clone();
                Box::pin(async move {
                    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o300))
                        .expect("chmod 0o300");
                })
            },
        )
    };
    let hooks = Hooks {
        pre_grave: Some(chmod),
        post_grave: Some(recorder(graved.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);
    tokio::fs::create_dir_all(&objects).await.unwrap();

    let sha = plant_orphan_blob(&s, b"w6b-durability").await;
    let t0 = SystemTime::now();
    seed_expired_tombstones(&root, t0, &[&sha]).await;

    let err = reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE)
        .await
        .expect_err("rename `Ok` 이후의 fsync 실패는 **무가공 Err**다 — 삼키면 M6가 부활한다");

    // 읽을 수 있어야 단언한다(그리고 tempdir 정리도 가능해진다)
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o700)).unwrap();

    // ① **무가공** — `NotFound`가 아니라 `PermissionDenied`다(B7: 부재 확인 팔에 닿지도 않는다)
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::PermissionDenied,
        "fsync의 EACCES가 **무가공**으로 전파돼야 한다. err={err:?}"
    );
    // ② rename은 **정말로 일어났다** — 무덤이 디스크에 있다(fail-CLOSED: 다음 패스가 복구한다)
    assert_eq!(
        grave_names(&root).await,
        vec![expected_grave_name(&sha)],
        "rename은 성공했다 ⇒ 무덤이 남는다. `SourceGone`으로 위조하면 이 무덤이 **없다**(M6)"
    );
    assert!(
        !tokio::fs::try_exists(s.blob_path(&sha)).await.unwrap(),
        "정본은 무덤으로 옮겨졌다"
    );
    // ③ `post_grave`는 rename `Ok` **직후**에 불린다 — 그러나 `Graved`는 fsync 실패로 태어나지 못했다
    assert!(
        graved.lock().unwrap().is_empty(),
        "fsync가 실패했으므로 `Graved`는 태어나지 않았다(`post_grave` 0회)"
    );
    // ④ 패스가 중단됐으므로 원장은 **재기록되지 않았다**.
    //    ⚠ `.objects`가 살아 있으므로 **심어 둔** `.gc-pending.json`은 그대로 있다(그것이 부재하는지를
    //    묻는 것은 틀렸다 — W-GRAVE-CD-A는 컨테이너째 파괴하므로 그 단언이 성립하는 것이다).
    //    옳은 명제: **완주했다면** 정본이 무덤으로 옮겨져 `try_exists(blob)`가 `Ok(false)`가 되고
    //    원장은 **`{}`로 재기록**됐을 것이다. 중단됐으므로 **심은 tombstone이 그대로 남아 있다**.
    let ledger = tokio::fs::read_to_string(s.layout().gc_pending_path())
        .await
        .unwrap();
    assert!(
        ledger.contains(&sha),
        "중단된 패스는 `write_atomic(.gc-pending.json)`에 **도달하지 못한다** ⇒ 심어 둔 tombstone이 \
         그대로 남는다(완주했다면 `{{}}`로 재기록됐을 것이다). ledger={ledger}"
    );
}

/// **W-GRAVE-CD-B — 파괴 → *재생성*이면 `SourceGone`이었음이 직접 증명된다** (green-only · Class B-ABA).
///
/// **A와 한 줄만 다르다**: park 중 `remove_dir_all(.objects)` **→ `create_dir(.objects)`**.
/// rename ENOENT → 소스 확인 ENOENT → **`SourceGone`** → skip → 가드(`get() == 1`) →
/// `metadata(.objects)` = **`Ok(dir)`**(재생성됐다) → **통과** → `try_exists(blob)` = `Ok(false)` →
/// `cleaned = {}` → **`Ok`**.
///
/// **논증**: `Ok`로 끝났다 = `?`로 죽지 않았다 ∧ **`post_grave` 0회** = `Renamed::Done`도 아니었다
/// ⇒ **남는 팔은 `SourceGone` 하나뿐이다.** (덤: 인간이 채택한 포기 **Class B-ABA**를 코드로 못박는다 —
/// 오늘은 `Err`인 세계가 조용한 `Ok`가 된다. **데이터 손실은 0**이다.)
#[tokio::test]
async fn container_destroyed_then_recreated_at_the_grave_rename_completes_with_empty_stats() {
    let fired = Arc::new(AtomicUsize::new(0));
    let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let hooks = Hooks {
        pre_grave: Some(destroy_and_recreate_objects_at_first(
            root.clone(),
            fired.clone(),
        )),
        post_grave: Some(recorder(graved.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);
    tokio::fs::create_dir_all(root.join(".objects"))
        .await
        .unwrap();

    // ⚠ **비예약 항목이 정확히 하나**(§C-A 규율 — 둘 이상이면 다른 항목이 스스로 집계를 올려
    //    `grave()`의 blocking 채널을 핀하지 못한다).
    let sha = plant_orphan_blob(&s, b"w-grave-cd-b").await;
    let t0 = SystemTime::now();
    seed_expired_tombstones(&root, t0, &[&sha]).await;

    let stats = reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE)
        .await
        .expect(
            "재생성된 컨테이너를 가드가 보면 패스는 완주한다(Class B-ABA — 인간이 채택한 포기)",
        );

    assert_eq!(
        fired.load(Ordering::SeqCst),
        1,
        "무덤 분기에 정확히 한 번 진입했다"
    );
    // ★ `Ok`다 ∧ `post_grave` 0회 ⇒ `Done`도 `?`도 아니었다 ⇒ **`SourceGone`이었다.**
    assert!(
        graved.lock().unwrap().is_empty(),
        "`post_grave` 0회 = `Renamed::Done`이 아니었다 ⇒ 남는 팔은 `SourceGone` 하나뿐이다"
    );
    assert_eq!(
        stats,
        ReconcileStats::default(),
        "재생성된 빈 컨테이너에서 패스는 전수 0으로 완주한다"
    );
    assert!(
        tokio::fs::try_exists(root.join(".objects")).await.unwrap(),
        "재생성된 컨테이너는 살아 있다(가드가 그것을 본다)"
    );
    assert!(
        !tokio::fs::try_exists(s.layout().grave_path(&sha))
            .await
            .unwrap(),
        "무덤은 태어나지 않았다"
    );
}
