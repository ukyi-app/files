---
id: R-5
title: KeyLocks::lock(bucket, key) 2인자 심화 — 락 키 포맷을 locks.rs private으로
status: open
blocked-by: [R-2]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed:
---

## What moves

- `KeyLocks::lock(&str)` → `lock(bucket: &str, key: &str)`;
  `format!("{bucket}/{key}")`는 locks.rs 내부의 유일 저작점이 된다.
- objects.rs 3곳(put/put_stream/delete)의 수기 포맷 소멸 →
  `self.locks.lock(bucket, key)`.
- locks.rs 인라인 단위 테스트는 모듈 인터페이스와 함께 이동(2인자 호출로 조정 —
  B8의 앵커는 상위 adversarial 동시성 테스트, Decision Log P-1 참조).

## Acceptance

- [ ] characterization suite green (`cargo test`) — 특히 adversarial 동시성(같은
      키 직렬화·상이 키 병렬)
- [ ] `cargo clippy` green
- [ ] no weakening of the characterization tests (anti-cheat)
- [ ] `format!("{bucket}/{key}")`가 locks.rs 밖에서 소멸

## Result

