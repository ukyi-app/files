---
id: R-4
title: reconcile의 레이아웃 재유도 소멸 — 워커 + 분류기 + Layout 경로 소비
status: open
blocked-by: [R-1]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed:
---

## What moves

- `collect_referenced`의 수동 2단 순회 → `layout.pointers_all()` 소비
  (`io::Result` `?` 무가공 전파 그대로 — B7; 내용 read/파싱의 조용한 스킵 잔존).
- `.objects` 스캔의 이름 판정(`.gc-pending.json`/`.corrupt`/`.tmp-`/64-hex) →
  `classify_objects_entry` match. **O1 순서 준수**: Reserved는 file_type 조회 전
  continue(예약 이름 무-stat 보존), O2: dir 스킵은 Temp/Blob 처리 앞.
- `.objects`·`.gc-pending.json`·`.corrupt` 경로 저작 → `Layout` 메서드 경유.
- `run_once(root, gc_grace)` pub 시그니처 불변 — 내부에서 `Layout::new`.
- grace/mtime 정책·격리 mechanics·tombstone 로직은 reconcile 소유 그대로.

## Acceptance

- [ ] characterization suite green (`cargo test`) — 특히 심링크-유일-포인터
      referenced:2, mid-stream temp 보존, 골든 트리
- [ ] `cargo clippy` green
- [ ] no weakening of the characterization tests (anti-cheat)
- [ ] reconcile.rs에서 레이아웃 리터럴(`.meta.json`·`.tmp-`·`.objects`·
      `.gc-pending.json`·`.corrupt`·64-hex 판정) 소멸

## Result

