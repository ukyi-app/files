---
id: R-3
title: listing을 CommitPointerWalk 소비로 전환(수동 DFS 루프 삭제)
status: open
blocked-by: [R-2]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed:
---

## What moves

- `Store::list`의 수동 스택-DFS + 3중 이름 필터 + key 복원 루프 →
  `self.layout.pointers_in_bucket(bucket)?` + `walk.next()` while-let.
- 에러 매핑은 단일 next() 지점 `map_err(AppError::Internal)` (현행 listing이
  walk 계열 io 에러 전부에 균일 적용하던 그 매핑 — B7).
- 읽기/파싱 실패 조용한 스킵·non-servable 제외(blob try_exists)·정렬은
  호출자(listing)에 그대로 잔존.

## Acceptance

- [ ] characterization suite green (`cargo test`) — 특히 심링크·중첩 키·정렬 핀
- [ ] `cargo clippy` green
- [ ] no weakening of the characterization tests (anti-cheat)
- [ ] `.meta.json`·`.tmp-` 리터럴이 listing.rs에서 소멸

## Result

