pub mod atomic;
pub mod locks;
pub mod reconcile;

mod buckets;
mod listing;
mod objects;
#[cfg(test)]
mod tests;

use crate::error::AppError;
use crate::layout::{meta_path, safe_object_path};
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
}
