use crate::error::AppError;
use crate::meta::ObjectMeta;
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

enum RangeResult {
    Sat(u64, u64),
    Unsat,
    Ignore,
}

/// 단일 byte-range 파싱. 문법 오류/미지원 단위 → Ignore(전체 응답), 불충족 → Unsat(416).
fn parse_range(value: &str, total: u64) -> RangeResult {
    let spec = match value.strip_prefix("bytes=") {
        Some(s) => s.trim(),
        None => return RangeResult::Ignore,
    };
    if spec.contains(',') {
        return RangeResult::Ignore; // multi-range 미지원 → 전체 응답
    }
    let (a, b) = match spec.split_once('-') {
        Some(x) => x,
        None => return RangeResult::Ignore,
    };
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        // suffix: bytes=-n
        let n: u64 = match b.parse() {
            Ok(n) => n,
            Err(_) => return RangeResult::Ignore,
        };
        if n == 0 || total == 0 {
            return RangeResult::Unsat;
        }
        let len = n.min(total);
        return RangeResult::Sat(total - len, total - 1);
    }
    let start: u64 = match a.parse() {
        Ok(s) => s,
        Err(_) => return RangeResult::Ignore,
    };
    if start >= total {
        return RangeResult::Unsat;
    }
    let end = if b.is_empty() {
        total - 1
    } else {
        match b.parse::<u64>() {
            Ok(e) => e.min(total - 1),
            Err(_) => return RangeResult::Ignore,
        }
    };
    if start > end {
        return RangeResult::Unsat;
    }
    RangeResult::Sat(start, end)
}

fn set_common_headers(resp: &mut Response, meta: &ObjectMeta, etag: &str) {
    let h = resp.headers_mut();
    h.insert(header::ETAG, HeaderValue::from_str(etag).unwrap());
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    let ct = HeaderValue::from_str(&meta.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_TYPE, ct);
    if let Ok(v) = HeaderValue::from_str(&meta.created_at) {
        h.insert(header::LAST_MODIFIED, v);
    }
}

/// 강한 ETag(=`"<sha256>"`) + If-None-Match(304) + 단일 Range(206/416) + 전체(200).
/// 본문은 blob 파일을 seek + ReaderStream으로 스트리밍.
pub async fn build_ranged(headers: &HeaderMap, meta: &ObjectMeta, mut file: tokio::fs::File) -> Response {
    let etag = format!("\"{}\"", meta.sha256);
    let total = meta.size;

    // If-None-Match → 304
    if let Some(inm) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        if inm.trim() == "*" || inm.split(',').any(|t| t.trim() == etag) {
            let mut resp = Response::new(Body::empty());
            *resp.status_mut() = StatusCode::NOT_MODIFIED;
            resp.headers_mut()
                .insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
            return resp;
        }
    }

    if let Some(range) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        match parse_range(range, total) {
            RangeResult::Sat(start, end) => {
                if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
                    return AppError::Internal(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "seek failed",
                    ))
                    .into_response();
                }
                let len = end - start + 1;
                let body = Body::from_stream(ReaderStream::new(file.take(len)));
                let mut resp = Response::new(body);
                *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
                set_common_headers(&mut resp, meta, &etag);
                let h = resp.headers_mut();
                h.insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")).unwrap(),
                );
                h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
                return resp;
            }
            RangeResult::Unsat => {
                let mut resp = Response::new(Body::empty());
                *resp.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
                set_common_headers(&mut resp, meta, &etag);
                resp.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{total}")).unwrap(),
                );
                return resp;
            }
            RangeResult::Ignore => {} // 전체 응답으로 진행
        }
    }

    // 200 전체
    let body = Body::from_stream(ReaderStream::new(file));
    let mut resp = Response::new(body);
    set_common_headers(&mut resp, meta, &etag);
    resp.headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(total));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::ObjectMeta;
    use axum::http::{header, HeaderMap, StatusCode};
    use axum::response::Response;
    use sha2::{Digest, Sha256};

    async fn fixture(content: &[u8]) -> (ObjectMeta, tokio::fs::File, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("blob");
        tokio::fs::write(&p, content).await.unwrap();
        let meta = ObjectMeta {
            content_type: "text/plain".into(),
            size: content.len() as u64,
            sha256: hex::encode(Sha256::digest(content)),
            created_at: "2026-01-01T00:00:00Z".into(),
            uploaded_by: "x".into(),
        };
        let f = tokio::fs::File::open(&p).await.unwrap();
        (meta, f, d)
    }

    async fn body_bytes(resp: Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    fn hdr<'a>(resp: &'a Response, name: header::HeaderName) -> &'a str {
        resp.headers().get(name).unwrap().to_str().unwrap()
    }

    #[tokio::test]
    async fn full_200_with_etag_and_length() {
        let (meta, f, _d) = fixture(b"hello world").await;
        let resp = build_ranged(&HeaderMap::new(), &meta, f).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(hdr(&resp, header::ETAG), format!("\"{}\"", meta.sha256));
        assert_eq!(hdr(&resp, header::ACCEPT_RANGES), "bytes");
        assert_eq!(hdr(&resp, header::CONTENT_LENGTH), "11");
        assert_eq!(body_bytes(resp).await, b"hello world");
    }

    #[tokio::test]
    async fn partial_206_closed_range() {
        let (meta, f, _d) = fixture(b"hello world").await;
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, "bytes=0-4".parse().unwrap());
        let resp = build_ranged(&h, &meta, f).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(hdr(&resp, header::CONTENT_RANGE), "bytes 0-4/11");
        assert_eq!(hdr(&resp, header::CONTENT_LENGTH), "5");
        assert_eq!(body_bytes(resp).await, b"hello");
    }

    #[tokio::test]
    async fn suffix_range_206() {
        let (meta, f, _d) = fixture(b"hello world").await;
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, "bytes=-5".parse().unwrap());
        let resp = build_ranged(&h, &meta, f).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(hdr(&resp, header::CONTENT_RANGE), "bytes 6-10/11");
        assert_eq!(body_bytes(resp).await, b"world");
    }

    #[tokio::test]
    async fn open_ended_range_206() {
        let (meta, f, _d) = fixture(b"hello world").await;
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, "bytes=6-".parse().unwrap());
        let resp = build_ranged(&h, &meta, f).await;
        assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(body_bytes(resp).await, b"world");
    }

    #[tokio::test]
    async fn if_none_match_304() {
        let (meta, f, _d) = fixture(b"hello world").await;
        let mut h = HeaderMap::new();
        h.insert(
            header::IF_NONE_MATCH,
            format!("\"{}\"", meta.sha256).parse().unwrap(),
        );
        let resp = build_ranged(&h, &meta, f).await;
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
        assert!(body_bytes(resp).await.is_empty());
    }

    #[tokio::test]
    async fn unsatisfiable_416() {
        let (meta, f, _d) = fixture(b"hello world").await;
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, "bytes=100-200".parse().unwrap());
        let resp = build_ranged(&h, &meta, f).await;
        assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(hdr(&resp, header::CONTENT_RANGE), "bytes */11");
    }

    #[tokio::test]
    async fn unknown_unit_ignored_full_200() {
        let (meta, f, _d) = fixture(b"hello world").await;
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, "items=0-4".parse().unwrap());
        let resp = build_ranged(&h, &meta, f).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_bytes(resp).await, b"hello world");
    }
}
