//! 스냅샷 항목 — 루프가 `.objects`를 만지는 **유일한 통로**. `read_dir`도 여기 있다
//! ⇒ `tokio::fs::DirEntry`가 `reconcile.rs`에 **한 번도 등장하지 않는다**(P-3).

use super::absence::{
    entry_is_absent, rename_durable_source_checked, rename_source_checked, Absent, Renamed,
    Vanished,
};
use crate::layout::{classify_objects_entry, ObjectsEntry};
use std::fs::{FileType, Metadata};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// `Gone`은 `T`를 들지 않는다 ⇒ **`T`를 주조할 필요가 없다** ⇒ 호출부가 오늘의 `ft.is_dir()` ·
/// `m.modified().unwrap_or(now)`를 **축자 그대로** 쓴다.
#[must_use]
pub(super) enum Seen<T> {
    Present(T),
    Gone(#[allow(dead_code)] Absent),
}

/// rename 두 갈래(`rename_into` · `rename_durable_to`)가 **글자 그대로 같은 변환**을 하고 있었다.
/// ⚠ **봉인 무영향**: `Seen`은 `pub(super)`(= reconcile 서브트리 전용)이라 이 `impl`을 **바깥에서
/// 이름조차 부를 수 없고**, `Renamed::SourceGone`은 `Absent`를 요구하므로 `pins`가 위조할 수도 없다.
impl From<Renamed> for Seen<()> {
    fn from(r: Renamed) -> Self {
        match r {
            Renamed::Done => Seen::Present(()),
            Renamed::SourceGone(a) => Seen::Gone(a),
        }
    }
}

pub(super) struct Entry<'v> {
    /// ★ 오늘의 핸들 **그대로**. `path()`/`file_type()`/`metadata()`의 주인.
    de: tokio::fs::DirEntry,
    /// `de.path()` (스냅샷 시점 1회 · syscall 0). **접근자 없음**.
    path: PathBuf,
    /// lossy. 분류·로깅·원장 키·**목적지 이름** 전용(= 오늘의 용도).
    name: String,
    class: ObjectsEntry,
    /// ⚠ **빌린다 — 소유/클론하지 않는다**(클론이 곧 대체 집계다).
    vanished: &'v Vanished,
}
// ⚠⚠ **`dir: PathBuf` 필드는 없다** — 경로는 **오직 `de.path()`에서만** 나온다(M46 표현 불가).

impl<'v> Entry<'v> {
    /// **오늘과 글자 그대로 동일한 `read_dir`/`next_entry`.**
    pub(super) async fn snapshot(dir: &Path, vanished: &'v Vanished) -> std::io::Result<Vec<Self>> {
        let mut out = Vec::new();
        let mut rd = tokio::fs::read_dir(dir).await?;
        while let Some(de) = rd.next_entry().await? {
            let path = de.path();
            let name = de.file_name();
            let name = name.to_string_lossy().to_string();
            let class = classify_objects_entry(&name); // 이름-전용 ⇒ syscall 0 ⇒ O1 보존
            out.push(Entry {
                de,
                path,
                name,
                class,
                vanished,
            });
        }
        Ok(out)
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn class(&self) -> ObjectsEntry {
        self.class
    }

    // ── FS 접촉은 전부 `self.seen(...)`을 지난다. 위임 대상은 **오늘의 호출 그 자체**다. ──

    /// tokio는 readdir 청크를 채우는 시점에 `d_type`을 캐시한다 ⇒ 소멸한 항목에도 `Ok`가 난다
    /// ⇒ **`Gone` 팔은 사실상 발화하지 않는다**(정책은 균일하게 유지한다).
    pub(super) async fn file_type(&self) -> std::io::Result<Seen<FileType>> {
        let r = self.de.file_type().await;
        self.seen(r).await
    }

    /// lstat 의미론(댕글링 심링크 → `Ok`) — W4.
    pub(super) async fn metadata(&self) -> std::io::Result<Seen<Metadata>> {
        let r = self.de.metadata().await;
        self.seen(r).await
    }

    /// `read`는 open이므로 **심링크를 추종한다** ⇒ 확인이 **load-bearing**한 유일한 지점(P-1).
    pub(super) async fn read(&self) -> std::io::Result<Seen<Vec<u8>>> {
        let r = tokio::fs::read(&self.path).await;
        self.seen(r).await
    }

    pub(super) async fn remove(&self) -> std::io::Result<Seen<()>> {
        let r = tokio::fs::remove_file(&self.path).await;
        self.seen(r).await
    }

    /// 격리 rename — 소스 = `self.path`(원시) · 목적지 = `dir.join(&self.name)`(lossy).
    /// **오늘과 같은 짝**이다.
    pub(super) async fn rename_into(&self, to_dir: &Path) -> std::io::Result<Seen<()>> {
        Ok(
            rename_source_checked(&self.path, &to_dir.join(&self.name), self.vanished)
                .await?
                .into(),
        )
    }

    /// 무덤 복구 rename(rename + parent fsync 융합 — rename `Ok` 이후의 fsync 실패는 **무가공**).
    pub(super) async fn rename_durable_to(
        &self,
        to: &Path,
        fsync_parent: &Path,
    ) -> std::io::Result<Seen<()>> {
        Ok(
            rename_durable_source_checked(&self.path, to, fsync_parent, self.vanished)
                .await?
                .into(),
        )
    }

    /// **`NotFound` 흡수의 유일한 지점.** 이름도 경로도 `self`뿐 ⇒ **목적지를 확인하는 판본은 없다.**
    async fn seen<T>(&self, r: std::io::Result<T>) -> std::io::Result<Seen<T>> {
        match r {
            Ok(v) => Ok(Seen::Present(v)),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                match entry_is_absent(self.vanished, &self.path).await {
                    Some(a) => Ok(Seen::Gone(a)), // ← 계수는 여기서 이미 일어났다
                    None => Err(e),               // ← 원본 그대로(댕글링 심링크 · 확인 불가)
                }
            }
            Err(e) => Err(e), // ← B7. **유일한 갈림길.**
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  **W1 · W2** — `seen`의 정책 표(유일한 갈림길)와 FS 메서드 전수의 `Gone` 보고.
// ══════════════════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    /// **W1 — `seen`은 *확인된 부재*만 흡수한다.** 네 팔 전부를 판다.
    ///
    /// (a) 부재 경로 + `NotFound` → **`Gone`**(+계수) · (b) **댕글링 심링크** + `NotFound` → **raw**
    /// (항목이 **있다** — P-1) · (c) 살아 있는 일반 파일 + `NotFound` → **raw** ·
    /// (d) 부재 경로 + **`NotFound` 이외** → **raw**(B7 — kind·메시지 **무변조** · 확인조차 하지 않는다).
    ///
    /// ⇒ **M-NOCHECK**(모든 `NotFound` skip)는 (b)(c)에서, **M-B7**(비-`NotFound`도 skip)은 (d)에서 RED다.
    #[tokio::test]
    async fn seen_absorbs_only_confirmed_absence() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        std::fs::write(dir.join("aaa-victim"), b"v").unwrap();
        std::fs::write(dir.join("bbb-present"), b"p").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("nowhere"), dir.join("ccc-dangling")).unwrap();

        let vanished = Vanished::new();
        let entries = Entry::snapshot(dir, &vanished).await.unwrap();
        let by = |n: &str| {
            entries
                .iter()
                .find(|e| e.name() == n)
                .unwrap_or_else(|| panic!("스냅샷에 {n}이 있어야 한다"))
        };

        // 스냅샷 **이후** 소멸 — 이제 `aaa-victim`의 경로는 진짜로 비어 있다.
        std::fs::remove_file(dir.join("aaa-victim")).unwrap();

        // ── (a) 부재 경로 + NotFound → **Gone** ─────────────────────────────────────────────
        let victim = by("aaa-victim");
        let r: std::io::Result<()> = Err(std::io::Error::from(ErrorKind::NotFound));
        assert!(
            matches!(victim.seen(r).await.unwrap(), Seen::Gone(_)),
            "(a) 확인된 부재만이 `Gone`이다"
        );
        assert_eq!(vanished.get(), 1, "(a) 부재 확인과 계수는 같은 행위다");

        // `Seen<T>`는 `Debug`를 유도하지 않는다(프로덕션 타입 diff 0) → **`match`로 언랩**한다.
        async fn raw_err(e: &Entry<'_>, r: std::io::Error, why: &str) -> std::io::Error {
            match e.seen::<()>(Err(r)).await {
                Ok(_) => panic!("{why}"),
                Err(e) => e,
            }
        }

        // ── (b) 댕글링 심링크 + NotFound → **raw**(메시지 무변조) ───────────────────────────
        #[cfg(unix)]
        {
            let dangling = by("ccc-dangling");
            let raw = std::io::Error::new(ErrorKind::NotFound, "w1-dangling-marker");
            let want = raw.to_string();
            let e = raw_err(
                dangling,
                raw,
                "(b) 댕글링 심링크는 **항목이 있다** ⇒ skip 금지 ⇒ 오늘의 Err 보존",
            )
            .await;
            assert_eq!(e.kind(), ErrorKind::NotFound, "(b) kind 무변조");
            assert_eq!(
                e.to_string(),
                want,
                "(b) **메시지 무변조** — 합성하지 않는다"
            );
            assert_eq!(vanished.get(), 1, "(b) 항목이 있으므로 계수하지 않는다");
        }

        // ── (c) 살아 있는 일반 파일 + NotFound → **raw** ────────────────────────────────────
        let e = raw_err(
            by("bbb-present"),
            std::io::Error::from(ErrorKind::NotFound),
            "(c) 경로가 살아 있으면 그 NotFound는 **이 항목의 부재가 아니다**",
        )
        .await;
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert_eq!(vanished.get(), 1, "(c) 계수 불변");

        // ── (d) 부재 경로 + **NotFound 이외** → raw. **B7의 유일한 갈림길.** ────────────────
        //    경로가 정말로 비어 있어도(= (a)와 같은 항목) 확인 팔에 **닿지 않는다**.
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::IsADirectory,
            ErrorKind::StorageFull,
            ErrorKind::Other,
        ] {
            let raw = std::io::Error::new(kind, "w1-b7-marker");
            let want = raw.to_string();
            let e = raw_err(
                victim,
                raw,
                "(d) B7: `NotFound` 이외의 io 에러는 **무가공 전파**한다",
            )
            .await;
            assert_eq!(e.kind(), kind, "(d) kind 무변조 — kind={kind:?}");
            assert_eq!(e.to_string(), want, "(d) 메시지 무변조 — kind={kind:?}");
        }
        assert_eq!(
            vanished.get(),
            1,
            "(d) B7 팔은 부재 확인을 **하지 않는다** ⇒ 계수는 (a)의 1에서 그대로다"
        );
    }

    /// **W2 — 소멸한 항목에서 FS 메서드 다섯이 각각 `Gone`을 보고한다**(그리고 각각 계수를 올린다).
    ///
    /// ⚠⚠ **`file_type()`은 이 목록에 없다** — tokio가 readdir 청크를 채우는 시점에 `d_type`을
    /// **캐시**하므로 **소멸한 항목에도 `Ok`** 가 난다(실행 확정). 숨기지 않고 **`Present`를 정직하게
    /// 특성화**한다 — `Gone` 팔은 이 코드에서 **도달 불가**다(그래서 M-FT가 Class B다).
    #[tokio::test]
    async fn every_fs_method_reports_gone_after_the_entry_vanishes() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path();
        for n in ["e-meta", "e-read", "e-remove", "e-rename", "e-durable"] {
            std::fs::write(dir.join(n), b"doomed").unwrap();
        }

        let vanished = Vanished::new();
        let entries = Entry::snapshot(dir, &vanished).await.unwrap();
        assert_eq!(entries.len(), 5, "무대 자기검증: 다섯 항목을 스냅샷했다");

        // 스냅샷 **이후** 전부 소멸시킨다.
        for e in &entries {
            std::fs::remove_file(dir.join(e.name())).unwrap();
        }
        let by = |n: &str| entries.iter().find(|e| e.name() == n).unwrap();

        let into = dir.join("quarantine");
        std::fs::create_dir(&into).unwrap(); // 스냅샷 **이후**에 만든다(스냅샷에 안 들어간다)

        assert!(matches!(
            by("e-meta").metadata().await.unwrap(),
            Seen::Gone(_)
        ));
        assert!(matches!(by("e-read").read().await.unwrap(), Seen::Gone(_)));
        assert!(matches!(
            by("e-remove").remove().await.unwrap(),
            Seen::Gone(_)
        ));
        assert!(matches!(
            by("e-rename").rename_into(&into).await.unwrap(),
            Seen::Gone(_)
        ));
        assert!(matches!(
            by("e-durable")
                .rename_durable_to(&dir.join("restored"), dir)
                .await
                .unwrap(),
            Seen::Gone(_)
        ));

        assert_eq!(
            vanished.get(),
            5,
            "다섯 메서드가 **각각** 부재를 확인하고 **각각** 같은 하나의 집계를 올렸다"
        );

        // ★ 정직한 특성화 — `file_type()`은 **캐시 히트**라 소멸해도 `Ok`다(syscall 0 ⇒ 계수 0).
        assert!(
            matches!(by("e-meta").file_type().await.unwrap(), Seen::Present(_)),
            "`file_type()`은 d_type 캐시 때문에 소멸한 항목에도 `Ok`를 낸다 — `Gone` 팔은 도달 불가다"
        );
        assert_eq!(
            vanished.get(),
            5,
            "`file_type()`은 FS를 만지지 않았다 ⇒ 집계가 오르지 않는다"
        );
    }
}
