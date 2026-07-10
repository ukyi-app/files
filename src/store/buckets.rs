use super::Store;
use super::atomic;
use crate::error::AppError;
use crate::meta::BucketMeta;
use crate::layout::valid_bucket;

impl Store {
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

    /// `.bucket.json`을 보유한 최상위 디렉터리를 버킷으로 열거(정렬).
    pub async fn list_buckets(&self) -> Result<Vec<(String, BucketMeta)>, AppError> {
        let mut rd = match tokio::fs::read_dir(&self.root).await {
            Ok(rd) => rd,
            Err(_) => return Ok(vec![]),
        };
        let mut out: Vec<(String, BucketMeta)> = Vec::new();
        while let Some(e) = rd.next_entry().await.map_err(AppError::Internal)? {
            if !e.file_type().await.map_err(AppError::Internal)?.is_dir() {
                continue;
            }
            let name = e.file_name();
            let name = name.to_string_lossy().to_string();
            if name == ".objects" {
                continue;
            }
            if let Ok(bm) = self.get_bucket(&name).await {
                out.push((name, bm));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}
