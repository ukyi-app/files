use crate::layout;
use std::path::Path;
use tokio::io::AsyncWriteExt;

/// temp→fsync(file)→rename→parent-dir fsync로 원자적·내구적 쓰기.
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

/// 디렉터리 fsync — rename/삭제 후 디렉터리 엔트리를 크래시 내구화.
pub async fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    let dir = dir.to_owned();
    tokio::task::spawn_blocking(move || std::fs::File::open(&dir)?.sync_all())
        .await
        .expect("join")
}

/// (발견 P3-1) 누락 디렉터리를 한 레벨씩 생성하고, 생성 직후 부모를 fsync해 새 엔트리를 내구화.
/// create_dir_all은 새 조상 디렉터리를 fsync하지 않아 새 버킷/중첩 키 첫 쓰기가 크래시에 유실될 수 있다.
pub async fn mkdir_p_durable(dir: &Path) -> std::io::Result<()> {
    let mut to_create: Vec<std::path::PathBuf> = Vec::new();
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
