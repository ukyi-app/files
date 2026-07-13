use crate::layout;
use std::io::Write;
use std::path::{Path, PathBuf};

/// temp→fsync(file)→rename→parent-dir fsync로 원자적·내구적 쓰기.
/// **공개 시그니처 불변.** 내부는 stage/commit 단일 정의에 위임한다(드리프트 0) —
/// syscall 시퀀스는 축자 동일하고, 취소 입도만 "부분 → 전무"로 좁아진다(부분 상태의 순감소).
///
/// ⚠ **입력을 한 번 복사한다**(`bytes.to_vec()`) — `spawn_blocking` 클로저가 `'static`을 요구하므로
/// 빌린 슬라이스를 그대로 넘길 수 없다. 현재 호출부는 **전부 작은 JSON**(`.gc-pending.json` ·
/// `.bucket.json` · 메타)**이거나 테스트 경로**(`Store::put`)이므로 무해하다 — HTTP 업로드 본문은
/// `put_stream`이 임시파일로 흘려보내며 이 함수를 지나지 않는다. **대용량 바이트를 이 함수로 쓰려는
/// 호출부가 생기면** 복사가 곧 비용이 된다 → 그때는 `Vec<u8>`의 **소유권을 받는 변종**이 필요하다.
pub async fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let (t, b) = (target.to_owned(), bytes.to_vec());
    tokio::task::spawn_blocking(move || stage_blocking(&t, &b)?.commit_blocking(|| {}))
        .await
        .expect("join") // 저장소 관행(fsync_dir)과 동일
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
pub(crate) struct Staged {
    tmp: PathBuf,
    target: PathBuf,
}

/// mkdir_p + create + write_all + sync_all. 여기까지의 실패는 target을 **전혀 건드리지 않는다**.
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
pub async fn mkdir_p_durable(dir: &Path) -> std::io::Result<()> {
    let d = dir.to_owned();
    tokio::task::spawn_blocking(move || mkdir_p_durable_blocking(&d))
        .await
        .expect("join")
}

/// `mkdir_p_durable`의 **유일한 정의**(async는 spawn_blocking 위임 · stage_blocking이 직접 호출).
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
}
