//! 실제 2 리스너 부트스트랩 E2E(reqwest) — 표면 분리 + 대용량 Range.

use files::config::Config;
use files::http;
use serde_json::json;
use sha2::{Digest, Sha256};

fn hex_sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

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
        .put(format!("{}/api/files/downloads/object?key=pub.txt", h.internal))
        .header("authorization", "Bearer writer")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    let r = c
        .put(format!("{}/api/files/secret/object?key=hid.txt", h.internal))
        .header("authorization", "Bearer writer")
        .body("classified")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);

    // 공개: public 버킷 다운로드 200
    let r = c.get(format!("{}/downloads/pub.txt", h.public)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "hello");

    // 공개: /api GET/PUT → 404(표면 분리)
    let r = c.get(format!("{}/api/files/downloads/object?key=pub.txt", h.public)).send().await.unwrap();
    assert_eq!(r.status(), 404);
    let r = c
        .put(format!("{}/api/files/downloads/object?key=x.txt", h.public))
        .body("nope")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);

    // 공개: internal 버킷 다운로드 → 404(존재 비노출)
    let r = c.get(format!("{}/secret/hid.txt", h.public)).send().await.unwrap();
    assert_eq!(r.status(), 404);

    // internal 리스너: 정상 다운로드
    let r = c
        .get(format!("{}/api/files/secret/object?key=hid.txt", h.internal))
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
        .put(format!("{}/api/files/downloads/object?key=big.bin", h.internal))
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
    let r = c.get(format!("{}/downloads/big.bin", h.public)).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.bytes().await.unwrap().len(), size);
}
