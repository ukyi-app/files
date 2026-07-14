//! **부재의 증거 — 경로 기반 · fd 0.**
//!
//! `NotFound`가 났을 때 *"그 항목이 정말로 사라졌는가"*를 판정하는 **유일한 정의**가 여기 있다.
//! 판정은 `symlink_metadata(<그 항목의 경로>)` **1회**이고(**no-follow** — P-1 봉인: 댕글링 심링크는
//! 항목이 **있으므로** `Ok` ⇒ skip 금지 ⇒ 오늘의 `Err`를 바이트 보존), 그 syscall이 `NotFound`를 낼
//! 때에만 `Absent`가 주조되고 **같은 행위로** 패스 집계가 오른다.
//!
//! ⚠⚠ **위치가 봉인이다.** 이 모듈은 `reconcile`의 **자식**이다 ⇒ `pub(super)` =
//! `pub(in crate::store::reconcile)` = **reconcile 서브트리 전용**(pins·atomic 제외).
//! `atomic.rs`에 두면 `pins`가 대체 집계를 지을 수 있다(r15/P-27의 컴파일 증거).

use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// **"소스 항목이 부재함"의 증거.** 필드 private ⇒ **이 모듈 밖에서 생성 불가**(`E0423`)
/// ⇒ 부모도 `pins`도 `Seen::Gone`·`Renamed::SourceGone`·`GraveOutcome::SourceGone`을 **합성할 수 없다**.
pub(crate) struct Absent(());

/// **소멸 계수기.** ⚠ **derive 0개** — `Default`/`Clone`/`Copy`/`Debug` 전부 없다(복제본이 곧
/// 대체 집계다). `Arc`가 남는 **유일한** 이유: private `share()`가 `spawn_blocking`의 `'static`
/// 클로저로 **같은** 집계를 나른다.
pub(crate) struct Vanished(Arc<AtomicUsize>);

impl Vanished {
    /// ⚠ **크레이트 전체 호출부는 `run_once_at` 하나뿐이다**(reconcile 서브트리 전용).
    pub(super) fn new() -> Self {
        Vanished(Arc::new(AtomicUsize::new(0)))
    }

    /// 루프-후 컨테이너 가드만 읽는다(`pins`에서 부르면 `E0624`).
    pub(super) fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }

    /// **모듈 private.** 호출부는 `entry_is_absent{,_blocking}` 둘뿐이다.
    fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    /// **모듈 private.** Arc 공유 = **같은 집계**(클론이 아니다 — 대체 집계를 만들 수 없다).
    fn share(&self) -> Vanished {
        Vanished(Arc::clone(&self.0))
    }

    /// ⚠ **테스트 다리(B-TESTBRIDGE).** `pins::tests`는 reconcile 서브트리 **밖**이라 `&Vanished`를
    /// 만들 방법이 없다 — `begin`/`grave` 호출부 9개가 그것을 요구한다. `#[cfg(test)]` ⇒ 릴리스
    /// 빌드에 **존재하지 않는다**.
    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Vanished(Arc::new(AtomicUsize::new(0)))
    }
}

/// rename의 **소스 부재**만 분리(P-2). `SourceGone`은 `Absent`를 **요구** ⇒ 위조 불가.
#[must_use]
pub(crate) enum Renamed {
    Done,
    SourceGone(Absent),
}

/// **부재 판정의 유일한 정의(async 채널).** `Absent(())` 리터럴도 `bump()`도 여기와
/// `entry_is_absent_blocking`에 **1회씩만** 등장한다.
pub(super) async fn entry_is_absent(tally: &Vanished, path: &Path) -> Option<Absent> {
    match tokio::fs::symlink_metadata(path).await {
        // ⚠ **no-follow** — 댕글링 심링크는 `Ok`다(항목이 **있다**) ⇒ P-1 봉인.
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tally.bump();
            Some(Absent(()))
        }
        // Ok(_) = 항목이 있다 · 그 외 Err = 확인 불가 ⇒ 보수적(원본 에러 전파).
        _ => None,
    }
}

/// **부재 판정의 유일한 정의(blocking 채널).** `rename_checked_blocking` 전용.
fn entry_is_absent_blocking(tally: &Vanished, path: &Path) -> Option<Absent> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tally.bump();
            Some(Absent(()))
        }
        _ => None,
    }
}

/// ⚠⚠ **`SourceGone`은 `std::fs::rename`의 `Err` 팔에서만 태어난다** ⇒ rename `Ok` 이후의 fsync
/// 실패는 **무가공 `io::Error`**다. `atomic::rename_durable`(rename+fsync **융합**)에 부재 확인을
/// 붙이면 rename 성공 후의 fsync ENOENT가 `SourceGone`으로 **위조**된다(M6 부활) ⇒ **확인은 여기에만.**
fn rename_checked_blocking(from: &Path, to: &Path, tally: &Vanished) -> std::io::Result<Renamed> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(Renamed::Done),
        // 목적지 부재도 `NotFound`다 → **소스를 확인해서** 걸러낸다(W5b · W9b).
        Err(e) if e.kind() == ErrorKind::NotFound => match entry_is_absent_blocking(tally, from) {
            Some(a) => Ok(Renamed::SourceGone(a)),
            None => Err(e), // 목적지발 NotFound · 댕글링 소스 → **원본 그대로**
        },
        Err(e) => Err(e), // EACCES · EXDEV · ENOTDIR · EIO … 무가공(B7)
    }
}

/// 격리 rename(부모 fsync는 **호출부의 raw `?`**).
pub(super) async fn rename_source_checked(
    from: &Path,
    to: &Path,
    tally: &Vanished,
) -> std::io::Result<Renamed> {
    let (f, t, share) = (from.to_owned(), to.to_owned(), tally.share());
    tokio::task::spawn_blocking(move || rename_checked_blocking(&f, &t, &share))
        .await
        .expect("join")
}

/// 무덤 rename — rename + parent fsync를 **한 무취소 클로저**에 유지한다(M6 봉인).
/// rename이 `Ok`를 낸 **이후의** fsync 실패는 **무가공**으로 전파된다(P-2).
pub(crate) async fn rename_durable_source_checked(
    from: &Path,
    to: &Path,
    fsync_parent: &Path,
    tally: &Vanished,
) -> std::io::Result<Renamed> {
    let (f, t, p, share) = (
        from.to_owned(),
        to.to_owned(),
        fsync_parent.to_owned(),
        tally.share(),
    );
    tokio::task::spawn_blocking(move || match rename_checked_blocking(&f, &t, &share)? {
        Renamed::Done => {
            crate::store::atomic::fsync_dir_blocking(&p)?; // ← rename Ok 이후 ⇒ 실패는 **무가공**
            Ok(Renamed::Done)
        }
        gone => Ok(gone),
    })
    .await
    .expect("join")
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  **W5(a~e′)** — rename의 `NotFound` 분류. **소스에만** 확인이 걸리고, **rename `Ok` 이후의
//  fsync 실패는 절대 `SourceGone`이 될 수 없다**(M6 봉인 · P-2).
//
//  이 다섯이 핀하는 것: **부재 판정과 계수는 같은 행위다**(a) · **목적지발 NotFound는 무가공**(b) ·
//  **rename은 성공했고 fsync가 실패한 세계는 무가공**(c) · **댕글링 소스는 항목이 있다**(d) ·
//  **EACCES는 부재가 아니다**(e′ — M-B7 킬).
// ══════════════════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    /// **W5a** — 소스 부재 → `SourceGone` ∧ **집계 +1**.
    /// 부재 판정(`symlink_metadata` ENOENT)과 `bump()`는 `entry_is_absent_blocking` **한 몸**이다
    /// ⇒ 계수를 지우는 뮤턴트(M-NOBUMP-BLOCKING)는 여기서 `get() == 0`으로 RED가 된다.
    #[tokio::test]
    async fn rename_with_absent_source_is_source_gone_and_counted() {
        let d = tempfile::tempdir().unwrap();
        let from = d.path().join("never-existed"); // **만들지 않는다**
        let to = d.path().join("dest");
        let tally = Vanished::new();

        match rename_source_checked(&from, &to, &tally).await.unwrap() {
            Renamed::SourceGone(_) => {}
            Renamed::Done => panic!("소스가 없는데 rename이 `Done`이라고 보고했다"),
        }
        assert_eq!(
            tally.get(),
            1,
            "부재 판정과 계수는 **같은 행위**다 — `Absent` 주조 = `bump()`"
        );
        assert!(!to.exists(), "아무것도 옮겨지지 않았다");
    }

    /// **W5b** — 소스는 **있고** 목적지 부모가 없다 → **raw `Err(NotFound)`** ∧ 소스 잔존 ∧ **계수 0**.
    /// 목적지발 `NotFound`를 부재로 오독하는 뮤턴트(M7 — 확인을 목적지에 건다)가 여기서 죽는다.
    #[tokio::test]
    async fn rename_with_missing_destination_propagates_raw_notfound() {
        let d = tempfile::tempdir().unwrap();
        let from = d.path().join("src");
        std::fs::write(&from, b"alive").unwrap();
        let to = d.path().join("no-such-dir").join("dest"); // **부모가 없다**
        let tally = Vanished::new();

        // `Renamed`는 `Debug`를 유도하지 않는다(프로덕션 타입 diff 0) → **`match`로 언랩**한다.
        let e = match rename_source_checked(&from, &to, &tally).await {
            Ok(_) => {
                panic!("목적지 부모가 없으면 rename은 ENOENT다 — 그것은 **소스 부재가 아니다**")
            }
            Err(e) => e,
        };
        assert_eq!(e.kind(), ErrorKind::NotFound, "kind 무변조. err={e:?}");
        assert!(from.exists(), "소스는 **그대로** 남아 있다");
        assert_eq!(tally.get(), 0, "목적지발 NotFound는 **계수하지 않는다**");
    }

    /// **W5c** — rename은 **실제로 일어났고** 그 뒤의 fsync가 실패한다 → **raw `Err`**(`SourceGone` **아님**).
    ///
    /// ⚠⚠ **M6의 봉인이 이것이다.** `atomic::rename_durable`(rename+fsync **융합**)에 소스 확인을 붙이면
    /// **rename이 성공했으므로 소스는 당연히 부재**이고, fsync의 ENOENT가 **`SourceGone`으로 위조**된다
    /// ⇒ `settle()`이 스킵된다. 확인은 **`std::fs::rename`의 `Err` 팔 전용**이라야 한다.
    #[tokio::test]
    async fn rename_ok_then_fsync_failure_propagates_raw() {
        let d = tempfile::tempdir().unwrap();
        let from = d.path().join("src");
        std::fs::write(&from, b"payload").unwrap();
        let to = d.path().join("dst");
        let bad_parent = d.path().join("no-such-parent"); // fsync 대상이 **없다** ⇒ `File::open` ENOENT
        let tally = Vanished::new();

        let e = match rename_durable_source_checked(&from, &to, &bad_parent, &tally).await {
            Ok(_) => panic!(
                "rename `Ok` 이후의 fsync 실패는 **무가공 io::Error**여야 한다(P-2) — \
                 `Renamed`를 돌려주면 `SourceGone` 위조의 문이 열린다(M6)"
            ),
            Err(e) => e,
        };

        assert_eq!(e.kind(), ErrorKind::NotFound, "무가공 전파. err={e:?}");
        // ★ rename은 **정말로 일어났다** — 그래서 이 세계가 위험한 것이다.
        assert!(to.exists(), "rename은 성공했다(목적지가 있다)");
        assert!(!from.exists(), "rename은 성공했다(소스가 없다)");
        // ★ 그런데도 `SourceGone`이 **아니다** ∧ 계수도 오르지 않는다.
        assert_eq!(
            tally.get(),
            0,
            "우리가 옮겼기 때문에 소스가 없는 것이다 — 그것은 **소멸이 아니다**"
        );
    }

    /// **W5d** — 소스가 **댕글링 심링크**: `rename`은 심링크를 **추종하지 않는다** ⇒ `Done`.
    /// 항목은 **있다** ⇒ 계수 0. (P-1의 rename 쪽 얼굴.)
    #[cfg(unix)]
    #[tokio::test]
    async fn rename_with_dangling_source_symlink_is_done() {
        let d = tempfile::tempdir().unwrap();
        let from = d.path().join("dangling");
        std::os::unix::fs::symlink(d.path().join("nowhere"), &from).unwrap();
        let to = d.path().join("moved");
        let tally = Vanished::new();

        match rename_source_checked(&from, &to, &tally).await.unwrap() {
            Renamed::Done => {}
            Renamed::SourceGone(_) => {
                panic!("댕글링 심링크는 **항목이 있다** — rename은 링크 자체를 옮긴다")
            }
        }
        assert!(
            to.symlink_metadata().is_ok(),
            "링크가 목적지로 옮겨졌다(타깃이 없어도)"
        );
        assert_eq!(tally.get(), 0, "항목이 있었으므로 소멸이 아니다");
    }

    /// **W5e′** — 확인 syscall 자체가 **EACCES**면 → **`None`**(= 부재가 **아니다**) ⇒ 원본 에러 전파.
    /// **M-B7 킬**: *"`NotFound` 이외도 skip"* 뮤턴트는 EACCES를 부재로 읽어 항목을 조용히 건너뛴다.
    ///
    /// ⚠ **root면 권한 검사가 우회된다** ⇒ 전제가 사라진다 ⇒ **사유를 출력하고 skip**(조용한 GREEN 금지).
    #[cfg(unix)]
    #[tokio::test]
    async fn absence_probe_eacces_is_not_absence() {
        use std::os::unix::fs::PermissionsExt;

        let d = tempfile::tempdir().unwrap();
        let locked = d.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let child = locked.join("child");
        std::fs::write(&child, b"x").unwrap();
        // no-search(0o600) ⇒ `lstat(locked/child)` = **EACCES**(부재가 아니다).
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).unwrap();

        let probe = std::fs::symlink_metadata(&child).err().map(|e| e.kind());
        if probe != Some(ErrorKind::PermissionDenied) {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
            eprintln!(
                "SKIP absence_probe_eacces_is_not_absence: 0o600 디렉터리에서 EACCES를 만들 수 없다 \
                 (root로 도는 중인가?) — probe={probe:?}. **조용한 GREEN이 아니라 명시적 skip이다.**"
            );
            return;
        }

        let tally = Vanished::new();
        let got = entry_is_absent(&tally, &child).await;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(
            got.is_none(),
            "EACCES는 **부재가 아니다** — 확인 불가는 보수적으로 `None`(원본 에러 전파)이어야 한다"
        );
        assert_eq!(tally.get(), 0, "확인하지 못한 것은 계수하지 않는다");
    }
}
