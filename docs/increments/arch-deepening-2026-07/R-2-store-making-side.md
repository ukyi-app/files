---
id: R-2
title: Store making-side를 Layout 소비로 전환(root 이중화 제거)
status: done
blocked-by: [R-1]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed: 2026-07-12
---

## What moves

- `Store`가 `layout: Layout` 보유(`root: PathBuf` 필드 제거 — Store::new(root)
  시그니처는 불변, 내부에서 Layout::new).
- **layout.rs: `pub(crate) fn root(&self) -> &Path` 추가(계획 개정 A-1, 2026-07-12
  인간 확정)**. root 필드 제거 후에도 루트 경로 자체가 필요한 소비자 2곳을 위한
  베이스 디렉터리 노출 — 경로 저작이 아니므로 온디스크 이름 규칙의 단일 소유는 불변.
- objects.rs: `.objects` join·`.tmp-` 이름 저작 → `layout.objects_dir()`/
  `layout.temp_blob_path(unique_suffix())` 경유.
- **atomic.rs: `write_atomic`의 temp 이름 저작(`format!(".tmp-{}", unique_suffix())`,
  atomic.rs:8) → `layout::temp_name(unique_suffix())` 경유** (S-1 수용). 이 writer는
  임의 부모 디렉터리의 형제로 temp를 두므로 root-비의존 이름 저작 API를 쓴다 —
  온디스크 바이트(`.tmp-<unique>`) 불변. 이로써 atomic writer가 seam의 실제 두 번째
  소비자가 되고, `.tmp-` 접두사의 저작점이 layout 하나로 수렴한다(접두사 드리프트 시
  중단된 atomic-write 파일이 `Other`로 분류돼 temp 정리를 회피하는 경로 차단).
- buckets.rs: `.bucket.json` join → `layout.bucket_meta_path()`, list_buckets의
  `.objects` 스킵 → `layout::OBJECTS_DIR`, 루트 `read_dir` → `self.layout.root()`(A-1).
- listing.rs: `self.root.join(bucket)` → `self.layout.root().join(bucket)`로만 기계
  치환(A-1). **이 파일의 나머지(수동 DFS 루프·이름 필터 리터럴)는 R-3 범위 — 손대지 말 것.**
- store/mod.rs: `blob_path`(pub 시그니처 불변)·`meta_for` → layout 위임.
- http/state.rs: `.objects` 생성 → Layout 경유.
- store 인라인 테스트의 `s.root` 접근이 있으면 descendant 규칙 내 동등 접근으로
  기계 조정(단언 불변). atomic.rs 인라인 테스트의 `.tmp-` 잔재 단언은 온디스크
  바이트를 핀하므로 raw 리터럴 유지(상수 경유 금지 — 동어반복 방지).

## Acceptance

- [x] characterization suite green (`cargo test`)
- [x] `cargo clippy` green
- [x] no weakening of the characterization tests (anti-cheat)
- [x] `.objects`·`.tmp-`·`.bucket.json` 리터럴이 **이 증분의 대상 파일**
      (`src/store/mod.rs` · `objects.rs` · **`atomic.rs`** · `buckets.rs` ·
      `src/http/state.rs`)의 **비-테스트 코드**에서 소멸 — layout만 보유(S-1).
      `listing.rs`(R-3)·`reconcile.rs`(R-4)의 리터럴은 **이 증분 범위 밖 — 남겨둘 것**
- [x] `atomic::write_atomic`이 `layout::temp_name`의 실제 소비자로 등록(seam 두 번째 소비자)

## Result

**커밋** `d562370` (증분 시작 fixed point `ca6229e`).

**행위 보존 증거**: `cargo test` = **101 passed / 8 suites** — baseline과 정확히 동일
(lock testCmd, 컨덕터가 직접 실행). `cargo clippy --all-targets` = 0 errors,
경고 5건 전부 기존 코드(capacity.rs·error.rs·ranged.rs — 범위 밖), **변경 파일
신규 경고 0**. 재작성된 표현의 온디스크 바이트 동등성 확인: `temp_name(u)` ≡
기존 `format!(".tmp-{u}")` · `temp_blob_path(u)` ≡ `root/.objects/.tmp-<u>` ·
`Layout::meta_for` ≡ 기존 `meta_path(safe_object_path(root,b,k)?)` ·
`objects_dir()` ≡ `data_dir/.objects`.

**컨덕터측 2축 리뷰**(fixed point `ca6229e`):
- **Spec 축 clean** — Blocker/Major 0. 누락·스코프 크립·구현 드리프트 없음.
  `buckets.rs`의 명시적 `valid_bucket(bucket)?` 가드 제거가 행위 동등임을
  개별 검증: `bucket_meta_path`가 경로 저작 **전**에 동일한 `valid_bucket`을
  호출하므로 `put_bucket`/`get_bucket`은 같은 `BadRequest("invalid_bucket")`을
  같은 시점(I/O 이전)에 내고, `list_buckets`가 루트 자식마다 부르는 `get_bucket`도
  예약·은닉·과장 이름을 동일하게 `Err`로 만들어 `if let Ok(bm)`에서 동일하게
  삼켜진다(B7 보존, syscall 증감 0, 정렬 순서 불변).
- **Standards 축**: S-2 **Accept·수정 완료**(아래), 문서 드리프트 1건 Accept(A-1
  소비자 수 2→3 정정), `Store::meta_for` 사적 위임 Middle Man 스멜 1건 Reject
  (호출부 5곳 간결성 유지가 이득, 리뷰어도 coin-flip 표기).

**S-2 (Standards, Accept)** — R-2가 `safe_object_path`·`meta_path`의 마지막 외부
소비자를 제거해 이 둘이 **소비자 0인 `pub` 경로-저작 함수**로 남았다(Layout을
우회하는 공개 통로 = CONTEXT.md 위반 + Target shape 자유함수 목록 미포함 + 계획서
YAGNI 규칙). → `pub(crate)`로 축소(**계획 개정 A-2**). B10 무저촉(보존은 소비되는
표면 기준, 외부 Rust 소비자 0 — `publish = false`). private이 아닌 이유: R-4의
crate 내부 사용 여지.

**계획 개정 A-1**(dispatch 전 발견, 인간 확정): `Store::root` 제거 시 `buckets.rs`
루트 열거(영구)와 `listing.rs` bucket_dir(R-3에서 소멸)이 root 경로를 잃어 컴파일
불가 — plan/structure 게이트가 놓친 공백. `Layout::root()` 접근자 추가로 해소.
root는 온디스크 이름 규칙이 아니라 config 값이라 seam 취지 불변, 제거 대상인
이중화만 소멸.

**Latent bugs 발견(고치지 않음 — 백로그 라우팅 대상)**: ① `Store::head`가 파싱
불가 meta JSON을 `NotFound`로 매핑해 진짜 손상을 404로 은폐하고, reconcile은 손상
**포인터**를 격리하지 않아(블롭만 격리) 해당 키가 영구히 비가시. ② `list_buckets`가
루트 `read_dir` 에러를 `Ok(vec![])`로 삼켜 권한 오류가 "버킷 0개"로 보고됨.
둘 다 현행 스위트가 핀하는 기존 행동이라 이 리팩터에서는 보존된다.

