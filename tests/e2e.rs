//! 실제 2 리스너 부트스트랩 E2E(reqwest) — 표면 분리 + 대용량 Range.

use files::config::Config;
use files::http;
use serde_json::json;

mod common;
use common::{f14_store, hex_sha};

struct Harness {
    internal: String,
    public: String,
    _d: tempfile::TempDir,
}

async fn start() -> Harness {
    let d = tempfile::tempdir().unwrap();
    let keys_path = d.path().join("keys.json");
    let keys = format!(
        r#"[{{"id":"w","sha256":"{}","service":"page","writeBuckets":["downloads","secret"],"readBuckets":["downloads","secret"]}},{{"id":"a","sha256":"{}","service":"ops","admin":true}}]"#,
        hex_sha(b"writer"),
        hex_sha(b"admin")
    );
    std::fs::write(&keys_path, keys).unwrap();
    let dd = d.path().join("data");
    let cfg = Config::from_env(|k| match k {
        "FILES_DATA_DIR" => Some(dd.to_string_lossy().to_string()),
        "FILES_KEYS_PATH" => Some(keys_path.to_string_lossy().to_string()),
        "FILES_MIN_FREE_BYTES" => Some("0".into()),
        _ => None,
    })
    .unwrap();
    let state = http::build_state(cfg).unwrap();
    let internal = http::internal::router(state.clone());
    let public = http::public::router(state);

    let il = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let pl = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ia = il.local_addr().unwrap();
    let pa = pl.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(il, internal).await.unwrap();
    });
    tokio::spawn(async move {
        axum::serve(pl, public).await.unwrap();
    });
    Harness {
        internal: format!("http://{ia}"),
        public: format!("http://{pa}"),
        _d: d,
    }
}

// ── M13.4: 공개 리스너 /api 거부 + internal 버킷 비공개 ──────────────────────

#[tokio::test]
async fn public_listener_isolates_api_and_internal_buckets() {
    let h = start().await;
    let c = reqwest::Client::new();

    // admin: public + internal 버킷
    let r = c
        .put(format!("{}/api/buckets/downloads", h.internal))
        .header("authorization", "Bearer admin")
        .json(&json!({"visibility":"public"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = c
        .put(format!("{}/api/buckets/secret", h.internal))
        .header("authorization", "Bearer admin")
        .json(&json!({"visibility":"internal"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);

    // writer: 객체
    let r = c
        .put(format!(
            "{}/api/files/downloads/object?key=pub.txt",
            h.internal
        ))
        .header("authorization", "Bearer writer")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = c
        .put(format!(
            "{}/api/files/secret/object?key=hid.txt",
            h.internal
        ))
        .header("authorization", "Bearer writer")
        .body("classified")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);

    // 공개: public 버킷 다운로드 200
    let r = c
        .get(format!("{}/downloads/pub.txt", h.public))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "hello");

    // 공개: /api GET/PUT → 404(표면 분리)
    let r = c
        .get(format!(
            "{}/api/files/downloads/object?key=pub.txt",
            h.public
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let r = c
        .put(format!("{}/api/files/downloads/object?key=x.txt", h.public))
        .body("nope")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // 공개: internal 버킷 다운로드 → 404(존재 비노출)
    let r = c
        .get(format!("{}/secret/hid.txt", h.public))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // internal 리스너: 정상 다운로드
    let r = c
        .get(format!(
            "{}/api/files/secret/object?key=hid.txt",
            h.internal
        ))
        .header("authorization", "Bearer writer")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "classified");
}

// ── M13.5: 대용량 스트리밍 put + 부분 Range 정확성 ───────────────────────────

#[tokio::test]
async fn large_object_streaming_put_and_range_download() {
    let h = start().await;
    let c = reqwest::Client::new();
    c.put(format!("{}/api/buckets/downloads", h.internal))
        .header("authorization", "Bearer admin")
        .json(&json!({"visibility":"public"}))
        .send()
        .await
        .unwrap();

    let size = 8 * 1024 * 1024usize;
    let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let r = c
        .put(format!(
            "{}/api/files/downloads/object?key=big.bin",
            h.internal
        ))
        .header("authorization", "Bearer writer")
        .body(data.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);

    // 부분 Range
    let (start, end) = (1_000_000usize, 1_000_099usize);
    let r = c
        .get(format!("{}/downloads/big.bin", h.public))
        .header("range", format!("bytes={start}-{end}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 206);
    let body = r.bytes().await.unwrap();
    assert_eq!(body.len(), end - start + 1);
    assert_eq!(&body[..], &data[start..=end]);

    // 전체 다운로드 무결성
    let r = c
        .get(format!("{}/downloads/big.bin", h.public))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.bytes().await.unwrap().len(), size);
}

// ══════════════════════════════════════════════════════════════════════════════════════════
//  **F-14 봉인 증인 — 통합 바이너리(`tests/`)에서.**
//
//  ⚠⚠ **`tests/`는 `cfg(test)` *없이* lib를 링크한다** ⇒ 여기 있는 증인은 **모든 조건부 뮤턴트의
//  프로덕션 팔을 탄다**(`if cfg!(test) {옳음} else {legacy}` 류가 여기서 죽는다). 그 대가로 **훅을
//  심을 수 없다**(`with_hooks`는 `#[cfg(test)]`) ⇒ "스냅샷 이후 소멸"은 **온디스크 관측치로 랑데부**
//  한다(W10c) 또는 **훅 없이 성립하는 무대**로 만든다(W3/W4/W7/W9).
//
//  * **W4**  `dangling_temp_symlink_keeps_lstat_semantics`         — `de.metadata()`는 **lstat**이다
//  * **W7**  `blob_symlink_to_directory_propagates_isadirectory`   — `IsADirectory ≠ NotFound`(B7)
//  * **W9a** `corrupt_dir_as_regular_file_propagates_enotdir`      — 목적지 ENOTDIR 무가공
//  * **W9b** `corrupt_dir_as_dangling_symlink_propagates_raw_notfound` — **목적지발** NotFound 무가공
//  * **W10c** `symlinked_objects_dir_with_a_vanished_entry_completes`  — 가드는 **follow**여야 한다
//  * **W10c′** `symlinked_objects_dir_without_vanishing_is_unchanged`  — 소멸 0이면 가드는 **안 돈다**
//  * **W17** `non_utf8_temp_name_is_stat_and_unlinked_by_raw_bytes`    — **원시 바이트**로 커널에 넘긴다
// ══════════════════════════════════════════════════════════════════════════════════════════

use files::layout::Layout;
use files::store::{reconcile, Store};
use std::time::Duration;

const F14_SETTLE: Duration = Duration::from_secs(30);
const F14_KEEP: Duration = Duration::from_secs(3600); // 무엇도 만료시키지 않는 grace
const F14_EXPIRE: Duration = Duration::ZERO; // `age.as_secs() > 0` ⇒ **1초 이상**이면 만료

/// 벽시계로 temp를 늙힌다. `run_once`는 `SystemTime::now()`를 쓰고(`run_once_at`은 crate-private),
/// **심링크의 mtime은 std로 백데이트할 수 없다**(lutimes 부재) ⇒ 여기서는 **실제로 기다린다**.
/// `age.as_secs() > 0`이 되려면 1초를 넘겨야 한다.
async fn age_past(grace: Duration) {
    tokio::time::sleep(grace + Duration::from_millis(1100)).await;
}

/// **W4 — 댕글링 *temp* 심링크는 `de.metadata()`의 lstat 의미론을 그대로 탄다.**
///
/// `metadata()`가 **심링크를 추종하면**(뮤턴트 **M3′**) 댕글링 링크는 ENOENT를 내고, 그것은
/// `symlink_metadata`(= 항목 **있음**) 때문에 `Gone`이 되지 못해 **패스가 `Err`로 죽는다** ⇒ RED.
/// 오늘의 코드(= 픽스)는 **lstat = `Ok(symlink)`** 를 보고 나이를 재고, 늙었으면 **링크를 unlink**한다.
#[cfg(unix)]
#[tokio::test]
async fn dangling_temp_symlink_keeps_lstat_semantics() {
    // ── (b) recent → **보존** ∧ `temps_deleted == 0` ────────────────────────────────────────
    {
        let (_d, s, l) = f14_store();
        let link = l.temp_blob_path("w4-recent");
        std::os::unix::fs::symlink(l.objects_dir().join("no-such-target"), &link).unwrap();

        let stats = reconcile::run_once(&s, F14_KEEP, F14_SETTLE)
            .await
            .expect("PASS ABORTED — 댕글링 temp 심링크는 **항목이 있다**(lstat Ok)");
        assert_eq!(stats.temps_deleted, 0, "grace 이내의 temp는 보존된다");
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "링크는 그대로 남는다"
        );
    }

    // ── (a) old → **삭제** ∧ `temps_deleted == 1` (M3′의 킬 지점) ──────────────────────────
    {
        let (_d, s, l) = f14_store();
        let link = l.temp_blob_path("w4-old");
        std::os::unix::fs::symlink(l.objects_dir().join("no-such-target"), &link).unwrap();
        age_past(F14_EXPIRE).await; // 심링크의 mtime은 백데이트할 수 없다 ⇒ 실제로 늙힌다

        let stats = reconcile::run_once(&s, F14_EXPIRE, F14_SETTLE)
            .await
            .expect("PASS ABORTED — lstat이 Ok이므로 이 패스는 완주한다");
        assert_eq!(
            stats.temps_deleted, 1,
            "**lstat 의미론**: 댕글링 링크도 `de.metadata()`에서 `Ok`이므로 나이를 재고 삭제한다 \
             — `metadata`(follow)로 바꾸면(M3′) ENOENT가 나서 이 값이 0이 되거나 패스가 죽는다"
        );
        assert!(
            std::fs::symlink_metadata(&link).is_err(),
            "늙은 temp 링크는 unlink된다"
        );
    }
}

/// **W7 — blob이 *디렉터리를 가리키는* 심링크: `read()`는 `IsADirectory`다. `NotFound`가 아니다.**
/// ⇒ `seen`의 **마지막 팔**(B7)로 떨어져 **무가공 전파**된다. 항목은 그대로 남는다.
#[cfg(unix)]
#[tokio::test]
async fn blob_symlink_to_directory_propagates_isadirectory() {
    let (_d, s, l) = f14_store();
    let target = l.objects_dir().parent().unwrap().join("a-real-dir");
    std::fs::create_dir(&target).unwrap();

    let sha = "b".repeat(64); // Blob으로 분류되는 이름
    let link = l.objects_dir().join(&sha);
    std::os::unix::fs::symlink(&target, &link).unwrap(); // ⚠ 타깃은 **절대 경로**

    let err = reconcile::run_once(&s, F14_KEEP, F14_SETTLE)
        .await
        .expect_err("`IsADirectory`는 흡수 대상이 아니다 — 오늘처럼 시끄럽게 죽어야 한다");
    assert_ne!(
        err.kind(),
        std::io::ErrorKind::NotFound,
        "`NotFound`가 아니다 ⇒ 부재 확인 팔에 **닿지도 않는다**(B7). err={err:?}"
    );
    assert!(
        std::fs::symlink_metadata(&link).is_ok(),
        "아무것도 지우지 않았다"
    );
    assert!(
        !std::fs::exists(l.gc_pending_path()).unwrap(),
        "중단된 패스는 원장을 발행하지 않는다"
    );
}

/// **W9a — `.corrupt`가 *일반 파일*: 격리 rename이 `ENOTDIR`을 낸다.** `NotFound`가 아니다 ⇒ 무가공.
/// (`mkdir_p_durable`는 `create_dir`의 `AlreadyExists`를 삼키므로 **통과한다** — 그래서 rename까지 간다.)
#[cfg(unix)]
#[tokio::test]
async fn corrupt_dir_as_regular_file_propagates_enotdir() {
    let (_d, s, l) = f14_store();
    std::fs::write(l.corrupt_dir(), b"i am not a directory").unwrap();

    // 비트로트 blob — 이름의 sha와 내용이 어긋난다 ⇒ 격리 분기로 간다
    let sha = hex_sha(b"w9a-name");
    std::fs::write(l.blob_path(&sha), b"w9a-different-bytes").unwrap();

    let err = reconcile::run_once(&s, F14_KEEP, F14_SETTLE)
        .await
        .expect_err("목적지가 디렉터리가 아니면 rename은 ENOTDIR이다 — 무가공 전파");
    assert_ne!(
        err.kind(),
        std::io::ErrorKind::NotFound,
        "ENOTDIR ≠ NotFound ⇒ 부재 확인에 닿지 않는다(B7). err={err:?}"
    );
    assert!(
        std::fs::exists(l.blob_path(&sha)).unwrap(),
        "격리에 실패했으므로 정본은 **보존**된다"
    );
}

/// **W9b — `.corrupt`가 *댕글링 심링크*: rename이 `NotFound`를 낸다. 그러나 그것은 *목적지*발이다.**
///
/// ⇒ `rename_checked_blocking`이 **소스를 확인**하고 소스는 **존재**하므로 → **원본 에러 그대로**.
/// 확인을 목적지에 거는 뮤턴트(**M7**)는 이것을 `SourceGone`으로 위조해 skip하고 패스를 `Ok`로 만든다 ⇒ RED.
#[cfg(unix)]
#[tokio::test]
async fn corrupt_dir_as_dangling_symlink_propagates_raw_notfound() {
    let (_d, s, l) = f14_store();
    // `.corrupt` → 존재하지 않는 곳. `try_exists` = Ok(false)이고 `create_dir`는 AlreadyExists를 낸다
    // (실측) ⇒ `mkdir_p_durable`가 **통과**하고 rename이 **목적지발 ENOENT**를 맞는다.
    std::os::unix::fs::symlink(
        l.objects_dir().parent().unwrap().join("nowhere-at-all"),
        l.corrupt_dir(),
    )
    .unwrap();

    let sha = hex_sha(b"w9b-name");
    std::fs::write(l.blob_path(&sha), b"w9b-different-bytes").unwrap(); // 비트로트

    let err = reconcile::run_once(&s, F14_KEEP, F14_SETTLE)
        .await
        .expect_err("목적지발 NotFound는 **소스 부재가 아니다** — 오늘처럼 Err여야 한다");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::NotFound,
        "kind는 NotFound다(그러나 **소스는 살아 있다**). err={err:?}"
    );
    assert!(
        std::fs::exists(l.blob_path(&sha)).unwrap(),
        "★ **소스는 그대로 있다** — 그래서 이것은 `SourceGone`이 될 수 없다(M7 킬)"
    );
    assert!(
        !std::fs::exists(l.gc_pending_path()).unwrap(),
        "중단된 패스는 원장을 발행하지 않는다"
    );
}

/// **W10c — `.objects`가 *심링크→dir*인 정상 배포에서, 항목 하나가 진짜로 소멸해도 패스가 완주한다.**
///
/// ⚠⚠ **루프-후 가드는 `metadata`(follow)여야 한다.** `symlink_metadata`(no-follow)로 바꾸면
/// (뮤턴트 **M-GUARD-LSTAT**) `.objects`의 lstat은 **심링크**이고 `is_dir() == false`이므로 가드가
/// **`Err(NotADirectory)`** 를 낸다 ⇒ **정상 배포가 죽는다** ⇒ 여기서 RED.
///
/// ⚠ **소멸이 0이면 가드는 아예 돌지 않는다**(그러면 아무 뮤턴트도 안 죽는다 — 그것이 W10c′다) ⇒
/// 이 증인은 **소멸을 실제로 밟아야** 한다. 훅이 없으므로(`tests/`) **온디스크 관측치로 랑데부**한다:
/// 비트로트 카나리아가 `.corrupt/<sha>`로 나타나면 **엔트리 루프가 이미 돌고 있다**는 뜻이다.
/// 그때 아직 살아 있는 victim을 지운다. **회계는 사후 디스크 상태로** 한다(우리 unlink의 반환값이
/// 아니라 — 우리의 unlink와 패스의 격리 rename은 **둘 다 성공할 수 있다**).
#[cfg(unix)]
#[tokio::test]
async fn symlinked_objects_dir_with_a_vanished_entry_completes() {
    const ROUNDS: usize = 6;
    const CANARY: usize = 4;
    const VICTIM: usize = 12;
    const MIN_STEPS: usize = 1;

    let mut stepped_total = 0usize;

    for round in 0..ROUNDS {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().to_path_buf();

        // ★ `.objects`를 **심링크→dir**로 만든다(정상 배포의 모양).
        let real = root.join("real-objects");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, root.join(".objects")).unwrap();

        let l = Layout::new(root.clone());
        let s = Store::new(root.clone());
        assert!(
            std::fs::metadata(l.objects_dir()).unwrap().is_dir(),
            "무대 자기검증: follow하면 디렉터리다"
        );
        assert!(
            !std::fs::symlink_metadata(l.objects_dir()).unwrap().is_dir(),
            "무대 자기검증: no-follow면 **디렉터리가 아니다** — M-GUARD-LSTAT은 여기서 죽는다"
        );

        // 카나리아 = 비트로트(격리되면서 `.corrupt/<sha>`를 만든다 ⇒ **루프 진입 신호**)
        let mut canaries = Vec::new();
        for i in 0..CANARY {
            let sha = hex_sha(format!("w10c-canary-{round}-{i}").as_bytes());
            std::fs::write(l.blob_path(&sha), format!("rot-{i}")).unwrap();
            canaries.push(sha);
        }
        // victim도 **비트로트**로 심는다 ⇒ 사후 디스크에서 `.corrupt/<sha>`의 유무가
        // "그 항목이 처리됐는가 / 소멸했는가"를 **날조 없이** 가른다.
        let mut victims = Vec::new();
        for i in 0..VICTIM {
            let sha = hex_sha(format!("w10c-victim-{round}-{i}").as_bytes());
            std::fs::write(l.blob_path(&sha), format!("rot-v-{i}")).unwrap();
            victims.push(sha);
        }

        let s2 = s.clone();
        let pass =
            tokio::spawn(async move { reconcile::run_once(&s2, F14_KEEP, F14_SETTLE).await });

        // 랑데부: `.corrupt`에 무엇이든 나타나면 **엔트리 루프가 돌고 있다**.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let entered = std::fs::read_dir(l.corrupt_dir())
                .map(|rd| rd.count() > 0)
                .unwrap_or(false);
            if entered || std::time::Instant::now() > deadline {
                break;
            }
            tokio::task::yield_now().await;
        }
        // 아직 디스크에 있는 victim을 지운다(= 동시 rename이 하는 일).
        for v in &victims {
            let _ = std::fs::remove_file(l.blob_path(v));
        }

        let stats = pass
            .await
            .expect("패스 태스크는 패닉하지 않는다")
            .expect("PASS ABORTED — 심링크 `.objects` + 소멸 항목에서 패스는 **완주**해야 한다");

        // 사후 회계: `.corrupt/<sha>`가 **없는** victim = 패스가 그것을 격리하지 못했다 = **소멸했다**.
        let escaped = victims
            .iter()
            .filter(|v| !std::fs::exists(l.corrupt_dir().join(v)).unwrap_or(false))
            .count();
        stepped_total += escaped;

        assert_eq!(
            stats.quarantined,
            CANARY + (VICTIM - escaped),
            "격리 수 = 카나리아 + (소멸하지 않은 victim). 라운드 {round}"
        );
        for c in &canaries {
            assert!(
                std::fs::exists(l.corrupt_dir().join(c)).unwrap(),
                "카나리아는 항상 격리된다(루프 진입 신호)"
            );
        }
    }

    // ★ 자기검증 — **창을 실제로 밟았는가.** 밟지 못했다면 이 증인은 아무것도 증명하지 않는다.
    assert!(
        stepped_total >= MIN_STEPS,
        "소멸을 한 번도 만들지 못했다(stepped={stepped_total}) ⇒ **가드가 한 번도 돌지 않았다** ⇒ \
         이 증인은 M-GUARD-LSTAT을 죽이지 못한다. 랑데부가 깨졌다는 뜻이다 — 조용히 넘어가지 않는다"
    );
}

/// **W10c′ — 심링크 `.objects` ∧ 소멸 0 → 오늘과 같이 `Ok`.**
///
/// ⚠ **정직: 이 증인은 어떤 뮤턴트도 죽이지 않는다.** 소멸이 0이므로 `vanished == 0`이고
/// **가드는 아예 발화하지 않는다**(P11). 그것을 핀할 뿐이다 — 숨기지 않고 그렇게 적는다.
#[cfg(unix)]
#[tokio::test]
async fn symlinked_objects_dir_without_vanishing_is_unchanged() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let real = root.join("real-objects");
    std::fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, root.join(".objects")).unwrap();

    let l = Layout::new(root.clone());
    let s = Store::new(root.clone());

    let mut shas = Vec::new();
    for i in 0..3 {
        let content = format!("w10c-prime-{i}");
        let sha = hex_sha(content.as_bytes());
        std::fs::write(l.blob_path(&sha), &content).unwrap(); // 내용 **정합** ⇒ 격리 없음
        shas.push(sha);
    }

    let stats = reconcile::run_once(&s, F14_KEEP, F14_SETTLE)
        .await
        .expect("PASS ABORTED — 소멸이 0인 패스는 오늘도 완주한다");

    assert_eq!(
        stats.gc_pending, 3,
        "셋 다 **최초 관측** tombstone을 얻는다"
    );
    assert_eq!(stats.quarantined, 0);
    assert_eq!(stats.gc_deleted, 0);
    for sha in &shas {
        assert!(std::fs::exists(l.blob_path(sha)).unwrap(), "전부 살아 있다");
    }
}

/// **W17 — 비-UTF-8 `.tmp-` 이름은 *원시 바이트*로 stat/unlink된다.** (**M46 킬**)
///
/// `Entry`는 경로를 **`de.path()`에서만** 얻는다(lossy `String`으로 **재구성하지 않는다**).
/// 재구성하는 뮤턴트(**M46** — `dir.join(&self.name)`)는 lossy가 만든 **다른 바이트**를 커널에 넘기고,
/// `symlink_metadata`가 ENOENT를 내어 **`Absent`를 정당하게 주조**한 뒤 skip한다
/// ⇒ `temps_deleted == 0` ∧ **살아 있는 temp가 영구 잔존**한다 ⇒ RED.
///
/// ⚠ **APFS는 비-UTF-8 파일명을 `EILSEQ`로 거부한다** ⇒ **Linux 전용**이다(B-12 — 개발기에선 안 돈다).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn non_utf8_temp_name_is_stat_and_unlinked_by_raw_bytes() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    // ⚠ `.tmp-` 접두는 **raw 리터럴**이다(ADR-0001: layout 상수를 경유시키면 동어반복이 된다).
    let raw = b".tmp-w17-\xff\xfe";
    let name = OsStr::from_bytes(raw);

    // ── (a) old → `Ok` ∧ `temps_deleted == 1` ∧ 항목 **부재** ───────────────────────────────
    {
        let (_d, s, l) = f14_store();
        let p = l.objects_dir().join(name);
        std::fs::write(&p, b"in flight").expect("비-UTF-8 파일명 생성(Linux)");
        // 자기검증: readdir이 **같은 바이트**를 돌려준다 ∧ lossy 경로로는 **찾을 수 없다**
        let listed: Vec<Vec<u8>> = std::fs::read_dir(l.objects_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().as_bytes().to_vec())
            .collect();
        assert!(listed.contains(&raw.to_vec()), "readdir 바이트 동일성");
        let lossy = name.to_string_lossy().to_string();
        assert!(
            std::fs::metadata(l.objects_dir().join(&lossy)).is_err(),
            "★ lossy로 재구성한 경로는 **디스크에 없다** — M46이 정확히 여기서 죽는다"
        );

        age_past(F14_EXPIRE).await;
        let stats = reconcile::run_once(&s, F14_EXPIRE, F14_SETTLE)
            .await
            .expect("PASS ABORTED — 비-UTF-8 temp도 오늘처럼 처리된다");
        assert_eq!(
            stats,
            files::store::reconcile::ReconcileStats {
                referenced: 0,
                gc_deleted: 0,
                gc_pending: 0,
                temps_deleted: 1, // ★ **원시 바이트**로 stat/unlink했다
                quarantined: 0,
            },
            "늙은 비-UTF-8 temp는 삭제된다(M46 뮤턴트에서는 0이 된다)"
        );
        assert!(!std::fs::exists(&p).unwrap(), "항목이 사라졌다");
    }

    // ── (b) recent → `Ok` ∧ `temps_deleted == 0` ∧ 항목 **잔존** ───────────────────────────
    {
        let (_d, s, l) = f14_store();
        let p = l.objects_dir().join(name);
        std::fs::write(&p, b"in flight").unwrap();

        let stats = reconcile::run_once(&s, F14_KEEP, F14_SETTLE)
            .await
            .expect("PASS ABORTED");
        assert_eq!(
            stats,
            files::store::reconcile::ReconcileStats {
                referenced: 0,
                gc_deleted: 0,
                gc_pending: 0,
                temps_deleted: 0,
                quarantined: 0,
            },
            "grace 이내의 temp는 보존된다(활성 스트리밍 보호)"
        );
        assert!(std::fs::exists(&p).unwrap(), "항목이 잔존한다");
    }
}
