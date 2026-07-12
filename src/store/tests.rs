use super::*;
use sha2::{Digest, Sha256};

fn hex_sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}
fn store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    (Store::new(dir.path().to_path_buf()), dir)
}

#[tokio::test]
async fn put_get_roundtrip_content_addressed() {
    let (s, _d) = store();
    let body = b"agent skill zip".to_vec();
    let m = s
        .put("skills", "a/b.zip", "application/zip", "page", body.clone())
        .await
        .unwrap();
    // blob = .objects/<sha>
    assert!(tokio::fs::try_exists(s.blob_path(&m.sha256)).await.unwrap());
    let (gm, got) = s.get_bytes("skills", "a/b.zip").await.unwrap();
    assert_eq!(got, body);
    assert_eq!(gm.sha256, m.sha256);
    assert_eq!(gm.sha256, hex_sha(&body));
    assert_eq!(gm.content_type, "application/zip");
    assert_eq!(gm.size, body.len() as u64);
}

#[tokio::test]
async fn same_size_overwrite_is_self_consistent() {
    let (s, _d) = store();
    s.put("b", "k", "text/plain", "x", b"AAAA".to_vec())
        .await
        .unwrap(); // size 4
    let m2 = s
        .put("b", "k", "text/plain", "x", b"BBBB".to_vec())
        .await
        .unwrap(); // size 4, 다른 내용
    let (gm, got) = s.get_bytes("b", "k").await.unwrap();
    assert_eq!(got, b"BBBB"); // 절대 구 데이터 노출 안 함
    assert_eq!(gm.sha256, m2.sha256);
    assert_eq!(hex_sha(&got), gm.sha256); // 메타-데이터 항상 정합
}

#[tokio::test]
async fn meta_pointing_to_missing_blob_is_not_found() {
    let (s, _d) = store();
    let bogus = crate::meta::ObjectMeta {
        content_type: "text/plain".into(),
        size: 3,
        sha256: "deadbeef".repeat(8), // 존재하지 않는 blob (64 hex)
        created_at: crate::clock::now_rfc3339(),
        uploaded_by: "x".into(),
    };
    let mp = s.meta_for("b", "k").unwrap();
    atomic::write_atomic(&mp, &serde_json::to_vec(&bogus).unwrap())
        .await
        .unwrap();
    assert!(matches!(s.head("b", "k").await, Err(AppError::NotFound)));
}

fn byte_stream(
    data: &[u8],
    chunk: usize,
) -> impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> {
    let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = data
        .chunks(chunk.max(1))
        .map(|c| Ok(bytes::Bytes::copy_from_slice(c)))
        .collect();
    futures::stream::iter(chunks)
}

async fn no_temp_residue(s: &Store) {
    // 온디스크 바이트를 핀하려 raw 리터럴 유지(layout 상수 경유 시 동어반복).
    let objects = s.layout.root().join(".objects");
    if !tokio::fs::try_exists(&objects).await.unwrap() {
        return;
    }
    let mut rd = tokio::fs::read_dir(&objects).await.unwrap();
    while let Some(e) = rd.next_entry().await.unwrap() {
        let n = e.file_name();
        let n = n.to_string_lossy();
        assert!(!n.starts_with(".tmp-"), "temp residue: {n}");
    }
}

#[tokio::test]
async fn put_stream_roundtrip_large() {
    let (s, _d) = store();
    let data = vec![7u8; 100_000];
    let m = s
        .put_stream("b", "k", "application/octet-stream", "x", byte_stream(&data, 7000), 1 << 30)
        .await
        .unwrap();
    assert_eq!(m.size, 100_000);
    assert_eq!(m.sha256, hex_sha(&data));
    let (_gm, got) = s.get_bytes("b", "k").await.unwrap();
    assert_eq!(got, data);
    no_temp_residue(&s).await;
}

#[tokio::test]
async fn put_stream_too_large_no_residue_not_committed() {
    let (s, _d) = store();
    let data = vec![0u8; 10_000];
    let err = s
        .put_stream("b", "k", "x", "x", byte_stream(&data, 1000), 5000)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::TooLarge));
    no_temp_residue(&s).await;
    assert!(matches!(s.head("b", "k").await, Err(AppError::NotFound)));
}

#[tokio::test]
async fn put_stream_heals_corrupt_blob() {
    let (s, _d) = store();
    let data = b"heal me please".to_vec();
    let sha = hex_sha(&data);
    // 손상 blob 미리 생성: 올바른 파일명, 잘못된 내용
    atomic::write_atomic(&s.blob_path(&sha), b"CORRUPT")
        .await
        .unwrap();
    let m = s
        .put_stream("b", "k", "x", "x", byte_stream(&data, 4), 1 << 30)
        .await
        .unwrap();
    assert_eq!(m.sha256, sha);
    assert_eq!(tokio::fs::read(s.blob_path(&sha)).await.unwrap(), data); // 치유됨
    let (_gm, got) = s.get_bytes("b", "k").await.unwrap();
    assert_eq!(got, data);
    no_temp_residue(&s).await;
}

#[tokio::test]
async fn list_returns_serving_only_with_nested_keys() {
    let (s, _d) = store();
    s.put("b", "top.txt", "text/plain", "x", b"a".to_vec())
        .await
        .unwrap();
    s.put("b", "dir/sub/nested.zip", "application/zip", "x", b"bb".to_vec())
        .await
        .unwrap();
    // 포인터-깨진 객체: 메타만, blob 없음
    let bogus = crate::meta::ObjectMeta {
        content_type: "x".into(),
        size: 1,
        sha256: "00".repeat(32),
        created_at: crate::clock::now_rfc3339(),
        uploaded_by: "x".into(),
    };
    atomic::write_atomic(
        &s.meta_for("b", "broken").unwrap(),
        &serde_json::to_vec(&bogus).unwrap(),
    )
    .await
    .unwrap();

    let listed = s.list("b").await.unwrap();
    let keys: Vec<&str> = listed.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, vec!["dir/sub/nested.zip", "top.txt"]); // 정렬·broken 제외
}

#[tokio::test]
async fn list_buckets_returns_those_with_bucket_json() {
    let (s, _d) = store();
    s.put_bucket(
        "pub1",
        &crate::meta::BucketMeta {
            visibility: crate::meta::Visibility::Public,
            owner: "o".into(),
            created_at: crate::clock::now_rfc3339(),
        },
    )
    .await
    .unwrap();
    s.put_bucket(
        "int1",
        &crate::meta::BucketMeta {
            visibility: crate::meta::Visibility::Internal,
            owner: "o".into(),
            created_at: crate::clock::now_rfc3339(),
        },
    )
    .await
    .unwrap();
    // .bucket.json 없는 디렉터리(객체만)는 목록 제외
    s.put("orphan", "k", "x", "u", b"x".to_vec()).await.unwrap();

    let buckets = s.list_buckets().await.unwrap();
    let names: Vec<&str> = buckets.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["int1", "pub1"]); // 정렬·.bucket.json 보유만
}

#[tokio::test]
async fn list_empty_bucket_is_ok() {
    let (s, _d) = store();
    assert!(s.list("nope").await.unwrap().is_empty());
}

#[tokio::test]
async fn bucket_meta_roundtrip() {
    let (s, _d) = store();
    let bm = crate::meta::BucketMeta {
        visibility: crate::meta::Visibility::Public,
        owner: "page".into(),
        created_at: crate::clock::now_rfc3339(),
    };
    s.put_bucket("b", &bm).await.unwrap();
    let got = s.get_bucket("b").await.unwrap();
    assert_eq!(got.owner, "page");
    assert_eq!(got.visibility, crate::meta::Visibility::Public);
    assert!(matches!(s.get_bucket("missing").await, Err(AppError::NotFound)));
}

#[tokio::test]
async fn delete_removes_pointer_idempotent() {
    let (s, _d) = store();
    s.put("b", "k", "text/plain", "x", b"data".to_vec())
        .await
        .unwrap();
    s.delete("b", "k").await.unwrap();
    assert!(matches!(s.head("b", "k").await, Err(AppError::NotFound)));
    s.delete("b", "k").await.unwrap(); // 멱등
}
