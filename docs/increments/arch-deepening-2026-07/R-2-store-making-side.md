---
id: R-2
title: Store making-side를 Layout 소비로 전환(root 이중화 제거)
status: open
blocked-by: [R-1]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed:
---

## What moves

- `Store`가 `layout: Layout` 보유(`root: PathBuf` 필드 제거 — Store::new(root)
  시그니처는 불변, 내부에서 Layout::new).
- objects.rs: `.objects` join·`.tmp-` 이름 저작 → `layout.objects_dir()`/
  `layout.temp_blob_path(unique_suffix())` 경유.
- buckets.rs: `.bucket.json` join → `layout.bucket_meta_path()`, list_buckets의
  `.objects` 스킵 → `layout::OBJECTS_DIR`.
- store/mod.rs: `blob_path`(pub 시그니처 불변)·`meta_for` → layout 위임.
- http/state.rs: `.objects` 생성 → Layout 경유.
- store 인라인 테스트의 `s.root` 접근이 있으면 descendant 규칙 내 동등 접근으로
  기계 조정(단언 불변).

## Acceptance

- [ ] characterization suite green (`cargo test`)
- [ ] `cargo clippy` green
- [ ] no weakening of the characterization tests (anti-cheat)
- [ ] `.objects`·`.tmp-`·`.bucket.json` 리터럴이 src/store/·src/http/state.rs에서 소멸(layout만 보유)

## Result

