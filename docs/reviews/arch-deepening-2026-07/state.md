---
refactor: arch-deepening-2026-07
invariant-class: refactor     # Rule 0 answered: behavior preserved, structural deepening, no metric, not breadth
entry-track: architecture     # Rule 0 answered: behavior does NOT change
review-track: full
pipeline-stage: plan-gate     # intake | discover | design | ... (source of truth는 이제 docs/refactors/<slug>.md)
issue-tracker: local
candidate: Layout 소유 모듈 — on-disk 컨벤션(경로·이름 계산)의 6파일 산포를 하나의 깊은 모듈로 응집 (인간 선택, 2026-07-10)
intake-grill: done            # "done" after discover's grilling — design runs capture-only
spike-1:                      # <path>@pending | @done | @deleted
---

## Track note

사용자 요청: "전체적으로 아키텍쳐 및 성능 등 개선할점을 찾고 진행하자" — 후보 미정
상태로 architecture 트랙 진입. discovery(improve-codebase-architecture)가 deletion-test
후보들을 제시하고 인간이 하나를 고른다. 고른 후보가 선언 가능한 beatable metric을
가지면 그 시점에 gated-perf로 재라우팅한다(무metric이면 이 파이프라인 유지).

→ discover 완료(2026-07-10): Explore 3방향(store 코어/http/testability) + grep 검증,
후보 7개 HTML 리포트 제시, 인간이 후보 1(Layout 소유 모듈)을 선택. 선택 후보는
metric 없음 — refactor 클래스 유지 확정.

## Deletion-test evidence (discover, 2026-07-10)

on-disk 레이아웃 지식의 소유 모듈이 없어 문자열 컨벤션이 6파일에 축자 중복(grep 검증):

- `.meta.json` — path.rs:4,51 · listing.rs:32,39 · reconcile.rs:49
- `.tmp-` — atomic.rs:8 · objects.rs:72 · listing.rs:32 · reconcile.rs:49,105
- `.objects` — store/mod.rs:32 · http/state.rs:17 · reconcile.rs:32,68 · objects.rs:68 · buckets.rs:37
- `.bucket.json` — buckets.rs:10,19 · path.rs:4 · listing.rs:32
- 락 키 `format!("{bucket}/{key}")` — objects.rs:22,66,154 (KeyLocks가 포맷을 소유하지 않음)
- `.gc-pending.json` · `.corrupt` · 64-hex 블롭명 — reconcile.rs:115,77,84만 인지

reconcile은 store가 정의한 레이아웃 전체를 독자 재유도 — 둘의 합의를 지키는
인터페이스가 없다. 흩어진 지식을 삭제한다고 상상하면 6곳에서 재출현 → 응집이
깊이(depth)를 번다. 온디스크 바이트 불변이므로 행동 보존 자명.

부수 발견(파일링, 이 파이프라인에서 수리 금지 — hard rule 10): reconcile GC↔put-dedup
레이스(reconcile.rs:74,135-139 vs objects.rs:26-29,83-86), HEAD 헤더 발산(files.rs:189-203),
Conflict(409) dead variant(error.rs:19), 시계 역행 시 temp-age 0(reconcile.rs:107).

## Grilling 결정 (2026-07-10, 인간 확정)

1. seam 위치: crate 레벨 `src/layout.rs`, path.rs 흡수(검증+물리 레이아웃+이름 분류).
   근거: RESERVED_SUFFIXES가 검증 규칙이자 레이아웃 지식 — 분리 유지 시 스미어 2곳 잔존.
2. 인터페이스: `struct Layout { root }` 값 타입 + 같은 모듈의 root 무관 순수 fn.
   근거: root 전달 방식 3종(Store.root/reconcile 파라미터/cfg.data_dir) 통일, 혼용 차단.
3. 락 키 포맷: `KeyLocks::lock(bucket, key)` 2인자로 심화 — 포맷은 locks.rs private
   (on-disk 지식 아님 → Layout에 두지 않음).
4. pub 표면: 기존 시그니처(Store::blob_path, reconcile::run_once(root,…)) 얇은
   delegate로 보존 — 테스트 무수정 = characterization 표면 불변.
5. 스코프 포함: public.rs 예약 라우트 404를 RESERVED_BUCKETS에서 파생(마지막 증분).
6. 행동 잠금: 전체 cargo test + 골든 레이아웃 트리 characterization 테스트 신규
   (스크립트된 연산 후 상대 파일트리 정확 단언; 리팩터 전 코드에서 초록 확인 후 커밋).

열린 설계 질문(design 단계 DESIGN-IT-TWICE): 이름 분류 인터페이스 형태 —
술어 묶음 vs 단일 분류 enum(EntryKind) vs 제3안.

도메인 모델: CONTEXT.md 생성(Layout 등 용어 등재) — 이 커밋에 포함(one-writer,
pre-plan-gate, base 브랜치 — hard rule 9 충족).
