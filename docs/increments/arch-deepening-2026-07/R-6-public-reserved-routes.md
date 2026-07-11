---
id: R-6
title: public 예약 경로 404를 layout::RESERVED_BUCKETS에서 파생(두-목록 종결)
status: open
blocked-by: [R-1]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed:
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

- [ ] characterization suite green (`cargo test`) — 특히 public.rs 표면 격리
      테스트(전 메서드 404)와 e2e 리스너 격리
- [ ] `cargo clippy` green
- [ ] no weakening of the characterization tests (anti-cheat)
- [ ] **등록되는 라우트 집합·모양이 개정 전과 동일**(위 실측 매트릭스 보존 —
      특히 `/healthz/foo`·`/readyz/foo`의 비-GET **405 + `Allow`** 유지,
      `/api/*`의 전 메서드 404 유지)
- [ ] **정합성 테스트가 존재하고, 예약 이름을 추가하면 실패한다**(뮤턴트 실증 필수:
      `RESERVED_BUCKETS`에 이름 하나를 임시로 더해 테스트가 FAIL하는지 확인 후 원복)

## Result

