# files — 홈랩 공용 파일 스토어 (설계)

- 날짜: 2026-06-30
- 상태: 승인됨 (brainstorming HARD-GATE 통과) + Phase A.5 설계 리뷰 반영
- 다음 단계: hardened-planning → writing-plans → codex 적대적 리뷰 → executing-plans

## 1. 배경 / 동기

홈랩의 여러 서비스가 파일(에셋, 백업 산출물, 사용자 업로드, 배포용 아티팩트 등)을 보관·공유할
중앙 저장소가 필요하다. 직접적 계기: **에이전트 스킬 ZIP을 올려두고 누구나 다운로드**하게 하고
싶다는 요구. 이를 일반화해, 다른 서비스들이 API로 파일을 올리고 일부를 공개 다운로드하는
**경량 blob 스토어**를 만든다. 저장 매체는 홈랩의 **외장 2TB SSD**(`bulk-ssd` StorageClass).

기존 `page`(공개 HTML/Markdown 렌더러)는 텍스트만 저장하고 공개 렌더를 엄격한 CSP 샌드박스
(`sandbox allow-scripts`, `allow-downloads` 없음)로 서빙하므로 다운로드가 원천 차단된다. 따라서
파일 호스팅은 page가 아니라 **별도 서비스**로 만든다.

## 2. 목표 / 비목표

**목표**
- 다른 홈랩 서비스가 API로 파일 업로드/조회/삭제.
- 일부 파일을 공개 다운로드(스킬 ZIP 등) + 공개 카탈로그.
- 외장 SSD에 영구 저장, 메모리 풋프린트 최소.
- 서비스별 API 키로 인증, 내부/공개 표면 분리(**앱 강제**).
- 객체-메타 일관성 보장(크래시·동시성에도 깨지지 않음).
- 공유 쓰기 서비스로서 용량 고갈 방어(v1부터).
- Node/Bun 서비스가 타입 안전하게 소비(OpenAPI + SDK).

**비목표 (YAGNI v1)**
- S3 호환 API.
- 동적 키 발급/회전 API(키는 SealedSecret 선언으로 관리).
- 재개가능/멀티파트 청크 업로드.
- 파일 버전 관리.
- **풀 per-bucket/per-service 쿼터**(v2 — v1은 전역 free-space 가드까지만).
- 관리용 React SPA(서버렌더 폼은 선택, 후순위).

## 3. 아키텍처 개요

- **단일 Rust 서비스** (axum + tokio). distroless/static, arm64, non-root.
- **저장**: `bulk-ssd` PVC를 `/data`에 마운트. DB 없음 — 파일시스템 + 사이드카 JSON 메타 + 파일시스템
  커밋 마커/락으로 일관성 확보(§4).
- **두 표면, 한 파드 — 앱이 리스너로 강제 분리**(라우팅에만 의존하지 않음, §6 A.5-1):
  - **내부 리스너**(예 `:8080`) → `/api/*` 전체(읽기·쓰기·목록·삭제·버킷). API 키 필수.
    내부 Service/HTTPRoute(`files.home.ukyi.app`)만 이 포트에 연결.
  - **공개 리스너**(예 `:8081`) → `GET /` 공개 카탈로그 + `GET /<public버킷>/<key>` 다운로드만.
    공개 Service/cloudflared 터널(`files.ukyi.app`)만 이 포트에 연결. **이 리스너엔 `/api` 핸들러 자체가 없음.**
  - 두 포트를 물리적으로 분리하므로, 공개 라우팅이 catch-all로 드리프트해도 쓰기/admin API에 **도달 불가**.
- **배포**: 홈랩 골든패스(create-app)는 스토리지 미지원(app-config 스키마에 볼륨 필드 없음) →
  **플랫폼 매니페스트** `homelab/platform/files/prod/`로 직접 배포(trip-mate valkey / adguard 패턴).
- **이미지 빌드**: 앱 레포의 `release.yaml`가 `ukyi-app/homelab`의 reusable-app-build 호출
  (Dockerfile 기반, 언어 중립 — Rust도 동일하게 빌드·GHCR push).

## 4. 데이터 모델 & 저장 레이아웃

```
/data/
  <bucket>/
    .bucket.json            # { "visibility": "public" | "internal", "owner", "createdAt" }
    <key>.meta.json         # { "contentType", "size", "sha256", "createdAt", "uploadedBy", "committed": true }
    <key>                   # 원본 바이트 (= 커밋 마커: 이 파일의 존재가 객체의 "존재"를 정의)
  .tmp/                     # 업로드 임시 파일(같은 파일시스템, atomic rename 보장)
```

**일관성 모델 (A.5-2 반영 — 데이터/메타 비원자성 해결):**
- **단일 권위 파일(커밋 마커)**: 객체는 **데이터 파일** 존재로만 "존재"로 간주. 쓰기 순서 고정 —
  ① `.meta.json`을 temp→fsync→rename(먼저), ② 데이터를 temp→fsync→rename(커밋). 데이터 rename이 커밋점.
- **non-servable 규칙**: 데이터 있는데 `.meta.json` 없거나 size/sha256 불일치 → 손상으로 보고 **서빙·목록 제외**,
  복구 대상 표시. 메타만 있고 데이터 없으면 고아 → 무시/정리.
- **키 단위 락**: 같은 `bucket/key`의 동시 PUT/DELETE를 직렬화(in-process keyed mutex). 서로 다른 키는 병렬.
- **부팅 시 reconciliation**: 시작 시(및 주기적) `.tmp` 잔재·고아 메타·불일치 객체 스캔→정리/격리.
- **원자성**: 모든 쓰기는 `.tmp`에 스트리밍→fsync→동일 파일시스템 내 rename. (SQLite 인덱스는 대안이나,
  커밋마커+락으로 "DB 없음"을 유지하며 충분.)
- bucket/key 명명 규칙으로 사이드카 충돌 방지(키는 `.meta.json`/`.bucket.json` 접미사·`.tmp` 예약).

## 5. API 계약 (REST)

내부 표면(키 필요):

| Method & path | 용도 |
|---|---|
| `PUT /api/files/{bucket}/{key}` | 업로드. **raw 바디 스트리밍**(멀티파트 아님). `Content-Type` 보존, sha256·size 계산, 메타→데이터 순 커밋 |
| `GET /api/files/{bucket}/{key}` | 다운로드(내부). Range·조건부(ETag/If-None-Match) |
| `HEAD /api/files/{bucket}/{key}` | 메타데이터만 |
| `GET /api/files/{bucket}` | 버킷 내 목록(JSON, non-servable 제외) |
| `DELETE /api/files/{bucket}/{key}` | 삭제(데이터→메타 순, 락 하에) |
| `PUT /api/buckets/{bucket}` | 버킷 생성/가시성 설정(admin). 바디 `{ visibility }` |
| `GET /api/buckets` | 버킷 목록(admin) |
| `GET /healthz` · `GET /readyz` | liveness/readiness(readyz는 /data 쓰기가능 + free-space 확인) |

공개 표면(인증 없음, public 버킷만, **별도 리스너**):

| Method & path | 용도 |
|---|---|
| `GET /` | 공개 카탈로그 HTML(public 버킷 목록 + 다운로드 링크). 비-샌드박스 |
| `GET /{public버킷}/{key}` | 다운로드. `Content-Disposition: attachment` + `X-Content-Type-Options: nosniff` + Range + 강한 ETag(sha256)/Last-Modified |

응답 규율(page 차용): 강한 ETag = `"<sha256>"`, `Last-Modified`, 조건부 304, Range(`Accept-Ranges: bytes`, 206).

## 6. 보안 모델

- **서비스별 API 키.** SealedSecret에 키 레지스트리(키별 `{ sha256, service, writeBuckets[], readBuckets[], admin? }`).
  부팅 시 메모리 로드. `Authorization: Bearer <key>` → sha256 **상수시간 비교**(`subtle`/`ring`) → 스코프 검사.
  내 admin 키 = 슈퍼유저. 키 회전 = SealedSecret 갱신 후 롤아웃(동적 발급 API 없음).

- **(A.5-1) 앱 강제 표면 분리 — 라우팅 단일점 제거**:
  - `/api`(쓰기·삭제·admin)는 **내부 전용 리스너/포트**에만 바인딩. 공개 리스너엔 그 라우트가 존재하지 않음.
  - 따라서 공개 HTTPRoute가 잘못 설정(catch-all 드리프트)돼도 쓰기 API에 물리적으로 도달 불가 —
    라우팅에만 의존하지 않는 진짜 이중 방어.
  - 추가로 공개 핸들러는 `internal` 버킷을 절대 서빙하지 않음(경로를 알아도 404).
  - **테스트**: HTTPRoute 설정과 독립적으로, 공개 리스너로 `/api/*` 호출이 거부됨을 검증.

- **공개 도메인 stored-XSS 차단**: 업로드 콘텐츠를 inline 렌더하지 않음 — 항상 `attachment` + `nosniff`(+ 다운로드 CSP). 공개 도메인이 `page`와 분리돼 쿠키/세션 공유 없음.

- **경로 안전**: bucket·key 정규화. `..`, 절대경로, 제어문자, 예약 접미사 거부. charset 화이트리스트. 정규화 후 데이터 루트 밖이면 거부.

- **NetworkPolicy**: 내부 API ingress를 알려진 소비 서비스 ns로 제한, egress 최소화.

- **크기 상한**: 파일당 설정값(기본 1 GiB). 바디 길이 가드 + 스트리밍 중 초과 시 중단·temp 정리.

- **(A.5-3) v1 용량 가드 — 공유 쓰기 서비스 디스크 고갈 방어**:
  - 업로드 **시작 전·스트리밍 중** free-space 저워터마크(예 가용 < N% 또는 < M GiB) 체크 → 초과 시 깔끔히 거부
    (`507 Insufficient Storage`), ENOSPC 중도실패로 인한 부분 손상 방지.
  - `.tmp` 파일 회계·만료 정리(중단된 업로드 누수 방지).
  - near-full 시 readiness 저하(쓰기 의존 소비자에 신호).
  - **풀 per-bucket/per-service 쿼터는 v2.**

## 7. 배포 & 라우팅 (홈랩)

`homelab/platform/files/prod/` (kustomize):
- `deployment.yaml` — GHCR 이미지, `bulk-ssd` PVC `/data` 마운트, `emptyDir`로 `/data/.tmp`는 같은 볼륨 사용,
  `strategy: Recreate`(RWO PVC), securityContext(runAsNonRoot, fsGroup, readOnlyRootFilesystem=true[쓰기는 /data·/tmp], cap drop ALL, seccomp RuntimeDefault). 컨테이너 포트 2개(내부 8080 / 공개 8081).
- `pvc.yaml` — `storageClassName: bulk-ssd`, RWO, 용량(예 50–100Gi, 확장 가능).
- `service.yaml` — 내부용·공개용 분리(또는 한 Service에 포트 2개 + 라우트별 포트 타깃).
- `httproute.yaml` ×2 — 내부 호스트(`files.home.ukyi.app` → 8080) + 공개 호스트(`files.ukyi.app` → 8081, `GET /`·`GET /{bucket}/{key}`). 공개는 cloudflared 터널.
- `networkpolicy.yaml` — ingress 제한.
- `*.sealed.yaml` — API 키 레지스트리(+ 소비 서비스별 키는 각 서비스 SealedSecret).
- `kustomization.yaml`, `*.bats` 매니페스트 테스트(homepage/adguard 패턴).

> 외장 SSD 경로: macOS `/Volumes/homelab` → OrbStack k3s VM `/mnt/mac/Volumes/homelab/k3s-bulk` → `bulk-ssd` 프로비저너. virtiofs 바인드라 SSD unmount/OrbStack 재기동 시 PV 경로 소실 가능 → readyz가 /data 쓰기가능 확인.

## 8. Node 연동 (OpenAPI + 타입드 SDK)

- **OpenAPI 3.1 계약**: `files` 레포에 `openapi.yaml`(SSOT) 커밋. 선택적으로 `/openapi.json` 서빙.
- **타입드 Node/TS 클라이언트** `@ukyi/files-client`:
  - `new FilesClient({ baseUrl, apiKey })` + `putFile/getFile/headFile/list/deleteFile/putBucket`.
  - 스트림 친화(Web Streams/Blob/Buffer), 완전 타입. **GitHub Packages** publish(files 레포 CI).
  - 타입은 OpenAPI에서 생성(openapi-to-typescript) 또는 작은 표면이라 수기 + 계약 일치 테스트.
- 결과: `await files.putFile("page-assets", key, bytes)` 한 줄.

## 9. 공개 스킬 카탈로그 (원래 동기)

`skills` **public 버킷**에 ZIP 업로드 → `files.ukyi.app/`가 목록 + 다운로드 링크 제공. page의 CSP 샌드박스와
무관(별도 앱·도메인)하므로 다운로드 정상 동작.

## 10. 테스트 전략 (TDD)

- **Rust 단위**: 경로 정규화/traversal 거부, 키 스코프·상수시간 인증, 가시성 경계, 메타 직렬화, ETag/Range.
- **Rust 통합(임시 디렉터리 `/data`)**: 업로드→다운로드→삭제, 큰 파일 Range.
- **적대적 테스트(A.5 반영)**:
  - 공개 리스너로 `/api/*` 호출 거부(HTTPRoute와 독립).
  - 같은 키 동시 PUT/DELETE → desync/고아 없음(락 검증).
  - 쓰기 중 크래시/재기동 → reconciliation으로 일관 상태 복구(부분 객체 non-servable).
  - free-space 저워터마크/ENOSPC → 507 거부 + temp 정리, 부분 손상 없음.
- **계약 테스트**: OpenAPI ↔ 실제 응답, SDK ↔ 서버 왕복.
- **매니페스트 bats**: netpol·route(공개가 /api 비노출)·deployment(Recreate·securityContext·포트 2개).

## 11. 리포 구조 (두 레포)

- **`ukyi-app/files`** (신규): Rust 앱(`src/`, `Cargo.toml`) + `openapi.yaml` + `clients/node/`(SDK) +
  `Dockerfile`(musl→distroless/static) + `.github/workflows/release.yaml` + `docs/plans/`(설계 + 계획).
- **`homelab`** (기존): `platform/files/prod/` 배포 매니페스트(별도 변경/PR).

## 12. 리스크 & 완화

| 리스크 | 완화 |
|---|---|
| 공개 표면으로 내부 쓰기/파일 노출 | **앱이 /api를 내부 리스너에만 바인딩**(라우팅 비의존) + internal 버킷 거부 + 독립 테스트 |
| 객체-메타 desync(크래시/동시성) | 단일 커밋 마커 + 키 단위 락 + non-servable 규칙 + 부팅 reconciliation |
| 디스크 고갈(공유 쓰기) | v1 free-space 워터마크 + temp 회계 + readiness 저하 (+ 쿼터 v2) |
| 업로드 콘텐츠 stored-XSS(공개) | attachment + nosniff + 별도 도메인, inline 금지 |
| 경로 traversal | 정규화·화이트리스트·루트 밖 거부, 사이드카 접미사 예약 |
| RWO PVC 교착 | `strategy: Recreate` |
| SSD/virtiofs 경로 소실 | readyz가 /data 쓰기가능 확인, 순차 R/W 한정 |
| 키 유출 | 서비스별 키·버킷 스코프로 폭발반경 제한, SealedSecret 회전 |

## 13. 결정 로그 (확정)

- 직접 구현. **Rust + axum/tokio**(풋프린트·린 바이너리; 처리량은 I/O 바운드라 언어 무관).
- 범용 파일서버 + 공용 서비스(다른 서비스가 이 서버 경유 업로드).
- **서비스별 API 키**(네트워크 신뢰만은 불가).
- **내부/공개 두 표면 — 앱이 별도 리스너로 강제 분리**(A.5-1). 가시성은 **버킷 단위**.
- 저장은 **파일시스템 + 사이드카 메타 + 커밋마커/락 일관성 모델**(DB 없음)(A.5-2), `bulk-ssd` PVC.
- **v1 용량 가드**(free-space 워터마크+temp 정리), 풀 쿼터는 v2(A.5-3).
- 배포는 **플랫폼 매니페스트**(골든패스 스토리지 미지원).
- Node 연동은 **OpenAPI + 타입드 SDK(@ukyi/files-client, GitHub Packages)**.
- 키→버킷 **스코프 적용**, 파일 상한 기본 **1 GiB**.

## A.5 설계 리뷰 dispositions (codex, 2026-06-30)

verdict: needs-attention (high ×3, 전부 수용). `ok:true`, `planInDiff:true`.

| # | 발견 | 판정 | 반영 |
|---|---|---|---|
| 1 | 표면 분리가 라우팅에만 의존(드리프트 시 쓰기 API 공개노출) | Accept | §3·§6 — /api를 내부 전용 리스너에만 바인딩, 독립 테스트 |
| 2 | 사이드카 메타 비원자성(crash/동시성 desync) | Accept | §4 — 단일 커밋마커·키 락·non-servable·reconciliation |
| 3 | 디스크 고갈 v2 연기(공유 쓰기 서비스) | Accept(범위조정) | §6 — v1 free-space 가드·temp 정리; 풀 쿼터만 v2 |
