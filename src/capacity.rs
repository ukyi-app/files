use crate::error::AppError;
use nix::sys::statvfs::statvfs;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub fn free_bytes(path: &Path) -> std::io::Result<u64> {
    let s = statvfs(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    Ok(s.blocks_available() as u64 * s.fragment_size() as u64)
}

type FreeFn = Arc<dyn Fn() -> std::io::Result<u64> + Send + Sync>;

/// 프로세스 전역 in-flight 예약으로 동시 업로더의 free-space overcommit 방지.
/// free 공간 소스는 주입형(프로덕션은 statvfs, 테스트는 결정적 값).
#[derive(Clone)]
pub struct Capacity {
    free_fn: FreeFn,
    min_free: u64,
    inflight: Arc<Mutex<u64>>,
}

impl Capacity {
    pub fn new(root: std::path::PathBuf, min_free: u64) -> Self {
        Self::with_free_fn(min_free, move || free_bytes(&root))
    }

    pub fn with_free_fn(
        min_free: u64,
        free_fn: impl Fn() -> std::io::Result<u64> + Send + Sync + 'static,
    ) -> Self {
        Self {
            free_fn: Arc::new(free_fn),
            min_free,
            inflight: Arc::new(Mutex::new(0)),
        }
    }

    /// reserve_bytes(Content-Length 또는 설정 max)를 예약. 체크+증가를 Mutex로 원자화(TOCTOU 제거).
    pub fn reserve(&self, reserve_bytes: u64) -> Result<Reservation, AppError> {
        let free = (self.free_fn)().map_err(AppError::Internal)?;
        let mut g = self.inflight.lock().unwrap();
        if free.saturating_sub(*g).saturating_sub(reserve_bytes) < self.min_free {
            return Err(AppError::InsufficientStorage); // 507
        }
        *g += reserve_bytes;
        Ok(Reservation {
            inflight: self.inflight.clone(),
            bytes: reserve_bytes,
        })
    }

    #[cfg(test)]
    pub fn inflight_for_test(&self) -> u64 {
        *self.inflight.lock().unwrap()
    }
}

/// RAII 예약. 완료/실패 시 드롭으로 in-flight 해제.
pub struct Reservation {
    inflight: Arc<Mutex<u64>>,
    bytes: u64,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        *self.inflight.lock().unwrap() -= self.bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_bytes_reports_positive_for_real_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_bytes(dir.path()).unwrap() > 0);
    }

    #[test]
    fn reservation_accounting_and_raii_release() {
        let cap = Capacity::with_free_fn(0, || Ok(u64::MAX)); // 거부 없음
        {
            let _r1 = cap.reserve(1000).unwrap();
            assert_eq!(cap.inflight_for_test(), 1000);
            let _r2 = cap.reserve(500).unwrap();
            assert_eq!(cap.inflight_for_test(), 1500);
        }
        assert_eq!(cap.inflight_for_test(), 0); // 드롭 시 해제
    }

    #[test]
    fn rejects_when_would_breach_min_free() {
        let cap = Capacity::with_free_fn(1_000_000, || Ok(500)); // min_free >> free
        assert!(matches!(cap.reserve(0), Err(AppError::InsufficientStorage)));
    }

    #[test]
    fn overcommit_prevented_then_freed() {
        // free=100000, min_free=90000 → 예약 budget=10000 (결정적)
        let cap = Capacity::with_free_fn(90_000, || Ok(100_000));
        let r1 = cap.reserve(4000).unwrap();
        let r2 = cap.reserve(4000).unwrap();
        assert!(cap.reserve(4000).is_err()); // 12000 > 10000 → overcommit 거부
        drop(r1);
        assert!(cap.reserve(4000).is_ok()); // 해제 후 재예약 성공
        drop(r2);
    }
}
