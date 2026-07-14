# P-21 반증 증거 (plan gate r13, critical, confidence 0.99)

**판정: P-21은 반증됐다.** 프로덕션 경로에서 F-1의 `landed` 술어가 복원된 blob S를 실제로 지킨다.
P-21은 보호 술어를 `refs` 하나로 가정했으나, F-1이 그 자리에 **두 번째 술어 `landed`**를 심어 뒀다.

실험: 스크래치패드 복제본 2개(baseline / fixed). fixed는 **봉인 장치 0**인 "가장 위험한 픽스"
(`NotFound`만 `continue`, 다른 io 에러는 `?` 유지 — 부재 확인·`Absent` 토큰·자식 모듈 없음).
증인 파일은 두 복제본에서 **바이트 동일**. `tokio::spawn`/채널/sleep 미사용 — park은 `pre_entry` 훅
안에서 부작용을 완주까지 await한다(패스가 훅 퓨처를 폴링하는 동안 루프는 물리적으로 진행 불가).

## 증인 소스 (baseline/fixed 바이트 동일)

```rust
//! **P-21 실험 증인** — plan 게이트 r13의 결함 주장을 **돌려서** 판정한다.
//!
//! ## 판정할 주장(P-21)
//! > 진짜로 사라진 항목(= 의도된 플립)만으로도 복원된 blob의 같은-패스 삭제가 열린다.
//! > 참조(`collect_referenced`)와 pending tombstone은 항목 루프 **이전에** 포착된다.
//! > 첫 스냅샷 blob X가 진짜로 사라지면 baseline은 X에서 중단하지만 픽스된 패스는 **계속한다**.
//! > 그러면 만료 tombstone을 가졌고 **참조 수집 이후에 포인터가 복원된** 뒤쪽 blob S가 grave/reap된다.
//!
//! ## 안무(결정적 — spawn 0 · 채널 0 · sleep 0)
//! `pre_entry` 훅은 항목 루프 **안에서**, 그 항목의 **첫 FS 접촉 직전**에 발화한다. 훅 **안에서**
//! 부작용을 **완주까지 await**하면 그것이 곧 park다 — 패스는 그 훅의 퓨처를 폴링하는 중이므로 루프는
//! **한 발짝도 나아가지 못한다**. `tokio::spawn`을 쓰지 않으므로 "spawn ≠ 폴링됨"(함정 8) 함정이
//! **구조적으로 존재하지 않는다**. 해제 신호도 필요 없다 — 훅이 반환하면 그대로 재개된다.
//!
//! ⓐ 만료 tombstone을 가진 **미참조 blob 2개**(X, S)를 심는다(포인터 0 ⇒ `referenced == 0`이 구조적).
//! ⓑ 패스를 **직접** 돌린다(주입형 시각 `t0`).
//! ⓒ **첫 `pre_entry`**에서: 그 이름을 X로 확정하고 —
//!    · X의 blob을 **진짜로 삭제**한다(스냅샷 이후 소멸 = 의도된 플립 발동)
//!    · **S의 커밋 포인터를 만든다** — 두 경로를 **각각** 시험한다:
//!      **(b1)** `Store::put` 프로덕션 경로(dedup → 핀 → 무취소 커밋) — F-1의 보호가 작동해야 하는 경로
//!      **(b2)** `.meta.json` **직접 기록**(핀 없음 · `landed` 없음) — F-1의 보호를 **우회**하는 경로
//! ⓓ 훅 반환 → 패스 완주(또는 중단) → **S의 blob이 살아 있는가? S가 GET으로 서빙되는가?**
//!
//! ## 이 파일은 baseline 복제본과 **바이트 동일**하다
//! 단언은 전부 **안전 속성**("S는 죽으면 안 된다")이다. 어느 복제본·어느 경로에서 RED가 나느냐가 곧 판정이다.

use super::*;
use crate::meta::ObjectMeta;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

/// S의 커밋 포인터를 **어떻게** 복원하는가.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Restore {
    /// (b1) `Store::put` — dedup → 핀 → 무취소 커밋. **F-1이 지켜야 하는 경로.**
    ProductionPut,
    /// (b2) `.meta.json` 직접 기록 — 핀도 `landed`도 없다. **F-1을 우회하는 경로.**
    RawMetaJson,
}

/// X를 park 중에 **진짜로** 지우는가(= 의도된 플립을 발동시키는가).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Vanish {
    Yes,
    No,
}

/// 패스가 남긴 **원문 관측**. 요약하지 않는다.
struct Observed {
    entries: Vec<String>,   // `pre_entry` 발화 순서
    collected: Vec<String>, // `during_collect` — **비어 있어야** 한다
    pre_graved: Vec<String>,
    graved: Vec<String>, // blob→무덤 rename **성공**
    /// **메커니즘 프로브 ①** — 무덤을 파기 **직전**의 `landed(sha)`.
    /// `pins.rs:266 fn landed()`가 `settle()`의 **유일한 보호 술어**다(P2).
    landed_at_grave: Vec<(String, bool)>,
    /// **메커니즘 프로브 ②** — `restore_io` 훅. `settle()`의 **보호 팔에서만** 발화한다
    /// (`pins.rs:610`: `if protect { self.pass.pins.hooks.restore_io(&self.sha)?; rename(무덤→정본) }`).
    /// 여기 이름이 오르면 그 sha는 `Restored`(또는 `Deferred`)다 — **회수되지 않았다**.
    restored: Vec<String>,
    x_sha: String,
    s_sha: String,
    s_expected: Vec<u8>,
    pass: std::io::Result<ReconcileStats>,
    x_blob_exists: bool,
    s_blob_exists: bool,
    s_pointer_exists: bool,
    s_get: Result<Vec<u8>, String>,
    graves_left: Vec<String>,
    pending_after: Option<HashMap<String, u64>>,
}

/// **첫 `pre_entry` 발화에서만** 부작용을 돌리는 훅.
/// `armed`는 클로저 **밖**에 산다 → 발화마다 새로 만들어지지 않는다(그러면 매 항목에서 부작용이 돈다).
/// `Store`는 `late`로 **늦게** 주입된다 — 훅이 `Store`를 품고 `Store`가 훅을 품는 순환을 끊는다.
/// `late`는 패스 시작 **전에** 채워지므로 발화 시점에는 반드시 `Some`이다(`expect`가 못박는다).
fn act_at_first_entry(
    late: Arc<Mutex<Option<Store>>>,
    contents: HashMap<String, Vec<u8>>,
    restore: Restore,
    vanish: Vanish,
    seen: Arc<Mutex<Vec<String>>>,
    first: Arc<Mutex<Option<String>>>,
) -> AsyncHook {
    let armed = Arc::new(AtomicBool::new(true));
    Arc::new(move |name: &str| {
        let (late, contents, seen, first, armed, name) = (
            late.clone(),
            contents.clone(),
            seen.clone(),
            first.clone(),
            armed.clone(),
            name.to_owned(),
        );
        Box::pin(async move {
            seen.lock().unwrap().push(name.clone());
            if !armed.swap(false, Ordering::SeqCst) {
                return; // 두 번째 항목부터는 통과 — 부작용은 **첫 항목에서 딱 한 번**
            }
            *first.lock().unwrap() = Some(name.clone());
            let s = late
                .lock()
                .unwrap()
                .clone()
                .expect("훅 발화 시점에 Store는 이미 주입돼 있다");

            // 첫 항목 = X. 나머지 하나 = S.
            let x_sha = name.clone();
            let s_sha = contents
                .keys()
                .find(|k| **k != x_sha)
                .expect("첫 항목은 심은 blob 중 하나여야 한다(예약 이름은 훅 이전에 skip)")
                .clone();
            let s_content = contents[&s_sha].clone();

            // (a) **X를 진짜로 삭제한다** — 스냅샷 이후 소멸(= 의도된 플립 발동).
            if vanish == Vanish::Yes {
                tokio::fs::remove_file(s.blob_path(&x_sha))
                    .await
                    .expect("X는 park 시점에 아직 디스크에 있어야 한다(= 이 항목은 미처리다)");
            }

            // (b) **S의 커밋 포인터를 만든다** — `collect_referenced`는 **이미 지나갔다**.
            match restore {
                Restore::ProductionPut => {
                    // dedup: S의 blob은 그대로 있다 → `blob_intact` == true → 바이트 재기록 없음
                    // → `commit_pointer`(무취소) → rename Ok → `landed.insert(S)`.
                    s.put("b", "s.bin", "text/plain", "u", s_content)
                        .await
                        .expect("dedup put은 성공한다(실패하면 엉뚱한 이유로 RED다)");
                }
                Restore::RawMetaJson => {
                    // 핀도 `landed`도 만들지 않는다. **포인터만** 디스크에 나타난다.
                    let meta = ObjectMeta {
                        content_type: "text/plain".into(),
                        size: s_content.len() as u64,
                        sha256: s_sha.clone(),
                        created_at: "2026-01-01T00:00:00Z".into(),
                        uploaded_by: "u".into(),
                    };
                    atomic::write_atomic(
                        &s.meta_for("b", "s.bin").unwrap(),
                        &serde_json::to_vec(&meta).unwrap(),
                    )
                    .await
                    .expect("포인터 직접 기록");
                }
            }
        })
    })
}

fn recorder(sink: Arc<Mutex<Vec<String>>>) -> AsyncHook {
    Arc::new(move |sha: &str| {
        let (sink, sha) = (sink.clone(), sha.to_owned());
        Box::pin(async move {
            sink.lock().unwrap().push(sha);
        })
    })
}

/// **메커니즘 프로브 ①** — `pre_grave`(= 무덤 rename **직전**)에서 `landed(sha)`를 읽는다.
/// `landed_has`는 이 모듈(=`pins`의 자손)에서만 볼 수 있는 등록부 직접 조회다.
/// 이것이 `settle()`의 판정을 **선행 관측**한다: `true` ⇒ `Settled::Restored` · `false` ⇒ `Settled::Reaped`.
fn grave_probe(
    late: Arc<Mutex<Option<Store>>>,
    pre_graved: Arc<Mutex<Vec<String>>>,
    landed_at_grave: Arc<Mutex<Vec<(String, bool)>>>,
) -> AsyncHook {
    Arc::new(move |sha: &str| {
        let (late, pre_graved, landed_at_grave, sha) = (
            late.clone(),
            pre_graved.clone(),
            landed_at_grave.clone(),
            sha.to_owned(),
        );
        Box::pin(async move {
            let s = late.lock().unwrap().clone().expect("Store 주입 완료");
            let l = landed_has(&s, &sha);
            pre_graved.lock().unwrap().push(sha.clone());
            landed_at_grave.lock().unwrap().push((sha, l));
        })
    })
}

/// **메커니즘 프로브 ②** — `settle()`의 **보호 팔**(`protect == true`)에서만 발화하는 `restore_io`.
/// 실패는 주입하지 않는다(`Ok(())`) — **발화 사실 자체**가 증거다.
fn restore_probe(sink: Arc<Mutex<Vec<String>>>) -> FailHook {
    Arc::new(move |sha: &str| {
        sink.lock().unwrap().push(sha.to_owned());
        Ok(())
    })
}

/// 시나리오 1회. **자기검증 단언은 여기 안에 있다** — 하니스가 헛돌면 시끄럽게 죽는다.
async fn run(restore: Restore, vanish: Vanish) -> (Observed, tempfile::TempDir) {
    let entries: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let pre_graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let graved: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let landed_at_grave: Arc<Mutex<Vec<(String, bool)>>> = Arc::new(Mutex::new(Vec::new()));
    let restored: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let first: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();

    // 바이트는 순수하게 먼저 정한다(sha는 내용에서 나온다 — Store가 필요 없다).
    let a_content = b"p21-blob-A".to_vec();
    let b_content = b"p21-blob-B".to_vec();
    let a_sha = hex_sha(&a_content);
    let b_sha = hex_sha(&b_content);
    let mut contents: HashMap<String, Vec<u8>> = HashMap::new();
    contents.insert(a_sha.clone(), a_content);
    contents.insert(b_sha.clone(), b_content);

    // **데이터 루트 하나당 Store 하나**(D-3). 훅 안의 put도 이 Store를 쓴다 → **같은 핀 등록부**(D-1).
    let late: Arc<Mutex<Option<Store>>> = Arc::new(Mutex::new(None));
    let hooks = Hooks {
        during_collect: Some(recorder(collected.clone())),
        pre_entry: Some(act_at_first_entry(
            late.clone(),
            contents.clone(),
            restore,
            vanish,
            entries.clone(),
            first.clone(),
        )),
        pre_grave: Some(grave_probe(
            late.clone(),
            pre_graved.clone(),
            landed_at_grave.clone(),
        )),
        post_grave: Some(recorder(graved.clone())),
        restore_io: Some(restore_probe(restored.clone())),
        ..Hooks::default()
    };
    let s = Store::with_hooks(root.clone(), hooks);
    *late.lock().unwrap() = Some(s.clone()); // 훅과 GC가 **같은 등록부**를 본다

    // ⓐ 미참조 blob 2개(포인터 0) + **만료** tombstone 2개.
    tokio::fs::create_dir_all(root.join(".objects")).await.unwrap();
    for (sha, bytes) in &contents {
        atomic::write_atomic(&s.blob_path(sha), bytes).await.unwrap();
    }
    let t0 = SystemTime::now();
    seed_expired_tombstones(&root, t0, &[&a_sha, &b_sha]).await;

    // ⓑ 패스를 **직접** 돌린다 — spawn 0. park는 훅 안의 인라인 await다.
    let pass = timeout(
        Duration::from_secs(20),
        reconcile::run_once_at_for_test(&s, t0, GRACE, SETTLE),
    )
    .await
    .expect("패스는 유한 시간에 끝난다(hang이면 시나리오가 아니라 하니스 결함이다)");

    // ── 자기검증 — "조용한 초록" 금지 ───────────────────────────────────────────────
    let entries_v = entries.lock().unwrap().clone();
    let collected_v = collected.lock().unwrap().clone();
    let pre_graved_v = pre_graved.lock().unwrap().clone();
    let graved_v = graved.lock().unwrap().clone();

    let x_sha = first
        .lock()
        .unwrap()
        .clone()
        .expect("자기검증 ①: `pre_entry` 훅이 **실제로 발화**해야 한다 — 안 했으면 시나리오 자체가 없다");
    assert!(
        contents.contains_key(&x_sha),
        "자기검증 ②: 첫 `pre_entry` 항목({x_sha})은 우리가 심은 blob이어야 한다"
    );
    assert_eq!(
        entries_v.first(),
        Some(&x_sha),
        "자기검증 ③: 첫 발화 항목이 곧 X여야 한다"
    );
    assert!(
        collected_v.is_empty(),
        "자기검증 ④: `collect_referenced`는 포인터가 **하나도 없는** 상태에서 스냅샷을 떴다 \
         ⇒ S의 포인터는 **반드시 collect 이후**에 생긴 것이다. collected={collected_v:?}"
    );
    if let Ok(st) = &pass {
        assert_eq!(
            st.referenced, 0,
            "자기검증 ④': 참조 스냅샷은 **비어 있다**(S는 refs에 없다)"
        );
    }
    if vanish == Vanish::Yes {
        assert!(
            !pre_graved_v.contains(&x_sha),
            "자기검증 ⑤: 사라진 X는 **미처리**여야 한다(grave 분기 진입 0). pre_graved={pre_graved_v:?}"
        );
        assert!(
            !graved_v.contains(&x_sha),
            "자기검증 ⑤': 사라진 X의 무덤이 파였다면 시나리오가 아니다. graved={graved_v:?}"
        );
    }

    // ── 메커니즘 교차검증 — **판정 술어는 `landed` 하나뿐이다**(P2) ────────────────────
    // 무덤을 판 모든 sha에 대해: `landed(sha)` ⇔ `restore_io` 발화(= settle의 보호 팔) ⇔ blob 생존.
    // 이 셋이 어긋나면 내 설명이 틀린 것이다 → **시끄럽게 죽는다**.
    let landed_at_grave_v = landed_at_grave.lock().unwrap().clone();
    let restored_v = restored.lock().unwrap().clone();
    for (sha, l) in &landed_at_grave_v {
        let blob_alive = tokio::fs::try_exists(s.blob_path(sha)).await.unwrap();
        assert_eq!(
            *l,
            restored_v.contains(sha),
            "메커니즘: 무덤을 판 {sha}의 운명은 `landed`가 **단독으로** 정한다 — \
             landed={l} 인데 restore_io(보호 팔) 발화={} 다",
            restored_v.contains(sha)
        );
        assert_eq!(
            *l, blob_alive,
            "메커니즘: landed={l} ⇒ blob 생존={l} 이어야 한다(Restored=복원 / Reaped=회수). 실제={blob_alive}"
        );
    }

    let s_sha = contents
        .keys()
        .find(|k| **k != x_sha)
        .expect("S는 X가 아닌 나머지 하나다")
        .clone();
    let s_expected = contents[&s_sha].clone();
    let s_pointer_exists = tokio::fs::try_exists(root.join("b").join("s.bin.meta.json"))
        .await
        .unwrap();
    assert!(
        s_pointer_exists,
        "자기검증 ⑥: S의 커밋 포인터가 **실제로 만들어졌어야** 한다(restore={restore:?})"
    );

    let obs = Observed {
        entries: entries_v,
        collected: collected_v,
        pre_graved: pre_graved_v,
        graved: graved_v,
        landed_at_grave: landed_at_grave.lock().unwrap().clone(),
        restored: restored.lock().unwrap().clone(),
        x_blob_exists: tokio::fs::try_exists(s.blob_path(&x_sha)).await.unwrap(),
        s_blob_exists: tokio::fs::try_exists(s.blob_path(&s_sha)).await.unwrap(),
        s_pointer_exists,
        s_get: match s.get_bytes("b", "s.bin").await {
            Ok((_, b)) => Ok(b),
            Err(e) => Err(format!("{e:?}")),
        },
        graves_left: grave_names(&root).await,
        pending_after: match tokio::fs::read(s.layout().gc_pending_path()).await {
            Ok(raw) => Some(serde_json::from_slice(&raw).unwrap()),
            Err(_) => None,
        },
        x_sha,
        s_sha,
        s_expected,
        pass,
    };
    (obs, d)
}

/// 원문 출력 — **요약하지 않는다.**
fn dump(tag: &str, o: &Observed) {
    println!("\n===== P21 {tag} =====");
    println!("X (첫 pre_entry 항목)  = {}", o.x_sha);
    println!("S (뒤쪽 blob)          = {}", o.s_sha);
    println!("pre_entry 발화 순서    = {:?}", o.entries);
    println!(
        "during_collect 관측    = {:?}  (빔 = 포인터 0에서 refs 스냅샷 → S는 refs에 없다)",
        o.collected
    );
    println!("pre_grave 관측         = {:?}", o.pre_graved);
    println!("post_grave 관측        = {:?}  (= blob→무덤 rename 성공)", o.graved);
    println!(
        "landed(sha) @무덤직전  = {:?}  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)",
        o.landed_at_grave
    );
    println!(
        "restore_io 발화(=보호팔) = {:?}  (pins.rs:610 `if protect {{ restore_io; rename(무덤→정본) }}`)",
        o.restored
    );
    match &o.pass {
        Ok(st) => println!("패스 결과              = Ok({st:?})"),
        Err(e) => println!("패스 결과              = Err(kind={:?}) {e}", e.kind()),
    }
    println!("X blob 존재            = {}", o.x_blob_exists);
    println!("S blob 존재            = {}   <<<<<<", o.s_blob_exists);
    println!("S 포인터 존재          = {}", o.s_pointer_exists);
    match &o.s_get {
        Ok(b) => println!(
            "S GET                  = Ok({} bytes = {:?})",
            b.len(),
            String::from_utf8_lossy(b)
        ),
        Err(e) => println!("S GET                  = Err({e})   <<<<<< 영구 404"),
    }
    println!("무덤 잔재              = {:?}", o.graves_left);
    println!("`.gc-pending.json`     = {:?}", o.pending_after);
    println!("===== end =====\n");
}

/// **S가 죽지 않았다**는 안전 속성. 이것이 RED가 되는 (복제본 × 경로)가 곧 P-21의 답이다.
fn assert_s_survives(o: &Observed) {
    assert!(
        o.s_blob_exists,
        "★ 데이터 손실 ★ 복원된 포인터가 가리키는 blob S({})가 **같은 패스에서 회수됐다** \
         → 포인터만 남고 blob 부재 = **영구 404**. pre_graved={:?} post_graved={:?} pass={:?}",
        o.s_sha,
        o.pre_graved,
        o.graved,
        o.pass.as_ref().map(|s| format!("{s:?}"))
    );
    let got = o
        .s_get
        .as_ref()
        .unwrap_or_else(|e| panic!("★ 데이터 손실 ★ S의 객체가 GET으로 서빙되지 않는다 — {e}"));
    assert_eq!(*got, o.s_expected, "S의 바이트가 온전해야 한다");
    assert!(o.graves_left.is_empty(), "무덤 잔재 0");
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  (b1) 프로덕션 경로 — `Store::put`(dedup → 핀 → 무취소 커밋). **F-1의 보호가 작동해야 하는 경로.**
// ══════════════════════════════════════════════════════════════════════════════════════════

/// **P-21 (b1) + 진짜 소멸.** Codex가 요구한 증인 — 단, 포인터를 **프로덕션 경로**로 복원한다.
#[tokio::test]
async fn p21_b1_production_put_after_collect_with_x_vanishing() {
    let (o, _d) = run(Restore::ProductionPut, Vanish::Yes).await;
    dump("b1 · vanish=YES · restore=Store::put", &o);
    assert_s_survives(&o);
}

/// **대조군 (b1) — X는 사라지지 않는다.** F-1의 핀/`landed`/settle이 단독으로 S를 지키는가?
#[tokio::test]
async fn p21_b1_production_put_after_collect_without_vanishing() {
    let (o, _d) = run(Restore::ProductionPut, Vanish::No).await;
    dump("b1 · vanish=NO · restore=Store::put", &o);
    assert_s_survives(&o);
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  (b2) 우회 경로 — `.meta.json` 직접 기록(핀 없음). **F-1의 보호를 우회한다.**
// ══════════════════════════════════════════════════════════════════════════════════════════

/// **P-21 (b2) + 진짜 소멸.**
#[tokio::test]
async fn p21_b2_raw_pointer_after_collect_with_x_vanishing() {
    let (o, _d) = run(Restore::RawMetaJson, Vanish::Yes).await;
    dump("b2 · vanish=YES · restore=raw .meta.json", &o);
    assert_s_survives(&o);
}

/// **대조군 (b2) — X는 사라지지 않는다.** 여기서도 S가 죽는다면 (b2)의 손실은 **픽스가 만든 것이
/// 아니라 원래 있던 구멍**이다(= 픽스는 우연한 방벽 하나를 치웠을 뿐이다).
#[tokio::test]
async fn p21_b2_raw_pointer_after_collect_without_vanishing() {
    let (o, _d) = run(Restore::RawMetaJson, Vanish::No).await;
    dump("b2 · vanish=NO · restore=raw .meta.json", &o);
    assert_s_survives(&o);
}
```

## 근사 픽스 diff (fixed 복제본, 파일 1개)

```diff
--- /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/p21/baseline/src/store/reconcile.rs	2026-07-14 00:50:06
+++ /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/p21/fixed/src/store/reconcile.rs	2026-07-14 12:59:27
@@ -174,6 +174,20 @@
     let mut rd = tokio::fs::read_dir(&objects).await?;
     while let Some(e) = rd.next_entry().await? {
         entries.push(e);
+    }
+
+    // ⚠⚠ **P-21 실험용 근사 픽스 (F-14의 플립만)** ⚠⚠
+    // 스냅샷 이후 사라진 항목(`ErrorKind::NotFound`)은 **그 항목만 건너뛴다**. 다른 io 에러는 `?` 그대로.
+    // 봉인 장치(부재 확인 · `Absent` 토큰 · 자식 모듈)는 **일부러 넣지 않았다** — P-21은 "플립 자체가
+    // 데이터 손실을 여는가"를 묻는 질문이므로 **가장 위험한 버전**으로 시험한다.
+    macro_rules! skip_if_vanished {
+        ($e:expr) => {
+            match $e {
+                Ok(v) => v,
+                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
+                Err(err) => return Err(err),
+            }
+        };
     }
 
     for e in entries {
@@ -196,23 +210,25 @@
         // 반환값은 `()`다 → 이 훅은 아래 분기 판정에 **개입할 수 없다**(P4 봉인 유지).
         pass.pins().hooks().pre_entry(&name).await;
         // O2: 디렉터리 스킵은 temp/blob 처리보다 앞.
-        let ft = e.file_type().await?;
+        let ft = skip_if_vanished!(e.file_type().await);
         if ft.is_dir() {
             continue;
         }
         match class {
             // 3) temp 잔재: mtime이 grace보다 오래된 것만 삭제(활성 스트리밍 보존)
             ObjectsEntry::Temp => {
-                let mtime = e.metadata().await?.modified().unwrap_or(now);
+                let mtime = skip_if_vanished!(e.metadata().await)
+                    .modified()
+                    .unwrap_or(now);
                 let age = now.duration_since(mtime).unwrap_or_default();
                 if age.as_secs() > grace_secs {
-                    tokio::fs::remove_file(&p).await?;
+                    skip_if_vanished!(tokio::fs::remove_file(&p).await);
                     stats.temps_deleted += 1;
                 }
             }
             ObjectsEntry::Blob => {
                 // 4) 무결성: 내용 sha == 파일명 검증, 불일치 → 격리
-                let content = tokio::fs::read(&p).await?;
+                let content = skip_if_vanished!(tokio::fs::read(&p).await);
                 if hex::encode(Sha256::digest(&content)) != name {
                     atomic::mkdir_p_durable(&corrupt_dir).await?;
                     tokio::fs::rename(&p, corrupt_dir.join(&name)).await?;
```

## 원문 출력 — fixed 복제본

```
   Compiling files v0.1.0 (/private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/p21/fixed)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.50s
     Running unittests src/lib.rs (target/debug/deps/files-3a147c720753eeaf)

running 4 tests
test store::pins::tests::p21_witness::p21_b1_production_put_after_collect_with_x_vanishing ... 
===== P21 b1 · vanish=YES · restore=Store::put =====
X (첫 pre_entry 항목)  = 647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746
S (뒤쪽 blob)          = 78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247
pre_entry 발화 순서    = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
during_collect 관측    = []  (빔 = 포인터 0에서 refs 스냅샷 → S는 refs에 없다)
pre_grave 관측         = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
post_grave 관측        = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (= blob→무덤 rename 성공)
landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)
restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)
패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })
X blob 존재            = false
S blob 존재            = true   <<<<<<
S 포인터 존재          = true
S GET                  = Ok(10 bytes = "p21-blob-B")
무덤 잔재              = []
`.gc-pending.json`     = Some({"78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247": 1783994815})
===== end =====

ok
test store::pins::tests::p21_witness::p21_b1_production_put_after_collect_without_vanishing ... 
===== P21 b1 · vanish=NO · restore=Store::put =====
X (첫 pre_entry 항목)  = 647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746
S (뒤쪽 blob)          = 78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247
pre_entry 발화 순서    = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
during_collect 관측    = []  (빔 = 포인터 0에서 refs 스냅샷 → S는 refs에 없다)
pre_grave 관측         = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
post_grave 관측        = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (= blob→무덤 rename 성공)
landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)
restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)
패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })
X blob 존재            = false
S blob 존재            = true   <<<<<<
S 포인터 존재          = true
S GET                  = Ok(10 bytes = "p21-blob-B")
무덤 잔재              = []
`.gc-pending.json`     = Some({"78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247": 1783994815})
===== end =====

ok
test store::pins::tests::p21_witness::p21_b2_raw_pointer_after_collect_with_x_vanishing ... 
===== P21 b2 · vanish=YES · restore=raw .meta.json =====
X (첫 pre_entry 항목)  = 647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746
S (뒤쪽 blob)          = 78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247
pre_entry 발화 순서    = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
during_collect 관측    = []  (빔 = 포인터 0에서 refs 스냅샷 → S는 refs에 없다)
pre_grave 관측         = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
post_grave 관측        = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (= blob→무덤 rename 성공)
landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)
restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)
패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })
X blob 존재            = false
S blob 존재            = false   <<<<<<
S 포인터 존재          = true
S GET                  = Err(NotFound)   <<<<<< 영구 404
무덤 잔재              = []
`.gc-pending.json`     = Some({})
===== end =====


thread 'store::pins::tests::p21_witness::p21_b2_raw_pointer_after_collect_with_x_vanishing' (16019182) panicked at src/store/pins/tests/p21_witness.rs:402:5:
★ 데이터 손실 ★ 복원된 포인터가 가리키는 blob S(78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247)가 **같은 패스에서 회수됐다** → 포인터만 남고 blob 부재 = **영구 404**. pre_graved=["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"] post_graved=["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"] pass=Ok("ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 }")
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
FAILED
test store::pins::tests::p21_witness::p21_b2_raw_pointer_after_collect_without_vanishing ... 
===== P21 b2 · vanish=NO · restore=raw .meta.json =====
X (첫 pre_entry 항목)  = 647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746
S (뒤쪽 blob)          = 78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247
pre_entry 발화 순서    = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
during_collect 관측    = []  (빔 = 포인터 0에서 refs 스냅샷 → S는 refs에 없다)
pre_grave 관측         = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
post_grave 관측        = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (= blob→무덤 rename 성공)
landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)
restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)
패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })
X blob 존재            = false
S blob 존재            = false   <<<<<<
S 포인터 존재          = true
S GET                  = Err(NotFound)   <<<<<< 영구 404
무덤 잔재              = []
`.gc-pending.json`     = Some({})
===== end =====


thread 'store::pins::tests::p21_witness::p21_b2_raw_pointer_after_collect_without_vanishing' (16019184) panicked at src/store/pins/tests/p21_witness.rs:402:5:
★ 데이터 손실 ★ 복원된 포인터가 가리키는 blob S(78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247)가 **같은 패스에서 회수됐다** → 포인터만 남고 blob 부재 = **영구 404**. pre_graved=["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"] post_graved=["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"] pass=Ok("ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 }")
FAILED

failures:

failures:
    store::pins::tests::p21_witness::p21_b2_raw_pointer_after_collect_with_x_vanishing
    store::pins::tests::p21_witness::p21_b2_raw_pointer_after_collect_without_vanishing

test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.23s

error: test failed, to rerun pass `--lib`
```

## 원문 출력 — baseline 복제본 (대조군)

```
   Compiling files v0.1.0 (/private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/p21/baseline)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.87s
     Running unittests src/lib.rs (target/debug/deps/files-3a147c720753eeaf)

running 4 tests
test store::pins::tests::p21_witness::p21_b1_production_put_after_collect_with_x_vanishing ... 
===== P21 b1 · vanish=YES · restore=Store::put =====
X (첫 pre_entry 항목)  = 647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746
S (뒤쪽 blob)          = 78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247
pre_entry 발화 순서    = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746"]
during_collect 관측    = []  (빔 = 포인터 0에서 refs 스냅샷 → S는 refs에 없다)
pre_grave 관측         = []
post_grave 관측        = []  (= blob→무덤 rename 성공)
landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)
restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)
패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)
X blob 존재            = false
S blob 존재            = true   <<<<<<
S 포인터 존재          = true
S GET                  = Ok(10 bytes = "p21-blob-B")
무덤 잔재              = []
`.gc-pending.json`     = Some({"647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746": 1783994828, "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247": 1783994828})
===== end =====

ok
test store::pins::tests::p21_witness::p21_b1_production_put_after_collect_without_vanishing ... 
===== P21 b1 · vanish=NO · restore=Store::put =====
X (첫 pre_entry 항목)  = 647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746
S (뒤쪽 blob)          = 78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247
pre_entry 발화 순서    = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
during_collect 관측    = []  (빔 = 포인터 0에서 refs 스냅샷 → S는 refs에 없다)
pre_grave 관측         = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
post_grave 관측        = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (= blob→무덤 rename 성공)
landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)
restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)
패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })
X blob 존재            = false
S blob 존재            = true   <<<<<<
S 포인터 존재          = true
S GET                  = Ok(10 bytes = "p21-blob-B")
무덤 잔재              = []
`.gc-pending.json`     = Some({"78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247": 1783994828})
===== end =====

ok
test store::pins::tests::p21_witness::p21_b2_raw_pointer_after_collect_with_x_vanishing ... 
===== P21 b2 · vanish=YES · restore=raw .meta.json =====
X (첫 pre_entry 항목)  = 647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746
S (뒤쪽 blob)          = 78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247
pre_entry 발화 순서    = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746"]
during_collect 관측    = []  (빔 = 포인터 0에서 refs 스냅샷 → S는 refs에 없다)
pre_grave 관측         = []
post_grave 관측        = []  (= blob→무덤 rename 성공)
landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)
restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)
패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)
X blob 존재            = false
S blob 존재            = true   <<<<<<
S 포인터 존재          = true
S GET                  = Ok(10 bytes = "p21-blob-B")
무덤 잔재              = []
`.gc-pending.json`     = Some({"647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746": 1783994828, "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247": 1783994828})
===== end =====

ok
test store::pins::tests::p21_witness::p21_b2_raw_pointer_after_collect_without_vanishing ... 
===== P21 b2 · vanish=NO · restore=raw .meta.json =====
X (첫 pre_entry 항목)  = 647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746
S (뒤쪽 blob)          = 78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247
pre_entry 발화 순서    = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
during_collect 관측    = []  (빔 = 포인터 0에서 refs 스냅샷 → S는 refs에 없다)
pre_grave 관측         = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]
post_grave 관측        = ["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (= blob→무덤 rename 성공)
landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)
restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)
패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })
X blob 존재            = false
S blob 존재            = false   <<<<<<
S 포인터 존재          = true
S GET                  = Err(NotFound)   <<<<<< 영구 404
무덤 잔재              = []
`.gc-pending.json`     = Some({})
===== end =====


thread 'store::pins::tests::p21_witness::p21_b2_raw_pointer_after_collect_without_vanishing' (16019778) panicked at src/store/pins/tests/p21_witness.rs:402:5:
★ 데이터 손실 ★ 복원된 포인터가 가리키는 blob S(78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247)가 **같은 패스에서 회수됐다** → 포인터만 남고 blob 부재 = **영구 404**. pre_graved=["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"] post_graved=["647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", "78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"] pass=Ok("ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 }")
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
FAILED

failures:

failures:
    store::pins::tests::p21_witness::p21_b2_raw_pointer_after_collect_without_vanishing

test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.20s

error: test failed, to rerun pass `--lib`
```

## 전 스위트 — fixed 복제본 (봉인 장치 0으로도 전부 초록)

```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.09s
     Running unittests src/lib.rs (target/debug/deps/files-3a147c720753eeaf)

running 120 tests
test capacity::tests::free_bytes_reports_positive_for_real_dir ... ok
test config::tests::parses_defaults_with_required ... ok
test config::tests::missing_required_errors ... ok
test config::tests::validate_requires_upload_timeout_below_grace ... ok
test clock::tests::now_is_parseable_rfc3339 ... ok
test capacity::tests::rejects_when_would_breach_min_free ... ok
test capacity::tests::reservation_accounting_and_raii_release ... ok
test capacity::tests::overcommit_prevented_then_freed ... ok
test error::tests::code_mapping ... ok
test error::tests::status_mapping ... ok
test auth::tests::malformed_keys_file_errors ... ok
test auth::tests::camelcase_fixture_scoped_read_write ... ok
test auth::tests::missing_scopes_default_to_empty ... ok
test http::internal::tests::healthz_ok ... ok
test http::internal::tests::create_bucket_non_admin_403 ... ok
test http::internal::tests::put_without_write_scope_403 ... ok
test http::internal::tests::create_reserved_bucket_400 ... ok
test http::public::tests::every_reserved_bucket_has_a_shadow_route ... ok
test http::public::tests::every_shadow_route_names_a_reserved_bucket ... ok
test http::internal::tests::get_missing_404 ... ok
test http::internal::tests::readyz_503_when_unwritable ... ok
test http::internal::tests::readyz_ok_when_writable ... ok
test http::internal::tests::create_bucket_admin_then_list ... ok
test http::internal::tests::head_returns_metadata_headers ... ok
test http::internal::tests::put_creates_201_then_get_roundtrip ... ok
test http::internal::tests::delete_then_get_404 ... ok
test http::public::tests::reserved_bucket_names_cannot_be_created ... ok
test http::ranged::tests::full_200_with_etag_and_length ... ok
test http::ranged::tests::if_none_match_304 ... ok
test http::internal::tests::list_files_returns_entries ... ok
test http::ranged::tests::partial_206_closed_range ... ok
test http::ranged::tests::open_ended_range_206 ... ok
test http::ranged::tests::suffix_range_206 ... ok
test http::ranged::tests::unknown_unit_ignored_full_200 ... ok
test http::tests::bad_bearer_is_401 ... ok
test http::ranged::tests::unsatisfiable_416 ... ok
test http::public::tests::catalog_lists_public_only ... ok
test http::tests::good_bearer_is_200 ... ok
test layout::tests::bucket_rules ... ok
test layout::tests::classify_objects_entry_table ... ok
test http::tests::missing_bearer_is_401 ... ok
test http::public::tests::internal_bucket_not_served_publicly_404 ... ok
test layout::tests::grave_name_round_trips ... ok
test layout::tests::hidden_and_control_chars_rejected ... ok
test layout::tests::making_methods_author_expected_paths ... ok
test layout::tests::meta_path_appends_suffix ... ok
test layout::tests::reserved_suffixes_rejected ... ok
test layout::tests::safe_object_path_stays_under_root ... ok
test layout::tests::temp_name_authors_prefix ... ok
test layout::tests::traversal_and_malformed_keys_rejected ... ok
test http::public::tests::no_method_reaches_api_surface_on_public ... ok
test layout::tests::valid_keys_accepted ... ok
test http::public::tests::public_api_path_404 ... ok
test http::tests::build_state_creates_objects_dir_and_loads_keys ... ok
test http::public::tests::missing_object_404 ... ok
test meta::tests::bucket_meta_roundtrip_camel_case ... ok
test meta::tests::object_meta_roundtrip_camel_case ... ok
test meta::tests::visibility_lowercase ... ok
test layout::tests::walker_rejects_reserved_bucket_and_empty_on_absent ... ok
test http::public::tests::public_download_200_with_security_headers ... ok
test store::locks::tests::busy_while_held_free_after_drop ... ok
test store::locks::tests::bucket_participates_in_lock_key ... ok
test store::atomic::tests::write_atomic_is_cancellable_before_rename ... ok
test store::locks::tests::different_keys_independent ... ok
test layout::tests::walker_round_trips_meta_for ... ok
test layout::tests::pointers_all_skips_objects_and_covers_buckets ... ok
test store::atomic::tests::mkdir_p_durable_creates_nested_idempotent ... ok
test store::locks::tests::lock_serializes_same_key ... ok
test store::atomic::tests::write_atomic_overwrites ... ok
test layout::tests::walker_yields_exactly_commit_pointers ... ok
test http::public::tests::reserved_route_shape_asymmetry_is_load_bearing ... ok
test store::atomic::tests::write_atomic_roundtrip_no_temp_residue ... ok
test store::pins::tests::drop_paths_survive_a_poisoned_registry_mutex ... ok
test store::pins::tests::commit_pointer_lands_and_releases_pin ... ok
test store::pins::tests::landed_trace_only_when_rename_returns_ok ... ok
test store::pins::tests::hooks_fire_on_production_put_path ... ok
test store::pins::tests::already_landed_at_grave_time_restores_without_waiting_for_the_cohort ... ok
test store::pins::tests::grave_planted_by_a_crashed_process_is_recovered_on_restart ... ok
test store::pins::tests::barrier_hooks_and_injected_clock_compose_in_one_witness ... ok
test store::pins::tests::pin_ids_are_monotonic_and_independent ... ok
test store::pins::tests::failed_commit_does_not_protect_blob_from_gc ... ok
test store::pins::tests::leaked_graved_token_leaves_a_grave_that_the_next_pass_recovers ... ok
test store::pins::tests::pin_and_put_do_not_block_while_pass_is_live ... ok
test store::pins::tests::commit_holds_key_lock_until_rename_lands ... ok
test store::pins::tests::pass_cancelled_after_grave_leaves_it_for_the_next_pass_to_recover ... ok
test store::pins::tests::store_clone_shares_pin_registry_but_new_does_not ... ok
test store::pins::tests::stage_failure_leaves_no_landed_trace ... ok
test store::pins::tests::caller_cancellation_mid_commit_still_protects_the_blob ... ok
test store::pins::tests::put_landing_between_pre_grave_and_grave_is_protected ... ok
test store::pins::tests::landing_during_settle_wait_is_woken_by_the_landed_notification ... ok
test store::pins::tests::restore_failure_keeps_the_grave_and_never_unlinks_it ... ok
test store::pins::tests::restore_failure_makes_the_reconcile_pass_return_the_raw_io_error ... ok
test store::pins::tests::put_landing_during_reference_collection_is_protected ... ok
test store::pins::tests::overlapping_failed_put_does_not_protect_the_blob ... ok
test store::reconcile::tests::corrupt_blob_quarantined ... ok
test store::reconcile::tests::old_temp_deleted_recent_preserved ... ok
test store::pins::tests::put_parked_after_observe_forces_cohort_wait_then_restore ... ok
test store::reconcile::tests::settle_timeout_derives_from_upload_timeout_and_is_monotonic ... ok
test store::reconcile::tests::recover_graves_adopts_the_grave_when_the_canonical_blob_is_rotten ... ok
test store::reconcile::tests::recover_graves_skips_a_directory_that_is_named_like_a_grave ... ok
test store::pins::tests::vanished_temp_regression::reconcile_pass_control_without_a_vanishing_temp_is_green ... ok
test store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot ... ok
test store::reconcile::tests::referenced_nested_blob_survives ... ok
test store::reconcile::tests::unreferenced_old_blob_is_gced ... ok
test store::tests::bucket_meta_roundtrip ... ok
test store::tests::list_empty_bucket_is_ok ... ok
test store::reconcile::tests::unreferenced_recent_blob_preserved ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_control_without_vanishing_entries_is_green ... ok
test store::tests::meta_pointing_to_missing_blob_is_not_found ... ok
test store::tests::delete_removes_pointer_idempotent ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot ... ok
test store::tests::put_get_roundtrip_content_addressed ... ok
test store::tests::put_stream_too_large_no_residue_not_committed ... ok
test store::pins::tests::wedged_commit_keeps_key_unwritable_and_says_so_loudly ... ok
test store::tests::list_buckets_returns_those_with_bucket_json ... ok
test store::tests::put_stream_heals_corrupt_blob ... ok
test store::tests::put_stream_roundtrip_large ... ok
test store::tests::list_returns_serving_only_with_nested_keys ... ok
test store::tests::same_size_overwrite_is_self_consistent ... ok
test store::pins::tests::stuck_pin_defers_reclamation_but_never_stalls_the_pass ... ok

test result: ok. 120 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 1.66s

```

## 전 스위트 — baseline (실패 2개 = 의도된 F-14 RED 증인)

```
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.24s
     Running unittests src/lib.rs (target/debug/deps/files-3a147c720753eeaf)

running 120 tests
test config::tests::missing_required_errors ... ok
test capacity::tests::free_bytes_reports_positive_for_real_dir ... ok
test config::tests::parses_defaults_with_required ... ok
test error::tests::code_mapping ... ok
test capacity::tests::reservation_accounting_and_raii_release ... ok
test capacity::tests::overcommit_prevented_then_freed ... ok
test capacity::tests::rejects_when_would_breach_min_free ... ok
test config::tests::validate_requires_upload_timeout_below_grace ... ok
test error::tests::status_mapping ... ok
test auth::tests::malformed_keys_file_errors ... ok
test clock::tests::now_is_parseable_rfc3339 ... ok
test auth::tests::missing_scopes_default_to_empty ... ok
test auth::tests::camelcase_fixture_scoped_read_write ... ok
test http::internal::tests::healthz_ok ... ok
test http::internal::tests::create_bucket_non_admin_403 ... ok
test http::internal::tests::create_reserved_bucket_400 ... ok
test http::internal::tests::put_without_write_scope_403 ... ok
test http::public::tests::every_reserved_bucket_has_a_shadow_route ... ok
test http::public::tests::every_shadow_route_names_a_reserved_bucket ... ok
test http::internal::tests::get_missing_404 ... ok
test http::internal::tests::readyz_503_when_unwritable ... ok
test http::internal::tests::readyz_ok_when_writable ... ok
test http::internal::tests::create_bucket_admin_then_list ... ok
test http::internal::tests::head_returns_metadata_headers ... ok
test http::public::tests::reserved_bucket_names_cannot_be_created ... ok
test http::internal::tests::delete_then_get_404 ... ok
test http::internal::tests::put_creates_201_then_get_roundtrip ... ok
test http::ranged::tests::full_200_with_etag_and_length ... ok
test http::internal::tests::list_files_returns_entries ... ok
test http::ranged::tests::if_none_match_304 ... ok
test http::ranged::tests::partial_206_closed_range ... ok
test http::ranged::tests::open_ended_range_206 ... ok
test http::ranged::tests::suffix_range_206 ... ok
test http::tests::bad_bearer_is_401 ... ok
test http::ranged::tests::unknown_unit_ignored_full_200 ... ok
test http::ranged::tests::unsatisfiable_416 ... ok
test http::tests::good_bearer_is_200 ... ok
test http::tests::missing_bearer_is_401 ... ok
test layout::tests::bucket_rules ... ok
test http::public::tests::catalog_lists_public_only ... ok
test layout::tests::classify_objects_entry_table ... ok
test layout::tests::hidden_and_control_chars_rejected ... ok
test layout::tests::grave_name_round_trips ... ok
test http::public::tests::missing_object_404 ... ok
test http::public::tests::internal_bucket_not_served_publicly_404 ... ok
test http::public::tests::public_api_path_404 ... ok
test layout::tests::making_methods_author_expected_paths ... ok
test http::public::tests::no_method_reaches_api_surface_on_public ... ok
test layout::tests::meta_path_appends_suffix ... ok
test layout::tests::reserved_suffixes_rejected ... ok
test layout::tests::safe_object_path_stays_under_root ... ok
test http::tests::build_state_creates_objects_dir_and_loads_keys ... ok
test layout::tests::temp_name_authors_prefix ... ok
test layout::tests::valid_keys_accepted ... ok
test layout::tests::traversal_and_malformed_keys_rejected ... ok
test meta::tests::bucket_meta_roundtrip_camel_case ... ok
test meta::tests::object_meta_roundtrip_camel_case ... ok
test meta::tests::visibility_lowercase ... ok
test layout::tests::walker_rejects_reserved_bucket_and_empty_on_absent ... ok
test store::locks::tests::bucket_participates_in_lock_key ... ok
test store::locks::tests::busy_while_held_free_after_drop ... ok
test store::locks::tests::different_keys_independent ... ok
test store::atomic::tests::write_atomic_is_cancellable_before_rename ... ok
test layout::tests::walker_round_trips_meta_for ... ok
test http::public::tests::public_download_200_with_security_headers ... ok
test layout::tests::pointers_all_skips_objects_and_covers_buckets ... ok
test store::atomic::tests::mkdir_p_durable_creates_nested_idempotent ... ok
test store::locks::tests::lock_serializes_same_key ... ok
test store::atomic::tests::write_atomic_roundtrip_no_temp_residue ... ok
test layout::tests::walker_yields_exactly_commit_pointers ... ok
test store::atomic::tests::write_atomic_overwrites ... ok
test store::pins::tests::drop_paths_survive_a_poisoned_registry_mutex ... ok
test http::public::tests::reserved_route_shape_asymmetry_is_load_bearing ... ok
test store::pins::tests::commit_pointer_lands_and_releases_pin ... ok
test store::pins::tests::hooks_fire_on_production_put_path ... ok
test store::pins::tests::landed_trace_only_when_rename_returns_ok ... ok
test store::pins::tests::already_landed_at_grave_time_restores_without_waiting_for_the_cohort ... ok
test store::pins::tests::grave_planted_by_a_crashed_process_is_recovered_on_restart ... ok
test store::pins::tests::pin_ids_are_monotonic_and_independent ... ok
test store::pins::tests::barrier_hooks_and_injected_clock_compose_in_one_witness ... ok
test store::pins::tests::failed_commit_does_not_protect_blob_from_gc ... ok
test store::pins::tests::leaked_graved_token_leaves_a_grave_that_the_next_pass_recovers ... ok
test store::pins::tests::pin_and_put_do_not_block_while_pass_is_live ... ok
test store::pins::tests::commit_holds_key_lock_until_rename_lands ... ok
test store::pins::tests::pass_cancelled_after_grave_leaves_it_for_the_next_pass_to_recover ... ok
test store::pins::tests::caller_cancellation_mid_commit_still_protects_the_blob ... ok
test store::pins::tests::store_clone_shares_pin_registry_but_new_does_not ... ok
test store::pins::tests::stage_failure_leaves_no_landed_trace ... ok
test store::pins::tests::put_landing_between_pre_grave_and_grave_is_protected ... ok
test store::pins::tests::landing_during_settle_wait_is_woken_by_the_landed_notification ... ok
test store::pins::tests::restore_failure_keeps_the_grave_and_never_unlinks_it ... ok
test store::pins::tests::restore_failure_makes_the_reconcile_pass_return_the_raw_io_error ... ok
test store::pins::tests::overlapping_failed_put_does_not_protect_the_blob ... ok
test store::pins::tests::put_landing_during_reference_collection_is_protected ... ok
test store::reconcile::tests::corrupt_blob_quarantined ... ok
test store::reconcile::tests::old_temp_deleted_recent_preserved ... ok
test store::pins::tests::put_parked_after_observe_forces_cohort_wait_then_restore ... ok
test store::reconcile::tests::settle_timeout_derives_from_upload_timeout_and_is_monotonic ... ok
test store::reconcile::tests::recover_graves_adopts_the_grave_when_the_canonical_blob_is_rotten ... ok
test store::reconcile::tests::recover_graves_skips_a_directory_that_is_named_like_a_grave ... ok
test store::pins::tests::vanished_temp_regression::reconcile_pass_control_without_a_vanishing_temp_is_green ... ok
test store::reconcile::tests::referenced_nested_blob_survives ... ok
test store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot ... FAILED
test store::reconcile::tests::unreferenced_old_blob_is_gced ... ok
test store::tests::list_empty_bucket_is_ok ... ok
test store::tests::bucket_meta_roundtrip ... ok
test store::reconcile::tests::unreferenced_recent_blob_preserved ... ok
test store::tests::meta_pointing_to_missing_blob_is_not_found ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot ... FAILED
test store::tests::delete_removes_pointer_idempotent ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_control_without_vanishing_entries_is_green ... ok
test store::tests::put_get_roundtrip_content_addressed ... ok
test store::tests::put_stream_too_large_no_residue_not_committed ... ok
test store::tests::list_buckets_returns_those_with_bucket_json ... ok
test store::tests::put_stream_heals_corrupt_blob ... ok
test store::tests::put_stream_roundtrip_large ... ok
test store::pins::tests::wedged_commit_keeps_key_unwritable_and_says_so_loudly ... ok
test store::tests::list_returns_serving_only_with_nested_keys ... ok
test store::tests::same_size_overwrite_is_self_consistent ... ok
test store::pins::tests::stuck_pin_defers_reclamation_but_never_stalls_the_pass ... ok

failures:

---- store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot stdout ----

thread 'store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot' (16024553) panicked at src/store/pins/tests/vanished_temp_regression.rs:196:9:
PASS ABORTED — 스냅샷 이후 사라진 **temp**(.tmp-f14-temp-victim)를 만난 reconcile 패스가 그 항목을 **건너뛰지 않고** 패스 **전체**를 Err로 중단시켰다. 범인 `?`는 Temp 분기의 `let mtime = e.metadata().await?…`(나이 판정 **전에** stat한다). 이것이 프론트매터가 적은 **바로 그 증상**이다: 동시 `put_stream`이 `.tmp-<uniq>`를 최종 blob 이름으로 rename하면 스냅샷에 잡힌 temp가 사라진다 → 패스 전체 중단. err=Os { code: 2, kind: NotFound, message: "No such file or directory" } kind=NotFound

---- store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot stdout ----

thread 'store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot' (16024544) panicked at src/store/pins/tests/vanished_entry_regression.rs:189:9:
PASS ABORTED — 스냅샷 이후 사라진 항목(victims=["02e8e4db0fb46bc832573124554faf3a24b05d4b4fe5d8e3e0a611ee6cd277aa", "e3dbdd09192f1cebd4185cf8ba31a68537920becf58c9d2c0bf81ab802c06b75"])을 만난 reconcile 패스가 그 항목을 **건너뛰지 않고** 패스 **전체**를 Err로 중단시켰다. 동시 쓰기(`atomic::write_atomic`의 `.tmp-<uniq>` → rename)가 있는 한 이것은 상시 발생한다. err=Os { code: 2, kind: NotFound, message: "No such file or directory" } kind=NotFound (NotFound = ENOENT: 범인 `?`는 reconcile.rs:199(Temp `metadata`) / :208(Blob `read`) — :192(`file_type`)는 DT_UNKNOWN FS에서의 잠복 범인)


failures:
    store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot
    store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot

test result: FAILED. 118 passed; 2 failed; 0 ignored; 0 measured; 4 filtered out; finished in 1.72s

error: test failed, to rerun pass `--lib`
```

## 결정성 20/20 — 판정 시그니처 집계

```
run01 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.21s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run02 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run03 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run04 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run05 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run06 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run07 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.18s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run08 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run09 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run10 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run11 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run12 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.18s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run13 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.18s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run14 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run15 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run16 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.18s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run17 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run18 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run19 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run20 :: test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.19s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = []  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Err(kind=NotFound) No such file or directory (os error 2)|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|

--- (baseline: 45KB 원문 중 앞부분. 20회 전부 동일 시그니처) ---

run01 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.24s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run02 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.22s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run03 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.21s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run04 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.22s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run05 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.22s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run06 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.22s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run07 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.21s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run08 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.23s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run09 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.22s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run10 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.22s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run11 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.21s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run12 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.22s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run13 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.21s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run14 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.22s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run15 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.21s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run16 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.21s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run17 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.23s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run18 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.22s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run19 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.22s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|
run20 :: test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 120 filtered out; finished in 0.22s :: ===== P21 b1 · vanish=YES · restore=Store::put =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 0, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b1 · vanish=NO · restore=Store::put =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", true)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = ["78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247"]  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 1, temps_deleted: 0, quarantined: 0 })|S blob 존재            = true   <<<<<<|S GET                  = Ok(10 bytes = "p21-blob-B")|===== P21 b2 · vanish=YES · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 1, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|===== P21 b2 · vanish=NO · restore=raw .meta.json =====|landed(sha) @무덤직전  = [("647c0cf6170f2285a8bddbcaf797a4cc3cfdc640f7a0ff47ac1c3e35282d8746", false), ("78b417a01f46b1487b7afe6c829be1ff78627b692598d1d7fdc52ad976c51247", false)]  <<<<<< settle()의 **유일한 보호 술어**(pins.rs:266)|restore_io 발화(=보호팔) = []  (pins.rs:610 `if protect { restore_io; rename(무덤→정본) }`)|패스 결과              = Ok(ReconcileStats { referenced: 0, gc_deleted: 2, gc_pending: 0, temps_deleted: 0, quarantined: 0 })|S blob 존재            = false   <<<<<<|S GET                  = Err(NotFound)   <<<<<< 영구 404|

--- (fixed: 51KB 원문 중 앞부분. 20회 전부 동일 시그니처) ---
```
