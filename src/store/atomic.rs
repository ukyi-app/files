//! 원자적·내구적 쓰기. **온디스크 시퀀스는 하나뿐이다** — 두 구현이 이것을 **축자 동일하게** 수행한다:
//!
//! ```text
//! mkdir_p_durable(parent) → tmp create → write_all → sync_all   (파일 내용 durable)
//!                         → rename(tmp → target) → fsync(parent) (디렉터리 엔트리 durable)
//! ```
//!
//! # ⚠ 같은 시퀀스의 **구현이 두 벌**이다 — 의도된 중복이다(DRY보다 **정확성**이 우선한다)
//!
//! | 경로 | 취소 의미론 | 무엇을 쓰나 |
//! |---|---|---|
//! | `write_atomic` (**async 체인**) | **취소 가능해야 한다** | 범용: `.bucket.json` · `.gc-pending.json` · blob · 무덤 · 메타 |
//! | `stage_blocking` + `Staged::commit_blocking` (**blocking 체인**) | **취소 불가능해야 한다** | **핀에 묶인 커밋 포인터 전용** — 유일한 호출자는 `PinGuard::commit_pointer` |
//!
//! **하나의 정의로 합칠 수 없다 — 두 경로의 취소 의미론이 서로 반대이기 때문이다:**
//!
//! - `PinGuard::commit_pointer`는 **무취소여야 한다.** rename과 **핀·키 락의 수명**이 한 blocking
//!   클로저에 묶여야, 호출자 취소(`upload_timeout`·disconnect)가 **in-flight rename에서 핀을 떼어내지
//!   못한다.** 떼어내면 GC가 착지 중인 blob을 회수한다 — **이 브랜치가 봉인한 데이터 손실 버그 그 자체다.**
//!   증인: **T-C2**(`caller_cancellation_mid_commit_still_protects_the_blob`) · **T-S1** · **T-S2**.
//! - `write_atomic`은 **취소 가능해야 한다.** 범용 헬퍼를 무취소로 만들면 **두 번째 관측 행동 플립**이
//!   되어 단일-플립 계약을 깬다(릴리스 게이트 **R-2**): 취소된 호출자의 `.bucket.json` ·
//!   `.gc-pending.json` 쓰기가 **취소 이후에 발행**되고, detach된 낡은 pending 쓰기가 **나중 패스와
//!   경합**한다. 그래서 이 함수는 baseline의 async 체인 **그대로**다 — 단계 사이의 취소가 **뒤 단계의
//!   폴링을 막는다**. 증인: **T-R2a**(`write_atomic_is_cancellable_before_rename`).
//!
//! ⚠ **드리프트 규율**: 한쪽 체인의 단계를 바꾸면 **반드시 다른 쪽도 같이 바꿔라.** 시퀀스가 갈라지면
//! 두 경로의 내구성 보장이 갈라진다. 갈라질 수 **없는** 조각(`fsync_dir_blocking` ·
//! `mkdir_p_durable_blocking`)은 **이미 공유한다** — 남은 중복은 **await 지점의 유무**뿐이고,
//! 그 차이가 **바로 이 모듈의 요점**이다.

use crate::layout;
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// temp→fsync(file)→rename→parent-dir fsync로 원자적·내구적 쓰기.
///
/// # ⚠ 이 함수는 **취소 가능**하다 — 그것이 계약이다(R-2)
///
/// 각 단계가 **제 자신의 `.await`**를 가진다. 호출자가 이 퓨처를 드롭하면(취소) **뒤 단계는 영영
/// 폴링되지 않는다** — 특히 **rename이 일어나지 않는다** → 취소된 호출자의 상태는 **발행되지 않는다.**
///
/// **`spawn_blocking` 하나로 감싸지 마라.** 그러면 stage·rename·fsync가 abort 불가능한 클로저가 되어
/// **취소 이후에도 완주한다** → detach된 낡은 `.gc-pending.json` 쓰기가 나중 GC 패스와 경합하고,
/// 취소된 버킷 생성이 `.bucket.json`을 발행한다. 무취소가 **필요한** 유일한 쓰기는 **핀에 묶인 커밋
/// 포인터**이며, 그것은 `PinGuard::commit_pointer`가 `stage_blocking`/`Staged::commit_blocking`으로
/// 따로 수행한다(모듈 doc의 표를 보라). **증인: T-R2a.**
pub async fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = target.parent().expect("target has parent");
    mkdir_p_durable(parent).await?; // (발견 P3-1) 조상 디렉터리도 내구적으로 생성
    // temp는 target의 형제(임의 부모 디렉터리) — 이름만 layout이 저작(root 비의존).
    let tmp = parent.join(layout::temp_name(&unique_suffix()));
    {
        let mut f = tokio::fs::File::create(&tmp).await?;
        f.write_all(bytes).await?;
        f.sync_all().await?; // 파일 내용 durable
    }
    tokio::fs::rename(&tmp, target).await?;
    fsync_dir(parent).await?; // rename(디렉터리 엔트리) durable
    Ok(())
}

/// 디렉터리 fsync의 **유일한 정의**(드리프트 0). 이 모듈에서 디렉터리 엔트리를 내구화하는 곳은
/// 넷뿐이며 — `rename_durable_blocking` · `Staged::commit_blocking` · `fsync_dir` ·
/// `mkdir_p_durable_blocking` — **전부 이것을 경유한다.** syscall 시퀀스는 `open` + `fsync`로 불변.
fn fsync_dir_blocking(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

/// rename + parent fsync. **증거 토큰을 발급하지 않는다** — 평범한 `io::Result<()>`다
/// (범용 rename이 내는 unit 토큰은 "blob→무덤 전이"에 아무 것도 바인딩하지 못한다).
pub(crate) fn rename_durable_blocking(
    from: &Path,
    to: &Path,
    parent: &Path,
) -> std::io::Result<()> {
    std::fs::rename(from, to)?;
    fsync_dir_blocking(parent)
}

pub(crate) async fn rename_durable(from: &Path, to: &Path, parent: &Path) -> std::io::Result<()> {
    let (f, t, p) = (from.to_owned(), to.to_owned(), parent.to_owned());
    tokio::task::spawn_blocking(move || rename_durable_blocking(&f, &t, &p))
        .await
        .expect("join")
}

/// 원자적 쓰기의 **stage 단계 산물**. commit이 아직 남았다.
///
/// ⚠ **커밋 포인터 전용**(`PinGuard::commit_pointer`). 범용 쓰기는 `write_atomic`을 쓴다 —
/// **이 타입을 경유하지 않는다**(R-2: 범용 헬퍼는 취소 가능해야 한다).
pub(crate) struct Staged {
    tmp: PathBuf,
    target: PathBuf,
}

/// mkdir_p + create + write_all + sync_all. 여기까지의 실패는 target을 **전혀 건드리지 않는다**.
///
/// ⚠ **커밋 포인터 전용 · 무취소 체인의 전반부.** 호출자는 `PinGuard::commit_pointer`의 blocking
/// 클로저 **하나뿐**이며, 그것이 이 stage와 `Staged::commit_blocking`을 **핀·키 락을 쥔 채** 잇는다
/// — 그래야 호출자 취소가 rename에서 핀을 떼어내지 못한다(T-C2 · T-S1).
/// **`write_atomic`을 여기로 다시 연결하지 마라** — 그것이 R-2가 잡아낸 두 번째 취소 플립이다.
/// `write_atomic`의 async 체인이 **같은 온디스크 시퀀스**를 수행한다(모듈 doc의 표).
pub(crate) fn stage_blocking(target: &Path, bytes: &[u8]) -> std::io::Result<Staged> {
    let parent = target.parent().expect("target has parent");
    mkdir_p_durable_blocking(parent)?; // (발견 P3-1) 조상 디렉터리도 내구적으로 생성
    // temp는 target의 형제(임의 부모 디렉터리) — 이름만 layout이 저작(root 비의존).
    let tmp = parent.join(layout::temp_name(&unique_suffix()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?; // 파일 내용 durable
    }
    Ok(Staged {
        tmp,
        target: target.to_owned(),
    })
}

impl Staged {
    /// rename이 **Ok를 반환한 직후에만** `on_landed`를 호출하고, 그 다음 parent를 fsync한다.
    /// `on_landed`는 **동기 클로저**다 — rename과 마킹 사이에 await/취소점이 존재할 수 없다.
    /// 이 시그니처는 이후 증분에서도 불변이다(훅은 호출자의 클로저 **안**에서 돈다).
    ///
    /// # ⚠ 순서 제약 (**load-bearing — 이 세 줄의 순서가 곧 픽스다**)
    ///
    /// `rename?` → `on_landed()` → `fsync_dir`. **`on_landed`를 `rename` 앞으로 옮기지 마라.**
    ///
    /// 이 콜백이 GC의 **유일한 보호 술어**(`landed`)를 심는다. 그 술어의 정의는 *"커밋 rename이 `Ok`를
    /// 반환했다"*(= **커밋 포인터가 VFS에 실재한다**)이지 *"커밋을 **시도**했다"*가 **아니다**.
    /// 앞으로 옮기면 stage 실패·rename `Err`로 죽은 put — **ENOSPC가 정확히 거기서 터진다** — 까지
    /// 흔적을 남겨, **포인터를 만들 수 없었던 put**이 blob을 보호한다 → **디스크가 찼을 때 GC가 공간을
    /// 회수하지 못하는 자기강화 루프**가 부활한다. **증인: T-C1**(`landed_trace_only_when_rename_returns_ok`).
    ///
    /// `fsync_dir`은 **내구성**이지 **가시성**이 아니다(POSIX rename이 `Ok`면 엔트리는 이미 VFS에 있다)
    /// → 흔적은 fsync **이전에** 심는 것이 맞다. fsync가 실패해도 포인터는 **존재한다**.
    pub(crate) fn commit_blocking(self, on_landed: impl FnOnce()) -> std::io::Result<()> {
        std::fs::rename(&self.tmp, &self.target)?; // ← 실패하면 on_landed는 절대 안 불린다
        on_landed(); // ← 착지 확정. 흔적은 여기서만 생긴다.
        fsync_dir_blocking(self.target.parent().expect("target has parent")) // rename(디렉터리 엔트리) durable
    }
}

/// 디렉터리 fsync — rename/삭제 후 디렉터리 엔트리를 크래시 내구화.
pub async fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    let dir = dir.to_owned();
    tokio::task::spawn_blocking(move || fsync_dir_blocking(&dir))
        .await
        .expect("join")
}

/// (발견 P3-1) 누락 디렉터리를 한 레벨씩 생성하고, 생성 직후 부모를 fsync해 새 엔트리를 내구화.
/// create_dir_all은 새 조상 디렉터리를 fsync하지 않아 새 버킷/중첩 키 첫 쓰기가 크래시에 유실될 수 있다.
///
/// ⚠ `write_atomic`과 **같은 이유로 취소 가능**해야 한다(R-2) — `write_atomic`의 **첫 단계**이자
/// `objects`/`reconcile`의 범용 헬퍼다. `spawn_blocking` 하나로 감싸면 그 취소 가능성이 사라진다.
/// blocking 체인이 쓰는 판본은 `mkdir_p_durable_blocking`이다(**같은 시퀀스** · await만 없다).
pub async fn mkdir_p_durable(dir: &Path) -> std::io::Result<()> {
    let mut to_create: Vec<PathBuf> = Vec::new();
    let mut cur = Some(dir.to_owned());
    while let Some(p) = cur {
        if tokio::fs::try_exists(&p).await? {
            break;
        }
        cur = p.parent().map(|x| x.to_owned());
        to_create.push(p);
    }
    for d in to_create.iter().rev() {
        match tokio::fs::create_dir(d).await {
            Ok(()) => {
                if let Some(parent) = d.parent() {
                    fsync_dir(parent).await?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// `mkdir_p_durable`의 **blocking 판본** — 무취소 커밋 경로(`stage_blocking`) 전용.
/// 위 async 판본과 **같은 시퀀스**(try_exists 상승 → 아래에서 위로 create_dir → 매 생성마다 부모 fsync).
/// ⚠ 한쪽을 고치면 **다른 쪽도 고쳐라**(모듈 doc의 드리프트 규율).
fn mkdir_p_durable_blocking(dir: &Path) -> std::io::Result<()> {
    let mut to_create: Vec<PathBuf> = Vec::new();
    let mut cur = Some(dir.to_owned());
    while let Some(p) = cur {
        if p.try_exists()? {
            break;
        }
        cur = p.parent().map(|x| x.to_owned());
        to_create.push(p);
    }
    for d in to_create.iter().rev() {
        match std::fs::create_dir(d) {
            Ok(()) => {
                if let Some(parent) = d.parent() {
                    fsync_dir_blocking(parent)?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

pub(crate) fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn write_atomic_roundtrip_no_temp_residue() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("sub/nested/file.bin");
        write_atomic(&target, b"hello durable").await.unwrap();
        let got = tokio::fs::read(&target).await.unwrap();
        assert_eq!(got, b"hello durable");

        // temp 잔재 없음
        let parent = target.parent().unwrap();
        let mut entries = tokio::fs::read_dir(parent).await.unwrap();
        while let Some(e) = entries.next_entry().await.unwrap() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            assert!(!name.starts_with(".tmp-"), "temp residue: {name}");
        }
    }

    #[tokio::test]
    async fn write_atomic_overwrites() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("f");
        write_atomic(&target, b"AAAA").await.unwrap();
        write_atomic(&target, b"BBBB").await.unwrap();
        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"BBBB");
    }

    #[tokio::test]
    async fn mkdir_p_durable_creates_nested_idempotent() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        mkdir_p_durable(&nested).await.unwrap();
        assert!(tokio::fs::try_exists(&nested).await.unwrap());
        mkdir_p_durable(&nested).await.unwrap(); // idempotent
    }

    // ── T-R2a ────────────────────────────────────────────────────────────────────────────

    /// **T-R2a — `write_atomic`은 취소 가능하다**(릴리스 게이트 R-2: **두 번째 관측 플립 금지**).
    ///
    /// 명제: *"첫 `.await` 이후 · rename 이전에 퓨처를 드롭하면 **타깃이 생기지 않는다**"*
    /// — 즉 취소가 **뒤 단계의 폴링을 막았다**. 이것이 baseline 의미론이고, 무취소 커밋 경로
    /// (`PinGuard::commit_pointer` → T-C2)와의 **경계**다.
    ///
    /// # ⚠ 결정성 (§5 랑데부 규율: **개시 ≠ 완료**)
    ///
    /// "첫 폴은 Pending이겠지"에 기대면 **flaky**다 — `spawn_blocking`이 폴 사이에 완료될 수 있다.
    /// 그래서 **blocking 풀을 구조적으로 봉쇄**한다: `max_blocking_threads(1)`로 스레드를 **하나**로
    /// 묶고, 그 하나를 우리가 **점거**한다(점거 *완료*를 채널로 관측한다 — spawn ≠ 실행).
    /// 그러면 `write_atomic`의 첫 `tokio::fs` 연산(`mkdir_p_durable`의 `try_exists`)은 **큐에만 쌓이고
    /// 실행될 수 없다** → **첫 폴은 Pending임이 보장된다** ∧ **rename은 아직 큐잉조차 되지 않았다**.
    ///
    /// **드레인도 결정적이다**: 블로킹 큐는 FIFO이고 스레드는 **하나**다 → 점거를 풀고 **펜스**
    /// (no-op `spawn_blocking`)를 await하면, 그보다 **먼저 큐잉된 detach 작업은 전부 끝났다**.
    /// 그 뒤의 단언이라야 정직하다.
    ///
    /// # 뮤턴트 킬
    ///
    /// `write_atomic`을 `spawn_blocking(|| stage_blocking(..)?.commit_blocking(|| {}))`로 되돌리면:
    /// 드롭이 JoinHandle을 **detach**할 뿐이라 클로저가 **완주**한다 → 펜스 이후 **타깃이 존재한다**
    /// → 마지막 단언 **RED**. (실증됨.)
    #[test]
    fn write_atomic_is_cancellable_before_rename() {
        use std::sync::mpsc;

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1) // ← blocking 스레드는 **하나뿐**이다
            .build()
            .unwrap();

        let dir = tempdir().unwrap();
        // 범용 호출부의 대표. R-2가 지목한 바로 그 파일들이다(버킷 메타 · GC pending).
        let target = dir.path().join("bucket").join(".bucket.json");
        let big = vec![b'x'; 4 << 20]; // "큰 바이트"

        let (occupied_tx, occupied_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        rt.block_on(async {
            // ① 유일한 blocking 스레드를 **점거**한다.
            let occupier = tokio::task::spawn_blocking(move || {
                occupied_tx.send(()).unwrap(); // ← 점거 **완료** 신호(클로저가 **실행 중**이다)
                let _ = release_rx.recv(); // sender drop = 해제
            });
            occupied_rx.recv().unwrap(); // ② 점거 완료를 **관측**한다(개시 ≠ 완료)

            // ③ 딱 **한 번** 폴한다 → 첫 await(`try_exists`)에서 Pending이 **보장**된다.
            {
                let mut fut = std::pin::pin!(write_atomic(&target, &big));
                assert!(
                    futures::poll!(fut.as_mut()).is_pending(),
                    "블로킹 풀이 봉쇄된 동안 write_atomic이 완료될 수는 없다"
                );
                // ④ **취소**: 드롭. 이 지점 이후 create/write_all/sync_all/**rename**은
                //    **영영 폴링되지 않는다** — async 체인에서는 그것이 곧 "일어나지 않는다"이다.
            }

            // ⑤ 점거 해제 → 큐에 남은(=detach된) 작업이 있다면 **지금** 실행된다.
            drop(release_tx);
            occupier.await.unwrap();
            // ⑥ **FIFO 펜스**: 스레드 1개 + FIFO 큐 → 이 no-op이 끝났다면 그 앞의 detach 작업은
            //    **전부 끝났다**. (뮤턴트라면 무취소 클로저가 여기서 rename까지 완주해 있다.)
            tokio::task::spawn_blocking(|| {}).await.unwrap();
        });

        // ⑦ **취소가 rename을 막았다.**
        assert!(
            !target.try_exists().unwrap(),
            "취소된 write_atomic이 타깃을 발행했다 — 무취소 플립이 부활했다(R-2)"
        );
    }
}
