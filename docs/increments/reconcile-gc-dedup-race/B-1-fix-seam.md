---
id: B-1
title: fix-seam — atomic stage/commit · pins.rs 신설 · 무덤 이름공간 · D-1 &Store 전환 (관측 행동 플립 0)
status: done
blocked-by: []
plan: docs/bugfixes/reconcile-gc-dedup-race.md
created: 2026-07-13
closed: 2026-07-13
---

# B-1 — fix-seam (**관측 행동 플립 0**)

> ## ⚠ 이 문서는 **자기완결적**이다
>
> 계획서(`docs/bugfixes/reconcile-gc-dedup-race.md`, 2646줄)를 **열지 않아도** 이 증분을 완수할 수 있다 —
> 필요한 설계·코드·근거·acceptance가 **전부 아래에 축자 발췌**돼 있다.
> 계획서는 **Codex plan gate r8에서 approve · 0 findings**를 받은 확정 설계이며, 이 문서와 어긋나는 것이
> 있으면 **계획서가 정본**이다.
>
> **이 증분에서 관측 행동은 하나도 뒤집히지 않는다.** 씨앗(seam)만 심는다.

---

## 0. 공통 계약 — 반드시 먼저 읽어라

### 0.1 불변식 — 뒤집히는 관측 행동은 **정확히 하나**다

이 파이프라인 전체(B-1 → B-2 → B-3)에서 뒤집히는 관측 행동은 **딱 하나**다:

> reconcile 패스 P가 blob X를 GC 삭제 후보로 확정했을 때(무참조 ∧ tombstone 만료), P는 X를 무덤 이름으로
> 옮기고, **그 순간 살아 있던 핀들(= 코호트)이 전부 종료될 때까지 — 단 `settle_timeout`까지만 — 기다린 다음**,
> 오직 하나의 술어를 평가한다:
>
> > **P가 시작된 이후 X에 대한 커밋 rename이 `Ok`를 반환한 적이 있는가**(= 커밋 포인터가 VFS에 실재하는가).
>
> **그렇다면** — 그리고 **그 경우에 한해서만** — X는 삭제 대신 정본 이름으로 복원되고, `gc_deleted`는 증가하지
> 않으며 tombstone은 유지된다.

- **characterization 105개는 전부 초록을 유지**한다. B-1에서도, B-2에서도, B-3에서도.
- 회귀 테스트 `tests/regression_reconcile_gc_dedup_race.rs`는 **B-1에서 여전히 RED**여야 하고(플립 미도달),
  **B-2에서 GREEN 20/20**이 돼야 한다.
- **B-1에서 위 플립이 일어나면 그것이 곧 실패다.** 핀·`landed`는 **기록되지만 아무도 읽지 않는다.**

### 0.2 anti-cheat

**테스트를 약화·삭제·스킵하지 마라.** 스위트가 red면 **구현을 고친다.**
단언을 느슨하게(`assert_eq!` → `assert!`, `==` → `>=`) 바꾸거나 `#[ignore]`를 다는 것은 **즉시 실패**다.

### 0.3 scope — 비-테스트 표면

```
src/store/**      src/main.rs      src/layout.rs
```

이 밖의 **프로덕션 파일을 건드리면 배리어 B4 위반**이다.
특히 **`src/config.rs`는 무변경**이다 — 새 env 노브를 만들지 않는다(`settle_timeout`은
`FILES_UPLOAD_TIMEOUT`에서 **파생**된다). `src/http/state.rs` · `src/http/internal/files.rs` ·
`src/store/{locks,listing,buckets}.rs` · `src/capacity.rs` — **전부 무변경**.

테스트 경로(`tests/**`)는 배리어 B4의 `isTestPath()`가 scope 검사 **전에** 제외한다. `docs/**`도 scope 밖이다.

### 0.4 ⚠ `ReconcileStats`에 필드를 **추가하지 마라**

`tests/layout_tree.rs:71,137,198`이 **구조체 전수 `assert_eq!`**로 stats를 핀한다 → 필드를 하나라도 늘리면
**그 3개가 깨진다 = 두 번째 관측 행동 플립**(하드룰 10 위반). 복구·복원·연기 카운트는 **전부 tracing으로만** 낸다.

```rust
// 이 정의는 B-1/B-2/B-3 어디에서도 바뀌지 않는다.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    pub referenced: usize,
    pub gc_deleted: usize,
    pub gc_pending: usize,
    pub temps_deleted: usize,
    pub quarantined: usize,
}
```

### 0.5 뮤턴트 킬은 **실증하라**

acceptance가 요구하는 **각 뮤턴트**에 대해 **실제로 코드를 임시 변형**해 테스트가 **RED가 되는지 확인**하고
**원복**한 뒤, **그 출력을 보고하라**. **주장은 증거가 아니다** — 이 계획서의 게이트가 8라운드 동안 잡아낸 것이
정확히 **"증명하지 못하는 증인"들**이었다.

### 0.6 이 증분의 위치

| id | what | 플립 |
|---|---|---|
| **B-1** ← **당신은 여기** | fix-seam. GC 삭제/격리 분기는 **아직 기존 그대로**. 핀·landed는 **기록되지만 아무도 읽지 않는다** | **0** |
| B-2 | the flip — GC 삭제 분기를 `pre_grave → pass.grave(sha) → settle()`로 교체 | **1** |
| B-3 | 위생·관측성·문서 | 0 |

---

## 1. 버그 (한 문단)

`reconcile::run_once_at`은 패스 시작에 `collect_referenced()`로 **참조 sha 집합의 스냅샷**을 뜨고, 그 스냅샷을
기준으로 `.objects` 항목마다 2단계 tombstone GC를 집행한다. 한편 `Store::put`의 **dedup 분기**는 기존 블롭이
온전하면 **바이트를 다시 쓰지 않고** 커밋 포인터만 기록한다 — 즉 **기존 블롭에 새 참조를 추가하면서 그 블롭의
유일한 사본에 아무 흔적도 남기지 않는다.** put은 `KeyLocks`를 잡지만 **reconcile은 어떤 락도 잡지 않고**,
경합 자원은 **블롭(sha)**이지 키가 아니며, `run_once(root: &Path, …)`는 경로만 받는 자유함수라 **구조적으로 그
락을 잡을 수 없다**. ⇒ **스냅샷 이후 참조를 얻은 블롭이 같은 패스 안에서 삭제된다.** 커밋 포인터만 남고 유일한
사본이 사라져 객체가 **영구 non-servable**이 된다(GET 404 / list 제외). **데이터 손실.**

---

## 2. 픽스 모델 — **무취소 커밋 + 착지 흔적(landed) + 무덤 코호트 정산**

**커밋 rename과 핀의 수명을 하나의 `spawn_blocking` 클로저 안에 가둔다.** 그러면 "커밋을 시도했다"는 불확실한
프록시가 필요 없고, **"착지(landed) = rename이 `Ok`를 반환했다"는 확정 사실**을 흔적으로 쓸 수 있다.

**코드로 확인한 근거 (전부 재확인함):**

| 사실 | 출처 |
|---|---|
| `tokio::fs::rename`은 `asyncify = spawn_blocking(f).await` — **퓨처를 드롭해도 blocking 클로저는 끝까지 실행된다** | `tokio-1.52.3/src/fs/mod.rs:312` |
| **"`spawn_blocking` tasks cannot be aborted once they start running… runtime shutdown will wait indefinitely for all started `spawn_blocking` to finish"** — 시작된 blocking 태스크는 **abort 불가**, `JoinHandle` 드롭은 detach일 뿐 | `tokio-1.52.3/src/task/blocking.rs:107-120` |
| 저장소는 이미 `spawn_blocking` + `std::fs` 관행을 쓴다 | `src/store/atomic.rs:16`(rename) · `:24`(fsync_dir) |
| 취소는 **상시 경로**다(가정이 아니다) | `src/http/internal/files.rs:87` `tokio::time::timeout(upload_timeout, put_stream_fut)` |

**핵심 사실 A (무취소).** `commit_pointer`의 클로저가 `PinGuard`를 **소유**한다. 따라서:

> **핀 id가 `live[sha]`에서 사라지는 시점 = stage 실패로 rename에 도달조차 못 했거나, rename이 `Err`를
> 반환했거나, rename이 `Ok`를 반환하고 `landed`가 이미 삽입된 이후.**
> 즉 **핀의 죽음 = 그 put의 종료 결과(terminal outcome) 확정**이며, 그 결과는 **이미 `landed`에 반영돼 있다.**

**핵심 사실 B (happens-before).** `landed` 삽입, `pass_live=true`, **코호트 스냅샷**, **코호트 drain 검사**,
`settle`의 `landed` read는 **모두 같은 `Mutex<Inner>` 임계구역**이다 → 전순서가 존재한다.

**핵심 사실 C (rename Ok ⇒ 즉시 가시).** POSIX rename이 `Ok`를 반환하면 디렉터리 엔트리가 VFS에 존재한다
(fsync는 *내구성*이지 *가시성*이 아니다). 부모 버킷 디렉터리는 `stage_blocking`의 `mkdir_p`가 rename 이전에
만든다 → `pointers_all`의 `SeedRoot`(`layout.rs:257-274`)가 루트를 readdir할 때 그 버킷이 보인다.

**핵심 사실 D (무덤 이후의 자급자족).** R = blob→무덤 rename이 `Ok`를 반환한 사건이라 하자. R 이후에 `pin()`한
put은 `blob_intact`에서 `blob_path(X)`를 읽는데 **그 이름은 더 이상 무덤 inode를 가리키지 않는다** → **ENOENT**를
보고 dedup 분기에 못 들어가 **바이트를 재기록**한다. ⇒ **post-R 핀은 자급자족이며, GC가 기다릴 이유가 없다.**

> ### ⚠ **무취소는 공짜가 아니다 — 유실 창을 닫은 대가로 대기에 상계가 사라진다**
>
> *"시작된 blocking 태스크는 abort 불가"*는 **아무도 그 클로저를 끊을 수 없다**는 뜻이고, 곧
> **`PinGuard`가 영원히 살 수 있다**는 뜻이다 — `upload_timeout`은 **호출자 퓨처를 드롭할 뿐** blocking 클로저를
> 죽이지 못한다. 멈춘 파일시스템 연산(NFS 정지 · EBS 열화 · dm-thin 고갈) 하나면 **코호트가 영영 드레인되지 않는다.**
> ⇒ **그것을 기다리는 쪽(GC)이 반드시 자기 벽시계 예산을 가져야 한다** → **`settle_timeout`**.
> **`upload_timeout`은 대기의 상계가 아니다.**

---

## 3. 이 증분이 만드는 것

### 3.1 `src/store/atomic.rs` — 커밋을 두 단계로 쪼갠다

> **`RenameReceipt`는 존재하지 않는다.** 범용 `rename_durable`이 임의의 경로에 대해 발급하는 unit 토큰은
> "blob→무덤 전이"에 **아무 것도 바인딩하지 못한다**. 증거는 토큰이 아니라 **`Graved` 그 자체**다(§3.3).

```rust
/// rename + parent fsync. **증거 토큰을 발급하지 않는다** — 평범한 `io::Result<()>`다.
pub(crate) fn rename_durable_blocking(from: &Path, to: &Path, parent: &Path) -> io::Result<()> {
    std::fs::rename(from, to)?;
    std::fs::File::open(parent)?.sync_all()
}
pub(crate) async fn rename_durable(from:&Path, to:&Path, parent:&Path) -> io::Result<()> { /* spawn_blocking 위임 */ }

/// 원자적 쓰기를 **stage / commit** 두 단계로 노출한다.
/// 이유: `landed` 마킹을 **rename의 Ok 반환 직후·fsync 이전**에, **await 없이** 끼워야 하기 때문.
pub(crate) struct Staged { tmp: PathBuf, target: PathBuf }

pub(crate) fn stage_blocking(target: &Path, bytes: &[u8]) -> io::Result<Staged>;   // mkdir_p + create + write_all + sync_all
impl Staged {
    /// rename이 **Ok를 반환한 직후에만** `on_landed`를 호출하고, 그 다음 parent를 fsync한다.
    /// on_landed는 동기 클로저다 — 이 사이에 await/취소점이 존재할 수 없다.
    pub(crate) fn commit_blocking(self, on_landed: impl FnOnce()) -> io::Result<()> {
        std::fs::rename(&self.tmp, &self.target)?;   // ← 실패하면 on_landed는 절대 안 불린다
        on_landed();                                 // ← 착지 확정. 흔적은 여기서만 생긴다.
        std::fs::File::open(self.target.parent().unwrap())?.sync_all()
    }
}

/// 기존 공개 시그니처 **불변**. 단일 정의 위임(드리프트 0).
pub async fn write_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let (t, b) = (target.to_owned(), bytes.to_vec());
    tokio::task::spawn_blocking(move || stage_blocking(&t, &b)?.commit_blocking(|| {}))
        .await.expect("join")     // 저장소 관행(atomic::fsync_dir:26)과 동일
}
```

**⚠ `write_atomic`의 공개 시그니처는 불변이다**(`pub async fn write_atomic(&Path, &[u8]) -> io::Result<()>`).
내부만 단일 blocking 클로저로 위임 — **syscall 시퀀스 축자 동일**, 취소 입도만 "부분 → 전무"로 **좁아진다**
(부분 상태의 순감소).

**⚠ `Staged::commit_blocking(self, on_landed: impl FnOnce())` 시그니처는 이후 증분에서도 불변이다.**
훅은 `pins.rs`가 넘기는 `on_landed` 클로저 **안에서** 호출되므로 `atomic.rs`는 더 손대지 않는다.

`mkdir_p_durable` · `fsync_dir` · `unique_suffix` · 기존 인라인 테스트 3개는 **그대로 둔다**.

### 3.2 `src/layout.rs` — 무덤 이름공간

```rust
const GRAVE_PREFIX: &str = ".gc-grave-";              // `.objects` 직속 **평면** 이름 (mkdir 0)
fn is_sha_name(s:&str)->bool { s.len()==64 && s.bytes().all(|b| b.is_ascii_hexdigit()) }
pub(crate) fn grave_sha(name:&str)->Option<&str> { name.strip_prefix(GRAVE_PREFIX).filter(|s| is_sha_name(s)) }
pub(crate) fn grave_name(sha:&str)->String { format!("{GRAVE_PREFIX}{sha}") }
impl Layout { pub(crate) fn grave_path(&self, sha:&str)->PathBuf { self.objects_dir().join(grave_name(sha)) } }

pub enum ObjectsEntry { Reserved, Temp, Blob, Grave, Other }   // payload 없는 Copy 유지
// classify_objects_entry: Reserved → Grave(grave_sha 재사용) → Temp(.tmp-) → Blob(64hex) → Other
```

**현행 `classify_objects_entry`**(이 순서에 `Grave`를 **`Reserved` 다음, `Temp` 앞**에 끼운다):

```rust
pub fn classify_objects_entry(name: &str) -> ObjectsEntry {
    if name == GC_PENDING_NAME || name == CORRUPT_DIR_NAME { return ObjectsEntry::Reserved; }
    // ← 여기에 Grave 분기가 들어간다: if grave_sha(name).is_some() { return ObjectsEntry::Grave; }
    if name.starts_with(TMP_PREFIX) { return ObjectsEntry::Temp; }
    if name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()) { return ObjectsEntry::Blob; }
    ObjectsEntry::Other
}
```

**왜 이 이름인가**: 이름이 **sha를 품는다** → 복구가 가능하다. `.tmp-` 접두가 **아니다** → **temp로 오분류되지
않는다**(원안의 `.tmp-<unique>` 무덤은 rename이 mtime을 보존하므로 다음 패스가 **만료 temp로 보고 즉시 지우고
`temps_deleted`로 셌다** — 그게 Codex r1/P-1의 사인이었다).
`.objects` **직속 평면 파일**이므로 **`mkdir`이 코드에 없다** → 빈 디렉터리 잔재가 **불가능**하다.

**롤백 안전성**: 구 바이너리의 `classify_objects_entry`(`layout.rs:162-173`)는 `.gc-grave-<sha>`를 `Other`로
떨어뜨린다 → temp 분기(`.tmp-` 접두)도 blob 분기(64hex)도 안 걸린다 → **구 코드는 절대 지우지 않는다.**

**서로소성 근거**(3개 렌즈가 반증에 실패한 항목): `segment_ok`가 `.` 시작 세그먼트를 금지(`layout.rs:19`) →
사용자 키와 무덤 이름 **충돌 불가** · `pointers_all`이 `.objects`를 스킵(`layout.rs:262`).

### 3.3 `src/store/pins.rs` (**신규**, crate-private) — **전문**

```rust
//! ## 불변식
//! P1 `pin()`은 절대 블록하지 않는다(상호배제 0 — put은 GC를 기다리지 않는다).
//! P2 **보호 술어는 `landed` 하나뿐이다.**
//!      landed(sha) = 이 패스 동안 **커밋 rename이 Ok를 반환한** sha (sticky)
//!      live(sha)   = 지금 존재하는 핀 = **결말이 아직 확정되지 않은** put → **대기 조건**이지 보호가 아니다
//!    GC 보호 술어: restore ⇔ landed(sha)   ← 코호트 대기가 끝난 **뒤에만** 평가된다
//! P3 **커밋은 취소 불가다.** PinGuard는 커밋 클로저가 **소유**하며, Drop은 rename·마킹·fsync가
//!    모두 끝난 뒤 그 클로저 안에서 실행된다 → "핀이 죽었는데 rename이 나중에 착지"는 **불가능**.
//!    ⇒ **핀의 죽음 = 그 put의 종료 결과(terminal outcome) 확정**이며, 결과는 landed에 이미 반영돼 있다.
//! P4 보호 판정 API는 **`Graved::settle(self)` 하나뿐**이다. `Graved`는 **`PassGuard::grave()`의
//!    blob→무덤 rename이 성공했을 때만** 태어나고(private 필드·같은 모듈 외 생성자 0·derive 0),
//!    자기 `sha`와 **무덤 시점 코호트**를 품는다 → 판정이 **그 전이·그 sha에** 바인딩된다.
//!    `BlobPins`에 sha로 조회하는 **공개 술어는 존재하지 않는다**(`protected()` 없음).
//!    ⚠ **`pins.rs`가 밖으로 내보내는 것의 전부**(이 목록을 늘리면 봉인이 풀린다):
//!      `pub(crate)`: `BlobPins::{new, pin}` · `BlobPins::hooks() -> &Hooks`(**배리어 전용**) ·
//!                    `PinGuard::{blob_intact, commit_pointer}` ·
//!                    `PassGuard::{begin, referenced, recovered, pins, grave}` ·
//!                    `Graved::settle(self)` · `Settled` · `Hooks`
//!      **private (pins.rs 전용)**: `Inner`의 **모든 필드**(`next_id`/`live`/`landed`/`pass_live`) ·
//!                    `cohort_at_grave` · `await_settlement` · `Settlement` · `landed` · `enter_pass`
//!    ⇒ `reconcile.rs`는 **훅과 `grave()`만** 볼 수 있고, **보호 상태는 읽을 수단이 아예 없다**
//!      → 사전확인 뮤턴트는 `reconcile.rs`에서 **표현 불가**다.
//! P5 pass_live 플래그는 `PassGuard`(Drop 보유)가 **fallible op 이전에** 획득한다 → `?` 누수 0.
//! P6 핀에는 **단조 증가 id**가 붙는다. 무덤 rename **직후** 그 sha의 live id를 스냅샷한 것이 **코호트**다.
//!    코호트는 **고정·유한**하며, 무덤 **이후**에 생긴 핀은 코호트에 **들어오지 않는다**
//!    (그 put은 blob_path에서 ENOENT를 보고 바이트를 재기록한다 → **자급자족**).
//! P7 **대기는 유한하며 fail-CLOSED다.**
//!    코호트가 **고정·유한**하다는 것과 **유한 시간에 종료된다**는 것은 **다른 명제다.** `PinGuard`는
//!    **abort 불가능한 `spawn_blocking` 클로저가 소유**하므로(P3), 멈춘 파일시스템 연산은 코호트 멤버를
//!    **영원히 살려 둘 수 있다**. `upload_timeout`은 **호출자 퓨처를 드롭할 뿐** blocking 클로저를
//!    **죽이지 못한다** → **`upload_timeout`은 대기의 상계가 아니다**.
//!    ⇒ `settle()`은 다음 셋 중 **먼저 오는 것**에서 깨어난다(무한 대기 **불가**):
//!      (a) **`landed(sha)` 확정** → 보호가 확정이므로 **나머지 코호트를 기다리지 않는다**(대기 0 · 즉시 복원)
//!      (b) **코호트 드레인** → 모든 멤버의 종료 결과 확정 → `landed`를 읽어 판정
//!      (c) **`settle_timeout` 소진** → **fail-CLOSED**: **무덤을 정본으로 복원**(데이터 보존 우선) ·
//!          tombstone **유지** · `gc_deleted` **무증가** · `tracing::error!` · **패스는 정상 해제**
//!    `settled: Notify`는 **핀 drop**과 **`landed` 삽입** **양쪽에서** 울린다 → (a)가 **즉시** 발화한다.

#[derive(Clone, Default)]
pub(crate) struct BlobPins {
    inner: Arc<Mutex<Inner>>,                 // 동기 Mutex — 임계구역이 await를 걸치지 않는다
    settled: Arc<tokio::sync::Notify>,        // **두 곳에서** 울린다:
                                              //   ① PinGuard::drop      → 코호트 드레인 진행
                                              //   ② landed 삽입         → **보호 확정 → 즉시 깨움**
    pass_lock: Arc<tokio::sync::Mutex<()>>,   // 프로세스 내 라이브 패스 ≤ 1
    hooks: Hooks,                             // 결정적 배리어. prod = 전부 None
}
#[derive(Default)]
struct Inner {
    next_id: u64,                             // 단조 증가 핀 id (P6)
    live: HashMap<String, HashSet<u64>>,      // sha → 살아있는 핀 id 집합
    landed: HashSet<String>,                  // 커밋 rename이 Ok를 반환한 sha (sticky, 패스 스코프)
    pass_live: bool,
}
// ※ `armed` 맵도, `touched := armed 스냅샷` 시드도 **없다**.

impl BlobPins {
    /// blob을 **보기 전에** 잡는다. 동기·무대기. 새 id를 발급한다.
    pub(crate) fn pin(&self, sha:&str) -> PinGuard {
        let mut g = self.inner.lock().unwrap();
        g.next_id += 1;
        let id = g.next_id;
        g.live.entry(sha.to_owned()).or_default().insert(id);
        PinGuard { pins: self.clone(), sha: sha.to_owned(), id }
    }

    // ── 아래 3개는 **private**이다(`pub(crate)` 아님) → `reconcile.rs`는 술어를 **부를 수조차 없다**. ──

    /// 무덤 rename **직후** 호출된다. 그 시점의 live id 집합 = **코호트**.
    fn cohort_at_grave(&self, sha:&str) -> HashSet<u64> {
        self.inner.lock().unwrap().live.get(sha).cloned().unwrap_or_default()
    }

    /// **유한 대기**. 셋 중 **먼저 오는 것**에서 깨어난다 — 무한 대기가 **표현 불가**하다.
    /// `landed`가 **이미** true면 첫 검사에서 즉시 `Landed`(await 0회) — **코호트를 기다리지 않는다**.
    /// 코호트가 비어 있으면 첫 검사에서 즉시 `Drained`(await 0회) — **정상 GC의 fast path**.
    async fn await_settlement(&self, sha:&str, cohort:&HashSet<u64>, budget: Duration) -> Settlement {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let notified = self.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();                        // **검사 이전에** 등록 → lost wakeup 불가
            {   // 동기 Mutex는 await를 **절대 걸치지 않는다**(P1 불변 유지)
                let g = self.inner.lock().unwrap();
                // ① 보호 **확정**. 나머지 코호트의 결말은 판정을 바꿀 수 없다(landed는 sticky·단일 술어)
                //    → 더 기다리는 것은 **순손해**다(그 객체가 그동안 404다).
                if g.landed.contains(sha) { return Settlement::Landed; }
                // ② 코호트 전원 종료 = 모든 멤버의 종료 결과 확정 → landed가 정확히 반영돼 있다(P3)
                if g.live.get(sha).is_none_or(|ids| ids.is_disjoint(cohort)) {
                    return Settlement::Drained;
                }
            }
            // ③ **유한**. 예산이 끊기면 fail-CLOSED로 빠진다 — 멈춘 핀은 GC를 정지시킬 수 없다.
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Settlement::TimedOut;
            }
        }
    }

    /// **유일한 보호 술어.** 코호트 결말이 확정된 뒤에만 읽힌다.
    fn landed(&self, sha:&str) -> bool { self.inner.lock().unwrap().landed.contains(sha) }
}

/// 대기가 **왜** 끝났는가. `pins.rs` private — `reconcile.rs`는 이 타입을 볼 수 없다(P4 봉인 유지).
enum Settlement { Landed, Drained, TimedOut }

pub(crate) struct PinGuard { pins: BlobPins, sha: String, id: u64 }

impl PinGuard {
    /// 관측은 핀을 통해서만(순서 = 타입). sha가 핀에서 나오므로 "핀은 A, 검사는 B" 뮤턴트도 표현 불가.
    pub(crate) async fn blob_intact(&self, layout:&Layout) -> bool {
        let ok = matches!(tokio::fs::read(layout.blob_path(&self.sha)).await,
                          Ok(b) if hex::encode(Sha256::digest(&b)) == self.sha);
        self.pins.hooks.post_observe(&self.sha).await;     // 결정적 배리어(T-B2/T-B4)
        ok
    }

    /// **커밋 = 이 핀을 소비하는 무취소 연산.**
    /// 단일 blocking 클로저가 가드를 **소유**한다 → 호출자 취소(upload_timeout·disconnect)가
    /// in-flight rename에서 핀을 떼어낼 수 없다. tokio: 시작된 blocking 태스크는 abort 불가.
    pub(crate) async fn commit_pointer(self, target: PathBuf, bytes: Vec<u8>) -> io::Result<()> {
        tokio::task::spawn_blocking(move || {
            let me = self;                                  // 가드를 클로저가 소유
            let staged = atomic::stage_blocking(&target, &bytes)?;   // ← 여기까지의 실패 = **흔적 0**
            me.pins.hooks.in_commit_pre_rename(&me.sha);             // 동기 훅(T-C2 · T-P4a)
            staged.commit_blocking(|| {                              // ← rename Ok 직후에만
                let landed_now = {
                    let mut g = me.pins.inner.lock().unwrap();
                    // **착지 흔적**. `insert`의 반환값 = 이번에 처음 들어갔는가(전이에서만 깨운다)
                    g.pass_live && g.landed.insert(me.sha.clone())
                };  // ← 락을 **먼저 놓고** 깨운다(PinGuard::drop과 같은 규율)
                // 보호가 **확정**됐다 → 코호트 대기 중인 settle()을 **즉시** 깨운다.
                // 이게 없으면 settle은 나머지 코호트가 죽을 때까지(또는 타임아웃까지) 기다리고,
                // **그 창 내내 실재하는 포인터가 404다**.
                // notify_waiters()는 동기·논블로킹이고 런타임 컨텍스트를 요구하지 않는다
                // → blocking 클로저 안에서 안전하다(PinGuard::drop이 이미 같은 호출을 한다).
                if landed_now { me.pins.settled.notify_waiters(); }
                me.pins.hooks.in_commit_post_landed(&me.sha);        // 동기 훅(T-P4b-1 · T-P4b-2) — rename **이후**
            })
            // me(PinGuard) drop: rename·마킹·fsync가 **전부 끝난 뒤** live[sha]에서 id 제거 + notify
        }).await.expect("join")
    }
}

impl Drop for PinGuard {
    /// **핀의 죽음 = 이 put의 종료 결과 확정**(P3). landed는 건드리지 않는다.
    fn drop(&mut self) {
        {
            let mut g = self.pins.inner.lock().unwrap();
            if let Some(ids) = g.live.get_mut(&self.sha) {
                ids.remove(&self.id);
                if ids.is_empty() { g.live.remove(&self.sha); }
            }
        }   // ← 락을 **먼저 놓고** 깨운다
        self.pins.settled.notify_waiters();   // 동기·논블로킹 → blocking 클로저 안에서도 안전
    }
}

pub(crate) struct PassGuard { pins: BlobPins, _pass: OwnedMutexGuard<()>, layout: Layout,
                              refs: HashSet<String>, recovered: usize,
                              settle_timeout: Duration }   // 호출자가 **명시**한다

impl PassGuard {
    /// **패스 순서의 유일한 소유자.** P5: 플래그를 든 가드를 **fallible op 이전에** 만든다.
    /// `settle_timeout`은 **주입**된다(기본값 없음) → 테스트가 짧은 값을 넣어 degraded 경로를 **결정적으로**
    /// 친다(T-P4a). prod 값은 `main.rs`가 `cfg.upload_timeout_secs`에서 파생한다.
    pub(crate) async fn begin(store:&Store, settle_timeout: Duration) -> io::Result<Self> {
        let _pass = store.pins.pass_lock.clone().lock_owned().await;
        let mut me = Self { pins: store.pins.clone(), _pass, layout: store.layout().clone(),
                            refs: HashSet::new(), recovered: 0, settle_timeout };
        me.pins.enter_pass();                                   // pass_live = true; landed.clear()
        // ↓ 이 아래 모든 `?`는 me(Drop 보유)를 통과한다 → pass_live/landed 누수 불가
        me.recovered = super::reconcile::recover_graves(&me.layout).await?;   // collect **이전**
        me.refs      = super::reconcile::collect_referenced(&me.layout, &me.pins.hooks).await?;
        Ok(me)
    }
    pub(crate) fn referenced(&self)->&HashSet<String> { &self.refs }
    pub(crate) fn recovered(&self)->usize { self.recovered }

    /// blob → 무덤 rename + fsync. **성공했을 때만** `Graved`를 낳는다 — `Graved`의 **유일한 생성자**다.
    pub(crate) async fn grave<'p>(&'p self, sha:&str) -> io::Result<Graved<'p>> {
        atomic::rename_durable(&self.layout.blob_path(sha),
                               &self.layout.grave_path(sha),
                               &self.layout.objects_dir()).await?;   // ← 여기가 실패하면 Graved는 없다
        // 무덤 이름이 **자리잡은 뒤에** 코호트를 뜬다(P6). 이 rename 이후에 pin한 put은
        // blob_path에서 ENOENT를 보므로 **자급자족**이다 → 구조적으로 코호트 밖.
        let cohort = self.pins.cohort_at_grave(sha);
        self.pins.hooks.post_grave(sha).await;
        Ok(Graved { pass: self, sha: sha.into(), cohort })
    }
}
impl Drop for PassGuard { fn drop(&mut self){ /* pass_live=false; landed.clear() — 디스크 무접촉 */ } }

/// **무덤 rename 이후에만 존재할 수 있는 증거.** 파괴적 Drop 없음(흘리면 무덤 잔존 → 다음 패스 복구).
/// 필드 전부 private · **`pins.rs` 밖에 생성자 없음** · `Default`/`Clone`/`Copy` **유도 금지**.
#[must_use = "Graved를 흘리면 무덤이 남는다 — settle하라"]
pub(crate) struct Graved<'p> {
    pass:   &'p PassGuard,
    sha:    String,
    cohort: HashSet<u64>,   // 무덤 rename 시점에 살아있던 핀 id들 — **고정·유한 집합**
}
/// `Restored`/`Deferred`는 **디스크 전이가 동일**하다(무덤 → 정본). 갈라지는 것은 **왜**뿐이다:
/// `Restored` = **보호가 확정**됐다(landed) · `Deferred` = **결말을 알아내지 못했다**(타임아웃 → fail-CLOSED).
/// 변이를 나누는 이유는 **정직성**이다 — 타임아웃 복원을 `"GC restored: landed commit"`으로 로깅하면 **거짓말**이다.
pub(crate) enum Settled { Restored, Reaped, Deferred }

impl Graved<'_> {
    /// **보호 판정의 유일한 API.** 자기 자신을 **소비**한다 → 판정은 이 무덤 전이·이 sha에 바인딩된다.
    /// 판정만 따로 얻을 수단이 없고, `Graved` 없이는 호출할 수조차 없다(P4).
    /// **유한·fail-CLOSED**(P7): 멈춘 핀 하나가 GC를 **영구 정지시킬 수 없다**.
    pub(crate) async fn settle(self) -> io::Result<Settled> {
        let began = tokio::time::Instant::now();

        // ① **결말을 기다린다 — 단, 유한하게.** 무덤은 그동안 안전하게 보존된다(파괴 연산은 ③에서만).
        let outcome = self.pass.pins
            .await_settlement(&self.sha, &self.cohort, self.pass.settle_timeout).await;

        let (g, b, o) = (self.pass.layout.grave_path(&self.sha),
                         self.pass.layout.blob_path(&self.sha),
                         self.pass.layout.objects_dir());

        // ② **판정.** 보호 술어는 여전히 `landed` 하나뿐이다(P2).
        let (protect, verdict) = match outcome {
            // 보호 확정. (코호트 잔여 멤버의 결말은 판정을 바꿀 수 없다 — landed는 sticky·단일 술어)
            Settlement::Landed  => (true, Settled::Restored),
            // 결말을 **알고 나서** 판정한다.
            Settlement::Drained => match self.pass.pins.landed(&self.sha) {
                true  => (true,  Settled::Restored),
                false => (false, Settled::Reaped),          // ← 실패·취소·ENOSPC put: 오늘과 동일하게 회수
            },
            // **fail-CLOSED.** 결말을 알아내지 **못했다** → 보호 여부를 알 수 없다 → **보존을 택한다**.
            // 무덤을 정본으로 되돌리고, tombstone은 **유지** → 다음 패스가 **새 스냅샷으로 재판정**한다.
            // `gc_deleted`는 **증가하지 않는다**(회수하지 않았으므로).
            Settlement::TimedOut => {
                tracing::error!(
                    sha = %self.sha,
                    cohort_size = self.cohort.len(),
                    waited_ms = began.elapsed().as_millis() as u64,
                    "gc settle timed out — grave restored, reclamation deferred"
                );
                (true, Settled::Deferred)
            }
        };

        // ③ **파괴/복원은 판정 이후에만.** 어느 분기든 `?`로 탈출해도 무덤이 남을 뿐이다
        //    → 다음 패스의 `recover_graves`가 복원한다(fail-CLOSED by construction).
        if protect {
            self.pass.pins.hooks.restore_io(&self.sha)?;      // fault injection
            atomic::rename_durable(&g, &b, &o).await?;        // 되돌리기
            Ok(verdict)                                       // Restored | Deferred
        } else {
            tokio::fs::remove_file(&g).await?;                // **무덤 이름만** 지운다
            atomic::fsync_dir(&o).await?;
            Ok(Settled::Reaped)
        }
    }
}
```

**`Hooks` — 필드는 정확히 7개다. 늘리지 마라.**

```rust
type AsyncHook = Arc<dyn Fn(&str) -> BoxFuture<'static, ()> + Send + Sync>;
type SyncHook  = Arc<dyn Fn(&str) + Send + Sync>;
type FailHook  = Arc<dyn Fn(&str) -> io::Result<()> + Send + Sync>;
#[derive(Clone, Default)]
pub(crate) struct Hooks { post_observe: Option<AsyncHook>, during_collect: Option<AsyncHook>,
                          pre_grave: Option<AsyncHook>, post_grave: Option<AsyncHook>,
                          in_commit_pre_rename: Option<SyncHook>,
                          in_commit_post_landed: Option<SyncHook>,
                          restore_io: Option<FailHook> }
```

훅 배선(**전부 실제 호출부가 있다**): `collect_referenced(layout, &hooks)` · `PinGuard::blob_intact` 끝의
`post_observe` · `commit_pointer` 클로저 **안**의 동기 `in_commit_pre_rename` · `on_landed` 클로저 안,
`landed` 삽입·notify **직후**의 동기 `in_commit_post_landed` · GC 루프의 `pre_grave`(**B-2에서 배선**) ·
`grave()`의 `post_grave` · `settle()`의 `restore_io`.

> ⚠ **`notify_waiters()`가 `in_commit_post_landed` 훅보다 먼저 호출된다** — 이 **순서가 T-P4b-2의
> load-bearing 지점**이다(B-2). 알림이 나간 **뒤에** 클로저가 park하므로, **핀이 살아있는 채로** settlement가
> 깨어나는지를 관측할 수 있다. **뒤집지 마라.**

**배리어는 프로덕션 코드와 같은 경로를 지난다.** 배리어는 `BlobPins`가 소유한다(`hooks: Hooks`, prod = 전부
`None`). put 경로와 GC 경로가 **같은 등록부의 같은 훅**을 본다 → `#[cfg(test)]` 코드 경로 분기가 **없다**.

**`sift_corrupt` / `Sifted`는 포함하지 않는다**(→ F-25). **`protected()`도 없다**(보호 판정 API =
`Graved::settle` 하나뿐).

### 3.4 `src/store/objects.rs` — pin → blob_intact → commit_pointer

```rust
pub async fn put(&self, bucket:&str, key:&str, ct:&str, by:&str, bytes: Vec<u8>) -> Result<ObjectMeta, AppError> {
    let meta_target = self.meta_for(bucket, key)?;
    let sha = hex::encode(Sha256::digest(&bytes));
    let _g = self.locks.lock(bucket, key).await;

    let pin = self.pins.pin(&sha);                                   // ① blob을 **보기 전에** 핀(무대기)
    if !pin.blob_intact(&self.layout).await {                        // ② 관측은 핀을 통해서만
        atomic::write_atomic(&self.blob_path(&sha), &bytes).await.map_err(AppError::Internal)?;
    }
    let meta = ObjectMeta { /* ... 기존 그대로 ... */ };
    // ③ 커밋 — 핀을 **소비**하는 무취소 연산. 성공 = rename Ok = landed 마킹 완료.
    pin.commit_pointer(meta_target, serde_json::to_vec(&meta).unwrap())
       .await.map_err(AppError::Internal)?;
    Ok(meta)
}
```

**현행 `put`에서 치환되는 것**(나머지는 무변경):
- `let intact = matches!(tokio::fs::read(&blob).await, Ok(b) if …)` → `pin.blob_intact(&self.layout).await`
- 마지막 `atomic::write_atomic(&meta_target, &serde_json::to_vec(&meta).unwrap())`
  → `pin.commit_pointer(meta_target, serde_json::to_vec(&meta).unwrap())`

**`put_stream` 동형**: `stream_to_temp` 반환 직후(sha 확정) `pin()` → 기존 `existing_intact` 표현식을
`pin.blob_intact(&self.layout).await`로 치환 → temp→blob rename 분기 전체가 핀 아래 → 마지막
`write_atomic(meta_target)`을 `pin.commit_pointer(...)`로 치환.
**스트리밍 본문은 취소 가능한 채로 남는다**(`upload_timeout` 예산 불변). 무취소가 되는 것은
**메타 커밋(수백 바이트)뿐**이다.

### 3.5 `src/store/mod.rs`

```rust
pub struct Store { layout: Layout, locks: locks::KeyLocks, pins: pins::BlobPins }
impl Store {
    /// ⚠ **데이터 루트 하나당 Store는 정확히 하나**(D-3). 핀 등록부는 in-process이고 `clone()`이 Arc 공유한다.
    /// 같은 root로 `Store::new`를 두 번 부르면 등록부가 갈라져 reconcile이 다른 Store의 put을 보지 못한다
    /// → `reconcile-gc-dedup-race` 부활. 공유가 필요하면 **`Store::clone()`**을 써라.
    pub fn new(root: PathBuf) -> Self { ... }
    #[cfg(test)] pub(crate) fn with_hooks(root: PathBuf, hooks: pins::Hooks) -> Self { ... }
    pub(crate) fn layout(&self)->&Layout { &self.layout }
    pub(crate) fn pins(&self)->&pins::BlobPins { &self.pins }
}
```

`mod pins;`를 추가한다(**crate-private** — `pub mod`가 아니다). `Store`의 `#[derive(Clone)]`은 유지된다
(`BlobPins`가 `Clone` + 내부 `Arc` 공유).

**`Store::new`는 `pub` 유지**(D-3) — `pub(crate)` 축소는 crate 외부인 `tests/*.rs` 다수를 고쳐야 해
anti-cheat 게이트와 충돌할 위험이 있다. 전제는 **doc + 테스트**로 못박는다.

### 3.6 `src/store/reconcile.rs`

```rust
/// ⚠ `settle_timeout`은 **명시 인자**다. 기본값을 숨기지 않는다 — 이 값이 **유일한 상계**이므로
///    호출자가 그것을 **알고 정해야** 한다. prod = `settle_timeout_from(cfg.upload_timeout)`.
pub async fn run_once(store:&Store, gc_grace: Duration, settle_timeout: Duration)
    -> io::Result<ReconcileStats>                                                            // D-1
async fn run_once_at(store:&Store, now: SystemTime, gc_grace: Duration, settle_timeout: Duration)
    -> io::Result<ReconcileStats>
pub(super) async fn collect_referenced(layout:&Layout, hooks:&Hooks) -> io::Result<HashSet<String>>
                                        // ↑ 포인터 1개 낼 때마다 hooks.during_collect(sha).await
/// 잔존 무덤 **보수적** 복구 — `PassGuard::begin`이 collect **이전에** 호출.
pub(super) async fn recover_graves(layout:&Layout) -> io::Result<usize> {
    // Grave로 분류된 엔트리만. **file_type().is_dir() → skip**(무검증 파괴 경로 제거).
    //   blob 부재                      → rename(grave → blob)          // 복구
    //   blob 존재 ∧ 내용 sha == sha    → remove_file(grave)             // 정본이 검증 통과 → 무덤 폐기
    //   blob 존재 ∧ 내용 sha != sha    → rename(grave → blob)           // 정본이 썩었다 → **무덤을 채택**
    // 모든 전이 fsync_dir. 어느 경우든 이번 패스의 Blob 분기가 내용을 재검증한다.
}
```

**`settle_timeout` — 상계를 무엇으로 잡는가** (`pub` — `main.rs`가 쓴다):

```rust
/// 무취소 커밋 **꼬리**의 여유분. 이 꼬리는 `commit_pointer`의 blocking 클로저가 rename 전후로 수행하는
/// **고정 크기 작업**이다: mkdir_p + create + write_all(**메타 JSON 수백 바이트**) + sync_all(file)
/// + rename + sync_all(parent). 업로드 **크기에 비례하지 않는다** → 여유분은 **상수**가 맞다(비율 아님).
/// 건강한 디스크에서 한 자릿수 ms · blocking 풀이 대형 스크럽으로 포화돼도 1초 미만.
/// **60초 = 그 위로 두 자릿수 배의 헤드룸**이다.
pub const GC_SETTLE_MARGIN: Duration = Duration::from_secs(60);

/// **명시적 상계.** `upload_timeout`에서 **파생**하되 — ⚠ **`upload_timeout`은 상계가 아니다**.
pub fn settle_timeout_from(upload_timeout: Duration) -> Duration { upload_timeout + GC_SETTLE_MARGIN }
```

**GC 루프 본문 — B-1에서 바뀌는 것은 오직 이 세 줄 + `Grave` arm이다:**

```rust
let pass = PassGuard::begin(store, settle_timeout).await?;   // ① 등록 → 무덤 복구 → 참조 스냅샷
let refs = pass.referenced();
stats.referenced = refs.len();
// ... pending 로드 / now_secs / .objects 엔트리 스냅샷 / Reserved continue / is_dir continue: 기존 그대로
match class {
    ObjectsEntry::Temp  => { /* 기존 grace 로직 그대로 */ }
    ObjectsEntry::Grave => { /* 도달 불가(복구가 비웠다). **아무것도 하지 않는다** — 절대 삭제 금지 */ }
    ObjectsEntry::Blob  => {
        // ⚠ B-1: **비트로트 격리 분기 · GC 삭제 분기 둘 다 기존 코드 그대로.**
        //    무덤은 **만들어지지 않으며** `recover_graves`는 clean 트리에서 no-op이다.
        //    (B-2가 삭제 분기만 `pre_grave → pass.grave(sha) → settle()`로 교체한다.)
    }
    ObjectsEntry::Reserved | ObjectsEntry::Other => {}
}
```

⚠ **`run_once_at`은 더 이상 `Layout::new(root.to_path_buf())`를 만들지 않는다** — `store.layout()`을 쓴다.
그것이 D-1의 요점이다(경로 기반 API는 **자기만의 빈 핀 등록부를 든 두 번째 GC 소유자**를 만든다).

### 3.7 `src/main.rs`

`cfg`가 `build_state`로 **move되기 전에** `settle_timeout`을 계산(오늘 `gc_grace`를 그렇게 뽑는 것과 **동형**)
→ `build_state`를 **먼저**(그것이 `.objects`를 만든다) → 부트 `reconcile::run_once(&state.store, gc_grace,
settle_timeout)` → 주기 루프는 `state.store.clone()`을 move(**같은 Arc 등록부**).

```rust
let gc_grace = Duration::from_secs(cfg.gc_grace_secs);
let reconcile_interval = Duration::from_secs(cfg.gc_grace_secs);           // 기존 그대로
// **유일한 상계**. cfg가 move되기 전에 뽑는다(gc_grace와 동형).
let settle_timeout = reconcile::settle_timeout_from(Duration::from_secs(cfg.upload_timeout_secs));

let state = http::build_state(cfg)?;                                        // ← 여기서 cfg가 move된다
```

기본값: `600s + 60s = 660s`.
주기 루프의 `let dd = data_dir.clone();`는 **`let s = state.store.clone();`**로 바뀐다.
`MissedTickBehavior::Skip`(`main.rs:42`)과 `warn!`만 하고 다음 틱으로 넘어가는 에러 처리는 **그대로 유지**한다.

> **왜 `Config`에 새 env 노브를 안 만드는가**(`FILES_GC_SETTLE_TIMEOUT`): `bugfix-lock.json`의 `scope`는
> `["src/store/**", "src/main.rs", "src/layout.rs"]`이고 이 개정은 거기에 **아무것도 더하지 않는다**.
> `src/config.rs`를 열면 **설계가 커지고** 컨덕터의 "국소 수정만" 제약을 깬다. **`upload_timeout`에서의 파생은
> 순수 함수**이고 `main.rs`(**scope 안**)가 그것을 호출한다 → 노브는 **`FILES_UPLOAD_TIMEOUT` 하나로 유지**된다.

---

## 4. 호출부 전수 (B-1의 **기계 치환** 대상)

`git grep -n 'run_once' -- src tests`로 **재검증한 실측**이다(2026-07-13 기준 **일치 확인됨**):

| 파일 | 라인 | 비고 |
|---|---|---|
| `src/main.rs` | `:35`, `:48` | 부트 1회 + 주기 루프 |
| `src/store/reconcile.rs` | `:21` | `run_once` → `run_once_at` **내부 위임** |
| `src/store/reconcile.rs` (유닛 테스트) | `:175`, `:194`, `:196`, `:211`, `:212`, `:227`, `:241`, `:246` | `run_once`×2 + `run_once_at`×6 = **8곳 / 테스트 함수 5개**. **5개 중 4개는 `Store`를 아예 안 만든다** → `Store::new(root.to_path_buf())` 생성 추가 필요 |
| `tests/layout_tree.rs` | `:71`, `:137`, `:198` | 골든 트리 3종 |
| `tests/adversarial.rs` | `:91` | |
| `tests/regression_reconcile_gc_dedup_race.rs` | `:150` | 회귀 |

**합계 15곳**(+ 정의 2곳: `:20`, `:43`).

**같은 15곳이 `settle_timeout` 인자도 함께 받는다**(치환은 **한 번**에 끝난다). 값:
- `main.rs` = `settle_timeout_from(cfg.upload_timeout)`
- **골든/adversarial/회귀** = **발화하지 않을 넉넉한 값**
- **T-P4a**(B-2) = **200ms** · **T-P4b-1 · T-P4b-2**(B-2) = **30s**

**기본값을 숨긴 편의 오버로드를 만들지 않는다** — 이 값이 **유일한 상계**이므로 호출자가 **알고 정해야** 한다.

⚠ **회귀 테스트는 B-1에서 기계적으로 편집된다** — 그 `tokio::spawn`이 `root`를 move하므로 `&Store` 공유를
하려면 불가피하다. **단언은 한 글자도 바뀌지 않으며**, 변경은 `reconcile::run_once(&root, g)` →
`reconcile::run_once(&s2, g, st)`(+ **`Store::clone()`** 캡처)뿐이다. **anti-cheat 정면 지점이므로 diff를
릴리스 게이트에 제시한다.**

⚠ **`tests/adversarial.rs:91`의 `let _ = reconcile::run_once(…).await;`는 그대로 둔다** — 결과를 버리지만
**`.await`는 붙어 있다**(폴링됨). 이것은 **기존 계약**이며 B-1은 그 줄의 **인자만** 치환한다.

⚠ **회귀 테스트의 언랩을 지우지 마라**: `rec.await.unwrap().unwrap()`(JoinError + `io::Result` **둘 다** 언랩) ·
put 핸들의 `h.await.unwrap()` — **치환은 `&root` → `&s2`와 `settle_timeout` 인자 추가뿐이다.**

---

## 5. ⚠⚠ **D-3 함정 — 여기서 미끄러지면 영구 RED다**

> `regression:148-151`과 `adversarial.rs:88-95`는 `root.clone()`을 `tokio::spawn`에 넘긴다. 이걸
> **`Store::new(root)`로 재구성하면 등록부가 갈라진다** — spawn된 reconcile이 **다른 Store의 put을 절대 보지
> 못한다** → 핀도 landed도 안 보임 → **회귀 테스트가 영구 RED**로 남고, 원인은 프로덕션 버그가 아니라
> 테스트 배선이다(**디버깅에 하루를 태울 자리다**).
>
> ## **반드시 `let s2 = (*s).clone();`로 같은 `Store`를 클론해 넘겨라.**
>
> `layout_tree.rs:198`(mid-flight)도 **동일한 `s`**를 써야 한다.
> 이 함정은 D-3(같은 root에 `Store` 둘 = 버그 부활)의 **테스트 코드 판본**이다.

---

## 6. `dead_code` — B-1이 유일하게 허용하는 clippy 우회

B-1은 `pins.rs`를 **전문 그대로** 착지시키지만(§3.3 — Scope 표가 그렇게 정의한다), **GC 삭제 분기는 B-2에서
바뀌므로** 다음 항목들은 **B-1에 프로덕션 호출부가 없다**:

`Graved`(+ `impl`) · `PassGuard::grave` · `Settled` · `Settlement` · `await_settlement` · `cohort_at_grave` ·
`landed()` · `Hooks::{pre_grave, post_grave, restore_io}` 접근자 · `PassGuard::{recovered, pins}` (일부)

**규율**:
1. `#[allow(dead_code)]`를 **최소 집합에만** 단다. 먼저 `Graved`(+`impl`) · `PassGuard::grave` · `Settled`에만
   달고 `cargo clippy -D warnings`를 돌려 **남는 경고를 보고** 필요한 곳에만 추가하라(rustc의 dead-code
   reachability가 호출 그래프를 타고 내려가므로 대개 이 3개로 충분하다 — **추측하지 말고 실측하라**).
2. 각 attribute에 **`// B-2에서 제거 — 그때 배선된다`** 주석을 반드시 단다. 단 `PassGuard::recovered`는
   **B-3의 관측성(tracing)이 소비**하므로 B-2가 지울 수 없다 → 그것만 **`// B-3에서 제거`**로 단다.
3. **B-2의 acceptance는 `git grep -n 'allow(dead_code)' -- src/store/pins.rs` → `PassGuard::recovered`의
   1건을 제외하고 0건**을 요구한다(그 1건은 **B-3**이 제거한다). **그 attribute가 사라지는 것이 곧 배선의 증거다.**
4. **`pins.rs` 밖 어디에도 `#[allow]`를 추가하지 마라.** 그것은 anti-cheat 위반이다.
5. **`pins.rs`를 B-1/B-2로 쪼개지 마라** — structure 게이트가 심사하는 것은 **B-1의 diff**이고, Scope 표는
   `pins.rs` 전문을 B-1에 둔다.

---

## 7. B-1 acceptance (**플립 0**)

- [ ] `cargo test` **105 green**. **골든 `expected` 리스트 바이트 동일**(파일·디렉터리 추가 0).
      mid-flight `.tmp-*` 정확히 1개. `ReconcileStats` 전수 `assert_eq!` 3곳 불변
- [ ] 회귀 테스트는 **여전히 RED**(플립 미도달) — 단 `Store::clone()`으로 spawn하도록 고쳐진 상태.
      **테스트 파일 diff에 단언 변경 0줄**임을 diff로 증명
- [ ] `git grep -n 'run_once(&root'` → **0건** (경로 기반 API 잔존 0 — D-1)
- [ ] GC 삭제/격리 분기 **무변경**(diff로 증명). 무덤은 **만들어지지 않으며** `recover_graves`는 clean 트리에서 no-op
- [ ] 신규 유닛(layout): `classify_objects_entry_table`에 `.gc-grave-<64hex> → Grave` /
      `.gc-grave-junk → Other` / `.gc-pending.json → Reserved` 추가. `grave_name`/`grave_sha` round-trip
- [ ] 신규 유닛(pins): `pin()`은 **패스 보유 중에도 블록하지 않는다**(timeout 5s, `locks.rs` 관행) ·
      `commit_pointer` 성공 → `landed ∋ sha` ∧ `live[sha]` **비어 있음** · **stage 실패**(타깃 부모가 파일) →
      `landed` **무흔적** · **핀 id 단조성**: 같은 sha를 두 번 `pin()`하면 **서로 다른 id** 2개가 live에 들어가고,
      하나를 drop하면 나머지 하나는 남는다(코호트 판정의 전제)
- [ ] 신규 유닛(reconcile): `settle_timeout_from(600s) == 660s`(= `upload_timeout + GC_SETTLE_MARGIN`)
      ∧ **파생이 단조**(`upload_timeout`을 올리면 `settle_timeout`도 오른다) — 운영자가 `FILES_UPLOAD_TIMEOUT`을
      올렸을 때 **정상적으로 느린 put이 타임아웃되지 않음**을 못박는다(정상 경로 연기 = 0 유지)
- [ ] **T-C1 — 두 번째 플립 회귀 가드**(이 증분에서 이미 걸 수 있다): `b/k.meta.json` 위치에 **디렉터리**를 심어
      `rename`을 결정적으로 EISDIR 실패시킨다 → `put()` = `Err(Internal)` ∧ `landed` **무흔적** ∧ (만료·미참조
      blob에 대해) `run_once_at` → **`gc_deleted == 1`**
      · **뮤턴트 킬**: `on_landed`를 rename **앞**으로 이동(= "커밋을 **시도**했다"는 흔적) → 흔적 발생 →
        Restore → `gc_deleted == 0` → **결정적 RED**. (ENOSPC 무한연기의 기계 증인)
      · **랑데부**: **park 0 · spawn 0.** put은 reconcile **시작 전에** 완주(`Err`)하고 그 핀은 **이미 죽어
        있다** → **spawn ≠ polled 함정이 구조적으로 없다.** *"확인 안 함"이 아니라 "확인했고 없음"이다.*
      · ⚠ **T-C1의 한계(정직하게)**: 이 테스트는 **실패한 put이 이미 반환되고 그 핀이 죽은 뒤에** reconcile을
        돌린다 → **겹치는(overlapping) 실패 put**을 전혀 재현하지 못한다. 그 창의 증인은 **T-C3**(B-2)이며,
        T-C1은 `landed` 흔적의 **위치**만 지킨다. **T-C1을 "겹치는 실패 put"의 증인으로 제시하지 마라**
- [ ] **D-3 테스트**: `store.clone()`은 등록부 공유(`Arc::ptr_eq`) ∧ 같은 root의 `Store::new` 2개는
      **공유하지 않음**을 단언 — 해저드를 테스트로 못박는다
- [ ] `cargo clippy -D warnings` (§6의 `#[allow(dead_code)]` 최소 집합 외에 경고 0)

---

## 8. 보고 (완료 시 반드시 포함)

1. `cargo test` 출력 — **105 passed** ∧ 회귀 테스트 **RED 유지**(exit 101 · symptomToken `DATA LOSS` 존재).
2. `git diff`에서 **GC 삭제 분기 · 격리 분기 0줄 변경**임을 보여라.
3. 회귀 테스트 파일의 diff — **단언 변경 0줄**(인자 치환 + `Store::clone()`만).
4. `git grep -n 'run_once(&root'` → 0건.
5. **뮤턴트 킬 실증**: T-C1의 `on_landed` 이동 뮤턴트를 **실제로 적용**해 RED 출력을 캡처하고 **원복**한 뒤,
   그 출력을 붙여라. **주장은 증거가 아니다.**
6. `#[allow(dead_code)]`를 단 **정확한 위치 목록**(B-2가 전부 제거한다).
7. `cargo clippy -D warnings` 출력.

## Result

**커밋** `6399b6e` (증분 시작 fixed point `1f646ae`).

**B-1의 본질 확인**: 회귀 테스트는 **여전히 RED**다(`DATA LOSS` 토큰 존재). seam만
기립했고 GC 삭제/격리 분기는 **바이트 동일**이며, 핀·landed는 기록되지만 아무도 읽지
않는다. B-1에서 회귀가 GREEN이 됐다면 B-2를 미리 한 것이고 실패였을 것이다.

**증거**: characterization **115 passed**(105 + 신규 유닛 10 — 골든 트리·전수
`ReconcileStats` `assert_eq!` 3곳·mid-flight `.tmp-*` 1개 전부 불변). dead-code 경고 0.
`git grep 'run_once(&root'` → 0건(호출부 15곳 전수 치환).

**anti-cheat**: `tests/` diff에 **단언 변경 0줄** — 시그니처 치환 + `Store::clone()`
캡처(D-3) + 상수 1개뿐.

**컨덕터측 2축 리뷰**(fixed point `1f646ae`):
- **Spec 축 clean** — 8개 구속 조건 전부 충족. 세 가지 load-bearing 결정(무취소
  커밋 클로저 / 착지-기반 흔적 / `.gc-grave-` 평면 이름공간)이 명세대로 구현됐음을
  코드로 확인. 구현자가 보고한 5건의 명세 이탈은 전부 정당.
- **Standards 축 hard violation 1건** — `#[allow(dead_code)]`가 최소집합이 아니었다
  (7개 중 5개면 충분). 리뷰어가 **실측으로 반증**: rustc의 dead-code 패스는 allow 항목을
  live root로 시드하고 그래프를 따라 전파하므로 `Graved`·`Settled`의 allow는 지배당한다.
  → 5개로 축소, dead-code 경고 0 실측 확인. 그 외 5건 수정(B-2 grep 자멸 방지,
  `recovered`의 제거 시점을 B-3으로 정정, `write_atomic`의 버퍼 복사 정직 기록,
  fsync-dir 중복 4곳 단일화, 안전-임계 함수의 한 글자 변수명 개선).
- **Reject 1건**: Drop-poison 봉인은 계획서가 **B-3에 배정**했다(저장소 선례도 있다 —
  `capacity.rs`의 `Reservation::drop`).

**구현자의 정직한 지적 2건**(둘 다 검증됨):
1. 명세 §7의 T-C1 뮤턴트 킬 주장이 **문자 그대로는 성립하지 않는다** — T-C1의 put은
   라이브 `PassGuard` 없이 돌아 `pass_live == false`이므로 `landed` 삽입이 단락되고,
   `on_landed`를 rename 앞으로 옮기는 뮤턴트가 **살아남는다**(실측 확인). 구현자가
   `landed_trace_only_when_rename_returns_ok`(라이브 패스 안에서 실패하는 커밋을 돌린다)를
   추가해 그 뮤턴트를 **결정적으로 죽인다**. T-C1은 원래의 가치(실패한 커밋은 블롭을
   보호하지 못한다)와 원래의 한계를 그대로 유지한다.
2. `cargo clippy -D warnings`는 이 브랜치에서 **원래 통과 불가**다 — 기존 린트 5건이
   scope 밖 파일(`error.rs`·`capacity.rs`·`ranged.rs`)에 있다. 고치면 배리어 B4 위반.
   → 기준을 **"변경 파일 신규 경고 0"**으로 확정(실측 충족).
