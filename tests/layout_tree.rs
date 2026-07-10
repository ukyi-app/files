// characterization: on-disk 레이아웃 이름 규칙 골든 트리 (arch-deepening-2026-07 · B3)
// Store 공개 API + reconcile::run_once만 사용(내부 미접근). 스크립트된 연산 후
// 데이터 루트의 상대 파일 경로 전체를 정확히 단언해 이름 규칙을 한 곳에서 핀한다.
// 골든 값은 현재 코드의 실제 산출을 기록한 것이며(characterization), 증분을
// 통과시키기 위한 재기록은 금지된다.
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
