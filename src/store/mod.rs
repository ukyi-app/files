pub mod atomic;
pub mod locks;
pub mod reconcile;

mod buckets;
mod listing;
mod objects;
#[cfg(test)]
mod tests;

use crate::error::AppError;
use crate::layout::Layout;
use std::path::PathBuf;

/// content-addressed 저장소. 바이트는 `.objects/<sha256>`에 불변 저장하고,
/// 키의 `<key>.meta.json`이 sha를 가리키는 단일 atomic 커밋 포인터다.
/// 온디스크 이름·경로 규칙은 보유하지 않는다 — 전부 `layout`에 위임(R-2).
#[derive(Clone)]
pub struct Store {
    layout: Layout,
    locks: locks::KeyLocks,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self {
            layout: Layout::new(root),
            locks: locks::KeyLocks::new(),
        }
    }

    pub fn blob_path(&self, sha: &str) -> PathBuf {
        self.layout.blob_path(sha)
    }

    fn meta_for(&self, bucket: &str, key: &str) -> Result<PathBuf, AppError> {
        self.layout.meta_for(bucket, key)
    }
}
