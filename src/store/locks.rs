use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// 락 획득이 이 시간을 넘기면 **한 번** `tracing::error!`를 낸다 — 그리고 **계속 기다린다**.
/// 임계값이 아니라 **관측 임계값**이다: 대기의 상계가 아니며, 에러를 반환하지 않는다.
///
/// **왜 30초인가**
/// - **정상 hold의 상한보다 압도적으로 크다.** 버퍼드 `put`이 키 락을 쥐는 구간은 blob read
///   (+필요 시 write) · stage · rename · fsync뿐이다 — 건강한 fs에서는 밀리초, 느린 HDD/NFS에서도
///   자릿수가 다르다. 30초를 넘겼다는 것은 **정상 fs로는 설명되지 않는다.**
/// - **사람이 신경 쓰기 시작하는 지점보다 작다.** 홈랩 파일 스토어에서 한 키가 30초간 쓰기 불가면
///   이미 그 키의 장애다. 임계를 `upload_timeout`(기본 600s) 위로 올리면 **정확히 이 신호가 존재하는
///   이유인 "영원히 안 풀리는 경우"에 10분짜리 맹점**이 생긴다 — 거꾸로다.
/// - ⚠ **알려진 오탐 클래스(정직하게)**: `put_stream`은 키 락을 **스트리밍 본문 내내** 쥔다
///   (`objects.rs`) → 같은 `bucket/key`의 **동시 writer**는 `upload_timeout`(기본 600s)까지
///   **정당하게** 기다릴 수 있다. 그때도 이 로그는 **거짓말하지 않는다**: 첫 절은 사실
///   ("락이 임계를 넘겨 잡혀 있다")이고 해석절은 유보("**may** be wedged")다. 게다가 수 분짜리
///   업로드 중에 같은 키로 또 쓰는 접근 패턴은 그 자체로 볼 만한 값이 있다.
const LOCK_WARN_AFTER: Duration = Duration::from_secs(30);

/// 같은 `bucket/key` PUT/DELETE를 직렬화(서로 다른 키는 병렬).
/// 단일 replica(replicas:1 + RWO PVC)라 in-process 락으로 충분.
#[derive(Clone)]
pub struct KeyLocks {
    map: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
    /// 관측 임계값. prod = `LOCK_WARN_AFTER`. 테스트만 `with_warn_after`로 줄인다.
    warn_after: Duration,
}

impl Default for KeyLocks {
    /// `derive(Default)`를 쓰지 않는 이유: `Duration::default() == ZERO`라 **매 락마다** 경고가
    /// 발화한다. 기본값은 **명시**한다.
    fn default() -> Self {
        Self {
            map: Arc::default(),
            warn_after: LOCK_WARN_AFTER,
        }
    }
}

/// 락 맵 키의 유일 저작점 — `bucket/key` 합성은 이 모듈 밖으로 새지 않는다.
fn lock_key(bucket: &str, key: &str) -> String {
    format!("{bucket}/{key}")
}

/// 직렬화 가드. **`KeyLocks::lock`만이 만든다**(필드 private · 같은 모듈 외 생성자 0)
/// → "아무 `OwnedMutexGuard<()>`나 커밋에 넘기기"가 **표현 불가**하다.
///
/// `'static`(owned)인 이유는 하나다: **커밋 클로저로 이전(move)되기 위해서**다.
/// 가드가 호출자 퓨처에 남으면 `upload_timeout`·disconnect가 그것을 드롭하는데,
/// **무취소 커밋 클로저는 아직 stage/rename 중**일 수 있다 → 같은 키의 재시도·delete가
/// 락을 얻어 먼저 끝나고, 뒤늦게 깨어난 낡은 rename이 **더 새로운 포인터를 덮어쓰거나
/// 삭제된 키를 되살린다**(B8 위반). 그래서 락의 수명은 **커밋과 같아야 한다**.
///
/// # ⚠ 재시작-필요 복구 계약 (S-2 — 의도된 교환)
///
/// 커밋 클로저는 `PinGuard`와 `KeyGuard`를 **함께 소유**하며 rename·fsync가 끝난 뒤에야 놓는다.
/// **시작된 `spawn_blocking`은 취소할 수 없다.** 따라서 **파일시스템 연산이 반환하지 않으면
/// 그 `bucket/key`는 syscall이 반환하거나 프로세스가 재시작될 때까지 쓰기 불가**가 된다
/// (같은 키의 DELETE는 타임아웃이 없다 — `objects.rs`의 `delete`).
///
/// **이것은 의도된 교환이다.** 가드를 (타임아웃 등으로) 먼저 놓으면 detach된 낡은 커밋이 더 새로운
/// 포인터를 덮어쓰거나 **성공적으로 삭제된 키를 되살린다**(무결성 손상 — S-1). **가용성을 잃는 편이
/// 낫다**: 멈춘 fs는 병리적 상황이고 그 경우 이 스토어는 이미 사실상 죽어 있으며(reconcile도 같은 fs를
/// 읽는다), 단일 replica + RWO PVC라 blast radius는 **그 키 하나**다.
/// **잠김(가용성) < 되살아나기(무결성)** — 삭제된 키의 부활은 **조용한 데이터 손상**이다.
///
/// 그 상황은 침묵하지 않는다: `KeyLocks::lock`이 `LOCK_WARN_AFTER`를 넘기면 **시끄럽게** 로그한다
/// (행동은 불변 — 여전히 무한정 기다린다). 증인: **T-S2**.
/// 잠김 없이 되살아나기를 막는 설계(키-바인드 펜싱 / 버전화된 포인터 발행)는 **F-30**.
pub struct KeyGuard(OwnedMutexGuard<()>);

impl KeyGuard {
    /// 해제의 **명시적** 지점. 커밋 클로저는 rename·fsync·핀drop이 **전부 끝난 뒤** 이것을 부른다
    /// → 드롭 순서가 암묵적 스코프 규칙이 아니라 **코드에** 박힌다.
    pub(crate) fn release(self) {
        drop(self.0);
    }
}

impl KeyLocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// 관측 임계값 주입(**테스트 전용** — `Store::with_hooks` 관행). prod 경로는 `new()`만 쓴다.
    #[cfg(test)]
    pub(crate) fn with_warn_after(warn_after: Duration) -> Self {
        Self {
            warn_after,
            ..Self::default()
        }
    }

    /// **무한정 기다린다.** 임계를 넘기면 **한 번 로그하고 계속 기다린다** — 반환 타입에 실패가 없다
    /// (`KeyGuard`, `Result` 아님) → "타임아웃으로 가드를 포기한다"가 **표현 불가**하다(S-1 부활 차단).
    pub async fn lock(&self, bucket: &str, key: &str) -> KeyGuard {
        let m = {
            self.map
                .lock()
                .unwrap()
                .entry(lock_key(bucket, key))
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        // 대기 퓨처는 **한 번만** 만들고 **드롭하지 않는다** → tokio Mutex의 FIFO 큐에서 자리를
        // 잃지 않는다. `timeout`이 매번 새 `lock_owned()`를 만들면 경고 시점마다 대기열 **뒤로**
        // 밀린다(공정성이 바뀐다) → 로그만 추가한다는 약속이 깨진다.
        let acquire = m.lock_owned();
        tokio::pin!(acquire);
        match tokio::time::timeout(self.warn_after, &mut acquire).await {
            Ok(g) => KeyGuard(g),
            Err(_) => {
                tracing::error!(
                    bucket,
                    key,
                    waited_ms = self.warn_after.as_millis() as u64,
                    "key lock held beyond threshold — an uncancellable commit may be wedged on a \
                     stalled filesystem; this key stays unwritable until the syscall returns or the \
                     process restarts (deliberate: releasing the guard would let a detached commit \
                     resurrect a deleted key)"
                );
                KeyGuard(acquire.await) // **계속 기다린다** — 행동 변화 0
            }
        }
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
