//! 적대적·동시성 통합 테스트(A.5 + Phase C 반영).

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use files::auth::{ApiKey, KeyRegistry};
use files::capacity::Capacity;
use files::config::Config;
use files::http::{self, AppState};
use files::store::{reconcile, Store};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

fn hex_sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

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
        .uri("/api/files/skills/new.txt")
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
