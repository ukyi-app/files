---
id: R-6
title: public 예약 경로 404를 layout::RESERVED_BUCKETS에서 파생(두-목록 종결)
status: done
blocked-by: [R-1]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed: 2026-07-12
---

## ⚠ 개정 A-4 (2026-07-12, 실측 후 인간 확정) — 원안 폐기

**원안**("`layout::RESERVED_BUCKETS` 루프 파생으로 교체 — 동일한 3개 라우트가
등록되므로 관측 행동 동일")의 **전제가 거짓임이 실측으로 확정됐다.** 현재 3라우트는
모양이 **비대칭**이고(`api`=서브트리 `/api/{*rest}`, `healthz`/`readyz`=정확 일치),
이 비대칭이 곧 관측 행동이다:

| 셀 | 현행(실측, wire) |
|---|---|
| `PUT`/`POST`/`DELETE`/`OPTIONS` `/healthz/foo`·`/readyz/foo` | **405 + `Allow: GET,HEAD`** (예약 라우트 부재 → `/{bucket}/{*key}`의 `get()` 전용 라우터에 매칭) |
| `GET /healthz/foo` | 404 **JSON** `{"error":"not_found"}` |
| `PUT`/`POST`/`DELETE` `/api/x/y` | 404 (빈 바디 — `/api/{*rest}`의 `any()`) |

균일 루프 파생은 3개가 아니라 **6개** 라우트를 등록하고 **10개 셀**(상태코드 8 +
바디 2)을 바꾼다 → 행위 보존 위반. 이름만 담은 `RESERVED_BUCKETS`로는 모양의
비대칭을 **파생할 수 없다**(증명: 단일 세그먼트 이름에서 균일하게 만들 수 있는
등록 모양 4가지가 전부 파손).

부수 발견: 행동을 실제로 지탱하는 예약 라우트는 **`/api/{*rest}` 하나뿐**이다 —
`/healthz`·`/readyz` 정확 일치 라우트는 axum fallback과 wire 레벨에서 구별 불가능한
no-op(제거해도 42셀 동일).

**대안(layout이 axum 라우트 문법을 소유)도 기각**: `/api/{*rest}`는 **온디스크 지식이
아니다**. CONTEXT.md가 Layout을 "온디스크 이름·경로 규칙의 단일 소유자"로 정의하므로,
HTTP 라우트 패턴을 layout.rs에 넣는 것은 한 쌍의 누수를 다른 쌍으로 갈아끼우는 것.

## What moves (개정판)

- **라우터의 모양은 그대로 보존한다**(비대칭 = 행위). 다만 라우트 패턴을 public.rs의
  상수 목록으로 뽑아 **기계 판독 가능**하게 만들고, 그 목록을 루프로 등록한다
  (등록되는 라우트 집합·모양·순서 불변 → 행위 보존).
- **정합성 테스트 추가**: `layout::RESERVED_BUCKETS`의 **모든** 예약 이름에 대해
  public 라우터에 그림자 라우트(`/{name}` 정확 일치 또는 `/{name}/…` 서브트리)가
  존재함을 단언. P4-2의 두-목록 드리프트를 **조용한 버그에서 테스트 실패로** 바꾼다.
- 관심사 분리: **이름**은 layout(버킷 명명 규칙), **모양**은 public.rs(HTTP 라우팅).
  테스트가 둘을 묶는다.

## Acceptance (개정판)

- [x] characterization suite green (`cargo test`) — 특히 public.rs 표면 격리
      테스트(전 메서드 404)와 e2e 리스너 격리
- [x] `cargo clippy` green
- [x] no weakening of the characterization tests (anti-cheat)
- [x] **등록되는 라우트 집합·모양이 개정 전과 동일**(위 실측 매트릭스 보존 —
      특히 `/healthz/foo`·`/readyz/foo`의 비-GET **405 + `Allow`** 유지,
      `/api/*`의 전 메서드 404 유지)
- [x] **정합성 테스트가 존재하고, 예약 이름을 추가하면 실패한다**(뮤턴트 실증 필수:
      `RESERVED_BUCKETS`에 이름 하나를 임시로 더해 테스트가 FAIL하는지 확인 후 원복)

## Result

**커밋** `9e867e1` (증분 시작 fixed point `d050f9b`).

**행위 보존 증거**: `cargo test` = **105 passed / 8 suites**(103 + 신규 2 — 추가이지
약화 아님). Spec 축 리뷰가 독립 프로브로 **11경로 × 7메서드 = 77셀**을 개정 전
라우터와 대조 → **전부 동일**. 특히 `PUT/POST/DELETE/OPTIONS /healthz/foo`·`/readyz/foo`의
**405 + `Allow: GET,HEAD`**(폐기된 원안이 404로 뒤집었을 셀)와 `GET /healthz/foo`의
404 JSON, `/api/*` 전 메서드 404(빈 바디) 보존 확인. clippy public.rs 신규 경고 0.

**컨덕터측 2축 리뷰**(fixed point `d050f9b`):
- **Spec 축 clean** — 77/77 셀 동일, 뮤턴트 실증 확인, 정합성 술어의 오탐 없음
  (`/apifoo`가 `api`를 그림자 처리하지 않음 — `subtree = "/{name}/"`의 후행 슬래시가
  접두 충돌을 차단).
- **Standards 축**: hard violation 0. judgement call 4건 →

| # | Finding | Decision |
|---|---|---|
| ① | 정합성 테스트 doc 주석의 근거가 **사실과 다름** — "그림자가 없으면 catch-all로 샌다"고 하나 `/{bucket}/{*key}`는 2세그먼트가 필요해 `/healthz` 단독은 도달 불가. 게다가 이 테스트가 허용하는 **정확 일치** 그림자는 `/healthz/foo`가 `public_download`에 도달하는 걸 막지 못한다(현행 행동) | **Accept** — 주석을 사실대로 정정(무엇을 막고 무엇을 안 막는지). 틀린 근거는 없느니만 못하다 |
| ② | **405 비대칭을 아무 테스트도 핀하지 않음** — `src/`·`tests/` 어디에도 `405`/`Allow` 단언이 없어, 이 관측 가능한 행동을 지키는 게 **doc 주석 하나뿐**이었다. B11의 "코드 리뷰로만 지킴" 면제는 *관측 불가* 불변식(O1/O2)에 대한 것이지 이건 관측 가능 행동 | **Accept(최중요)** — `reserved_route_shape_asymmetry_is_load_bearing` 추가. **"균일하게 정리" 뮤턴트(`/healthz/{*rest}` 추가)가 기존 전 스위트를 통과했음을 실증**하고, 새 테스트가 그걸 죽임을 확인. R-6의 존재 이유를 스위트가 지키게 됨 |
| ③ | 드리프트 가드가 **단방향** — 라우트만 있고 예약명이 없으면 사용자가 만들 수 있는 버킷이 영구 도달 불가(라우트가 가로챔) | **Accept** — `every_shadow_route_names_a_reserved_bucket` 역방향 단언 추가(뮤턴트 실증) |
| ④ | `RESERVED_ROUTES` → `SHADOW_ROUTES` 개명 제안 | **Reject** — 미용적. doc 주석이 이미 "균일하게 펴지 마라" 경고를 담고 있고, 릴리스 게이트 직전 파일을 흔들 이유 없음 |

**컨덕터 판정 1건 추가**: 구현자가 처음 핀한 `OPTIONS /healthz/foo` → 405를 **제외**시켰다.
그 단언은 "공개 origin에 CorsLayer가 없음"에 테스트를 결합시키는데, 나중에 CORS를 붙이면
OPTIONS가 정당하게 달라지며 이는 R-6이 지키려는 성질(라우트 **모양**의 비대칭)과 무관하다.
**무관한 이유로 깨지는 단언은 다음 사람에게 "테스트를 약화시켜라"를 학습시킨다** — 이
파이프라인이 가장 경계하는 것. PUT/POST/DELETE만으로 뮤턴트 킬이 그대로 성립함을 실증.

**Latent bugs 발견(고치지 않음)**: F-21에 통합(예약 경로의 405/404 응답 비대칭 —
`Allow: GET,HEAD`가 라우트 존재를 광고) + F-22(같은 "없음"인데 `/healthz/foo`는 404 JSON,
`/api/x/y`는 404 빈 바디 — 바디 파싱으로 예약 경로 종류 구분 가능).

