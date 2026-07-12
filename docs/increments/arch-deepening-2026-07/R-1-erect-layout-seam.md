---
id: R-1
title: src/layout.rs 기립 — path.rs 흡수 + 경로 메서드 + 분류기 + 커밋 포인터 워커
status: done
blocked-by: [none]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed: 2026-07-10
---

## What moves

seam 전체를 한 번에 세운다(소비자 로직 무변경 — 이후 증분이 이 위에 앉는다):

- `src/layout.rs` 신설 — 계획서 Target shape의 인터페이스 그대로:
  - path.rs의 `valid_bucket`/`valid_key`/`safe_object_path`/`meta_path` + 인라인
    테스트를 **단언 불변으로 축자 이주**(이동 ≠ 약화), `RESERVED_SUFFIXES`·
    `RESERVED_BUCKETS`(pub(crate)) 포함. path.rs 삭제.
  - `struct Layout { root }` + making 메서드: `new`·`meta_for`·`blob_path`·
    `objects_dir`·`temp_blob_path`·`bucket_meta_path`·`gc_pending_path`·`corrupt_dir`.
  - `classify_objects_entry(name) -> ObjectsEntry{Reserved,Temp,Blob,Other}`
    (이름-전용 순수 총함수, C1) + `pub(crate) const OBJECTS_DIR`.
  - `CommitPointerWalk`(풀 방식, `io::Result`) + `CommitPointerEntry` +
    `Layout::pointers_in_bucket`(검증은 I/O 전 BadRequest)·`pointers_all`
    (`.objects` 미진입, 루트 직속 dir만 시드) — 불변식 W1(비-디렉터리, lstat
    의미론)~W5 준수.
- layout 자체 단위 테스트 신규: 분류 테이블(`.bucket.json` 공허성 포섭,
  `.tmp-x.meta.json`→Temp 우선순위, 대문자 hex→Blob), making↔reading round-trip
  속성, 워커 tempdir 테스트(중첩 키·tmp 스킵·버킷 부재→빈 결과·`.objects` 미진입).
- `lib.rs`: `pub mod path` → `pub mod layout`.
- `Cargo.toml`: `publish = false` 추가(P-1).
- `crate::path::` 임포트 기계 갱신: store/mod.rs · store/listing.rs ·
  store/buckets.rs (경로만 교체, 로직 무변경).

## Acceptance

- [ ] characterization suite green at this increment's commit (`cargo test`, 94+)
- [ ] increment-local checks green (`cargo clippy`)
- [ ] no weakening of the characterization tests (anti-cheat) — path.rs 테스트는
      단언 불변 이주만
- [ ] 소비자(listing/reconcile/objects/buckets/state) 로직 diff 없음(임포트 제외)

## Result

커밋 dbb9efa. cargo test 100 passed(94 baseline + layout 신규 6) · clippy 신규 경고 0.
/code-review: Spec clean(이주 단언 바이트 동일·W1-W5 구현 확인), Standards 판단성 4건 중
1건(모듈 내 리터럴 중복·unwrap 결합) fix 반영 — 온디스크 리터럴 각 1회 정의로 상수화.
과도기 중복(Layout↔Store/reconcile 경로 저작)은 계획 승인 스캐폴딩 — R-2~R-4가 해소.
