use super::Store;
use crate::error::AppError;
use crate::meta::ObjectMeta;

impl Store {
    /// 버킷의 커밋 포인터를 워크하며 메타를 수집(중첩 키 포함 — 워커가 재귀).
    /// 이름 규칙(temp 제외·`.bucket.json` 자연 배제)은 워커 소유(R-3).
    /// 여기 남는 정책: 읽기/파싱 실패 조용한 skip, 포인터-깨진(blob 부재) 객체
    /// 비서빙, 키 정렬(워커는 순서를 보장하지 않는다).
    pub async fn list(&self, bucket: &str) -> Result<Vec<(String, ObjectMeta)>, AppError> {
        // valid_bucket은 pointers_in_bucket이 I/O 전에 수행 — 같은 BadRequest.
        let mut walk = self.layout.pointers_in_bucket(bucket)?;
        let mut out: Vec<(String, ObjectMeta)> = Vec::new();
        while let Some(entry) = walk.next().await.map_err(AppError::Internal)? {
            let raw = match tokio::fs::read(&entry.meta_path).await {
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
            out.push((entry.key, meta));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}
