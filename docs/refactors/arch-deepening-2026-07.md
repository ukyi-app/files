---
refactor: arch-deepening-2026-07
invariant-class: refactor
entry-track: architecture
review-track: full
pipeline-stage: executing
issue-tracker: local
behavior-baseline: c889f75ad3bc222ce4c13c7adfb103e54b87b534
characterization-lock: done
first-increment: [R-1]
structure-gate: done
increments: [R-1, R-2, R-3, R-4, R-5, R-6]
spike-1:
---

# Layout 소유 모듈 — on-disk 이름·경로 규칙의 응집 (행위 보존)

## Current shape (the problem)

on-disk 컨벤션의 소유 모듈이 없다. 문자열 지식이 6개 파일에 축자 중복된다
(discover에서 grep 검증, `docs/reviews/arch-deepening-2026-07/state.md`의
deletion-test evidence 절):

- `.meta.json` — path.rs:4,51 · listing.rs:32,39 · reconcile.rs:49
- `.tmp-` — atomic.rs:8 · objects.rs:72 · listing.rs:32 · reconcile.rs:49,105
- `.objects` — store/mod.rs:32 · http/state.rs:17 · reconcile.rs:32,68 · objects.rs:68 · buckets.rs:37
- `.bucket.json` — buckets.rs:10,19 · path.rs:4 · listing.rs:32
- 락 키 `format!("{bucket}/{key}")` — objects.rs:22,66,154
- `.gc-pending.json` · `.corrupt` · 64-hex 블롭명 — reconcile.rs만 인지

reconcile은 store가 정의한 레이아웃 전체를 독자 재유도하고(합의를 지키는
인터페이스 부재), 재귀 순회+커밋 포인터 필터+key 복원의 ~20줄 루프가
listing.rs:21-41과 reconcile::collect_referenced:39-58에 중복된다. deletion test:
이 흩어진 지식을 지우면 6곳에서 재출현 → 응집이 depth를 번다. path.rs는 이름
정책의 절반(검증·예약 접미사)만 소유한 shallow 모듈 — `RESERVED_SUFFIXES`는
검증 규칙이자 레이아웃 지식인데 두 관심사가 분리돼 있다.

HTML 리포트(discover 산출물, scratchpad): architecture-review-20260710-133341.html
후보 1 · Strong.

## Target shape (the deepening)

**seam**: crate 레벨 `src/layout.rs` — path.rs를 흡수(삭제)하고, 이름을 만들
줄도(검증·경로 계산) 읽을 줄도(분류·워커) 아는 단일 소유자가 된다(CONTEXT.md
"Layout"). DESIGN-IT-TWICE 3안(최소/유연/호출자-우선) 비교 후 하이브리드 C+A
승자 — 인간 확정 2026-07-10.

```rust
// ── 검증 (root 무관 순수 fn — path.rs에서 이주, 시그니처 불변) ──
pub fn valid_bucket(b: &str) -> Result<(), AppError>;   // invalid_bucket
pub fn valid_key(k: &str) -> Result<(), AppError>;      // invalid_key | reserved_suffix
pub(crate) const RESERVED_BUCKETS: &[&str];             // R-6이 public 라우트 파생에 사용
pub(crate) const OBJECTS_DIR: &str;                     // buckets.rs 루트 스킵 1곳용
pub(crate) fn temp_name(unique: &str) -> String;        // ".tmp-<unique>" — root 무관 이름 저작
                                                        // (S-1) atomic::write_atomic처럼 임의 부모의
                                                        // 형제로 temp를 두는 소비자용. temp 접두사의 유일 저작점.

// ── 경로 만들기 (값 타입 — root를 한 번 묶음) ──
#[derive(Clone)] pub struct Layout { root: PathBuf }
impl Layout {
    pub fn new(root: PathBuf) -> Self;
    pub fn meta_for(&self, bucket, key) -> Result<PathBuf, AppError>; // 검증 포함
    pub fn blob_path(&self, sha: &str) -> PathBuf;        // root/.objects/<sha>
    pub fn objects_dir(&self) -> PathBuf;
    pub fn temp_blob_path(&self, unique: &str) -> PathBuf; // root/.objects/ + temp_name(u) 위임
    pub fn bucket_meta_path(&self, bucket) -> Result<PathBuf, AppError>;
    pub fn gc_pending_path(&self) -> PathBuf;
    pub fn corrupt_dir(&self) -> PathBuf;

    // ── 이름 읽기 1: 커밋 포인터 워커 (지배 패턴 흡수) ──
    pub fn pointers_in_bucket(&self, bucket) -> Result<CommitPointerWalk, AppError>;
    pub fn pointers_all(&self) -> CommitPointerWalk;      // .objects 서브트리 미진입
}
pub struct CommitPointerEntry { pub bucket: String, pub key: String, pub meta_path: PathBuf }
pub struct CommitPointerWalk { /* private: DFS 스택 + ReadDir */ }
impl CommitPointerWalk { pub async fn next(&mut self) -> io::Result<Option<CommitPointerEntry>>; }

// ── 이름 읽기 2: .objects 항목 분류 (이름-전용 순수 총함수) ──
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectsEntry { Reserved /*.gc-pending.json|.corrupt*/, Temp, Blob, Other }
pub fn classify_objects_entry(name: &str) -> ObjectsEntry;
```

**인터페이스 불변식**(설계안에서 승계, layout 단위 테스트로 고정):

- W1 워커 yield 집합 = {**비-디렉터리 항목**(DirEntry `file_type().is_dir()==false`,
  lstat 의미론 — 심링크 포함; `is_file()` 아님) ∧ ¬`.tmp-` 접두 ∧ `.meta.json` 접미}
  정확히 전부·그것만 — 현행 분기 그대로(P-2 수용: 심링크 characterization으로 핀).
  술어 자체는 비공개.
- W2 `meta_path == root/bucket/key + ".meta.json"` (making의 역). 워커는 분류이지
  재검증이 아님 — 디스크의 비정상 이름도 현행대로 그대로 냄.
- W3 yield 순서 계약상 비보장(구현은 현행 LIFO-DFS 유지; 호출자는 정렬/HashSet).
- W4 워커는 낸 파일을 열지 않음. fs 호출 종류·순서 현행 동일(read_dir/next_entry/
  file_type/(단일 버킷) try_exists).
- W5 fused: Err 후 next()는 Ok(None) (두 호출자 모두 첫 Err 탈출이라 관측 불가).
- C1 ObjectsEntry 4변종은 이름 공간 분할(총함수, 분류 순서 무의존).
- O1 (호출자 계약, 문서화) `.objects` 스캔에서 Reserved는 file_type 조회 **전**
  continue — 예약 이름 무-stat 현행 보존. O2 dir 스킵은 Temp/Blob 처리 앞.
- 에러 모드: pointers_in_bucket은 I/O 전 BadRequest("invalid_bucket")만; walk는
  io::Error 무가공; classify는 불가침(총함수).

**의존성 카테고리**: 검증·경로·분류 = **in-process pure**(문자열/경로 계산,
인터페이스가 곧 테스트 표면). 워커 = **local-substitutable**(tokio::fs 유일 I/O,
로컬 대역 = tempdir — 기존 dev-dep·프로젝트 관행). fs port/trait 도입 금지
(adapter 1개 = hypothetical seam).

**seam이 실재하는 이유**: adapter 다중성이 아니라 **소비자 다중성 + deletion
test**다 — 같은 인터페이스를 store 내부 4파일·reconcile·http/state가 건너고(≥6),
모듈을 지우면 문법+우선순위+재귀 전략이 그 전부에 재출현한다. 대체 가능성이
아닌 지식 응집(locality)이 이 seam의 존재 근거.

**의도적 미구축**(YAGNI 경계 — B안의 기여): BlobName newtype, 사영 술어(is_*),
classify_root_entry, locate(), fs port, futures::Stream 구현. 소비자 0 =
hypothetical interface.

**기확정 결정**(grilling, state.md): KeyLocks::lock(bucket,key) 2인자 심화(포맷은
locks.rs private — on-disk 지식 아님); 기존 pub 시그니처(Store::blob_path,
reconcile::run_once(root,…))는 얇은 delegate로 보존 → 기존 테스트 무수정.
lib.rs의 `pub mod path`는 제거(→ `pub mod layout`) — crate 외부 소비자 0
(tests/·clients/ grep 검증, 2026-07-10). B10의 "보존"은 소비되는 표면(테스트가
실제로 건너는 시그니처) 기준이다.

## Behavior Contract

증분 전 과정에서 변하면 안 되는 관측 행동. characterization이 핀하고 모든
게이트가 이 목록으로 판정한다.

| # | 관측 행동 | 핀 위치 |
|---|---|---|
| B1 | HTTP 계약 전체(내부/공개 표면: 상태코드·헤더·에러 code 문자열·바디) | src/http/*tests · tests/adversarial · tests/e2e |
| B2 | OpenAPI 문서 불변 | tests/openapi.rs · tests/contract.rs |
| B3 | on-disk 이름 규칙: `<bucket>/<key>.meta.json` · `.bucket.json` · `.objects/<sha256hex>` · `.objects/.tmp-*` · `.objects/.gc-pending.json` · `.objects/.corrupt/` | **신규 골든 레이아웃 트리 테스트** + store/tests |
| B4 | 파일 내용 형식(meta JSON camelCase 등) | src/meta.rs tests · tests/contract.rs |
| B5 | list 의미: 수용집합 {¬`.tmp-`접두 ∧ `.meta.json`접미}(listing의 `.bucket.json` 절은 외연상 공허 — 접미사 불일치로 이미 배제됨을 layout 테이블 테스트로 증명·고정) · 중첩 키 재귀 · non-servable 제외 · 키 정렬 | store/tests · adversarial · layout 단위 테스트(R-1) |
| B6 | reconcile 판정: temp grace 보존/삭제 · 2단계 tombstone GC · 내용≠이름 격리(대문자 hex 이름도 Blob 분류 후 내용 검증서 격리 — 현행 보존) · 예약 이름 불가침 | reconcile tests · adversarial |
| B7 | 에러 표면: listing=AppError(Internal 균일 매핑+읽기/파싱 실패 조용한 스킵), reconcile=io::Result 무가공 전파, valid_* 에러코드(invalid_bucket/invalid_key/reserved_suffix)와 발생 시점(I/O 전) | path tests(→layout으로 이주) · adversarial 400 modes |
| B8 | 같은 bucket/key 쓰기 직렬화, 상이 키 병렬 | locks tests · adversarial 동시성 |
| B9 | 공개 표면 예약 경로(/api·/healthz·/readyz → 404, 전 메서드) | public.rs tests · e2e |
| B10 | lib pub 시그니처 불변(Store::blob_path · reconcile::run_once(root,…) 등) — "보존"은 **소비되는 표면** 기준(외부 Rust 소비자 0: crates.io 미발행 + R-1의 `publish = false`로 기계 보증). 대상 모듈 자신의 단위 테스트(locks.rs 등)는 모듈 인터페이스와 함께 이동하며, B8의 앵커는 상위 adversarial 동시성 테스트다(P-1 수용) | 컴파일 타임(소비-표면 테스트 무수정 원칙) |
| B11 | (관측-불가 설계 제약, 테스트 핀 없음) 예약 이름 stat-전 스킵 순서(O1) · yield 순서 비보장(W3) — 코드 리뷰로만 지킴 | — |

## Characterization plan

**래더 rung (a)** — 현 인터페이스(Store pub API·HTTP 라우터·reconcile::run_once·
tempdir fs 관측)가 그대로 테스트 가능. 앵커는 전부 리팩터 대상 **위**의 불변
seam.

- **lock testCmd**: `cargo test` (전체 스위트 — baseline aa854ef에서 94 passed,
  8 스위트).
- **신규** `tests/layout_tree.rs` (Store pub API + `reconcile::run_once`만 사용,
  내부 미접근):
  1. 골든 레이아웃 트리 — 스크립트된 연산(put_bucket → put → put_stream 중첩 키 →
     delete → reconcile) 후 tempdir 상대 경로 전체를 정렬 스냅샷으로 정확 단언.
     B3을 한 곳에서 직접 핀. 이름은 결정적(sha·고정 문자열), timestamps 미포함.
  2. 심링크 커밋 포인터 현행 행동(P-2 수용) — lstat 비-디렉터리 통과·read 링크
     추종·dangling 조용한 제외를 list/reconcile 양쪽에서 핀.
  3. 업로드 중 temp 관측(P-3 수용) — put_stream을 채널 스트림으로 중간 정지시켜
     라이터 생성 `.objects/.tmp-*` 정확히 1개 관측 → grace 내 reconcile 보존 →
     스트림 에러 시 stream_error + 잔재 0. temp 접두사 변경 회귀를 잡는다.
- **앵커 주의**(characterize.md 경고 대응): path.rs 인라인 테스트는 흡수 예정
  모듈에 사는 핀 → R-1에서 **단언 불변으로 layout.rs 테스트로 축자 이주**(이동
  ≠ 약화). 같은 행동의 독립 상위 핀이 존재: adversarial HTTP 400 modes(B7),
  골든 트리(B3). lock이 삭제 예정 모듈에만 사는 행동은 없음.
- 리팩터 전 코드에서 초록 확인 후 커밋 → `characterization-lock.json` +
  `behavior-baseline` 설정. 골든 값 재기록 금지(anti-cheat).

## Increment plan

| id | what moves | blocked-by | notes |
|---|---|---|---|
| R-1 | `src/layout.rs` 신설: path.rs 흡수(fn·테스트 축자 이주) + Layout 경로 메서드 + classify_objects_entry + 상수 + CommitPointerWalk 워커 + 자체 단위 테스트(분류 테이블·round-trip 속성·워커 tempdir). path.rs 삭제, `crate::path::` 임포트 기계 갱신. Cargo.toml `publish = false` 명시(P-1 — 외부 소비자 부재의 기계 보증). 소비자 로직 무변경 | none | **first-increment** — seam 전체 기립, 자체 검증 포함 |
| R-2 | Store가 `layout: Layout` 보유(root 이중화 제거), making-side 소비: objects.rs(blob·temp 경로) · **atomic.rs(`write_atomic`의 temp 이름 → `layout::temp_name` — S-1)** · buckets.rs(bucket_meta_path·OBJECTS_DIR) · store/mod.rs(blob_path 위임·meta_for 위임) · http/state.rs(.objects 생성 Layout 경유) | R-1 | atomic writer = seam의 두 번째 소비자 |
| R-3 | listing.rs → pointers_in_bucket 워커 소비(수동 DFS 루프 삭제) | R-2 | 에러 매핑은 단일 next() 지점 map_err(AppError::Internal) |
| R-4 | reconcile: collect_referenced → pointers_all, `.objects` 스캔 → classify_objects_entry(O1 순서 준수), gc/corrupt/objects 경로 Layout 경유. run_once(root,…) 시그니처 불변 | R-1 | reconcile의 레이아웃 재유도 소멸 |
| R-5 | KeyLocks::lock(bucket,key) 2인자 심화 — 포맷 locks.rs private, objects.rs 3곳 갱신 | R-2 | 같은 파일(objects.rs) 충돌 회피용 순서 |
| R-6 | public.rs 예약 경로 404를 layout::RESERVED_BUCKETS에서 루프 파생 | R-1 | 두-목록 드리프트 종결(B9가 판정) |

각 증분: 독립적으로 행위 보존, lock testCmd 초록 유지 → 커밋.

## Follow-up backlog

| id | 항목 | 라우팅 |
|---|---|---|
| F-1 | reconcile GC↔put-dedup 레이스(참조 스냅샷 후 dedup 커밋된 블롭 GC 가능 — reconcile.rs:74,135-139 vs objects.rs:26-29,83-86) | gated-bugfix |
| F-2 | HEAD 발산: Last-Modified 누락·If-None-Match/Range 무시(files.rs:189-203) | gated-bugfix(플립 다수면 pipeline) |
| F-3 | Conflict(409) dead variant(error.rs:19) | 별도 정리 |
| F-4 | 시계 역행 시 temp-age 0 → 정리 지연(reconcile.rs:107, 보존 방향이라 안전) | gated-bugfix(관찰) |
| F-5 | 구성 seam: build_state를 clock·free-space adapter로 파라미터화(리포트 후보 2, Strong) | 차기 gated-refactor |
| F-6 | authz seam(후보 3) · 쓰기 경로 이중화(후보 4) · 응답 헤더 응집(후보 5) · OpenAPI C3(후보 6) | 후속 후보 풀 |
| F-7 | 성능 후보군: dedup 전체 재독 · blocking statvfs · 카탈로그 N+1 · reconcile 전량 스캔 · KeyLocks 무한 성장 | gated-perf(metric 선언 시) |
| F-8 | buckets.rs list_buckets의 `.objects` 스킵 — classify_root_entry는 미구축 결정(소비자 1), OBJECTS_DIR 상수로 충족 | 기록만 |

## Review Decision Log

### Codex Plan Review — r1 (verdict: needs-attention · docs/reviews/arch-deepening-2026-07/plan-r1.json)
| ID | Finding | Severity | Decision | Reason | Action |
|----|---------|----------|----------|--------|--------|
| P-1 | Simpler alternative: 기존 모듈을 호환 파사드 뒤에서 심화 | critical | Accept(완화 수정) | 위험(공개 표면 파손)은 실재하나 이 crate는 미발행 애플리케이션 crate(외부 Rust 소비자 0) — 올바른 완화는 파사드가 아니라 기계 보증. 파사드/`lock(&str)` 병존은 release 게이트의 dead 1-adapter seam 렌즈와 충돌하므로 그 치유책은 기각 | R-1에 Cargo.toml `publish = false` 추가, B10을 소비-표면 기준으로 재서술(locks.rs 단위 테스트의 모듈 동반 이동 명문화) |
| P-2 | W1이 현재 순회를 '정규 파일'로 잘못 좁힘 | critical | Accept | 현행 의미론은 lstat 비-디렉터리(심링크 포함)가 맞음 — W1 문구가 구현자를 `is_file()`로 오도할 수 있었음 | W1 재서술(비-디렉터리·현행 분기 그대로) + 심링크 characterization 테스트 추가·커밋(aa854ef) |
| P-3 | lock이 R-2가 바꾸는 임시 경로를 관측하지 않음 | high | Accept | 실제 갭 — temp 접두사가 바뀌어도 기존 스위트는 초록으로 남아 중단-업로드 정리 파손을 놓침 | mid-stream temp 관측/보존/정리 테스트 추가·커밋(aa854ef), lock 갱신(baseline aa854ef·94 green, ec86899) |

### Codex Plan Review — r2: needs-attention (escalated → 인간이 수동 r3 승인)
| ID | Finding | Severity | Decision | Reason | Action |
|----|---------|----------|----------|--------|--------|
| P-4 | P-2 reconciliation characterization이 심링크에 의존하지 않음(동어반복) | critical | Accept (인간 승인, 수동 r3) | real 포인터가 같은 sha를 참조해 reconcile 단언이 심링크 무시 회귀를 변별 못 함 — 데이터 손실급 회귀를 lock이 놓치는 구멍 | 심링크를 유일 포인터로 재구성(referenced:2·gc_pending:0·블롭 생존 단언), 커밋 c889f75, lock 갱신(baseline c889f75·94 green) |

### Codex Structure Review — r1 (verdict: needs-attention · docs/reviews/arch-deepening-2026-07/structure-r1.json)
| ID | Finding | Severity | Decision | Reason | Action |
|----|---------|----------|----------|--------|--------|
| S-1 | temp-경로 seam이 atomic writer를 흡수할 수 없음(atomic.rs:8이 미이관 `.tmp-` 생산자 — R-2 acceptance 원리적 불충족, 접두사 드리프트 시 중단된 atomic-write 파일이 temp 정리 회피) | high (Blocker) | Accept | 계획의 스미어 목록이 atomic.rs:8을 열거해 놓고 어떤 증분도 이관하지 않은 진짜 구멍 — Layout이 "온디스크 이름의 단일 소유자"라는 헌장을 못 지킴 | R-1 seam 보강: layout에 root-비의존 `temp_name(unique)` 추가(`.tmp-` 유일 저작점), `temp_blob_path`가 위임; R-2에 atomic.rs 이관 항목 + acceptance 추가(atomic writer = seam 두 번째 소비자); 계획 인터페이스·증분표 갱신 → structure r2 |

### Codex Structure Review — r2: clean — verdict approve, 0 findings, reviewedSha bcd86ce. "S-1 is resolved: temp_name centralizes prefix authoring, temp_blob_path delegates to it, and R-2 now requires atomic.rs as the real second consumer. Characterization tests were not weakened, and no new critical issue was introduced."
(Codex 주: 샌드박스가 read-only라 cargo test를 독립 재실행하지 못함 — machine-owns-GREEN 원칙대로 lock testCmd 실행은 컨덕터 몫이며, 아래 구조-게이트 후 재검증에서 101 green 확인. verification 단계가 다시 전량 재실행한다.)

### Codex Plan Review — r3: clean — verdict approve, 0 findings. "P-4 is resolved. The symlink is now the sole metadata pointer to a distinct blob, so ignoring symlinks changes referenced from 2 to 1 and gc_pending from 0 to 1."
