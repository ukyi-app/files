//! 회귀: reconcile GC ↔ dedup-put 경합(`reconcile-gc-dedup-race`).
//!
//! ## 증상(= 이 테스트의 판정 정의)
//!
//! `reconcile::run_once_at`은 패스 **시작 시점**에 뜬 참조 스냅샷(`collect_referenced`)을
//! 기준으로 blob 삭제를 판정한다. 그런데 `Store::put`의 **dedup 분기**(`if !intact` —
//! 기존 blob이 온전하면 바이트를 다시 쓰지 않고 커밋 포인터만 기록)는 reconcile이
//! 관측하지 않는 락(`KeyLocks`, Store private, bucket/key 키) 아래에서 새 참조를 커밋한다.
//! → 스냅샷 **이후** 참조를 얻은 blob이 같은 패스 안에서 삭제된다.
//!
//! 결과: `put()`이 Ok를 반환했는데 커밋 포인터만 남고 유일한 사본이 사라진다
//! (`get_bytes` 404 + `list` 제외 = 영구 non-servable). **데이터 손실**이다.
//! 아래 판정은 이 정의를 그대로 4중으로 확인한다:
//! 커밋 포인터 존재 ∧ blob 부재 ∧ `get_bytes` 404 ∧ `list` 제외.
//!
//! ## 창을 여는 3요소 (전부 load-bearing — 하나라도 빠지면 초록으로 지나간다)
//!
//! 1. **과거 타임스탬프 tombstone**(`seed_expired_tombstones`). `.gc-pending.json`에
//!    희생 sha를 backdate해 심으면 tombstone이 이미 grace를 넘긴 상태라 **첫** 패스에서
//!    곧바로 삭제 조건이 성립한다(2-pass 대기 불필요). 안 심으면 그 패스는 "최초 관측"으로
//!    pending에 넣고 보존만 하므로 아무것도 안 지운다.
//! 2. **두 번째 put이 같은 내용**(= dedup 경로). 내용이 다르면 `intact == false`라
//!    put이 새 blob을 기록하고, 그 blob은 방금 쓴 것이라 GC 대상이 아니다 → 생존.
//!    같은 내용일 때만 put이 바이트를 재기록하지 않아 GC가 지운 사본이 복구되지 않는다.
//! 3. **put이 스냅샷 이후(창 안)에 착지**. 스냅샷 전에 커밋되면 refs에 잡혀
//!    `pending.remove()`가 걸려 안전하다. 그래서 put은 reconcile을 띄운 뒤
//!    `PUT_DELAY`만큼 늦게 쏜다.
//!
//! ## 왜 `tests/adversarial.rs::concurrent_nested_puts_with_reconcile_loop_preserve_all`은
//!    이 버그를 못 잡나 (중복 테스트가 아니다 — 지우지 말 것)
//!
//! 그 테스트도 put과 reconcile 루프를 동시에 돌리지만 위 3요소가 **전부** 빠져 있다:
//!   * 40개 put이 전부 **새로운 내용**이라 dedup 분기가 아니라 write 분기를 탄다(요소 2 위배).
//!   * `grace = 3600s`이고 tombstone을 심지 않아 GC가 **아무것도** 삭제하지 않는다
//!     (갓 기록된 blob은 grace가 보호). `gc_deleted`가 0인 패스만 도는 셈이다(요소 1 위배).
//! 즉 그 테스트는 "GC가 안 지우는 상황에서 put이 살아남나"를 보고, 이 테스트는
//! "GC가 **지우는** 상황에서 dedup put이 커밋한 참조가 무시되나"를 본다. 서로 다른 성질이다.
//!
//! 이 테스트는 fix 전 baseline에서 **RED**여야 하고(그게 red-capture의 목적),
//! fix 후에는 항상 GREEN이어야 한다.

use files::store::{reconcile, Store};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod common;
use common::hex_sha;

/// 라운드 반복 — 창은 스케줄러 타이밍에 의존하므로 라운드를 늘려 탐지 확률을 올린다.
/// 수정 후에는 항상 초록이라 비용은 실행 시간뿐. 3라운드로도 전체 10초 예산 안에 든다.
const ROUNDS: usize = 3;

/// 희생 객체 수. blob 루프는 `read_dir` 순서로 도는데, 희생이 **맨 앞**에 걸리면 put이
/// 착지하기 전에 지워지고 → put이 blob을 재기록해 자가 치유된다(그 객체는 안 잃는다).
/// 희생을 여러 개 흩뿌리면 그중 하나라도 put 착지 뒤에 방문되어 유실이 잡히므로,
/// 재현이 `read_dir` 순서라는 우연에 걸리지 않는다. 희생은 수십 바이트라 시간 비용 ≈ 0.
const VICTIMS: usize = 4;

/// 미끼 blob 개수·크기. reconcile의 blob 루프는 **항목마다 내용을 전량 read + sha 검증**
/// 하므로(무결성 격리 로직) 큰 미끼를 심으면 루프가 느려지고 창(스냅샷 → 희생 blob 방문)이
/// 넓어진다. 12 × 1 MiB = 12 MiB면 미끼 하나당 루프 체류가 `PUT_DELAY`를 크게 웃돌아
/// 창이 충분히 열리고(측정: 20/20 재현), 3라운드 총 실행이 10초 예산 안에 든다.
/// (dev 프로파일은 opt-level 0 → sha256이 느려 미끼 바이트가 곧 실행 시간이다.)
/// 미끼는 미참조지만 pending에 없어 이번 패스에선 "최초 관측"으로 보존된다(부작용 없음).
const DECOYS: usize = 12;
const DECOY_MIB: usize = 1;

/// 희생 blob은 **작게**(수십 바이트). put은 (a) 내용 sha 계산 (b) 기존 blob read+sha
/// (intact 검사)를 하므로, 희생이 크면 put이 느려 GC가 먼저 지우고 → put이 blob을
/// **재기록**해 자가 치유된다(= 버그가 안 드러난다). put은 창 안에서 빨리 끝나야 한다.
fn victim_content(i: usize) -> Vec<u8> {
    format!("victim-{i}-payload").into_bytes()
}

/// reconcile을 띄운 뒤 put까지의 지연 — 스냅샷(`collect_referenced`)이 끝난 **뒤**,
/// 그러나 blob 루프가 희생 blob에 닿기 **전**에 put이 착지해야 한다(요소 3).
const PUT_DELAY: Duration = Duration::from_millis(15);

/// grace=0 → tombstone이 조금이라도 과거면 즉시 삭제 대상.
const GC_GRACE: Duration = Duration::from_secs(0);

/// tombstone backdate 폭. grace(0)를 압도적으로 넘겨 첫 패스에서 삭제가 확정되게 한다(요소 1).
const TOMBSTONE_BACKDATE_SECS: u64 = 3600;

/// 미끼: 큰 blob. 이름 == sha(content)라 격리되지 않는다. 역할은 오직 blob 루프를
/// 느리게 만들어 창을 넓히는 것.
async fn plant_decoys(root: &std::path::Path) {
    for d in 0..DECOYS {
        let content: Vec<u8> = (0..DECOY_MIB * 1024 * 1024)
            .map(|i| ((i + d * 7) % 251) as u8)
            .collect();
        let name = hex_sha(&content);
        tokio::fs::write(root.join(".objects").join(&name), &content)
            .await
            .unwrap();
    }
}

/// 희생 blob들을 "이미 grace를 넘긴 tombstone"으로 심는다 → GC 삭제 조건 즉시 성립(요소 1).
async fn seed_expired_tombstones(root: &std::path::Path, shas: &[String]) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut pending = serde_json::Map::new();
    for sha in shas {
        pending.insert(
            sha.clone(),
            serde_json::json!(now - TOMBSTONE_BACKDATE_SECS),
        );
    }
    tokio::fs::write(
        root.join(".objects").join(".gc-pending.json"),
        serde_json::to_vec(&pending).unwrap(),
    )
    .await
    .unwrap();
}

/// 한 라운드. 반환 = (증상 정의에 정확히 부합하는 유실 객체 수, 커밋 객체 수, reconcile stats).
/// 유실 = 커밋 포인터 존재 ∧ blob 부재 ∧ get_bytes 404 ∧ list 제외.
async fn run_round() -> (usize, usize, reconcile::ReconcileStats) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let s = Arc::new(Store::new(root.clone()));

    // 1) 희생 객체를 올렸다 지운다 → blob은 디스크에 남고 미참조가 된다.
    let mut shas = Vec::new();
    for i in 0..VICTIMS {
        let m = s
            .put(
                "b",
                &format!("v{i}.bin"),
                "application/octet-stream",
                "u",
                victim_content(i),
            )
            .await
            .unwrap();
        s.delete("b", &format!("v{i}.bin")).await.unwrap();
        shas.push(m.sha256);
    }
    plant_decoys(&root).await; // 요소: 창 넓히기
    seed_expired_tombstones(&root, &shas).await; // 요소 1

    // 2) reconcile 시작. 이 시점엔 희생 meta가 없으므로 collect_referenced() 스냅샷은
    //    희생 sha를 모른다 → 삭제 후보로 확정된다.
    let rec = {
        let root = root.clone();
        tokio::spawn(async move { reconcile::run_once(&root, GC_GRACE).await })
    };

    // 3) 스냅샷 이후(= 창 안)에 **같은 내용**으로 재-put → dedup 분기(요소 2 + 요소 3).
    //    put은 Ok를 반환하고 커밋 포인터를 남기지만 바이트는 재기록하지 않는다.
    tokio::time::sleep(PUT_DELAY).await;
    let mut hs = Vec::new();
    for i in 0..VICTIMS {
        let s = s.clone();
        hs.push(tokio::spawn(async move {
            s.put(
                "b",
                &format!("v{i}.bin"),
                "application/octet-stream",
                "u",
                victim_content(i),
            )
            .await
            .unwrap()
        }));
    }
    let mut committed = Vec::new();
    for h in hs {
        committed.push(h.await.unwrap());
    }
    let stats = rec.await.unwrap().unwrap();

    // 4) 판정: 성공 반환한 put이 커밋한 객체가 서빙 가능한가?
    let listed = s.list("b").await.unwrap();
    let mut lost = 0;
    for (i, m) in committed.iter().enumerate() {
        let key = format!("v{i}.bin");
        let meta_exists = tokio::fs::try_exists(root.join("b").join(format!("{key}.meta.json")))
            .await
            .unwrap();
        let blob_exists = tokio::fs::try_exists(root.join(".objects").join(&m.sha256))
            .await
            .unwrap();
        let servable = s.get_bytes("b", &key).await.is_ok();
        let in_list = listed.iter().any(|(k, _)| *k == key);
        if meta_exists && !blob_exists && !servable && !in_list {
            lost += 1;
        }
    }
    (lost, committed.len(), stats)
}

/// 플립 증인: reconcile 창 안에서 dedup 경로로 커밋된 put은 blob을 잃으면 안 된다.
/// 성공 반환한 put이 커밋한 객체는 **반드시** 서빙 가능해야 한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dedup_put_during_reconcile_window_must_not_lose_blob() {
    let mut lost_total = 0usize;
    let mut committed_total = 0usize;
    let mut per_round = Vec::new();
    let mut last_stats = None;

    for _ in 0..ROUNDS {
        let (lost, committed, stats) = run_round().await;
        lost_total += lost;
        committed_total += committed;
        per_round.push(lost);
        last_stats = Some(stats);
    }

    assert_eq!(
        lost_total, 0,
        "DATA LOSS: put()이 OK를 반환했는데 reconcile GC가 그 블롭을 삭제 — 커밋 포인터는 남고 \
         블롭 부재 → GET 404 / list 제외 (영구 non-servable). \
         유실 {lost_total}/{committed_total} (라운드별={per_round:?}), stats={last_stats:?}"
    );
}
