use super::Store;
use crate::error::AppError;
use crate::meta::ObjectMeta;
use crate::layout::valid_bucket;

impl Store {
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
}
