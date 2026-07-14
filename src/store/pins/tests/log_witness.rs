//! **W-LOG — 로그 스트림 증인 (P-33).**
//!
//! P16은 *"로깅 행동이 동일하다"*고 선언하면서 **증인을 면제**했다 — 근거는 *"스위트가 tracing
//! subscriber를 설치하지 않는다"*였고, 그것은 **거짓**이다(`pins.rs`의 `capture_subscriber`를 **4개
//! 테스트가 설치**한다). 그리고 픽스는 **사라진 항목 뒤에도 계속 도므로**, 베이스라인이 `?`로 중단하던
//! 자리에서 **기존 하류 이벤트가 새로 발화한다.** 이 증인은 그 셋을 **결정적으로** 나눠 못박는다:
//!
//! * **W-LOG-A**(특성화 · 양쪽 GREEN) — **소멸 0**이면 이벤트 스트림이 **완전히 동일**하다
//!   (레벨 · target · 메시지 · 필드 · **순서**). ⇒ *"호출부·스키마 보존"*.
//! * **W-LOG-B**(green-only) — **소멸 1** 뒤에도 패스가 완주하므로 **기존 하류 이벤트가 발화한다**
//!   (격리 WARN ×(N−1)). ⇒ *"허용된 플립의 하류 결과"*를 **명시적으로 특성화**한다.
//! * **W-LOG-C**(green-only) — **skip 경로는 침묵한다**: 소멸 항목을 건너뛸 때 **어떤 레벨의 이벤트도
//!   0건**이다(**Blob `read()` 팔 하나**). ⇒ *"skip 전용 이벤트 없음"*.
//! * **W-LOG-D**(★차단 요건) — 그 침묵을 **밟을 수 있는 skip 팔 전부**로 넓힌다(무대 6개 · 아래 표).
//!
//! ## ⚠ 레벨 상한을 쓸 수 없는 이유 (측정으로 확인했다)
//!
//! `capture_subscriber`의 `enabled`는 `*m.level() <= tracing::Level::INFO`다 ⇒ **DEBUG/TRACE를 버린다.**
//! *"skip 시 `tracing::debug!` 한 줄을 추가하는 뮤턴트"*는 그 구독자에게 **보이지 않는다**(실측: 뮤턴트
//! 아래에서도 그 시야는 0건). ⇒ **W-LOG는 레벨을 거르지 않고 `target`으로 거른다** — 그것이
//! `event_tap`이다.
//!
//! **구독자의 기계는 이제 하나다**(`pins::tests`의 `Capture`): `Subscriber` 구현 · 스팬 no-op ·
//! 스레드-로컬 `set_default`가 **한 번만** 쓰이고, 증인은 **(무엇을 보는가, 한 줄을 어떻게 적는가)**
//! 두 축만 고른다. 형식은 **합치지 않았다** — 레거시 줄의 `k=v`(따옴표 없음)를 T-S2가 판다(`Capture` 참조).

use super::*;
use crate::store::reconcile::ReconcileStats;

const WLOG_GRACE: Duration = Duration::from_secs(3600);
const WLOG_SETTLE: Duration = Duration::from_secs(30);

/// 첫 `pre_entry` 발화에서 **바로 그 항목**을 unlink한다 — "스냅샷 이후 소멸"의 결정적 재현.
/// (readdir 순서와 무관: *어느* 항목이 첫 번째든 **첫 항목이 사라진다**는 명제는 항상 참이다.)
fn vanish_first_entry(
    objects: std::path::PathBuf,
    victim: Arc<Mutex<Option<String>>>,
) -> AsyncHook {
    on_first(
        |_name| {},
        move |name| {
            let (objects, victim) = (objects.clone(), victim.clone());
            Box::pin(async move {
                *victim.lock().unwrap() = Some(name.clone()); // ⚠ 가드는 await를 넘지 않는다
                tokio::fs::remove_file(objects.join(&name))
                    .await
                    .expect("victim unlink — 창을 실제로 밟았다");
            })
        },
    )
}

/// 이름 = `sha(name_seed)` · 내용 = **다른 바이트** ⇒ 격리(WARN) 대상.
async fn plant_rotten(s: &Store, name_seed: &[u8], content: &[u8]) -> String {
    let sha = hex_sha(name_seed);
    tokio::fs::write(s.blob_path(&sha), content).await.unwrap();
    sha
}

fn tap() -> (Arc<Mutex<Vec<String>>>, tracing::subscriber::DefaultGuard) {
    let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let g = tracing::subscriber::set_default(event_tap(logs.clone()));
    (logs, g)
}

// ═══ W-LOG-A — **특성화 · 양쪽 GREEN.** 소멸 0 ⇒ 스트림이 완전히 동일하다. ═══════════════════
//
// 이것이 P16이 **정말로** 주장할 수 있는 것이다: **호출부·레벨·target·메시지·필드·순서 보존**.
// 뮤턴트: 레벨 변경(WARN→INFO) · 메시지 변경 · 필드 변경/삭제 · 호출부 삭제 → **전부 RED**.
#[tokio::test]
async fn w_log_a_no_vanish_stream_is_identical() {
    let (logs, _g) = tap();

    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let s = Store::with_hooks(root.clone(), Hooks::default());
    tokio::fs::create_dir_all(root.join(".objects"))
        .await
        .unwrap();

    // 잔존 무덤(정본 부재) → 복구 ⇒ INFO ×2 · 비트로트 blob → 격리 ⇒ WARN ×1
    let g_content = b"wlog-grave-payload".to_vec();
    let g_sha = hex_sha(&g_content);
    tokio::fs::write(s.layout().grave_path(&g_sha), &g_content)
        .await
        .unwrap();
    let c_sha = plant_rotten(&s, b"wlog-corrupt-name", b"wlog-rotten-bytes").await;

    let stats = reconcile::run_once_at_for_test(&s, SystemTime::now(), WLOG_GRACE, WLOG_SETTLE)
        .await
        .expect("PASS ABORTED — 소멸이 0인 패스는 오늘도 완주한다");

    // self-verify: 두 이벤트 공급원이 **실제로** 발화 조건을 밟았다
    assert_eq!(stats.quarantined, 1, "격리가 일어났다");
    assert!(
        tokio::fs::try_exists(s.layout().corrupt_dir().join(&c_sha))
            .await
            .unwrap(),
        "격리된 정본이 .corrupt에 있다"
    );
    assert!(
        !tokio::fs::try_exists(s.layout().grave_path(&g_sha))
            .await
            .unwrap(),
        "무덤이 정본으로 되돌아갔다"
    );

    // ★ 전수 · 순서까지 핀한다.
    assert_eq!(
        *logs.lock().unwrap(),
        vec![
            format!("INFO files::store::reconcile grave recovered sha={g_sha}"),
            "INFO files::store::reconcile graves recovered from a previous pass recovered=1"
                .to_owned(),
            format!("WARN files::store::reconcile quarantined corrupt blob (bit rot) sha={c_sha}"),
        ],
        "소멸 0인 패스의 이벤트 스트림은 레벨·target·메시지·필드·순서까지 **바이트 동일**해야 한다"
    );
}

// ═══ W-LOG-B — **green-only.** 완주한 패스의 **하류 이벤트**를 명시적으로 특성화한다. ═════════
//
// 베이스라인은 첫 항목의 ENOENT에서 `?`로 중단하므로 **이벤트가 0건**이다(실측). 픽스는 그 항목만
// 건너뛰고 완주하므로 **남은 (N−1)개의 격리 WARN이 발화한다** — 이것이 *"허용된 플립의 하류 결과"*다.
// 뮤턴트: 하류 이벤트를 억누르면(`vanished > 0`일 때 WARN 생략) **여기서만** RED가 된다(W-LOG-A는 GREEN).
#[tokio::test]
async fn w_log_b_downstream_events_fire_after_the_pass_survives() {
    const ROTTEN: usize = 3;

    let (logs, _g) = tap();

    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let objects = root.join(".objects");
    tokio::fs::create_dir_all(&objects).await.unwrap();

    let victim: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let hooks = Hooks {
        pre_entry: Some(vanish_first_entry(objects.clone(), victim.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);

    let mut planted = Vec::new();
    for i in 0..ROTTEN {
        planted.push(
            plant_rotten(
                &s,
                format!("wlog-b-corrupt-{i}").as_bytes(),
                format!("wlog-b-rotten-{i}").as_bytes(),
            )
            .await,
        );
    }

    let stats = reconcile::run_once_at_for_test(&s, SystemTime::now(), WLOG_GRACE, WLOG_SETTLE)
        .await
        .expect("PASS ABORTED — 사라진 항목 하나가 패스를 죽이면 안 된다");

    // self-verify: 창을 **실제로** 밟았다
    let v = victim
        .lock()
        .unwrap()
        .clone()
        .expect("pre_entry가 발화했다");
    assert!(planted.contains(&v), "victim은 심은 blob 중 하나다");
    assert!(
        !tokio::fs::try_exists(objects.join(&v)).await.unwrap(),
        "victim은 스냅샷 이후 디스크에서 사라졌다"
    );

    // 하류 **행동**: 사라진 것을 뺀 전부가 격리됐다
    assert_eq!(stats.quarantined, ROTTEN - 1);

    // ★ 하류 **로그**: 살아남은 (N−1)개의 격리 WARN이 **정확히** 그만큼 난다. 그 외 이벤트는 0.
    let mut got = logs.lock().unwrap().clone();
    got.sort();
    let mut want: Vec<String> = planted
        .iter()
        .filter(|sha| *sha != &v)
        .map(|sha| {
            format!("WARN files::store::reconcile quarantined corrupt blob (bit rot) sha={sha}")
        })
        .collect();
    want.sort();
    assert_eq!(
        got, want,
        "완주한 패스는 베이스라인이 중단으로 도달하지 못하던 **기존 하류 이벤트**를 낸다 — \
         그것이 단일 플립의 하류 결과다(새 이벤트 종류는 0)"
    );
}

// ═══ W-LOG-C — **green-only.** skip 경로의 **침묵**(모든 레벨에서 0건). ══════════════════════
//
// 항목이 **하나뿐**이고 그것이 사라진다 ⇒ 이 패스의 이벤트 공급원은 **0개**다. 픽스는 완주하지만
// **아무 로그도 내지 않는다.** 뮤턴트: skip에 `tracing::debug!` 한 줄 추가 → **여기서 RED**.
// ⚠ `capture_subscriber`(level ≤ INFO)로는 그 뮤턴트가 **보이지 않는다** — `event_tap`이라야 잡는다.
#[tokio::test]
async fn w_log_c_skip_path_emits_no_event_at_any_level() {
    let (logs, _g) = tap();

    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let objects = root.join(".objects");
    tokio::fs::create_dir_all(&objects).await.unwrap();

    let victim: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let hooks = Hooks {
        pre_entry: Some(vanish_first_entry(objects.clone(), victim.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);

    // **무결한** blob 하나뿐 — 격리도 GC도 무덤 복구도 없다 ⇒ 이벤트 공급원 0.
    let content = b"wlog-c-intact".to_vec();
    let sha = hex_sha(&content);
    tokio::fs::write(s.blob_path(&sha), &content).await.unwrap();

    let stats = reconcile::run_once_at_for_test(&s, SystemTime::now(), WLOG_GRACE, WLOG_SETTLE)
        .await
        .expect("PASS ABORTED — 사라진 유일 항목은 skip되고 패스는 완주한다");

    // self-verify: 창을 밟았고, 그 항목은 **아무 카운터도 올리지 않았다**
    assert_eq!(victim.lock().unwrap().as_deref(), Some(sha.as_str()));
    assert!(!tokio::fs::try_exists(objects.join(&sha)).await.unwrap());
    assert_eq!(stats, ReconcileStats::default(), "건너뛴 항목은 무카운트다");

    // ★ **침묵.** 어떤 레벨에서도 0건 — `tracing::debug!`를 추가하는 뮤턴트가 여기서 죽는다.
    assert_eq!(
        *logs.lock().unwrap(),
        Vec::<String>::new(),
        "사라진 항목을 건너뛸 때 **어떤 레벨의 이벤트도** 내지 않는다(skip 전용 이벤트 0)"
    );
}

// ═══ W-LOG-D — ★**차단 요건.** skip 침묵의 **전수화**(밟을 수 있는 팔 전부). ════════════════════
//
// ⚠⚠ **W-LOG-C가 실제로 밟는 skip 팔은 Blob `read()` *하나뿐*이다**(반증 실측 — 정직하게 적는다).
// 나머지 팔은 **무보호**였고, 거기에 로그를 넣는 뮤턴트가 **전 스위트를 통과했다**(실측 생존).
//
// ## `Seen::Gone` → `continue` 팔 **전수표**(`reconcile.rs`) — 이 증인의 정본
//
// | 팔 | 분기 | 오늘 로그 | 무대 |
// |---|---|---|---|
// | `:133` | 무덤 루프 `file_type()`      | 없음 | **도달 불가**(d_type 캐시 — W2가 실측) → Class **B-FT** |
// | `:149` | 무덤 루프 `remove()`         | 없음 | **⑤** |
// | `:154` | 무덤 루프 `rename_durable_to()` | 없음 | **⑥** |
// | `:227` | 엔트리 루프 `file_type()`    | 없음 | **도달 불가**(같은 이유) → Class **B-FT** |
// | `:236` | Temp `metadata()`            | 없음 | **①** |
// | `:244` | Temp `remove()`              | 없음 | **②**(★신규 — *"우리가 지운 게 아니다"*) |
// | `:252` | Blob `read()`                | 없음 | **③**(W-LOG-C와 같은 팔 — 여기서도 판다) |
// | `:257` | 격리 `rename_into()`         | 없음 | **무보호** — `read()`와 `rename_into()` 사이에 **배리어가 없다** → Class **B-QUAR** |
// | `:280` | `grave()` `SourceGone`       | 없음 | **④** |
//
// **덮지 못한 팔은 숨기지 않는다**: `:133`·`:227`(도달 불가) · `:257`(배리어 부재 — 새 훅 = 프로덕션
// 변경이므로 별도 증분이다). 그것이 **P16 ②의 정확한 범위**다 — 독 코멘트가 커버리지를 앞지르지 않는다.
//
// ## 각 무대가 **자기가 그 팔을 밟았음을 스스로 증명한다**(조용한 초록 금지)
//
// 특히 무대 **②**는 `metadata()`(**`Ok`**)와 `remove()`(**ENOENT**)를 **갈라야** 한다 — 둘 다 `Gone`이면
// 관측 결과가 **같기 때문이다**(패스가 그 항목을 건너뛴다). 그래서 ②는 **`now`만 다른 두 실행**(α·β)을
// 같은 무대에서 돌린다:
//   · **β**(`age ≤ grace`) ⇒ `remove()`에 **도달하지 않는다** ⇒ 소멸 계수 **0** ⇒ 루프-후 가드가 **돌지
//     않는다** ⇒ `write_atomic`이 `.objects`를 **되살리고** 패스는 **`Ok`** 다.
//   · **α**(`age > grace`) ⇒ `remove()`에 **도달한다** ⇒ ENOENT ⇒ 계수 **1** ⇒ 가드가 돌고 **`Err`** 다.
// 두 실행의 **디스크 상태는 동일**하고 **`now`만** 다르다 ⇒ β의 `Ok`는 *"`file_type()`도 `metadata()`도
// 계수하지 않았다"*(= 둘 다 **`Present`**)의 **직접 증거**이고, 그러면 α에서 계수를 올릴 수 있는 곳은
// **`remove()` 하나뿐**이다 ⇒ **α는 `:244`를 밟았다.** ∎
//
// ⇒ **W10b와 같은 등급의 이식 차단 요건이다.**

/// 첫 `pre_entry` 발화에서 **`.objects` 컨테이너를 통째로 옮긴다**(재생성 없음).
///
/// ⚠⚠ **이것이 `:244`의 유일한 결정적 무대다.** `Entry::metadata()`는 `de.metadata()`이고 std는 그것을
/// **열린 디렉터리 fd 기준 `fstatat`**로 낸다(스냅샷의 `DirEntry`가 `DIR*`를 살려 둔다) ⇒ 컨테이너를
/// **경로에서** 치워도 **`Ok`** 다. 반면 `Entry::remove()`는 **경로**(`de.path()`)로 `unlink`하므로
/// **ENOENT**다 ⇒ `metadata()` = `Present` ∧ `remove()` = `Gone` — 그 사이에 훅이 **하나도 없어도**
/// 그 창이 열린다. (이 전제는 무대 ②-β가 **실행으로** 확인한다 — 전제가 깨지면 β가 **RED**다.)
fn move_objects_away_on_first_entry(
    objects: std::path::PathBuf,
    away: std::path::PathBuf,
    victim: Arc<Mutex<Option<String>>>,
) -> AsyncHook {
    on_first(
        |_name| {},
        move |name| {
            let (objects, away, victim) = (objects.clone(), away.clone(), victim.clone());
            Box::pin(async move {
                *victim.lock().unwrap() = Some(name.clone()); // ⚠ 가드는 await를 넘지 않는다
                tokio::fs::rename(&objects, &away)
                    .await
                    .expect("컨테이너 이동 — 창을 실제로 밟았다");
            })
        },
    )
}

/// 무대 ②의 몸. **`now`만이 α와 β를 가른다** — 디스크 상태·훅·grace는 **글자 그대로 같다**.
/// 반환 = (패스 결과 · 로그 · victim 이름 · `.objects` 부활 여부 · 옮겨 둔 temp의 생존 여부).
async fn temp_remove_stage(
    now_shift: Duration,
) -> (
    std::io::Result<ReconcileStats>,
    Vec<String>,
    Option<String>,
    bool,
    bool,
) {
    let (logs, _g) = tap();

    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let objects = root.join(".objects");
    let away = root.join(".objects-away");
    tokio::fs::create_dir_all(&objects).await.unwrap();

    let victim: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let hooks = Hooks {
        pre_entry: Some(move_objects_away_on_first_entry(
            objects.clone(),
            away.clone(),
            victim.clone(),
        )),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);

    // 비예약 항목은 **이 temp 하나뿐**이다 ⇒ 첫 `pre_entry`가 곧 그 temp다 ⇒ 결정적.
    let temp = s.layout().temp_blob_path("wlog-d-remove");
    atomic::write_atomic(&temp, b"in flight").await.unwrap();
    let temp_name = temp.file_name().unwrap().to_string_lossy().to_string();

    let result =
        reconcile::run_once_at_for_test(&s, SystemTime::now() + now_shift, WLOG_GRACE, WLOG_SETTLE)
            .await;

    let got = victim.lock().unwrap().clone();
    assert_eq!(
        got.as_deref(),
        Some(temp_name.as_str()),
        "자기검증: 훅이 발화한 항목은 **그 temp**다(비예약 항목이 하나뿐이므로 결정적)"
    );
    let objects_alive = tokio::fs::try_exists(&objects).await.unwrap();
    let temp_alive = tokio::fs::try_exists(away.join(&temp_name)).await.unwrap();
    let lines = logs.lock().unwrap().clone();
    (result, lines, got, objects_alive, temp_alive)
}

#[tokio::test]
async fn w_log_d_every_reachable_skip_arm_is_silent() {
    // ── 무대 ①: **Temp `metadata()`**(`:236`) — grace를 넘긴 temp가 스냅샷 이후 소멸한다 ──────────
    {
        let (logs, _g) = tap();

        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let objects = root.join(".objects");
        tokio::fs::create_dir_all(&objects).await.unwrap();

        let victim: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let hooks = Hooks {
            pre_entry: Some(vanish_first_entry(objects.clone(), victim.clone())),
            ..Hooks::default()
        };
        let s = Store::with_hooks(root.clone(), hooks);

        // temp **하나뿐** ⇒ 첫 `pre_entry`가 곧 그 temp다 ⇒ 결정적.
        let temp = s.layout().temp_blob_path("wlog-d");
        atomic::write_atomic(&temp, b"in flight").await.unwrap();
        let temp_name = temp.file_name().unwrap().to_string_lossy().to_string();

        // ⚠ **grace를 넘긴 temp다** — 주입 시각을 미래로 밀어 `age > grace`를 만든다
        //    ⇒ 소멸하지 않았다면 이 패스는 그것을 **삭제**했을 것이다(`temps_deleted = 1`).
        //    소멸했으므로 **skip**되고 `temps_deleted = 0`이며 **아무 로그도 나지 않아야** 한다.
        let t_future = SystemTime::now() + 2 * WLOG_GRACE;

        let stats = reconcile::run_once_at_for_test(&s, t_future, WLOG_GRACE, WLOG_SETTLE)
            .await
            .expect("PASS ABORTED — 사라진 temp는 skip되고 패스는 완주한다");

        // self-verify: **Temp `metadata()` 팔의 skip을 실제로 밟았다**
        assert_eq!(
            victim.lock().unwrap().as_deref(),
            Some(temp_name.as_str()),
            "자기검증: 소멸시킨 것은 **그 temp**다"
        );
        assert!(!tokio::fs::try_exists(&temp).await.unwrap());
        assert_eq!(
            stats,
            ReconcileStats::default(),
            "사라진 temp는 **우리가 지운 것이 아니다** ⇒ `temps_deleted`는 오르지 않는다(Mut-Count)"
        );

        // ★ **침묵** — 모든 레벨에서 0건. M-LOG-DEBUG-TEMP가 여기서 죽는다.
        assert_eq!(
            *logs.lock().unwrap(),
            Vec::<String>::new(),
            "**Temp `metadata()` 팔**(`:236`)의 skip은 어떤 레벨에서도 이벤트를 내지 않는다"
        );
    }

    // ── 무대 ②: **Temp `remove()`**(`:244` — *"우리가 지운 게 아니다"*) ────────────────────────
    //    β(대조) → α(그 팔). **`now`만 다르다.** §머리말의 논증이 여기서 실행된다.
    {
        // β — `age ≤ grace` ⇒ `remove()`에 **도달하지 않는다** ⇒ 계수 0 ⇒ 가드 미발화 ⇒ **`Ok`**.
        //     ⇒ **`file_type()`도 `metadata()`도 `Present`였다**(둘 중 하나라도 `Gone`이었다면 계수가
        //        1이 되어 가드가 `Err(NotFound)`를 냈을 것이다 — `.objects`가 경로에서 사라졌으므로).
        let (result, logs, victim, objects_alive, temp_alive) =
            temp_remove_stage(Duration::ZERO).await;
        let stats = result.expect(
            "β: 소멸 계수 0 ⇒ 루프-후 가드는 **돌지 않는다** ⇒ `write_atomic`이 `.objects`를 \
             되살리고 패스는 `Ok`다. 여기가 RED라면 `de.metadata()`가 **경로 기준**이라는 뜻이고, \
             그러면 아래 α는 `:244`가 아니라 `:236`을 밟고 있는 것이다 — **조용한 초록을 막는 자물쇠다**",
        );
        assert_eq!(stats, ReconcileStats::default(), "β: 아무것도 세지 않는다");
        assert!(victim.is_some(), "β: 훅이 발화했다");
        assert!(
            objects_alive,
            "β: `write_atomic`이 `.objects`를 되살렸다(가드가 돌지 않았다)"
        );
        assert!(
            temp_alive,
            "β: 옮겨 둔 컨테이너 안의 temp는 **그대로**다(아무도 지우지 않았다)"
        );
        assert_eq!(logs, Vec::<String>::new(), "β: 이벤트 공급원 0");

        // α — `age > grace` ⇒ `remove()`에 **도달한다** ⇒ 경로가 죽었으므로 ENOENT ⇒ 확인 ⇒ `Gone`
        //     ⇒ **`:244` continue** ⇒ 계수 1 ⇒ 가드가 돌고 `metadata(.objects)`가 ENOENT ⇒ **`Err`**.
        let (result, logs, victim, objects_alive, temp_alive) =
            temp_remove_stage(2 * WLOG_GRACE).await;
        let e = match result {
            Ok(s) => panic!(
                "α: `remove()`가 소멸을 계수했으므로 루프-후 가드가 **반드시** 돈다 — got Ok({s:?})"
            ),
            Err(e) => e,
        };
        assert_eq!(
            e.kind(),
            std::io::ErrorKind::NotFound,
            "α: 가드는 `metadata(.objects)`의 에러를 **무가공** 전파한다. err={e:?}"
        );
        assert!(victim.is_some(), "α: 훅이 발화했다");
        assert!(
            !objects_alive,
            "α: 가드가 **`write_atomic` 이전에** `Err`를 냈다 ⇒ `.objects`는 부활하지 않는다"
        );
        // ★ 자기검증의 핵심 — 그 temp는 **옮겨 둔 컨테이너 안에 멀쩡히 있다.**
        //   ⇒ 패스의 `remove_file`은 그것을 지우지 **못했다**(경로가 죽었으므로) = `Gone` 팔이다.
        //   ⇒ 그런데 `metadata()`는 **`Ok`** 였다(β가 증명한다) ⇒ 계수를 올린 것은 `remove()`뿐이다.
        assert!(
            temp_alive,
            "α: temp는 옮겨 둔 컨테이너에 **그대로** 있다 — 패스는 그것을 지우지 못했다(`Gone`)"
        );
        // ★ **침묵** — `:244`에 로그 한 줄을 넣는 뮤턴트가 **여기서만** 죽는다.
        assert_eq!(
            logs,
            Vec::<String>::new(),
            "**Temp `remove()` 팔**(`:244`)의 skip은 어떤 레벨에서도 이벤트를 내지 않는다"
        );
    }

    // ── 무대 ③: **Blob `read()`**(`:252`) — W-LOG-C와 같은 팔을 **전수표 안에서** 다시 판다 ──────
    {
        let (logs, _g) = tap();

        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let objects = root.join(".objects");
        tokio::fs::create_dir_all(&objects).await.unwrap();

        let victim: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let hooks = Hooks {
            pre_entry: Some(vanish_first_entry(objects.clone(), victim.clone())),
            ..Hooks::default()
        };
        let s = Store::with_hooks(root.clone(), hooks);

        // ⚠ **비트로트 blob 하나뿐** — 소멸하지 않았다면 이 패스는 그것을 **격리**하고 **WARN**을 냈다.
        //   소멸했으므로 `read()`가 `Gone`이고 패스는 **침묵**한다.
        let sha = plant_rotten(&s, b"wlog-d-read-name", b"wlog-d-read-rotten").await;

        let stats = reconcile::run_once_at_for_test(&s, SystemTime::now(), WLOG_GRACE, WLOG_SETTLE)
            .await
            .expect("PASS ABORTED — 사라진 blob은 skip되고 패스는 완주한다");

        assert_eq!(victim.lock().unwrap().as_deref(), Some(sha.as_str()));
        assert!(!tokio::fs::try_exists(objects.join(&sha)).await.unwrap());
        assert_eq!(stats, ReconcileStats::default(), "건너뛴 항목은 무카운트다");
        // ★ 자기검증 — **`.corrupt`가 없다** ⇒ `mkdir_p_durable`에 **도달하지 못했다**
        //   ⇒ 멈춘 곳은 `read()`의 `Gone`(`:252`)이지 격리 rename(`:257`)이 **아니다**.
        assert!(
            !tokio::fs::try_exists(s.layout().corrupt_dir()).await.unwrap(),
            "자기검증: 격리 블록에 **도달하지 않았다**(`.corrupt`가 만들어지지 않았다) ⇒ `:252` 팔이다"
        );
        assert_eq!(
            *logs.lock().unwrap(),
            Vec::<String>::new(),
            "**Blob `read()` 팔**(`:252`)의 skip은 어떤 레벨에서도 이벤트를 내지 않는다"
        );
    }

    // ── 무대 ④: **`grave()`의 `SourceGone`**(`:280`) — park 중 정본이 사라진다 ─────────────────
    {
        let (logs, _g) = tap();

        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        tokio::fs::create_dir_all(root.join(".objects"))
            .await
            .unwrap();

        let layout = crate::layout::Layout::new(root.clone());
        let content = b"wlog-d-grave-outcome".to_vec();
        let sha = hex_sha(&content);
        let blob = layout.blob_path(&sha);
        tokio::fs::write(&blob, &content).await.unwrap();

        // **지난 패스의 tombstone** — 최초 관측 시각 = epoch ⇒ grace를 이미 넘겼다 ⇒ 이 패스가 회수를
        // 시도한다(= `pre_grave`에 도달한다). 손으로 심는 이유: 패스를 **한 번만** 돌려야 로그 스트림이
        // 그 무대의 것으로만 남는다.
        tokio::fs::write(
            &layout.gc_pending_path(),
            format!("{{\"{sha}\":0}}").as_bytes(),
        )
        .await
        .unwrap();

        let parked: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let moved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let hooks = {
            let (parked, blob) = (parked.clone(), blob.clone());
            Hooks {
                pre_grave: Some(on_first(
                    move |sha| parked.lock().unwrap().push(sha.to_owned()),
                    move |_sha| {
                        let blob = blob.clone();
                        Box::pin(async move {
                            tokio::fs::remove_file(&blob)
                                .await
                                .expect("자기검증: park 시점에 정본은 아직 디스크에 있다");
                        })
                    },
                )),
                post_grave: Some(recorder(moved.clone())),
                ..Hooks::default()
            }
        };
        let s = Store::with_hooks(root.clone(), hooks);

        let stats = reconcile::run_once_at_for_test(&s, SystemTime::now(), WLOG_GRACE, WLOG_SETTLE)
            .await
            .expect("PASS ABORTED — 회수 직전 사라진 정본은 skip되고 패스는 완주한다");

        // ★ self-verify — **`SourceGone` 팔을 밟았다는 직접 증거**(W-GRAVE-CD-B와 같은 논증):
        //   ① `pre_grave` 1회 ⇒ tombstone이 만료였고 `read()`·sha 검증을 통과했다 = 회수 지점에 닿았다
        //   ② **`post_grave` 0회** ⇒ rename이 `Renamed::Done`이 **아니었다** ⇒ `Graved`도 `settle()`도
        //      태어나지 않았다 ⇒ 남는 팔은 **`SourceGone` 하나뿐이다**
        //   ③ 무덤이 디스크에 **없다** ⇒ rename은 정말로 일어나지 않았다
        assert_eq!(
            *parked.lock().unwrap(),
            vec![sha.clone()],
            "`pre_grave`는 **그 sha 하나에 대해 정확히 1회** 발화한다"
        );
        assert_eq!(
            *moved.lock().unwrap(),
            Vec::<String>::new(),
            "**`post_grave` 0회** ⇒ 무덤은 태어나지 않았다 ⇒ `SourceGone` 팔이다"
        );
        assert!(
            !tokio::fs::try_exists(&blob).await.unwrap(),
            "정본은 사라졌다"
        );
        assert!(
            !tokio::fs::try_exists(layout.grave_path(&sha))
                .await
                .unwrap(),
            "무덤은 **태어나지 않았다** — rename이 ENOENT였다"
        );
        assert_eq!(
            stats,
            ReconcileStats::default(),
            "회수하지 못한 blob은 `gc_deleted`도 `gc_pending`도 올리지 않는다"
        );

        // ★ **침묵** — `settle()`의 `Restored` INFO도 `Deferred` ERROR도 없다(무덤이 없으므로).
        assert_eq!(
            *logs.lock().unwrap(),
            Vec::<String>::new(),
            "**`grave()`의 `SourceGone` 팔**(`:280`)의 skip은 어떤 레벨에서도 이벤트를 내지 않는다"
        );
    }

    // ── 무대 ⑤: **무덤 루프 `remove()`**(`:149`) — 정본 **무손상** ⇒ remove 분기 ────────────────
    {
        let (logs, _g) = tap();

        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        tokio::fs::create_dir_all(root.join(".objects"))
            .await
            .unwrap();

        let layout = crate::layout::Layout::new(root.clone());
        let seeds: [&[u8]; 2] = [b"wlog-d-keep-0", b"wlog-d-keep-1"];
        let shas: Vec<String> = seeds.iter().map(|s| hex_sha(s)).collect();

        // 첫 `pre_recover_grave` 발화에서 **처리 중인 그 무덤**을 지운다(readdir 순서 무관 — *어느* 것이
        // 첫 번째든 "첫 무덤이 사라진다"는 명제는 항상 참이다).
        let victim: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let fired: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let hooks = {
            let (layout, victim, fired) = (layout.clone(), victim.clone(), fired.clone());
            Hooks {
                pre_recover_grave: Some(on_first(
                    move |sha| fired.lock().unwrap().push(sha.to_owned()),
                    move |sha| {
                        let (layout, victim) = (layout.clone(), victim.clone());
                        Box::pin(async move {
                            *victim.lock().unwrap() = Some(sha.clone()); // ⚠ 가드는 await를 넘지 않는다
                            tokio::fs::remove_file(layout.grave_path(&sha))
                                .await
                                .expect("자기검증: 처리 중인 무덤은 아직 디스크에 있다");
                        })
                    },
                )),
                ..Hooks::default()
            }
        };
        let s = Store::with_hooks(root.clone(), hooks);

        // **정본은 무손상**(내용 = seed · 이름 = sha(seed)) ⇒ `blob_intact = true` ⇒ **remove 분기**.
        // ⚠ **무덤 내용은 쓰레기다** — 이것이 분기 판별기다: rename 분기로 갔다면 그 쓰레기가 정본을
        //   **덮어써서** 엔트리 루프가 그것을 **격리**하고 **WARN**을 냈을 것이다. 아래의
        //   `quarantined == 0` ∧ **정본 바이트 동일** ∧ **로그 0건**이 *"remove 분기를 탔다"*의 양성 증거다.
        for (seed, sha) in seeds.iter().zip(&shas) {
            tokio::fs::write(layout.blob_path(sha), seed).await.unwrap();
            tokio::fs::write(layout.grave_path(sha), b"wlog-d-garbage")
                .await
                .unwrap();
        }

        let stats = reconcile::run_once_at_for_test(&s, SystemTime::now(), WLOG_GRACE, WLOG_SETTLE)
            .await
            .expect("PASS ABORTED — 사라진 무덤은 skip되고 패스는 완주한다");

        // self-verify ① 훅이 **무덤 하나당 한 번** 발화했다 ⇒ 루프가 끝까지 돌았다 ∧ `file_type()`은
        //               둘 다 `Present`였다(`:133`을 타지 않았다)
        let survivor_victim = victim.lock().unwrap().clone().expect("훅이 발화했다");
        let mut got = fired.lock().unwrap().clone();
        got.sort();
        let mut want = shas.clone();
        want.sort();
        assert_eq!(
            got, want,
            "`pre_recover_grave`는 무덤 **하나당 한 번** 발화한다"
        );
        assert!(
            shas.contains(&survivor_victim),
            "victim은 심은 무덤 중 하나다"
        );

        // self-verify ② 무덤은 둘 다 사라졌다(하나는 **우리가**, 하나는 **패스가**) ∧ 정본은 둘 다
        //               **바이트 그대로**다 ⇒ **remove 분기**였다(쓰레기 무덤이 정본을 덮지 않았다)
        for (seed, sha) in seeds.iter().zip(&shas) {
            assert!(
                !tokio::fs::try_exists(layout.grave_path(sha)).await.unwrap(),
                "무덤은 남지 않는다"
            );
            assert_eq!(
                tokio::fs::read(layout.blob_path(sha)).await.unwrap(),
                seed.to_vec(),
                "정본은 **바이트 그대로**다 ⇒ 쓰레기 무덤이 덮어쓰지 않았다 ⇒ **remove 분기**였다"
            );
        }
        assert_eq!(
            stats,
            ReconcileStats {
                referenced: 0,
                gc_deleted: 0,
                gc_pending: 2, // 무손상 정본 둘이 **최초 관측** tombstone을 얻는다
                temps_deleted: 0,
                quarantined: 0, // 쓰레기가 정본을 덮지 않았다 ⇒ 격리 0
            },
            "정본이 무손상이면 무덤은 **폐기**된다 — 사라진 무덤은 그저 건너뛴다"
        );

        // ★ **침묵** — `recovered == 0`이므로 기존 INFO 2건도 나지 않는다.
        //   `:149`에 로그 한 줄을 넣는 뮤턴트(M-LOG-INFO-GRAVE의 remove 쪽 얼굴)가 여기서 죽는다.
        assert_eq!(
            *logs.lock().unwrap(),
            Vec::<String>::new(),
            "**무덤 루프 `remove()` 팔**(`:149`)의 skip은 어떤 레벨에서도 이벤트를 내지 않는다"
        );
    }

    // ── 무대 ⑥: **무덤 루프 `rename_durable_to()`**(`:154`) — 정본 **부재** ⇒ rename 분기 ────────
    {
        let (logs, _g) = tap();

        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();
        let objects = root.join(".objects");
        tokio::fs::create_dir_all(&objects).await.unwrap();

        // 첫 `pre_recover_grave` 발화에서 **나머지 무덤을 전부 삭제**한다(발화 중인 것은 남긴다).
        // 무덤 루프는 스냅샷 순서로 돌므로 **첫 발화 항목 뒤는 전부 미처리**다 ⇒ 결정적.
        let survivors: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let fired: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seeds: [&[u8]; 3] = [b"wlog-d-grave-0", b"wlog-d-grave-1", b"wlog-d-grave-2"];
        let shas: Vec<String> = seeds.iter().map(|s| hex_sha(s)).collect();

        // ⚠ 무덤 경로는 **`Layout`이 짓는다**(이 파일의 다른 곳과 같은 방식 — ADR-0001: 온디스크
        //   이름의 **저작**은 Layout의 것이다).
        let layout = crate::layout::Layout::new(root.clone());
        let hooks = {
            let (layout, survivors, fired, shas) = (
                layout.clone(),
                survivors.clone(),
                fired.clone(),
                shas.clone(),
            );
            let hook = on_first(
                move |sha| fired.lock().unwrap().push(sha.to_owned()),
                move |sha| {
                    let (layout, survivors, shas) =
                        (layout.clone(), survivors.clone(), shas.clone());
                    Box::pin(async move {
                        *survivors.lock().unwrap() = Some(sha.clone()); // ⚠ 가드는 await를 넘지 않는다
                        for other in shas.iter().filter(|s| **s != sha) {
                            tokio::fs::remove_file(layout.grave_path(other))
                                .await
                                .expect("자기검증: 미처리 무덤은 아직 디스크에 있어야 한다");
                        }
                    })
                },
            );
            Hooks {
                pre_recover_grave: Some(hook),
                ..Hooks::default()
            }
        };
        let s = Store::with_hooks(root.clone(), hooks);

        // 무덤 3개 — **정본 blob은 전부 부재** ⇒ rename 분기. 내용은 **자기 sha와 정합**하게 심는다
        // (복원된 정본을 엔트리 루프가 재검증한다 — 어긋나면 격리 WARN이 나서 무대가 오염된다).
        for (seed, sha) in seeds.iter().zip(&shas) {
            tokio::fs::write(s.layout().grave_path(sha), seed)
                .await
                .unwrap();
        }

        let stats = reconcile::run_once_at_for_test(&s, SystemTime::now(), WLOG_GRACE, WLOG_SETTLE)
            .await
            .expect("PASS ABORTED — 사라진 무덤은 skip되고 패스는 완주한다");

        // self-verify: 훅이 **무덤 하나당 한 번씩** 발화했다 ⇒ 루프가 끝까지 돌았다(= skip 팔을 밟았다)
        let survivor = survivors.lock().unwrap().clone().expect("훅이 발화했다");
        let mut got = fired.lock().unwrap().clone();
        got.sort();
        let mut want = shas.clone();
        want.sort();
        assert_eq!(
            got, want,
            "`pre_recover_grave`는 무덤 **하나당 한 번** 발화한다 — 소멸한 둘도 포함(d_type 캐시)"
        );
        // 살아남은 하나만 복원됐다
        assert!(
            tokio::fs::try_exists(s.blob_path(&survivor)).await.unwrap(),
            "파킹되지 않은 무덤 하나는 정본으로 복원된다"
        );
        assert_eq!(
            stats,
            ReconcileStats {
                referenced: 0,
                gc_deleted: 0,
                gc_pending: 1, // 복원된 정본 하나가 **최초 관측** tombstone을 얻는다
                temps_deleted: 0,
                quarantined: 0, // 무덤 내용이 자기 sha와 정합 ⇒ 격리 0
            },
            "사라진 무덤 둘은 건너뛰고 살아남은 하나만 복원된다"
        );

        // ★ **기존 INFO 2건이 정확히 그만큼** — skip 팔은 **아무 말도 하지 않는다**.
        //   (§하류 표 4·5: `grave recovered` + `graves recovered from a previous pass`.)
        //   `:154`에 `info!` 한 줄을 넣는 뮤턴트는 **여기서 죽는다** — 소멸한 무덤 둘이 각각 한 줄씩
        //   더 내므로 이 전수 `assert_eq!`가 RED가 된다.
        assert_eq!(
            *logs.lock().unwrap(),
            vec![
                format!("INFO files::store::reconcile grave recovered sha={survivor}"),
                "INFO files::store::reconcile graves recovered from a previous pass recovered=1"
                    .to_owned(),
            ],
            "**무덤 루프 `rename_durable_to()` 팔**(`:154`)은 침묵한다 — 발화하는 것은 *복원된 하나*의 \
             기존 INFO 2건뿐이다"
        );
    }
}
