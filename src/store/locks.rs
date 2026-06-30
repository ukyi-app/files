use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// 같은 `bucket/key` PUT/DELETE를 직렬화(서로 다른 키는 병렬).
/// 단일 replica(replicas:1 + RWO PVC)라 in-process 락으로 충분.
#[derive(Clone, Default)]
pub struct KeyLocks {
    map: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
}

impl KeyLocks {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn lock(&self, key: &str) -> OwnedMutexGuard<()> {
        let m = {
            self.map
                .lock()
                .unwrap()
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        m.lock_owned().await
    }

    #[cfg(test)]
    pub fn try_busy(&self, key: &str) -> bool {
        self.map
            .lock()
            .unwrap()
            .get(key)
            .map(|m| m.try_lock().is_err())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn busy_while_held_free_after_drop() {
        let locks = KeyLocks::new();
        let g = locks.lock("bucket/key").await;
        assert!(locks.try_busy("bucket/key"));
        drop(g);
        assert!(!locks.try_busy("bucket/key"));
    }

    #[tokio::test]
    async fn different_keys_independent() {
        let locks = KeyLocks::new();
        let _g1 = locks.lock("a").await;
        assert!(!locks.try_busy("b")); // 미사용 키는 미점유
        let _g2 = locks.lock("b").await; // 다른 키는 블록 안 됨
        assert!(locks.try_busy("a"));
        assert!(locks.try_busy("b"));
    }

    #[tokio::test]
    async fn lock_serializes_same_key() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        let locks = KeyLocks::new();
        let counter = Arc::new(AtomicU32::new(0));
        let max_seen = Arc::new(AtomicU32::new(0));
        let mut handles = vec![];
        for _ in 0..8 {
            let locks = locks.clone();
            let counter = counter.clone();
            let max_seen = max_seen.clone();
            handles.push(tokio::spawn(async move {
                let _g = locks.lock("same").await;
                let cur = counter.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                counter.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(max_seen.load(Ordering::SeqCst), 1, "락은 같은 키를 직렬화해야 함");
    }
}
