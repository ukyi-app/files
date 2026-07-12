---
id: B-3
title: 위생·관측성·문서 — tracing · Drop poison 봉인 · ADR 0002 · 롤백 런북 · Graved 봉인 체크리스트 (행동 무변경)
status: open
blocked-by: [B-2]
plan: docs/bugfixes/reconcile-gc-dedup-race.md
created: 2026-07-13
closed:
---

# B-3 — 위생 · 관측성 · 문서 (**행동 무변경**)

> ## ⚠ 이 문서는 **자기완결적**이다
>
> 계획서(`docs/bugfixes/reconcile-gc-dedup-race.md`, 2646줄)를 **열지 않아도** 이 증분을 완수할 수 있다 —
> 필요한 설계·근거·문구·acceptance가 **전부 아래에 축자 발췌**돼 있다.
> 계획서는 **Codex plan gate r8에서 approve · 0 findings**를 받은 확정 설계이며, 이 문서와 어긋나는 것이
> 있으면 **계획서가 정본**이다.
>
> ## ⚠⚠ **격리(quarantine) 분기는 현행 그대로다 — 손대지 마라**
>
> 이 증분의 acceptance에는 **"격리 분기 diff 0줄"**이 **명시 항목**으로 들어 있다.
> 격리 분기를 "고치는" 것은 **두 번째 관측 행동 플립**이며 **하드룰 10 위반**이다 → **F-25로 분리**(§5).

---

## 0. 공통 계약 — 반드시 먼저 읽어라

### 0.1 불변식 — 뒤집히는 관측 행동은 **정확히 하나**다

그 하나는 **B-2가 이미 뒤집었다.** **B-3은 관측 행동을 하나도 바꾸지 않는다.**

> reconcile 패스 P가 blob X를 GC 삭제 후보로 확정했을 때(무참조 ∧ tombstone 만료), P는 X를 무덤 이름으로 옮기고,
> **그 순간 살아 있던 핀들(= 코호트)이 전부 종료될 때까지 — 단 `settle_timeout`까지만 — 기다린 다음**, 오직
> 하나의 술어를 평가한다: **"P가 시작된 이후 X에 대한 커밋 rename이 `Ok`를 반환한 적이 있는가."**
> 그렇다면 — 그 경우에 한해서만 — X는 삭제 대신 복원된다.

- **characterization 105개는 전부 초록을 유지**한다.
- 회귀 테스트 `tests/regression_reconcile_gc_dedup_race.rs`는 **GREEN 20/20을 유지**한다.
- **B-3에서 어떤 관측 행동이라도 바뀌면 그것이 곧 실패다.** stats·골든 트리·서빙 계약 **비트 동일**.

### 0.2 anti-cheat

**테스트를 약화·삭제·스킵하지 마라.** 스위트가 red면 **구현을 고친다.**
단언을 느슨하게(`assert_eq!` → `assert!`, `==` → `>=`) 바꾸거나 `#[ignore]`를 다는 것은 **즉시 실패**다.

### 0.3 scope — 비-테스트 표면

```
src/store/**      src/main.rs      src/layout.rs
```

이 밖의 **프로덕션 파일을 건드리면 배리어 B4 위반**이다.
문서(`docs/**` · `CONTEXT.md`)는 **scope 밖**이므로 자유롭게 쓴다.
**`Hooks` 필드는 7개다 — 하나도 늘리지 마라.**

### 0.4 ⚠ `ReconcileStats`에 필드를 **추가하지 마라**

`tests/layout_tree.rs:71,137,198`이 **구조체 전수 `assert_eq!`**로 stats를 핀한다 → 필드를 하나라도 늘리면
**그 3개가 깨진다 = 두 번째 관측 행동 플립**(하드룰 10 위반).

**B-3은 관측성 증분이다. 그래서 이 유혹이 가장 강한 자리다.** `deferred: usize`를 넣고 싶어질 것이다 —
**넣지 마라.** 관측성은 **tracing으로만** 낸다. 연기 카운터가 필요하면 **후속 파이프라인**(**F-29**)이다.

### 0.5 뮤턴트 킬은 **실증하라**

B-3이 새로 거는 테스트(`Store::new` D-3 테스트 등)의 뮤턴트에 대해 **실제로 코드를 임시 변형**해 RED를 확인하고
**원복**한 뒤 그 출력을 보고하라. **주장은 증거가 아니다.**

---

## 1. tracing — 관측성 (`ReconcileStats` 필드 **0개 추가**)

이 픽스가 내는 **모든** 관측 이벤트를 정리하고 필드를 못박는다:

| 이벤트 | 레벨 | 필드 | 어디서 | 언제 |
|---|---|---|---|---|
| `"GC restored: landed commit"` | `info` | `sha` | `reconcile.rs` GC 루프의 `Settled::Restored` arm | 보호가 확정돼 복원됨 |
| `"gc settle timed out — grave restored, reclamation deferred"` | **`error`** | `sha` · `cohort_size` · `waited_ms` | **`pins.rs`의 `settle()`** | `settle_timeout` 소진 → fail-CLOSED 복원 |
| `"grave recovered"` | `info` | `sha` | `reconcile.rs`의 `recover_graves` | 잔존 무덤을 정본으로 되돌림 |
| `"quarantined corrupt blob (bit rot)"` | `warn` | `sha` | 격리 분기 | **기존 그대로 — 건드리지 마라** |

⚠ **`Settled::Deferred` arm에서 로그를 다시 내지 마라** — `settle()`이 이미 `tracing::error!`를 냈다.
**중복 로깅 금지.**

**왜 `error` 레벨인가**: `main.rs:49`는 `run_once`의 `Err`를 **`warn!`만 하고 다음 틱으로 넘어간다.** 즉 중단을
택해도 **운영자에게 더 잘 보이지 않는다.** `tracing::error!`는 `warn!`보다 **더 높은 레벨**로, **sha·cohort_size·
waited_ms**까지 실어 나른다.

**왜 `io::Error`를 합성하지 않는가**(B7 계약 — **이미 B-2에서 확정, 여기서 뒤집지 마라**): 타임아웃은
**`io::Error`가 아니다** — **어떤 syscall도 실패하지 않았다.** `io::Error::new(ErrorKind::TimedOut, …)`로
**합성**하는 것이야말로 B7이 금지하는 **가공**이다.

---

## 2. 위생 (`src/store/**` — **행동 무변경**)

### 2.1 Drop poison 봉인

`PinGuard::drop`과 `PassGuard::drop`은 **`Mutex<Inner>`를 잠근다.** 다른 스레드가 그 뮤텍스를 쥔 채 패닉하면
뮤텍스가 **poison**되고, `Drop` 안의 `.lock().unwrap()`이 **패닉하며 unwind 중 패닉 = abort**가 된다.

```rust
// 모든 Drop 경로에서:
let mut g = self.pins.inner.lock().unwrap_or_else(|e| e.into_inner());
```

**Drop 안의 모든 `lock().unwrap()`을 `unwrap_or_else(|e| e.into_inner())`로 바꾼다.**
poison된 상태에서도 **핀을 반드시 제거**해야 한다 — 그러지 않으면 코호트가 영영 드레인되지 않는다.

⚠ **Drop이 아닌 경로**(`pin()`·`landed()`·`cohort_at_grave()`·`await_settlement()`·`on_landed` 클로저)의
`unwrap()`은 **그대로 둔다** — 거기서는 poison이 **진짜 버그의 신호**이고, 삼키면 안 된다.

### 2.2 `shrink_to_fit`

`PassGuard::drop`의 `landed.clear()` 이후 / `enter_pass`의 `landed.clear()` 이후 `landed.shrink_to_fit()`.
`Inner::live`도 sha 엔트리가 비면 `remove`되므로(이미 `PinGuard::drop`이 한다) 추가 조치 불필요.
**행동 무변경 — 메모리 위생일 뿐이다.**

### 2.3 세 순서 제약의 doc 고정

코드 주석으로 **못박는다**(리뷰가 이 세 줄을 확인한다):

1. **`on_landed`는 rename의 `?` *뒤에서만* 호출된다** (`atomic.rs::Staged::commit_blocking`).
   → 앞으로 옮기면 "커밋을 **시도**했다"는 흔적이 되고 **ENOSPC 무한연기**가 부활한다(T-C1이 죽인다).
2. **`notify_waiters()`가 `in_commit_post_landed` 훅보다 *먼저* 호출된다** (`pins.rs::commit_pointer`).
   → 뒤집으면 **T-P4b-2의 load-bearing 지점이 사라진다**(알림이 나간 뒤에 클로저가 park해야 **핀이 살아있는
   채로** settlement가 깨어나는지 관측할 수 있다).
3. **`recover_graves`는 `collect_referenced`보다 *먼저* 호출된다** (`pins.rs::PassGuard::begin`).
   → 뒤집으면 크래시 창에 커밋된 포인터가 refs에 안 잡힌다.

### 2.4 `gc_deleted` doc 정정

`ReconcileStats::gc_deleted`의 doc을 정확히 고친다:

> `gc_deleted` = **회수(reap)된 blob 수.** 무덤으로 옮겼다가 **복원**된 blob(`Restored`)과 **연기**된
> blob(`Deferred`)은 **세지 않는다** — 회수하지 않았기 때문이다.
> ⚠ 이 필드는 **`tests/layout_tree.rs`의 전수 구조체 `assert_eq!` 3곳**에 핀돼 있다. **필드를 추가하지 마라.**

### 2.5 `Store::new`의 **D-3 doc**

```rust
/// ⚠ **데이터 루트 하나당 `Store`는 정확히 하나.**
///
/// 핀 등록부(`BlobPins`)는 **in-process**이고 `Store::clone()`이 `Arc`로 공유한다.
/// 같은 root로 `Store::new`를 **두 번** 부르면 **등록부가 갈라져** reconcile이 다른 `Store`의 put을
/// 보지 못한다 → **`reconcile-gc-dedup-race`가 부활한다**(커밋 포인터만 남고 blob 부재 → 영구 404).
///
/// **공유가 필요하면 `Store::clone()`을 써라.** `Store::new`를 다시 부르지 마라.
pub fn new(root: PathBuf) -> Self { ... }
```

⚠ **D-3 테스트는 B-1에서 이미 걸었다**(`store.clone()`은 등록부 공유(`Arc::ptr_eq`) ∧ 같은 root의 `Store::new`
2개는 **공유하지 않음**). **여기서는 doc만 붙인다** — 테스트가 여전히 초록인지 확인하라.
**`Store::new`는 `pub` 유지**(D-3) — `pub(crate)` 축소는 crate 외부인 `tests/*.rs` 다수를 고쳐야 해 anti-cheat
게이트와 충돌할 위험이 있다.

---

## 3. `docs/adr/0002-*.md` (**신규**)

**ADR 0002 — 무취소 커밋 + 착지 흔적 + 무덤 코호트 정산.**

담아야 할 것:

- **결정**: GC가 blob을 지우기 전에 **무덤 이름으로 rename**하고, **그 시점의 핀 코호트**가 종료될 때까지 —
  **`settle_timeout`까지만** — 기다린 뒤 **`landed` 하나만** 보고 판정한다.
- **기각한 대안과 이유**:
  - **블롭 락(sha 뮤텍스)** — GC가 락을 **전 포인터 트리 워크 내내** 쥐고, put은 그 락을 **KeyLocks를 쥔 채**
    기다린다. 그 대기가 `upload_timeout` 예산(`files.rs:89`)과 `cap.reserve` 전역 누산(`:82`)에 계상되어 부하
    하에서 **400 `upload_timeout` / 무관한 업로드의 507**이 새로 발생한다 — characterization 105개가 **절대 못
    잡는 두 번째 관측 행동**이며 **하드룰 10 위반**이다. 승자는 **put이 블록되지 않으므로 그 표면이 존재하지
    않는다**(P1).
  - **"put이 항상 바이트를 재기록"** — GC가 put의 기록 **이후**에 지우면 여전히 유실.
  - **"삭제 직전에 refs를 재확인"** — 재확인과 `remove_file` 사이에 put이 커밋 가능. **여전히 TOCTOU.**
  - **`arm()`(= "커밋을 **시도**했다"는 흔적)** — 흔적의 수명은 `PinGuard::drop`(취소 시 즉시 동기 실행)에
    묶여 있는데 커밋(`rename`)은 `spawn_blocking`이라 **취소를 뚫고 착지한다**. **흔적과 커밋이 다른 스레드에서
    다른 시각에 결정된다** — 이 비대칭이 crash 유실 시퀀스도, ENOSPC 무한연기도 낳았다.
  - **`live`를 보호 술어로 쓰기** — `live`는 **"성공할 결말"의 프록시**인데 **프록시는 결말이 아니다.**
    겹치는 실패 put이 회수를 **무기한 연기**한다. ⇒ **`live`는 대기 조건으로 강등**됐다.
  - **`RenameReceipt`(unit 토큰)** — **아무 것에도 바인딩되지 않아** 무관한 rename에서 발급받을 수 있었다.
    **증거가 전이에 바인딩되지 않으면 증거가 아니다.** ⇒ 증거는 **`Graved` 그 자체**다.

- **⚠ ADR에 반드시 남길 P-4의 교훈** (계획서가 **명시 요구**한 문구):

  > **"무취소 커밋은 유실 창을 닫는 대신 *대기의 상계를 파괴한다*. `upload_timeout`은 **호출자 퓨처만** 자른다 —
  > abort 불가능한 blocking 클로저를 기다리는 코드는 **반드시 자기 벽시계 예산을 가져야 한다.**"**

  근거: `tokio-1.52.3/src/task/blocking.rs:107-120` — *"`spawn_blocking` tasks cannot be aborted once they
  start running."* 이 표를 **계획서에 직접 적어 놓고도** 같은 성질이 **대기의 상계를 파괴한다는 것은 보지
  못했다**(Codex r3/P-4가 잡았다). **자기 코드가 반증하는 주장이었다.**

- **⚠ 여덟 라운드의 메타 교훈**(ADR에 남길 두 번째 문단 — **이것이 이 프로젝트의 진짜 산출물이다**):

  > **`src/` 설계는 여섯 라운드 내내 한 글자도 바뀌지 않았다. 매번 틀린 것은 증인이었다.**
  > 병의 이름: **「비동기 연산의 *개시*를 그것의 *완료*로 착각한다」**
  > (`spawn` ≠ 폴링됨 · `abort()` ≠ 취소 완료 · 호출 ≠ 폴링 · park ≠ 영원한 정지 · `timeout` `Err` ≠ 안쪽
  > 퓨처 종료). **아무것도 증명하지 못하는 증인은 봉인을 제거해도 초록으로 남는다** — 그것이 **버그보다 위험하다.**
  > **논증은 근거가 아니다. 신호가 근거다.**

---

## 4. `CONTEXT.md` — Language (용어가 **코드 식별자와 1:1**)

| 용어 | 정의 | 코드 식별자 |
|---|---|---|
| **Pin** | put이 blob을 **보기 전에** 잡는 무대기 예약. **커밋 클로저가 소유**하므로 취소가 떼어낼 수 없다. **핀의 죽음 = 그 put의 종료 결과 확정** | `BlobPins::pin` → `PinGuard` |
| **Landed** | **커밋 rename이 `Ok`를 반환했다**(= 커밋 포인터가 VFS에 실재한다). **유일한 보호 술어**이며 **sticky**하고 **패스 스코프**다. ⚠ *"커밋을 시도했다"가 아니다* — 흔적은 rename의 `?` **뒤에서만** 생긴다 | `Inner::landed` · `Staged::commit_blocking(on_landed)` |
| **Grave** | GC가 blob을 지우기 **전에** 옮겨 두는 이름(`.objects/.gc-grave-<sha>`). **평면 파일**(mkdir 0) · **sha를 품는다**(복구 가능) · 구 바이너리는 `Other`로 **무시한다**(롤백 안전) | `layout::grave_name` · `Layout::grave_path` · `ObjectsEntry::Grave` |
| **Cohort** | **무덤 rename 시점에 살아 있던 핀 id 집합** — **고정·유한**. **대기 조건이지 보호가 아니다.** 무덤 **이후**에 생긴 핀은 들어오지 않는다(그 put은 ENOENT를 보고 바이트를 재기록한다 → **자급자족**) | `Graved::cohort` · `BlobPins::cohort_at_grave` |
| **Settle** | **유한·fail-CLOSED 정산**: **landed 확정 → 즉시 복원**(대기 0) / **코호트 드레인 → `landed`를 보고 판정** / **`settle_timeout` 초과 → 복원 + 연기 + `error!`**. **보호 판정의 유일한 API**이며 `Graved`를 **소비**한다 | `Graved::settle(self)` → `Settled{Restored, Reaped, Deferred}` |

⚠ **`live`는 Language에 올리지 마라 — 보호 술어가 아니다.** 굳이 쓴다면:
*"live = 지금 존재하는 핀 = **결말이 아직 확정되지 않은** put. **대기 조건이지 보호가 아니다.**"*

---

## 5. ⚠⚠ **부분 해결 명시 — F-25 (릴리스 게이트 제출물)**

> ### 이 픽스는 **부분 해결**이다 (D-4)
>
> **격리 분기의 유실 경로는 미해결로 남는다.** `reconcile.rs`의 비트로트 격리 분기는 여전히 핀·무덤을 거치지
> 않고 `read → rename(blob → .corrupt)`를 한다. 손상 blob을 동시 put이 `write_atomic`으로 **치유한 직후**
> 패스가 그 **치유된 inode**를 `.corrupt`로 옮기면 — **오늘 고치는 것과 같은 증상**(커밋 포인터만 남고 blob
> 부재 → 영구 404)이 재현된다. 선행 비트로트가 필요하므로 이번 플립과 **직교**하지만, **"포인터만 남고 blob
> 부재" 증상 클래스는 이 픽스 이후에도 완전히 닫히지 않는다.**
>
> **왜 여기서 안 고치는가**: 고치면 **두 번째 관측 행동 플립**이다("치유된 blob이 격리되어 404가 된다" →
> "안 된다"). gated-bugfix **하드룰 10**: *"두 번째 관측 행동 플립은 근본 원인을 공유하거나 first-increment
> diff 안에 들어오더라도 **항상 별도 파이프라인**."* → **F-25**로 분리한다.
>
> **릴리스 게이트에 이 문장을 그대로 제시한다.** **이 사실을 숨기고 "증상 클래스 해결"이라 주장하면 Blocker다.**

**⇒ 격리 분기의 diff는 정확히 0줄이어야 한다.** acceptance 항목이다. `git diff`로 증명하라.
`corrupt_blob_quarantined` 유닛 테스트는 **불변 초록**이어야 한다.

**F-25가 재사용할 것**(청사진은 계획서 §F-25에 보존돼 있다 — **B-3은 그것을 구현하지 않는다**):
`Graved` · `PassGuard` · 무덤 이름공간 · **T-Q2 · T-Q3**(이 픽스의 B-2 acceptance에 이미 있다).
`sift_corrupt`/`Sifted`는 **이 픽스에 착지하지 않는다.**

---

## 6. 롤백 런북 (문서)

**구 바이너리는 `.gc-grave-*`를 절대 지우지 않는다.** 구 `classify_objects_entry`(`layout.rs:162-173`)에서
`Other`로 떨어진다 → temp 분기(`.tmp-` 접두)도 blob 분기(64hex)도 안 걸린다.
원안의 `.tmp-<unique>` 이름은 **신·구 양쪽이 삭제**했다 — 그게 P-1의 사인이었다.

**수동 복구 — 이름이 sha를 품으므로 한 줄이다:**

```bash
# 무덤 개수 세기
ls -1 <DATA_DIR>/.objects/ | grep -c '^\.gc-grave-' || echo 0

# 무덤 목록
ls -1 <DATA_DIR>/.objects/.gc-grave-* 2>/dev/null

# 복구 (blob이 부재할 때만 — 존재하면 내용을 먼저 검증하라)
mv <DATA_DIR>/.objects/.gc-grave-<sha> <DATA_DIR>/.objects/<sha>
```

⚠ **정본 blob이 이미 존재하면 덮어쓰기 전에 sha를 검증하라** — 신 바이너리의 `recover_graves`는 그것을 자동으로
한다(`blob 존재 ∧ 내용 sha == sha` → 무덤 폐기 / `내용 sha != sha` → **무덤을 채택**).

**롤백 후 재롤포워드**: 신 바이너리를 다시 올리면 첫 패스의 `recover_graves`가 `collect_referenced` **이전에**
남은 무덤을 전부 정본으로 되돌린다. **수동 개입이 필요 없다.**

---

## 7. ⚠⚠ **`Graved` 봉인 체크리스트 (10항목)** — 리뷰가 반드시 확인한다

> **이 열 줄 중 하나라도 어기면 봉인이 풀린다.**
> ①~⑤: **사전확인 뮤턴트가 컴파일된다** · ⑥: **P-4가 부활한다** · ⑦⑧⑨⑩: **증인이 아무것도 증명하지 못한다.**

1. **`Graved`의 필드는 전부 private.**
2. **`Default`/`Clone`/`Copy` 유도 금지.**
3. **`PassGuard::grave()` 밖에 생성자를 만들지 말 것.**
4. **`BlobPins`에 sha로 조회하는 공개 보호 술어를 추가하지 말 것**(`landed()`는 `pins.rs` private 유지).
5. **보호 판정 API는 `Graved::settle(self)` 하나로 유지**(판정만 따로 얻는 메서드 **금지**).
6. **`settle()`의 모든 대기 경로는 유한해야 한다** — `await_settlement`의 `timeout_at` **제거 금지** ·
   `settle_timeout`에 **기본값을 숨긴 오버로드 금지**(호출자가 **알고 정한다**) · 타임아웃 분기는
   **fail-CLOSED**(**복원**)여야 하며 **절대 `remove_file(grave)`로 가지 않는다** ·
   **`landed` 삽입의 `notify_waiters()` 제거 금지**(제거하면 착지한 객체가 코호트 잔여 멤버를 기다리며 404가
   된다 — **유실은 아니지만 가용성 회귀다**. **증인 = T-P4b-2**).
7. **배리어 테스트는 `stats.referenced`와 `post_grave` 관측으로 *삭제 분기 진입*을 자기검증한다** —
   이 두 단언을 **약화하지 말 것**(없애면 테스트가 **참조됨 분기로 새고도 초록**일 수 있다).
8. **배리어 테스트의 모든 park에는 「도착 신호 + 해제 신호」가 쌍으로 있다** — **spawn만 하고 다음 단계로
   넘어가는 지점을 만들지 말 것**(`tokio::spawn`은 **폴링을 보장하지 않는다** → 핀이 생기기도 전에 GC가
   **빈 코호트**를 캡처한다). **랑데부 규율의 체크리스트 표를 함께 갱신하지 않고는** 배리어 테스트의 안무를
   바꿀 수 없다(⚠ **T-C3는 이 함정에서 *조용히 GREEN*이 된다** — `gc_deleted == 1`이 기대값과 같다).
9. **「개시 ≠ 완료」 — 비동기 연산의 *개시*를 *완료*로 쓰지 말 것.**
   **`abort()` 뒤에는 반드시 그 `JoinHandle`을 유한 타임아웃으로 await하고 `JoinError::is_cancelled()`를
   단언한다**(⚠ **`abort()`는 취소를 *스케줄만* 한다** — `join.rs:227-229`. 이것을 빠뜨리면 **T-C2의
   caller-owned 뮤턴트가 경합으로 GREEN이 되고**, **T-B5①은 `pass_lock`에서 hang한다**) ·
   **`timeout`의 `Err`는 안쪽 퓨처를 *드롭*할 뿐이다**(`&mut handle`로 프로브할 것 — 값으로 넘기면 태스크가
   detach된다) · **완주를 await하는 모든 핸들은 `JoinError`를 언랩한다**(버려진 핸들은 **패닉을 삼킨다**) ·
   ⚠ **"의도적으로 await하지 않는 핸들"이라는 예외는 *없다*.** **park된 태스크도 teardown에서 재개된다**
   (sender 드롭 = 재개) ⇒ **park sender를 *명시적으로* 드롭하고, 핸들을 유한 타임아웃으로 await하며,
   `JoinError`와 안쪽 결과를 *둘 다* 언랩한다**(**단언을 전부 마친 뒤에** — 먼저 해제하면 시나리오가 사라진다).
10. **async 표현식은 반드시 `.await`한다.** `let _ = <async fn>(..)`는 **폴링되지 않은 퓨처를 드롭**할 뿐
    **아무 일도 하지 않는다**(`#[must_use]`도 `let _ =`가 **삼킨다**). **부작용을 노린 async 호출을 결과째
    버리지 말 것** — T-B5④의 `let _ = pass.grave(..)`가 **무덤을 파지 않아** `recover_graves` 뮤턴트를 **통째로
    놓쳤다**. **rename·복원 같은 파괴/복구 연산은 await하고 *디스크 상태로* 확인한다.**

> **새 배리어 테스트를 쓸 때는 「개시 ≠ 완료」 클래스의 10개 함정 항목을 1:1로 대조하고 그 매트릭스에 행을
> 추가한다** — *"이전 라운드에서 safe 판정"은 근거가 아니다.*

**이 체크리스트를 `docs/adr/0002-*.md` 또는 `CONTEXT.md`(혹은 둘 다)에 넣는다.** 코드 리뷰가 참조할 수 있어야 한다.

---

## 8. B-3 acceptance (**위생·관측성·문서 — 행동 무변경**)

- [ ] `cargo test` **105 green** + 회귀 **GREEN 유지**. **`ReconcileStats` 정의 무변경**(필드 추가 **금지**)
- [ ] **⚠ 격리 분기 diff 0줄**(D-4) — **`git diff`로 증명.** `corrupt_blob_quarantined` **불변 초록**
- [ ] tracing: `GC restored` / `grave recovered` 필드(`sha`) · **Drop poison 봉인**
      (`unwrap_or_else(into_inner)`) · `shrink_to_fit`
- [ ] **ADR 0002** + **CONTEXT.md Language**: **Pin / Landed / Grave / Cohort / Settle** (특히 *"landed = 커밋
      rename이 `Ok`를 반환했다"* — **유일한 보호 술어** — 와 *"cohort = 무덤 rename 시점에 살아있던 핀 집합"* —
      **대기 조건이지 보호가 아니다** — 와 *"settle = **유한·fail-CLOSED** 정산: landed 확정 → 즉시 복원 /
      코호트 드레인 → 판정 / **`settle_timeout` 초과 → 복원 + 연기 + `error!`**"*). **용어가 코드 식별자와 1:1.**
      **ADR에 P-4의 교훈을 남긴다**: *"무취소 커밋은 유실 창을 닫는 대신 **대기의 상계를 파괴한다**.
      `upload_timeout`은 **호출자 퓨처만** 자른다 — abort 불가능한 blocking 클로저를 기다리는 코드는 **반드시
      자기 벽시계 예산을 가져야 한다.**"*
- [ ] `Store::new` **D-3 doc** ("데이터 루트 하나당 Store 하나 — 공유는 `Store::clone()`")
- [ ] **롤백 런북**: 구 바이너리는 `.gc-grave-*`를 `Other`로 무시한다(절대 안 지운다) → 수동 복구는
      `mv .objects/.gc-grave-<sha> .objects/<sha>`. **무덤 개수 세는 원라이너 포함**
- [ ] **`Graved` 봉인 체크리스트**(§7 — 10항목) — 리뷰 항목으로 문서에 남긴다
- [ ] **부분 해결 명시**: doc에 **F-25(격리 분기 유실 경로 미해결)를 굵게** 남긴다 — **릴리스 게이트 제출물**
- [ ] `cargo clippy -D warnings`, doc 링크, `#[must_use]` 경고 0

---

## 9. 이 증분이 **하지 않는** 것 (명시)

| 항목 | 왜 안 하는가 | 어디로 |
|---|---|---|
| **격리(quarantine) 분기 봉인** | **두 번째 관측 행동 플립** — 하드룰 10 (D-4) | **F-25** (별도 gated-bugfix · **필수**) |
| **`ReconcileStats`에 `deferred: usize`** | `layout_tree.rs`의 전수 `assert_eq!` 3곳이 깨진다 = **두 번째 플립** | **F-29** |
| **`FILES_GC_SETTLE_TIMEOUT` 독립 env 노브** | `src/config.rs` + `validate()` + `state.rs` 배선이 필요 → **scope 밖** | **F-29** |
| **rename 앞의 사전 확인**(최적화) | `pins.rs`에 **새 술어 API를 추가해야만** 가능 — **그 추가가 곧 봉인 해제다** | **F-26** (기본 **제외**) |
| **`.objects` 온디스크 패스 락파일** | 다중 replica 위험 완화 — 이 픽스의 범위가 아니다 | **F-27** (`files#gc-pass-lockfile`) |
| **SIGTERM graceful shutdown** | `shutdown_signal()`이 `ctrl_c()`만 본다 — **별개 버그** | **F-28** (`files#sigterm-graceful-shutdown`) |

**남은 위험(문서에 남긴다)**:
1. **격리 분기 유실 경로 미해결 (F-25)** — **릴리스 게이트에 명시 제출.**
2. **다중 프로세스/레플리카** — 핀은 **in-process**다. 같은 PVC에 replica ≥ 2면 등록부가 갈라져 **버그가
   부활한다.** **`replicas:1 + RWO`가 load-bearing 배포 불변식이다**(`locks.rs`가 이미 같은 전제 위에 있다).
   완화: **배포 매니페스트 주석** + 향후 F-27.
3. **`Store::new` 2회 (D-3)** — doc + 테스트로 못박지만 **컴파일 강제는 아니다.**
4. **뮤턴트 경계** — **`Graved` 봉인은 모듈 경계이지 타입 마법이 아니다.** `pins.rs`를 편집해 새 술어 API를
   추가하면 풀린다. **"타입이 모든 것을 막는다"고 주장하지 않는다.** T-B2가 **2차 방어선**이다.
5. **정상 Restore 경로의 transient non-servable 창** — 무덤 rename ~ 복원 rename 사이(fsync 2회 폭) 404/list
   제외. **오늘 없던 상태**다(오늘은 그 자리가 **영구 유실**이므로 순개선이지만 **숨기지 않는다**).
6. **degraded-path 회수 연기** — 코호트 멤버의 FS 연산이 영영 돌아오지 않으면 그 blob의 회수가 **스톨이 풀릴
   때까지 매 패스 연기**된다. **정상 입력에서 도달 불가능** · **국소적** · **시끄럽다**(`error!`) ·
   **자기치유적**. **이것은 실재하는 행동 변화이며 숨기지 않는다.**
7. **`.gc-grave-<비-sha>` 쓰레기** — `Other`로 영구 무시(누구도 안 지운다). 의도된 보수성이지만 **누수**다.
8. **미측정**: 코호트가 큰 병리적 부하(같은 sha를 수백 개 동시 dedup-put)에서의 패스 시간은 **벤치하지 않았다.**
   안전성 결함은 아니지만(대기에 상계가 있다) **지연 특성은 미실측**이다.

---

## 10. 보고 (완료 시 반드시 포함)

1. `cargo test` 출력 — **105 passed** ∧ 회귀 **GREEN 유지**.
2. **`git diff`에서 격리 분기가 0줄 변경**임을 보여라 — 이것이 이 증분의 **가장 중요한 단일 증거**다.
3. `ReconcileStats` 정의의 diff가 **doc 주석뿐**임을 보여라(**필드 0개 추가**).
4. 생성한 문서 경로: `docs/adr/0002-*.md` · `CONTEXT.md`(Language 섹션 · 롤백 런북 · 봉인 체크리스트).
5. **뮤턴트 킬 실증** — Drop poison 봉인이 실제로 load-bearing인지 확인하고(뮤텍스를 강제 poison시킨 뒤
   `PinGuard::drop`이 abort하지 않고 핀을 제거하는지) 그 출력을 보고하라. **주장은 증거가 아니다.**
6. `cargo clippy -D warnings` 출력 · `#[must_use]` 경고 0.
7. **부분 해결 선언 문구**(§5)를 **그대로** 릴리스 게이트에 제출하라 — **숨기면 Blocker다.**
