// characterization: on-disk 레이아웃 이름 규칙 골든 트리 (arch-deepening-2026-07 · B3)
// Store 공개 API + reconcile::run_once만 사용(내부 미접근). 스크립트된 연산 후
// 데이터 루트의 상대 파일 경로 전체를 정확히 단언해 이름 규칙을 한 곳에서 핀한다.
// 골든 값은 현재 코드의 실제 산출을 기록한 것이며(characterization), 증분을
// 통과시키기 위한 재기록은 금지된다.
use files::error::AppError;
use files::meta::{BucketMeta, Visibility};
use files::store::{reconcile, Store};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

fn byte_stream(
    chunks: Vec<Vec<u8>>,
) -> impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Unpin {
    futures::stream::iter(chunks.into_iter().map(|c| Ok(bytes::Bytes::from(c))))
}

/// 루트 아래 모든 정규 파일의 상대 경로(정렬). 디렉터리 자체는 파일 경로에 함의된다.
async fn rel_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut rd = tokio::fs::read_dir(&dir).await.unwrap();
        while let Some(e) = rd.next_entry().await.unwrap() {
            let p = e.path();
            if e.file_type().await.unwrap().is_dir() {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(root).unwrap().to_string_lossy().to_string());
            }
        }
    }
    out.sort();
    out
}

#[tokio::test]
async fn on_disk_layout_golden_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let s = Store::new(root.clone());

    // 스크립트: 버킷 생성 → 버퍼드 put → 스트리밍 put(중첩 키) → 삭제 → reconcile
    s.put_bucket(
        "b",
        &BucketMeta {
            visibility: Visibility::Public,
            owner: "test".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
        },
    )
    .await
    .unwrap();
    let alpha = b"alpha-bytes".to_vec();
    let beta = b"beta-bytes".to_vec();
    s.put("b", "k", "text/plain", "test", alpha.clone()).await.unwrap();
    s.put_stream(
        "b",
        "d/n",
        "application/octet-stream",
        "test",
        byte_stream(vec![beta.clone()]),
        1 << 20,
    )
    .await
    .unwrap();
    s.delete("b", "k").await.unwrap();

    // 삭제된 k의 블롭은 미참조 → 첫 관측으로 tombstone 유예 등재(삭제 아님)
    let stats = reconcile::run_once(&root, Duration::from_secs(3600)).await.unwrap();
    assert_eq!(
        stats,
        reconcile::ReconcileStats {
            referenced: 1,
            gc_deleted: 0,
            gc_pending: 1,
            temps_deleted: 0,
            quarantined: 0,
        }
    );

    let sha_alpha = hex::encode(Sha256::digest(&alpha));
    let sha_beta = hex::encode(Sha256::digest(&beta));
    let mut expected = vec![
        ".objects/.gc-pending.json".to_string(),
        format!(".objects/{sha_alpha}"),
        format!(".objects/{sha_beta}"),
        "b/.bucket.json".to_string(),
        "b/d/n.meta.json".to_string(),
    ];
    expected.sort();
    assert_eq!(rel_files(&root).await, expected);
}

/// characterization (plan-gate P-2, r2 P-4 반영): 심링크 커밋 포인터의 현행 행동.
/// 순회는 lstat 의미론의 비-디렉터리 분기(file_type().is_dir())라 심링크가 파일처럼
/// 통과하고, 내용 read는 링크를 추종한다. dangling 링크는 조용히 제외된다.
/// 핵심(비-동어반복): `only`의 블롭은 **심링크로만** 참조된다 — reconcile이 심링크를
/// 무시하는 회귀가 생기면 gc_pending:1로 즉시 적발된다.
#[cfg(unix)]
#[tokio::test]
async fn symlinked_commit_pointer_current_behavior() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let s = Store::new(root.clone());
    let real_meta = s
        .put("b", "real", "text/plain", "test", b"payload".to_vec())
        .await
        .unwrap();

    // 심링크로만 참조되는 블롭: put으로 생성한 진짜 포인터를 루트 직속(워커 비대상
    // 위치)으로 옮기고, 그 자리를 심링크로 대체한다
    let only_meta = s
        .put("b", "only", "text/plain", "test", b"only-payload".to_vec())
        .await
        .unwrap();
    let target = root.join("link-target.json");
    tokio::fs::rename(root.join("b").join("only.meta.json"), &target)
        .await
        .unwrap();
    std::os::unix::fs::symlink(&target, root.join("b").join("only.meta.json")).unwrap();
    // dangling 심링크 — read 실패 → 조용히 제외
    std::os::unix::fs::symlink(root.join("nope"), root.join("b").join("gone.meta.json")).unwrap();

    let listed = s.list("b").await.unwrap();
    assert_eq!(
        listed,
        vec![
            ("only".to_string(), only_meta.clone()),
            ("real".to_string(), real_meta),
        ]
    );

    // collect_referenced도 심링크를 추종: referenced에 sha_only가 포함돼야(=2) 하며,
    // 심링크 무시 회귀 시 sha_only 미참조 → gc_pending:1로 이 단언이 깨진다
    let stats = reconcile::run_once(&root, Duration::from_secs(3600)).await.unwrap();
    assert_eq!(
        stats,
        reconcile::ReconcileStats {
            referenced: 2,
            gc_deleted: 0,
            gc_pending: 0,
            temps_deleted: 0,
            quarantined: 0,
        }
    );
    assert!(
        tokio::fs::try_exists(s.blob_path(&only_meta.sha256)).await.unwrap(),
        "심링크로만 참조되는 블롭 생존"
    );
}

async fn tmp_entries(objects: &Path) -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(objects).await {
        while let Some(e) = rd.next_entry().await.unwrap() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with(".tmp-") {
                v.push(n);
            }
        }
    }
    v
}

/// characterization (plan-gate P-3): 업로드 진행 중 라이터가 실제로 생성하는 임시
/// 파일 이름을 관측한다 — `.objects/.tmp-*` 정확히 1개, grace 내 reconcile 보존,
/// 스트림 에러 종료 시 정리까지. 접두사가 바뀌면 이 테스트가 잡는다.
#[tokio::test]
async fn put_stream_midflight_temp_observed_and_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let s = Store::new(root.clone());
    let objects = root.join(".objects");

    let (tx, rx) = futures::channel::mpsc::unbounded::<Result<bytes::Bytes, std::io::Error>>();
    let s2 = s.clone();
    let task = tokio::spawn(async move {
        s2.put_stream("b", "big", "application/octet-stream", "test", rx, 1 << 20)
            .await
    });
    tx.unbounded_send(Ok(bytes::Bytes::from_static(b"chunk-1"))).unwrap();

    // 업로드가 열어둔 temp가 나타날 때까지 폴링(로컬 fs — 수 ms 내)
    let mut tmps = Vec::new();
    for _ in 0..500 {
        tmps = tmp_entries(&objects).await;
        if !tmps.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(tmps.len(), 1, "진행 중 업로드의 temp는 정확히 1개: {tmps:?}");
    assert!(tmps[0].starts_with(".tmp-"));

    // grace 내 reconcile은 활성 temp를 보존한다
    let stats = reconcile::run_once(&root, Duration::from_secs(3600)).await.unwrap();
    assert_eq!(
        stats,
        reconcile::ReconcileStats {
            referenced: 0,
            gc_deleted: 0,
            gc_pending: 0,
            temps_deleted: 0,
            quarantined: 0,
        }
    );
    assert!(
        tokio::fs::try_exists(objects.join(&tmps[0])).await.unwrap(),
        "grace 내 활성 temp 보존"
    );

    // 스트림 에러 → stream_error + temp 정리
    tx.unbounded_send(Err(std::io::Error::other("boom"))).unwrap();
    drop(tx);
    let res = task.await.unwrap();
    assert!(matches!(res, Err(AppError::BadRequest("stream_error"))));
    assert!(tmp_entries(&objects).await.is_empty(), "에러 종료 후 temp 잔재 없음");
}
