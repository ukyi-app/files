# files — 홈랩 공용 파일 스토어 (설계)

- 날짜: 2026-06-30
- 상태: 승인됨 (brainstorming HARD-GATE 통과)
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
- 서비스별 API 키로 인증, 내부/공개 표면 분리.
- Node/Bun 서비스가 타입 안전하게 소비(OpenAPI + SDK).

**비목표 (YAGNI v1)**
- S3 호환 API.
- 동적 키 발급/회전 API(키는 SealedSecret 선언으로 관리).
- 재개가능/멀티파트 청크 업로드.
- 파일 버전 관리.
- 동적 버킷 쿼터.
- 관리용 React SPA(서버렌더 폼은 선택, 후순위).

## 3. 아키텍처 개요

- **단일 Rust 서비스** (axum + tokio). distroless/static, arm64, non-root.
- **저장**: `bulk-ssd` PVC를 `/data`에 마운트. DB 없음 — 파일시스템 + 사이드카 JSON 메타로 자기완결.
- **두 표면, 한 파드** (라우팅 + 앱 이중 분리):
  - **내부** `files.home.ukyi.app` → `/api/*` 전체(읽기·쓰기·목록·삭제·버킷). API 키 필수.
  - **공개** `files.ukyi.app` → `GET /` 공개 카탈로그 + `GET /<public버킷>/<key>` 다운로드만.
    `/api`는 공개 라우팅하지 않음, 쓰기 불가.
- **배포**: 홈랩 골든패스(create-app)는 스토리지 미지원(app-config 스키마에 볼륨 필드 없음) →
  **플랫폼 매니페스트** `homelab/platform/files/prod/`로 직접 배포(trip-mate valkey / adguard 패턴).
- **이미지 빌드**: 앱 레포의 `release.yaml`가 `ukyi-app/homelab`의 reusable-app-build 호출
  (Dockerfile 기반, 언어 중립 — Rust도 동일하게 빌드·GHCR push).

## 4. 데이터 모델 & 저장 레이아웃

```
/data/
  <bucket>/
    .bucket.json            # { "visibility": "public" | "internal", "owner": "<service>" , "createdAt": ... }
    <key>                   # 원본 바이트
    <key>.meta.json         # { "contentType", "size", "sha256", "createdAt", "uploadedBy" }
```

- **버킷 = 가시성 경계.** `public`이면 공개 표면에서 다운로드 가능, `internal`이면 절대 비노출.
- 목록은 디렉터리 스캔으로 생성. 메타데이터는 사이드카 `.meta.json`.
- **원자적 쓰기**: temp 파일에 스트리밍 기록 → fsync → 동일 파일시스템 내 atomic rename. 메타도 동일.
- bucket/key 명명 규칙으로 사이드카 충돌 방지(키는 `.meta.json`/`.bucket.json` 접미사 예약).

## 5. API 계약 (REST)

내부 표면(키 필요):

| Method & path | 용도 |
|---|---|
| `PUT /api/files/{bucket}/{key}` | 업로드. **raw 바디 스트리밍**(멀티파트 아님). `Content-Type` 헤더 보존, sha256·size 계산, temp→rename |
| `GET /api/files/{bucket}/{key}` | 다운로드(내부). Range·조건부(ETag/If-None-Match) 지원 |
| `HEAD /api/files/{bucket}/{key}` | 메타데이터만 |
| `GET /api/files/{bucket}` | 버킷 내 목록(JSON) |
| `DELETE /api/files/{bucket}/{key}` | 삭제(파일 + 사이드카) |
| `PUT /api/buckets/{bucket}` | 버킷 생성/가시성 설정(admin 스코프). 바디 `{ visibility }` |
| `GET /api/buckets` | 버킷 목록(admin) |
| `GET /healthz` · `GET /readyz` | liveness/readiness |

공개 표면(인증 없음, public 버킷만):

| Method & path | 용도 |
|---|---|
| `GET /` | 공개 카탈로그 HTML(public 버킷의 파일 목록 + 다운로드 링크). 비-샌드박스 |
| `GET /{public버킷}/{key}` | 다운로드. `Content-Disposition: attachment` + `X-Content-Type-Options: nosniff` + Range + 강한 ETag(sha256)/Last-Modified |

응답 규율(page 차용): 강한 ETag = `"<sha256>"`, `Last-Modified`, 조건부 304, Range(`Accept-Ranges: bytes`, 206). Cloudflare 친화.

## 6. 보안 모델

- **서비스별 API 키.** SealedSecret에 키 레지스트리(예: 키별 `{ sha256, service, writeBuckets[], readBuckets[], admin? }`).
  부팅 시 메모리 로드. `Authorization: Bearer <key>` → sha256 **상수시간 비교**(`subtle`/`ring`) → 스코프 검사.
  내 admin 키 = 슈퍼유저. 키 회전 = SealedSecret 갱신 후 롤아웃(동적 발급 API 없음).
- **표면 분리 이중 방어**:
  1) 공개 HTTPRoute는 `/api` 경로를 라우팅하지 않음(업로드 자체가 인터넷에서 도달 불가).
  2) 앱의 공개 핸들러는 `internal` 버킷을 절대 서빙하지 않음(경로를 알아도 404).
- **공개 도메인 stored-XSS 차단**: 업로드 콘텐츠를 inline 렌더하지 않음 — 항상 `attachment` + `nosniff`(+ 다운로드 응답 CSP). 공개 도메인이 `page`와 분리돼 쿠키/세션 공유 없음.
- **경로 안전**: bucket·key 정규화. `..`, 절대경로, 제어문자, 심볼릭/예약 접미사 거부. charset 화이트리스트. 정규화 후 데이터 루트 밖이면 거부.
- **NetworkPolicy**: 내부 API ingress를 알려진 소비 서비스 ns로 제한, egress 최소화.
- **크기 상한**: 파일당 설정값(기본 예: 1 GiB; 2TB SSD라 여유). 바디 길이 가드 + 스트리밍 중 초과 시 중단·temp 정리.
- **디스크 고갈 방어(후순위 가능)**: 업로드 전 가용 용량 확인 또는 쿼터(v2).

## 7. 배포 & 라우팅 (홈랩)

`homelab/platform/files/prod/` (kustomize):
- `deployment.yaml` — GHCR 이미지, `bulk-ssd` PVC `/data` 마운트, `strategy: Recreate`(RWO PVC),
  securityContext(runAsNonRoot, fsGroup, readOnlyRootFilesystem=true[쓰기는 /data·/tmp만], cap drop ALL, seccomp RuntimeDefault).
- `pvc.yaml` — `storageClassName: bulk-ssd`, RWO, 용량(예 50–100Gi, 확장 가능).
- `service.yaml` — ClusterIP :8080.
- `httproute.yaml` ×2 — 내부 호스트(`files.home.ukyi.app`, 전체) + 공개 호스트(`files.ukyi.app`, `GET /`·`GET /{bucket}/{key}`만). 공개는 cloudflared 터널 경유.
- `networkpolicy.yaml` — ingress 제한.
- `*.sealed.yaml` — API 키 레지스트리(+ 소비 서비스별 키는 각 서비스 SealedSecret).
- `kustomization.yaml`, `*.bats` 매니페스트 테스트(homepage/adguard 패턴).

> 외장 SSD 경로: macOS `/Volumes/homelab` → OrbStack k3s VM `/mnt/mac/Volumes/homelab/k3s-bulk` → `bulk-ssd` 프로비저너. virtiofs 바인드라 SSD unmount/OrbStack 재기동 시 PV 경로 소실 가능(순차 파일 R/W엔 적합).

## 8. Node 연동 (OpenAPI + 타입드 SDK)

- **OpenAPI 3.1 계약**: `files` 레포에 `openapi.yaml`(SSOT) 커밋. 선택적으로 `/openapi.json` 서빙.
- **타입드 Node/TS 클라이언트** `@ukyi/files-client`:
  - `new FilesClient({ baseUrl, apiKey })` + `putFile/getFile/headFile/list/deleteFile/putBucket`.
  - 스트림 친화(Web Streams/Blob/Buffer 허용·반환), 완전 타입.
  - **GitHub Packages**로 publish(files 레포 CI). 소비 서비스는 `bun add @ukyi/files-client`.
  - 타입은 OpenAPI에서 생성(openapi-to-typescript) 또는 작은 표면이라 수기 + 계약 일치 테스트.
- 결과: `await files.putFile("page-assets", key, bytes)` 한 줄.

## 9. 공개 스킬 카탈로그 (원래 동기)

`skills`(또는 유사) **public 버킷**에 ZIP 업로드 → `files.ukyi.app/`가 목록 + 다운로드 링크 제공.
page의 CSP 샌드박스와 무관(별도 앱·도메인)하므로 다운로드 정상 동작.

## 10. 테스트 전략 (TDD)

- **Rust 단위**: 경로 정규화/traversal 거부, 키 스코프·상수시간 인증, 가시성 경계, 메타 직렬화, ETag/Range 계산.
- **Rust 통합**: 임시 디렉터리를 `/data`로 한 실제 파일시스템 — 업로드→다운로드→삭제, 원자성, 큰 파일 Range, 동시성.
- **계약 테스트**: OpenAPI 스펙 ↔ 실제 응답 일치. SDK ↔ 서버 왕복(테스트 서버 기동).
- **매니페스트 bats**: netpol·route(공개가 /api 비노출)·deployment(Recreate·securityContext) 검증.

## 11. 리포 구조 (두 레포)

- **`ukyi-app/files`** (신규): Rust 앱(`src/`, `Cargo.toml`) + `openapi.yaml` + `clients/node/`(SDK) +
  `Dockerfile`(멀티스테이지 musl→distroless/static) + `.github/workflows/release.yaml`(reusable-app-build 호출) +
  `docs/plans/`(이 설계 + 구현 계획).
- **`homelab`** (기존): `platform/files/prod/` 배포 매니페스트(별도 변경/PR).

## 12. 리스크 & 완화

| 리스크 | 완화 |
|---|---|
| 공개 표면으로 내부 파일 유출 | 라우팅에서 /api·internal 비노출 + 앱에서 internal 버킷 거부(이중) |
| 업로드 콘텐츠로 stored-XSS(공개 도메인) | attachment + nosniff + 별도 도메인, inline 렌더 금지 |
| 경로 traversal | 정규화·화이트리스트·루트 밖 거부, 사이드카 접미사 예약 |
| RWO PVC 교착 | `strategy: Recreate` |
| SSD/virtiofs 경로 소실 | 순차 R/W 용도 한정, readiness가 /data 쓰기 가능 확인 |
| 디스크 고갈 | 파일 상한 + (v2) 쿼터/가용량 체크 |
| 키 유출 | 서비스별 키·버킷 스코프로 폭발반경 제한, SealedSecret 회전 |

## 13. 결정 로그 (확정)

- 직접 구현(기성품 아님). **Rust + axum/tokio**(풋프린트·린 바이너리 목적; 처리량은 I/O 바운드라 언어 무관).
- 범용 파일서버 + 공용 서비스(다른 서비스가 이 서버 경유 업로드).
- **서비스별 API 키**(네트워크 신뢰만은 불가).
- **내부/공개 두 표면 분리**, 가시성은 **버킷 단위**.
- 저장은 **파일시스템 + 사이드카 메타**(DB 없음), `bulk-ssd` PVC.
- 배포는 **플랫폼 매니페스트**(골든패스 스토리지 미지원).
- Node 연동은 **OpenAPI + 타입드 SDK(@ukyi/files-client, GitHub Packages)**.
- 키→버킷 **스코프 적용**, 파일 상한 기본 **1 GiB**(조정 가능).
