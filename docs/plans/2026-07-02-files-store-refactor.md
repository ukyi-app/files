# files-store 모듈 분할 리팩토링 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 비대한 `src/store/mod.rs`(546줄)와 `src/http/internal.rs`(637줄), 그리고 4관심사가 섞인 `src/http/mod.rs`(169줄)를 관용적 Rust 모듈로 분산해 "한 파일에 너무 많은 코드"를 해소한다. 행위(공개 시그니처·HTTP 계약·on-disk 포맷·OpenAPI 스펙 바이트)는 완전히 불변.

**Architecture:** 순수 이동(behavior-preserving) 리팩터. `impl Store`는 자식 모듈(objects/buckets/listing)로 분산하고(자식 모듈은 조상 private 멤버 접근 가능 → trait·가시성 변경 불필요), 자유 함수 핸들러는 리소스별 파일(files/buckets/health)로 나눈다. `http/mod.rs`는 state/extract/response로 분해하고 파사드 `pub use`로 외부 경로를 보존한다. 인라인 테스트는 각 모듈의 `tests.rs`로, 통합 테스트 공용 헬퍼는 `tests/common/mod.rs`로 추출한다.

**Tech Stack:** Rust 2021(툴체인 1.93 핀), axum 0.8, utoipa 5.5(code-first OpenAPI), tokio, sha2. content-addressed blob 스토어.

---

## ⚠️ 이 계획은 표준 TDD가 아니다 — 리팩터 안전망 모델

**표준 red-green-refactor를 쓰지 않는다.** 이유: 행위를 바꾸지 않으므로 새로 실패시킬 테스트가 없다. 대신 **기존 91개 테스트(7 스위트)가 characterization 안전망**이다.

- **매 태스크의 검증 = `cargo test`가 그대로 91 passed(카운트 불변) + `cargo clippy`가 새 경고 없음.**
- 인라인 테스트를 별도 파일로 옮겨도 여전히 같은 크레이트의 유닛 테스트라 카운트·private 접근 100% 유지된다.
- 어떤 태스크에서든 테스트가 빨개지거나 카운트가 바뀌면 → **그 이동이 순수 이동이 아니다. 즉시 멈추고 원인 파악**(대개 누락된 `use` 또는 openapi 경로 오류).

**공통 검증 커맨드(모든 태스크에서 동일):**
```bash
cargo test --quiet          # 기대: 91 passed, 0 failed (카운트 불변)
# clippy 게이트(소견#4): -D warnings로 경고를 에러화. 단 baseline에 pre-existing 경고 2종
# (needless_lifetimes 1건·io_other_error 4건, 모두 리팩터 미대상 ranged/error/capacity)이 있어
# 그 2종만 -A로 허용하고 나머지는 전부 실패시킨다 → 리팩터가 만드는 unused import 등 새 경고는 반드시 잡힌다.
cargo clippy --all-targets -- -D warnings -A clippy::needless_lifetimes -A clippy::io_other_error
```
아래 모든 태스크는 이 clippy 커맨드를 그대로 쓴다: **exit 0 = 새 경고 없음**, non-zero = 새 경고 발생(제거할 것).

**OpenAPI 바이트 불변 검증(소견#2·#3):** 기존 `tests/openapi.rs`·`contract.rs`는 스펙의 **구조만** 검증하므로 설명 텍스트·필드 순서 같은 바이트 변동은 놓친다. DoD의 "스펙 바이트 완전 불변"을 실제로 보장하려면, **스펙을 건드리는 Task 2(internal 분할) 안에서** ① 편집 전 baseline과 ② 편집 후를 각각 캡처해 diff한다. 캡처 대상은 클라이언트가 실제로 받는 계약 — `http::internal::router`에 `GET /openapi.json`을 쳐서 나온 **응답 바디 바이트**(`Json(ApiDoc::openapi())`의 compact JSON)이며 pretty 덤프가 아니다(소견#3). 두 캡처를 같은 태스크 안에서 하므로 `/tmp` 아티팩트가 태스크 사이에 사라질 위험이 없다. 캡처용 임시 테스트는 즉시 삭제하므로 커밋 테스트 카운트(91)는 불변이다.

**Rust 모듈 규칙(이 계획의 근거 — 자주 틀리는 지점):**
- 자식 모듈은 **조상 모듈의 private 항목(필드·메서드·`use` 별칭)에 접근 가능**하다. 그래서 `Store`의 private `root`/`locks`/`meta_for`를 자식 `objects.rs`/`listing.rs`에서 pub으로 열지 않고 그대로 쓴다.
- `#[cfg(test)] mod tests;` + 별도 `tests.rs`로 빼도 private 접근은 유지(접근 권한은 파일이 아니라 모듈 트리 위치로 결정).
- `#[utoipa::path]`는 대상 fn에 붙는 attribute 매크로라 **핸들러와 분리 불가** → 핸들러와 함께 통째로 이동한다.
- utoipa가 생성하는 숨은 `__path_<fn>` 동반 타입은 `pub use` 재노출을 **따라오지 않는다** → `openapi.rs`의 `paths(...)`를 **새 경로로 직접 갱신**해야 한다.

---

## Task 0: 베이스라인 고정

**Files:** (없음 — 확인만)

**Step 1: 클린 상태·베이스라인 확인**

```bash
git status --short           # 기대: 출력 없음(클린) — docs/plans 커밋은 이미 반영
cargo test --quiet           # 기대: 91 passed
# clippy 게이트 baseline 확인(공통 검증 커맨드와 동일)
cargo clippy --all-targets -- -D warnings -A clippy::needless_lifetimes -A clippy::io_other_error
# 기대: exit 0. pre-existing 경고 5건은 -A로 baseline됨:
#   needless_lifetimes → src/http/ranged.rs:167
#   io_other_error     → src/error.rs:68, src/error.rs:82, src/http/ranged.rs:95, src/capacity.rs:7
```

만약 위 clippy가 exit 0이 아니면(clippy 버전차로 pre-existing 경고가 더 있으면), 그 lint를 `-A`에 추가해 baseline을 맞춘 뒤 **모든 태스크에 동일 적용**한다(pre-existing 경고는 이 리팩터 범위 밖 — 별도 과제).

**Step 2:** 코드 변경 없음 — 커밋 없음. 다음 태스크로. (OpenAPI 바이트 불변 baseline 캡처는 스펙을 건드리는 Task 2 **안**에서 한다 — 소견#3.)

---

## Task 1: store `impl` 분산 (objects/buckets/listing) + 테스트 분리

**대상:** `src/store/mod.rs`(546줄) → `mod.rs`(구조체+공용 헬퍼) + `objects.rs` + `buckets.rs` + `listing.rs` + `tests.rs`.

**Files:**
- Modify: `src/store/mod.rs`
- Create: `src/store/objects.rs`, `src/store/buckets.rs`, `src/store/listing.rs`, `src/store/tests.rs`

**Step 1: `objects.rs` 생성** — `impl Store`의 객체 연산 6종 + 자유 함수 `stream_to_temp`를 이동.

`src/store/objects.rs`:
```rust
use super::Store;
use super::atomic;
use crate::error::AppError;
use crate::meta::ObjectMeta;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::AsyncWriteExt;

impl Store {
    // mod.rs 38–73행 put(...) 통째로 이동
    // mod.rs 78–136행 put_stream(...) 통째로 이동
    // mod.rs 138–151행 head(...) 통째로 이동
    // mod.rs 153–163행 get_bytes(...) 통째로 이동
    // mod.rs 166–176행 open(...) 통째로 이동
    // mod.rs 274–288행 delete(...) 통째로 이동
}

// mod.rs 293–317행 stream_to_temp(...) 통째로 이동(private 자유 함수)
```
메서드/함수 본문은 **원문 그대로** 옮긴다(로직 수정 금지).

**Step 2: `buckets.rs` 생성**

`src/store/buckets.rs`:
```rust
use super::Store;
use super::atomic;
use crate::error::AppError;
use crate::meta::BucketMeta;
use crate::path::valid_bucket;

impl Store {
    // mod.rs 178–185행 put_bucket(...) 이동
    // mod.rs 187–192행 get_bucket(...) 이동
    // mod.rs 195–216행 list_buckets(...) 이동
}
```

**Step 3: `listing.rs` 생성**

`src/store/listing.rs`:
```rust
use super::Store;
use crate::error::AppError;
use crate::meta::ObjectMeta;
use crate::path::valid_bucket;

impl Store {
    // mod.rs 221–272행 list(...) 이동
}
```

**Step 4: `tests.rs` 생성** — 인라인 테스트 블록(`mod tests { ... }`의 **내부** 내용)을 이동.

`src/store/tests.rs`: `mod.rs`의 320–545행(`use super::*;`부터 마지막 테스트 `}` 직전까지, 즉 `mod tests {`와 그 닫는 `}`를 제외한 내부 전체)을 그대로 붙여넣는다. 파일 첫 줄은 기존 그대로 `use super::*;` + `use sha2::{Digest, Sha256};`.

> 참고: 이 테스트들은 meta 타입을 `crate::meta::ObjectMeta`처럼 **완전 경로**로 참조하고, `Store`/`atomic`/`AppError`는 `use super::*`로 얻는다. `s.meta_for`·`s.root`(private) 접근은 자식 모듈 규칙으로 성립한다.

**Step 5: `mod.rs` 재작성** — 구조체 + 공용 헬퍼 + 모듈 선언만 남긴다.

`src/store/mod.rs` 전체를 다음으로 교체:
```rust
pub mod atomic;
pub mod locks;
pub mod reconcile;

mod buckets;
mod listing;
mod objects;
#[cfg(test)]
mod tests;

use crate::error::AppError;
use crate::path::{meta_path, safe_object_path};
use std::path::PathBuf;

/// content-addressed 저장소. 바이트는 `.objects/<sha256>`에 불변 저장하고,
/// 키의 `<key>.meta.json`이 sha를 가리키는 단일 atomic 커밋 포인터다.
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    locks: locks::KeyLocks,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root, locks: locks::KeyLocks::new() }
    }

    pub fn blob_path(&self, sha: &str) -> PathBuf {
        self.root.join(".objects").join(sha)
    }

    fn meta_for(&self, bucket: &str, key: &str) -> Result<PathBuf, AppError> {
        Ok(meta_path(&safe_object_path(&self.root, bucket, key)?))
    }
}
```
(주석·독스트링은 원문 유지. `bytes`/`futures`/`sha2`/`tokio::io`/`BucketMeta`/`ObjectMeta`/`valid_bucket`/`Path` 등 이제 안 쓰는 import는 제거 — clippy가 unused import로 지목한다.)

**Step 6: 검증**
```bash
cargo test --quiet           # 기대: 91 passed
cargo clippy --all-targets -- -D warnings -A clippy::needless_lifetimes -A clippy::io_other_error   # 기대: exit 0(새 경고 없음)
```
빨개지면 대개 자식 모듈의 누락 import. 컴파일러 메시지대로 해당 `use` 추가.

**Step 7: 커밋**
```bash
git add src/store/
git commit -m "refactor(store): Store impl을 objects/buckets/listing 모듈로 분산 + 테스트 분리"
```

---

## Task 2: `internal.rs` 리소스별 분할 + openapi 경로 갱신

**대상:** `src/http/internal.rs`(637줄) → `internal/{mod,files,buckets,health,tests}.rs` + `src/http/openapi.rs` 경로 갱신.

**Files:**
- Delete(내용 이동): `src/http/internal.rs`
- Create: `src/http/internal/mod.rs`, `files.rs`, `buckets.rs`, `health.rs`, `tests.rs`
- Modify: `src/http/openapi.rs` (paths/schemas 경로), `src/http/mod.rs` (`pub mod internal;`는 디렉터리 모듈로 그대로 해석되어 무변경)

**Step 0: OpenAPI 스펙 baseline 캡처(편집 전 — 소견#2·#3)**

⚠️ `internal.rs`를 **아직 건드리기 전에** 클라이언트가 받는 계약을 캡처한다. 임시 테스트 `tests/_spec_dump.rs` 생성(서빙 응답 바디를 그대로 캡처):
```rust
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

#[tokio::test]
async fn dump_served_openapi() {
    let d = tempfile::tempdir().unwrap();
    let keys = d.path().join("keys.json");
    std::fs::write(&keys, r#"[{"id":"a","sha256":"00","service":"ops","admin":true}]"#).unwrap();
    let dd = d.path().join("data");
    let cfg = files::config::Config::from_env(|k| match k {
        "FILES_DATA_DIR" => Some(dd.to_string_lossy().to_string()),
        "FILES_KEYS_PATH" => Some(keys.to_string_lossy().to_string()),
        _ => None,
    })
    .unwrap();
    let state = files::http::build_state(cfg).unwrap();
    let app = files::http::internal::router(state);
    let res = app
        .oneshot(Request::builder().uri("/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    std::fs::write(std::env::var("SPEC_OUT").unwrap(), &bytes).unwrap();
}
```
실행 후 즉시 삭제(임시 파일 — 커밋 안 함):
```bash
SPEC_OUT=/tmp/files-openapi-baseline.json cargo test --test _spec_dump
rm tests/_spec_dump.rs
test -s /tmp/files-openapi-baseline.json && echo "baseline(served) 캡처 OK"
git status --short   # 기대: 출력 없음(임시 테스트 삭제됨 — 클린)
```
> **baseline 유실 복구(소견#3):** /tmp가 정리돼 baseline이 없으면, 이 Step 0을 **Task 1 커밋(store 분할까지, http 미변경) 시점의 워크트리에서 재실행**하면 pristine 스펙을 다시 얻는다. 이 Step 0과 아래 Step 8b는 같은 Task 2 안에 있어 정상 흐름에선 유실 위험이 없다.

**Step 1: `internal/` 디렉터리 + `mod.rs`(파사드) 생성**

`src/http/internal/mod.rs`:
```rust
use super::AppState;
use axum::routing::{get, put};
use axum::{Json, Router};
use utoipa::OpenApi;

// ⚠️ pub(crate) 필수(Phase C 소견#1): openapi.rs의 paths(...)가 sibling 모듈에서
// crate::http::internal::files::* 등 경로를 직접 참조한다. `mod files;`(private)이면
// 핸들러가 pub(crate)여도 경로가 `files` 컴포넌트에서 막혀 컴파일 실패한다.
// 아이템 자체는 pub(crate) 유지 → 외부 크레이트 API는 넓히지 않는다.
pub(crate) mod buckets;
pub(crate) mod files;
pub(crate) mod health;
#[cfg(test)]
mod tests;

/// 내부 API 라우터(파일 CRUD + 버킷 + 헬스 + OpenAPI 문서). 헬스/문서 외 라우트는 인증 필요.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route(
            "/api/files/{bucket}/object",
            put(files::put_file)
                .get(files::get_file)
                .head(files::head_file)
                .delete(files::delete_file),
        )
        .route("/api/files/{bucket}", get(files::list_files))
        .route("/api/buckets/{bucket}", put(buckets::put_bucket))
        .route("/api/buckets", get(buckets::get_buckets))
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .with_state(state);
    // 코드-우선 OpenAPI: /openapi.json(스펙)만 서빙. UI는 공급망 리스크로 의도적 미서빙.
    let docs = Router::new().route(
        "/openapi.json",
        get(|| async { Json(super::openapi::ApiDoc::openapi()) }),
    );
    api.merge(docs)
}
```
(현 `internal.rs` 44–65행 `router()` 본문 + 56–59행 주석을 옮기되 핸들러 참조에 `files::`/`buckets::`/`health::` 접두사만 추가.)

**Step 2: `files.rs` 생성** — DTO 3종 + files 핸들러 5종(각 `#[utoipa::path]` 포함).

`src/http/internal/files.rs` 상단 import:
```rust
use crate::capacity::free_bytes;
use crate::error::AppError;
use crate::http::openapi::ErrorResponse;
use crate::http::ranged::build_ranged;
use crate::http::{AppState, AuthKey};
use crate::meta::ObjectMeta;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::time::Duration;
```
그 아래에 현 `internal.rs`에서 다음을 **원문 그대로** 이동:
- 18–20행 `OctetStreamBody` struct
- 24–41행 `KeyQuery` struct(긴 doc 포함)
- 67–122행 `put_file`(+ `#[utoipa::path]`)
- 124–184행 `get_file`(+ 애노테이션)
- 186–231행 `head_file`(+ 애노테이션)
- 233–255행 `delete_file`(+ 애노테이션)
- 322–354행 `ObjectEntry` struct + `list_files`(+ 애노테이션)

**Step 3: `buckets.rs` 생성**

`src/http/internal/buckets.rs` 상단:
```rust
use crate::error::AppError;
use crate::http::openapi::ErrorResponse;
use crate::http::{AppState, AuthKey};
use crate::meta::{BucketMeta, Visibility};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
```
이동(원문 그대로): 257–260행 `CreateBucket`, 262–290행 `put_bucket`(+애노테이션), 292–297행 `BucketEntry`, 299–320행 `get_buckets`(+애노테이션).

**Step 4: `health.rs` 생성**

`src/http/internal/health.rs` 상단:
```rust
use crate::capacity::free_bytes;
use crate::http::AppState;
use axum::extract::State;
use axum::http::StatusCode;
```
이동(원문 그대로): 356–359행 `healthz`(+애노테이션), 361–379행 `readyz`(+애노테이션).

**Step 5: `tests.rs` 생성** — 인라인 테스트 블록 내부를 이동.

`src/http/internal/tests.rs`: 현 `internal.rs`의 383–636행(`use super::*;`부터 마지막 테스트까지, `mod tests {` 래퍼 제외)을 그대로 이동. 테스트는 `router(state)`를 HTTP(oneshot)로 구동하고 `AppState { .. }` 리터럴을 만든다 — `router`는 `super`(=internal/mod.rs), `AppState`는 `super::AppState`(재노출)로 해결된다. 나머지(`ApiKey`/`KeyRegistry`/`Capacity`/`Config`/`Store`)는 기존 `use crate::...` 그대로.

**Step 6: `internal.rs` 원본 삭제**
```bash
git rm src/http/internal.rs   # 내용은 위 파일들로 전부 이동됨
```
(`src/http/mod.rs`의 `pub mod internal;`는 `internal/mod.rs`를 자동 해석하므로 무수정.)

**Step 7: `openapi.rs` paths/schemas 경로 갱신** — ⚠️ 이 태스크의 핵심 함정.

`src/http/openapi.rs`의 `paths(...)`(39–49행)를 새 모듈 경로로 교체:
```rust
    paths(
        crate::http::internal::files::put_file,
        crate::http::internal::files::get_file,
        crate::http::internal::files::head_file,
        crate::http::internal::files::delete_file,
        crate::http::internal::files::list_files,
        crate::http::internal::buckets::put_bucket,
        crate::http::internal::buckets::get_buckets,
        crate::http::internal::health::healthz,
        crate::http::internal::health::readyz,
    ),
```
`components(schemas(...))`(50–58행)의 internal 항목 3개를 교체(meta 3종·ErrorResponse는 불변):
```rust
    components(schemas(
        crate::meta::ObjectMeta,
        crate::meta::BucketMeta,
        crate::meta::Visibility,
        crate::http::internal::files::ObjectEntry,
        crate::http::internal::buckets::BucketEntry,
        crate::http::internal::buckets::CreateBucket,
        ErrorResponse,
    )),
```

**Step 8: 검증**
```bash
cargo test --quiet           # 기대: 91 passed
cargo clippy --all-targets -- -D warnings -A clippy::needless_lifetimes -A clippy::io_other_error   # 기대: exit 0(새 경고 없음)
```
- `tests/openapi.rs`·`tests/contract.rs`가 빨개지면 → paths/schemas 경로 갱신 오류(utoipa가 핸들러를 못 찾음). Step 7 재확인.
- 컴파일 에러(핸들러 못 찾음)면 → `files::`/`buckets::`/`health::` 참조 또는 import 누락.

**Step 8b: OpenAPI 스펙 바이트 불변 대조(편집 후 — 소견#2·#3)** — 이 태스크만 스펙 소스를 건드리므로 여기서 대조.

Step 0의 임시 테스트(`tests/_spec_dump.rs`, **동일 내용** — 서빙 응답 바디 캡처)를 다시 생성한 뒤:
```bash
SPEC_OUT=/tmp/files-openapi-after.json cargo test --test _spec_dump
rm tests/_spec_dump.rs
diff /tmp/files-openapi-baseline.json /tmp/files-openapi-after.json && echo "스펙 바이트 불변 OK"
git status --short   # 기대: internal 관련 변경만(임시 테스트는 삭제됨)
```
`diff`가 **아무것도 출력하지 않아야**(served 응답 바이트 동일) 한다. 출력이 있으면 → 분할이 클라이언트 계약을 바꿨다(대개 paths/schemas 순서 오류 또는 애노테이션 이동 중 변형). 커밋 전 원인 제거.

**Step 9: 커밋**
```bash
git add src/http/internal src/http/internal.rs src/http/openapi.rs
git commit -m "refactor(http): internal API를 files/buckets/health 모듈로 분할 + openapi 경로 갱신"
```

---

## Task 3: `http/mod.rs` 해체 (state/extract/response) + 파사드

**대상:** `src/http/mod.rs`(169줄) → 모듈 선언 + 파사드 `pub use`만; `AppState`/`build_state`→`state.rs`, `AuthKey`→`extract.rs`, `IntoResponse`→`response.rs`, 인라인 테스트→`tests.rs`.

**Files:**
- Modify: `src/http/mod.rs`
- Create: `src/http/state.rs`, `src/http/extract.rs`, `src/http/response.rs`, `src/http/tests.rs`

**Step 1: `state.rs` 생성**

`src/http/state.rs`:
```rust
use crate::auth::KeyRegistry;
use crate::capacity::Capacity;
use crate::config::Config;
use crate::store::Store;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub keys: Arc<KeyRegistry>,
    pub cap: Capacity,
    pub cfg: Arc<Config>,
}

// mod.rs 27–39행 build_state(...) 통째로 이동
```
`build_state`는 원문 그대로 이동(`pub fn build_state(cfg: Config) -> std::io::Result<AppState>`).

**Step 2: `extract.rs` 생성**

`src/http/extract.rs`:
```rust
use super::AppState;
use crate::auth::ApiKey;
use crate::error::AppError;
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

/// `Authorization: Bearer <token>` 추출 + 인증. 실패 시 401.
pub struct AuthKey(pub ApiKey);

// mod.rs 55–74행 impl FromRequestParts<AppState> for AuthKey { ... } 통째로 이동
```

**Step 3: `response.rs` 생성**

`src/http/response.rs`:
```rust
use crate::error::AppError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

// mod.rs 41–50행 impl IntoResponse for AppError { ... } 통째로 이동
```

**Step 4: `tests.rs` 생성** — mod.rs 인라인 테스트 이동.

`src/http/tests.rs`: 현 `mod.rs`의 78–168행(`use super::*;`부터, `mod tests {` 래퍼 제외)을 그대로 이동. 테스트는 `AppState`/`build_state`/`AuthKey`를 쓰며 이는 mod.rs 파사드 `pub use`(Step 5)로 `super::*`에서 해결된다.

**Step 5: `mod.rs` 재작성** — 선언 + 파사드만.

`src/http/mod.rs` 전체를 교체:
```rust
pub mod internal;
pub mod openapi;
pub mod public;
pub mod ranged;

mod extract;
mod response;
mod state;
#[cfg(test)]
mod tests;

pub use extract::AuthKey;
pub use state::{build_state, AppState};
```
(`response`는 `IntoResponse` impl만 담아 re-export 불필요 — 모듈 선언만으로 impl이 등록된다.)

**Step 6: 검증**
```bash
cargo test --quiet           # 기대: 91 passed
cargo clippy --all-targets -- -D warnings -A clippy::needless_lifetimes -A clippy::io_other_error   # 기대: exit 0(새 경고 없음)
```
파사드가 `http::AppState`·`http::build_state`·`http::AuthKey`를 보존하므로 `main.rs`·`public.rs`·`internal/*`·`tests/*`는 무수정이어야 한다. 컴파일 에러가 이 경로에서 나면 파사드 `pub use` 누락.

**Step 7: 커밋**
```bash
git add src/http/mod.rs src/http/state.rs src/http/extract.rs src/http/response.rs src/http/tests.rs
git commit -m "refactor(http): mod.rs를 state/extract/response로 해체 + 파사드 재노출"
```

---

## Task 4: 통합 테스트 공용 헬퍼 추출 (`tests/common`)

**대상:** `tests/{adversarial,e2e,contract,openapi}.rs`의 복붙 헬퍼 → `tests/common/mod.rs`. **정밀 스코프**: 진짜 동일한 것만. adversarial의 in-memory 상태 빌더(`normal_state`/`state_rejecting_capacity`)와 e2e의 실-리스너 `Harness`는 각 파일 고유라 **유지**.

**Files:**
- Create: `tests/common/mod.rs`
- Modify: `tests/adversarial.rs`, `tests/e2e.rs`, `tests/contract.rs`, `tests/openapi.rs`

**Step 1: `tests/common/mod.rs` 생성**

```rust
#![allow(dead_code)] // 각 테스트 바이너리가 common을 독립 컴파일 → 일부만 사용

use axum::body::Body;
use axum::http::{header, Request};
use files::config::Config;
use files::http::{self, AppState};
use sha2::{Digest, Sha256};

pub fn hex_sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

pub async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let b = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&b).unwrap()
}

/// keys.json 문자열을 받아 tempdir 기반 내부 라우터를 만든다(contract/openapi 공용 패턴).
pub fn internal_app(keys_json: &str) -> (axum::Router, tempfile::TempDir) {
    let d = tempfile::tempdir().unwrap();
    let keys_path = d.path().join("keys.json");
    std::fs::write(&keys_path, keys_json).unwrap();
    let dd = d.path().join("data");
    let cfg = Config::from_env(|k| match k {
        "FILES_DATA_DIR" => Some(dd.to_string_lossy().to_string()),
        "FILES_KEYS_PATH" => Some(keys_path.to_string_lossy().to_string()),
        _ => None,
    })
    .unwrap();
    let state: AppState = http::build_state(cfg).unwrap();
    (http::internal::router(state), d)
}

pub fn bearer(method: &str, uri: &str, token: &str, ct: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, ct)
        .body(Body::from(body.to_string()))
        .unwrap()
}
```

**Step 2: `tests/contract.rs` 리팩터** — 로컬 `sha`/`body_json`/`app`/`bearer` 제거, common 사용.
- 파일 상단에 `mod common;` 추가, `use common::{bearer, body_json, hex_sha, internal_app};`.
- 로컬 `fn sha`, `fn body_json`, `fn app`, `fn bearer` 삭제.
- `sha("writer")` 호출부는 `hex_sha(b"writer")`로 치환(동일 값).
- `app()` 호출부는 다음으로:
  ```rust
  let keys = format!(
      r#"[{{"id":"w","sha256":"{}","service":"page","writeBuckets":["skills"],"readBuckets":["skills"]}},{{"id":"a","sha256":"{}","service":"ops","admin":true}}]"#,
      hex_sha(b"writer"), hex_sha(b"admin")
  );
  let (app, _d) = internal_app(&keys);
  ```
- `files::http::openapi::ApiDoc::openapi()` 참조는 **무변경**(경로 유지 확인).

**Step 3: `tests/openapi.rs` 리팩터**
- `mod common;` + `use common::{body_json, internal_app};`.
- 로컬 `fn app`, `fn body_json` 삭제.
- `app()` 호출부는:
  ```rust
  let (app, _d) = internal_app(r#"[{"id":"a","sha256":"00","service":"ops","admin":true}]"#);
  ```

**Step 4: `tests/adversarial.rs` 리팩터**
- `mod common;` + `use common::hex_sha;`.
- 로컬 `fn hex_sha` 삭제(16–18행). `normal_state`/`state_rejecting_capacity`/`writer_req`는 **유지**(파일 고유). 이들이 쓰는 `hex_sha`는 이제 common의 것.

**Step 5: `tests/e2e.rs` 리팩터**
- `mod common;` + `use common::hex_sha;`.
- 로컬 `fn hex_sha` 삭제(8–10행). `Harness`/`start()`는 **유지**(실-리스너 부트스트랩은 고유).

**Step 6: 검증**
```bash
cargo test --quiet           # 기대: 91 passed
cargo clippy --all-targets -- -D warnings -A clippy::needless_lifetimes -A clippy::io_other_error   # 기대: exit 0(새 경고 없음)
```
`tests/common/mod.rs`는 `mod.rs` 이름이라 cargo가 별도 테스트 바이너리로 취급하지 않는다(테스트 카운트 불변 확인).

**Step 7: 커밋**
```bash
git add tests/
git commit -m "test: 통합 테스트 공용 헬퍼를 tests/common으로 추출"
```

---

## 완료 기준 (Definition of Done)

- 4개 커밋 모두 `cargo test` 91 passed / clippy 게이트(`-D warnings -A needless_lifetimes -A io_other_error`) exit 0(새 경고 0).
- `src/http/internal.rs`·비대 `src/store/mod.rs` 제거, 리소스/관심사별 모듈로 분산.
- 공개 경로(`files::http::{AppState, build_state, AuthKey, internal::router, public::router, openapi::ApiDoc}`)·HTTP 계약·on-disk 포맷 불변.
- **OpenAPI 스펙 바이트 완전 불변** — `GET /openapi.json` 서빙 응답 바디를 Task 2 Step 0(편집 전)·Step 8b(편집 후) 캡처해 `diff`한 결과가 **출력 0**으로 증명됨.
- 최종: `git log --oneline`에 `refactor(store)` → `refactor(http)` ×2 → `test` 순서.

## 하지 말 것

- 로직 수정(순수 이동만). 시그니처·본문 변경 금지.
- `Store` 필드/`meta_for`를 pub으로 열기(자식 모듈 규칙으로 불필요).
- 유닛 테스트를 `tests/`로 이동(private 접근 깨짐).
- `openapi.rs` 위치 이동(`tests/contract.rs`가 `files::http::openapi::ApiDoc` 직접 참조).
- utoipa 애노테이션 축약/변경(C2 제외 — 스펙 바이트 불변 위해).
- adversarial/e2e의 상태 빌더·Harness를 common으로 강제 통합(과통합).

---

## Adversarial review dispositions (감사 기록)

hardened-planning Phase C에서 codex 적대적 리뷰 3패스를 실행했다. 소견 4건 전부 판정·반영. **주목: 4건 모두 "검증 하네스"에 관한 것이었고, 실제 리팩터 코드 이동(store/internal/http-mod.rs 분할)은 한 건도 지적되지 않았다** — 설계 견고성의 방증.

| Pass | # | 소견 | 심각도 | 판정 | 처리 |
|---|---|---|---|---|---|
| 1 | 1 | Task 2가 `openapi.rs`에서 private 자식 모듈(`mod files;` 등) 경로 참조 → 컴파일 실패 | HIGH | **Accepted** | `internal/mod.rs`의 자식 모듈을 `pub(crate) mod`으로(아이템은 `pub(crate)` 유지, 외부 API 불변) |
| 1 | 2 | "스펙 바이트 불변"이 DoD에 명시됐으나 미검증(기존 테스트는 구조만) | MEDIUM | **Accepted** | OpenAPI golden 캡처→diff 검증 스텝 추가(이후 pass 2에서 정밀화) |
| 2 | 3 | golden이 서빙 계약이 아닌 pretty 덤프를 비교 + /tmp 유실 복구 경로 부재 | MEDIUM | **Accepted** | 캡처를 `GET /openapi.json` **서빙 응답 바이트**로 변경 + before/after를 Task 2 안으로 이동(태스크 간 유실 제거) + 복구 절차 명시 |
| 3 | 4 | `cargo clippy`가 경고에도 exit 0 → "새 경고 0" 게이트 미강제 | MEDIUM | **Accepted** | `-D warnings` + pre-existing 2 lint만 `-A`(baseline: needless_lifetimes 1·io_other_error 4, 모두 리팩터 미대상 파일). baseline에서 exit 0 실증 확인 |

- **최종 pass 3 verdict:** `needs-attention` (summary: "the plan's main warning gate can report success while violating its own acceptance criteria") — 소견#4는 위와 같이 반영.
- **캡(3패스) 도달:** 미해결 high/critical 없음(소견#4는 MEDIUM·반영 완료). 사용자가 캡 게이트에서 "반영 후 확정(Phase D)"을 승인.
- 각 pass의 런처 slim JSON(`ok:true`·`planInDiff:true`)은 대화 기록에 증거로 첨부됨.

## Execution directives

- **Skill:** `executing-plans`로 **별도 세션, 이 워크트리에서** 구현한다.
- **연속 실행:** 배치 사이에 루틴 리뷰로 멈추지 말 것. 진짜 블로커에서만 멈춘다 — 의존성 누락, 반복 실패하는 검증, 불명확·모순 지시, 중대한 계획 공백(executing-plans의 "When to Stop and Ask"). 그 외에는 모든 태스크를 완료까지 진행.
- **커밋 — 아래 규칙을 직접 적용하고 `Skill(commit)`은 호출하지 말 것**(인터랙티브 확인이 연속 실행을 깬다):
  - **언어:** 커밋 메시지 **한국어**. **AI 마커 금지** — `🤖 Generated with`, `Co-Authored-By: Claude`, `Claude-Session` 등 절대 포함하지 않는다(이 레포 규약이 전역 CLAUDE.md의 마커 강제보다 우선).
  - **형식:** `<type>(<scope>): 한국어 설명` (필요 시 `- 상세` 본문 라인).
  - **type — 다음만 사용:** `feat`·`fix`·`refactor`·`docs`·`style`·`test`·`chore`. `perf`/`build`/`ci` 등 금지.
  - **이 계획의 커밋(그대로 사용):** Task 1 `refactor(store): Store impl을 objects/buckets/listing 모듈로 분산 + 테스트 분리` · Task 2 `refactor(http): internal API를 files/buckets/health 모듈로 분할 + openapi 경로 갱신` · Task 3 `refactor(http): mod.rs를 state/extract/response로 해체 + 파사드 재노출` · Task 4 `test: 통합 테스트 공용 헬퍼를 tests/common으로 추출`.
  - **위치:** 각 태스크 완료 시 현재 피처 워크트리(`worktree-refactor-module-split`)에 커밋(이미 main 밖이라 새 브랜치 불필요).
