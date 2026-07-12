---
id: R-5
title: KeyLocks::lock(bucket, key) 2인자 심화 — 락 키 포맷을 locks.rs private으로
status: done
blocked-by: [R-2]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed: 2026-07-12
---

## What moves

- `KeyLocks::lock(&str)` → `lock(bucket: &str, key: &str)`;
  `format!("{bucket}/{key}")`는 locks.rs 내부의 유일 저작점이 된다.
- objects.rs 3곳(put/put_stream/delete)의 수기 포맷 소멸 →
  `self.locks.lock(bucket, key)`.
- locks.rs 인라인 단위 테스트는 모듈 인터페이스와 함께 이동(2인자 호출로 조정 —
  B8의 앵커는 상위 adversarial 동시성 테스트, Decision Log P-1 참조).

## Acceptance

- [x] characterization suite green (`cargo test`) — 특히 adversarial 동시성(같은
      키 직렬화·상이 키 병렬)
- [x] `cargo clippy` green
- [x] no weakening of the characterization tests (anti-cheat)
- [x] `format!("{bucket}/{key}")`가 locks.rs 밖에서 소멸

## Result

**커밋** `0f7120e` (증분 시작 fixed point `c62e4ee`).

**행위 보존 증거**: `cargo test` = **102 passed / 8 suites** — 101 + **신규 1**(아래 보강).
감소·약화 0. clippy 변경 파일 신규 경고 0.
- 락 맵의 **키 문자열이 이전과 바이트 동일**: 구버전은 호출부가 `format!("{bucket}/{key}")`를
  만들어 넘기고 `entry(key.to_string())`, 신버전은 `entry(lock_key(bucket, key))`로
  같은 합성식·인자·순서 → 산출 문자열 동일. 이동한 것은 포맷의 **위치**뿐.
- guard 획득 위치·스코프·drop 시점이 3곳(put/put_stream/delete) 모두 불변
  (hunk가 전부 동일 줄번호·net 0줄).
- B8 앵커 `tests/adversarial.rs` **무수정** green(8 passed).

**컨덕터측 2축 리뷰**(fixed point `c62e4ee`):
- **Spec 축 clean** — Blocker/Major/Minor 0. `lock`과 `try_busy`가 단일 private
  composer를 공유해 키 공간이 일치(어긋났다면 동시성 테스트가 공허해졌을 것).
- **Standards 축**: hard violation 0. **Minor 1건 Accept** →

**보강(리뷰 지적 수용)**: R-5가 도입한 `bucket` 인자를 **어떤 테스트도 핀하지 않았다**.
뮤턴트 `lock_key(_bucket, key) = key.to_string()`(버킷 무시)이 locks.rs 3개 테스트와
상위 adversarial 앵커를 **전부 통과**한다 — 앵커의 동시성 테스트가 모두 단일 버킷
(`"skills"`)에서만 돌아 버킷 축을 변별하지 못하기 때문. 게다가 R-5가 기존
`different_keys_independent`의 축을 좁혔다(구: 최상위 락 키 `"a"`/`"b"` 2개 → 신:
동일 버킷 내 2키). → **버킷 축 단언 추가**(다른 버킷의 같은 키 = 별개 락) +
`bucket_participates_in_lock_key` 테스트. **뮤턴트에서 두 단언이 FAIL함을 실증**하고
원복 후 초록 확인. 기존 3개 테스트·단언은 무변경(`assert_eq!(max_seen, 1)` 바이트 동일) —
**약화가 아니라 강화**이므로 anti-cheat 무저촉.

**남은 커버리지 갭(F-17로 파일링)**: 상위 adversarial 층은 여전히 단일 버킷만
동시성 테스트한다 — 호출부가 버킷을 잘못 넘기는 회귀(예: 상수 전달)는 unit 층에서만
잡힌다. 2-버킷 동시성 케이스는 후속.

**Latent bugs 발견(고치지 않음)**: F-18~F-20으로 파일링.

