//! 디스크 레이아웃의 단일 소유자: 경로 저작(making)과 이름 읽기(reading).
//! 검증·경로 규칙은 구 src/path.rs에서 축자 이주.

use crate::error::AppError;
use std::path::{Path, PathBuf};

/// 커밋 포인터(`<key>.meta.json`) 접미사 — 온디스크 리터럴의 단일 정의.
const META_SUFFIX: &str = ".meta.json";
/// 버킷 메타 파일명 — 온디스크 리터럴의 단일 정의.
const BUCKET_META_NAME: &str = ".bucket.json";
/// 스트리밍 업로드 temp blob 접두사 — 온디스크 리터럴의 단일 정의.
const TMP_PREFIX: &str = ".tmp-";
/// 2단계 GC tombstone 파일명 — 온디스크 리터럴의 단일 정의.
const GC_PENDING_NAME: &str = ".gc-pending.json";
/// bit-rot 격리 디렉터리명 — 온디스크 리터럴의 단일 정의.
const CORRUPT_DIR_NAME: &str = ".corrupt";
/// GC 무덤 접두사 — 온디스크 리터럴의 단일 정의.
/// `.objects` **직속 평면 이름**(`mkdir` 없음 → 빈 디렉터리 잔재 불가)이고 `.tmp-`가 **아니다**
/// (temp로 오분류돼 만료 삭제되는 경로를 원천 차단). 이름이 sha를 품으므로 복구가 가능하다.
/// 구 바이너리의 `classify_objects_entry`는 이 이름을 `Other`로 떨어뜨린다 → 롤백해도 삭제되지 않는다.
const GRAVE_PREFIX: &str = ".gc-grave-";

const RESERVED_SUFFIXES: &[&str] = &[META_SUFFIX, BUCKET_META_NAME];

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
pub(crate) const RESERVED_BUCKETS: &[&str] = &["api", "healthz", "readyz"];

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
fn safe_object_path(root: &Path, bucket: &str, key: &str) -> Result<PathBuf, AppError> {
    valid_bucket(bucket)?;
    valid_key(key)?;
    Ok(root.join(bucket).join(key))
}

fn meta_path(object: &Path) -> PathBuf {
    let mut s = object.as_os_str().to_owned();
    s.push(META_SUFFIX);
    PathBuf::from(s)
}

/// content-addressed 바이트 저장 디렉터리 이름.
pub(crate) const OBJECTS_DIR: &str = ".objects";

/// 디스크 레이아웃. 모든 경로 저작의 단일 소유자.
#[derive(Clone)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// 레이아웃의 베이스 디렉터리. 경로 저작이 아니라 루트 자체가 필요한 소비자용
    /// (루트 `read_dir`, 버킷 서브트리 워크 시작점) — 이름 규칙의 단일 소유는 불변.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// (bucket, key)의 커밋 포인터(`<key>.meta.json`) 경로.
    pub fn meta_for(&self, bucket: &str, key: &str) -> Result<PathBuf, AppError> {
        Ok(meta_path(&safe_object_path(&self.root, bucket, key)?))
    }

    /// sha256 blob 경로(`root/.objects/<sha>`).
    pub fn blob_path(&self, sha: &str) -> PathBuf {
        self.root.join(OBJECTS_DIR).join(sha)
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.root.join(OBJECTS_DIR)
    }

    /// 스트리밍 업로드용 temp blob 경로(`root/.objects/.tmp-<unique>`).
    pub fn temp_blob_path(&self, unique: &str) -> PathBuf {
        self.root.join(OBJECTS_DIR).join(temp_name(unique))
    }

    /// 버킷 메타(`root/<bucket>/.bucket.json`) 경로. 버킷명 검증 후 저작.
    pub fn bucket_meta_path(&self, bucket: &str) -> Result<PathBuf, AppError> {
        valid_bucket(bucket)?;
        Ok(self.root.join(bucket).join(BUCKET_META_NAME))
    }

    /// 2단계 GC tombstone 파일 경로.
    pub fn gc_pending_path(&self) -> PathBuf {
        self.root.join(OBJECTS_DIR).join(GC_PENDING_NAME)
    }

    /// bit-rot 격리 디렉터리 경로.
    pub fn corrupt_dir(&self) -> PathBuf {
        self.root.join(OBJECTS_DIR).join(CORRUPT_DIR_NAME)
    }

    /// GC 무덤 경로(`root/.objects/.gc-grave-<sha>`) — `.objects` 직속 평면 파일.
    pub(crate) fn grave_path(&self, sha: &str) -> PathBuf {
        self.root.join(OBJECTS_DIR).join(grave_name(sha))
    }

    /// 버킷 하나의 커밋 포인터 워크. `valid_bucket` 실패 시 I/O 전에 Err.
    /// 버킷 dir 부재는 빈 워크(첫 next()가 Ok(None)).
    pub fn pointers_in_bucket(&self, bucket: &str) -> Result<CommitPointerWalk, AppError> {
        valid_bucket(bucket)?;
        Ok(CommitPointerWalk {
            state: WalkState::SeedBucket {
                bucket: bucket.to_string(),
                bucket_dir: self.root.join(bucket),
            },
            stack: Vec::new(),
            current: None,
        })
    }

    /// 전 버킷 커밋 포인터 워크. 루트 직속 디렉터리만 버킷으로 시드하고
    /// `.objects`는 스킵(루트 직속 파일은 절대 후보 아님).
    pub fn pointers_all(&self) -> CommitPointerWalk {
        CommitPointerWalk {
            state: WalkState::SeedRoot {
                root: self.root.clone(),
            },
            stack: Vec::new(),
            current: None,
        }
    }
}

/// `.objects` 직속 항목의 이름-전용 분류(총함수 — I/O 없음, 변종이 이름 공간을 분할).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectsEntry {
    Reserved,
    Temp,
    Blob,
    Grave,
    Other,
}

/// 64자 ascii hex 여부(정규화 없음 — 대문자 허용).
fn is_sha_name(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 무덤 이름에서 sha 추출. 접두사가 맞고 나머지가 sha 모양일 때만 Some.
pub(crate) fn grave_sha(name: &str) -> Option<&str> {
    name.strip_prefix(GRAVE_PREFIX).filter(|s| is_sha_name(s))
}

/// 무덤 이름 저작(`.gc-grave-<sha>`) — 온디스크 무덤 접두사의 유일한 저작점.
pub(crate) fn grave_name(sha: &str) -> String {
    format!("{GRAVE_PREFIX}{sha}")
}

/// Reserved = `.gc-pending.json`/`.corrupt` 정확 일치; Grave = `.gc-grave-<64hex>`;
/// Temp = `.tmp-` 접두(우선); Blob = 64자 ascii hex(대문자 허용 — 정규화 금지); 그 외 Other.
pub fn classify_objects_entry(name: &str) -> ObjectsEntry {
    if name == GC_PENDING_NAME || name == CORRUPT_DIR_NAME {
        return ObjectsEntry::Reserved;
    }
    if grave_sha(name).is_some() {
        return ObjectsEntry::Grave;
    }
    if name.starts_with(TMP_PREFIX) {
        return ObjectsEntry::Temp;
    }
    if is_sha_name(name) {
        return ObjectsEntry::Blob;
    }
    ObjectsEntry::Other
}

/// 임시 파일명(`.tmp-<unique>`) — 온디스크 temp 접두사의 유일한 저작점.
/// 경로가 아닌 이름만 만든다: atomic::write_atomic처럼 임의 부모 디렉터리의
/// 형제로 temp를 두는 소비자가 사용한다.
pub(crate) fn temp_name(unique: &str) -> String {
    format!("{TMP_PREFIX}{unique}")
}

/// 커밋 포인터 파일명 판별(W1). 디렉터리 여부는 워커가 file_type으로 별도 판정.
/// `.bucket.json`은 `.meta.json`으로 끝나지 않아 외연상 자연 배제된다.
fn is_commit_pointer_name(name: &str) -> bool {
    !name.starts_with(TMP_PREFIX) && name.ends_with(META_SUFFIX)
}

/// 워커가 낸 커밋 포인터 한 건. `meta_path`는 절대 경로, `key`는 버킷-상대.
pub struct CommitPointerEntry {
    pub bucket: String,
    pub key: String,
    pub meta_path: PathBuf,
}

enum WalkState {
    /// pointers_in_bucket: 첫 next()에서 버킷 dir 존재 확인 후 시드.
    SeedBucket { bucket: String, bucket_dir: PathBuf },
    /// pointers_all: 첫 next()에서 루트 read_dir로 버킷 dir들을 시드.
    SeedRoot { root: PathBuf },
    Walking,
    /// 소진 또는 Err 이후(fused — W5).
    Done,
}

struct Frame {
    dir: PathBuf,
    bucket: String,
    bucket_dir: PathBuf,
}

struct OpenDir {
    rd: tokio::fs::ReadDir,
    bucket: String,
    bucket_dir: PathBuf,
}

/// 커밋 포인터 풀-방식 워커(LIFO 스택-DFS, yield 순서 비보장 — W3).
/// 낸 파일을 절대 열지 않는다(W4: read_dir/next_entry/file_type + 시드 try_exists뿐).
pub struct CommitPointerWalk {
    state: WalkState,
    stack: Vec<Frame>,
    current: Option<OpenDir>,
}

impl CommitPointerWalk {
    /// 다음 커밋 포인터. Err 반환 이후의 호출은 Ok(None)(fused — W5).
    pub async fn next(&mut self) -> std::io::Result<Option<CommitPointerEntry>> {
        if matches!(self.state, WalkState::Done) {
            return Ok(None);
        }
        match self.step().await {
            Ok(Some(entry)) => Ok(Some(entry)),
            Ok(None) => {
                self.state = WalkState::Done;
                Ok(None)
            }
            Err(e) => {
                self.state = WalkState::Done;
                Err(e)
            }
        }
    }

    async fn step(&mut self) -> std::io::Result<Option<CommitPointerEntry>> {
        // 시드 단계(첫 next()에서 1회). Err는 next()가 Done으로 봉인.
        match std::mem::replace(&mut self.state, WalkState::Walking) {
            WalkState::SeedBucket { bucket, bucket_dir } => {
                if !tokio::fs::try_exists(&bucket_dir).await? {
                    return Ok(None);
                }
                self.stack.push(Frame {
                    dir: bucket_dir.clone(),
                    bucket,
                    bucket_dir,
                });
            }
            WalkState::SeedRoot { root } => {
                let mut rd = tokio::fs::read_dir(&root).await?;
                while let Some(e) = rd.next_entry().await? {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    if name == OBJECTS_DIR {
                        continue;
                    }
                    if e.file_type().await?.is_dir() {
                        let dir = e.path();
                        self.stack.push(Frame {
                            dir: dir.clone(),
                            bucket: name.into_owned(),
                            bucket_dir: dir,
                        });
                    }
                }
            }
            WalkState::Walking => {}
            WalkState::Done => unreachable!("next()가 Done을 걸러냄"),
        }
        loop {
            if let Some(cur) = self.current.as_mut() {
                match cur.rd.next_entry().await? {
                    Some(entry) => {
                        // lstat 의미론(심링크 비추적) — is_file() 사용 금지(W1).
                        if entry.file_type().await?.is_dir() {
                            self.stack.push(Frame {
                                dir: entry.path(),
                                bucket: cur.bucket.clone(),
                                bucket_dir: cur.bucket_dir.clone(),
                            });
                            continue;
                        }
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        if !is_commit_pointer_name(&name) {
                            continue;
                        }
                        let meta_path = entry.path();
                        // W2: 버킷-상대 경로에서 META_SUFFIX 접미 제거로 key 복원.
                        // is_commit_pointer_name이 같은 META_SUFFIX로 필터했으므로 unwrap 안전.
                        let rel = meta_path.strip_prefix(&cur.bucket_dir).unwrap();
                        let key = rel
                            .to_string_lossy()
                            .strip_suffix(META_SUFFIX)
                            .unwrap()
                            .to_string();
                        return Ok(Some(CommitPointerEntry {
                            bucket: cur.bucket.clone(),
                            key,
                            meta_path,
                        }));
                    }
                    None => {
                        self.current = None;
                    }
                }
            } else if let Some(frame) = self.stack.pop() {
                let rd = tokio::fs::read_dir(&frame.dir).await?;
                self.current = Some(OpenDir {
                    rd,
                    bucket: frame.bucket,
                    bucket_dir: frame.bucket_dir,
                });
            } else {
                return Ok(None);
            }
        }
    }
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

    #[test]
    fn classify_objects_entry_table() {
        use ObjectsEntry::*;
        assert_eq!(classify_objects_entry(".gc-pending.json"), Reserved);
        assert_eq!(classify_objects_entry(".corrupt"), Reserved);
        assert_eq!(classify_objects_entry(".tmp-x"), Temp);
        let lower = "a".repeat(64);
        assert_eq!(classify_objects_entry(&lower), Blob);
        let upper = "A0F3".repeat(16); // 64자 대문자 혼합 hex — 정규화 없이 Blob
        assert_eq!(classify_objects_entry(&upper), Blob);
        assert_eq!(classify_objects_entry(&"a".repeat(63)), Other);
        assert_eq!(classify_objects_entry(&"a".repeat(65)), Other);
        assert_eq!(classify_objects_entry(&"g".repeat(64)), Other); // 비-hex
        assert_eq!(classify_objects_entry(".tmp-x.meta.json"), Temp); // 접두 우선
        // 무덤: `.gc-grave-<64hex>`만 Grave. temp도 blob도 아니다(오분류 삭제 차단).
        assert_eq!(classify_objects_entry(&grave_name(&lower)), Grave);
        assert_eq!(classify_objects_entry(&grave_name(&upper)), Grave);
        assert_eq!(classify_objects_entry(".gc-grave-junk"), Other); // sha 아님
        assert_eq!(classify_objects_entry(".gc-grave-"), Other);
        assert_eq!(classify_objects_entry(&grave_name(&"a".repeat(63))), Other);
    }

    #[test]
    fn grave_name_round_trips() {
        let sha = "b".repeat(64);
        let name = grave_name(&sha);
        assert_eq!(name, format!(".gc-grave-{sha}"));
        assert_eq!(grave_sha(&name), Some(sha.as_str()));
        // 비-무덤 이름은 sha를 내지 않는다
        assert_eq!(grave_sha(&sha), None);
        assert_eq!(grave_sha(".tmp-x"), None);
        assert_eq!(grave_sha(".gc-grave-nope"), None);
        // 무덤 이름은 `.tmp-` 이름공간과 서로소다
        assert!(!name.starts_with(".tmp-"));
    }

    #[test]
    fn temp_name_authors_prefix() {
        assert_eq!(temp_name("u1"), ".tmp-u1");
    }

    #[test]
    fn making_methods_author_expected_paths() {
        let l = Layout::new(PathBuf::from("/data"));
        assert_eq!(l.objects_dir(), Path::new("/data/.objects"));
        assert_eq!(l.blob_path("abc"), Path::new("/data/.objects/abc"));
        assert_eq!(l.temp_blob_path("u1"), Path::new("/data/.objects/.tmp-u1"));
        assert_eq!(
            l.gc_pending_path(),
            Path::new("/data/.objects/.gc-pending.json")
        );
        assert_eq!(l.corrupt_dir(), Path::new("/data/.objects/.corrupt"));
        assert_eq!(
            l.grave_path("abc"),
            Path::new("/data/.objects/.gc-grave-abc")
        );
        assert_eq!(
            l.bucket_meta_path("b").unwrap(),
            Path::new("/data/b/.bucket.json")
        );
        assert!(l.bucket_meta_path("api").is_err());
        assert_eq!(
            l.meta_for("b", "a/c.zip").unwrap(),
            Path::new("/data/b/a/c.zip.meta.json")
        );
    }

    async fn plant(path: &Path) {
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(path, b"x").await.unwrap();
    }

    async fn collect(mut w: CommitPointerWalk) -> Vec<(String, String)> {
        let mut out = Vec::new();
        while let Some(e) = w.next().await.unwrap() {
            out.push((e.bucket, e.key));
        }
        out.sort();
        out
    }

    #[tokio::test]
    async fn walker_yields_exactly_commit_pointers() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // ".bucket.json"은 ".meta.json"으로 안 끝나므로 술어가 자연 배제(포섭 핀).
        plant(&root.join("b/.bucket.json")).await;
        plant(&root.join("b/.tmp-x.meta.json")).await;
        plant(&root.join("b/x.meta.json")).await;
        plant(&root.join("b/d/n.meta.json")).await;
        plant(&root.join("b/plain.txt")).await;
        let l = Layout::new(root.to_path_buf());
        let got = collect(l.pointers_in_bucket("b").unwrap()).await;
        assert_eq!(
            got,
            vec![
                ("b".to_string(), "d/n".to_string()),
                ("b".to_string(), "x".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn walker_round_trips_meta_for() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path().to_path_buf());
        let (b, k) = ("skills", "a/b.zip");
        let mp = l.meta_for(b, k).unwrap();
        plant(&mp).await;
        let mut w = l.pointers_in_bucket(b).unwrap();
        let e = w.next().await.unwrap().unwrap();
        assert_eq!(e.bucket, b);
        assert_eq!(e.key, k);
        assert_eq!(e.meta_path, mp);
        assert!(w.next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn walker_rejects_reserved_bucket_and_empty_on_absent() {
        let d = tempfile::tempdir().unwrap();
        let l = Layout::new(d.path().to_path_buf());
        assert!(matches!(
            l.pointers_in_bucket("api"),
            Err(AppError::BadRequest("invalid_bucket"))
        ));
        let mut w = l.pointers_in_bucket("nope").unwrap();
        assert!(w.next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn pointers_all_skips_objects_and_covers_buckets() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        plant(&root.join(".objects/x.meta.json")).await; // 가짜 — 절대 안 냄
        plant(&root.join("b1/k1.meta.json")).await;
        plant(&root.join("b2/d/k2.meta.json")).await;
        let l = Layout::new(root.to_path_buf());
        let got = collect(l.pointers_all()).await;
        assert_eq!(
            got,
            vec![
                ("b1".to_string(), "k1".to_string()),
                ("b2".to_string(), "d/k2".to_string()),
            ]
        );
    }
}
