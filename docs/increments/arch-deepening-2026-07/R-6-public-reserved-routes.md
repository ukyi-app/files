---
id: R-6
title: public 예약 경로 404를 layout::RESERVED_BUCKETS에서 파생(두-목록 종결)
status: open
blocked-by: [R-1]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed:
---

## What moves

- public.rs의 수기 3라우트(`/api`·`/healthz`·`/readyz` 404)를
  `layout::RESERVED_BUCKETS` 루프 파생으로 교체 — 동일한 3개 라우트가 등록되므로
  관측 행동 동일(B9가 판정).
- 예약 버킷 지식의 마지막 독립 사본 소멸(P4-2 상호 인용 주석 정리 포함).

## Acceptance

- [ ] characterization suite green (`cargo test`) — 특히 public.rs 표면 격리
      테스트(전 메서드 404)와 e2e 리스너 격리
- [ ] `cargo clippy` green
- [ ] no weakening of the characterization tests (anti-cheat)
- [ ] 예약 이름 리터럴이 public.rs에서 소멸

## Result

