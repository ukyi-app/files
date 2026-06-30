pub mod atomic;
pub mod locks;

use crate::error::AppError;
use crate::meta::{BucketMeta, ObjectMeta};
use crate::path::{meta_path, safe_object_path, valid_bucket};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// content-addressed 저장소. 바이트는 `.objects/<sha256>`에 불변 저장하고,
/// 키의 `<key>.meta.json`이 sha를 가리키는 단일 atomic 커밋 포인터다.
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    locks: locks::KeyLocks,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            locks: locks::KeyLocks::new(),
        }
    }

    pub fn blob_path(&self, sha: &str) -> PathBuf {
        self.root.join(".objects").join(sha)
    }

    fn meta_for(&self, bucket: &str, key: &str) -> Result<PathBuf, AppError> {
        Ok(meta_path(&safe_object_path(&self.root, bucket, key)?))
    }

    pub async fn put(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        by: &str,
        bytes: Vec<u8>,
    ) -> Result<ObjectMeta, AppError> {
        let meta_target = self.meta_for(bucket, key)?; // 검증 포함
        let sha = hex::encode(Sha256::digest(&bytes));
        let _g = self.locks.lock(&format!("{bucket}/{key}")).await; // 같은 키 쓰기 직렬화
        // 1) 불변 blob. 있으면 무결성 검증 후 재사용; 손상(sha 불일치)이면 덮어써 치유.
        //    (발견 P3-2: 무검증 dedup은 손상 blob을 재사용·서빙)
        let blob = self.blob_path(&sha);
        let intact = matches!(
            tokio::fs::read(&blob).await,
            Ok(b) if hex::encode(Sha256::digest(&b)) == sha
        );
        if !intact {
            atomic::write_atomic(&blob, &bytes)
                .await
                .map_err(AppError::Internal)?;
        }
        // 2) 메타 = 단일 atomic 커밋 포인터
        let meta = ObjectMeta {
            content_type: content_type.into(),
            size: bytes.len() as u64,
            sha256: sha,
            created_at: crate::clock::now_rfc3339(),
            uploaded_by: by.into(),
        };
        atomic::write_atomic(&meta_target, &serde_json::to_vec(&meta).unwrap())
            .await
            .map_err(AppError::Internal)?;
        Ok(meta)
    }

    /// 스트리밍 put — `.objects/.tmp-*`에 청크 기록하며 증분 sha·size 계산.
    /// 누적 size > max면 중단·temp 삭제·TooLarge. 완료 후 blob이 이미 있으면
    /// 무결성 검증 후 일치 시 temp 삭제, 불일치(손상) 시 temp를 blob으로 rename해 치유(발견 P3-2).
    pub async fn put_stream<S, E>(
        &self,
        bucket: &str,
        key: &str,
        content_type: &str,
        by: &str,
        stream: S,
        max: u64,
    ) -> Result<ObjectMeta, AppError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        let meta_target = self.meta_for(bucket, key)?; // 검증
        let _g = self.locks.lock(&format!("{bucket}/{key}")).await;

        let objects_dir = self.root.join(".objects");
        atomic::mkdir_p_durable(&objects_dir)
            .await
            .map_err(AppError::Internal)?;
        let tmp = objects_dir.join(format!(".tmp-{}", atomic::unique_suffix()));

        let (size, sha) = match stream_to_temp(&tmp, stream, max).await {
            Ok(v) => v,
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await; // temp 정리
                return Err(e);
            }
        };

        let blob = self.blob_path(&sha);
        let existing_intact = matches!(
            tokio::fs::read(&blob).await,
            Ok(b) if hex::encode(Sha256::digest(&b)) == sha
        );
        if existing_intact {
            let _ = tokio::fs::remove_file(&tmp).await;
        } else {
            // 없거나 손상 → temp를 blob으로 원자적 교체 + parent fsync로 치유/생성
            tokio::fs::rename(&tmp, &blob)
                .await
                .map_err(AppError::Internal)?;
            atomic::fsync_dir(&objects_dir)
                .await
                .map_err(AppError::Internal)?;
        }

        let meta = ObjectMeta {
            content_type: content_type.into(),
            size,
            sha256: sha,
            created_at: crate::clock::now_rfc3339(),
            uploaded_by: by.into(),
        };
        atomic::write_atomic(&meta_target, &serde_json::to_vec(&meta).unwrap())
            .await
            .map_err(AppError::Internal)?;
        Ok(meta)
    }

    pub async fn head(&self, bucket: &str, key: &str) -> Result<ObjectMeta, AppError> {
        let raw = tokio::fs::read(self.meta_for(bucket, key)?)
            .await
            .map_err(|_| AppError::NotFound)?;
        let meta: ObjectMeta = serde_json::from_slice(&raw).map_err(|_| AppError::NotFound)?;
        if !tokio::fs::try_exists(self.blob_path(&meta.sha256))
            .await
            .map_err(AppError::Internal)?
        {
            // 포인터가 가리키는 blob 부재 → 비서빙(reconciliation이 정리)
            return Err(AppError::NotFound);
        }
        Ok(meta) // 락 불필요: 메타는 단일 atomic 파일, blob 불변
    }

    pub async fn get_bytes(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<(ObjectMeta, Vec<u8>), AppError> {
        let meta = self.head(bucket, key).await?;
        let bytes = tokio::fs::read(self.blob_path(&meta.sha256))
            .await
            .map_err(AppError::Internal)?;
        Ok((meta, bytes))
    }

    /// Range 스트리밍용 blob 핸들(메타와 함께).
    pub async fn open(
        &self,
        bucket: &str,
        key: &str,
    ) -> Result<(ObjectMeta, tokio::fs::File), AppError> {
        let meta = self.head(bucket, key).await?;
        let f = tokio::fs::File::open(self.blob_path(&meta.sha256))
            .await
            .map_err(|_| AppError::NotFound)?;
        Ok((meta, f))
    }

    pub async fn put_bucket(&self, bucket: &str, meta: &BucketMeta) -> Result<(), AppError> {
        valid_bucket(bucket)?;
        let path = self.root.join(bucket).join(".bucket.json");
        atomic::write_atomic(&path, &serde_json::to_vec(meta).unwrap())
            .await
            .map_err(AppError::Internal)?;
        Ok(())
    }

    pub async fn get_bucket(&self, bucket: &str) -> Result<BucketMeta, AppError> {
        valid_bucket(bucket)?;
        let path = self.root.join(bucket).join(".bucket.json");
        let raw = tokio::fs::read(&path).await.map_err(|_| AppError::NotFound)?;
        serde_json::from_slice(&raw).map_err(|_| AppError::NotFound)
    }

    /// 버킷 서브트리를 재귀 순회하며 `*.meta.json`을 수집(중첩 키 포함).
    /// `.bucket.json`/temp 제외, 포인터-깨진(blob 부재) 객체 제외. 키 정렬 반환.
    /// (발견 P2-1: 비재귀 스캔은 중첩 키 객체를 누락하므로 반드시 재귀)
    pub async fn list(&self, bucket: &str) -> Result<Vec<(String, ObjectMeta)>, AppError> {
        valid_bucket(bucket)?;
        let bucket_dir = self.root.join(bucket);
        if !tokio::fs::try_exists(&bucket_dir)
            .await
            .map_err(AppError::Internal)?
        {
            return Ok(vec![]);
        }
        let mut out: Vec<(String, ObjectMeta)> = Vec::new();
        let mut stack = vec![bucket_dir.clone()];
        while let Some(dir) = stack.pop() {
            let mut rd = tokio::fs::read_dir(&dir).await.map_err(AppError::Internal)?;
            while let Some(entry) = rd.next_entry().await.map_err(AppError::Internal)? {
                let path = entry.path();
                let ft = entry.file_type().await.map_err(AppError::Internal)?;
                if ft.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(".tmp-") || name == ".bucket.json" || !name.ends_with(".meta.json")
                {
                    continue;
                }
                let rel = path.strip_prefix(&bucket_dir).unwrap();
                let key = rel
                    .to_string_lossy()
                    .strip_suffix(".meta.json")
                    .unwrap()
                    .to_string();
                let raw = match tokio::fs::read(&path).await {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let meta: ObjectMeta = match serde_json::from_slice(&raw) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if !tokio::fs::try_exists(self.blob_path(&meta.sha256))
                    .await
                    .map_err(AppError::Internal)?
                {
                    continue; // 포인터-깨진 객체 비서빙
                }
                out.push((key, meta));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    pub async fn delete(&self, bucket: &str, key: &str) -> Result<(), AppError> {
        let mp = self.meta_for(bucket, key)?;
        let _g = self.locks.lock(&format!("{bucket}/{key}")).await;
        // 커밋 포인터만 제거 → 즉시 사라짐. blob은 reconciliation이 미참조 GC.
        match tokio::fs::remove_file(&mp).await {
            Ok(()) => {
                if let Some(p) = mp.parent() {
                    atomic::fsync_dir(p).await.map_err(AppError::Internal)?;
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::Internal(e)),
        }
    }
}

/// 스트림을 temp 파일에 기록하며 증분 sha·size 계산. 누적 size>max면 TooLarge.
/// 성공 시 `(size, hex_sha)` 반환. temp 정리는 호출자 책임.
async fn stream_to_temp<S, E>(tmp: &Path, mut stream: S, max: u64) -> Result<(u64, String), AppError>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    let mut f = tokio::fs::File::create(tmp)
        .await
        .map_err(AppError::Internal)?;
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            tracing::warn!(error = %e, "upload stream error");
            AppError::BadRequest("stream_error")
        })?;
        size += chunk.len() as u64;
        if size > max {
            return Err(AppError::TooLarge);
        }
        hasher.update(&chunk);
        f.write_all(&chunk).await.map_err(AppError::Internal)?;
    }
    f.sync_all().await.map_err(AppError::Internal)?;
    Ok((size, hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod tests {
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
        let objects = s.root.join(".objects");
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
}
