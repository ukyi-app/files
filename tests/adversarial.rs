//! 적대적·동시성 통합 테스트(A.5 + Phase C 반영).

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use files::auth::{ApiKey, KeyRegistry};
use files::capacity::Capacity;
use files::config::Config;
use files::http::{self, AppState};
use files::store::{reconcile, Store};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

mod common;
use common::hex_sha;

// ── M13.1: 동시 PUT/DELETE + 같은-size 덮어쓰기 일관성 ──────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_key_put_delete_self_consistent() {
    let d = tempfile::tempdir().unwrap();
    let s = Arc::new(Store::new(d.path().to_path_buf()));
    let mut handles = vec![];
    for i in 0..60u32 {
        let s = s.clone();
        handles.push(tokio::spawn(async move {
            if i % 5 == 0 {
                let _ = s.delete("b", "k").await;
            } else {
                // 일부는 같은 size(9바이트) 다른 내용
                let body = format!("content-{}", i % 4).into_bytes();
                let _ = s.put("b", "k", "text/plain", "u", body).await;
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    // 최종: 존재하면 메타-데이터 정합, 없으면 NotFound — 절대 desync 없음
    if let Ok((meta, bytes)) = s.get_bytes("b", "k").await {
        assert_eq!(hex_sha(&bytes), meta.sha256, "메타-데이터 desync");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_readers_never_observe_desync_on_same_size_overwrite() {
    let d = tempfile::tempdir().unwrap();
    let s = Arc::new(Store::new(d.path().to_path_buf()));
    s.put("b", "k", "text/plain", "u", b"AAAA".to_vec()).await.unwrap();
    let stop = Arc::new(AtomicBool::new(false));

    let mut readers = vec![];
    for _ in 0..4 {
        let s = s.clone();
        let stop = stop.clone();
        readers.push(tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                if let Ok((meta, bytes)) = s.get_bytes("b", "k").await {
                    assert_eq!(hex_sha(&bytes), meta.sha256, "reader가 desync 관측");
                }
            }
        }));
    }
    // 같은 size(4바이트) 다른 내용으로 반복 덮어쓰기
    for i in 0..300u32 {
        let body = vec![b'A' + (i % 26) as u8; 4];
        s.put("b", "k", "text/plain", "u", body).await.unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        r.await.unwrap();
    }
}

// ── M13.2: 중첩 키 + 업로드 경합 + reconciliation 공존 ────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_nested_puts_with_reconcile_loop_preserve_all() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let s = Arc::new(Store::new(root.clone()));
    let stop = Arc::new(AtomicBool::new(false));

    // grace 1h reconcile 루프(진행 중 업로드와 공존해야)
    let rec = {
        let root = root.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let _ = reconcile::run_once(&root, Duration::from_secs(3600)).await;
                tokio::time::sleep(Duration::from_millis(3)).await;
            }
        })
    };

    let mut hs = vec![];
    for i in 0..40u32 {
        let s = s.clone();
        hs.push(tokio::spawn(async move {
            let key = format!("dir/sub/file-{i}.bin");
            s.put("b", &key, "application/octet-stream", "u", vec![i as u8; 200])
                .await
                .unwrap();
        }));
    }
    for h in hs {
        h.await.unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    rec.await.unwrap();

    // 중첩 키 40개 모두 생존 + 정합(grace가 갓-기록 blob 보호)
    let listed = s.list("b").await.unwrap();
    assert_eq!(listed.len(), 40, "중첩 키가 reconcile에서 유실됨");
    for (k, _) in &listed {
        let (m, b) = s.get_bytes("b", k).await.unwrap();
        assert_eq!(hex_sha(&b), m.sha256);
    }
}

// ── M13.3(a): ENOSPC 거부 + temp 잔재 없음 + 기존 무손상 ──────────────────────

fn state_rejecting_capacity(data_dir: std::path::PathBuf) -> AppState {
    let writer = ApiKey {
        id: "w".into(),
        sha256: hex_sha(b"writer"),
        service: "page".into(),
        write_buckets: vec!["skills".into()],
        read_buckets: vec!["skills".into()],
        admin: false,
    };
    let dd = data_dir.to_string_lossy().to_string();
    let cfg = Config::from_env(move |k| match k {
        "FILES_DATA_DIR" => Some(dd.clone()),
        "FILES_KEYS_PATH" => Some("/tmp/keys.json".into()),
        "FILES_MAX_FILE_BYTES" => Some("1000".into()),
        _ => None,
    })
    .unwrap();
    AppState {
        store: Store::new(data_dir),
        keys: Arc::new(KeyRegistry::from_keys(vec![writer])),
        // 항상 거부: min_free가 free보다 큼
        cap: Capacity::with_free_fn(1 << 40, || Ok(10)),
        cfg: Arc::new(cfg),
    }
}

#[tokio::test]
async fn upload_rejected_507_no_temp_residue_existing_intact() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join(".objects")).unwrap();
    let state = state_rejecting_capacity(d.path().to_path_buf());
    // 기존 객체는 store로 직접(핸들러 우회) 생성
    state
        .store
        .put("skills", "existing.txt", "text/plain", "u", b"keep".to_vec())
        .await
        .unwrap();

    let app = http::internal::router(state.clone());
    let req = Request::builder()
        .method("PUT")
        .uri("/api/files/skills/object?key=new.txt")
        .header(header::AUTHORIZATION, "Bearer writer")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from("blocked"))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::INSUFFICIENT_STORAGE); // 507

    // 기존 객체 무손상
    let (m, b) = state.store.get_bytes("skills", "existing.txt").await.unwrap();
    assert_eq!(b, b"keep");
    assert_eq!(hex_sha(&b), m.sha256);

    // .objects에 temp 잔재 없음
    let mut rd = tokio::fs::read_dir(d.path().join(".objects")).await.unwrap();
    while let Some(e) = rd.next_entry().await.unwrap() {
        let n = e.file_name();
        assert!(!n.to_string_lossy().starts_with(".tmp-"), "temp 잔재 발견");
    }
}

// ── codex pass5: 쿼리-키 디코딩/검증 계약 + 인증 읽기 캐시 헤더 ──────────────────

/// skills 읽기·쓰기 가능한 writer 키 + 수용 용량의 정상 상태.
fn normal_state(data_dir: std::path::PathBuf) -> AppState {
    let writer = ApiKey {
        id: "w".into(),
        sha256: hex_sha(b"writer"),
        service: "page".into(),
        write_buckets: vec!["skills".into()],
        read_buckets: vec!["skills".into()],
        admin: false,
    };
    let dd = data_dir.to_string_lossy().to_string();
    let cfg = Config::from_env(move |k| match k {
        "FILES_DATA_DIR" => Some(dd.clone()),
        "FILES_KEYS_PATH" => Some("/tmp/keys.json".into()),
        _ => None,
    })
    .unwrap();
    AppState {
        store: Store::new(data_dir),
        keys: Arc::new(KeyRegistry::from_keys(vec![writer])),
        cap: Capacity::with_free_fn(0, || Ok(u64::MAX)),
        cfg: Arc::new(cfg),
    }
}

fn writer_req(method: &str, uri: &str) -> Request<Body> {
    common::bearer(method, uri, "writer", "application/octet-stream", "payload")
}

/// finding2: SDK 제거로 URL 구성이 소비자에게 넘어갔으므로, 쿼리-키 디코딩/검증이 정확한
/// 계약이어야 한다(모호·위험 입력은 하드-투-디버그 400이 아니라 명확한 400·정합 동작).
#[tokio::test]
async fn query_key_decoding_and_validation_contract() {
    let d = tempfile::tempdir().unwrap();
    let app = http::internal::router(normal_state(d.path().to_path_buf()));

    // 원시 슬래시로 중첩 키 생성
    let res = app
        .clone()
        .oneshot(writer_req("PUT", "/api/files/skills/object?key=dir/a.txt"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED, "원시 슬래시 중첩 키 PUT");

    // 인코딩된 슬래시(%2F)는 동일 객체를 가리켜야(서버가 디코드 → a/b 와 a%2Fb 동일)
    let res = app
        .clone()
        .oneshot(writer_req("GET", "/api/files/skills/object?key=dir%2Fa.txt"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "%2F는 동일 객체를 가리켜야");

    // 이중 인코딩(%252F)은 리터럴 '%'가 되어 세그먼트 문법 위반 → 400
    let res = app
        .clone()
        .oneshot(writer_req("GET", "/api/files/skills/object?key=dir%252Fa.txt"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "이중 인코딩은 400");

    // 빈 키 → 400
    let res = app
        .clone()
        .oneshot(writer_req("GET", "/api/files/skills/object?key="))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "빈 키는 400");

    // 중복 key 파라미터 → 모호 → 400(추출 거부)
    let res = app
        .clone()
        .oneshot(writer_req("GET", "/api/files/skills/object?key=a.txt&key=b.txt"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "중복 key는 400");

    // 과길이 키(>1024) → 400
    let long = "a".repeat(1025);
    let res = app
        .clone()
        .oneshot(writer_req("GET", &format!("/api/files/skills/object?key={long}")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "과길이 키는 400");

    // traversal(a/../b) → 세그먼트 '..' → 400
    let res = app
        .oneshot(writer_req("GET", "/api/files/skills/object?key=a/../b"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "traversal 키는 400");
}

/// finding3: 인증된 내부 객체 읽기(GET·HEAD)는 no-store/private + Vary:Authorization 이어야
/// 프록시의 쿼리-무시 캐시 오배달·접근 로그 키 노출을 막는다.
#[tokio::test]
async fn internal_object_reads_are_no_store_and_vary_authorization() {
    let d = tempfile::tempdir().unwrap();
    let app = http::internal::router(normal_state(d.path().to_path_buf()));
    let res = app
        .clone()
        .oneshot(writer_req("PUT", "/api/files/skills/object?key=c.bin"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);

    for method in ["GET", "HEAD"] {
        let res = app
            .clone()
            .oneshot(writer_req(method, "/api/files/skills/object?key=c.bin"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{method} 200");
        assert_eq!(
            res.headers().get(header::CACHE_CONTROL).and_then(|v| v.to_str().ok()),
            Some("no-store, private"),
            "{method} Cache-Control"
        );
        assert_eq!(
            res.headers().get(header::VARY).and_then(|v| v.to_str().ok()),
            Some("Authorization"),
            "{method} Vary"
        );
    }
}

/// finding1(실응답↔계약 대조): 다운로드 응답 Content-Type은 저장된 타입(octet-stream 고정 아님)이며
/// 200·206 모두 캐시/Range 헤더를 실제로 내보낸다 — 스펙 `*/*` 문서화의 근거.
#[tokio::test]
async fn download_content_type_is_stored_type_and_206_has_all_headers() {
    let d = tempfile::tempdir().unwrap();
    let app = http::internal::router(normal_state(d.path().to_path_buf()));
    // text/plain 으로 업로드(octet-stream 아님)
    let put = Request::builder()
        .method("PUT")
        .uri("/api/files/skills/object?key=note.txt")
        .header(header::AUTHORIZATION, "Bearer writer")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from("hello world"))
        .unwrap();
    assert_eq!(app.clone().oneshot(put).await.unwrap().status(), StatusCode::CREATED);

    // 전체 GET(200): Content-Type = 저장 타입 + no-store/Vary + ETag
    let res = app
        .clone()
        .oneshot(writer_req("GET", "/api/files/skills/object?key=note.txt"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()),
        Some("text/plain"),
        "다운로드 Content-Type은 저장 타입이어야(octet-stream 고정 아님)"
    );
    assert_eq!(
        res.headers().get(header::CACHE_CONTROL).and_then(|v| v.to_str().ok()),
        Some("no-store, private")
    );
    assert_eq!(
        res.headers().get(header::VARY).and_then(|v| v.to_str().ok()),
        Some("Authorization")
    );
    assert!(res.headers().get(header::ETAG).is_some(), "200 ETag");

    // Range GET(206): Content-Type=저장 타입 + Content-Range + Last-Modified + 캐시 헤더(스펙과 정합)
    let ranged = Request::builder()
        .method("GET")
        .uri("/api/files/skills/object?key=note.txt")
        .header(header::AUTHORIZATION, "Bearer writer")
        .header(header::RANGE, "bytes=0-4")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(ranged).await.unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()),
        Some("text/plain"),
        "206 Content-Type도 저장 타입"
    );
    assert_eq!(
        res.headers().get(header::CONTENT_RANGE).and_then(|v| v.to_str().ok()),
        Some("bytes 0-4/11")
    );
    assert!(res.headers().get(header::LAST_MODIFIED).is_some(), "206 Last-Modified");
    assert_eq!(
        res.headers().get(header::CACHE_CONTROL).and_then(|v| v.to_str().ok()),
        Some("no-store, private"),
        "206도 캐시 헤더"
    );
    assert_eq!(
        res.headers().get(header::VARY).and_then(|v| v.to_str().ok()),
        Some("Authorization")
    );
}

/// finding2: 스펙 pattern은 세그먼트 문법만 모델링(예약 접미사 못 거름). 서버 valid_key가 진실원임을
/// 증명 — 생성 검증기가 통과시켜도 런타임이 400으로 거부한다(lookahead 미채택의 안전망).
#[tokio::test]
async fn reserved_suffix_keys_rejected_at_runtime() {
    let d = tempfile::tempdir().unwrap();
    let app = http::internal::router(normal_state(d.path().to_path_buf()));
    for key in ["foo.meta.json", "x/foo.bucket.json"] {
        let uri = format!("/api/files/skills/object?key={key}");
        let res = app.clone().oneshot(writer_req("PUT", &uri)).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "예약 접미사 {key}는 런타임 400");
        // 상태뿐 아니라 에러 body 코드까지 계약대로(reserved_suffix) 잠금
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let j: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(j["error"].as_str(), Some("reserved_suffix"), "{key} 에러 코드: {j}");
    }
}
