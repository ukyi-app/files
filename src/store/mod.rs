pub mod atomic;
pub mod locks;
pub mod reconcile;

mod buckets;
mod listing;
mod objects;
mod pins;
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
    /// blob 핀 등록부(in-process). `clone()`은 내부 `Arc`를 공유한다.
    pins: pins::BlobPins,
}

impl Store {
    /// ⚠ **데이터 루트 하나당 `Store`는 정확히 하나다**(D-3).
    /// 핀 등록부는 in-process이고 `clone()`이 `Arc`를 공유한다. 같은 root로 `Store::new`를
    /// 두 번 부르면 등록부가 갈라져 reconcile이 다른 `Store`의 put을 보지 못한다
    /// → `reconcile-gc-dedup-race` 부활. 공유가 필요하면 **`Store::clone()`**을 써라.
    pub fn new(root: PathBuf) -> Self {
        Self {
            layout: Layout::new(root),
            locks: locks::KeyLocks::new(),
            pins: pins::BlobPins::new(),
        }
    }

    /// 결정적 배리어를 주입한 `Store`(테스트 전용). 훅은 프로덕션과 **같은 경로**를 지난다.
    #[cfg(test)]
    pub(crate) fn with_hooks(root: PathBuf, hooks: pins::Hooks) -> Self {
        Self {
            layout: Layout::new(root),
            locks: locks::KeyLocks::new(),
            pins: pins::BlobPins::with_hooks(hooks),
        }
    }

    /// 배리어 **+ 키 락 경고 임계값**을 주입한 `Store`(테스트 전용).
    /// `LOCK_WARN_AFTER`(prod 30s)를 그대로 두면 T-S2가 30초를 기다려야 한다 → 관측 가능하게 줄인다.
    /// **프로덕션 경로에는 영향이 없다** — `Store::new`는 여전히 `KeyLocks::new()`를 쓴다.
    #[cfg(test)]
    pub(crate) fn with_hooks_and_lock_warn(
        root: PathBuf,
        hooks: pins::Hooks,
        warn_after: std::time::Duration,
    ) -> Self {
        Self {
            layout: Layout::new(root),
            locks: locks::KeyLocks::with_warn_after(warn_after),
            pins: pins::BlobPins::with_hooks(hooks),
        }
    }

    pub fn blob_path(&self, sha: &str) -> PathBuf {
        self.layout.blob_path(sha)
    }

    pub(crate) fn layout(&self) -> &Layout {
        &self.layout
    }

    pub(crate) fn pins(&self) -> &pins::BlobPins {
        &self.pins
    }

    fn meta_for(&self, bucket: &str, key: &str) -> Result<PathBuf, AppError> {
        self.layout.meta_for(bucket, key)
    }
}
