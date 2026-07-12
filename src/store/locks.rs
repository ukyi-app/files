use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// 같은 `bucket/key` PUT/DELETE를 직렬화(서로 다른 키는 병렬).
/// 단일 replica(replicas:1 + RWO PVC)라 in-process 락으로 충분.
#[derive(Clone, Default)]
pub struct KeyLocks {
    map: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
}

/// 락 맵 키의 유일 저작점 — `bucket/key` 합성은 이 모듈 밖으로 새지 않는다.
fn lock_key(bucket: &str, key: &str) -> String {
    format!("{bucket}/{key}")
}

impl KeyLocks {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn lock(&self, bucket: &str, key: &str) -> OwnedMutexGuard<()> {
        let m = {
            self.map
                .lock()
                .unwrap()
                .entry(lock_key(bucket, key))
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        m.lock_owned().await
    }

    #[cfg(test)]
    pub fn try_busy(&self, bucket: &str, key: &str) -> bool {
        self.map
            .lock()
            .unwrap()
            .get(&lock_key(bucket, key))
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
        let g = locks.lock("bucket", "key").await;
        assert!(locks.try_busy("bucket", "key"));
        drop(g);
        assert!(!locks.try_busy("bucket", "key"));
    }

    #[tokio::test]
    async fn different_keys_independent() {
        let locks = KeyLocks::new();
        let _g1 = locks.lock("b", "k1").await;
        assert!(!locks.try_busy("b", "k2")); // 미사용 키는 미점유
        let _g2 = locks.lock("b", "k2").await; // 다른 키는 블록 안 됨
        assert!(locks.try_busy("b", "k1"));
        assert!(locks.try_busy("b", "k2"));
        // 버킷 축: 다른 버킷의 같은 키는 별개 락(= bucket이 락 키에 참여한다)
        assert!(!locks.try_busy("other", "k1"));
    }

    /// `lock_key`가 bucket을 무시하면(= key만으로 락을 잡으면) 서로 다른 버킷의
    /// 같은 키가 한 락으로 접혀 불필요하게 직렬화된다. 그 뮤턴트를 죽이는 테스트.
    #[tokio::test]
    async fn bucket_participates_in_lock_key() {
        use std::time::Duration;
        let locks = KeyLocks::new();
        let _g1 = locks.lock("b1", "same").await;
        assert!(locks.try_busy("b1", "same"));
        // 같은 키라도 버킷이 다르면 미점유
        assert!(!locks.try_busy("b2", "same"));
        // 그리고 블록되지 않고 실제로 잠긴다(타임아웃으로 hang 대신 실패하게 고정)
        let _g2 = tokio::time::timeout(Duration::from_secs(5), locks.lock("b2", "same"))
            .await
            .expect("다른 버킷의 같은 키는 블록되면 안 됨");
        assert!(locks.try_busy("b2", "same"));
        assert!(locks.try_busy("b1", "same")); // 원래 락은 그대로 유지
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
                let _g = locks.lock("b", "same").await;
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
