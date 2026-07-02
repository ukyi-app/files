# files-store 리팩토링 설계 — 행위 보존 모듈 분할 + utoipa 압축 + http/mod.rs 해체

- 날짜: 2026-07-02
- 상태: 승인됨(brainstorming HARD-GATE 통과)
- 브랜치/워크트리: `worktree-refactor-module-split` @ `.claude/worktrees/refactor-module-split`
- 베이스라인: `cargo test` 91 passed / 7 suites / 0 failures

## 배경

문제 제기: "한 파일 안에 너무 많은 코드가 있는 것 같다. Rust에서 자주 쓰는 패턴·아키텍처를 리서치해서 리팩토링하고 싶다."

코드베이스 매핑 + Rust 관용 패턴 웹 리서치(모듈 조직 / axum 구조 / 테스트 배치 / utoipa / 리팩토링 기법)를 종합해 진단한 결과, 비대의 두 축은:

1. **인라인 테스트가 프로덕션의 ~40%** — `internal.rs`(테스트 257/637), `store/mod.rs`(테스트 228/546).
2. **여러 리소스/관심사가 물리적으로 한 파일에 축적** — `internal.rs`에 3 리소스(files/buckets/health) × 5 레이어(DTO·utoipa 애노테이션·라우터·핸들러·테스트); `store/mod.rs`에 객체/버킷/리스팅이 한 `impl Store`에 응집.

`ranged.rs`(Range 엔진)는 단일 책임·응집도 높음 → **손대지 않는다**(리서치: "크기가 아니라 응집도로 쪼갠다").

## 목표

관용적 Rust 모듈 분산으로 위 두 축을 해소하되 **행위 보존**을 절대 제약으로 둔다.

## 불변 제약 (모든 커밋에서)

- **행위 보존** — 공개 시그니처·HTTP 계약·on-disk 포맷(`.meta.json`/`.bucket.json`/`.objects/<sha>`)·OpenAPI 스펙 바이트 불변. 순수 이동/재배치만.
- **매 커밋 `cargo test && cargo clippy` 초록** — 인라인 유닛 테스트(화이트박스, private 접근) + `tests/` 통합 테스트가 안전망.
- **코드-우선 OpenAPI(utoipa)·content-addressed 저장 존중.**
- **범위 제외(별도 PR)**: C3 `utoipa-axum` `OpenApiRouter`, `clock` 주입화, `ValidKey` newtype, `#![deny(unreachable_pub)]` 도입.

## 확정된 설계 결정

| 결정 | 선택 | 근거 |
|---|---|---|
| 스코프 | **구조 분할만**: store/internal/`http/mod.rs` 분할 + tests/common. **C2 utoipa 압축·C3·행위 변경 제외** | 아래 "C2 제외 근거" 참조 — C2는 스펙 바이트 불변과 충돌 |
| DTO 배치 | **리소스 파일이 소유**(colocated) | 핸들러+DTO+라우트를 리소스 단위로 자기완결; axum 관용 |
| 공유 utoipa 타입 | `http/openapi.rs` 유지(ErrorResponse·ApiDoc·SecurityAddon) | 위치 불변; paths/schemas 경로만 갱신 |
| `openapi.rs` 위치 | **`http/` 레벨 유지**(internal/로 이동 안 함) | 67줄로 이미 깔끔 + `tests/{openapi,contract}.rs`가 `files::http::openapi::ApiDoc` 직접 참조 → 이동 시 컴파일 깨짐 |
| tests/common | **이번에 포함**(정밀 스코프) | 진짜 동일 헬퍼만: `hex_sha`·`body_json`·`internal_app(keys)`·`bearer`. adversarial의 in-memory 상태 빌더·e2e의 실-리스너 Harness는 유지(과통합 금지) |
| repository trait/레이어 | **도입 금지(YAGNI)** | `Store` 구현 1개·제네릭 호출 0·테스트가 실제 tempdir 사용(목킹 불요); 이 규모는 크레이트 분할 임계 이하 |

### C2 제외 근거 (writing-plans 준비 중 utoipa 5.5 검증으로 발견)

docs.rs 5.5.0 + utoipa-gen 스냅샷 + 실제 `tests/openapi.rs` 대조 결과, C2 3종 중 헤드라인 이득이 스펙 바이트 불변과 충돌:
- **401 dedup**(`IntoResponses`): 안전(7핸들러 설명 균일).
- **403 dedup**: 불가 — 설명이 엔드포인트별로 다름(`쓰기/읽기 스코프 없음`·`admin 아님`·`""`) → 단일 재사용 타입이 바이트·정보 손실.
- **`ObjectHeaders`**(`ToResponse`, get_file 42→7줄의 핵심): 불가 — GET200/206/HEAD200 헤더셋이 전부 달라 하나로 못 맞춤(바이트 일치엔 3 struct=DRY 무의미), 바이너리 바디 미검증, `response = X`(non-inline)는 `$ref`를 내보내 `tests/openapi.rs`가 깨짐.
- **`BucketPath`**(`IntoParams`): put_file만 `description="버킷명"` → 6곳 중 일부 스펙 설명 델타.

→ 바이트 안전하게 남는 건 401 dedup(미미)뿐이라 **C2 전체를 이번 리팩터에서 제외**. utoipa 압축은 스펙 설명 변경을 의도적으로 결정할 수 있는 별도 과제로 미룸. 구조 분할은 100% 바이트 안전하며 "한 파일에 너무 많은 코드" 문제를 그대로 해결한다.

## 아키텍처 / 타깃 파일 맵

### store — `impl Store`를 자식 모듈로 분산

**핵심 근거**: 자식 모듈은 조상의 private 멤버(`root`/`locks`/`meta_for`)에 접근 가능 → **가시성 변경·trait 도입 0**. store가 이미 `atomic`/`locks`/`reconcile`로 관심사 분리한 선례를 따른다.

```
src/store/
  mod.rs      struct Store + new / blob_path(pub 유지) / meta_for(private 유지) + 서브모듈 선언   (~60줄)
  objects.rs  impl Store { put, put_stream, head, get_bytes, open, delete } + stream_to_temp(private)
  buckets.rs  impl Store { put_bucket, get_bucket, list_buckets }
  listing.rs  impl Store { list }
  tests.rs    이동된 228줄 인라인 테스트  (#[cfg(test)] mod tests;  use super::*)
  atomic.rs / locks.rs / reconcile.rs   (그대로)
```

- 테스트의 `s.root`(private 필드)·`s.meta_for`(private 메서드)·`atomic::write_atomic` 접근은 모두 descendant 규칙으로 성립 → 그대로 이동만.
- 공용 픽스처(`store()`/`hex_sha`/`byte_stream`/`no_temp_residue`)를 리소스별로 흩뜨리면 3중복 → **단일 `store/tests.rs`**로 통합.

### http/internal — 리프 파일 → 디렉터리 모듈 (리소스별)

**핵심 근거**: 핸들러가 전부 자유 함수(`pub(crate) async fn`)라 coherence/orphan 제약 0.

```
src/http/internal/
  mod.rs      router() + /openapi.json 서빙만
  files.rs    OctetStreamBody · KeyQuery · ObjectEntry + put/get/head/delete_file, list_files + 각 #[utoipa::path]
  buckets.rs  CreateBucket · BucketEntry + put_bucket, get_buckets + 각 #[utoipa::path]
  health.rs   healthz, readyz + 각 #[utoipa::path]
  tests.rs    이동된 257줄 인라인 테스트
```

- `#[utoipa::path]`는 핸들러 fn에 붙는 매크로라 **핸들러와 함께 이동**(레이어 분리 불가). DTO struct만 리소스 파일로 colocate.
- 리소스(files/buckets/health) 단위가 스윗스팟 — **엔드포인트당 파일은 과분할(금지)**.
- 인라인 테스트는 `router()`를 HTTP(oneshot)로 구동하므로 `use super::*`(= internal 모듈, `router` 포함)로 충족.

### http/mod.rs 해체 — 4개 관심사 분리 + 파사드 re-export

```
src/http/
  mod.rs      서브모듈 선언 + pub use state::{AppState, build_state}; pub use extract::AuthKey;
  state.rs    AppState struct + build_state()          (+ 이동된 build_state 테스트)
  extract.rs  AuthKey (FromRequestParts)               (+ 이동된 인증 401/200 테스트)
  response.rs impl IntoResponse for AppError
  openapi.rs  ApiDoc · ErrorResponse · SecurityAddon (위치 불변); paths/schemas 경로만 갱신
  public.rs / ranged.rs   (그대로)
```

- **파사드 재노출**로 `http::AppState`·`http::build_state`·`http::AuthKey` 외부 경로가 그대로 유지 → `main.rs`·`public.rs` 등 대부분 무수정. 내부 리소스 파일만 `use crate::http::{AppState, AuthKey}`로 조정.
- `IntoResponse for AppError`는 로컬 크레이트 타입 impl이라 crate 내 어디든 배치 가능 → `response.rs`.

### tests/common — 통합 테스트 헬퍼 공유

```
tests/common/mod.rs   hex_sha · AppState 빌더(normal / rejecting-capacity) · 요청 빌더
tests/{adversarial,e2e,contract,openapi}.rs   `mod common;` 으로 공유
```

## utoipa 애노테이션 압축(C2) — 제외

위 "C2 제외 근거" 참조. `internal.rs` 분할 시 `#[utoipa::path]` 애노테이션은 **핸들러와 함께 그대로 이동**하며(축약 없음), 스펙 바이트는 완전히 불변으로 유지된다. utoipa 재사용 타입 도입은 별도 과제로 미룬다.

## 강결합·churn 처리 (리스크 핵심)

1. **openapi.rs ↔ internal 양방향 결합** — 핸들러/DTO 이동 시 `paths(...)` 9개 + `components(schemas(...))`의 internal 항목 3개(`ObjectEntry`→files, `BucketEntry`/`CreateBucket`→buckets)를 **새 모듈 경로로 동기 갱신 필수**. utoipa가 생성하는 숨은 `__path_put_file` 동반 타입은 `pub use` 재노출을 **따라오지 않으므로** `paths(...)` 경로 직접 갱신만이 정답. `tests/contract.rs`(응답 키↔스키마 required 대조) + `tests/openapi.rs`(스펙 스냅샷)가 이 회귀를 잡는다.
2. **facade re-export** — `http/mod.rs`의 `pub use`로 외부 import 파급을 리소스 파일 내부로 국한.
3. **private 접근 무변경** — store 자식 모듈의 `self.root/locks/meta_for` 접근, 테스트의 `s.root`·`s.meta_for` 접근 모두 descendant 규칙으로 성립 → 필드/메서드를 pub으로 열지 않음(캡슐화 유지).

## 커밋 순서 (strangler · 한 번에 한 조각)

각 단계 후 `cargo test && cargo clippy` 초록 확인. 프로젝트 규약(**한국어 conventional, AI 마커 없음**) 준수.

1. `test`: 베이스라인 초록 확인(안전망 고정). — 이미 91 passed 확인.
2. `refactor(store)`: `mod.rs` → objects/buckets/listing + `store/tests.rs`.
3. `refactor(http)`: `internal.rs` → `internal/{mod,files,buckets,health}` + `internal/tests.rs` + **openapi.rs paths/schemas 갱신**.
4. `refactor(http)`: `http/mod.rs` 해체(state/extract/response + facade).
5. `test`: `tests/common/mod.rs` 추출 + 4파일 공유.

각 단계가 독립 커밋 → 문제 시 해당 커밋만 revert. 스펙/계약 테스트 실패 시 즉시 중단.

## 리서치 근거(요약)

- **테스트 배치**: `#[cfg(test)] mod tests;` + 별도 `tests.rs`. private 접근은 파일 위치가 아니라 모듈 트리 위치로 결정 → 파일을 빼도 화이트박스 접근 100% 유지. 유닛 테스트를 `tests/`로 옮기면 컴파일 불가(별도 크레이트).
- **inherent impl 분산**: 자식 모듈이 조상 private 멤버 접근 가능 → trait/필드 pub 불필요.
- **핸들러 리소스별 분리**: 자유 함수 핸들러는 coherence 제약 없음.
- **utoipa**: path 애노테이션은 fn 결합이라 물리 분리 불가; DTO 분리 + 재사용 타입 압축이 관용.
- **YAGNI(Concrete Abstraction 스멜)**: 수신자가 항상 한 구체 타입이고 제네릭이 아니면 trait은 순수 비용.
