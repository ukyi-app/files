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
- **atomic.rs: `write_atomic`의 temp 이름 저작(`format!(".tmp-{}", unique_suffix())`,
  atomic.rs:8) → `layout::temp_name(unique_suffix())` 경유** (S-1 수용). 이 writer는
  임의 부모 디렉터리의 형제로 temp를 두므로 root-비의존 이름 저작 API를 쓴다 —
  온디스크 바이트(`.tmp-<unique>`) 불변. 이로써 atomic writer가 seam의 실제 두 번째
  소비자가 되고, `.tmp-` 접두사의 저작점이 layout 하나로 수렴한다(접두사 드리프트 시
  중단된 atomic-write 파일이 `Other`로 분류돼 temp 정리를 회피하는 경로 차단).
- buckets.rs: `.bucket.json` join → `layout.bucket_meta_path()`, list_buckets의
  `.objects` 스킵 → `layout::OBJECTS_DIR`.
- store/mod.rs: `blob_path`(pub 시그니처 불변)·`meta_for` → layout 위임.
- http/state.rs: `.objects` 생성 → Layout 경유.
- store 인라인 테스트의 `s.root` 접근이 있으면 descendant 규칙 내 동등 접근으로
  기계 조정(단언 불변). atomic.rs 인라인 테스트의 `.tmp-` 잔재 단언은 온디스크
  바이트를 핀하므로 raw 리터럴 유지(상수 경유 금지 — 동어반복 방지).

## Acceptance

- [ ] characterization suite green (`cargo test`)
- [ ] `cargo clippy` green
- [ ] no weakening of the characterization tests (anti-cheat)
- [ ] `.objects`·`.tmp-`·`.bucket.json` 리터럴이 src/store/(**atomic.rs 포함**)·
      src/http/state.rs의 **비-테스트 코드**에서 소멸 — layout만 보유(S-1)
- [ ] `atomic::write_atomic`이 `layout::temp_name`의 실제 소비자로 등록(seam 두 번째 소비자)

## Result

