use super::Store;
use super::atomic;
use crate::error::AppError;
use crate::meta::ObjectMeta;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::AsyncWriteExt;

impl Store {
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
