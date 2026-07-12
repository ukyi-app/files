---
id: R-3
title: listing을 CommitPointerWalk 소비로 전환(수동 DFS 루프 삭제)
status: done
blocked-by: [R-2]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed: 2026-07-12
---

## What moves

- `Store::list`의 수동 스택-DFS + 3중 이름 필터 + key 복원 루프 →
  `self.layout.pointers_in_bucket(bucket)?` + `walk.next()` while-let.
- 에러 매핑은 단일 next() 지점 `map_err(AppError::Internal)` (현행 listing이
  walk 계열 io 에러 전부에 균일 적용하던 그 매핑 — B7).
- 읽기/파싱 실패 조용한 스킵·non-servable 제외(blob try_exists)·정렬은
  호출자(listing)에 그대로 잔존.

## Acceptance

- [x] characterization suite green (`cargo test`) — 특히 심링크·중첩 키·정렬 핀
- [x] `cargo clippy` green
- [x] no weakening of the characterization tests (anti-cheat)
- [x] `.meta.json`·`.tmp-` 리터럴이 listing.rs에서 소멸

## Result

**커밋** `c16fb48` (증분 시작 fixed point `47351cc`). 수동 DFS 49줄 삭제 → 워커 소비 21줄.

**행위 보존 증거**: `cargo test` = **101 passed / 8 suites** — baseline 동일(lock testCmd,
컨덕터 직접 실행). clippy 0 errors, 변경 파일 신규 경고 0. 테스트 파일 미접촉(diff에 없음).

**컨덕터측 2축 리뷰**(fixed point `47351cc`):
- **Spec 축 clean** — Blocker/Major/Minor 0. B5 수용집합 등가성을 적대적으로 검증:
  구 술어(`.tmp-`접두 ∨ `== .bucket.json` ∨ ¬`.meta.json`접미 → skip)와 워커
  술어(`is_commit_pointer_name`)를 `.tmp-`·`.bucket.json`·`.meta.json`·`.`·`..`·`/`의
  1~3중 결합 **5,219개 적대적 이름**에 대해 전수 비교 → **불일치 0**. `.bucket.json`
  절의 외연적 공허성 확인(`.bucket.json`의 끝 10자 = `ucket.json` ≠ `.meta.json`이라
  접미 절이 이미 배제). 순회 형태도 동일(LIFO 스택, 반복 중 dir push, 열린 ReadDir
  소진 우선 + 이름 필터 **전에** 전 항목 `file_type()` 조회) → **첫 io 에러의 정체까지 보존**.
  B7 에러 시점(`pointers_in_bucket`은 non-async, 첫 문장이 `valid_bucket` → I/O 이전),
  버킷 dir 부재 시 동일 fs 호출(seed `try_exists`), 재귀(P2-1), 조용한 skip,
  non-servable 제외, 정렬(W3), 심링크 lstat 의미론 전부 보존 확인.
- **Standards 축**: hard violation 0, 스멜 0. judgement call 2건 —
  ① `valid_bucket`/`valid_key`가 layout.rs 밖 **코드 소비자 0**이 됨(R-3이 마지막
  `use`를 제거) → `pub(crate)` 축소 제안. **Reject**(계획서 Decision Log에 근거 기록):
  A-2와 달리 이 둘은 Target shape가 `pub fn`으로 **명시 핀**했고, 순수 검증자라
  CONTEXT.md가 금하는 "Layout 우회 경로 저작"이 원천 불가능하다. 게이트 승인된 핀을
  필요 없이 뒤집지 않는다.
  ② `src/http/internal/files.rs:28`이 R-1에서 삭제된 `src/path.rs`를 doc 주석에서 참조
  → **Accept**, 단 R-3 범위가 아닌 R-1 잔재이므로 별도 커밋 `d764629`로 분리 수정.

**Latent bugs 발견(고치지 않음)**: F-11~F-13으로 파일링(비-UTF-8 파일명 키 손상 ·
손상 meta의 무-신호 소멸 · 하위 디렉터리 1개의 EACCES가 목록 전체를 500으로 실패시킴).

