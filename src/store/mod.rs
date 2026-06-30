pub mod atomic;
pub mod locks;

use crate::error::AppError;
use crate::meta::ObjectMeta;
use crate::path::{meta_path, safe_object_path};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

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
