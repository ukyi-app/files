use crate::error::AppError;
use std::path::{Path, PathBuf};

const RESERVED_SUFFIXES: &[&str] = &[".meta.json", ".bucket.json"];

fn segment_ok(seg: &str) -> bool {
    !seg.is_empty()
        && seg != "."
        && seg != ".."
        && !seg.starts_with('.')
        && seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// (발견 P4-2) 공개 catch-all/헬스 경로 충돌 방지를 위해 예약된 버킷명.
const RESERVED_BUCKETS: &[&str] = &["api", "healthz", "readyz"];

pub fn valid_bucket(b: &str) -> Result<(), AppError> {
    if b.len() > 64 || !segment_ok(b) || RESERVED_BUCKETS.contains(&b) {
        return Err(AppError::BadRequest("invalid_bucket"));
    }
    Ok(())
}

pub fn valid_key(k: &str) -> Result<(), AppError> {
    if k.is_empty() || k.len() > 1024 || k.starts_with('/') {
        return Err(AppError::BadRequest("invalid_key"));
    }
    for s in RESERVED_SUFFIXES {
        if k.ends_with(s) {
            return Err(AppError::BadRequest("reserved_suffix"));
        }
    }
    if k.split('/').any(|s| !segment_ok(s)) {
        return Err(AppError::BadRequest("invalid_key"));
    }
    Ok(())
}

/// 메타 파일 경로 계산용(바이트는 content-addressed `.objects/`에 저장 — M3).
/// segment_ok가 traversal을 차단하므로 존재하지 않는 경로라도 안전(canonicalize 불요).
pub fn safe_object_path(root: &Path, bucket: &str, key: &str) -> Result<PathBuf, AppError> {
    valid_bucket(bucket)?;
    valid_key(key)?;
    Ok(root.join(bucket).join(key))
}

pub fn meta_path(object: &Path) -> PathBuf {
    let mut s = object.as_os_str().to_owned();
    s.push(".meta.json");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn valid_keys_accepted() {
        assert!(valid_key("a").is_ok());
        assert!(valid_key("a/b.zip").is_ok());
        assert!(valid_key("dir/sub/file.tar.gz").is_ok());
    }

    #[test]
    fn traversal_and_malformed_keys_rejected() {
        for k in ["../x", "a/../../etc", "/abs", "a//b", "a/./b", ".", "..", ""] {
            assert!(valid_key(k).is_err(), "should reject key {k:?}");
        }
    }

    #[test]
    fn reserved_suffixes_rejected() {
        assert!(valid_key("foo.meta.json").is_err());
        assert!(valid_key("x/foo.bucket.json").is_err());
    }

    #[test]
    fn hidden_and_control_chars_rejected() {
        assert!(valid_key(".tmp").is_err());
        assert!(valid_key("a/.hidden").is_err());
        assert!(valid_key("a\0b").is_err());
        assert!(valid_key("a\nb").is_err());
    }

    #[test]
    fn bucket_rules() {
        assert!(valid_bucket("skills").is_ok());
        assert!(valid_bucket("a/b").is_err());
        assert!(valid_bucket("").is_err());
        assert!(valid_bucket(".hidden").is_err());
        assert!(valid_bucket("api").is_err());
        assert!(valid_bucket("healthz").is_err());
        assert!(valid_bucket("readyz").is_err());
    }

    #[test]
    fn safe_object_path_stays_under_root() {
        let root = Path::new("/data");
        let p = safe_object_path(root, "skills", "a/b.zip").unwrap();
        assert_eq!(p, Path::new("/data/skills/a/b.zip"));
        assert!(p.starts_with("/data/skills"));
        assert!(safe_object_path(root, "skills", "../escape").is_err());
        assert!(safe_object_path(root, "api", "x").is_err());
    }

    #[test]
    fn meta_path_appends_suffix() {
        let obj = Path::new("/data/skills/a/b.zip");
        assert_eq!(meta_path(obj), Path::new("/data/skills/a/b.zip.meta.json"));
    }
}
