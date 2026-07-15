---
bugfix: reconcile-vanished-entry-aborts-pass
invariant-class: bugfix
entry-track: bug
review-track: standard
pipeline-stage: done
issue-tracker: local
symptom: "reconcile가 .objects 스냅샷을 뜬 뒤 항목별 stat/read를 하는 사이, 동시 put_stream이 .tmp-<uniq>를 최종 blob 이름으로 rename하면, 사라진 경로에 대한 stat/read가 ENOENT를 하드 io::Error로 전파해 **패스 전체가 Err로 중단**된다(그 항목만 건너뛰는 게 아니라). 쓰기 트래픽이 있는 동안 reconcile이 사실상 완주하지 못해 GC·temp 정리·격리가 안 돌고 디스크가 찬다."
red-baseline: 3b1e44f608cd00d0a580a3f5deb595d85a28a9d9
bugfix-lock: red
first-increment: [B-1]
increments: [B-1]
spike-1:
---

# reconcile 패스가 스냅샷 이후 사라진 항목에 죽는다 (F-14)

> **D안 — 최소 픽스 (2026-07-14 · 인간 판정. C안 전면 폐기).**
> **부재 판정은 경로 기반이고 fd를 열지 않는다.** `Container`·핀 fd·`O_DIRECTORY`·`fstatat`·`(dev,ino)`
> 정체성·EMFILE fallback·fd 회계가 **코드와 문서에서 전부 사라진다** ⇒ **신규 외부 API 0 · `unsafe` 0 ·
> fd +0 · `Cargo.toml` 무변경.**
>
> **왜 전이했나**: 그 기계장치 전체가 오직 **P5**(*"컨테이너 파괴는 시끄럽다"*) 하나를 위해 존재했고,
> **P5는 설계된 계약이 아니라 `?`의 부수효과**였다. 그것을 지키려는 **컨테이너 생존 합취**가
> **P-12 → P-13 → P-14 → P-20 → P-22 · P-23**을 줄줄이 낳았다. 그리고 **실험이 증명했다**
> (`docs/reviews/reconcile-vanished-entry-aborts-pass/evidence-p21-refutation.md`): **봉인 장치 0**인 픽스로도
> 전 스위트 **120 passed / 0 failed** · **새 데이터 손실 경로 0**.
>
> **시끄러움은 값싸게 되찾는다** — 항목 루프 **이후**, `.gc-pending.json` 발행 **이전**에
> `metadata(.objects)` **1회**(§B). ⇒ **현실적 파국**(파괴 후 **미재생성** = SSD 언마운트 · 운영자 `rm -rf`)은
> **여전히 오늘과 같은 `Err(NotFound/2)`** 다(실측). **포기하는 것은 파괴-후-재생성이라는 적대적 ABA뿐이고
> 거기에도 데이터 손실은 없다**(Class B-ABA).
>
> ⚠ **red.sha는 그대로다**(`ac58bd7982d06e46f37cd4aa6a9c274d93bd8195`). 그 커밋은 **8번째 훅 `pre_entry`**
> (프로덕션 `None` ⇒ no-op)를 열고 **Temp 분기 RED 증인**을 추가해 `flips[]`를 2행으로 만들었다.
> **`?` 전파는 무변경이므로 버그는 살아 있다.** ⇒ **B-1이 여는 `pre_recover_grave`는 9번째 훅이다.**
>
> **모든 수치는 실행 결과다**(미수정 red.sha 트리 복제본에서 프로덕션 코드를 그대로 돌렸다 · T1~T5 ·
> W10-계열 실측 · 뮤턴트 5종 실측).

---

## Root cause

`run_once_at`은 `.objects` 직속 항목을 **`Vec<DirEntry>`로 스냅샷**한 뒤(`reconcile.rs:173-177`),
루프에서 항목별로 **경로를 다시 만진다**(stat/read/rename/remove). 스냅샷은 **뜬 순간의 사진**이고,
그 사이 동시 쓰기가 항목을 rename해 치우면 **사진 속 이름은 더 이상 디스크에 없다.**

그때 나는 `ErrorKind::NotFound`(ENOENT)를 코드가 *"이 항목은 이제 없다 = 할 일이 없다"*로 읽지 않고
**`?`로 전파**한다 → `run_once_at`이 **`Err`를 반환하며 루프를 통째로 탈출**한다.

**그러면 루프 끝의 이 줄이 영영 실행되지 않는다:**

```rust
// reconcile.rs:277 — 패스가 중단되면 여기에 도달하지 못한다
atomic::write_atomic(&pending_path, &serde_json::to_vec(&cleaned).unwrap()).await?;
```

`.gc-pending.json`이 재기록되지 않으면 **2단계 tombstone GC가 전진하지 못한다**(1단계 "최초 관측"이
지속화되지 않으므로 2단계 "grace 경과"에 영영 도달하지 못한다) → **미참조 blob이 회수되지 않는다**
→ **디스크가 찬다.** temp 정리·비트로트 격리도 같은 이유로 돌지 않는다. 이것이 증상의 메커니즘이다.

### 범인 — 루프 안에서 경로를 만지는 모든 연산

| # | 위치 | 연산 | 사라졌을 때 |
|---|---|---|---|
| ① | `reconcile.rs:206` | Temp 분기 `e.metadata().await?` | **ENOENT → 패스 중단**(계측 확정). 심링크 **무추종**(lstat 의미론) |
| ② | `reconcile.rs:215` | Blob 분기 `tokio::fs::read(&p).await?` | **ENOENT → 패스 중단**(계측 확정). `read`는 open이므로 **심링크를 추종**한다 |
| ③ | `reconcile.rs:199` | `e.file_type().await?` | **거의 도달 불가**. tokio는 **readdir 청크를 채우는 시점에** `std.file_type().ok()`를 부르고(`read_dir.rs:145`) 그 결과를 **캐시**한다(`:344-345`) ⇒ `d_type`을 채우는 FS(계측한 다섯 FS **전부**)에서는 **소멸한 항목에도 `Ok`가 나온다** |
| ④ | `reconcile.rs:209` | Temp `tokio::fs::remove_file(&p).await?` | 좁은 창(성공적 stat **이후** 소멸) → 같은 중단 |
| ⑤ | `reconcile.rs:218` | 격리 `tokio::fs::rename(&p, …)` | 좁은 창(성공적 read **이후** 소멸) → 같은 중단 |
| ⑥ | `reconcile.rs:238` | GC `pass.grave(&name)` 의 blob→무덤 rename | 좁은 창(성공적 read **이후** 소멸) → 같은 중단 |
| ⑦ | `reconcile.rs:110,120,123` | **`recover_graves`의 같은 스냅샷 루프** — `e.file_type()?` · `remove_file(grave)?` · `rename_durable(grave→blob)?` | 같은 중단. `PassGuard::begin`이 부르므로 **`run_once`가 그대로 `Err`** |

**근본 원인은 특정 syscall이 아니라 루프의 전제다**: *"스냅샷에 있으면 지금도 있다."* 그 전제는
**구조적으로 거짓**이다. ①②만 고치면 **증상 모양의 픽스**이고, 다음 사람이 루프에 syscall 하나를
더하면 버그가 부활한다.

### blast radius — `put_stream` 한정이 **아니다**

`atomic::write_atomic`(`atomic.rs:53,59`)이 **모든** blob 기록에 `.objects/.tmp-<uniq>` 생성 + rename을
쓴다 ⇒ **버퍼드 `put`도 같은 레이스를 만든다.** 진단이 버퍼드 `put` 40개 + reconcile 루프만으로
재현했다(20/20). "스트리밍 중에만 나는 버그"가 아니라 **쓰기 트래픽이 있는 한 상시**다.

### 프로덕션에서 무엇이 실제로 사라지는가 (도달성 표 — 정직하게)

| `.objects` 항목 | 사라뜨리는 프로덕션 경로 | 도달성 |
|---|---|---|
| `.tmp-<uniq>` | `write_atomic`의 rename(`atomic.rs:59`) · `put_stream`의 `rename(tmp→blob)`(`objects.rs:92`) · 실패/dedup 정리 `remove_file(&tmp)`(`objects.rs:80,89`) | **상시** — ①④가 여기서 발화한다 |
| `<sha>` blob | **단일 프로세스에서는 없다.** blob을 지우는 코드는 reconcile뿐이고 패스는 `pass_lock`으로 직렬화된다 | **아웃오브밴드 삭제**(운영자 `rm`·복구 도구) 또는 **D-3 해저드**(같은 root에 `Store::new` 2개 → 패스 2개 동시 진행). ②⑤⑥⑦이 여기서 발화한다 |
| `.gc-grave-<sha>` | reconcile만 만들고 지운다(직렬화) | 위와 동일(D-3·아웃오브밴드) — ⑦ |

정직한 요약: **①(Temp)은 정상 프로덕션에서 상시 발화하고, ②는 커밋된 RED 증인이 결정적으로 재현하지만
프로덕션 도달 경로는 아웃오브밴드/D-3다.** 그럼에도 ②를 고치는 이유는 **근본 원인이 syscall이 아니라
루프의 전제**이기 때문이다.

### 계측된 FS 사실 (load-bearing — 전부 직접 돌렸다 · std)

```
lstat(missingdir/child)         = Err(NotFound)          ← ★ 컨테이너 소멸이 "항목 부재"로 위장된다 ⇒ §B의 가드가 필요하다
lstat(regfile/child)            = Err(NotADirectory/20)  ← 일반 파일로 교체되면 B7이 무가공 전파한다
try_exists(dangling .corrupt)   = Ok(false)
create_dir(dangling .corrupt)   = Err(AlreadyExists)     ← atomic.rs:176이 삼킨다 → mkdir_p 통과
rename(blob → dangling .corrupt/blob) = Err(NotFound) ∧ lstat(src) = Ok  ← 목적지발 NotFound, 소스 잔존
rename(blob → regfile/blob)     = Err(NotADirectory)
read(symlink→dir, 절대경로)     = Err(IsADirectory)
rename(src → missing/t)         = Err(NotFound)
read_dir(missing)               = Err(NotFound/2)   ·   read_dir(regfile) = Err(NotADirectory/20)
```

**댕글링 심링크 — P-1 봉인의 근거 (실측)**

```
dangling: read()              = Err(NotFound/2)   ← 오늘 여기서 패스가 죽는다
dangling: symlink_metadata()  = Ok(symlink)       ← 항목이 **있다** ⇒ skip 금지 ⇒ 오늘의 Err를 바이트 보존
dangling: metadata()(follow)  = Err(NotFound/2)   ← 뮤턴트(M-FOLLOW)가 여기서 P-1을 깬다
vanished: symlink_metadata()  = Err(NotFound/2)   ← 진짜 소멸 ⇒ skip (유일한 플립)
```

**소멸한 항목에 대한 `DirEntry` 메서드의 실제 반응 (APFS · 스크래치 크레이트 실행)**

```
victim  (스냅샷 후 삭제)  de.file_type()=Ok(dir=false)  de.metadata()=Err(NotFound)  fs::read=Err(NotFound)
dangling(심링크)          de.file_type()=Ok(lnk=true)   de.metadata()=Ok             fs::read=Err(NotFound)
```

⇒ **`file_type()`은 `d_type` 캐시 때문에 소멸한 항목에도 `Ok`를 낸다** ⇒ **`Seen::Gone`이 `file_type()`
에서 나올 수 없다**(W2 명세·뮤턴트 표가 이것을 반영한다) ⇒ **`de.metadata()`는 lstat 의미론이다**
(댕글링 심링크 → `Ok`) ⇒ **W4가 성립한다.**

**`.objects`가 심링크→dir인 정상 배포 (실측)**

```
metadata(.objects).is_dir()          = true      ← 가드는 **follow**여야 한다
symlink_metadata(.objects).is_dir()  = false     ← no-follow면 정상 배포를 죽인다(M-GUARD-LSTAT)
```

---

## The fix — D안

### 0. GC의 보호 술어는 **둘**이다 — `refs` ∨ `landed` (⚠ **그러나 `refs`는 *하계*다**)

**이 사실이 문서에 없어서 P-21(critical)이 나왔다.** 명문화하되 **과장하지 않는다.**

| 술어 | 무엇을 덮나 | 언제 채워지나 | 권위 |
|---|---|---|---|
| **`refs`** | **패스 시작 시점**의 커밋 포인터 — ⚠ **하계(lower bound)** | `collect_referenced` — **항목 루프 이전**(1회) | `reconcile.rs:74` |
| **`landed`** (F-1의 **착지 흔적**) | **패스가 도는 *도중*에 착지한 커밋** | 커밋 rename이 **`Ok`를 낸 직후** — `g.pass_live && g.landed.insert(sha)` | **`pins.rs:376`** |

`refs`는 **사진**이고 `landed`는 **그 사진 이후에 일어난 일**이다. 무덤은 `refs`만 보고 파이지만,
**무덤을 정본으로 되돌릴지는 `landed`가 단독으로 정한다**(`pins.rs:266`의 `fn landed()`가 스스로
*"유일한 보호 술어"*라고 적혀 있다): `settle()`이 `Settlement::Landed`(`:250`) → `Settled::Restored`(`:586`)
→ **`restore_io` + `rename(무덤 → 정본)`**(`:610`)으로 **복원한다.**

⇒ **참조 수집 *이후에* 복원된 포인터의 blob은 `refs`에 없어도 죽지 않는다.** **쓰기 쪽**에서 커밋 포인터를
만드는 프로덕션 코드는 **`objects.rs:44`(`put`) · `:110`(`put_stream`) 둘뿐이고 둘 다 `pin.commit_pointer`를
지나므로 `landed`가 *반드시* 선다**(grep 전수 — `buckets.rs`가 쓰는 `.bucket.json`은 포인터가 아니다).
**실험으로 확인했다**(증거: `evidence-p21-refutation.md`).

> ⚠⚠ **정정 — 두 술어의 완전성 주장은 *읽기 쪽*에서 거짓이다** (r14 적대적 반증이 실행으로 잡았다).
> `collect_referenced`가 포인터 read/parse 실패를 **조용히 삼킨다**:
> ```rust
> // src/store/reconcile.rs:74-79
> if let Ok(raw) = tokio::fs::read(&entry.meta_path).await {      // ← EACCES · EIO · EMFILE 전부 삼킨다
>     if let Ok(meta) = serde_json::from_slice::<ObjectMeta>(&raw) { … refs.insert(meta.sha256); }
> }
> ```
> ⇒ **살아 있고 참조된 객체**의 `.meta.json` 읽기가 **일시적으로** 실패하면 그 sha는 `refs`에서 빠지고,
> 그 패스에 put이 없으므로 `landed`도 비어 있다 ⇒ **두 술어 모두 눈이 멀고** grace 경과 후 blob이
> **회수된다**. **실측**(포인터를 `0o000`으로 — 파일은 **존재**한다):
> `pass1 = referenced:0 · gc_pending:1` → `pass2 = gc_deleted:1` → **포인터 존재 ∧ blob 부재 ∧ GET = 404.**
> **red.sha에서 바이트 동일하게 재현된다 ⇒ 기존 구멍이며 D안이 만든 것이 아니다.**
> ⇒ **Class B-REFS**(§5) + **F-34**(백로그 — 등급 상향). **"`refs` ∨ `landed`가 모든 것을 덮는다"고
> 쓰지 않는다.** ⚠ EMFILE은 이 리뷰가 P-14로 이미 심각하게 다룬 실패 클래스다 — *"비현실적"*이라고
> **쓰지 않는다.**

**"`?`를 없앤다"가 아니다.** `NotFound`가 났고 **∧** 그 syscall이 만진 **소스 디렉터리 항목이 부재**할
때에**만** 그 항목을 건너뛴다. 다른 **모든** io 에러(EACCES·EIO·ENOSPC·EISDIR…)와 **다른 모든
`NotFound`**(댕글링 심링크 · 목적지 부재 · rename 이후 fsync 실패)는 **여전히 무가공 전파**한다(**B7**).
그러지 않으면 진짜 I/O 장애를 조용히 삼켜 **두 번째 관측 행동 플립**이 되고 **하드룰 10 위반**이다.

봉인 방식은 **둘**이다:
**(가) 부재의 증거를 타입으로 만든다**(`reconcile::absence::Absent` — 위조 불가) ·
**(나) 컨테이너 소멸은 *루프 이후의 가드*가 잡는다**(§B — `.objects`가 죽으면 항목별 `NotFound`가 항목
부재로 **위장**되기 때문이다. 실측: `lstat(missingdir/child)` = ENOENT).

> ⚠ **되살리기 금지 못** (C안이 실측으로 반려한 것 — §C-2의 원칙): **아무 일도 하지 않거나 새 실패를
> 날조하는 술어는 넣지 않는다.** `nlink > 0` 합취(경로 stat이 성공한 이상 **구조적 상수 참** — 여섯 FS
> 실측) · `is_dir()` **합취**(일반 파일이 ino를 탈취해도 `lstat`은 **ENOTDIR**이지 ENOENT가 아니므로
> 부재 판정이 **이미** 실패한다 — 위조 0/200) · `de.ino()` 합취(존재 팔의 ino 불일치를 "부재"로 읽는 것은
> **합취가 아니라 이접** = 살아 있는 항목을 skip하는 **비보수적 확장**) · **W8/W12(소스-문자열 규율)**.

### A. 오늘 vs D안 — `run_once_at` 엔트리 루프 한 줄씩 대조

**원칙: 바뀌는 것은 `?` 하나의 팔과 루프 뒤 한 줄뿐이다. fd·핀·정체성은 전부 사라진다.**

| # | 오늘 (red.sha `ac58bd7`) | D안 | 델타 |
|---|---|---|---|
| 0 | *(없음)* | *(없음)* | **핀 `open`/`fstat` 삭제 ⇒ syscall +0 · fd +0** |
| 1 | `read_dir(&objects).await?` + `next_entry().await?` (`:174-177`) | **글자 그대로 동일**(`Entry::snapshot`이 감싸기만) | 0. ⚠ **이 `?`가 `recover_graves` 이후 컨테이너의 사실상의 가드다**(§D) |
| 2 | `let p = e.path();` (`:180`) | **`de.path()` 그대로**, `Entry.path`에 1회 보관. 커널에 넘기는 **원시 바이트 불변**(P15) | 0 |
| 3 | `e.file_name().to_string_lossy()` (`:181`) | 동일 → `Entry.name`(lossy). 용도 = 분류·로깅·원장 키·목적지 이름(**오늘과 동일**) | 0 |
| 4 | `classify_objects_entry(&name)` (`:185`) | 동일(이름-전용 ⇒ syscall 0 ⇒ **O1 불변**) | 0 |
| 5 | `if Reserved { continue }` (`:188`) — `file_type` **이전** | 동일 (**O1**) | 0 |
| 6 | `hooks().pre_entry(&name).await` (`:197`) | **한 글자도 안 바꾼다**(red.sha가 이미 열었다) | 0 |
| 7 | `e.file_type().await?` (`:199`) | `entry.file_type()` → `Seen<FileType>` — **내부는 같은 `de.file_type()`** | **0. `Gone` 팔은 영영 발화하지 않는다**(d_type 캐시) |
| 8 | `if ft.is_dir() { continue }` (`:200`) | 동일 (**O2**) | 0 |
| 9 | `e.metadata().await?` (`:206`) | `entry.metadata()` → `Seen<Metadata>`. 호출부는 `.modified().unwrap_or(now)` **축자 보존**. lstat 의미론 유지 | `NotFound` 시에만 **`symlink_metadata(e.path())` 1회** |
| 10 | `remove_file(&p).await?` (`:209`) | `entry.remove()` → `Seen<()>`. **`temps_deleted += 1`은 `Present` 팔에서만** | 〃 |
| 11 | `tokio::fs::read(&p).await?` (`:215`) | `entry.read()` → `Seen<Vec<u8>>` | 〃 (**여기서만 확인이 load-bearing** — `read`=open이라 심링크를 **추종**한다) |
| 12 | `mkdir_p_durable(&corrupt_dir).await?` (`:217`) | **동일 · raw `?`** | 0. ⚠ **이것이 `.objects`를 되살린다 — §C의 유일한 자기무효화 지점** |
| 13 | `rename(&p, corrupt_dir.join(&name)).await?` (`:218`) | `entry.rename_into(&corrupt_dir)` → `Seen<()>`. 소스=`self.path`(원시) · 목적지=`dir.join(&name)`(lossy) — **오늘과 같은 짝** | 〃 |
| 14 | `fsync_dir(&objects).await?` (`:219`) | **동일 · raw `?`**(rename `Ok` 이후 ⇒ **P-2**) | 0 |
| 15 | `pending.remove(&name)`(`:220`) · `hooks().pre_grave`(`:235`) | **동일** | 0 |
| 16 | `pass.grave(&name).await?` (`:238`) | **`pass.grave(&name, &vanished)`** → `GraveOutcome{Moved, SourceGone}`. ⚠ **소스 부재 분류는 반드시 `reconcile::absence::rename_durable_source_checked` → `rename_checked_blocking`을 경유한다**(§③) — **`SourceGone`은 `std::fs::rename`의 `Err` 팔에서만 태어난다**. **`Option<&Container>` 인자는 삭제**(M30 계열 소멸) · **새 인자 `&Vanished`는 쓰기 전용 계수기라 아무것도 위조할 수 없다** | rename `Err(NotFound)` 팔에서만 소스 확인 |
| 17 | `.settle().await?` (`:238`) | **한 글자도 안 바꾼다 · raw `?`** | 0 |
| 18 | *(없음)* | **★ 루프-후 컨테이너 가드**(§B) | **소멸이 1건 이상일 때에만** `metadata(objects)` 1회 |
| 19 | `try_exists(blob_path)`(`:272`) · `write_atomic(pending)`(`:277`) | **동일 · raw `?`** · **§E: 추가 `pending.remove`는 넣지 않는다** | 0 |

> ⚠ **행 16의 함정 (r14 적대적 반증이 프로브로 실증했다 — 숨기지 않는다)**: `atomic::rename_durable`은
> **rename과 fsync를 융합**한다(`rename Ok` → `File::open(parent)` = `NotFound`). 거기에 부재 확인을 붙이면
> **rename이 성공한 뒤의 fsync ENOENT가 `SourceGone`으로 위조**된다(우리가 옮겼으니 소스는 당연히 부재다)
> ⇒ `settle()` 미호출 ⇒ **M6 부활**. ⇒ **`rename_durable`은 그대로 두고**, 소스 확인은 **`rename_checked_blocking`
> 안 `std::fs::rename`의 `Err` 팔 전용**이다. **§③이 이것을 타입으로 못박는다.**

**부재 판정 = 경로 기반, fd 0. 그리고 계수는 부재 판정과 *같은 행위*다.**

> ⚠⚠ **위치가 봉인이다 (r15/P-27 · 실컴파일 확정)** — 이 타입들은 **`atomic.rs`에 살 수 없다**: 거기서
> `pub(in crate::store::reconcile)`은 **`E0433`**(`pub(in …)`은 **조상 모듈만** 허용)이고, `pub(super)`(=`store`)나
> `pub(crate)`면 **`pins`가 대체 집계를 짓는다**(둘 다 BUILD OK — `p27-a`/`p27-b`) ⇒ **신규 `src/store/reconcile/absence.rs`**
> (= `reconcile`의 자식). 거기서 `pub(super)` = **`pub(in crate::store::reconcile)`** = **reconcile 서브트리 전용**
> = **pins·atomic 제외**. ⚠ **`absence`의 부모는 `reconcile`이지 `store`가 아니다** — 이 한 글자가 봉인 전체를 진다.

### A-0. **import / re-export 맵 — 한 곳에 못박는다** (r16/P-28 · **스크래치 크레이트 실컴파일 확정**)

**`atomic.rs`의 기존 API는 *전부 그대로* 남는다 — 삭제 0 · 이동 0**(red.sha `ac58bd7` 기준 전수):
**`write_atomic`**(`pub`) · **`fsync_dir`**(`pub`) · **`mkdir_p_durable`**(`pub`) ·
**`rename_durable`**(`pub(crate)` · rename+fsync **융합** · **fail-CLOSED 유지** — `Graved::settle`의 복원
rename이 쓴다) · **`rename_durable_blocking`**(`pub(crate)`) · **`unique_suffix`**(`pub(crate)`) ·
**F-1의 취소불가 커밋 파이프라인 `stage_blocking` · `Staged` · `Staged::commit_blocking`**(전부 `pub(crate)`) ·
모듈 private **`fsync_dir_blocking`** · **`mkdir_p_durable_blocking`**.
⚠ **이 목록에서 하나라도 지우면 red.sha가 컴파일되지 않는다**: `pins.rs:369-371`(`PinGuard::commit_pointer`)이
`atomic::stage_blocking` → `Staged::commit_blocking`을 부르고 · `objects.rs:75`(`put_stream`)가
`atomic::unique_suffix`를 부르며 · `rename_durable_blocking`/`mkdir_p_durable_blocking`은 `rename_durable`·
`stage_blocking`이 내부에서 부른다. **그것들을 재작성하면 F-14와 무관한 취소·내구성 회귀**(T-C1/T-C2/T-R2a가
지키는 계약)**를 부른다.**

**이번 픽스가 `atomic.rs`에 가하는 변경은 정확히 둘이다 — 그리고 둘 다 그 파일의 *본문*을 건드리지 않는다:**

1. **부재 관련 심볼은 `atomic.rs`에 *신설되지 않는다*.** `Absent` · `Vanished` · `Renamed` ·
   `entry_is_absent{,_blocking}` · `rename_{source,durable_source}_checked` · `rename_checked_blocking`은
   **`src/store/reconcile/absence.rs`에 신설**된다(위 ⚠⚠ — 라운드 16의 **컴파일 증거**).
   ⚠ **"`atomic.rs`에서 옮긴다"가 아니다** — **red.sha의 `atomic.rs`에는 그 심볼이 하나도 없다**(F-14가
   **새로 만드는** 것이다). 옮겨진 것은 코드가 아니라 **초기 개정판의 *계획*** 이고, 그 계획을 r15/P-27이
   컴파일로 반려했다. ⇒ **`atomic.rs`에는 그 심볼이 하나도 생기지 않는다.**
2. **`fsync_dir_blocking`의 *가시성만* 넓힌다** — 모듈 private → **`pub(crate)`**(§Scope). `absence.rs`가
   rename+fsync를 **한 무취소 클로저**에 유지하려면 필요하다(M6 봉인). **시그니처·본문·syscall 시퀀스는
   한 글자도 바뀌지 않는다.**

| 심볼 | 산지 | 선언 가시성 | **실효 도달 범위** | 누가 쓰나 |
|---|---|---|---|---|
| `Absent` | `absence` | `pub(crate)` (필드 `()` **private**) | 크레이트 전역(**주조는 `absence` 안에서만**) | `entry::Seen::Gone` · `absence::Renamed::SourceGone` · **`pins::GraveOutcome::SourceGone`** |
| `Vanished` | `absence` | `pub(crate)` (**derive 0**) | 크레이트 전역(**타입 이름만**) | `reconcile`(생성·판독) · `entry`(빌림) · **`pins`(빌려서 전달만)** |
| `Vanished::{new,get}` | `absence` | **`pub(super)`** = `pub(in …::reconcile)` | **reconcile 서브트리 전용** | `run_once_at`(`new` 1회 · `get` = 가드) |
| `Vanished::{bump,share}` | `absence` | **모듈 private** | `absence` 안 | `entry_is_absent{,_blocking}` · `rename_*_source_checked` |
| `Vanished::new_for_test` | `absence` | `#[cfg(test)] pub(crate)` | 크레이트 전역(**테스트 빌드만**) | **`pins::tests` 9개 호출부** — B-TESTBRIDGE |
| `Renamed` | `absence` | `pub(crate)` | 크레이트 전역 | `entry::rename_*` · **`pins::grave`** |
| `rename_durable_source_checked` | `absence` | **`pub(crate)` — 좁힐 수 없다** | **재수출된 만큼만** ↓ | `entry::rename_durable_to` · **`pins::grave`** |
| `rename_source_checked` | `absence` | **`pub(super)`** (r18 프로토타입에서 축소 확정) | 재수출 **불필요**(자손이 `super::absence::`로 직행) | `entry::rename_into` |
| `entry_is_absent` | `absence` | **`pub(super)`** (〃) | 〃 | `entry::seen` |
| `entry_is_absent_blocking` · `rename_checked_blocking` | `absence` | **모듈 private** | `absence` 안 | `rename_*_source_checked` |
| `Seen<T>` · `Entry<'v>` | `entry` | `pub(super)` | reconcile 서브트리 | `reconcile.rs` 루프 |
| `recover_graves{,_from}` | `reconcile` | `pub(super)` = `pub(in …::store)` | **store 서브트리** ⇒ **`pins`에서 호출 가능** | **`PassGuard::begin`** |

```rust
// src/store/reconcile.rs
mod absence;                     // ← **private 모듈** (pins는 `reconcile::absence::…` 경로를 지나갈 수 없다)
mod entry;
pub(crate) use absence::{rename_durable_source_checked, Absent, Renamed, Vanished};
//                       ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ ⚠⚠ **"타입만"이면 컴파일되지 않는다**

// src/store/pins.rs
use super::reconcile::{rename_durable_source_checked, Absent, Renamed, Vanished};
```

> ⚠⚠ **r16이 잡은 실제 컴파일 오류 — "타입만 재수출한다"는 거짓이었다.** `mod absence`가 **private**이므로
> `pins`는 `reconcile::absence::rename_durable_source_checked`를 **경로로 지나갈 수 없다**. 타입만 재수출하면
> `grave()`가 **`E0425`(cannot find function … in module `super::reconcile`)** 로 죽는다(스크래치 실컴파일).
> ⇒ **`rename_durable_source_checked`를 재수출 목록에 반드시 넣는다.**
> **비대칭이 핵심이다**: **연관함수는 타입을 타고 따라온다**(`Vanished::new_for_test()`는 타입 재수출만으로
> `pins::tests`에서 **부를 수 있다** — 실행 확인) · **자유함수는 스스로 재수출돼야 한다.**
>
> **그리고 재수출은 봉인을 넓히지 않는다 — 메서드 가시성은 따라오지 않는다**(실컴파일):
> `pins`에서 **`Vanished::new()` = `E0624`** · **`.get()` = `E0624`** · **`Absent(())` = `E0423`**
> ⇒ **`pins`는 집계를 *짓지도 읽지도 올리지도* 못하고 오직 *빌려서 전달*만 한다** ⇒ **M-FRESH/M-BUMP-OUTSIDE/
> M-GET-IN-PINS/M8이 프로덕션 빌드에서 표현 불가** ⇒ **라운드 16의 결론이 `pins`에서 그대로 성립한다. 모순 없다.**
> ⚠ **r16의 "최소 수정 제안"을 프로토타입이 *채택하고 컴파일했다*** — `entry_is_absent`/`rename_source_checked`는
> **`pub(super)`로 좁혔다**(선언 = 실효). **봉인에는 무해**하고 B-5 diff 항목으로 남는다.
> ⚠⚠ **그러나 `rename_durable_source_checked`는 `pub(crate)`여야 한다 — 좁히면 컴파일되지 않는다**(r18 프로토타입의
> 컴파일 증거): `pub(crate) use`는 **가시성을 넓힐 수 없다**(**`E0364`**) ⇒ `pub(super)`인 항목은 `pub(crate)`로
> 재수출할 수 없고, `pins::grave`가 그것을 필요로 한다. **좁힌 것은 정확히 둘뿐이다.**

```rust
// src/store/reconcile/absence.rs
/// **"소스 항목이 부재함"의 증거.** 필드 private ⇒ **이 모듈 밖에서 생성 불가** ⇒ 부모는 `Seen::Gone`·
/// `Renamed::SourceGone`·`GraveOutcome::SourceGone`을 **합성할 수 없다**(`E0423`).
pub(crate) struct Absent(());

/// **소멸 계수기.** ⚠ **derive 0개** — `Default`·`Clone`·`Copy`·`Debug` **전부 없다**(복제본이 곧 대체 집계다).
/// `Arc`가 남는 **유일한** 이유: private `share()`가 `spawn_blocking`의 `'static` 클로저로 **같은** 집계를 나른다.
pub(crate) struct Vanished(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl Vanished {
    pub(super) fn new() -> Self;         // ← **reconcile 서브트리 전용**. 크레이트 전체 호출부 = `run_once_at` 1개
    pub(super) fn get(&self) -> usize;   // ← 루프-후 가드만 읽는다(`pins`에서 부르면 `E0624`)
    fn bump(&self);                      // ← **absence 모듈 private**
    fn share(&self) -> Vanished;         // ← **absence 모듈 private** · Arc 공유 = **같은 집계**
    #[cfg(test)] pub(crate) fn new_for_test() -> Self;   // ⚠ **테스트 다리** — §Scope 참조. Class B-TESTBRIDGE
}

/// **부재 판정의 유일한 정의.** `Absent(())` 리터럴도 `bump()`도 **`entry_is_absent{,_blocking}` 안에 1회씩만**
/// 등장한다. ⚠ **두 채널이므로 `bump()` 누락 뮤턴트는 *2행*이다**(M-NOBUMP-ASYNC/-BLOCKING — "표현 불가"는 거짓).
pub(super) async fn entry_is_absent(tally: &Vanished, path: &Path) -> Option<Absent> {
    match tokio::fs::symlink_metadata(path).await {   // ⚠ **no-follow** — P-1 봉인
        Err(e) if e.kind() == ErrorKind::NotFound => { tally.bump(); Some(Absent(())) }
        _ => None,   // Ok(_) = 항목이 **있다**(댕글링 심링크 포함) · 그 외 Err = 확인 불가 ⇒ 보수적
    }
}
fn entry_is_absent_blocking(tally: &Vanished, path: &Path) -> Option<Absent>;   // rename_checked_blocking 전용
```

**⇒ 전문은 §구현 ①에 있다 — 그것이 실제로 컴파일된 소스다.**

> ⚠⚠ **정직한 한계 — "M-FRESH 표현 불가"는 *모듈 간 · 프로덕션 빌드에서만* 참이다** (실컴파일):
> **(1) 모듈 간 봉인은 선다** — `pins::grave`의 대체 집계는 `E0624`/`E0599`/`E0423`(§뮤턴트 표 M-FRESH).
> **(2) 모듈 *안*은 못 막는다** — `run_once_at`의 `let decoy = Vanished::new()`(**M-FRESH′**)는 **BUILD OK**.
> **(3) 테스트 다리가 `cargo test --lib`에서 봉인을 연다** — `pins::tests`의 **9개 호출부**(§Scope)가 `&Vanished`를
> 요구해 `new_for_test()`가 **필수**이고 **뮤턴트가 평가되는 빌드가 바로 그 빌드**다 ⇒ M-FRESH가 거기서 컴파일된다.
> ⇒ **(2)(3) 때문에 M-FRESH를 A(타입) → A(행동)으로 강등하고 증인으로 덮는다 — 킬러 = `W-GRAVE-CD-A`(§C-A) · B-TESTBRIDGE.**

**아무것도 안 바뀌는 것**: `collect_referenced` · `CommitPointerWalk` · pending 로드/저장 ·
**`ReconcileStats`** · `Graved`/`Settled`/`settle()` · **`atomic.rs`의 기존 API 전부**
(`write_atomic`/`rename_durable{,_blocking}`/`mkdir_p_durable{,_blocking}`/`fsync_dir` · **커밋 파이프라인
`stage_blocking`/`Staged::commit_blocking`/`unique_suffix`** — `fsync_dir_blocking`은 **가시성만** 넓어지고
본문은 불변) · 루프 밖 삼킴 3곳(`:74`·`:115`·`:166`) · **`Cargo.toml`** · **`unsafe` 0**(자명).

---

## B. 루프-후 컨테이너 가드 — 위치 · 의미론 · 오늘과의 차이

### B-1. 위치와 코드

```rust
    }   // ← 엔트리 for 루프 끝

    // ── 컨테이너 가드 ───────────────────────────────────────────────────────────────
    // ⚠ **반드시 `write_atomic` 이전**: `write_atomic` → `mkdir_p_durable(.objects)`가 컨테이너를
    //    **되살린다**(실측 T1/T3) ⇒ 뒤로 옮기면 가드는 **영영 참**이 된다(뮤턴트 M-GUARD-AFTER).
    // ⚠ **`vanished.get() > 0`으로 게이트한다**(§B-3). 게이트가 없으면 **오늘 `Ok`인 패스가 `Err`가 되는
    //    새 실패 클래스**가 생긴다(= 두 번째 플립 · 뮤턴트 M-GUARD-ALWAYS).
    // ⚠ **`metadata`(follow)** — `symlink_metadata`면 `.objects`가 심링크→dir인 정상 배포를 죽인다(실측).
    if vanished.get() > 0 {
        match tokio::fs::metadata(&objects).await {
            Ok(m) if m.is_dir() => {}
            Ok(_)  => return Err(std::io::Error::from(ErrorKind::NotADirectory)),   // ⚠ **합성 에러** — §I B″
            Err(e) => return Err(e),   // ← **무가공**. `.objects` 부재 = ENOENT/2 = **오늘과 같은 kind·errno**
        }
    }
```

`vanished`는 **`run_once_at`의 지역 `Vanished`**이고 `Entry`·`recover_graves`·`grave()`는 **`&`로 빌리기만 한다**
(클론이 곧 대체 집계다 — r15). **`ReconcileStats`에 올리지 않는다**(P10 — 필드 추가 0).

### B-2. 파괴된 세계의 루프를 **코드로 추적한다** (추측하지 않고 쟀다)

```
[destroyed] lstat(entry)                = Err(NotFound/2)
[destroyed] read(entry)                 = Err(NotFound/2)
[destroyed] remove_file(entry)          = Err(NotFound/2)
[destroyed] rename(entry -> .corrupt/x) = Err(NotFound/2)
[destroyed] rename(blob -> grave)       = Err(NotFound/2)
```

⇒ **`.objects`가 파괴된 세계에서 루프가 하는 일은 정확히 0이다:**

| 분기 | 파괴 후 실제로 무엇이 일어나나 | 카운터 |
|---|---|---|
| `file_type()` | **캐시 히트 `Ok`**(syscall 0) → `is_dir()=false` → 진행 | — |
| Temp | `metadata()` → ENOENT → 확인 → `Gone` → **skip**. `remove_file`은 **호출되지 않는다** | `temps_deleted` **불변** |
| Blob | `read()` → ENOENT → `Gone` → **skip**. **격리도 `grave()`도 도달하지 못한다**(둘 다 `read` `Ok` 뒤에 있다) | `quarantined`·`gc_deleted` **불변** |
| Grave/Other/Reserved | 본문 없음 | — |

⇒ **"더 많은 reap/격리/temp 삭제가 일어난다"는 파괴된 세계에서 거짓이다.** 파괴 이후의 추가 반복은
**syscall 실패 N회 + skip N회**가 전부다 ⇒ **관측 가능한 부수효과 0 · 카운터 0.**
**그리고 stats가 달라져도 관측 불가하다**: 두 세계 모두 `Err`를 반환하므로 stats는 **드롭된다**
(`main.rs:38,51`이 `Err(e) => tracing::warn!`). **이중 봉인.**

**실측 (미수정 red.sha 트리 · T2 — blob 3개, 첫 `pre_entry`에서 `.objects` 삭제)**

```
[T2] 오늘        = Err((NotFound, 2, "No such file or directory (os error 2)"))
                   .objects 부활=false   원장=false   pre_entry 발화 1회  ← 오늘은 루프가 **여기서 죽는다**
[T2] D안         = Err((NotFound, 2, "No such file or directory (os error 2)"))
                   .objects 부활=false   원장=false   pre_entry 발화 **3회** ← 끝까지 돌고 **가드가** 낸다
```

⇒ **호출자가 보는 것이 바이트 동일하다.** 같은 무대를 **Temp 클래스만**으로 바꿔도 동일하다(실측 W10-TEMP:
base `Err(NotFound/2)` · D안 `Err(NotFound/2)` · 부활 0 · 원장 0).

### B-3. **`vanished > 0` 게이트가 load-bearing이다 — 게이트가 없으면 두 번째 플립이 생긴다**

**꼬리 파괴**(마지막 FS-접촉 항목 **이후**에 컨테이너가 죽는 창)에서 **오늘이 무엇을 하는지** 쟀다.
무대: `Other` 클래스 항목 **하나뿐**(63자 이름 ⇒ 분기 본문이 비어 있다 = syscall 0), 그 항목의
`pre_entry`에서 `.objects` 삭제:

```
[T1 꼬리파괴] 오늘의 반환값  = Ok(ReconcileStats{referenced:0, gc_deleted:0, gc_pending:0, temps_deleted:0, quarantined:0})
[T1 꼬리파괴] .objects 부활  = true    ← write_atomic → mkdir_p_durable이 **되살렸다**
[T1 꼬리파괴] 원장          = "{}"    ← 심어 둔 {"deadbeef":1}이 **지워졌다**
```

⇒ **오늘 이 창은 조용히 `Ok`다.** 무조건 가드는 이것을 **`Err`로 뒤집는다 = 새 실패 클래스 = 두 번째
관측 플립.** ⇒ **`vanished > 0` 게이트가 그것을 정확히 막는다**(꼬리 파괴에서는 소멸이 **0건**이므로
가드가 **발화하지 않는다**). **실측으로 확인**: 게이트를 제거한 뮤턴트(`m_always`)에서 **꼬리 파괴가
`Ok` → `Err(NotFound/2)`로 뒤집힌다.**

**게이트의 전수 논증 (모든 모양 — r14 반증이 요구한 두 세계를 추가했다)**

| # | 세계 | 소멸 수 | 오늘 | D안(게이트 있음) | 동일? |
|---|---|---|---|---|---|
| 1 | 정상 패스(소멸 0) | 0 | `Ok` | **가드 미발화**(syscall 0) → `Ok` | **바이트 동일** ✔ (실측: `m_noguard`와 D안이 **같은 `Ok(…gc_pending:1)`**) |
| 2 | 항목 1개 소멸(컨테이너 생존) | ≥1 | `Err` | 가드 → `metadata` = `Ok(dir)` → **`Ok`(완주)** | **← 유일한 플립** |
| 3 | 컨테이너 파괴(항목 접촉 잔존 · Blob 또는 Temp) | ≥1 | `Err(NotFound/2)` | 가드 → **`Err(NotFound/2)`**(무가공) | ✔ **실측 T2 · W10-TEMP** — ⚠ **문언 주의**: 가드의 `metadata`가 **`NotFound` 아닌 에러**(EACCES/EIO/ELOOP)를 낼 수 있는 세계에서는 **오늘 `Err`인 패스의 kind가 달라질 수 있다**(둘 다 `Err` = **극성 동일** · 전파는 **무가공**). ***"바이트 동일"이라고 쓰지 않는다.*** |
| 4 | 컨테이너가 **일반 파일로 교체** | ≥1 | `Err(NotADirectory/20)` | **항목 연산이 먼저 ENOTDIR을 낸다** ⇒ **B7이 무가공 전파**(가드에 닿지도 않는다) | **바이트 동일** ✔ (실측: base·D안 둘 다 `(NotADirectory, 20)`) |
| 5 | 컨테이너 **꼬리** 파괴(접촉 0) | **0** | **`Ok`** + 부활 + 원장 `{}` | **가드 미발화** → **`Ok`** + 부활 + 원장 `{}` | ✔ (실측 T1 — **오늘의 버그를 보존한다** · F-42) |
| 6 | 컨테이너 파괴 → **재생성**(엔트리 루프 안) | ≥1 | `Err` | 가드가 **살아 있는 dir**을 본다 → **`Ok`** | **✗ Class B-ABA**(인간이 채택한 포기 · **데이터 손실 0**) |
| 7 | 컨테이너 파괴 → **재생성**(**무덤 루프 안** — r14 반증 신규) | **≥1**(r15 정정) | `Err`(`recover_graves`의 `?`) | 무덤 루프의 부재가 **패스 집계를 올린다**(A2·A4 — r15/P-27 수리) → `read_dir` = **`Ok(빈 디렉터리)`** → 엔트리 루프 **0회** → **가드는 돈다** → **재생성된 살아 있는 dir**을 보고 통과 → `{}` 원장 → **`Ok`** | **✗ Class B-ABA**(같은 포기의 **두 번째 얼굴** — §D-② 조건절). ⚠ **r15 정정**: *"가드가 아예 안 돈다"* 는 **거짓이 됐다**(집계 관통). **결론은 불변** |

⇒ **가드가 추가하는 syscall은 "오늘이라면 `Err`로 죽었을 패스"에서만 발행된다**(P11 복원의 근거:
모든 `Absent` 주조는 `Err(NotFound)`를 받은 뒤에만 도달하므로 **오늘 `Ok`인 패스는 `vanished == 0`**이다).
⇒ **오늘 완주하는 모든 패스의 syscall 트레이스는 *항목별로도 패스 전체로도* 바이트 동일하다** —
C안이 핀 때문에 포기해야 했던 문장(P11)이 **복원된다.**

---

## C. 가드의 자기무효화 검사 (실측)

**우리 코드가 `.objects`를 재생성하는 지점 — 전수(grep + 실행 확인 · r14 반증이 독립 재확인)**

| # | 지점 | 가드보다 앞서 도나 | 판정 |
|---|---|---|---|
| ① | **`reconcile.rs:217` `mkdir_p_durable(&corrupt_dir)`**(격리 분기) | **예 — 루프 *안*이다** | ⚠ **자기무효화 벡터. 유일하다.** |
| ② | `reconcile.rs:277` `write_atomic(pending)` → `mkdir_p_durable(.objects)` | **아니오 — 가드 뒤** | ✔ **그래서 가드가 그 앞에 있어야 한다** |
| ③ | `objects.rs:30` `write_atomic(blob)` · `objects.rs:72` `mkdir_p_durable(objects_dir)`(put/put_stream) | 패스 밖(**동시 쓰기**) | **Class B-ABA**(인간이 채택한 포기) |
| ④ | `http/state.rs:18` `create_dir_all(objects_dir)` | 부팅 시 1회 | 무관 |
| ⑤ | `fsync_dir` · `rename_durable` · `settle()` · `grave()` · `recover_graves` | **생성 없음**(`File::open`/`rename`뿐 — 소스 확인) | ✔ |

**①의 실증 (프로덕션 호출을 그대로 부른다 · T3)**

```
[T3] 파괴 전 .objects = false
[T3] mkdir_p_durable(.corrupt) 이후 .objects = true      ← 루프 안에서 컨테이너가 **부활한다**(조상까지 만든다)
[T3] 그 뒤 가드 metadata() = Ok                           ← **가드가 무효화된다**
```

### ⚠ 그러나 ①은 **파괴된 세계에서 도달 불가능하다** — 코드로 추적한다

격리 분기의 진입 조건은 **`entry.read()`가 `Seen::Present`** 다(`reconcile.rs:215`). `.objects`가 죽으면
`read()`는 **ENOENT**다(실측) ⇒ `Gone` ⇒ **skip** ⇒ `mkdir_p_durable`에 **도달하지 못한다.**
⇒ ①이 발화하려면 **파괴가 `read()`의 `Ok`와 `mkdir_p_durable` *사이*의 µs 창에 정확히 착지**해야 한다.
그 구간에는 **훅이 하나도 없다**(`:215`→`:217` — 소스 확인) ⇒ **결정적 증인 불가 · 프로덕션 ABA 레이스로만
도달.**

⇒ **판정: 가드는 자기무효화되지 않는다 — 단 하나의 예외를 정직하게 등재한다(§I B′-SELFINVAL).**
⇒ **W10 계열의 무대 규율(구성상 봉인)**: **첫 `pre_entry`에서 파괴**한다 ⇒ 그 뒤 **어떤 `read()`도 성공할
수 없다** ⇒ 격리 분기가 **원리적으로 발화 불가**. (무대에 **비트로트 blob·`.corrupt`·동시 put을 두지
않는다**는 규율은 벨트+멜빵이며 **B-5 diff 항목**이다.)
⇒ **W10의 `.objects` 미부활 단언이 곧 자기무효화 검사 그 자체다**(실측: D안 W10에서 부활 = **false**).

### §C-A. **W-GRAVE-CD** — `grave()` 경로의 결정적 증인 (r15/P-27 · 스크래치에서 **구현·실행 확인**)

**무대(A·B 공유)**: `.objects` = { `<sha>`(내용 **정합** — 비트로트 아님) · `.gc-pending.json` = `{<sha>: t0−2·GRACE}` }.
커밋 포인터 0 · 무덤 0 · `.corrupt` 0 · 동시 put 0 · **spawn 0**(`pre_grave` 안에서 테스트가 **직접 완주까지 await**
⇒ *"spawn ≠ 폴링됨"* 함정 부재). **결정성 100%**(항목 1개 ⇒ readdir 순서 무관). 단언·self-verify = **증인 표**.
⚠⚠ **무대 규율 — 비예약 항목이 *정확히 하나*여야 한다 (load-bearing)**: 파괴 후에도 루프는 돌고 **남은 Blob/Temp
항목은 `Entry::seen`에서 스스로 집계를 올린다** ⇒ 둘 이상이면 **`grave()`가 집계를 안 올려도 가드가 발화**해
**M-FRESH/M-FRESH′가 산다**(실측: blob 3개 변종은 M-FRESH 아래 **GREEN**). `.gc-pending.json`은 `Reserved` ⇒
`pre_entry` **이전에** `continue` ⇒ **FS 접촉 0** ⇒ bump 후보 아님(`reconcile.rs:188` · `layout.rs:189`).
**자기무효화 검사 — 전부 닫혔다(실행 확인)**: 격리 분기 `mkdir_p_durable(.corrupt)` **도달 불가**(비트로트 blob 0) ·
`settle`의 복원 rename **도달 불가**(`SourceGone`이면 `Graved`가 없다 — §C-0) · `write_atomic`의 `mkdir_p_durable`은
**가드 뒤** ⇒ A의 *".objects 부재"* 단언이 그 순서를 핀한다(**M-GUARD-AFTER 킬**) · 동시 put/spawn **무대에 없다**.
**킬 능력의 근거는 코드에 실재한다**: `atomic.rs:51` — `write_atomic`의 **첫 줄이 `mkdir_p_durable(parent)`** 다 ⇒
가드를 건너뛰면 `.objects`가 **부활**하고 `{}` 원장이 발행되며 `Ok`가 난다. **⚠ 이 한 줄이 리팩터로 사라지면
W-GRAVE-CD-A가 조용히 무력화된다 — B-5 항목이다.**

---

## D. `recover_graves` 정책 — 사라진 무덤은 skip. **가드는 넣지 않는다.**

**① 같은 경로 기반 정책을 적용한다**(근본 원인은 syscall이 아니라 루프의 전제다 — 범인 표 ⑦):

* `entry.file_type()?` → `Seen`(`Gone` 팔은 캐시 때문에 발화 안 함) · `entry.remove()?` → `Gone` ⇒ **skip** ·
  `entry.rename_durable_to(&blob, &objects)?` → `SourceGone` ⇒ **skip**
* `blob_intact`의 `matches!(read(&blob), Ok(b) if …)`는 **스냅샷 항목이 아니다** ⇒ **축자 보존**(F-33)
* **9번째 훅 `pre_recover_grave`** · **`recover_graves(&Layout, &Hooks, &Vanished)`**(⚠ **패스 집계 관통** —
  r15/P-27) + `recover_graves_from(...)` 분해 — **전부 유지**

**② 별도 가드를 넣지 않는다 — `read_dir`이 이미 가드다. ⚠ 단, 그 문장은 *조건부*다.**

`recover_graves`는 `PassGuard::begin`(`pins.rs:455`) 안이고, 그 직후 `collect_referenced`(`.objects`를
**스킵**한다 — `layout.rs:291`)를 지나 **`reconcile.rs:174`의 `read_dir(&objects).await?`** 가 온다.

⇒ 무덤 루프에서 컨테이너가 죽고 **재생성되지 않으면** `read_dir`이 **오늘과 같은 `Err`**(`NotFound/2` ·
일반 파일이면 `NotADirectory/20`)로 패스를 죽인다(실측). 파괴된 세계에서 무덤 루프가 하는 일은 **0**이다
(remove·rename 전부 ENOENT → skip · `fsync_dir`은 rename `Ok` 뒤에만 있어 도달 불가).

> ⚠⚠ **조건절을 숨기지 않는다 (r14 반증 · r15 정정)**: **재생성되면** `read_dir`은 **`Ok(빈 디렉터리)`** 를 주고
> 엔트리 루프가 **0회** 돈다. ⚠ **r15/P-27 수리 후에는 무덤 루프의 부재가 *패스 집계*를 올리므로**(A2·A4)
> **가드는 돌고**, **재생성된 살아 있는 dir**을 보고 **통과**한다 ⇒ `try_exists`가 전부 `Ok(false)` ⇒
> **`{}` 원장 발행 + 패스 `Ok`**(오늘은 `Err`). **손실은 0**(운영자가 이미 지웠다)이고
> **Class B-ABA의 두 번째 얼굴**이다(§B-3 행 7 · §I). *(수리 전 문언 "가드가 아예 안 돈다"는 **거짓이 됐다** — 결론은 불변.)*

**③ 무덤 루프 뒤에 별도 가드를 넣으면 *두 번째 플립*이 생긴다 — 그래서 넣지 않는다.**
무덤이 **0개인 clean 트리**(정상)에서 컨테이너가 파괴→재생성되면 오늘은 **아무 것도 만지지 않고 `Ok`로
완주**한다. 거기에 무조건 가드를 넣으면 **`Ok → Err`**. `vanished > 0` 게이트를 무덤 루프에 별도로 다는
것도 `read_dir`이 이미 하는 일의 중복이다.
⇒ **원칙 그대로: 아무 일도 하지 않거나, 새 실패를 날조하는 술어는 넣지 않는다.**

---

## E. `pending.remove`(C안 §D-2) — **폐기한다**

C안의 근거는 **"위조된 `Gone` 뒤 복원된 blob"**(P-20)이었다. D안에는 **위조된 `Gone`이 없다**(경로 확인이
그 순간에는 **정직**하다 — 루프의 **모든 파괴 연산은 `Present` 팔 뒤에만** 있으므로 위조 부재는 **항상
skip = no-op**로 귀결된다). 남은 것은 **"정직한 `Gone` 뒤의 부활"** 창 하나뿐이고, 그것을 **코드로** 따진다:

| 케이스 | `pending.remove` **없이** | `pending.remove` **있으면** | **오늘**(패스 중단 ⇒ 원장 미기록) |
|---|---|---|---|
| **소멸 후 그대로** | 루프 끝 `try_exists(blob_path)`(`:272`)가 **어차피 떨군다**(실측: 파괴된 세계에서 `try_exists` = `Ok(false)` ⇒ `cleaned = {}`) | 동일 | 원장 유지 |
| **소멸 → 부활** | `{X: t_old}` **재발행** → 다음 패스 0-grace 회수 **시도** | `{X}` 탈락 → 다음 패스 **full-grace** | **디스크의 옛 원장이 `{X: t_old}`를 그대로 들고 있다 ⇒ 다음 패스가 똑같이 0-grace 회수 시도** |

⇒ **① 정직한 소멸에서는 완전한 동어반복**(`try_exists` 정리와 중복) · **② 부활 창에서는 *오늘의 원장
의미론에서 이탈*한다**(오늘은 `{X:t_old}`가 살아남는다) ⇒ **증인 없는 추가 행동 델타**(C안 스스로
**M-PENDING = Class B, 증인 없음**이라 적었다).
⇒ **③ 그것이 막으려던 손실은 이미 다른 곳이 막는다**: 부활의 프로덕션 경로는 `objects.rs:44`(`put`)·
`:110`(`put_stream`) **둘뿐이고 둘 다 `pin.commit_pointer`를 지난다** ⇒ **`landed`가 반드시 선다** ⇒
`settle()`이 `Settlement::Landed`(`pins.rs:250`) → `Settled::Restored`(`:586`) → **무덤을 정본으로 되돌린다**
(`:610`). 남는 것은 **아웃오브밴드 복원**뿐이며 그것은 **오늘도 열려 있는 기존 구멍**(C-1 · F-41)이고
이 픽스와 인과가 없다.

**실측이 이 판정을 뒷받침한다**(D안 구현본 · 파괴 → 빈 재생성 → 다음 패스에서 아웃오브밴드 복원):
`pass1 = Ok · 원장 {}` → `pass2 = blob 3/3 생존`(fresh tombstone = **full grace**). red.sha는 pass1이 `Err`다.
⇒ **D안이 오히려 더 보수적이다.**

⇒ **판정: `pending.remove`를 `Seen::Gone`/`SourceGone` 팔에 넣지 않는다.** `pending`의 삽입·만료·정리
로직은 **한 글자도 바뀌지 않는다** ⇒ **P6이 "무변경"이 된다.** 격리 성공 팔(`:220`)·재참조 팔(`:227`)·
`Reaped` 팔(`:241`)의 기존 `pending.remove`는 **축자 보존**.

---

## 구현 — 타입 경계

> ⚠⚠ **이 절의 코드는 의사코드가 아니다.** 아래 §①~§⑥의 모든 스니펫은 **스크래치 프로토타입에서 실제로
> 컴파일되고 스위트를 통과한 소스를 그대로 옮긴 것이다**(2026-07-14 · D안 **전체**를 red.sha 복제본에
> 구현 — `cargo build` **경고 0** · `cargo test --lib --bins --tests` **전부 GREEN**(lib **123 passed**) ·
> 회귀 증인 2개 **RED → GREEN** · 봉인 `E0624`×2 / `E0423` / `E0425`를 **컴파일러로 실증**).
> **가시성·lifetime·재수출·시그니처는 컴파일되는 그대로다** — 이 절을 그대로 옮겨 적으면 컴파일된다.
> **프로토타입 코드는 저장소에 넣지 않았다**(B1 배리어 — 구조 게이트 전 fix 코드 금지). **옮긴 것은 사실뿐이다.**
> 프로토타입이 **계획의 의사코드를 그대로 쓰면 죽는 곳**으로 발견한 5곳은 **§E-COMPILE에 전수 등재**했다.

### ① `src/store/reconcile/absence.rs` — 부재의 증거는 위조할 수 없다 (핀은 없다)

**`Absent` 발행 지점 전수 × 집계 연결 — 다섯이고, 전부 `entry_is_absent{,_blocking}`를 경유하며 전부 *하나뿐인
패스 집계*를 올린다**(r15/P-27 수리 — 구 D안에는 **무덤 루프의 지역 집계**가 따로 있어 발행이 **버려졌다**):

| # | 발행 지점 | 소스 경로 | 올리는 집계 |
|---|---|---|---|
| **A1** | `Entry::seen` — **엔트리 루프**(`metadata`·`read`·`remove`) | `self.path` (= `de.path()`) | **패스 집계** ✅ |
| **A2** | `Entry::seen` — **무덤 루프**(`remove`) | `self.path` | **패스 집계** ✅ *(수리 전: 버려지는 집계)* |
| **A3** | `Entry::rename_into` — **격리 rename** ← `rename_source_checked` ← `rename_checked_blocking` | `self.path` | **패스 집계** ✅ |
| **A4** | `Entry::rename_durable_to` — **무덤 복구 rename** ← `rename_durable_source_checked` | `self.path` | **패스 집계** ✅ *(수리 전: 버려지는 집계)* |
| **A5** | **`pins::grave()`** — blob→무덤 rename ← `rename_durable_source_checked` | `layout.blob_path(sha)` ≡ `objects.join(sha)` | **패스 집계** ✅ ← **P-27의 수리** |

**발행하지 않는 곳(무가공 `?` — 여섯 번째 산지는 없다)**: `Graved::settle`의 복원 rename·`remove_file(grave)`·
`fsync_dir` · `recover_graves`의 `blob_intact` read(F-33) · `collect_referenced`의 삼킴(B-REFS) · 루프-후
`try_exists` · `write_atomic`/`mkdir_p_durable`/`fsync_dir` · `read_dir`/`next_entry` — **전부 `Err`로 시끄럽다.**

> ⚠ **A4의 소스 경로는 lossy에서 나온다 — 안전하지만 그 안전성은 `layout.rs`에 *암묵 의존*한다.**
> `grave()`는 `layout.blob_path(sha)`로 소스를 짓고 `sha`는 호출부의 **lossy `name`** 이다. 안전한 이유는
> **`classify_objects_entry`가 `Blob`으로 분류하는 이름이 `is_sha_name` = 64자 ASCII hex**이기 때문이다
> (`layout.rs:172-173,198`) ⇒ 그 이름들에서 **lossy == raw**다. **`layout.rs`는 `scope[]` 밖이다** ⇒
> 드리프트하면 **이 안전성이 조용히 깨진다** ⇒ **B-5 릴리스-게이트 확인 항목**(B-3).

**전문 — 프로토타입에서 컴파일되는 실제 소스 그대로**(138줄):

```rust
//! **부재의 증거 — 경로 기반 · fd 0.**
//!
//! `NotFound`가 났을 때 *"그 항목이 정말로 사라졌는가"*를 판정하는 **유일한 정의**가 여기 있다.
//! 판정은 `symlink_metadata(<그 항목의 경로>)` **1회**이고(**no-follow** — P-1 봉인: 댕글링 심링크는
//! 항목이 **있으므로** `Ok` ⇒ skip 금지 ⇒ 오늘의 `Err`를 바이트 보존), 그 syscall이 `NotFound`를 낼
//! 때에만 `Absent`가 주조되고 **같은 행위로** 패스 집계가 오른다.
//!
//! ⚠⚠ **위치가 봉인이다.** 이 모듈은 `reconcile`의 **자식**이다 ⇒ `pub(super)` =
//! `pub(in crate::store::reconcile)` = **reconcile 서브트리 전용**(pins·atomic 제외).

use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// **"소스 항목이 부재함"의 증거.** 필드 private ⇒ **이 모듈 밖에서 생성 불가**(`E0423`).
pub(crate) struct Absent(());

/// **소멸 계수기.** ⚠ **derive 0개**(복제본이 곧 대체 집계다).
/// `Arc`가 남는 **유일한** 이유: private `share()`가 `spawn_blocking`의 `'static` 클로저로 **같은** 집계를 나른다.
pub(crate) struct Vanished(Arc<AtomicUsize>);

impl Vanished {
    /// ⚠ **크레이트 전체 호출부는 `run_once_at` 하나뿐이다**(reconcile 서브트리 전용).
    pub(super) fn new() -> Self {
        Vanished(Arc::new(AtomicUsize::new(0)))
    }

    /// 루프-후 컨테이너 가드만 읽는다(`pins`에서 부르면 `E0624`).
    pub(super) fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }

    /// **모듈 private.** 호출부는 `entry_is_absent{,_blocking}` 둘뿐이다.
    fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    /// **모듈 private.** Arc 공유 = **같은 집계**(클론이 아니다 — 대체 집계를 만들 수 없다).
    fn share(&self) -> Vanished {
        Vanished(Arc::clone(&self.0))
    }

    /// ⚠ **테스트 다리(B-TESTBRIDGE).** `pins::tests`는 reconcile 서브트리 **밖**이라 `&Vanished`를
    /// 만들 방법이 없다 — `begin`/`grave` 호출부 9개가 그것을 요구한다. `#[cfg(test)]` ⇒ 릴리스
    /// 빌드에 **존재하지 않는다**.
    #[cfg(test)]
    pub(crate) fn new_for_test() -> Self {
        Vanished(Arc::new(AtomicUsize::new(0)))
    }
}

/// rename의 **소스 부재**만 분리(P-2). `SourceGone`은 `Absent`를 **요구** ⇒ 위조 불가.
#[must_use]
pub(crate) enum Renamed {
    Done,
    SourceGone(Absent),
}

/// **부재 판정의 유일한 정의(async 채널).**
pub(super) async fn entry_is_absent(tally: &Vanished, path: &Path) -> Option<Absent> {
    match tokio::fs::symlink_metadata(path).await {
        // ⚠ **no-follow** — 댕글링 심링크는 `Ok`다(항목이 **있다**) ⇒ P-1 봉인.
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tally.bump();
            Some(Absent(()))
        }
        // Ok(_) = 항목이 있다 · 그 외 Err = 확인 불가 ⇒ 보수적(원본 에러 전파).
        _ => None,
    }
}

/// **부재 판정의 유일한 정의(blocking 채널).** `rename_checked_blocking` 전용.
fn entry_is_absent_blocking(tally: &Vanished, path: &Path) -> Option<Absent> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tally.bump();
            Some(Absent(()))
        }
        _ => None,
    }
}

/// ⚠⚠ **`SourceGone`은 `std::fs::rename`의 `Err` 팔에서만 태어난다** ⇒ rename `Ok` 이후의 fsync
/// 실패는 **무가공 `io::Error`**다. `atomic::rename_durable`(rename+fsync **융합**)에 부재 확인을
/// 붙이면 rename 성공 후의 fsync ENOENT가 `SourceGone`으로 **위조**된다(M6 부활) ⇒ **확인은 여기에만.**
fn rename_checked_blocking(from: &Path, to: &Path, tally: &Vanished) -> std::io::Result<Renamed> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(Renamed::Done),
        // 목적지 부재도 `NotFound`다 → **소스를 확인해서** 걸러낸다(W5b · W9b).
        Err(e) if e.kind() == ErrorKind::NotFound => match entry_is_absent_blocking(tally, from) {
            Some(a) => Ok(Renamed::SourceGone(a)),
            None => Err(e), // 목적지발 NotFound · 댕글링 소스 → **원본 그대로**
        },
        Err(e) => Err(e), // EACCES · EXDEV · ENOTDIR · EIO … 무가공(B7)
    }
}

/// 격리 rename(부모 fsync는 **호출부의 raw `?`**).
pub(super) async fn rename_source_checked(
    from: &Path,
    to: &Path,
    tally: &Vanished,
) -> std::io::Result<Renamed> {
    let (f, t, share) = (from.to_owned(), to.to_owned(), tally.share());
    tokio::task::spawn_blocking(move || rename_checked_blocking(&f, &t, &share))
        .await
        .expect("join")
}

/// 무덤 rename — rename + parent fsync를 **한 무취소 클로저**에 유지한다(M6 봉인).
/// rename이 `Ok`를 낸 **이후의** fsync 실패는 **무가공**으로 전파된다(P-2).
/// ⚠ **`pub(crate)`여야 한다** — `pub(super)`로 좁히면 `reconcile.rs`의 `pub(crate) use` 재수출이
/// 가시성을 **넓힐 수 없어**(`E0364`) 컴파일되지 않는다(§E-COMPILE 2).
pub(crate) async fn rename_durable_source_checked(
    from: &Path,
    to: &Path,
    fsync_parent: &Path,
    tally: &Vanished,
) -> std::io::Result<Renamed> {
    let (f, t, p, share) = (
        from.to_owned(),
        to.to_owned(),
        fsync_parent.to_owned(),
        tally.share(),
    );
    tokio::task::spawn_blocking(move || match rename_checked_blocking(&f, &t, &share)? {
        Renamed::Done => {
            crate::store::atomic::fsync_dir_blocking(&p)?; // ← rename Ok 이후 ⇒ 실패는 **무가공**
            Ok(Renamed::Done)
        }
        gone => Ok(gone),
    })
    .await
    .expect("join")
}
```

`rename_durable`(`atomic.rs:73-87`)은 **그대로 남는다** — `Graved::settle`의 복원 rename은 `NotFound`가
**진짜 에러**이므로 fail-CLOSED를 유지해야 한다.

> **격리 rename도 같은 함수를 경유한다** ⇒ `NotFound`를 분류하는 코드는 **`Entry::seen` +
> `absence::rename_checked_blocking` 두 곳뿐**이고, 목적지-에러 증인(**W5b**)이 **두 경로 모두**를 핀한다.
> `Entry::rename_into`의 인라인 분류(= 무증인 우회 뮤턴트 **M8**)는 **`Absent`를 만들 수 없어 컴파일되지
> 않는다**(컴파일러 확인: **`E0423`** — 필드가 private인 튜플 구조체는 초기화할 수 없다).

### ② `src/store/reconcile/entry.rs` — 자식 모듈 경계 + **read_dir까지** 이 안으로 (P-3)

> ⚠⚠ **P-31 (r18) — 가시성을 여기서 봉인한다.** 이전 개정판은 **FS 메서드 6개를 private으로** 적어 두었다.
> `reconcile.rs`는 `entry`의 **부모**이지 자손이 아니므로 그대로 쓰면 호출부가 **`E0624`(private method)** 로
> 죽는다. **`Entry`의 FS 메서드 6개 + `snapshot`/`name`/`class` = `pub(super)`**(= reconcile 서브트리) ·
> **`seen()`만 모듈 private**(= `NotFound` 흡수의 유일한 지점은 여전히 봉인된다) · **`impl<'v> Entry<'v>`**
> (`snapshot`은 **자체 `<'v>`를 다시 선언하지 않는다** — impl의 `'v`를 쓴다). **아래는 프로토타입에서
> 컴파일되는 실제 소스다**(125줄).

```rust
//! 스냅샷 항목 — 루프가 `.objects`를 만지는 **유일한 통로**. `read_dir`도 여기 있다
//! ⇒ `tokio::fs::DirEntry`가 `reconcile.rs`에 **한 번도 등장하지 않는다**(P-3).

use super::absence::{
    entry_is_absent, rename_durable_source_checked, rename_source_checked, Absent, Renamed,
    Vanished,
};
use crate::layout::{classify_objects_entry, ObjectsEntry};
use std::fs::{FileType, Metadata};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// `Gone`은 `T`를 들지 않는다 ⇒ **`T`를 주조할 필요가 없다** ⇒ 호출부가 오늘의 `ft.is_dir()` ·
/// `m.modified().unwrap_or(now)`를 **축자 그대로** 쓴다.
/// ⚠ 페이로드는 **읽히지 않으므로** `#[allow(dead_code)]`가 **필요하다**(없으면 `dead_code` 경고 —
/// 경고 0 정책 위반. §E-COMPILE 3).
#[must_use]
pub(super) enum Seen<T> {
    Present(T),
    Gone(#[allow(dead_code)] Absent),
}

pub(super) struct Entry<'v> {
    /// ★ 오늘의 핸들 **그대로**. `path()`/`file_type()`/`metadata()`의 주인.
    de: tokio::fs::DirEntry,
    /// `de.path()` (스냅샷 시점 1회 · syscall 0). **접근자 없음**.
    path: PathBuf,
    /// lossy. 분류·로깅·원장 키·**목적지 이름** 전용(= 오늘의 용도).
    name: String,
    class: ObjectsEntry,
    /// ⚠ **빌린다 — 소유/클론하지 않는다**(클론이 곧 대체 집계다).
    vanished: &'v Vanished,
}
// ⚠⚠ **`dir: PathBuf` 필드는 없다** — 경로는 **오직 `de.path()`에서만** 나온다(M46 표현 불가).

impl<'v> Entry<'v> {
    /// **오늘과 글자 그대로 동일한 `read_dir`/`next_entry`.**
    pub(super) async fn snapshot(dir: &Path, vanished: &'v Vanished) -> std::io::Result<Vec<Self>> {
        let mut out = Vec::new();
        let mut rd = tokio::fs::read_dir(dir).await?;
        while let Some(de) = rd.next_entry().await? {
            let path = de.path();
            let name = de.file_name();
            let name = name.to_string_lossy().to_string();
            let class = classify_objects_entry(&name); // 이름-전용 ⇒ syscall 0 ⇒ O1 보존
            out.push(Entry {
                de,
                path,
                name,
                class,
                vanished,
            });
        }
        Ok(out)
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn class(&self) -> ObjectsEntry {
        self.class
    }

    // ── FS 접촉은 전부 `self.seen(...)`을 지난다. 위임 대상은 **오늘의 호출 그 자체**다. ──

    /// tokio는 readdir 청크를 채우는 시점에 `d_type`을 캐시한다 ⇒ 소멸한 항목에도 `Ok`가 난다
    /// ⇒ **`Gone` 팔은 사실상 발화하지 않는다**(정책은 균일하게 유지한다).
    pub(super) async fn file_type(&self) -> std::io::Result<Seen<FileType>> {
        let r = self.de.file_type().await;
        self.seen(r).await
    }

    /// lstat 의미론(댕글링 심링크 → `Ok`) — W4.
    pub(super) async fn metadata(&self) -> std::io::Result<Seen<Metadata>> {
        let r = self.de.metadata().await;
        self.seen(r).await
    }

    /// `read`는 open이므로 **심링크를 추종한다** ⇒ 확인이 **load-bearing**한 유일한 지점(P-1).
    pub(super) async fn read(&self) -> std::io::Result<Seen<Vec<u8>>> {
        let r = tokio::fs::read(&self.path).await;
        self.seen(r).await
    }

    pub(super) async fn remove(&self) -> std::io::Result<Seen<()>> {
        let r = tokio::fs::remove_file(&self.path).await;
        self.seen(r).await
    }

    /// 격리 rename — 소스 = `self.path`(원시) · 목적지 = `dir.join(&self.name)`(lossy).
    /// **오늘과 같은 짝**이다.
    pub(super) async fn rename_into(&self, to_dir: &Path) -> std::io::Result<Seen<()>> {
        match rename_source_checked(&self.path, &to_dir.join(&self.name), self.vanished).await? {
            Renamed::Done => Ok(Seen::Present(())),
            Renamed::SourceGone(a) => Ok(Seen::Gone(a)),
        }
    }

    /// 무덤 복구 rename(rename + parent fsync 융합 — rename `Ok` 이후의 fsync 실패는 **무가공**).
    pub(super) async fn rename_durable_to(
        &self,
        to: &Path,
        fsync_parent: &Path,
    ) -> std::io::Result<Seen<()>> {
        match rename_durable_source_checked(&self.path, to, fsync_parent, self.vanished).await? {
            Renamed::Done => Ok(Seen::Present(())),
            Renamed::SourceGone(a) => Ok(Seen::Gone(a)),
        }
    }

    /// **`NotFound` 흡수의 유일한 지점.** 이름도 경로도 `self`뿐 ⇒ **목적지를 확인하는 판본은 없다.**
    /// ⚠ **모듈 private** — 이 하나만 private이다(FS 메서드 6개는 `pub(super)`, P-31).
    async fn seen<T>(&self, r: std::io::Result<T>) -> std::io::Result<Seen<T>> {
        match r {
            Ok(v) => Ok(Seen::Present(v)),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                match entry_is_absent(self.vanished, &self.path).await {
                    Some(a) => Ok(Seen::Gone(a)), // ← 계수는 여기서 이미 일어났다(async 채널)
                    None => Err(e),               // ← 원본 그대로(댕글링 심링크 · 확인 불가)
                }
            }
            Err(e) => Err(e), // ← B7. **유일한 갈림길.**
        }
    }
}
```

**`Send`/lifetime — 컴파일로 확인했다**(r12 · **r15 재확인 `p27-s`: BUILD OK**). `Entry<'v>{de: tokio::fs::DirEntry, …}`를
`Vec`에 담아 **await를 가로질러** 보유하고 `tokio::spawn`에 넘겨 **통과**. **`unsafe` 0.**
`&Vanished: Send`(`AtomicUsize: Sync`) ⇒ 퓨처가 `Send`다 — **`Arc` 클론을 `Entry`마다 들고 다니던 서술은 소멸했다.**

> ⚠⚠ **`io::Result<Seen<T>>`의 `Result`가 곧 중단 채널이다 — 타입은 흡수를 *강제하지 않는다*.**
> `Absent`가 막는 것은 **`Gone`의 위조**이지 **`Gone`의 주조**가 아니다 ⇒
> `Err(e) if e.kind() == NotFound && cfg!(test) => …` 같은 한 줄 뮤턴트(**M19**)는 **컴파일된다**
> ⇒ **W13이 이것을 행동으로 잡는다**(`tests/`는 `cfg(test)` **없이** lib를 링크한다 — §W13-0).

**확인 syscall이 왜 3/4 지점에서 동어반복인가**: `metadata`(lstat) · `remove`(unlink) · `rename`(소스)은
**심링크를 추종하지 않는다** ⇒ 그들의 `NotFound`는 **이미** 항목 부재의 증거다. **`read()`(open = 추종)
에서만 load-bearing**하다. 정책은 균일하되 **행동이 갈리는 지점은 정확히 두 곳**(`read()` ·
`rename_*`의 목적지)이다.

### ③ `pins.rs` — `grave()`의 타입 경계 (P-2)

> ⚠⚠ **★B-1 `/code-review` B-J7 (수용) — 타입 이름을 `Grave` → `GraveOutcome`으로 고쳤다.**
> `CONTEXT.md`의 **무덤(Grave)** 은 *"GC가 블롭을 지우기 전에 옮겨 두는 **이름**"* 인데, 이 타입은 그 이름이
> 아니라 **`grave()`가 옮기기를 시도한 결과**이고 `SourceGone`은 *"무덤이 **태어나지 않았다**"* 를 뜻한다
> ⇒ 용어집대로 읽으면 **자기모순**이었다. 게다가 `Graved`와 **한 글자 차이**였다.
> **P4 봉인은 이름과 무관하다**(`SourceGone`이 `Absent`를 **요구**하는 것이 봉인이다 — `E0423`)
> ⇒ **킬 능력 손실 0**(스위트 · M-NOBUMP-BLOCKING/M-FOLLOW/M-GUARD-ALWAYS 재확인 완료).
> 이 절 아래의 소스와 §A 행 16 · §E-COMPILE 3 · 증인 표는 **새 이름으로 갱신**했다.
> (본 절의 나머지 서술과 §Review Decision Log의 과거 라운드 기록은 **당시의 판단을 그대로 보존**한다 —
> 타입 이름만 현행화했다.)

```rust
// ⚠ **`pins`는 `reconcile::absence`를 경로로 지나갈 수 없다**(private 모듈) ⇒ **재수출을 통해서만** 닿는다(§A-0)
use super::reconcile::{rename_durable_source_checked, Absent, Renamed, Vanished};

/// `grave()`의 **결과**(무덤 그 자체가 아니다). `SourceGone`은 `Absent`를 **요구**한다 ⇒ `pins`에서
/// **위조 불가**(`E0423`). 봉인은 **이름이 아니라 타입**에 걸려 있다.
#[must_use = "GraveOutcome을 흘리면 무덤이 남는다"]
pub(crate) enum GraveOutcome<'p> {
    Moved(Graved<'p>),
    SourceGone(#[allow(dead_code)] Absent),   // ← 페이로드 미판독 ⇒ allow 필수(§E-COMPILE 3)
}

/// ⚠ **`Option<&Container>` 인자는 사라졌다** — 정체성을 보지 않기 때문이다(M30/M30′/M32/M33 소멸).
/// ⚠ 새 인자 `&Vanished`는 **쓰기 전용 계수기**다 — `pins`는 그것을 **짓지도(`E0624`) 읽지도(`E0624`)
///   올리지도(`bump`는 absence private) 못하고 오직 *빌려서 전달*만 한다**(§A-0의 실컴파일).
///   ⚠⚠ **그러나 "bump 누락은 표현 불가"는 거짓이다**(r15/P-27이 철회했다) — `bump()`의 호출부는
///   **두 채널**(`entry_is_absent` async · `entry_is_absent_blocking`)이고 private `bump()`는 **위조**를
///   막을 뿐 **한 채널에서의 누락**을 못 막는다 ⇒ **행동 킬러 2행**: **M-NOBUMP-ASYNC**(킬러 = **W10 ∧
///   W10-TEMP**) · **M-NOBUMP-BLOCKING**(킬러 = **W-GRAVE-CD-A** — `grave()`가 타는 채널이 바로 이쪽이다).
///   **둘은 서로를 못 덮는다.**
/// ⚠ 소스 확인은 **`rename_durable_source_checked` → `rename_checked_blocking`의 `std::fs::rename`
///   `Err` 팔 전용**이다 — **`rename_durable`(융합)에 붙이면 M6가 부활한다**(§A 행 16).
pub(crate) async fn grave<'p>(
    &'p self,
    sha: &str,
    vanished: &Vanished,
) -> std::io::Result<GraveOutcome<'p>> {
    // ← 목적지 에러 · **rename 이후의 fsync 실패**는 여기서 **무가공 전파**된다(P-2)
    match rename_durable_source_checked(
        &self.layout.blob_path(sha),
        &self.layout.grave_path(sha),
        &self.layout.objects_dir(),
        vanished,
    )
    .await?
    {
        // 회수할 정본이 스냅샷 이후 사라졌다 → **무덤은 태어나지 않는다**.
        Renamed::SourceGone(a) => Ok(GraveOutcome::SourceGone(a)),
        Renamed::Done => {
            // 무덤 이름이 **자리잡은 뒤에** 코호트를 뜬다(P6).
            let cohort = self.pins.cohort_at_grave(sha);
            self.pins.hooks.post_grave(sha).await;
            Ok(GraveOutcome::Moved(Graved {
                pass: self,
                sha: sha.into(),
                cohort,
            }))
        }
    }
}
```

**`PassGuard::begin` — 프로토타입의 실제 본문**(§⑥의 diff가 적용된 결과):

```rust
pub(crate) async fn begin(
    store: &Store,
    settle_timeout: Duration,
    vanished: &Vanished,
) -> std::io::Result<Self> {
    let _pass = store.pins().pass_lock.clone().lock_owned().await;
    let mut me = Self {
        pins: store.pins().clone(),
        _pass,
        layout: store.layout().clone(),
        refs: HashSet::new(),
        recovered: 0,
        settle_timeout,
    };
    me.pins.enter_pass(); // pass_live = true; landed.clear()
    // ↓ 이 아래 모든 `?`는 me(Drop 보유)를 통과한다 → pass_live/landed 누수 불가
    let recovered =
        super::reconcile::recover_graves(&me.layout, me.pins.hooks(), vanished).await?; // collect **이전**
    me.recovered = recovered;
    let refs = super::reconcile::collect_referenced(&me.layout, me.pins.hooks()).await?;
    me.refs = refs;
    Ok(me)
}
```

`Graved` · `Settled` · `settle()`은 **한 글자도 바뀌지 않는다.** 봉인 체크리스트 ③은
*"`grave()`의 `Renamed::Done` 팔 밖에 생성자 없음"*으로 **더 좁아진다**.
⚠ **`pins::tests`의 호출부 9개**(`begin` 7 · `grave` 2)를 함께 고친다 — **단조 강화**(§Scope · r15).

### ④ 호출부 (`reconcile.rs`) — syscall 순서 불변

**재수출 줄 (`reconcile.rs` 상단) — 실제 소스:**

```rust
/// **부재의 증거 · 소멸 계수기** — `pins`는 이 모듈을 **경로로 지나갈 수 없다**(private 모듈).
mod absence;
/// 스냅샷 항목 — `.objects`를 만지는 **유일한 통로**(`read_dir` 포함).
mod entry;

// ⚠⚠ **자유함수는 타입 재수출을 타고 오지 않는다** — `rename_durable_source_checked`를 빼면
// `pins::grave`가 `E0425`로 죽는다(라운드 17의 컴파일 증거). 그리고 **재수출은 봉인을 넓히지 않는다**:
// `pins`에서 `Vanished::new()`/`.get()`은 `E0624`, `Absent(())`는 `E0423`이다.
pub(crate) use absence::{rename_durable_source_checked, Absent, Renamed, Vanished};
use entry::{Entry, Seen};
```

**`run_once_at` 엔트리 루프 + 가드 — 프로토타입에서 컴파일되는 실제 소스:**

```rust
    // ★ **크레이트 전체에서 유일한 `Vanished::new()` 호출부.** 이 하나의 집계가 무덤 루프
    //   (`recover_graves`) · 엔트리 루프(`Entry`) · `grave()`를 **관통**한다 — 전부 `&`로 빌리기만 한다.
    let vanished = Vanished::new();
    let pass = PassGuard::begin(store, settle_timeout, &vanished).await?;
    …
    // .objects 직속 항목 스냅샷(순회 중 변경 회피) — `read_dir`/`next_entry`는 `Entry::snapshot` 안이다.
    for e in Entry::snapshot(&objects, &vanished).await? {
        let class = e.class();
        // O1: 예약 이름(.gc-pending.json/.corrupt)은 file_type 조회 **전에** continue.
        if matches!(class, ObjectsEntry::Reserved) {
            continue;
        }
        let name = e.name().to_owned();
        // ⚠⚠ **red.sha에 이미 있는 seam이다 — 지우지 마라**(§회귀 증인). prod = `None` ⇒ no-op.
        pass.pins().hooks().pre_entry(&name).await;
        // O2: 디렉터리 스킵은 temp/blob 처리보다 앞.
        let Seen::Present(ft) = e.file_type().await? else {
            continue; // 스냅샷 이후 소멸(d_type 캐시 때문에 사실상 도달 불가)
        };
        if ft.is_dir() {
            continue;
        }
        match class {
            // 3) temp 잔재: mtime이 grace보다 오래된 것만 삭제(활성 스트리밍 보존)
            ObjectsEntry::Temp => {
                let Seen::Present(m) = e.metadata().await? else {
                    continue; // 스냅샷 이후 소멸 — 우리가 지울 것이 없다
                };
                let mtime = m.modified().unwrap_or(now); // ← **축자 보존**
                let age = now.duration_since(mtime).unwrap_or_default();
                if age.as_secs() > grace_secs {
                    // ⚠ **let-else로 쓸 수 없다** — `Gone`에서 `temps_deleted`를 올리면 안 된다(§E-COMPILE 5).
                    match e.remove().await? {
                        // 증가는 **이 한 곳뿐**이다.
                        Seen::Present(()) => stats.temps_deleted += 1,
                        Seen::Gone(_) => continue, // 우리가 지운 게 아니다
                    }
                }
            }
            ObjectsEntry::Blob => {
                // 4) 무결성: 내용 sha == 파일명 검증, 불일치 → 격리
                // ⚠ `pending.remove`를 여기에 추가하지 않는다(§E — 오늘의 원장 의미론 보존).
                let Seen::Present(content) = e.read().await? else {
                    continue; // 스냅샷 이후 소멸 — 검증할 정본이 없다
                };
                if hex::encode(Sha256::digest(&content)) != name {
                    atomic::mkdir_p_durable(&corrupt_dir).await?; // raw `?` (⚠ §C ①)
                    let Seen::Present(()) = e.rename_into(&corrupt_dir).await? else {
                        continue; // 격리할 정본이 사라졌다
                    };
                    atomic::fsync_dir(&objects).await?; // raw `?`
                    pending.remove(&name);
                    stats.quarantined += 1;
                    tracing::warn!(sha = %name, "quarantined corrupt blob (bit rot)");
                    continue;
                }
                // 2) 2단계 tombstone GC: 미참조 지속시간 기준 — 판정식은 한 줄도 안 바뀐다
                if refs.contains(&name) {
                    pending.remove(&name); // 다시 참조됨
                } else {
                    match pending.get(&name) {
                        Some(&first) if now_secs.saturating_sub(first) > grace_secs => {
                            pass.pins().hooks().pre_grave(&name).await;
                            // ⚠ **`match` 안에서 `g.settle()`을 이어 쓸 수 없다** — arm의 `?`/`continue`로
                            //    타입이 갈린다 ⇒ **`let g = match … ;`로 분리한다**(§E-COMPILE 4).
                            let g = match pass.grave(&name, &vanished).await? {
                                // 회수할 정본이 스냅샷 이후 사라졌다 — 무덤은 태어나지 않았다.
                                GraveOutcome::SourceGone(_) => continue,
                                GraveOutcome::Moved(g) => g,
                            };
                            match g.settle().await? {
                                Settled::Reaped => {
                                    pending.remove(&name);
                                    stats.gc_deleted += 1;
                                }
                                Settled::Restored => {
                                    tracing::info!(sha = %name, "GC restored: landed commit");
                                }
                                Settled::Deferred => {}
                            }
                        }
                        Some(_) => {} // 아직 grace 내 — 보존
                        None => {
                            pending.insert(name.clone(), now_secs); // 최초 관측
                        }
                    }
                }
            }
            ObjectsEntry::Grave => {}
            ObjectsEntry::Reserved | ObjectsEntry::Other => {}
        }
    }

    // ── 루프-후 컨테이너 가드 (§B-1) ────────────────────────────────────────────────────
    // ⚠ **반드시 `write_atomic` 이전**: `write_atomic`의 첫 줄 `mkdir_p_durable(parent)`가 `.objects`를
    //    **되살린다** ⇒ 뒤로 옮기면 가드는 **영영 참**이 된다(M-GUARD-AFTER).
    // ⚠ **`vanished.get() > 0`으로 게이트한다**: 게이트가 없으면 **꼬리 파괴**(소멸 0 · 오늘 조용한
    //    `Ok`)가 `Err`로 뒤집혀 **두 번째 관측 플립**이 된다(M-GUARD-ALWAYS).
    // ⚠ **`metadata`(follow)** — `symlink_metadata`면 `.objects`가 심링크→dir인 정상 배포를 죽인다.
    if vanished.get() > 0 {
        match tokio::fs::metadata(&objects).await {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(std::io::Error::from(ErrorKind::NotADirectory)), // ⚠ 합성 — §I B″
            // **무가공**. `.objects` 부재 = ENOENT/2 = 오늘과 같은 kind·errno(원장 미발행).
            Err(e) => return Err(e),
        }
    }

    // 존재하지 않는 blob의 pending 엔트리 정리 — **한 글자도 안 바뀐다**
    let mut cleaned = HashMap::new();
    for (sha, t) in pending.into_iter() { … }
    stats.gc_pending = cleaned.len();
    atomic::write_atomic(&pending_path, &serde_json::to_vec(&cleaned).unwrap()).await?;
```

⚠ **`use std::io::ErrorKind;`를 `reconcile.rs`에 추가**하고 **`classify_objects_entry` import는 제거**한다
(그 분류는 `entry.rs`로 이사했다). 프로토타입에서 확인된 정확한 import 델타다.

### ⑤ `recover_graves` — 스냅샷 이음매를 함수로 열고, 루프 안에 훅을 꽂는다 (P-4 · **P-5**)

**분해**(순수 extract-function — syscall 순서·횟수·행동 전부 동일 · `pub(super)`):

```rust
pub(super) async fn recover_graves<'v>(
    layout: &Layout,
    hooks: &Hooks,
    vanished: &'v Vanished,
) -> std::io::Result<usize> {
    // ⚠ **지역 집계를 만들지 않는다**(r15/P-27 수리 — 그러면 집계가 둘이 되고 무덤 루프의 발행이 버려진다)
    let entries = Entry::snapshot(&layout.objects_dir(), vanished).await?; // ← `read_dir`의 `?` = 오늘의 가드
    recover_graves_from(layout, hooks, entries).await // ← 스냅샷을 뜨지 않는다 ⇒ 증인이 창을 연다
}
```

⚠ **여기서는 `<'v>`가 필요하다**(자유함수이므로 impl의 lifetime을 물려받지 않는다) — **`Entry::snapshot`과
반대다**(§E-COMPILE 1). 프로토타입에서 **그대로 컴파일된다.**

**행동 델타 0 · 두 번째 플립 없음**: 무덤 루프의 bump 후보는 `remove_file(grave)`·`rename_durable(grave→blob)`의
ENOENT뿐이고(`file_type()`은 d_type 캐시로 `Gone` 도달 불가) **둘 다 오늘 `?`로 패스를 죽이는 지점**이다 ⇒ **오늘
`Ok`인 패스는 bump 0 ⇒ 가드 syscall 0**(P11). **가드는 무덤 루프에 넣지 않는다**(§D-③) — 관통한 것은 **집계**다.

**루프 본문의 변경은 넷뿐이다**: `entry.file_type()?` → `Seen`(`Gone` 팔은 캐시 때문에 발화 안 함) ·
`entry.remove()?` → `Gone` ⇒ **skip** · `entry.rename_durable_to(&blob, &objects)?` → `SourceGone` ⇒ **skip** ·
**★ 9번째 훅 `hooks.pre_recover_grave(&sha).await`**. `blob_intact`의 `matches!(read(&blob), Ok(b) if …)`는
**스냅샷 항목이 아니다** ⇒ **축자 보존**(F-33). `fsync_dir`는 remove `Present` 뒤 **raw `?`**.

**훅의 위치가 load-bearing이다.** `grave_sha` 필터와 `file_type` 검사 **뒤**, `blob_intact` 판정 **앞**
⇒ **무덤 항목 하나당 정확히 한 번**, **rename/remove 어느 분기로 갈 항목이든 예외 없이** 발화한다.
**W11이 이 성질을 자기검증한다**(훅이 관측한 sha 집합 == 심은 무덤 4개 전부) — **FS-독립적으로 성립한다**
(`file_type()`이 소멸한 무덤에도 **캐시된 `Ok`** 를 주므로 `Gone`으로 빠지지 않는다).

### ⑥ `pins.rs` — **9번째 훅** `pre_recover_grave` (P-5)

**P-5의 핵심**: 구조적 불변식만으로는 *"헬퍼는 초록인데 프로덕션이 그 헬퍼를 안 쓴다"*를 못 잡는다.
증인을 **진짜 프로덕션 진입점**(`PassGuard::begin`)에 꽂아야 하고, 그러려면 복구 루프 안에 배리어가
필요한데 그 구간에서 발화하는 훅이 **하나도 없다** ⇒ **9번째 훅을 연다**(`Hooks` **필드 9개** —
`pins.rs:62`의 *"정확히 8개"*를 개정한다. **8번째 `pre_entry`는 red.sha가 이미 열었다 — 보존한다**).
**프로덕션에서는 항상 `None`**(`Hooks::default()`; `with_hooks`는 `#[cfg(test)]`) ⇒ **no-op** ⇒
**관측 행동 변화 0.**

**왜 보호 판정 경로를 만들지 않는가** (ADR-0002 P4 봉인 → **P14**):
`AsyncHook = Arc<dyn Fn(&str) -> BoxFuture<'static, ()> + …>`(`pins.rs:57`) — **`()`를 반환한다** ⇒ 값도
제어 흐름도 못 바꾼다 · 받는 것은 **`&str` 하나** ⇒ `landed`/`live`를 **보지 못한다** · 발화 지점이
`collect_referenced` **이전**이라 **`refs`조차 없다.** ⇒ **보호 판정은 여전히 `Graved::settle(self)`로만
도달 가능**하다.

`PassGuard::begin`(`pins.rs:427`)의 **선언과 호출부** — ⚠ **`&Vanished`가 *그대로 관통한다***
(r16/P-29: **패스 집계는 하나뿐이다** — §구현 ④의 `Vanished::new()` 호출부 1개 · §①의 A2·A4):

```diff
- pub(crate) async fn begin(store: &Store, settle_timeout: Duration) -> io::Result<PassGuard<'_>> {
+ pub(crate) async fn begin(store: &Store, settle_timeout: Duration, vanished: &Vanished)
+     -> io::Result<PassGuard<'_>> {
      …
-     let recovered = super::reconcile::recover_graves(&me.layout).await?;                  // collect **이전**
+     //  ⚠ **받은 그 참조를 그대로 넘긴다** — `pins`는 `Vanished`를 지을 수 없다(`E0624` · §A-0)
+     let recovered =
+         super::reconcile::recover_graves(&me.layout, me.pins.hooks(), vanished).await?;   // collect **이전**
```

⚠ **`begin`의 호출부는 8개다**: **프로덕션 1개**(`reconcile.rs`의 `run_once_at` — §구현 ④) ·
**`pins::tests` 7개**(`pins.rs:665·691·713·736·813·2536·2724` — §Scope). **전부 3인자로 고친다.**
(`grave`의 호출부는 **3개**: 프로덕션 1(`run_once_at`) · `pins::tests` 2(`:2547`·`:2728`).)

### ⑦ `atomic.rs` — **이 파일의 변경은 이 한 줄뿐이다** (실제 소스)

```rust
/// ⚠ 가시성만 `pub(crate)`로 넓혔다(F-14) — `reconcile::absence`가 rename+fsync를 **한 무취소
/// 클로저**에 유지하려면 필요하다(M6 봉인). **시그니처·본문·syscall 시퀀스는 불변.**
pub(crate) fn fsync_dir_blocking(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}
```

### §E-COMPILE. **의사코드를 그대로 쓰면 죽는 곳 — 전수** (r18 프로토타입의 컴파일 증거)

> **정직한 보고**: 프로토타입의 프로덕션 코드는 `cargo build`/`cargo test --lib` 모두 **첫 시도에
> 컴파일됐다(E0xxx 0건)** — **계획을 글자 그대로 옮기지 않고 아래 5곳을 작성 시점에 미리 교정했기
> 때문이다.** 각 항목은 반사실을 **컴파일러로 확인**했다. **이제 위 §①~§⑦이 교정된 실코드다.**

| # | 계획의 옛 의사코드 | 그대로 쓰면 | **실제 코드**(위에 반영됨) |
|---|---|---|---|
| **1** | `impl Entry { pub(super) async fn snapshot<'v>(dir, vanished: &'v Vanished) -> io::Result<Vec<Entry<'v>>> }` | `Entry`는 `Entry<'v>`다 ⇒ **`E0106`/lifetime 불일치** | **`impl<'v> Entry<'v>`** + `async fn snapshot(dir, vanished: &'v Vanished) -> io::Result<Vec<Self>>` (**자체 `<'v>` 재선언 금지**) |
| **2** | *"`rename_durable_source_checked`도 `pub(super)`로 좁혀라"*(§A-0 r16 각주) | `pub(crate) use`는 **가시성을 넓힐 수 없다** ⇒ **`E0364`** | **`pub(crate)` 유지.** 좁힌 것은 `entry_is_absent`·`rename_source_checked` **둘뿐** |
| **3** | `Seen::Gone(Absent)` · `GraveOutcome::SourceGone(Absent)` | 페이로드를 아무도 읽지 않아 **`dead_code` 경고**(경고 0 정책 위반) | **`Gone(#[allow(dead_code)] Absent)`** · **`SourceGone(#[allow(dead_code)] Absent)`** |
| **4** | `match pass.grave(..).await? { Moved(g) => match g.settle().await? {…} }` | arm 안의 `?`/`continue` 혼합으로 **타입이 갈린다** | **`let g = match … { SourceGone(_) => continue, Moved(g) => g };`** 로 분리한 뒤 `match g.settle().await? {…}` |
| **5** | Temp 분기의 `let Seen::Present(()) = entry.remove()… else { continue }` | `Gone`에서도 `temps_deleted += 1`이 돌거나 계수가 어긋난다 | **`match e.remove().await? { Present(()) => stats.temps_deleted += 1, Gone(_) => continue }`** |

**부수 — `reconcile.rs`의 import 델타**: `use std::io::ErrorKind;` **추가**(가드) · `classify_objects_entry`
**제거**(`entry.rs`로 이사). **`pins.rs`는 `use super::reconcile::{rename_durable_source_checked, Absent,
Renamed, Vanished};` 가 없으면 `E0433`** 이다.

**의도적으로 재현한 봉인 에러 (프로토타입 원문)**: `pins.rs`에서 `Vanished::new()` = **`E0624`** ·
`vanished.get()` = **`E0624`** · `Absent(())` = **`E0423`** · 자유함수를 재수출에서 빼면 **`E0425`**.
⇒ **`pins`는 집계를 짓지도·읽지도 못하고 `Absent`를 합성하지도 못한다. 재수출은 봉인을 넓히지 않는다.**

### 이 픽스가 **하지 않는** 것 (경계)

- **`Graved`/`Settled`/`settle()`은 무변경.** `settle()` 안의 `?`는 **이 패스가 방금 만든 무덤**을 다루므로
  "스냅샷 항목이 사라졌다" 클래스가 **아니다**(fail-CLOSED 유지).
- **`ReconcileStats`에 필드를 추가하지 않는다.** "skipped 카운터"는 **금지**다(→ P10 · F-29).
- **`src/layout.rs`의 `CommitPointerWalk`는 건드리지 않는다** — 같은 잠복 클래스이지만 `scope[]` 밖이다(**F-31**).
- **`reconcile.rs`의 기존 삼킴 3곳**(`:74` · `:115` · `:166`)은 **축자 보존**한다(→ **F-33 · F-34**).

---

## 왜 두 번째 플립이 없는가 — **죽는 지점별** 표

> **구조적 논증**: D안이 추가하는 syscall은 **둘뿐**이다 — ① `NotFound`가 났을 때의
> `symlink_metadata(path)` ② **소멸이 1건 이상인 패스**에서 루프 뒤의 `metadata(objects)` **1회**.
> ①은 **오늘이라면 `?`로 죽었을 자리**에서만 발행되고, ②는 **①이 최소 한 번 성공했을 때에만** 발행된다
> ⇒ **오늘 완주하는 패스에는 둘 다 발행되지 않는다** ⇒ **관측 플립은 하나다.**
> ⚠ **가드가 `Err`를 내는 세계에서는 `Err`의 *kind*가 오늘과 다를 수 있다**(가드의 `metadata`가
> EACCES/EIO/ELOOP을 낼 때). **극성은 같고 전파는 무가공**이다 — *"바이트 동일"*이라고 쓰지 않는다.

| 시나리오 | 오늘 (red.sha) | D안 | 동일? |
|---|---|---|---|
| **정상 패스(소멸 0)** | `Ok` | **확인 syscall 0 · 가드 syscall 0** → `Ok` | **패스 전체 트레이스가 바이트 동일** ✔ (실측: `m_noguard`와 D안이 같은 `Ok`) |
| **댕글링 blob 심링크** `objects/<64hex>→nope` | `read`(추종) → ENOENT → `?` → **`Err(NotFound)`**, 링크 잔존, pending 미기록 | `read` → ENOENT → 확인 = **`symlink_metadata` `Ok(symlink)` → `None`** → **원본 Err 무가공** | **바이트 동일** ✔ **W3** |
| **댕글링 temp 심링크** (old) | `e.metadata()` = **lstat 의미론 → `Ok`** → age>grace → unlink(링크) → **`temps_deleted=1`** | **같은 `de.metadata()`** → `Ok` · 확인 **미발행** → **`temps_deleted=1`** | **바이트 동일** ✔ **W4** |
| **비-UTF-8 `.tmp-` 이름** (old) | `e.path()`(원시 바이트)로 stat/unlink → **`temps_deleted = 1`** | **`e.path()` 그대로** ⇒ 같은 바이트를 커널에 넘긴다 ⇒ **`temps_deleted = 1`** | **바이트 동일** ✔ **W17** *(lossy 재구성 판본(**M46**)은 `symlink_metadata` ENOENT → `Absent` **정당 주조** → skip → `temps_deleted = 0` ∧ **live temp 영구 잔존** ⇒ RED)* |
| **`DT_UNKNOWN` FS의 타입 해석 시점** | tokio가 **readdir 시점에** 해석·캐시하고 실패는 `.ok()`로 삼킨다 | **같은 `de.file_type()`을 부른다** ⇒ 시점·캐시·삼킴이 **정의상 동일** | **동일** ✔ — **P-18이 정의상 소멸** |
| **musl/glibc/apple std 발산** | `DirEntry::metadata()`의 syscall이 타깃마다 다르다 | **그 메서드를 그대로 부른다** ⇒ 타깃과 무관하게 baseline과 동일 | **동일** ✔ — **B-15가 정의상 소멸** |
| **심링크→디렉터리**(절대 타깃) | `is_dir()`=false → `read` → **`IsADirectory`** → `?` | `IsADirectory ≠ NotFound` ⇒ `seen` 마지막 팔 → 무가공 | 동일 ✔ **W7** |
| **내구성 실패**: grave rename **Ok** → 부모 fsync 실패 | 하나의 `Result` → `Err`, 무덤 잔존, 다음 패스 복구 | `Renamed::SourceGone`이 **태어날 수 없는 코드 경로** → fsync 에러 **무가공** | **바이트 동일** ✔ **W5c · W6b** |
| **목적지 부재**: `.corrupt`가 **댕글링 심링크** | `mkdir_p` 통과 → rename → **ENOENT** → `?` → `Err`, blob 보존 | rename ENOENT → **소스 확인 = 존재** → **원본 Err**, blob 보존, `quarantined=0` | **동일** ✔ **W9b** |
| **목적지 ENOTDIR**: `.corrupt`가 일반 파일 | `Err(NotADirectory)` | `NotFound` 아님 → 갈림길에 닿지도 않음 | 동일 ✔ **W9a** |
| **EACCES/EIO/ENOSPC/EISDIR** (어디서든) | `?` → `Err(kind)` | 마지막 팔 → `Err(kind)`, 메시지 무변조 | 동일 ✔ **W1(d)** |
| **확인 `symlink_metadata` 자체가 EACCES/ENOTDIR** | (해당 없음) | `None` ⇒ **원본 에러** 전파 | 동일 ✔ **W5e′** |
| **★ `.objects` 파괴(재생성 없음) — 항목 접촉 잔존** | 첫 항목의 NotFound → **패스 `Err(NotFound/2)`**, 원장 미기록 | 전 항목 skip(부수효과 0) → **루프-후 가드** → `metadata` = ENOENT → **`Err(NotFound/2)` 무가공** · **부활 0 · 원장 0** | **동일** ✔ **W10 · W10-TEMP**(실측 T2 — 문자열까지 동일). ⚠ 가드가 **다른 errno**를 낼 수 있는 세계는 kind만 다르다(무가공) |
| **★ `.objects` 파괴 — 항목 접촉 0(꼬리 파괴)** | **`Ok`** + 컨테이너 부활 + 원장 `{}` (**오늘의 버그** — 실측 T1) | **가드 미발화**(`vanished == 0`) → **`Ok`** + 부활 + 원장 `{}` | **바이트 동일** ✔ **W10b** — **오늘의 버그를 보존한다**(고치면 두 번째 플립 · **F-42**) |
| **★ `.objects`가 일반 파일로 교체(ABA)** | 항목 연산 → **ENOTDIR** → `?` → `Err(NotADirectory/20)` | **B7이 무가공 전파**(가드에 닿지 않는다) | **바이트 동일** ✔ (실측: base·D안 둘 다 `(NotADirectory, 20)`) |
| **★ `.objects` 파괴 → 재생성 (적대적 ABA)** | 스냅샷 항목의 NotFound → **패스 `Err`** | 가드가 **살아 있는 dir**을 본다 → **`Ok`** + 빈 원장 | **✗ 다르다 — Class B-ABA**(§I · **데이터 손실 0** · 인간이 채택한 포기) |
| **★ 무덤 루프 안에서 파괴 → 재생성** | `recover_graves`의 `?` → **`Err`** | **무덤 루프의 부재가 패스 집계를 올린다**(A2·A4 — r15/P-27의 집계 관통) → `read_dir` = `Ok(빈 dir)` → 엔트리 루프 0회 → **`vanished > 0` ⇒ 가드가 돈다** → **재생성된 살아 있는 dir**을 보고 **통과** → **`Ok`** + `{}` 원장 | **✗ 다르다 — Class B-ABA의 두 번째 얼굴**(§D-② · **데이터 손실 0**). ⚠ **r15 정정**: *"`vanished == 0`이라 가드 미발화"* 는 **거짓이 됐다**(집계 관통) — **결론은 불변** |
| **9번째 훅 `pre_recover_grave`** | (존재하지 않음) | 프로덕션 `Hooks::default()` ⇒ `None` ⇒ **즉시 반환**: syscall 0 · FS 접촉 0 · 상태 0 · 반환값 없음 | **바이트 동일** ✔ — 기존 8개 훅과 **같은 논증**(P14) |
| **진짜 소멸**: 스냅샷을 뜬 그 `.objects`에서 항목이 rename/unlink로 사라짐 | 각 syscall ENOENT → `?` → **패스 전체 `Err`** | 확인 → **부재** → **그 항목만 skip · 패스 완주**(가드는 `Ok(dir)`) | **← 유일한 플립** |

---

## Single-Flip Contract

### 뒤집히는 **하나**의 관측 행동

> **before** — `.objects`(및 `recover_graves`) 스냅샷 항목에 대한 stat/read/rename/remove가
> `ErrorKind::NotFound`를 내면, 그것이 **무엇 때문이든** `?`로 전파되어 **`run_once`가 `Err`를 반환하고
> 패스 전체가 중단**된다 → 루프 끝 `write_atomic(.gc-pending.json)`에 **도달하지 못한다** → 2단계 GC 정지.
>
> **after** — `NotFound`가 났고 **∧** 그 syscall이 만진 **소스 디렉터리 항목**이
> **`symlink_metadata(e.path())` = `NotFound`** 로 **부재임이 확인된** 경우에**만** 그 항목을 **건너뛰고**
> (`continue`) 패스가 **완주**한다(`Ok(stats)` · pending 재기록).
> ⚠ **건너뛴 항목은 아무 카운터도 올리지 않고 아무 파일도 지우지 않는다**(`temps_deleted`는 `Present`
> 팔에서만 `:210` · `quarantined`는 rename `Present`일 때만 `:221` · `gc_deleted`는 `Reaped`일 때만 `:241`)
> ⇒ **`pending` 의미론 무변경**(§E).
>
> **그 외 모든 `NotFound`는 오늘과 바이트 동일하게 `Err`로 전파된다** — 댕글링 심링크(항목 **있음**) ·
> 목적지 부재 · **rename 성공 이후의 fsync 실패**. `NotFound` 이외의 io 에러도 전부 무가공 전파(**B7**).
> **`.objects` 자체의 파괴(재생성 없음)** 는 **루프-후 가드**가 오늘과 같은 `Err`로 낸다.
>
> ⚠ **잔여 (숨기지 않는다)**: **`.objects` 파괴 → *재생성*(적대적 ABA)은 조용한 `Ok`가 된다** —
> **엔트리 루프 안**과 **무덤 루프 안** 두 얼굴이다(⚠ **r15 정정**: 둘 다 **가드는 돌고** 재생성된 **살아 있는 dir**을 본다).
> **데이터 손실은 0**이다(운영자가 이미 지운 것 외에는) ⇒ **Class B-ABA**. 그리고 **꼬리 파괴**는
> **오늘도 조용한 `Ok`**이며 D안이 그것을 **보존한다**(F-42). ⚠ **F-14가 *새로* 만드는 데이터 손실 경로는
> 0이다** — *"데이터 손실 경로 0"*이 **아니다**(기존 구멍 **F-41 · B-REFS**는 별개이며 **오늘도 열려 있다**).

### 하류 범위 — **완주한 패스가 하는 모든 일이 이 플립의 하류다** (★r20/P-33 · 로그 스트림 실측)

> **플립은 *"패스가 완주한다"* 이다.** 따라서 **완주한 패스가 수행하는 모든 기존 연산과 그것이 내는 기존
> 로그는 그 플립의 *하류 결과*이지 별개의 관측 행동 변화가 아니다.** 오늘은 `?`가 패스를 죽여 그 연산들에
> **도달하지 못했을 뿐**이다 — 도달하면 **그 연산들은 오늘과 똑같은 의미론으로** 실행된다.
>
> ⚠ **이것은 B7 · `ReconcileStats` · 격리 · GC 의미론의 보존 주장과 모순되지 않는다.** 두 층위가 다르다:
> **보존 주장은 *연산의 의미론*** (격리는 여전히 sha 불일치일 때만 · GC는 여전히 `refs ∨ landed`로 보호 ·
> 비-`NotFound`는 여전히 무가공 전파 · stats 필드는 여전히 5개)이고, **하류 도달성은 *그 연산이 실행되느냐***
> 의 문제다. **한 번도 실행되지 못하던 격리가 실행된다고 해서 격리의 의미론이 바뀐 것이 아니다.**
> (⇒ P1·P6·P9·P10은 전부 그대로 선다. 실측: S2에서 격리 WARN 2건이 새로 나지만 격리된 것은 **오늘 코드도
> 격리했을 바로 그 비트로트 blob 2개**다.)

**런타임 스트림이 달라지는 지점 — 전수(7곳 · 두 방향).** 근거는 §B의 13개 발화 지점 표.

| # | 발화 지점 | 레벨 · 메시지 | 방향 | 근거 |
|---|---|---|---|---|
| 1 | `reconcile.rs:262` | WARN `quarantined corrupt blob (bit rot)` | **새로 도달** | **실측 S2**: red `Err`+**0 이벤트** → D안 `Ok`+**WARN ×2** |
| 2 | `reconcile.rs:290` | INFO `GC restored: landed commit` | **새로 도달** | 엔트리 루프 · 소멸 항목 **뒤**의 landed 무덤 — 코드 경로상 도달 가능(**무대 미측정 · 정직**) |
| 3 | `pins.rs:648` | ERROR `gc settle timed out` | **새로 도달** | 같음(`grave()`→`settle()` 경유 · **무대 미측정**) |
| 4 | `reconcile.rs:159` | INFO `grave recovered` | **새로 도달** | **실측 S4**(무덤 루프에서 첫 무덤 소멸 · 무덤 2개) |
| 5 | **`reconcile.rs:190`** | INFO `graves recovered from a previous pass` (`recovered=1`) | **새로 도달** | **실측 S4 — ★ r20 초안이 빠뜨렸던 것.** 이 이벤트는 엔트리 루프가 **아니라 그 앞**(무덤 루프 종료 직후)에서 난다 ⇒ *"엔트리 루프 안의 넷"* 이라는 열거는 **틀렸다.** red는 `rename_durable(grave→blob)`의 `?`(`reconcile.rs:121`)에서 죽어 **둘 다 못 낸다**(실측 `kind=NotFound errno=2`) |
| 6 | `main.rs:52` | INFO `reconcile` (`?stats`) | **새로 도달** | **실측 S2/S5**: 패스 결과가 `Err` → `Ok(stats)`로 뒤집히므로 호출부의 `match` 팔이 바뀐다 |
| 7 | `main.rs:53` (부팅 경로는 `main.rs:39`) | WARN `reconcile failed` / `boot reconciliation failed` | **★ 사라진다** | 같은 실측의 **반대 방향**. **오늘 매 패스마다 뜨던 WARN이 침묵한다** — 운영자가 그 WARN에 알람을 걸어 뒀다면 **알람이 멈춘다**(그것이 이 픽스의 *목적*이지만, **로그 삭제 방향을 숨기지 않는다**) |

**로그가 없는 하류**(정직 · 실측): `.gc-pending.json` 발행 · settle `Deferred` · 격리 **stats** 카운터 ·
`locks.rs:116`(reconcile 패스는 key lock을 잡지 않는다 ⇒ **도달 불가** · grep + 무필터 구독자로 확인) —
**전부 침묵**이다.

### `scope[]` (bugfix-lock.json — ⚠ **개정 필요: `docs/adr/**` + `scripts/f14-witness-gate.sh`**)

```json
"scope": ["src/store/**", "docs/adr/**", "scripts/f14-witness-gate.sh"]
```

비-테스트·비-문서 변경 경로는 `src/store/pins.rs` · `src/store/reconcile.rs` · **신규**
`src/store/reconcile/absence.rs` · **신규** `src/store/reconcile/entry.rs` · `src/store/atomic.rs`
(**`fsync_dir_blocking`의 가시성 1줄뿐 — 그 파일의 API는 전부 그대로다**, §Scope) —
**전부 `src/store/**` 안**이다. **늘어나는 것은 둘**이다:

1. **`docs/adr/**`** — **ADR-0002 개정**(9번째 훅 + P4 봉인 논증의 기록).
   **B4 근거**: ADR은 마크다운이라 컴파일·링크·실행되지 않는다 ⇒ **관측 행동을 만들 수 없다.**
2. **★ `scripts/f14-witness-gate.sh`**(★r22/P-35·P-36) — **증인 레지스트리의 단일 권위**(§B-1 0단계).
   ⚠ **`isTestPath`가 이것을 테스트로 판정하지 않는다**(실측:
   `isTestPath("scripts/f14-witness-gate.sh") = false` — 정규식은 `(^|/)(tests?|__tests__|spec)(/|$)`다)
   ⇒ **선언하지 않으면 `scopeViolationsOf`가 B4 위반으로 잡는다.**
   ⚠ **와일드카드가 아니라 *정확 경로*다** — 비-테스트 표면이 **정확히 파일 하나만큼** 넓어진다
   (실측: `globMatch("scripts/f14-witness-gate.sh", "scripts/f14-witness-gate.sh") = true` ∧
   `globMatch(…, "scripts/other.sh") = false`). **`scripts/**`로 넓히지 않는다.**
   **B4 근거**: 셸 스크립트는 **컴파일·링크되지 않는다** ⇒ **관측 행동을 만들 수 없다**(ADR과 같은 논증).
   **왜 파일이어야 하는가**: 게이트가 하네스 **안**에 있으면 **`#[ignore]` 한 줄로 꺼진다**(§B-1 0-e 실측)
   ⇒ **`tests/` 안의 Rust 테스트(scope 개정 불필요)로는 P-36을 봉인할 수 없다.**

(`tests/**`는 `bugfix-status.mjs`의 `isTestPath`가 테스트로 판정하므로 B4 표면 바운드 밖이다 — §F-3.)

### `flips[]` (**2행 · 동결**)

```json
"flips": [
  { "testId": "reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot",
    "symptomToken": "PASS ABORTED" },
  { "testId": "reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot",
    "symptomToken": "PASS ABORTED" }
]
```

**두 행은 *두 개의 플립*이 아니다 — *하나의 관측 행동에 대한 두 증인*이다**(하드룰 10이 명시적으로 허용).

| | Blob 증인 (기존) | **Temp 증인 (P-15)** |
|---|---|---|
| 때리는 `?` | **②** `tokio::fs::read(&p).await?` (`:215`) | **①** `e.metadata().await?` (`:206`) |
| park하는 훅 | `pre_grave` | **`pre_entry`** (8번째 · red.sha가 열었다) |
| 결정성의 출처 | **첫 발화에서 park** + gravable ≥ 2 | **이름으로 표적한 park**(readdir 순서 무관) |
| 프로덕션 도달성 | 아웃오브밴드 / D-3 | **상시** — `write_atomic`·`put_stream`의 rename |

---

## Preserved Contract

| # | 보존할 것 | 왜 안 깨지는가 | 핀하는 증인 |
|---|---|---|---|
| **P1** | **B7 — `NotFound` 이외 무가공 전파** (⚠ 정확히: *스냅샷 항목 연산에 대해*) | `seen`의 마지막 팔이 **유일한 갈림길**. **정직**: 이 파일에는 픽스 이전부터 io 에러를 삼키는 지점이 셋 있다(`:74` · `:115` · `:166`) — **전부 축자 보존**(→ F-33/F-34) | **W1(d)** · **W7** · **W9a** · **W13** · 기존 T-B5 |
| **P2** | **댕글링 심링크 행동** (P-1) | `Gone`은 **`symlink_metadata`(no-follow)** 부재 확인에서만 난다. 심링크는 항목이 **있다** | **W3** · **W4** |
| **P3** | **내구성 오류 전파** (P-2) | `Renamed::SourceGone`은 **`std::fs::rename`의 `Err` 팔**에서만 태어나고, `Absent` 없이는 만들 수 없다. **`rename_durable`(융합)에는 확인을 붙이지 않는다** | **타입(`Absent`)** · **W5c** · **W6b** |
| **P4** | **목적지 에러 전파** | 확인은 **소스 경로**에만 걸린다(`seen`은 `self.path`만 · `rename_*_source_checked`는 `from`만) | **W5b** · **W9b** |
| **P5** | **컨테이너 파괴는 *현실적 파국에 한해* 여전히 시끄럽다** — 파괴 후 **재생성되지 않으면**(SSD 언마운트 · 운영자 `rm -rf`) 루프-후 가드가 **오늘과 같은 `Err`** 를 낸다. ⚠⚠ **P5는 전면적이지 않다**(아래) | **루프-후 가드**(§B)가 `write_atomic` **이전**에 `metadata(.objects)`를 1회 본다 ⇒ 파괴 후 미재생성이면 **`Err(NotFound/2)`** — 오늘과 **kind·errno·메시지 동일**(실측 T2). ⚠ **포기한 것 ①**: **파괴 → 재생성(적대적 ABA)** 은 **조용한 `Ok`** 다 — 엔트리 루프 안 · **무덤 루프 안** 모두 **가드가 돌고 재생성된 살아 있는 dir을 본다**(r15 정정) ⇒ **Class B-ABA**(데이터 손실 0). ⚠ **포기한 것 ②**: **꼬리 파괴**(FS-접촉 0)는 **오늘도 조용한 `Ok`** 이며 D안이 **보존**한다 ⇒ **F-42**. ⚠ **③** 가드가 낼 수 있는 `Err`의 **kind는 오늘과 다를 수 있다**(EACCES/EIO/ELOOP — 무가공) | **W10 · W10-TEMP · W10-G · W10b · W10c** |
| **P6** | **2단계 tombstone GC 의미론 — 무변경** | `pending`의 삽입·만료·정리 로직이 **한 글자도 바뀌지 않는다**(§E: C안의 `pending.remove` 추가를 **폐기**했다) | `unreferenced_old_blob_is_gced` · `unreferenced_recent_blob_preserved` · `layout_tree.rs:74,142,205` |
| **P7** | **무덤/정산 봉인**(ADR-0002) | `Graved`·`settle()`·`Settled` **무변경**. 생성자 조건은 **더 좁아진다** | `pins.rs` T-C1/T-C2/T-S1/T-S2/T-P4b-1/T-P4b-2/T-B5 |
| **P8** | **temp grace 보존** | 나이 판정 불변 · **`de.metadata()`를 그대로 부른다** ⇒ **모든 타깃에서 baseline과 같은 syscall** · `.modified().unwrap_or(now)` **축자 보존** | `old_temp_deleted_recent_preserved` · `put_stream_midflight_temp_observed_and_preserved` · **W4** |
| **P9** | **비트로트 격리** | 판정식(`hex::encode(Sha256::digest(&content)) != name`) 불변 · `mkdir_p_durable`·`fsync_dir`은 **raw `?`** | `corrupt_blob_quarantined` · **W9a/W9b** |
| **P10** | **`ReconcileStats` 정의 불변** | 필드 추가 0. **`vanished`는 stats가 아니라 패스 지역 `Vanished`다** | `layout_tree.rs:74,142,205`의 **전수 구조체 `assert_eq!`** · **W13의 전수 `assert_eq!`** |
| **P11** | **O1/O2 syscall 순서 · 무-stat 예약 이름 · *패스 전체* 트레이스** — *"**오늘 완주하는 모든 패스**의 syscall 트레이스가 **항목별로도 패스 전체로도** 모든 타깃에서 바이트 동일하다"*(**C안이 핀 때문에 포기했던 문언이 복원된다**) | `class()` syscall 0 ⇒ O1 · `file_type()` → skip 순서 ⇒ O2 · 확인 syscall은 **`NotFound`가 났을 때에만** · **가드는 `vanished > 0`일 때에만** — 그리고 **`vanished > 0`이면 그 패스는 오늘 `Err`로 죽었을 패스다**(모든 `Absent` 주조는 `Err(NotFound)` 뒤에만 도달한다) ⇒ **오늘 `Ok`인 패스는 가드 syscall을 0회 발행한다**(실측 확인) | **W2** · **W13**(전수 `assert_eq!`) |
| **P12** | **커밋 포인터 심링크 행동** | reconcile은 `layout.rs` 워커를 만지지 않는다 | `symlinked_commit_pointer_current_behavior` |
| **P13** | **characterization 전원 초록 = `0 failed`.** ⚠ **계수는 트리마다 다르다**: red.sha에서 **138**(`--verify-red` 실측 · 동결) · **픽스 트리에서 141**(lib 121 = 123 − 2 skip — r18 프로토타입 **실측**). **"138과 같아야 한다"고 쓰지 않는다** — 픽스가 lib 증인 3개를 더하므로 **원리적으로 불가능**하다 | 위 P1~P12의 합 | `characterizationCmd` (**`0 failed`로 게이트한다**) |
| **P14** | **ADR-0002의 P4 봉인 — "보호 판정은 `Graved::settle(self)`로만 도달 가능"이 9번째 훅에도 불구하고 성립** | ① `AsyncHook`의 반환형이 **`()`** ⇒ 값도 제어 흐름도 못 바꾼다 ② 받는 것은 **`&str` 하나** ⇒ `landed`/`live`를 못 본다 ③ 발화 지점이 **`collect_referenced` 이전** ⇒ `refs`가 없다 ④ `Graved`의 유일한 생성자는 여전히 `grave()`의 `Renamed::Done` 팔이다 | ADR-0002 봉인 체크리스트 ④⑤ · 기존 `pins.rs` T-* 전원 초록 · **W11** |
| **P15** | **원시 파일명 바이트** — `.objects` 항목의 stat/read/unlink/rename이 커널에 넘기는 **이름 바이트가 오늘과 동일**하다 | **`e.path()`를 그대로 쓴다**(std가 원시 바이트를 보장) ⇒ **P-16이 정의상 소멸.** lossy `String`은 **분류·로깅·원장 키·목적지 이름 전용**이며 그 넷에 도달하는 클래스(`Blob`·`Grave`)는 **정의상 ASCII**다(`is_sha_name`). **자물쇠**: `Entry`에 **`dir` 필드가 없어** `dir.join(&name)` 재구성 뮤턴트(**M46**)는 **필드 추가**를 요구한다(diff에 크게 보인다). **행동 자물쇠는 W17 하나뿐이고 Linux에서만 돈다** → **B-12** | **W17**(Linux) · B-5 diff 리뷰 |
| **P16** *(★r19 — P-32 · **r20/P-33에서 로그 스트림 실측으로 재작성**)* | **로깅 계약 = ① 호출부·스키마 보존 + ② skip 시 침묵 + ③ 완주로 도달되는 기존 이벤트는 *단일 플립의 하류*** — ⚠ *"로깅 **행동**이 동일하다"* 고 쓰지 않는다(**그것은 런타임 스트림에 대해 거짓이다**) | **①**(실측: 전수 grep + S1) **신규 tracing 이벤트 0** · 발화 지점 **13곳 = 13곳**(추가 0 · 삭제 0), 기존 이벤트의 **호출부·레벨·target·메시지·필드 무변경**(줄번호만 이동: `reconcile.rs:127/155/222/245 → 159/190/262/290` · `pins.rs:597 → 648`) · **`debug!`/`trace!`는 크레이트 전체 0건이고 이 픽스도 만들지 않는다** · **소멸 0인 패스의 이벤트 스트림은 red.sha와 바이트 동일**(S1 — 3 이벤트, 순서·필드까지. 적대적 반증이 **독립 재현**: 양쪽 블록 md5 `a97d0942…` · diff 무출력). **②**(실측: S3) 사라진 항목을 건너뛰는 `continue`는 **어떤 레벨에서도 이벤트를 내지 않는다**. ⚠⚠ **★r26 — 이 칸의 "전수"는 이제 *실측된 전수*다.** **`Seen::Gone` → skip 팔은 `:252` + 7개가 아니라 *9개*다**: `:133`·`:149`·`:154`·`:227`·`:236`·**`:244`**·`:252`·`:257`·`:280` — 이전 개정판은 **`:244`(Temp `remove()`의 `Seen::Gone` 팔 = *"우리가 지운 게 아니다"*)를 열거조차 하지 못했다**(정본 = `log_witness.rs`의 **전수표**). **커버리지(실측)**: **W-LOG-C** = `:252` 하나 · **W-LOG-D** = **`:149`·`:154`·`:236`·`:244`·`:252`·`:280`**(무대 6개) ⇒ **밟을 수 있는 팔 6/6 전수**. **덮지 못한 3개는 정직하게 등재한다**: **`:133`·`:227`**(무덤/엔트리 루프의 `file_type()`) = **도달 불가**(d_type 캐시) — **`continue` → `panic!` 프로브로 실증**: 전 스위트 × **3회 반복** **0 failed**(= 어떤 테스트도 그 팔을 밟지 않는다) ⇒ **Class B-FT** · **`:257`**(격리 `rename_into()`) = **배리어 부재** — `read()`와 `rename_into()` **사이에 훅이 하나도 없다** ⇒ 결정적 무대를 지을 수 없고, 랑데부 통합 증인(Phase E)조차 **밟지 못한다**(같은 `panic!` 프로브 · 3회 **0 failed**) ⇒ **Class B-QUAR**. **뮤턴트 킬 실측 9/9**(§4의 M-LOG-ARM-\* 표가 원문): **6 KILLED**(전부 **W-LOG-D**가 킬러 — `:252`만 W-LOG-B/C도 함께 죽인다) · **3 SURVIVED**(= 위 3팔, **커버 불가임을 실측으로 증명한 것들**) ⇒ **W-LOG-D가 이식의 차단 요건이다**. **③** **패스가 완주하므로 베이스라인이 중단으로 도달하지 못하던 *기존* 이벤트가 발화할 수 있고, 오늘 매 패스마다 뜨던 *기존* WARN이 사라진다** — **하류 목록은 §Single-Flip Contract의 “하류 범위” 표**(7곳 · 두 방향)가 정본이다. **그것은 별개의 관측 행동 변화가 아니라 “패스가 완주한다”는 단일 플립의 하류 결과다** | **W-LOG**(`src/store/pins/tests/log_witness.rs` — **`pins.rs:995-1037`의 `CaptureSubscriber`를 재사용·확장한 `EventTap`**): **W-LOG-A**(특성화 · 소멸 0 스트림 **전수·순서** `assert_eq!`) · **W-LOG-B**(green-only · 하류 이벤트가 **정확히 (N−1)건**, 그 외 0) · **W-LOG-C**(green-only · Blob `read()` skip 시 **모든 레벨 0건**) · **W-LOG-D**(★차단 요건 · **밟을 수 있는 skip 팔 6개 전수의 침묵** — 무대 ①`:236` ②`:244` ③`:252` ④`:280` ⑤`:149` ⑥`:154`). ⚠ **r19의 근거였던 *“스위트가 tracing subscriber를 설치하지 않는다”* 는 거짓이었다** — `pins.rs`의 **4개 테스트가 `set_default`로 설치해 이벤트 수를 정확히 단언한다**(`:1091`·`:2043`·`:2178`·`:2282`). **`flips[]`에는 넣지 않는다**(2행 동결 · 같은 하나의 플립의 추가 증인). 시야 한계(정직): `EventTap`은 **`files*` target · `set_default` 스레드**만 본다(오늘 그 밖의 발화는 **0건** — 무필터 구독자로 실측) ⇒ **B-5 diff 항목**. 관측성 카운터를 여는 일은 별도 파이프라인(→ **F-29**) |

### ⚠ **금지** — "skipped 카운터"를 `ReconcileStats`에 올리는 것

`tests/layout_tree.rs`가 **전수 구조체 `assert_eq!`**로 stats를 핀한다 → 필드를 하나라도 늘리면 그 3개가
깨진다 = **두 번째 관측 행동 플립**. 관측성이 필요하면 **`tracing` 전용**이다(→ **F-29**).

---

## Regression witnesses — **둘 다 이미 RED at red.sha**

| 항목 | 값 |
|---|---|
| **red.sha** | **`ac58bd7982d06e46f37cd4aa6a9c274d93bd8195`** (main) |
| **regressionCmd** | `cargo test --lib -- reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot` |
| **symptomToken** | `PASS ABORTED` (**두 증인 공통**) |
| **`--verify-red`** | **통과** — regression: exit **101** · **2 failed** · `symptomTokenPresent: true` / characterization: exit **0** · **138 passed** |

**증인 ① — Blob 분기** (`src/store/pins/tests/vanished_entry_regression.rs`) — `pre_grave`가 **스냅샷
순서대로** 발화하므로 첫 발화에서 park하면 파킹된 항목 뒤는 전부 미처리다. gravable orphan 3개를 심고
파킹되지 않은 2개를 지우면 Blob 분기의 `read()`가 ENOENT → 패스 중단. **readdir 순서 무관 100% 결정적.**
대조군 `reconcile_pass_control_without_vanishing_entries_is_green` 동봉.

**증인 ② — Temp 분기** (`src/store/pins/tests/vanished_temp_regression.rs`) — `pre_entry`가 **바로 그 temp
이름**으로 발화할 때만 park한다 ⇒ **readdir 순서 무관 100% 결정적**. park 중 그 temp를 지우면 `metadata()`
ENOENT → `?` → **패스 전체 Err**. **RED 20/20 · 대조군 GREEN 20/20.**

**⇒ 둘 다 D안에서 GREEN이 된다** (r14 적대적 반증이 D안 구현본에서 실행으로 확인: 회귀 증인 2개 RED→GREEN ·
전 스위트 GREEN).

**⚠ 8번째 훅 `pre_entry`는 red.sha에 이미 들어갔다 — B-1이 만드는 것이 아니라 *보존*한다.**
⇒ **B-1이 여는 `pre_recover_grave`는 9번째 훅이다.**

---

## Increment plan

| id | 이 증분이 하는 일 | blocked-by | notes |
|---|---|---|---|
| **B-1** | **신규 `src/store/reconcile/absence.rs`**: **`Absent`**(위조 불가 토큰) · **`Vanished`**(derive 0 · `new`/`get` = `pub(super)` · `bump`/`share` = 모듈 private · **`#[cfg(test)] new_for_test` 다리**) · `entry_is_absent{,_blocking}(&Vanished, &Path)`(**`symlink_metadata` no-follow** · **fd 0**) · `Renamed` · `rename_source_checked` / `rename_durable_source_checked`(**`SourceGone`은 `std::fs::rename`의 `Err` 팔 전용**) · 신규 자식 모듈 **`src/store/reconcile/entry.rs`**(`Seen<T>` · `Entry{de, path, name, class, vanished}` · `Entry::snapshot` = **`read_dir` 그대로**) · **`pins::Grave` 반환형 + `grave(sha, &Vanished)`** · **`PassGuard::begin(store, settle, &vanished)`** · **`pins::tests`의 9개 호출부 갱신**(§Scope) · `.objects` 루프와 `recover_graves` 루프의 **모든** 경로-접촉 연산을 `Entry`로 통과 · **★ 루프-후 컨테이너 가드**(`vanished > 0` 게이트 · `write_atomic` **이전** · `metadata`+`is_dir`) · **`recover_graves` 분해**(P-4) · **9번째 훅 `pre_recover_grave` 신설**(P-5 — ⚠ **8번째 `pre_entry`는 red.sha에 이미 있다. 보존하라**) · 결정적 증인 **W1~W7 · W9 · W10 · W10-TEMP · W10-G · W10b · W10c · W10c′ · W-GRAVE-CD-A/B · W11 · W17 · W-LOG-A/B/C/D** 추가 · **통합 증인 W13**(`tests/reconcile_vanishing_entries.rs`) 신설 · **★ 증인 파일 3개 신설 + `mod` 등록 3줄**(`log_witness` · `vanished_container_witnesses` · `recover_graves_production_seam` — §Scope의 표가 정본. **등록을 빠뜨리면 증인이 컴파일조차 되지 않는다** = **M-NOMOD**) · **★ 증인 게이트 `scripts/f14-witness-gate.sh` 신설**(★r22 · **★r23에서 파서 재작성** — **증인 ID의 단일 권위**: **① 타깃별 `--list` 발견 단언**(앵커 `(^\|::)<id>: test$`) **∧ ② 결과 게이트**(`test result:` 줄에서 **숫자를 뽑아 정수 0과 비교** — `ignored`·`failed`·결과 줄 수·cargo exit. ⚠ **부분문자열 매칭 금지** — `grep -vc '0 ignored'`는 **`10 ignored`를 통과시킨다**(P-37)) **∧ `--selftest`**(**§0-h가 정의하는 전 케이스** — ⚠ **케이스 수·술어 목록의 정본은 §0-h이며 이 칸은 숫자를 반복하지 않는다**(★r25/P-40) · **게이트가 자기를 증명한다**). ⚠ **`scope[]` 개정 필요** · ⚠ **W-REG는 폐기한다**) · **훅 계수 개정**(⚠ **개정 위치의 정본은 §7의 표다 — 이 칸은 열거하지 않는다**) · **ADR-0002 봉인 체크리스트 개정** · `tests/adversarial.rs`의 눈가리개 제거 | none | **fix-seam = 유일 증분** |

### 왜 단일 증분인가

- 픽스는 **한 seam**이다(부재 판정의 단일화). `Absent`/`Entry`를 도입하는 증분과 호출부를 옮기는 증분으로
  쪼개면 앞 증분은 **아무 호출자도 없는 죽은 타입**을 커밋하는 것이 되고 회귀 증인은 **여전히 RED**다.
- 락의 `review-track`은 **standard**다 ⇒ 의존 증분이 없으면 structure 게이트는 발화하지 않는다.
  **plan 게이트와 release 게이트가 이 diff 전체를 본다.**

### B-1 acceptance

> ⚠⚠ **r18 프로토타입 실측 — *확인된 것*과 *아직 확인 안 된 것*을 나눈다.**
> D안 **전체**를 red.sha 복제본에 구현해 돌렸다(원본 저장소는 읽기만 — git 0회).
>
> **✅ 프로토타입에서 실제로 확인된 것**
> * `cargo build` — **경고 0**. `cargo test --lib --bins --tests` — **전부 GREEN**
>   (lib **123 passed** = red.sha의 120 + 새 증인 3 · adversarial 8 · contract 1 · e2e 2 · layout_tree 3 ·
>   openapi 5 · regression_reconcile_gc_dedup_race 1). `--lib` **3회 반복 전부 123 passed** ⇒ **flaky 0**.
> * **회귀 증인 2개 RED → GREEN**(+ 대조군 2개 GREEN) — red.sha에서 둘 다 **실제로 FAILED**함을 재확인.
> * **characterization 전원 GREEN**(아래 2번 — **숫자는 141이다. 138이 아니다**).
> * **가시성 봉인**: `pins`에서 `Vanished::new()`/`.get()` = **`E0624`** · `Absent(())` = **`E0423`** ·
>   자유함수 재수출 누락 = **`E0425`** (전부 컴파일러 원문으로 실증).
> * **단일 집계**: `Vanished::new()` **코드 호출부 1개**(`reconcile.rs`) · `new_for_test` 호출부는
>   `pins::tests` **9곳**(begin 7 · grave 2)뿐 — 계획의 계수와 **정확히 일치**.
> * **`ReconcileStats` 필드 추가 0**(`layout_tree.rs`의 전수 `assert_eq!` 3개 GREEN) · **`unsafe` 0** ·
>   **`Cargo.toml` 바이트 동일** · **크레이트 외부 신규 `pub` 심볼 0** · clippy 경고가 **위치까지
>   baseline과 동일**(변경/신설 파일에서 **신규 0**).
> * **행동 보존 6렌즈**(댕글링 심링크 · 비-NotFound · O1/O2 순서 · 정상 패스 추가 syscall 0 ·
>   rename `Ok` 후 fsync 실패 · stats 불변)를 **실행으로 반증 시도** — red.sha 트리와 픽스 트리에
>   **동일 바이트의 프로브 11 시나리오**를 넣고 출력을 diff ⇒ **완전 동일**.
> * **뮤턴트 킬 3건 실측**: **M-NOBUMP-BLOCKING** → `W-GRAVE-CD-A`**만** RED(*"두 채널은 서로를 못
>   덮는다"*가 **실행으로** 확인됐다) · **M-FOLLOW** → `W3`만 RED · **M-GUARD-AFTER** → `W10` ∧
>   `W-GRAVE-CD-A` **둘 다** RED.
>
> **❌ 아직 확인되지 않은 것 (이식 시 반드시 해야 한다)**
> * **증인 6개 + 게이트 스크립트를 구현했다**(★r22에서 재계수): **W10 · W-GRAVE-CD-A · W3**(r18) +
>   **W-LOG-A · W-LOG-B · W-LOG-C**(r20) + **`scripts/f14-witness-gate.sh`**(r22 — **구 W-REG를 대체한다** ·
>   **★r23에서 파서를 숫자 비교로 재작성 + `--selftest` 추가** — 둘 다 프로토타입에서 **실행 확인**).
>   **나머지는 미구현**: W1 · W2 · W4 · W5(a~e′) · W6 · W6b · W7 · W9a/W9b · **W10-TEMP** · W10-G ·
>   **W10b** · W10c · W10c′ · W11 · W13 · W17 · W-GRAVE-CD-B · **W-LOG-D**.
>   (프로토타입 lib = **132 passed**. ⚠ **`log_probe.rs`는 이식하지 않는다** — 증인이 아니라 계측 프로브다.)
>   ⚠ **게이트를 정본 레지스트리 전행**(§0-b `WITNESSES`가 정본 · **실측 당시 35행**)**으로 프로토타입에
>   돌리면 미구현 24개가 전부 `MISSING WITNESS` · exit 1**이다(실측) ⇒ **이식이 증인을 빠뜨린 채로는
>   머지될 수 없다**(M-NOMOD′).
>   ⚠ **W13 통합 파일은 프로토타입에서 *스텁*이다** — `--list` 형태와 게이트 기계장치를 실증하기 위한
>   최소 골격이고, **§W13의 규범 명세(Phase E/G/T 본문)는 이식 시 구현한다.**
> * ⚠⚠ **W10b가 없으므로 `vanished.get() > 0` 게이트를 지우는 뮤턴트(M-GUARD-ALWAYS)를 현 스위트가
>   못 잡는다** — 프로토타입에서 `if true`로 바꾼 뮤턴트가 `--lib --bins --tests` **전부 GREEN**으로
>   살아남았다(실측). **두 번째 관측 플립을 막는 유일한 장치가 증인 0으로 나간다.** ⇒ **W10b는 이식의
>   차단 요건이다.**
> * **9번째 훅 `pre_recover_grave`는 배선만 했고 W11은 없다**(프로덕션 `None` ⇒ no-op이라 컴파일·스위트
>   무영향).
> * **문서·계수 개정(§7-a~7-d)은 프로토타입에서 하지 않았다** — `pins.rs:62`의 *"필드는 정확히 8개다"*와
>   `run_once_at_for_test`의 독 코멘트가 **코드(9개)와 모순인 채로 있다** · `docs/adr/0002-*` ·
>   `bugfix-lock.json`의 `scope[]`도 동일. **이식 시 필수 동반 수정.**
> * **`--release` 2줄**(B-1 보상 통제)은 프로토타입에서 **돌리지 않았다.**
> * **B-ABA**(파괴 → 재생성)는 여전히 **오늘의 `Err`를 조용한 `Ok`로 뒤집는다** — 픽스에 내재하고
>   6렌즈 어디에도 걸리지 않는다(인간이 채택한 포기 · **데이터 손실 0**).

**0) ★ 증인 게이트 `scripts/f14-witness-gate.sh` — 단일 권위 (★r24/P-38·P-39 · 스위트보다 **먼저** 돈다)**

> **0개 발견이 통과가 될 수 없다.** `cargo test`는 **등록되지 않은 파일의 테스트를 그냥 없는 것으로 취급하고
> `0 failed`를 보고한다**(P-34). 그리고 **`#[ignore]`가 붙은 증인도 `0 failed`로 보고된다**(P-36).
> ⇒ 게이트는 **① 선언된 증인이 그 타깃의 바이너리에 실재하는가**와 **② 재갈 물린 증인이 0인가**를 **둘 다
> 실행으로** 판정해야 한다.
>
> ⚠⚠ **r22가 이 두 가지를 다 틀렸고(P-35·P-36), r23은 그 수리 자체가 틀렸고(P-37), r24는 *게이트와 그
> selftest*가 또 틀렸다(★P-38·★P-39). 전부 실행으로 잡혔다.**
> **(P-35)** 앵커가 **`::<id>: test$`** 라 **통합 최상위 증인 10개**가 거짓 MISSING으로 죽었다. **(P-36)**
> *"`0 ignored` 게이트"* 가 **산문**이었다. **(★P-37)** 그 수리가 **`grep -vc '0 ignored'`** — **부분문자열**이라
> **`10 ignored`를 통과시켰다**(하필 **10 = 통합 증인의 수** ⇒ **차단 증인 포함 열 개를 재갈 물려도 초록**).
> **(★P-38)** 발견 검사가 **`pipefail` + `list_for | grep -qE`** 였다 ⇒ `grep -q`가 조기 종료하면 상류가
> **SIGPIPE** ⇒ 파이프라인 **141** ⇒ **존재하는 증인이 거짓 `MISSING WITNESS`**(§0-d (f) — 목록이 클수록
> **비결정적으로** acceptance를 막는다).
> **(★P-39)** 그리고 **그 게이트의 `--selftest`가 selftest가 아니었다** — (d)·(f) 픽스처가 **cargo exit 101을
> 함께 넘겨** 다른 술어가 기대 실패를 대신 공급했다 ⇒ **숫자 `failed` 검사와 결과-줄-0개 가드를 지워도
> 6/6이 초록**이었다(실측) ⇒ **M-FAILED-10과 no-results 가드는 핀되어 있지 않았다.**
> ⇒ **게이트가 게이트가 아니었다. 다섯 번. 그리고 게이트의 selftest도 selftest가 아니었다.**
> ⇒ **파이프를 없애고**(파일 캐시 + 종료 상태 검사) **selftest를 직교화한다**(픽스처 = **(출력, rc) 쌍** ·
> **술어 하나당 케이스 하나** · **모든 술어에 대해 "지우면 RED"를 실증** — §0-h).

**0-a. 타깃별 `--list`가 무엇을 내는가 — 실행 원문** (프로토타입 · macOS/APFS · 2026-07-14)

```
$ cargo test --lib -- --list                      # lib = 모듈 경로 전체
store::pins::tests::log_witness::w_log_a_no_vanish_stream_is_identical: test
store::reconcile::tests::corrupt_blob_quarantined: test
store::reconcile::tests::old_temp_deleted_recent_preserved: test

$ cargo test --test e2e -- --list                 # 통합 크레이트 **최상위 함수 = `::` 가 없다**
large_object_streaming_put_and_range_download: test
public_listener_isolates_api_and_internal_buckets: test

2 tests, 0 benchmarks

$ cargo test --test reconcile_vanishing_entries -- --list
phase_e_entry_loop_survives_vanishing_entries: test
phase_t_temp_deletion_counts_only_what_we_deleted: test

2 tests, 0 benchmarks
# ⚠ phase_g(W13-G)는 **lib**로 옮겼다(훅 park — `Store::with_hooks`가 crate-private) ⇒
#   `store::pins::tests::recover_graves_production_seam::phase_g_recover_graves_survives_vanishing_graves`

$ cargo test --test layout_tree -- --list
on_disk_layout_golden_tree: test
put_stream_midflight_temp_observed_and_preserved: test
symlinked_commit_pointer_current_behavior: test

3 tests, 0 benchmarks
```

**통합 테스트 *안의 중첩 모듈*은?** — **모듈-상대 경로**로 나온다(크레이트/타깃 이름은 **붙지 않는다**):

```
$ cargo test --test reconcile_vanishing_entries -- --list     # 프로브: mod nested_probe { #[test] fn … }
nested_probe::nested_in_an_integration_crate: test
phase_e_entry_loop_survives_vanishing_entries: test
```

⇒ **세 형태가 공존한다**: `a::b::<id>`(lib 중첩) · `<id>`(통합 최상위) · `<mod>::<id>`(통합 중첩).
⇒ **앵커는 `(^|::)<id>: test$` 여야 한다.** (⚠ 저장소의 **기존** 테스트도 22개가 `::` 없는 최상위다 —
`put_stream_midflight_temp_observed_and_preserved`(P8이 핀한다) · `symlinked_commit_pointer_current_behavior`
(P12) · `on_disk_layout_golden_tree`(P10) 를 포함한다 ⇒ **옛 앵커는 그것들도 못 찾는다**.)

⚠ **타깃 경계는 stdout에 없다** — `cargo test --lib --tests -- --list`를 한 번에 리다이렉트하면 **타깃별
`Running …` 줄은 stderr**로 가고 stdout에는 ID만 이어 붙는다 ⇒ **어느 ID가 어느 바이너리에서 왔는지 사라진다.**
그래서 게이트는 **타깃별로 따로 묻는다**(= Codex의 *"타깃을 아는 발견 스크립트"*) — 그러면 매칭이
**타깃 한정**이 되어 부분문자열 충돌도, 타깃 오배치도 함께 잡힌다.

**0-b. 스크립트 — 증인 ID의 유일한 정본**

> ⚠⚠ **목록은 한 곳에만 산다.** 셸과 크레이트 양쪽에 두면 **반드시 어긋난다**(Codex r22의 simpler
> alternative) ⇒ **구 W-REG는 폐기한다**(근거는 §0-d). 계획의 **증인 ID 표(0-c)는 이 스크립트의 거울**이며,
> 증인을 개명하면 **둘을 같은 커밋에서** 고쳐야 한다(**B-5 diff 항목**).
>
> ⚠⚠⚠ **SSOT — 게이트 명세의 유일한 규범 정의 (★r25/P-40)**: **아래 스크립트**와 **§0-h의 술어×케이스
> 매트릭스**가 정본이다. **케이스 수 · 술어 목록 · 픽스처 목록 · 레지스트리 행 수는 이 두 곳에만 산다** ⇒
> **다른 모든 곳**(B-1 증분 표 · B-1 acceptance · B-5 · §5 · 뮤턴트 표)**은 숫자를 반복하지 않고
> 상호참조한다.** **숫자를 맞추는 것이 아니라 중복을 없애는 것이 봉인이다**(근거·이력 = RDL r25/P-40).

```bash
#!/usr/bin/env bash
# F-14 증인 게이트 — 증인 레지스트리의 **단일 권위**.
#   ① 발견 — 타깃별 `--list`를 **파일로 캐시** · **cargo 종료 상태 검사** · **캐시 파일에 직접** grep.
#   ② 결과 — 전 스위트를 돌려 `test result:` 줄에서 **숫자를 뽑아 정수 0과 비교**한다.
#   --selftest — **술어 하나당 케이스 하나**(직교) · **모든 술어에 "지우면 RED"를 실증**했다 (§0-h).
#
# ⚠⚠ **발견에 파이프라인 금지 (r24/P-38)**: `list_for | grep -qE` 는 `pipefail` 아래에서 **성공한 매치를
#     실패로 뒤집는다** — `grep -q` 가 첫 매치에 종료하면 상류 `cat` 이 **SIGPIPE**를 맞아 파이프라인이
#     **141**을 낸다 ⇒ **거짓 `MISSING WITNESS`**(실측: 첫 줄 매치 · 1.1 MB → **20/20** · 57 KB → **4/30**).
# ⚠  `cargo … --list` 의 **종료 상태를 본다** — 빌드가 깨지면 목록이 비고, 그것을 grep 하면 증인 전부가
#     `MISSING WITNESS` 로 나온다 = **오진**(진짜 원인은 빌드 실패다 — 실측).
# ⚠⚠ **부분문자열 매칭 금지 (r23/P-37)**: `grep -vc '0 ignored'` 는 `10 ignored` 를 통과시킨다.
# ⚠  exit code 하나만 믿지 마라 — cargo 는 ignored 가 있어도 **0으로 끝난다**(실측). 둘 다 본다.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

OS="$(uname -s)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

# ── 정본 레지스트리 —  "<target>|<id>|<platform>" ────────────────────────────
#   target   : lib | <통합 테스트 바이너리 이름>      platform : all | unix | linux
WITNESSES=$(cat <<'REG'
lib|reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot|all
lib|reconcile_pass_control_without_vanishing_entries_is_green|all
lib|reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot|all
lib|reconcile_pass_control_without_a_vanishing_temp_is_green|all
lib|objects_container_destroyed_mid_pass_still_fails_the_pass_and_publishes_nothing|all
lib|container_destroyed_at_the_grave_rename_fails_the_pass_without_publishing_or_resurrecting|all
lib|container_destroyed_then_recreated_at_the_grave_rename_completes_with_empty_stats|all
lib|temp_only_container_destruction_still_fails_the_pass_and_publishes_nothing|all
lib|container_guard_fires_after_the_loop_runs_to_completion|all
lib|tail_destruction_without_any_vanished_entry_stays_ok_like_today|all
lib|grave_source_vanished_during_park_lets_the_pass_finish|all
lib|recover_graves_production_seam_survives_vanished_graves|all
lib|phase_g_recover_graves_survives_vanishing_graves|all
lib|seen_absorbs_only_confirmed_absence|all
lib|every_fs_method_reports_gone_after_the_entry_vanishes|all
lib|rename_with_absent_source_is_source_gone_and_counted|all
lib|rename_with_missing_destination_propagates_raw_notfound|all
lib|rename_ok_then_fsync_failure_propagates_raw|all
lib|w_log_a_no_vanish_stream_is_identical|all
lib|w_log_b_downstream_events_fire_after_the_pass_survives|all
lib|w_log_c_skip_path_emits_no_event_at_any_level|all
lib|w_log_d_every_reachable_skip_arm_is_silent|all
lib|a_dangling_blob_symlink_still_aborts_the_pass_exactly_like_today|unix
lib|grave_rename_ok_then_fsync_eacces_propagates_raw|unix
lib|rename_with_dangling_source_symlink_is_done|unix
lib|absence_probe_eacces_is_not_absence|unix
e2e|dangling_temp_symlink_keeps_lstat_semantics|unix
e2e|blob_symlink_to_directory_propagates_isadirectory|unix
e2e|corrupt_dir_as_regular_file_propagates_enotdir|unix
e2e|corrupt_dir_as_dangling_symlink_propagates_raw_notfound|unix
e2e|symlinked_objects_dir_with_a_vanished_entry_completes|unix
e2e|symlinked_objects_dir_without_vanishing_is_unchanged|unix
e2e|non_utf8_temp_name_is_stat_and_unlinked_by_raw_bytes|linux
reconcile_vanishing_entries|phase_e_entry_loop_survives_vanishing_entries|all
reconcile_vanishing_entries|phase_t_temp_deletion_counts_only_what_we_deleted|all
REG
)

# ── ① 발견 — **캐시 파일 · 종료 상태 검사 · 파이프 없음** (P-38) ─────────────
list_file() {                                  # $1 = target → 목록 **파일 경로**를 stdout에. 실패 = rc 2
  local t="$1" f="$TMP/list.$1" rc=0
  if [ ! -f "$TMP/ok.$1" ]; then
    if [ "$t" = "lib" ]; then cargo test --lib        -- --list > "$f" 2> "$TMP/err.$1"
    else                      cargo test --test "$t"  -- --list > "$f" 2> "$TMP/err.$1"
    fi
    rc=$?                                      # ⚠ **리스트 자체의 종료 상태**(빌드 실패 ≠ 증인 부재)
    [ "$rc" -ne 0 ] && { printf '%s\n' "$rc" > "$TMP/rc.$1"; return 2; }
    : > "$TMP/ok.$1"
  fi
  printf '%s\n' "$f"                           # ← 파이프가 아니라 **경로**를 넘긴다
}

has_witness() { grep -qE "(^|::)${2}: test\$" "$1"; }   # $1 = 목록 파일 · $2 = id  ⇒ **파이프 없음**

required() {                                   # $1 = platform
  case "$1" in
    all)   return 0 ;;
    unix)  case "$OS" in Darwin|Linux|FreeBSD) return 0 ;; esac ;;
    linux) [ "$OS" = "Linux" ] && return 0 ;;
  esac
  return 1
}

discover() {                                   # $1 = 목록-해결자 함수명 · stdin = 레지스트리 ⇒ 0/1
  local resolve="$1" bad=0 target id platform f
  while IFS='|' read -r target id platform; do
    [ -z "${target:-}" ] && continue
    if ! required "$platform"; then
      echo "   skip  [$target] $id   (platform=$platform · OS=$OS)"; continue
    fi
    if ! f="$("$resolve" "$target")"; then     # ← PRED-LIST-RC: 목록 명령이 죽으면 오진하지 않는다
      echo "   LIST FAILED  [$target]  cargo --list exit=$(cat "$TMP/rc.$target" 2>/dev/null)"
      echo "                (빌드 실패다. '증인 없음'이 아니다 — 오진 금지)"
      bad=1; continue
    fi
    if has_witness "$f" "$id"; then            # ← PRED-DISC
      echo "   ok    [$target] $id"
    else
      echo "   MISSING WITNESS  [$target] $id"; bad=1
    fi
  done
  return "$bad"
}

# ── ② 파서 — **숫자를 뽑아 정수 비교한다** ───────────────────────────────────
#   원문:  test result: ok. 132 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.83s
tally() {                                      # $1 = 스위트 출력 → "<결과줄수> <passed> <failed> <ignored>"
  awk '/^test result:/ {
         n++
         for (i = 1; i < NF; i++) {
           if ($(i+1) ~ /^passed;?$/)  p += $i + 0
           if ($(i+1) ~ /^failed;?$/)  f += $i + 0
           if ($(i+1) ~ /^ignored;?$/) g += $i + 0
         }
       }
       END { printf "%d %d %d %d\n", n+0, p+0, f+0, g+0 }' "$1"
}

verdict() {                                    # $1 = 출력 파일 · $2 = cargo exit  →  0 PASS / 1 FAIL
  local n p f g bad=0
  read -r n p f g < <(tally "$1")
  echo "   결과 줄 ${n}개 · passed=${p} · failed=${f} · ignored=${g} · cargo exit=${2}"
  if [ "$n" -eq 0 ]; then echo "   FAIL: 'test result:' 줄이 0개 — 스위트가 돌지 않았다"; bad=1; fi
  if [ "$g" -ne 0 ]; then echo "   FAIL: ignored=${g} (≠0) — 스킵된 red = 위조된 red (하드룰 9)"; bad=1; fi
  if [ "$f" -ne 0 ]; then echo "   FAIL: failed=${f} (≠0)"; bad=1; fi
  if [ "$2" -ne 0 ]; then echo "   FAIL: cargo exit=${2} (≠0)"; bad=1; fi
  return "$bad"
}

old_parser() {                                 # r22/r23 판본 재현 — **회귀 핀**(P-37). 0 PASS / 1 FAIL
  local bad; bad=$(grep 'test result:' "$1" | grep -vc '0 ignored')
  [ "$bad" -eq 0 ]
}

# ── --selftest — **술어 하나당 케이스 하나**(직교 · r24/P-39) ────────────────
#   ⚠⚠ 픽스처 = **(출력, rc) 쌍**이다. cargo 종료코드를 **합성 파라미터**로 분리하지 않으면
#      (d)·(f)가 **rc≠0으로도** 실패해 `failed`·결과줄수 술어가 **핀되지 않는다**(P-39 · 실측).
selftest() {
  local rc=0 n_cases=0 n_ok=0        # ⚠ **케이스 수는 세지 않고 *센다*** — 아래 res/dis 호출이 정본이다(P-40)
  local FX_0="test result: ok. 132 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.83s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s"
  local FX_1="test result: ok. 131 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.78s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s"
  local FX_10="test result: ok. 122 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 1.75s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s"
  local FX_F10="test result: FAILED. 122 passed; 10 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.90s"
  local FX_NONE="error: could not compile \`files\` (lib test) due to 1 previous error"

  res() {              # 결과 케이스. $1 이름 · $2 출력 · $3 **합성 cargo rc** · $4 기대 · $5 옛-파서 기대
    n_cases=$((n_cases + 1)); local ok=1
    printf '%s\n' "$2" > "$TMP/fx"
    verdict "$TMP/fx" "$3" > "$TMP/out" 2>&1; local st=$?
    local got=PASS; [ "$st" -ne 0 ] && got=FAIL
    local mark="ok  "; [ "$got" != "$4" ] && { mark="FAIL"; rc=1; ok=0; }
    printf '   [%s] %-20s rc=%-3s  게이트=%-4s (기대 %s)' "$mark" "$1" "$3" "$got" "$4"
    if [ -n "${5:-}" ]; then
      local ogot=PASS; old_parser "$TMP/fx" || ogot=FAIL
      local omark="ok  "; [ "$ogot" != "$5" ] && { omark="FAIL"; rc=1; ok=0; }
      printf '  · [%s] 옛 파서=%-4s (기대 %s)' "$omark" "$ogot" "$5"
    fi
    printf '\n'
    [ "$ok" -eq 1 ] && n_ok=$((n_ok + 1))
    return 0
  }

  FX_LIST_RC=0
  fx_list() {          # 목록-해결자의 **테스트 대역**(cargo 미호출). $1 = target
    [ "$FX_LIST_RC" -ne 0 ] && { printf '%s\n' "$FX_LIST_RC" > "$TMP/rc.$1"; return 2; }
    printf '%s\n' "$TMP/fxlist.$1"
  }
  dis() {              # 발견 케이스. $1 이름 · $2 레지스트리 1행 · $3 기대
    n_cases=$((n_cases + 1))
    discover fx_list <<< "$2" > "$TMP/dout" 2>&1; local st=$?
    local got=PASS; [ "$st" -ne 0 ] && got=FAIL
    local mark="ok  "; if [ "$got" != "$3" ]; then mark="FAIL"; rc=1; else n_ok=$((n_ok + 1)); fi
    printf '   [%s] %-20s          발견=%-4s (기대 %s)\n' "$mark" "$1" "$got" "$3"
  }

  echo "== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) =="
  echo "-- ② 결과 게이트 --"
  res "(a) 1 ignored"      "$FX_1"    0   FAIL FAIL   # → PRED-IGN
  res "(b) 10 ignored"     "$FX_10"   0   FAIL PASS   # → PRED-IGN  + 옛-파서 회귀 핀(P-37)
  res "(c) 전부 정상"      "$FX_0"    0   PASS PASS   # → 대조군: 어떤 술어도 발화하지 않는다
  res "(d) 10 failed"      "$FX_F10"  0   FAIL PASS   # → PRED-FAIL ⚠ **rc=0**(r23은 101 — P-39)
  res "(e) cargo rc!=0"    "$FX_0"    101 FAIL        # → PRED-RC   nonzero-exit **전용** 증인
  res "(f) 결과 줄 0개"    "$FX_NONE" 0   FAIL        # → PRED-N0   ⚠ **rc=0**(r23은 101 — P-39)

  echo "-- ① 발견 게이트 --"
  printf 'store::pins::tests::log_witness::w_log_a_no_vanish_stream_is_identical: test\n' > "$TMP/fxlist.lib"
  dis "(g) 증인 누락"      "lib|w_log_d_every_reachable_skip_arm_is_silent|all"  FAIL   # → PRED-DISC
  { printf 'store::pins::tests::log_witness::w_log_a_no_vanish_stream_is_identical: test\n'
    awk 'BEGIN{for(i=0;i<20000;i++) printf "store::pins::tests::filler::padding_%06d: test\n", i}'
  } > "$TMP/fxlist.big"                              # 1.1 MB · **첫 줄이 매치** ⇒ SIGPIPE 무대
  dis "(h) 조기매치+큰목록" "big|w_log_a_no_vanish_stream_is_identical|all"        PASS   # → M-SIGPIPE 킬러
  FX_LIST_RC=101
  dis "(i) 목록 rc!=0"     "lib|w_log_a_no_vanish_stream_is_identical|all"         FAIL   # → PRED-LIST-RC
  FX_LIST_RC=0

  echo
  # ⚠ **숫자를 박지 않는다** — 위 res/dis 호출에서 **센다**(케이스를 더하면 분모가 저절로 는다 · P-40).
  if [ "$rc" -eq 0 ]; then echo "SELFTEST: PASS  (${n_ok}/${n_cases} · 케이스·술어의 정본 = §0-h 매트릭스)"
  else                     echo "SELFTEST: FAIL  (${n_ok}/${n_cases})"; fi
  return "$rc"
}

[ "${1:-}" = "--selftest" ] && { selftest; exit $?; }

echo "== ① 발견 단언  (타깃별 --list → **캐시 파일** · 종료상태 검사 · 앵커 = (^|::)<id>: test\$) =="
if ! discover list_file <<< "$WITNESSES"; then
  echo; echo "DISCOVERY FAILED — 선언된 증인이 그 타깃의 바이너리에 없다(또는 목록 명령이 죽었다)."
  echo "  원인: mod 등록 누락(M-NOMOD) · 파일 미작성(M-NOMOD') · 개명 · cfg 축출 · **빌드 실패**."
  exit 1
fi
echo "   -> DISCOVERY OK"

echo
echo "== ② 결과 게이트  (전 스위트 실행 · 숫자 파싱 · ignored/failed/결과줄수/exit) =="
cargo test --tests > "$TMP/suite.txt" 2>&1     # ⚠ --tests 는 lib·bins·통합을 **전부** 포함한다
suite_rc=$?
grep '^test result:' "$TMP/suite.txt" | sed 's/^/   /'
if ! verdict "$TMP/suite.txt" "$suite_rc"; then
  echo; echo "RESULT GATE FAILED — 실행 결과의 **숫자**를 판다(소스 grep이 아니다)"
  echo "  ⇒ #[ignore] · #[cfg_attr(…, ignore)] · 매크로 판본을 표기와 무관하게 전부 잡는다."
  exit 1
fi
echo "   -> 0 ignored · 0 failed · 스위트 GREEN"
echo; echo "F-14 WITNESS GATE: PASS"
```

⚠⚠ **파서가 게이트다 — 그리고 파서가 결함의 자리였다**(P-37). **부분문자열 매칭은 쓰지 않는다**:
`grep -vc '0 ignored'`는 `10 ignored`를, `grep -c '0 failed'`는 `10 failed`를 **통과시킨다**(실측 — §0-d).
⇒ 필드를 **토큰으로 쪼개 숫자를 뽑고 정수 0과 비교**한다. `; 0 ignored;`처럼 **구분자를 포함한 고정
문자열**로 매칭하는 것도 가능하지만(구분자 = `; ` — §0-d의 원문 참조), **숫자 비교가 더 강하다**:
`ignored`가 **여러 결과 줄에 흩어져 있어도 합산**되고, **결과 줄이 0개인 경우**(컴파일 실패 · 타깃이 통째로
빠진 경우)도 같은 판정 함수가 잡는다.
⚠ **exit code에만 의존하지 않는다** — cargo는 **ignored가 있어도 0으로 끝난다**(실측 §0-d). 그러나
**exit code도 함께 본다**(FAILED 줄이 없는 실패 · 링크 에러 · 패닉으로 죽은 하네스).
⚠ **`--tests`는 lib 타깃을 포함한다**(실측: `Running unittests src/lib.rs` · `src/main.rs` 가 둘 다 뜬다)
⇒ **전 스위트가 한 번에 덮인다**(프로토타입 = 결과 줄 **9개** = lib · bins · adversarial · contract · e2e ·
layout_tree · openapi · regression_… · reconcile_vanishing_entries).

**0-c. 증인 ID 표 (정본의 거울 — *타깃별*).** *확정* = 프로토타입에서 **실제로 컴파일·실행된 이름** ·
*규범* = **구현 시 이 이름으로 만든다**.
**`--list` 형태**: **lib** = `<모듈경로>::<id>: test` · **통합(최상위)** = `<id>: test` (**`::` 없음**).

| 증인 | **cargo 타깃** | 파일 (모듈) | **테스트 함수명 = ID** | 이름 | 플랫폼 |
|---|---|---|---|---|---|
| **회귀 ①**(Blob · `flips[]`) | **lib** | `pins/tests/vanished_entry_regression.rs` | `reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot` | **확정**(락) | all |
| **대조군 ①** | **lib** | 〃 | `reconcile_pass_control_without_vanishing_entries_is_green` | **확정**(red.sha) | all |
| **회귀 ②**(Temp · `flips[]`) | **lib** | `pins/tests/vanished_temp_regression.rs` | `reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot` | **확정**(락) | all |
| **대조군 ②** | **lib** | 〃 | `reconcile_pass_control_without_a_vanishing_temp_is_green` | **확정**(red.sha) | all |
| **W10** | **lib** | `pins/tests/vanished_container_witnesses.rs` | `objects_container_destroyed_mid_pass_still_fails_the_pass_and_publishes_nothing` | **확정**(proto) | all |
| **W-GRAVE-CD-A** | **lib** | 〃 | `container_destroyed_at_the_grave_rename_fails_the_pass_without_publishing_or_resurrecting` | **확정**(proto) | all |
| **W-GRAVE-CD-B** | **lib** | 〃 | `container_destroyed_then_recreated_at_the_grave_rename_completes_with_empty_stats` | **규범** | all |
| **W10-TEMP** | **lib** | 〃 | `temp_only_container_destruction_still_fails_the_pass_and_publishes_nothing` | **규범** | all |
| **W10-G** | **lib** | 〃 | `container_guard_fires_after_the_loop_runs_to_completion` | **규범** | all |
| **W10b** *(차단 요건)* | **lib** | 〃 | `tail_destruction_without_any_vanished_entry_stays_ok_like_today` | **규범** | all |
| **W6** | **lib** | 〃 | `grave_source_vanished_during_park_lets_the_pass_finish` | **규범** | all |
| **W3** | **lib** | 〃 (⚠ §3 표는 *e2e*라 적었으나 **프로토타입이 실행한 소스는 `pins/tests`다** ⇒ **여기가 정본**) | `a_dangling_blob_symlink_still_aborts_the_pass_exactly_like_today` | **확정**(proto) | unix |
| **W6b** | **lib** | 〃 | `grave_rename_ok_then_fsync_eacces_propagates_raw` | **규범** | unix · root 프로브 |
| **W11** | **lib** | `pins/tests/recover_graves_production_seam.rs` | `recover_graves_production_seam_survives_vanished_graves` | **규범** | all |
| **W13-G** *(★재작성 · lib 이전)* | **lib** | 〃 `pins/tests/recover_graves_production_seam.rs` | `phase_g_recover_graves_survives_vanishing_graves` | **규범** | all |
| **W-LOG-A** | **lib** | `pins/tests/log_witness.rs` | `w_log_a_no_vanish_stream_is_identical` | **확정**(proto) | all |
| **W-LOG-B** | **lib** | 〃 | `w_log_b_downstream_events_fire_after_the_pass_survives` | **확정**(proto) | all |
| **W-LOG-C** | **lib** | 〃 | `w_log_c_skip_path_emits_no_event_at_any_level` | **확정**(proto) | all |
| **W-LOG-D** *(차단 요건)* | **lib** | 〃 | `w_log_d_every_reachable_skip_arm_is_silent` | **확정**(★r26 — 개명 · 무대 6개로 전수화) | all |
| **W1** | **lib** | `reconcile/entry.rs` 인라인 `mod tests` | `seen_absorbs_only_confirmed_absence` | **규범** | all |
| **W2** | **lib** | 〃 | `every_fs_method_reports_gone_after_the_entry_vanishes` | **규범** | all |
| **W5a** | **lib** | `reconcile/absence.rs` 인라인 `mod tests` | `rename_with_absent_source_is_source_gone_and_counted` | **규범** | all |
| **W5b** | **lib** | 〃 | `rename_with_missing_destination_propagates_raw_notfound` | **규범** | all |
| **W5c** | **lib** | 〃 | `rename_ok_then_fsync_failure_propagates_raw` | **규범** | all |
| **W5d** | **lib** | 〃 | `rename_with_dangling_source_symlink_is_done` | **규범** | unix |
| **W5e′** | **lib** | 〃 | `absence_probe_eacces_is_not_absence` | **규범** | unix · root 프로브 |
| **W4** | **`e2e`** | `tests/e2e.rs` (**최상위**) | `dangling_temp_symlink_keeps_lstat_semantics` | **규범** | unix |
| **W7** | **`e2e`** | 〃 | `blob_symlink_to_directory_propagates_isadirectory` | **규범** | unix |
| **W9a** | **`e2e`** | 〃 | `corrupt_dir_as_regular_file_propagates_enotdir` | **규범** | unix |
| **W9b** | **`e2e`** | 〃 | `corrupt_dir_as_dangling_symlink_propagates_raw_notfound` | **규범** | unix |
| **W10c** | **`e2e`** | 〃 | `symlinked_objects_dir_with_a_vanished_entry_completes` | **규범** | unix |
| **W10c′** | **`e2e`** | 〃 | `symlinked_objects_dir_without_vanishing_is_unchanged` | **규범** | unix |
| **W17** | **`e2e`** | 〃 | `non_utf8_temp_name_is_stat_and_unlinked_by_raw_bytes` | **규범** | **linux 전용** |
| **W13-E** | **`reconcile_vanishing_entries`** | `tests/reconcile_vanishing_entries.rs` (**최상위**) | `phase_e_entry_loop_survives_vanishing_entries` | **규범** | all |
| **W13-T** | 〃 | 〃 | `phase_t_temp_deletion_counts_only_what_we_deleted` | **규범** | all |

> ⚠ **W13-G는 이 표의 위쪽(W11 아래)으로 이동했다** — 무덤 복구는 결정적 훅 park로만 신뢰성 있게
> 조율되고, 훅을 심는 `Store::with_hooks`가 `#[cfg(test)]` crate-private이라 통합 바이너리에서 닿지
> 못한다(구 통합 무대는 동시성 랑데부라 green.sha에서 5/5 RED였다 — 하이젠버그). ⇒ **lib**로 옮기고
> 게이트 레지스트리·이 표를 **같은 커밋에서** 갱신했다(B-5).

⚠⚠ **아래 9개가 크레이트 최상위 통합 증인이다** — **r22의 `::<id>` 앵커가 죽이던 바로 그것들**:
`e2e`의 **W4 · W7 · W9a · W9b · W10c · W10c′ · W17** + `reconcile_vanishing_entries`의 **W13-E/T**
(W13-G는 lib로 이전 — 위 참조).

⚠ **플랫폼 분할은 여전히 load-bearing이다** — **`#[cfg(target_os = "linux")]`가 걸린 테스트는 macOS의
`--list`에 *나오지 않는다***(실측). 분할하지 않으면 개발기에서 **거짓 RED**가 나고, 그것을 잠재우려고
목록을 깎는 순간 **단언 자체가 약화**된다(W17이 정확히 그 경우다 — B-12).

**0-d. 실증 — 두 결함이 실제로 봉인됐는가** (프로토타입 · 원문)

**(P-35) 옛 앵커는 올바른 통합 증인을 거짓 MISSING으로 죽인다**

```
########## 옛 앵커 (r22 0단계 그대로):  grep -q "::${id}: test$" ##########
  OK      : w_log_a_no_vanish_stream_is_identical                 ← lib(중첩) — 통과
  MISSING WITNESS: phase_e_entry_loop_survives_vanishing_entries  ← 통합(최상위) — **거짓 RED**
  ==> exit 1

########## 새 앵커:  grep -qE "(^|::)${id}: test$" ##########
  OK      : w_log_a_no_vanish_stream_is_identical
  OK      : phase_e_entry_loop_survives_vanishing_entries
  ==> exit 0
```

**(P-36 · ★P-37) 결과 줄의 *원문*과 구분자 — 파서는 이것을 판다**

```
$ cargo test --tests | grep '^test result:'          # ⚠ 결과 줄은 **행 선두**에서 시작한다
test result: ok. 132 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.83s

--- 구분자 (`;` 를 [;] 로 치환해 눈으로 확인) ---
test result: ok. 132 passed[;] 0 failed[;] 0 ignored[;] 0 measured[;] 0 filtered out[;] finished in 1.83s<EOL>
  ⇒ 필드 구분자 = **`; `**(세미콜론+공백) · 각 필드 = **`<수> <이름>`** · 마지막 필드만 `finished in …`
  ⇒ 구분자 매칭이라면 **`; 0 ignored;`** 여야 한다. 우리는 **숫자를 뽑아 정수 비교**한다(더 강하다).
```

**★ 부분문자열 함정 — 실측(합성 줄이 아니라 grep의 실제 판정)**

```
$ printf 'test result: ok. 122 passed; 0 failed; 10 ignored; …\n' | grep -c '0 ignored'   → 1  ← 매치한다!
$ printf 'test result: FAILED. 122 passed; 10 failed; 0 ignored; …\n' | grep -c '0 failed' → 1  ← 매치한다!
  ⇒ `grep -vc '0 ignored'` 는 **10 · 20 · 100 ignored 를 전부 "위반 아님"으로 센다.**
```

**★ 실증 (a)(b)(c) — 프로토타입에 실제 `#[ignore]`를 주입해 게이트를 돌린 원문** (macOS/APFS · 2026-07-14)

> 프로토타입에 **구현된 증인은 13개**(lib 10 · `reconcile_vanishing_entries` 3)다 ⇒ **정본 레지스트리 전행**
> (§0-b)을 그대로 돌리면 ①에서 죽어 ②에 닿지 못한다 ⇒ **레지스트리를 그 13행으로 좁힌 사본**으로 ②를 실증했다.
> **10개를 무엇으로 채웠는가**: **lib에 구현된 증인 전부** = **회귀 ①·② + 대조군 ①·②**(= `flips[]`의
> **차단 증인**) **+ W10 + W-GRAVE-CD-A + W3 + W-LOG-A/B/C**. **정확히 10개다.**

```
################ (c) 0 ignored — 정상 ################
   test result: ok. 132 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.84s
   … (결과 줄 9개) …
   결과 줄 9개 · passed=155 · failed=0 · ignored=0 · cargo exit=0
   -> 0 ignored · 0 failed · 스위트 GREEN
F-14 WITNESS GATE: PASS
   ==> 게이트 exit = 0                              ← **통과**

################ (a) 1 ignored — M-IGNORE-1 (W10 에 #[ignore] 한 줄) ################
   주입된 #[ignore] = 1개
   -> DISCOVERY OK                                  ← `--list`에 그대로 나온다 ⇒ 발견 단언은 못 잡는다
   test result: ok. 131 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.88s
   결과 줄 9개 · passed=154 · failed=0 · ignored=1 · cargo exit=0    ← **cargo는 0으로 끝난다**
   FAIL: ignored=1 (≠0) — 스킵된 red = 위조된 red (하드룰 9)
RESULT GATE FAILED
   ==> 게이트 exit = 1                              ← **죽는다**

################ (b) 정확히 10 ignored — M-IGNORE-10 (lib 증인 10개 전부) ################
   주입된 #[ignore] = 10개
   -> DISCOVERY OK
   test result: ok. 122 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 1.66s
   결과 줄 9개 · passed=145 · failed=0 · ignored=10 · cargo exit=0
   FAIL: ignored=10 (≠0) — 스킵된 red = 위조된 red (하드룰 9)
RESULT GATE FAILED
   ==> 새 게이트 exit = 1                           ← **죽는다**

   --- ⚠⚠ 같은 출력에 **옛 파서**(r22/r23: grep -vc '0 ignored')를 돌리면? ---
     test result: ok. 122 passed; 0 failed; 10 ignored; …          ← `10 ignored` ⊃ `0 ignored`
     옛 파서: bad(위반 결과 줄) = 0   · cargo exit = 0
     ==> 옛 게이트 = **PASS (exit 0)**  ← **증인 10개를 전부 재갈 물렸는데 위반 0을 보고했다** (P-37)

################ 원복 후 ################
test result: ok. 132 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.84s
```

⇒ **(b)가 P-37의 증거다.** 옛 파서는 **1개는 잡고 10개는 놓친다** — *"0이 아닌 모든 ignored를 잡는다"*가
거짓이었다. **차단 증인(회귀 ①·②)까지 포함해 전부 침묵시켜도 게이트는 초록이었다.**

**(d) `failed` 필드에도 같은 함정이 있었는가 — 전수 확인 결과: *파싱 자체가 없었다***

r22/r23의 ②는 `failed`를 **한 번도 파싱하지 않았다** — `suite_rc`(cargo exit)에만 맡겼다.
⇒ **부분문자열 버그가 *있지는* 않았다.** 그러나 **그 필드가 무방비였다**: 누가 대칭성을 맞추려고
`grep -vc '0 failed'`를 넣는 순간 **`10 failed`가 통과한다**(위 실측). ⇒ 새 파서는 **`failed`도 숫자로**
검사한다(**+ exit code도 함께** — 둘 다). `--selftest`의 (d)가 이 회귀를 **핀으로 박는다**.

**(e) ① 발견 단언의 앵커에도 같은 부분문자열 함정이 있는가 — 실제 ID로 실증. 답: *없다*.**

앵커 `(^|::)<id>: test$`는 **접두사 `(^|::)` + 접미사 `: test$`로 양끝이 막혀 있다** ⇒ **세그먼트 전체
일치**다. 프로토타입에 **접두사 충돌 ID를 실제로 심어** `--list`를 다시 받아 검증했다(`w_log_a` ⊂
`w_log_ab` ⊂ `w_log_a_no_vanish_stream_is_identical`):

```
--- --list 원문 (프로브 주입 후) ---
store::pins::tests::log_witness::w_log_a: test
store::pins::tests::log_witness::w_log_a_no_vanish_stream_is_identical: test
store::pins::tests::log_witness::w_log_ab: test

--- 결정적 실험: **짧은 증인 `w_log_a`가 삭제되고 `w_log_ab`만 남으면?** ---
  id=w_log_a        →  MISSING WITNESS       ← **접미사 앵커 `: test$`가 막았다**(거짓 양성 0)
--- 접두 절단 · 접미사 조각 ---
  id=w_log_         →  no match     id=log_a →  no match     id=no_vanish_stream_is_identical →  no match
```

⇒ **`: test$`가 접두사 충돌을, `(^|::)`가 접미사 충돌을 막는다.** 추가로 **정본 레지스트리(§0-b `WITNESSES`)의
*전 행*에 접두사 충돌 쌍이 0개**이고 **ID 중복도 0개**다(전수 스캔). ⇒ 앵커는 **P-37과 같은 클래스의 결함이
없다.** ⚠ **행 수는 여기 적지 않는다 — 레지스트리의 행 집합은 §0-b가 정본이다**(★r25/P-40) ⇒ **증인을 더할
때마다 이 두 성질(충돌 0 · 중복 0)을 다시 확인한다.**

**(f) ★ P-38 — `pipefail` + `grep -q` ⇒ SIGPIPE 141 ⇒ *거짓* `MISSING WITNESS`** (r24 · 실측 원문)

r23의 발견 검사는 **`list_for "$target" | grep -qE …`**(`list_for` 말미 = `cat "$f"`)이고 1행은 **`set -uo pipefail`**이다. **`grep -q`는 첫 매치에서 즉시 종료**하고 상류 `cat`은 **SIGPIPE**를 맞아 죽는다 ⇒ 파이프라인 = **141**(=128+13) ⇒ **`MISSING WITNESS`**. **증인은 거기 있는데 게이트가 없다고 말한다.**

```
--- r23의 발견 검사 형태 그대로 (첫 줄이 매치 · 뒤에 충분한 출력) ---
  small.txt       5777 bytes   파이프라인 rc=0    ok    (증인 발견)
  medium.txt     57077 bytes   파이프라인 rc=141  MISSING WITNESS   ← **거짓 RED**
  big.txt      1140077 bytes   파이프라인 rc=141  MISSING WITNESS   ← **거짓 RED**
  (bash 3.2 · bash 5.3 **동일** ⇒ 셸 판본 문제가 아니다 · `kill -l 13` = PIPE ⇒ 141 = 128+13)
--- **비결정성** — 매 시행마다 새 셸 ---
  57 077 bytes    →  rc=141 :  4/30   ← ⚠ **같은 입력이 어떤 때는 통과하고 어떤 때는 죽는다**
  1 140 077 bytes →  rc=141 : 10/10   ← 파이프 용량을 확실히 넘으면 **항상** 죽는다
--- **매치 위치**가 결정한다 (같은 1.1 MB 목록) ---
  매치 = 첫 줄     →  rc=141  20/20   ← grep이 일찍 나가니 cat이 SIGPIPE
  매치 = 마지막 줄 →  rc=141   0/20   ← grep이 끝까지 읽으니 cat도 끝까지 쓴다
--- **덤: `--list`의 종료 상태**(r23은 rc를 안 봤다) — 빌드를 깬 트리 ---
  [r23]    MISSING WITNESS [lib] w_log_a… / w_log_b… → DISCOVERY FAILED
           "원인: mod 등록 누락 · 파일 미작성 · 개명 · cfg 축출"                    ← **전부 오답**
  [정정본] LIST FAILED [lib] cargo --list exit=101 — "빌드 실패다. '증인 없음'이 아니다"  ← **정답**
```

⚠ **정직 — 오늘은 발화하지 않는다**: 실제 `--list` = **8 561 bytes**(134줄) ⇒ 파이프 용량 아래 ⇒ **0/30. 잠복
이다.** 그러나 ⑴ 계획은 증인을 **24개 더 이식**하고 ⑵ 파이프 용량은 **플랫폼·부하 의존**이며 ⑶ 발화하면
**비결정적 거짓 RED**다 ⇒ **방어적으로 봉인한다**(플랫폼 차이로 이미 여러 번 덴 자리다).
**봉인**: `list_file()`이 목록을 **파일에 캐시**해 **경로**를 넘기고 `has_witness()`가 **그 파일을 직접 `grep
-qE`** 한다 ⇒ **파이프라인 0개** ⇒ SIGPIPE **구조적으로 불가능**. **`--list`의 rc≠0은 `LIST FAILED`**로 낸다.


**(M-NOMOD′) 선언만 하고 안 만든 증인은 머지될 수 없다** — **정본 레지스트리 전행**(§0-b · **실측 당시
35행**)을 프로토타입(증인 10개 구현)에 그대로 돌리면 **미구현 24개가 전부 MISSING**이고 **exit 1**이다(원문 발췌):

```
   ok    [lib] w_log_a_no_vanish_stream_is_identical
   MISSING WITNESS  [lib] w_log_d_every_reachable_skip_arm_is_silent        ← 차단 요건
   MISSING WITNESS  [lib] tail_destruction_without_any_vanished_entry_stays_ok_like_today   ← 차단 요건
   MISSING WITNESS  [e2e] dangling_temp_symlink_keeps_lstat_semantics
   skip  [e2e] non_utf8_temp_name_is_stat_and_unlinked_by_raw_bytes   (platform=linux · OS=Darwin)
   ok    [reconcile_vanishing_entries] phase_e_entry_loop_survives_vanishing_entries
DISCOVERY FAILED — 선언된 증인이 그 타깃의 바이너리에 없다.
  ==> exit 1
```

⇒ **W10b · W-LOG-D의 *"차단 요건"* 선언이 산문에서 기계로 바뀐다.**

**0-e. ★ W-REG를 폐기한다 — 크레이트 안의 검사는 자기가 막겠다던 공격에 스스로 당한다**

r21의 W-REG(`pins.rs` 인라인 · `current_exe() --list`로 자기 레지스트리를 묻는 테스트)는 **두 가지 이유로
폐기한다**:

1. **목록이 두 곳이 된다** — 셸과 크레이트가 각자 ID 배열을 들면 **반드시 어긋난다**(Codex r22:
   *"basename 목록을 중복시키지 말고 타깃을 아는 발견 스크립트 하나를 권위로 삼아라"*).
2. **⚠⚠ 그것은 `#[ignore]` 한 줄로 무력화된다 — 자기가 막겠다던 바로 그 공격에.** **실측**(복합 공격:
   W-REG에 `#[ignore]` + `mod log_witness;` 한 줄 삭제):

```
test result: ok. 128 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.79s
  ==> cargo test --lib exit = 0      ← W-LOG-A/B/C가 증발했고(132 → 129) W-REG는 ignored라 침묵한다

--- 같은 트리 · 하네스 **밖**의 게이트 스크립트 ---
   MISSING WITNESS  [lib] w_log_a_no_vanish_stream_is_identical
   MISSING WITNESS  [lib] w_log_b_downstream_events_fire_after_the_pass_survives
   MISSING WITNESS  [lib] w_log_c_skip_path_emits_no_event_at_any_level
DISCOVERY FAILED …
  ==> 게이트 exit = 1
```

⇒ **레지스트리 게이트는 그것이 감사하는 하네스 *밖*에 있어야 한다.** 이것이 **스크립트를 `tests/` 안의
Rust 테스트로 만들지 않은 이유**이기도 하다(§0-f).

**0-f. 스크립트를 어디에 두는가 — 선택지 평가와 판정**

| 안 | 무엇 | scope 개정 | 판정 |
|---|---|---|---|
| **(a)** | **인라인 셸**을 B-1 acceptance에 적는다(파일 없음) | **불필요** | **반려.** 파일이 없으면 **아무도 그것을 실행하지 않는다** — 사람이 문서에서 복사해 붙여야 한다. **P-34가 죽인 실패 양식과 정확히 같은 클래스**(*"선언만 하고 컴파일·실행에 넣지 않았다"*). 릴리스 게이트·CI가 **하나의 산출물로 돌릴 수 없고**, diff로 **약화를 검토할 대상도 없다** |
| **(c)** | **`tests/` 안의 Rust 테스트** | **불필요**(`^tests/`) | **반려 — 실행으로 반려했다.** ⑴ **발견 절반은 된다**: 통합 테스트에서 `Command::new(env!("CARGO"))`로 `cargo test --lib -- --list`를 부르는 것은 **실제로 작동한다**(실측: 웜 트리 **178 ms** · lib 하네스가 stale이라 내부 cargo가 **컴파일까지 하는** 경우에도 **1.59 s · exit 0** — cargo는 테스트 바이너리를 **실행하는 동안 빌드 락을 쥐지 않는다**). ⑵ **그러나 `0 ignored` 절반이 원리적으로 불가능하다**: ignored 수를 알려면 **스위트를 실행**해야 하는데, 그 스위트 안에 자기 자신이 있다 ⇒ **무한 재귀**. ⑶ **⚠ 치명**: 하네스 **안**의 게이트는 **`#[ignore]` 한 줄로 꺼진다**(§0-e 실측 — 구 W-REG가 정확히 그렇게 죽었다) ⇒ **`#[ignore]`를 막는 게이트가 `#[ignore]`로 꺼진다**. 자기참조 결함이다 |
| **(b)** | **`scripts/f14-witness-gate.sh`** | **⚠ 필요** | **채택.** 하네스 **밖**의 실행 가능한 단일 산출물 ⇒ ⑴ `#[ignore]`로 끌 수 없다 ⑵ 릴리스 게이트·CI가 **한 줄로 돌린다** ⑶ **diff로 약화를 검토**할 수 있다(B-5) ⑷ **타깃을 알므로** 정확 매칭이 된다 |

⚠⚠ **`scope[]` 개정이 필요하다 — 지휘자가 lock을 고쳐야 한다.**
`isTestPath`(`bugfix-status.mjs:89` = `/(^|\/)(tests?|__tests__|spec)(\/|$)|\.(test|spec)\.[a-z0-9]+$/i`)는
**`scripts/…`를 테스트 경로로 판정하지 않는다**(실측: `isTestPath("scripts/f14-witness-gate.sh") = false`)
⇒ `scopeViolationsOf`가 **B4 위반으로 잡는다**. ⇒ **`scope[]`에 다음 한 줄을 추가한다**:

```json
"scope": ["src/store/**", "docs/adr/**", "scripts/f14-witness-gate.sh"]
```

⚠ **와일드카드가 아니라 *정확 경로*다** — 비-테스트 표면이 **정확히 파일 하나만큼** 넓어진다(실측:
`globMatch("scripts/f14-witness-gate.sh", "scripts/f14-witness-gate.sh") = true` ∧
`globMatch(…, "scripts/other.sh") = false`). **`scripts/**`로 넓히지 않는다.**
**B4 근거**: 셸 스크립트는 **컴파일·링크되지 않는다** ⇒ **관측 행동을 만들 수 없다**(ADR과 같은 논증).

**0-g. 게이트가 여전히 *못* 잡는 것** (정직 — §5의 **B-DISCOVERY** · **B-GATESELF**가 정본):
**내용 없는 증인**(`assert!(true)`도 발견을 통과한다 — 발견은 *존재*를 증명할 뿐 *내용*을 증명하지 않는다) ·
**개명**(스크립트와 표를 함께 고치면 통과한다 — 그것이 의도된 동작이다) · **레지스트리에서 ID를 지우는 것**
(사라진 ID = 사라진 증인 ⇒ **B-5**) · **플랫폼 게이트 오용**(`unix` → `linux`로 낮추면 한쪽에서 조용히 빠진다) ·
**②의 명령을 좁히는 것**(`--tests` → `--lib`이면 통합 타깃의 ignored가 안 보인다 — **selftest는 판정 함수를
증명하지 입력의 진위를 증명하지 않는다**) ·
**★ 게이트 스크립트가 아예 실행되지 않는 경우** — **가장 값싼 공격이고, 게이트 자신은 이것을 막지 못한다**
(실행되지 않은 스크립트는 아무 말도 하지 않는다 · `--selftest`도 마찬가지다). **막는 것은 셋뿐이고 전부 게이트
밖에 있다**: ⑴ **B-1 acceptance의 0단계 두 줄**(지휘자·사람이 돌린다) ⑵ **릴리스 게이트의 anti-cheat diff
리뷰**(*"스크립트가 살아 있고 0단계가 그것을 부르는가"* — **B-5 (나)**) ⑶ **`scope[]`의 정확 경로**(지우거나 고치면 **diff에 반드시 뜬다**). ⇒ **초록 불이 아니라 사람의 눈이다. 그렇게 적는다.** ⇒ **내용은 뮤턴트 표가, 삭제·개명·미실행은 릴리스 게이트의 anti-cheat diff 리뷰가 맡는다.**

**0-h. ★ `--selftest` — 게이트가 자기를 증명한다 · **술어 × 케이스 직교** (★r24/P-39 · §5의 **B-GATESELF**)**

> **P-34~P-38은 전부 *게이트 자신의* 결함이었다. ★P-39는 그 보상 통제(selftest)마저 결함이었음을 보였다** — **픽스처가 *다른 술어로도* 실패했기 때문에, 검사하려던 술어를 지워도 초록이었다.**
>
> **⚠⚠ P-39 재현 (r23 스크립트 · 원문)** — 술어를 지우고 `--selftest`를 돌린다:
> ```
> $ grep -v 'FAIL: failed=' r23.sh > x1.sh && bash x1.sh --selftest | tail -2
> SELFTEST: PASS (6/6 · 옛 파서는 (b)·(d)를 놓친다)  ==> exit 0   ← **`failed` 검사를 지웠는데 6/6 초록**
> $ (결과-줄-0개 가드 삭제) → **6/6 · exit 0**   ·   $ (둘 다 삭제) → **6/6 · exit 0**
> ```
> **원인**: (d) 10-failed와 (f) 결과-줄-0개가 **cargo exit 101을 함께 넘겼다** ⇒ **PRED-RC(nonzero-exit
> 검사)가 기대 실패를 대신 공급했다.** ⇒ **픽스처를 `(출력, rc)` 쌍으로 분리**한다: **(d)·(f) → rc 0** ·
> **(e)만 rc 101**(nonzero-exit **전용** 증인). 그리고 **발견 술어는 selftest가 아예 안 건드리고 있었다**
> ⇒ **발견 케이스 3종을 신설**한다(`discover()`에 **목록-해결자를 주입** — cargo 미호출).

**★★ 술어 × 케이스 매트릭스 — 게이트 명세의 정본(SSOT)이다 (★r25/P-40).**
**이 표와 §0-b의 임베드된 스크립트가 케이스·술어의 *유일한* 규범 정의다.** 케이스를 더하거나 빼는 유일한
방법은 **이 표와 스크립트를 같은 커밋에서 함께 고치는 것**이다. **문서의 다른 어떤 곳도 케이스 수·술어 수를
적지 않는다 — 전부 여기를 가리킨다**(B-1 · B-5 · §5의 B-GATESELF · 뮤턴트 표).
**케이스 하나가 술어 하나만 죽인다**(●=킬러 · ○=통과해야 한다):

| 케이스 (픽스처 = 출력 · **rc**) | PRED-DISC | PRED-LIST-RC | PRED-N0 | PRED-IGN | PRED-FAIL | PRED-RC | M-SIGPIPE |
|---|---|---|---|---|---|---|---|
| **(a)** 1 ignored · **rc 0** | | | | **●** | | | |
| **(b)** 10 ignored · **rc 0** (+옛-파서 핀) | | | | **●** | | | |
| **(c)** 전부 정상 · **rc 0** (대조군) | ○ | ○ | ○ | ○ | ○ | ○ | |
| **(d)** 10 failed · **rc 0** *(r23은 101)* | | | | | **●** | | |
| **(e)** 정상 출력 · **rc 101** | | | | | | **●** | |
| **(f)** 결과 줄 0개 · **rc 0** *(r23은 101)* | | | **●** | | | | |
| **(g)** 목록 ok · 증인 누락 | **●** | | | | | | |
| **(h)** 조기 매치 + **1.1 MB 목록** | ○ | | | | | | **●** |
| **(i)** 목록 명령 rc 101 | | **●** | | | | | |

**술어별 뮤턴트 킬 실증 — "지우면 RED가 되는가"를 *전부* 돌렸다** (프로토타입 · 원문):

```
뮤턴트               exit   RED가 된 케이스 (= 그 술어를 죽인 케이스)
(없음/원본)          0      — (9/9 PASS)
M-SIGPIPE            1      (h) 조기매치+큰목록   발견=FAIL (기대 PASS)
M-PRED-DISC          1      (g) 증인 누락         발견=PASS (기대 FAIL)
M-PRED-LIST-RC       1      (i) 목록 rc!=0        발견=PASS (기대 FAIL)
M-PRED-N0            1      (f) 결과 줄 0개       게이트=PASS (기대 FAIL)
M-PRED-IGN           1      (a) 1 ignored · (b) 10 ignored   게이트=PASS (기대 FAIL)
M-PRED-FAIL          1      (d) 10 failed         게이트=PASS (기대 FAIL)
M-PRED-RC            1      (e) cargo rc!=0       게이트=PASS (기대 FAIL)
M-OLDPARSER          1      (b) 10 ignored · (d) 10 failed   게이트=PASS (기대 FAIL)
```
⇒ **8/8 RED · 살아남은 술어 0.** (대조: **r23은 M-PRED-FAIL · M-PRED-N0가 GREEN으로 살아남았고 발견 술어는 케이스가 아예 없었다.**)

```
$ scripts/f14-witness-gate.sh --selftest    # 9개 케이스 전부 [ok] · 발췌 (rc 열이 P-39의 수리다):
   [ok] (d) 10 failed  rc=0  게이트=FAIL (기대 FAIL) · 옛 파서=PASS   [ok] (f) 결과 줄 0개  rc=0  FAIL
   [ok] (h) 조기매치+큰목록   발견=PASS (기대 PASS)   ← P-38          [ok] (e) rc=101       FAIL
SELFTEST: PASS (9/9)  ==> exit 0   (bash 3.2 · 5.3 · zsh **전부 exit 0** · cargo 미호출 · 밀리초)
```

⚠ **옛 파서(`grep -vc '0 ignored'`)를 *일부러 남겨 두고* (b)·(d)에서 그것이 *통과함*을 단언한다** — **파서를 부분문자열로 되돌리는 리팩터가 selftest를 깨도록**(M-OLDPARSER).
⚠⚠ **규칙으로 박는다**: *"selftest에 술어를 추가할 때마다 **그 술어를 지운 뮤턴트를 돌려 RED를 확인**한다. 확인하지 않은 술어는 **핀되지 않은 것**이다."* — **P-39가 정확히 그 미확인의 대가였다.**

⇒ **acceptance 0단계는 두 줄이다. 게이트 자신이 게이트를 통과해야 한다.**

```bash
bash scripts/f14-witness-gate.sh --selftest   # 게이트가 자기를 증명한다 (cargo 미호출 · 밀리초)
bash scripts/f14-witness-gate.sh              # ① 발견 단언  ∧  ② 결과 게이트
```

**★ 0단계의 규범 요구 — 명시한다 (★r25/P-40).** ⚠ **아래 어느 문장도 숫자를 반복하지 않는다. 케이스·술어의
정본은 위 매트릭스(§0-h)와 §0-b의 스크립트뿐이다.**

1. **`--selftest`가 §0-h가 정의하는 *모든* 케이스에 대해 PASS**해야 한다 — **하나라도 빠뜨리면 계획 위반**이다.
   ⚠ **케이스를 골라 돌리지 마라**: 지금 (g)·(h)·(i)가 **수용된 P-38·P-39 봉인을 핀하는 유일한 케이스**이고,
   그것을 빠뜨리면 **목록-상태(LIST-RC)와 SIGPIPE 회귀가 미검증으로 나간다**(그것이 **P-40**이다).
2. **§0-h의 술어 전부**(**DISC · LIST-RC · N0 · IGN · FAIL · RC** — **목록의 정본은 §0-h 매트릭스의 열**)
   **+ M-SIGPIPE + M-OLDPARSER**가 **전부 뮤테이션-킬**돼야 한다: 구현자가 **각 술어를 지운 뮤턴트를 실제로
   돌려 selftest가 RED가 됨을 실증**한다. **선언으로 대신할 수 없다** — 프로토타입에서 **8/8 RED · 살아남은
   술어 0**으로 확인됐고(§0-h의 킬 표가 그 원문이다), **확인하지 않은 술어는 핀되지 않은 것이다**(P-39의 대가).
3. **게이트는 acceptance의 0단계다** — `--selftest` → 본 게이트 **두 줄**이 **스위트(1~3)보다 먼저** 돈다.
   **0개 발견이 통과가 될 수 없다.**

⚠ **정직**: selftest는 **판정 함수**를 증명하지 **입력의 진위**를 증명하지 않는다(②를 `--lib`로 좁히거나 스크립트를 아예 안 부르는 공격은 못 잡는다) ⇒ **§0-g · B-5 diff 항목이다.**

**그 다음에야 스위트를 돌린다 (아래 1)~3)).**

**1) 플립 증인 *둘 다* RED → GREEN** (⚠ **하나만 초록으로 만드는 픽스는 acceptance 실패다** — P-15)

```
cargo test --lib -- reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot \
                    reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot   # → 2 PASS
```
그리고 **두 대조군**은 **계속 초록**이다. ⚠ **Temp 증인이 park하는 `pre_entry` seam(`reconcile.rs`의 그
한 줄)을 픽스 도중 지우면 증인이 컴파일은 되면서 영원히 park하지 못한다** ⇒ **B-5의 릴리스-게이트 확인
항목**이다.

**2) characterization 전원 초록 — ⚠ *픽스 트리에서는 138이 아니라 141이다*** (r18 프로토타입 실측)

```
cargo test --lib --bins --test adversarial --test contract --test e2e --test layout_tree \
  --test openapi --test regression_reconcile_gc_dedup_race \
  -- --skip reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot \
     --skip reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot
```

> ⚠⚠ **숫자를 정직하게 못박는다** — 이 명령이 내는 합계는 **트리마다 다르다**:
>
> | 트리 | `--lib` | 합계 | 근거 |
> |---|---|---|---|
> | **red.sha (`ac58bd7`)** | **118** passed (120 − 2 skip) | **138** = 118+8+1+2+3+5+1 | **`--verify-red` 실측 · 동결** |
> | **픽스 트리 (프로토타입)** | **121** passed (**123** − 2 skip) | **141** = 121+8+1+2+3+5+1 | **실측** |
>
> **픽스가 lib 테스트를 120 → 123으로 늘리고(신규 증인 3개) `--skip`은 2개만 제외한다 ⇒ 123 − 2 = 121.**
> ⇒ **픽스 트리에서 "138"은 원리적으로 나올 수 없다.** acceptance의 실질 요구는 *"이 명령이 **0 failed**"*
> 이고 그것은 **충족된다**. **`characterizationCmd` 문자열은 동결이고 red.sha에서의 138도 동결이다** —
> 바뀌는 것은 **픽스 트리에서 기대할 숫자**뿐이다. (증인을 더 이식하면 그 숫자는 더 커진다 ⇒
> **합계로 게이트하지 말고 `0 failed`로 게이트하라.**)

**3) 결정적 증인 전원 GREEN**

```
cargo test --lib --test adversarial              # W1~W7 · W9 · W10 계열 · W11 · W17 · 회귀 증인 2개
cargo test --test reconcile_vanishing_entries    # W13 (debug)
cargo test --release --test reconcile_vanishing_entries                      # B-1 보상 통제(프로파일 편향)
cargo test --release --lib -- reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot \
                             reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot
```

**3-a) 원 repro(reproCmd) + stress(별도) — R-2′**

```
cargo test --test repro_concurrent_puts_reconcile    # reproCmd = 원 40-put 안무(정본 증거)
cargo test --test stress_concurrent_puts_reconcile   # 증폭 1,000-put stress(별도 커버리지 — reproCmd 아님)
```

> **repro 규모 = 정확히 40 total puts**(`PUT_WORKERS = 40` × `PUTS_PER_WORKER = 1` = `TOTAL_PUTS = 40`).
> 이것이 **`reproCmd`의 정본**이며 진단이 적은 원 안무
> (`adversarial.rs::concurrent_nested_puts_with_reconcile_loop_preserve_all` — 40 tasks × put 1개)와
> **글자 그대로 같은 put 안무**다(키 `dir/sub/file-{i}.bin` · 바디 `vec![i; 200]`). 재현율은 **오직
> reconcile/관측 쪽 밀도**(sleep 제거·`RECONCILE_LOOPS = 4`·`OBSERVER_LOOPS = 8`·`DECOY_BLOBS = 100`)로만
> 올린다 — **put 수는 불변**이다. 비공허 자기검증: **반복 증인**(`overlapped ≥ MIN_OVERLAPPED_PASSES = 3`)
> ∧ **레이스 증인**(`put_temp_vanishes = vanished_during_pass − passes ≥ MIN_PUT_TEMP_VANISHES = 20` —
> `.objects`에 `.tmp-`를 만드는 주체가 put과 reconcile의 gc-pending 둘뿐이라 **산술로 강제**된다) ·
> red 실패 메시지에 **`PASS ABORTED`**. 실측 **RED 20/20 FAIL · GREEN 20/20 PASS**.
> ⚠ **증폭된 1,000-put(`40 × 25`)은 `tests/stress_concurrent_puts_reconcile.rs`로 분리**됐다 —
> **timing-sensitive race라 증폭 workload의 실패가 원 40-put 시나리오를 증명하지 않으므로**(R-2′) **stress로만**
> 유지한다. 두 파일은 **게이트 레지스트리 증인이 아니다**(`scripts/f14-witness-gate.sh`의 `WITNESSES`에 미등록 —
> reproCmd/stress는 릴리스 게이트 커버리지이지 F-14 증인이 아니다).

⚠ **`tests/reconcile_fd_pressure.rs`(W15′)는 존재하지 않는다** — **fd를 하나도 더 쓰지 않으므로 잴 것이
없다**(C안의 fd 압박 밴드 C-5가 **소멸**했다).
⚠ **W5e′ · W6b는 프로브 후 skip될 수 있다**(root) — **사유를 출력한다. 조용한 GREEN 금지.**
⚠ **W17은 `#[cfg(target_os = "linux")]`로 게이트된다**(APFS는 비-UTF-8 파일명을 `EILSEQ`로 거부한다) →
**B-12**.
⚠ **`--release` 두 줄이 acceptance의 일부다** — **B-1(프로파일 편향)의 유일한 보상 통제**다.
⚠ **W10-G · W10c는 green-only다** — red.sha에서 RED이지만 **`flips[]`에 넣지 않는다**(§F-1: red.sha 트리에
그 파일이 **존재하지 않는다**. 그리고 그것들은 **두 번째 플립이 아니라 *같은 하나의 플립*의 추가 증인**이다
— 하드룰 10이 명시 허용하며 `flips[]`의 2행이 이미 같은 근거로 서 있다). **`regressionCmd`·`flips[]`·
`red.sha`·`characterizationCmd`는 전부 동결 그대로다.**

**배리어를 쓰는 증인**: **W6 · W6b · W10 계열**(기존 `pre_grave`/`pre_entry`) · **Temp 회귀 증인**
(`pre_entry`) · **W11**(신규 `pre_recover_grave`). **전부 park + spawn을 쓰고 ADR-0002 봉인 체크리스트
⑦⑧⑨의 규율을 따른다**(도착 신호 ≺ park · `notify_one()` 해제 · 유한 타임아웃 · `JoinError`와 내부
`io::Result`를 **둘 다** 언랩 · **분기 진입 자기검증**).

**W10 계열의 공통 무대 규율** (§C — **자기무효화 구성상 봉인**): `pre_entry` 훅이 **모든 발화 이름을
기록**하고 **첫 발화에서만** `remove_dir_all(.objects)`를 **완주까지 await**한다(spawn 0 · 채널 0 · sleep 0
⇒ *"spawn ≠ 폴링됨"* 함정이 구조적으로 없다). **비트로트 blob 0 · `.corrupt` 0 · 동시 put 0**
(⇒ 격리 분기의 `mkdir_p_durable`이 **원리적으로 발화 불가**) — **B-5 diff 항목**.

| id | 위치 | 무엇을 심고 무엇을 단언하나 | red/green |
|---|---|---|---|
| **W1** | `reconcile/entry.rs` unit | `seen` 정책 표. (a) 부재 경로 + `NotFound` → `Gone` · **(b) 댕글링 심링크 + `NotFound` → raw `Err(NotFound)`, 메시지 무변조** · (c) 일반 파일 경로 + `NotFound` → raw · **(d) 부재 경로 + `PermissionDenied`/`IsADirectory`/`StorageFull`/`Other` → raw, kind·메시지 무변조** | green only |
| **W2** | `reconcile/entry.rs` unit | 스냅샷 → 삭제 → **`metadata()` · `read()` · `remove()` · `rename_into()` · `rename_durable_to()` 각각** → `Gone`. ⚠⚠ **`file_type()`은 이 목록에 넣지 않는다** — d_type 캐시 때문에 소멸한 항목에도 **`Ok`** 다(실행 확정) ⇒ **정직한 특성화로 `Present`를 단언한다** | green only |
| **W3** | e2e `#[cfg(unix)]` | `.objects/<64hex>` = 댕글링 심링크 → `run_once` **`Err`**, `kind == NotFound`, **링크 잔존**, `.gc-pending.json` **미기록** | **양쪽 GREEN** |
| **W4** | e2e `#[cfg(unix)]` | `.objects/.tmp-x` = 댕글링 심링크. old → `Ok` ∧ **`temps_deleted == 1`** ∧ 링크 삭제 / recent → 보존·0 (`de.metadata()`의 lstat 의미론) | **양쪽 GREEN** |
| **W5** | **`reconcile/absence.rs` unit ×5**(⚠ **r16/P-28: `atomic.rs`가 아니다** — `rename_*_source_checked`는 **`absence.rs`에 신설**된다. red.sha의 `atomic.rs`에는 없던 심볼이다 — **이사가 아니다**(r17/P-30)) (**전부 `&Vanished`를 받는다**) | **a** 소스 부재 → `SourceGone` ∧ **`vanished.get() == 1`**(계수와 부재 판정이 **같은 행위**임을 핀한다) · **b** 소스 존재 + 목적지 부모 부재 → **raw `Err(NotFound)` ∧ 소스 잔존 ∧ `vanished == 0`** · **c** 소스 존재 + `fsync_parent`를 **없는 디렉터리**로 → rename은 **실제로 일어났고**(`to` 존재 ∧ `from` 부재) 반환은 **raw `Err(NotFound)`**(**`SourceGone` 아님** — P-2) · **d** 소스 = 댕글링 심링크 → **`Done`** · **e′** 확인 `symlink_metadata`가 **EACCES**(디렉터리 `0o600`) → **`None`**(EACCES는 부재가 아니다) ⇒ **M-B7 킬**. ⚠ **root 민감 → 프로브 후 skip**(사유 출력) | green only |
| **W6** | `pins/tests/` (기존 `pre_grave`) | park 중 **파킹된 sha의 blob 삭제** → `GraveOutcome::SourceGone` → `Ok`, **`gc_deleted == ORPHANS-1`**, tombstone 정리, 무덤 잔재 0. **결정적** | green only |
| **W6b** | `pins/tests/` `#[cfg(unix)]` | park 중 `.objects`를 **`0o300`**(no read)으로 chmod → rename은 성공하고 fsync의 `File::open(dir)`는 **EACCES** → **raw `Err(PermissionDenied)`** ∧ **무덤이 디스크에 남아 있다**. ⚠ **root → 프로브 후 skip** | green only |
| **W7** | e2e `#[cfg(unix)]` | `.objects/<64hex>` = **디렉터리를 가리키는 심링크**(⚠ 타깃은 절대 경로) → `read` = **`IsADirectory`** → `Err`, `kind != NotFound`, 항목 잔존 | **양쪽 GREEN** |
| **W9** | e2e `#[cfg(unix)]` ×2 | **a** `.corrupt`가 **일반 파일** + 비트로트 blob → rename **ENOTDIR** → `Err`, `kind != NotFound`, **blob 보존** · **b** `.corrupt`가 **댕글링 심링크** → rename → **`NotFound` ∧ 소스 존재** → **raw `Err(NotFound)`**, blob 보존, `quarantined == 0` | **양쪽 GREEN** |
| **W10** *(특성화 · **가드의 `Err` 팔**)* | `pins/tests/` (`pre_entry`) | **Blob 무대**: 미참조 blob **3개**(tombstone 없음). 첫 `pre_entry`에서 `remove_dir_all(.objects)`. **단언** ① `run_once` = **`Err`** ∧ `kind == NotFound` ∧ **errno 2 · 메시지 무변조**(가드가 `metadata`의 에러를 **무가공** 전파) ② **`.objects` 미부활**(= §C의 자기무효화 검사 그 자체) ③ **`.gc-pending.json` 부재** | **양쪽 GREEN**(실측 T2) |
| **W10-TEMP** *(★신규 · 특성화 · **계수의 클래스 전수화**)* | `pins/tests/` (`pre_entry`) | **Temp 무대**: `.objects = {.tmp-x0, .tmp-x1, .tmp-x2}`(blob 0). 같은 파괴. **단언 = W10과 동일 3종.** ⇒ **`vanished`가 Blob 팔뿐 아니라 Temp 팔에서도 선다는 것을 행동으로 핀한다** — r14 반증이 *"W10(blob 무대)만으로는 계수 누락 뮤턴트가 살아남는다"*를 **실측으로** 보였다(그 뮤턴트는 `Ok` + 부활 + 원장 발행을 냈다). **타입 자물쇠(§A의 `Vanished`)가 1차 방어이고 이 증인이 2차다** | **양쪽 GREEN**(실측: base `Err(NotFound/2)` · D안 `Err(NotFound/2)`) |
| **W10-G** *(★신규 · **green-only** · 가드 경로 self-verify)* | `pins/tests/` (`pre_entry`) | ④ **`pre_entry` 발화 sha 집합 == 심은 3개 전부** ⇒ **루프가 끝까지 돌았다** ⇒ 파괴 이후 모든 항목 연산은 skip됐다 ⇒ **그 `Err`는 항목 연산이 낼 수 없다 = 가드가 냈다**(실측: D안 발화 **3회** · 오늘 **1회**) ⑤ 창을 실제로 밟았다: `remove_dir_all`의 `unwrap()` ∧ 심은 항목 수 ≥ 3 | **green only**(오늘은 발화 1회) |
| **W10b** *(★신규 · 특성화 · **게이트를 핀한다**)* | `pins/tests/` (`pre_entry`) | **꼬리 파괴**(단일 `Other` 항목 = 63자 이름 ⇒ 분기 본문 없음 = syscall 0): `Ok(ReconcileStats::default())` ∧ **`.objects` 부활** ∧ **원장 `{}`** — **T1이 red.sha에서 실측한 그대로.** ⇒ **M-GUARD-ALWAYS를 죽이는 유일한 증인**(무조건 가드는 여기서 `Ok → Err`) | **양쪽 GREEN**(실측 T1) |
| **W10c** *(★신규 · **green-only** · 가드의 `Ok(dir)` 팔)* | e2e `#[cfg(unix)]` | ⚠⚠ **r14 반증의 정정을 반영했다** — 무대는 *"심링크 `.objects` + **소멸 0**"*이 **아니다**(거기서는 `vanished == 0`이라 **가드가 아예 돌지 않아 아무 뮤턴트도 죽지 않는다** — 실측). 무대: **`.objects`가 심링크→dir인 정상 배포 ∧ 항목 1개가 진짜로 소멸**. **단언**: 패스 **`Ok`**(= 유일한 플립이 심링크 배포에서도 성립) ⇒ **M-GUARD-LSTAT**(가드를 `symlink_metadata`로) 는 `is_dir()==false`를 보고 **`Err(NotADirectory)`** 를 낸다 ⇒ **RED**(실측) | **green only**(오늘은 `Err` — 그것이 **같은 하나의 플립**이다) |
| **W10c′** *(특성화 · **정직: 뮤턴트 킬 0**)* | e2e `#[cfg(unix)]` | 심링크 `.objects` **∧ 소멸 0** → 양쪽 `Ok`. **가드가 발화하지 않는다는 것**(P11)을 핀할 뿐 **어떤 뮤턴트도 죽이지 않는다** — **숨기지 않고 그렇게 적는다** | **양쪽 GREEN** |
| **W-GRAVE-CD-A** *(★r15 · 특성화 · **M-FRESH/M-FRESH′의 유일한 킬러**)* | `pins/tests/` (`pre_grave`) | **무대 = §C-A**(비예약 항목 **정확히 1개**). park 중 **`remove_dir_all(.objects)`**(재생성 **없음**) → 해제. 픽스 트리: `grave()` rename ENOENT → `entry_is_absent(blob)` ENOENT → **`SourceGone` + bump(1)** → skip → 루프 종료 → **가드**(`get()==1`) → `metadata` ENOENT → **`Err`**. **단언**: `Err` ∧ `kind==NotFound` ∧ **`raw_os_error()==Some(2)`**(무가공) ∧ **`.objects` 부재** ∧ **`.gc-pending.json` 부재** ∧ **`.corrupt` 부재**. **self-verify** ① park 도달 ∧ `sha==VICTIM` ② park 시점 `.objects/<sha>` **존재** ∧ `.gc-grave-*` **0개** ∧ `remove_dir_all().unwrap()` ∧ 직후 `.objects` 부재 ③ `pre_grave` **정확히 1회** ④ **`post_grave` 0회** ⇒ `Renamed::Done`이 아니었다 ⇒ **`Graved`도 `settle()`도 없었다** ⑤ **`pre_entry` 발화 집합 == {sha}** ⇒ **집계의 bump 후보는 `grave()`뿐이었다**(C-A 규율의 구조적 자기검증). **가드가 냈다는 논증**: `post_grave` 0회 ∧ B가 GREEN ⇒ 이 무대에서 `grave()`는 `?`로 죽지 않는다 ⇒ 그 `Err`는 **항목 연산이 낼 수 없다** | **양쪽 GREEN**(특성화 — red.sha에서는 `rename_durable`의 `?`가 같은 `Err(NotFound/2)`를 내고 부활·원장도 없다 ⇒ **`flips[]` 미등재**) |
| **W-GRAVE-CD-B** *(★r15 · **green-only** · `SourceGone` self-verify)* | `pins/tests/` (`pre_grave`) | **A와 한 줄만 다르다**: park 중 `remove_dir_all(.objects)` **→ `create_dir(.objects)`**(빈 dir) → 해제. 픽스 트리: rename ENOENT → 확인 ENOENT → **`SourceGone`** → 가드 → `metadata` = **`Ok(dir)`** → 통과 → `try_exists(blob)`=`Ok(false)` → `cleaned={}` → **`Ok`**. **단언**: `Ok` ∧ 전수 `assert_eq!(stats, ReconcileStats::default())` ∧ **`post_grave` 0회** ∧ `pre_grave` 1회. ⇒ **`Ok`로 끝났다 = `?`로 안 죽었다** ∧ **`post_grave` 0회 = `Moved`도 아니었다** ⇒ **남는 팔은 `SourceGone` 하나뿐이다**(rename이 정말 `SourceGone`이었음의 **직접 증거**). 덤: **Class B-ABA의 특성화 증인**(인간이 채택한 포기를 코드로 못박는다 · 손실 0) | **green only**(red.sha에서는 `grave()`의 `?`가 `Err`를 내므로 `Ok` 단언이 RED ⇒ **같은 하나의 플립의 추가 증인** ⇒ `flips[]` 미등재) |
| **W11** | `src/store/pins/tests/recover_graves_production_seam.rs` | **프로덕션 진입점 행동 증인 (P-5).** ⚠ `recover_graves_from`을 **직접 부르지 않는다** — `run_once_at_for_test`를 spawn해 **`PassGuard::begin` → `recover_graves`** 경로를 탄다. **무대**: 무덤 4개 — **R 계급**(정본 blob **부재** ⇒ rename 분기) 2개 · **K 계급**(정본 blob **무손상** ⇒ remove 분기) 2개. `pre_recover_grave`가 **모든 발화 sha를 채널로 보내고 첫 발화에서만 park**. park 중 **파킹된 것을 뺀 무덤 3개 삭제** → 재개. **단언**: 패스 **`Ok(stats)`** · `.gc-pending.json` **존재** · 파킹/victim의 사후-디스크 상태 · `ReconcileStats` **전수 `assert_eq!`**. **자기검증** ④ **훅 발화 sha 집합 == {R1,R2,K1,K2}**(FS-독립 — d_type 캐시) | **green only** |
| **W13** | **`tests/reconcile_vanishing_entries.rs`** (신규 통합 바이너리 · 프로덕션 공개 API만) | **프로덕션 빌드 행동 증인 3종** — `tests/`는 **`cfg(test)` 없이 lib를 링크**하므로 **모든 조건부 뮤턴트의 프로덕션 팔을 탄다**. **Phase E**(엔트리 루프) · **Phase G**(복구 두 분기) · **Phase T**(temp 삭제 · `Mut-Count`) → **§W13** | **green only** |
| **W17** *(`#[cfg(target_os = "linux")]`)* | e2e — **비-UTF-8 `.tmp-` 이름** | `OsStr::from_bytes(b".tmp-w17-\xff\xfe")`로 temp를 직접 만든다. **(a) old** → `Ok` ∧ **`temps_deleted == 1`** ∧ 항목 **부재** · **(b) recent** → `Ok` ∧ **`temps_deleted == 0`** ∧ 항목 **잔존**. 전수 `assert_eq!`. **자기검증**: 파일 생성 성공 · readdir 바이트 동일성 · `fs::metadata(dir.join(lossy)).is_err()`. **M46 킬**. ⚠ 개발기(APFS)에서는 돌지 않는다 → **B-12** | **양쪽 GREEN** |
| **W-LOG-A** *(★r20 · 특성화 · **로그 스트림**)* | `src/store/pins/tests/log_witness.rs` (`EventTap` = `CaptureSubscriber` 확장 · **레벨 무관 + `target` 필터** · `set_default`) | **소멸 0**(무덤 1 + 비트로트 1) → 이벤트 스트림을 **전수·순서까지** `assert_eq!`(레벨·target·메시지·필드). **P16 ①의 핀** | **양쪽 GREEN**(실측 · 두 트리 바이트 동일 출력) |
| **W-LOG-B** *(★r20 · **green-only** · 하류 이벤트)* | 같음 (`pre_entry`) | 비트로트 3개 · 첫 항목 소멸 → 패스 `Ok` ∧ `quarantined == N−1` ∧ **로그 = 생존자 (N−1)개의 격리 WARN 정확히 그만큼, 그 외 0건**. **P16 ③의 핀** | **red RED**(`PASS ABORTED … NotFound`) → **D안 GREEN**(실측 · **10회 반복 전부 green ⇒ flaky 0**) |
| **W-LOG-C** *(★r20 · **green-only** · skip 침묵)* | 같음 (`pre_entry`) | 무결 blob **1개**가 소멸 → 패스 `Ok` ∧ `stats == ReconcileStats::default()` ∧ **모든 레벨에서 이벤트 0건**. ⚠ **정직: 이것이 밟는 skip 팔은 `:252`(Blob `read`) 하나뿐이다**(반증 실측) | **red RED** → **D안 GREEN** |
| **W-LOG-D** *(★r20 → **★r26에서 전수화 완결**)* · **차단 요건** | 같음 (`pre_entry` + `pre_grave` + `post_grave` + `pre_recover_grave`) | **밟을 수 있는 skip 팔 *전부*의 침묵**(무대 6개 · 정본 = `log_witness.rs`의 전수표). **①`:236`** Temp `metadata()` — grace 초과 temp를 소멸 · **②`:244`** Temp `remove()` — ★신규 · 아래 α/β · **③`:252`** Blob `read()` — 비트로트 blob 소멸(자기검증: **`.corrupt` 부재** ⇒ 격리 블록 **미도달** ⇒ `:257`이 아니라 `:252`다) · **④`:280`** `grave()` `SourceGone` — 손으로 심은 만료 tombstone + `pre_grave`에서 정본 삭제(자기검증: **`post_grave` 0회** ⇒ 무덤 미탄생 ⇒ `SourceGone` 팔이다) · **⑤`:149`** 무덤 `remove()` — 정본 **무손상** ∧ **무덤 내용은 쓰레기**(자기검증: 쓰레기가 정본을 덮지 않았다 ⇒ rename 분기가 **아니었다**) · **⑥`:154`** 무덤 `rename_durable_to()` — 정본 부재. 무대 ①②③④⑤는 **모든 레벨 0건**, ⑥은 §하류 표 4·5의 **기존 INFO 2건**을 기대값으로 단언한다. **★ 무대 ②의 α/β 논증**(`:236`과 `:244`는 관측 결과가 같으므로 **갈라야 한다**): 첫 `pre_entry`에서 **`.objects`를 통째로 옮긴다**(재생성 없음) ⇒ `de.metadata()`는 **열린 dir fd 기준 `fstatat`**라 **`Ok`**, `remove_file(de.path())`는 **경로 기준**이라 **ENOENT**다. **β**(`age ≤ grace` ⇒ `remove()` 미도달) ⇒ 계수 **0** ⇒ 가드 미발화 ⇒ `write_atomic`이 `.objects`를 **되살리고** 패스 **`Ok`** · **α**(`age > grace` ⇒ `remove()` 도달) ⇒ 계수 **1** ⇒ 가드 ⇒ **`Err(NotFound)`**. **두 실행은 `now`만 다르다** ⇒ β의 `Ok`가 *"`file_type()`도 `metadata()`도 계수하지 않았다"*를 증명하고, 그러면 α에서 계수를 올릴 수 있는 곳은 **`remove()` 하나뿐**이다 ⇒ **α는 `:244`를 밟았다** ∎ (β가 RED가 되면 전제가 깨진 것이고 **조용한 초록이 불가능하다**). ⚠⚠ **이것이 없으면 6개 팔의 로깅 뮤턴트가 전부 전 스위트를 통과한다**(실측) ⇒ **W10b와 같은 등급의 이식 차단 요건** | **green only** (실측 GREEN) |
| ~~**W-REG**~~ *(★r21 → **r22/P-35·P-36에서 폐기**)* | ~~`pins.rs` 인라인~~ | ~~`current_exe() --list`로 자기 레지스트리를 묻는다~~ | **폐기.** ⑴ **목록이 두 곳이 되어 반드시 어긋난다**(Codex r22의 simpler alternative) ⑵ ⚠⚠ **그것은 `#[ignore]` 한 줄로 무력화된다 — 자기가 막겠다던 바로 그 공격에**(실측 §B-1 0-e: W-REG에 `#[ignore]` + `mod log_witness;` 삭제 ⇒ lib **128 passed; 1 ignored** · **exit 0**). ⇒ **레지스트리 게이트는 감사 대상 하네스 *밖*에 있어야 한다** ⇒ **`scripts/f14-witness-gate.sh` 하나가 단일 권위다** |
| 기존 | — | 회귀 증인 2개(RED→GREEN) · 대조군 · **characterization**(⚠ **합계는 트리마다 다르다 — §B-1 acceptance 2)의 표가 정본이고, 게이트는 합계가 아니라 `0 failed`다**. 여기서 숫자를 반복하지 않는다 — ★r25/P-40) · **강화된 `tests/adversarial.rs`**(`let _ =` → `.expect(…)`) | — |
| **repro / stress** *(릴리스 게이트 R-2′ — **F-14 증인이 *아니다***)* | `tests/repro_concurrent_puts_reconcile.rs`(reproCmd) · `tests/stress_concurrent_puts_reconcile.rs`(별도) | **reproCmd = 원 40-put 안무**(`PUT_WORKERS 40 × 1 = 40 total puts` · 정본 증거) · **stress = 증폭 1,000-put**(`40 × 25` · 별도 커버리지). 둘 다 반복 증인 ∧ 레이스 증인 ∧ `PASS ABORTED`. **정본은 §B-1 acceptance 3-a**(여기서 규모를 반복하지 않는다). ⚠ **게이트 `WITNESSES`에 미등록** — 릴리스 게이트 커버리지이지 발견-단언 대상 증인이 아니다 ⇒ **이름을 바꿔도 게이트/레지스트리는 불변**(reproCmd만 `bugfix-lock.json`에서 참조) | red RED 20/20 · green GREEN 20/20 |

> ⚠⚠ **파일 · 타깃 · 테스트 함수명의 정본은 `scripts/f14-witness-gate.sh`의 레지스트리다**(★r22/P-35).
> §B-1의 **증인 ID 표(0-c)는 그 스크립트의 거울**이고, 위 표는 *무엇을 단언하는가*를 적는다.
> **스크립트에 없는 증인은 존재하지 않는 것으로 간주한다** — 게이트가 스위트보다 먼저 그것을 강제한다.

**4) 뮤턴트 — 어떻게 죽는가**

범례: **A** = 결정적 증인 · **B** = 증인 없음, 리뷰 · **C** = 구조적 관측 불가.

| # | 뮤턴트 | 무엇이 죽이나 | Class |
|---|---|---|---|
| **M-NOCHECK** | **부재 확인 제거 — 모든 `NotFound` skip**(= P-21 실험의 "봉인 0" 픽스 · **P-1 위반**) | **W3**(댕글링 blob 심링크 → 오늘 `Err(NotFound)` · 뮤턴트는 skip → `Ok` ⇒ RED) · **W9b**(목적지 부재 rename → 소스 **존재** ⇒ raw `Err` 유지) · **W1(b)** | **A** |
| **M-FOLLOW** | 확인을 **`metadata`(follow)** 로 | **W3**(실측: `metadata(dangling)` = `NotFound` ⇒ 부재로 **오판** → skip → `Ok` ⇒ RED) · **W4** | **A** |
| **M-B7** | `NotFound` **외**의 에러도 skip | **W1(d)**(EACCES/EIO/ENOSPC 무가공) · **W7**(`IsADirectory`) · **W9a**(`ENOTDIR`) · **W5e′** | **A** |
| **M-NOGUARD** *(= M-GUARD-SKIP)* | **루프-후 가드 제거** | **W10 · W10-TEMP · W-GRAVE-CD-A** — 파괴된 세계에서 전 항목 skip → 루프 완주 → `write_atomic`이 **`.objects`를 부활**시키고 **`{}` 원장을 발행** → **`Ok`**(오늘 `Err`) ⇒ ①②③ **전부 RED**(실측: 뮤턴트 `m_noguard`에서 부활 · 원장 발행 확인) | **A** |
| **M-GUARD-AFTER** | 가드를 **`write_atomic` 뒤로** | **W10 · W10-TEMP · W-GRAVE-CD-A** — `mkdir_p_durable`이 컨테이너를 되살린 뒤라 `metadata` = `Ok(dir)` ⇒ 가드가 **영영 참** ⇒ `Ok` + 부활 + 원장 ⇒ **RED**(실측 T3) | **A** |
| **M-GUARD-ALWAYS** | **`vanished > 0` 게이트 제거**(무조건 가드) | **W10b — 그리고 *W10b뿐*이다.** 꼬리 파괴에서 오늘 `Ok` → 뮤턴트 `Err(NotFound/2)` ⇒ RED (**두 번째 플립의 유일한 방벽** · 실측 `m_always`). ⚠⚠ **r18 프로토타입이 실행으로 확증했다**: W10b **없이** `if vanished.get() > 0` → `if true`로 바꾸면 `--lib --bins --tests`가 **전부 GREEN**으로 살아남는다 ⇒ **W10b는 이식의 차단 요건이다**(그것 없이 머지하면 게이트가 지키려던 봉인이 **무보호**로 나간다) | **A** — ⚠ **W10b가 있을 때에만** |
| **M-GUARD-LSTAT** | 가드를 **`symlink_metadata`**(no-follow)로 | **W10c**(**green-only** — 심링크→dir `.objects` ∧ 소멸 1건: D안 `Ok` · 뮤턴트 **`Err(NotADirectory)`** ⇒ RED. 실측). ⚠ **r14 반증 정정**: *"심링크 + 소멸 0"* 무대(구 W10c)로는 **아무것도 죽지 않는다** — 가드가 돌지 않기 때문이다 | **A** |
| **M-GUARD-NODIR** | 가드에서 `is_dir()` 절 제거 | ⚠ **증인 없음** — `.objects`가 일반 파일이면 **루프가 이미 ENOTDIR로 시끄럽다**(실측: base·D안 둘 다 `(NotADirectory, 20)`) ⇒ 가드에 **닿지 않는다**. **정직하게 등재**(잔여 **B-6′**) | **B** |
| **M-FRESH** *(★r15 · **P-27이 지목한 그것**)* | **`pins::grave`가 대체 집계를 지어** `rename_durable_source_checked`에 넘긴다 (`new`/`default`/튜플 리터럴/`share`) | **프로덕션 빌드에서는 컴파일 불가** — `E0624`/`E0599`/`E0423`/`E0624`(실컴파일). ⚠ **그러나 `cargo test --lib`에서는 테스트 다리(`new_for_test`) 때문에 *컴파일된다*** ⇒ **정직하게 강등한다** ⇒ **`W-GRAVE-CD-A`가 죽인다**(`Ok` + `.objects` 부활 + `{}` 원장 ⇒ 3단언 RED · 실측) | **A(행동)** *(구 A(타입))* |
| **M-FRESH′** *(★r15 · 정직)* | **`run_once_at` *안*** 에서 `let decoy = Vanished::new()` | ⚠ **BUILD OK** — 타입 봉인은 **모듈 간에만** 선다(자기 모듈 안의 지역 변수 날조는 어떤 가시성으로도 못 막는다) ⇒ **`W-GRAVE-CD-A`가 죽인다**(실측 RED). ⚠ **W10/W10-TEMP는 못 죽인다** — 그 무대는 항목이 3개라 **다른 항목이 스스로 집계를 올린다**(§C-A 규율) | **A(행동)** |
| **M-FRESH-CLONE** | `&vanished.clone()` | ⚠ **컴파일되지만 무해** — `Vanished`에 `Clone`이 없으므로 이것은 **`&Vanished`의 참조 클론**(자동 역참조) = **같은 Arc = 같은 집계**. by-value 강제 시 `E0308` | — |
| ~~**M-COUNT**~~ *(★r15 — **과대주장 철회**)* | ~~`vanished += 1`을 **한 팔에서 누락**~~ | ⚠ **"표현 불가"는 거짓이었다.** private `bump()`는 **위조**를 막을 뿐 **두 채널 중 하나에서의 누락**은 못 막는다 ⇒ **아래 두 행으로 쪼갠다** | — |
| **M-NOBUMP-ASYNC** *(★r15)* | **`entry_is_absent`(async)** 의 `tally.bump()` 한 줄 삭제 | **W10 ∧ W10-TEMP** — `Entry::seen` 채널이 죽는다 ⇒ 가드 영영 미발화 ⇒ 파괴된 세계가 **조용한 `Ok` + 부활 + 원장** ⇒ RED. ⚠ **`W-GRAVE-CD-A`는 이것을 못 죽인다**(그 무대의 유일한 bump는 `grave()` = blocking 채널 ⇒ **GREEN** · 실측) | **A** |
| **M-NOBUMP-BLOCKING** *(★r15)* | **`entry_is_absent_blocking`** 의 `tally.bump()` 한 줄 삭제 | **`W-GRAVE-CD-A`** — `grave()`/격리/무덤 rename 채널이 죽는다 ⇒ 조용한 `Ok` + 부활 + 원장 ⇒ RED(실측). ⚠ **W10/W10-TEMP는 못 죽인다**(그 무대는 `Entry::seen` = async 채널만 탄다) | **A** |
| ~~**M-PENDING**~~ | ~~`Seen::Gone`에서 `pending.remove`를 **넣는다**~~ | **애초에 넣지 않는다**(§E) ⇒ 뮤턴트가 아니라 **반려된 설계**다 | — |
| **M8 · M-BUMP-OUTSIDE · M-GET-IN-PINS** | `pins`가 `Absent(())` 주조 / `absence` 밖에서 `bump()` / `pins`에서 `get()` | **컴파일 불가** — `E0423` / `E0624` / `E0624`(실컴파일 · r15). ⚠ 이 셋은 **테스트 다리와 무관하다**(다리는 `new_for_test`만 연다) ⇒ **Class A(타입) 유지** | **A(타입)** |
| **M6** | fsync를 rename 앞으로 / `Ok` 팔에서 `SourceGone` / **확인을 `rename_durable`(융합)에 붙인다** | **W5c · W6b**(rename `Ok` 이후 fsync 실패 = **무가공** — P-2). ⚠ **r14 프로브가 실증했다**: 융합에 확인을 붙이면 **rename 성공 후의 fsync ENOENT가 `SourceGone`으로 위조**된다 ⇒ **§A 행 16 · §③이 `rename_checked_blocking` 전용임을 못박는다** | **A** |
| **M7** | 확인을 **목적지** 경로에 | **W5b · W9b** | **A** |
| **M46** | `Entry`가 **lossy 이름으로 경로 재구성**(`dir.join(&self.name)`) | **W17**(Linux) + **자물쇠**(`Entry`에 `dir` 필드 **없음** ⇒ 뮤턴트는 **필드 추가**를 요구한다) | **A**(Linux) / **B**(macOS) |
| **M3′** | `Entry::metadata()`를 `tokio::fs::metadata`(추종)로 | **W4** | **A** |
| **M-B1** | `Entry`의 FS 호출을 `de.*` 대신 경로 stat으로 | ⚠ **증인 없다** — 정상 경로에서 결과가 같다. 자물쇠: *"모든 FS 호출은 `de.*` 또는 `de.path()`를 경유한다"* + **B-5 diff 리뷰** | **B** |
| **M-FT** | `Entry::file_type()`을 raw `?`로 되돌린다 | ⚠ **증인 0 ∧ 행동 차이 0** — d_type 캐시 때문에 그 팔은 **애초에 발화하지 않는다**. **정직하게 등재** | **B** |
| **M5** | grave 호출부가 `NotFound`를 흡수 | ⚠ **증인 없음** — 창이 `spawn_blocking`의 **동기 컨텍스트** | **B-2** · F-35 |
| **M12 · M10 · M11 · M13 · M14~M27 · M19 · Mut-Count** | (기존 — raw `?` 복귀 · raw `DirEntry` 재보유 · 경로 재구성 · cfg-편향 · dead-helper · 위쪽 사다리 · temp 계수) | **회귀 증인 2개 · W2 · W6 · W11 · W13-E/G/T** | **A** |
| **M-LOG-DEBUG** *(★r20)* | skip 팔(Blob `read` `:252`)에 `tracing::debug!(entry = %name, …)` 추가 | **W-LOG-C뿐** — `left: ["DEBUG … skipping vanished entry entry=0957f0b8…"] / right: []`(실측 RED). ⚠⚠ **기존 스위트 123개는 전부 GREEN으로 살려 보낸다**(실측) ⇒ **W-LOG 없이는 무보호**. ⚠ **`CaptureSubscriber`를 *그대로* 쓰면 못 잡는다** — `enabled()`가 `level <= INFO`라 DEBUG를 버린다(실측) ⇒ `EventTap`은 **레벨-무관**이어야 한다(**B-5 diff 항목**) | **A** |
| **M-LOG-SUPPRESS-DOWNSTREAM** *(★r20)* | 격리 WARN을 `if vanished.get() == 0`으로 감싸 **하류 이벤트를 억누른다** | **W-LOG-B뿐**(실측 RED · A·C는 GREEN · 기존 스위트 123 GREEN) ⇒ **P16 ③(하류 도달성)의 유일한 자물쇠** | **A** |
| **M-LOG-DEBUG-TEMP** *(★r20 → **r26에서 봉인**)* | **Temp `metadata()` 팔(`:236`)에만** `tracing::debug!(entry = %name, …)` | ⚠⚠ r20 반증 실측에서는 **W-LOG-A/B/C 전부 GREEN · 전 스위트 `131 passed; 0 failed`로 생존**했다 — 하필 **fix-plan이 "증상 그 자체"라 부르는 Temp 분기**의 침묵이 **무보호**였다(`vanished_temp_regression.rs`는 행동만 보고 로그를 안 본다). **★r26 실측: 죽는다** — `passed=140 failed=1` · **킬러 = `w_log_d_…`(단독)** | **A** — ⚠ **W-LOG-D가 있을 때에만** |
| **M-LOG-INFO-GRAVE** *(★r20 → **r26에서 봉인 + 정정**)* | 무덤 루프 skip 팔에 `tracing::info!(…)`. ⚠ **r20의 지목(`:133`·`:154`)은 부정확했다** — `:133`은 **도달 불가**이고, 실제로 살아 있던 무보호 팔은 **`:149`(remove 분기)** 와 **`:154`(rename 분기)** 다 | ⚠⚠ r20 실측: **전 스위트 `131 passed; 0 failed`로 생존**(**INFO다** — `CaptureSubscriber`의 INFO 상한조차 필요 없었다. **아무도 그 무대를 안 밟았다**). **★r26 실측: 둘 다 죽는다** — `:149` → `passed=140 failed=1` · `:154` → `passed=140 failed=1` · **킬러는 둘 다 `w_log_d_…`(단독)** | **A** — ⚠ **W-LOG-D가 있을 때에만** |
| **★ M-LOG-ARM-\*** *(★r26 — **skip 팔 전수 뮤테이션**)* | **9개 `Seen::Gone` skip 팔 각각에** tracing 이벤트 한 줄을 넣는다(`:133`·`:149`·`:154`·`:227`·`:236`·`:244`·`:252`·`:257`·`:280`). ⚠ **`:244`는 r25까지 계획이 *열거조차 하지 못한* 팔이다** | **실행 원문(전 스위트 `cargo test --tests` · 뮤턴트당 1회)** — 기준선 `passed=170 failed=0`: <br>· `:133` → **SURVIVED** `170/0` (도달 불가 → **B-FT**) <br>· `:149` → **KILLED** `140/1` — `w_log_d_…` <br>· `:154` → **KILLED** `140/1` — `w_log_d_…` <br>· `:227` → **SURVIVED** `170/0` (도달 불가 → **B-FT**) <br>· `:236` → **KILLED** `140/1` — `w_log_d_…` <br>· **`:244`** → **KILLED** `140/1` — `w_log_d_…` <br>· `:252` → **KILLED** `138/3` — `w_log_b_…` · `w_log_c_…` · `w_log_d_…` <br>· `:257` → **SURVIVED** `170/0` (배리어 부재 → **B-QUAR**) <br>· `:280` → **KILLED** `140/1` — `w_log_d_…` <br>⇒ **밟을 수 있는 6개 전부 KILLED이고 킬러는 전부 W-LOG-D다**(`:252`만 B·C도 함께) | **A**(6) / **B**(3 — 아래) |
| **★ M-LOG-ARM-133 · -227** *(도달 불가 — Class **B-FT**)* | 무덤/엔트리 루프의 **`file_type()` `Gone` 팔**에 로그 | ⚠ **증인이 약한 게 아니라 그 팔이 *실행되지 않는다*.** **`continue` → `panic!` 프로브로 실증**(★r26): 전 스위트 × **3회** **`passed=170 failed=0`** ⇒ **어떤 테스트도 밟지 않는다**(d_type 캐시 — **W2가 `Present`를 직접 특성화**한다). **대조군이 프로브의 유효성을 증명한다**: 같은 프로브를 `:244`에 걸면 **3/3 REACHED**(`w_log_d_…`가 RED) ⇒ **프로브는 작동하고, 133·227은 정말로 죽은 코드다** | **B** (= 기존 **M-FT**의 두 얼굴) |
| **★ M-LOG-ARM-257** *(배리어 부재 — Class **B-QUAR**)* | **격리 `rename_into()`의 `SourceGone` 팔**(`:257`)에 로그 | ⚠ **프로덕션에서는 도달 가능**(TOCTOU)하지만 **결정적 무대를 지을 수 없다**: `e.read()`와 `e.rename_into()` **사이에 훅이 하나도 없다**(그 사이의 `mkdir_p_durable`도 훅을 부르지 않는다) ⇒ 배리어가 **없다**. 새 훅 = **프로덕션 변경**이므로 이 증분의 범위 밖이다. **실측**: `panic!` 프로브 × 3회 → **`passed=170 failed=0`** ⇒ **랑데부 통합 증인(Phase E)조차 밟지 못한다**(victim들은 전부 `:252`(`read`)에서 탈출한다) ⇒ **억지 증인을 만들지 않는다.** **보상 통제 = B-QUAR**(§5) + **F-43**(배리어 훅 신설 = 별도 증분) | **B** |
| **M-NOMOD** *(★r21/P-34 — **증인이 아예 컴파일되지 않는다**)* | **`src/store/pins/tests/`의 증인 파일을 만들고 `mod <name>;` 등록을 빠뜨린다**(또는 기존 `mod` 줄을 지운다). **Rust는 그 파일을 컴파일하지 않는다** ⇒ 그 안의 증인이 **조용히 사라진다** | ⚠⚠ **기존 스위트는 그것을 못 잡는다 — 실측**: `mod log_witness;` 한 줄 삭제 → `cargo test --lib --tests` **exit 0** · lib **132 passed → 129 passed; 0 failed** · 나머지 타깃 전부 `0 failed` · **경고 0**(파일이 컴파일되지 않으니 `dead_code`조차 없다) ⇒ **W-LOG-A/B/C가 통째로 증발하는데 스위트는 초록이다.** **킬러 = `scripts/f14-witness-gate.sh` ①**(실측: `MISSING WITNESS [lib] w_log_a…/b…/c…` · **DISCOVERY FAILED · exit 1**). ⚠⚠ **r21이 두 번째 킬러로 세운 W-REG는 폐기됐다 — 그것은 `#[ignore]` 한 줄로 꺼진다**(§B-1 0-e 실측: W-REG에 `#[ignore]` + `mod` 삭제 ⇒ **exit 0**) ⇒ **하네스 안의 검사는 킬러가 될 수 없다** | **A** — ⚠ **게이트 스크립트가 있을 때에만** |
| **M-NOMOD′** *(★r21)* | **신규 증인 파일을 아예 안 만들거나**(W-LOG-D · W10b · W11 · W13 …) 만들고 등록만 빠뜨린다 | **같은 킬러**(게이트 ①). **실측**: **정본 레지스트리 전행**(§0-b가 정본 · **실측 당시 35행**)을 증인 10개만 구현된 프로토타입에 돌리면 **미구현 24개가 전부 `MISSING WITNESS` · exit 1**. ⇒ *"차단 요건"*(W10b · W-LOG-D)이 **선언에서 그치지 않고 기계로 강제된다** — **0개 발견이 통과가 될 수 없다** | **A** |
| **M-IGNORE-1** *(★r22/P-36 — **증인을 등록한 채로 재갈을 물린다**)* | **증인 *하나*에 `#[ignore]`를 붙인다**(또는 `#[cfg_attr(…, ignore)]`). 테스트는 **여전히 컴파일되고 `--list`에 나온다** ⇒ **발견 단언을 통과한다** ⇒ 스위트는 **`0 failed; 1 ignored`**로 **초록으로 보인다** · **cargo exit = 0** | ⚠⚠ **r22의 계획은 이것을 못 잡았다** — `0 ignored`가 *산문*이었고 파싱해 실패시키는 명령이 하나도 없었다. **킬러 = 게이트 ②**(숫자 파싱): 실측 — W10에 `#[ignore]` ⇒ **DISCOVERY OK** · `131 passed; 0 failed; 1 ignored` · **cargo exit 0** · **게이트 exit 1**(`FAIL: ignored=1`). **소스 grep이 아니라 실행 결과의 숫자를 파므로** `cfg_attr`·매크로 판본도 함께 죽는다 | **A** — ⚠ **게이트 ②가 *숫자 파싱*일 때에만** |
| **★ M-IGNORE-10** *(★r23/P-37 — **게이트 자신의 회귀 증인**)* | **선언된 증인 *열 개*에 `#[ignore]`를 붙인다**(= 정본 레지스트리의 **통합 증인 수**이자, 프로토타입 lib 증인의 **전부**). 결과 줄이 **`10 ignored`**가 된다 | ⚠⚠ **r23의 파서(`grep -vc '0 ignored'`)가 이것을 통과시켰다 — 실측**: `10 ignored` ⊃ **부분문자열 `0 ignored`** ⇒ **위반 결과 줄 = 0** · cargo exit 0 ⇒ **게이트 PASS**. **회귀 ①·②(차단 증인 = `flips[]`)를 포함해 열 개를 전부 침묵시켰는데 게이트가 초록이었다.** (`20`·`100 ignored`도 같다.) **킬러 = 게이트 ②의 *숫자 파싱***(실측: `FAIL: ignored=10` · **exit 1**) **∧ `--selftest` (b)** — selftest가 *"옛 파서는 이것을 통과시킨다"*를 **단언으로 박아** 파서를 되돌리는 리팩터를 RED로 만든다 | **A** — ⚠ **숫자 파싱 + `--selftest`가 있을 때에만** |
| **M-FAILED-10** *(★r23/P-37 — **같은 함정의 다른 필드**)* | ② 가 `failed`를 **부분문자열로** 검사하도록 "대칭성을 맞춘다"(`grep -vc '0 failed'`) | ⚠ **r22/r23에는 이 코드가 *없었다*** — `failed`는 **아예 파싱되지 않았고** `suite_rc`에만 맡겨져 있었다(전수 확인) ⇒ **버그는 없었으나 필드가 무방비였다.** 넣는 순간 **`10 failed`가 통과한다**(실측: `grep -c '0 failed'` → **1**). **킬러 = 게이트 ②가 `failed`도 숫자로 검사** **∧ `--selftest` (d)** | **A**(예방) |
| **★ M-SIGPIPE** *(★r24/P-38 — **게이트가 존재하는 증인을 죽인다**)* | 발견 검사를 **파이프라인으로 되돌린다**: `has_witness()` → `cat "$1" \| grep -qE …` (= r23의 `list_for … \| grep -qE`). `set -o pipefail` 아래에서 `grep -q`가 **첫 매치에 종료**하면 상류가 **SIGPIPE**를 맞아 파이프라인이 **141**을 낸다 ⇒ **존재하는 증인이 `MISSING WITNESS`** | ⚠⚠ **r23의 selftest는 이것을 못 잡았다** — 발견 검사를 **아예 돌리지 않았다**. **킬러 = `--selftest` (h)**(조기 매치 + **1.1 MB** 목록 → 기대 **PASS**): 실측 — 정정본 **PASS** · 뮤턴트 **발견=FAIL** ⇒ **SELFTEST: FAIL · exit 1**. ⚠ **오늘의 목록(8.5 KB)에서는 발화하지 않는다**(0/30) — **잠복**이므로 selftest 픽스처가 **파이프 용량을 확실히 넘는 크기**여야 한다 | **A** — ⚠ **(h)가 있을 때에만** |
| **★ M-PRED-\*** *(★r24/P-39 — **술어별 삭제 뮤턴트**)* | 게이트 술어를 **하나씩 지운다**: **M-PRED-DISC**(`MISSING WITNESS`의 `bad=1`) · **M-PRED-LIST-RC**(`LIST FAILED`의 `bad=1`) · **M-PRED-N0**(결과-줄-0개 가드) · **M-PRED-IGN**(숫자 `ignored` 검사) · **M-PRED-FAIL**(숫자 `failed` 검사) · **M-PRED-RC**(cargo exit 검사). ⚠ **이 목록의 정본은 §0-h 매트릭스의 열이다**(★r25/P-40 — 여기서 술어 수를 세지 않는다) | ⚠⚠ **r23에서는 M-PRED-FAIL · M-PRED-N0가 살아남았다 — 실측**: 지워도 **`SELFTEST: PASS (6/6)` · exit 0**(둘 다 지워도 6/6) ⇒ **M-FAILED-10과 no-results 가드가 무핀이었다.** **원인 = 픽스처의 비직교성**((d)·(f)가 rc 101을 함께 넘겨 **PRED-RC가 기대 실패를 대신 공급**했다). **킬러 = 직교화된 selftest** — **각 술어가 정확히 한 케이스에 대응한다**(⚠ **술어→케이스 대응의 정본은 §0-h 매트릭스다 — 여기서 되풀이하지 않는다**). **살아남은 술어 0**(실측 — §0-h의 킬 표) | **A** |
| **삭제** | ~~M-PIN · M-FAILOPEN · M-NOODIR · M-NOIDENT · M-ATFDCWD · M-ORDER · M30 · M30′ · M32 · M33 · M33′~~ | **핀·정체성이 코드에서 사라져 *표현 불가*** | — |

### ⚠ 아무것도 안 죽이는 뮤턴트 — 정직하게 등재한다 (Class B)

| # | 왜 아무것도 안 죽나 | 보상 통제 |
|---|---|---|
| **M-GUARD-NODIR** | `.objects`가 일반 파일이면 **루프가 이미 ENOTDIR로 시끄럽다** ⇒ 가드에 닿지 않는다 | **B-6′** · B-5 diff 항목 |
| **M-B1** (`de.*` → 경로 stat) | 정상 경로 동일 | **B-5 diff 항목** |
| **M-FT** (`file_type()` raw `?`) · **M-LOG-ARM-133/227** | 그 팔이 발화하지 않는다 — **★r26에 `panic!` 프로브로 실증**(전 스위트 × 3회 `170 passed; 0 failed`). **대조군**(`:244`에 같은 프로브 → **3/3 REACHED**)이 프로브의 유효성을 증명한다 | **B-FT**(§5) · B-5 diff 항목 |
| **★ M-LOG-ARM-257** *(★r26)* | **격리 `rename_into()`의 `SourceGone` 팔**(`:257`) — `read()`와 `rename_into()` **사이에 훅이 없다** ⇒ 결정적 무대 **구성 불가**. **실측**: `panic!` 프로브 × 3회 → 전 스위트 `170 passed; 0 failed` ⇒ **랑데부 증인(Phase E)조차 밟지 못한다** | **B-QUAR**(§5) · **F-43** |
| **M5** (grave 호출부 흡수) | 동기 경계 | `Absent` 타입 · W6b · **F-35** |

**5) ⚠ 잡을 수 없는 것 — 정직한 분류 + 보상 통제**

> **이것은 테스트가 아니라 *release gate의 anti-cheat diff 리뷰* · conductor `/code-review`가 막는다.
> 거짓 안심을 주지 않는다: 이 항목들에 대해 우리가 가진 것은 사람의 눈이지 초록 불이 아니다.**

| id | 무엇 | 등급 | 통제 |
|---|---|---|---|
| **B-ABA** *(인간이 채택한 포기 · **적대적 ABA**)* | **`.objects` 파괴 → 재생성**(동시 put의 `write_atomic`/`mkdir_p_durable` — `objects.rs:30,72` · 운영자 스크립트)이면 **두 얼굴로** 조용해진다: ① **엔트리 루프 안** — 가드가 **살아 있는 디렉터리**를 보고 `Ok` ② **무덤 루프 안** — `read_dir`이 `Ok(빈 dir)`를 주어 엔트리 루프가 0회 돌지만 **무덤 루프의 부재가 패스 집계를 올려 가드는 돌고**(r15/P-27 수리) **재생성된 살아 있는 dir**을 본다(§D-②). 정체성을 안 보므로 **원리적으로 못 잡는다** | **B** | **데이터 손실 0**(운영자가 이미 지운 것 외에는 — r14 반증이 **실행으로 확인**: 원장은 `try_exists` 정리로 **비어서** 발행되므로 이후 tombstone은 **full-grace로 재시작** = 보수적). 닫으려면 **핀/`(dev,ino)` = C안** ⇒ **인간 판정이 반려했다** ⇒ **닫지 않고 공개한다.** ⚠ **백로그 항목으로 신설하지 않는다**(§Follow-up의 F-42 판정) |
| **B′-SELFINVAL** | **격리 분기의 `mkdir_p_durable(&corrupt_dir)`가 `.objects`를 되살린다**(실측 T3). `read()` `Ok` **직후** µs 창에서 컨테이너가 죽으면 **우리 코드가 스스로 가드를 무효화**하고 `Ok`를 낸다 | **B** | 그 구간에 **훅이 없어 결정적 증인 불가**. **W10 계열 무대 규율**(비트로트 blob·`.corrupt`·동시 put 0)로 증인 쪽은 **구성상 봉인**. 닫으려면 격리 분기에 컨테이너 확인을 추가해야 하고 **그것이 새 `Err` 클래스 = 두 번째 플립** ⇒ **닫지 않고 공개한다** |
| **B″-SYNTH** *(★r14 신규)* | **가드의 `Ok(_)` 팔은 *무가공이 아니다*** — `io::Error::from(ErrorKind::NotADirectory)`는 `raw_os_error() = None` · `msg = "not a directory"`인 **합성 에러**이고, 오늘의 ENOTDIR은 `Some(20)` · `"Not a directory (os error 20)"`다 | **B** | **현실적 ABA에서는 도달 불가**임을 실행으로 확인했다(`.objects`가 일반 파일로 대체되면 **항목 연산이 먼저 ENOTDIR**을 내고 **B7이 무가공 전파**한다 — base·D안 **바이트 동일**). 도달하려면 *파괴 → 전 항목 skip → **그 뒤** 비-디렉터리로 대체*라는 **µs 창**이 필요하다. ⚠ **"가드 에러는 무가공"이라고 쓰지 않는다 — 그것은 거짓 안심이다** |
| **B-6′** | **M-GUARD-NODIR**(가드의 `is_dir()` 절 제거)에 증인이 없다 | **B** | B-5 diff 항목. ⚠ 정직: **가드의 세 팔 중 증인이 있는 것은 `Err` 팔(W10/W10-TEMP)과 `Ok(dir)` 팔(W10c)이고, `Ok(non-dir)` 팔은 무증인이다** |
| **★ B-QUAR** *(★r26 · **P16 ②가 덮지 못하는 유일한 *도달 가능* 팔**)* | **격리 `rename_into()`의 `SourceGone` 팔**(`reconcile.rs:257`)의 **침묵에 증인이 없다** — 거기에 tracing 이벤트를 넣는 뮤턴트가 **전 스위트를 통과한다**(★r26 실측: `170 passed; 0 failed`). **원인은 증인의 나태가 아니라 *배리어의 부재*다**: `e.read()`(`:251`)와 `e.rename_into()`(`:256`) 사이에 **발화하는 훅이 하나도 없고**(그 사이의 `atomic::mkdir_p_durable`도 훅을 부르지 않는다), 그 창은 **µs 단위**라 랑데부로도 못 밟는다 — **`panic!` 프로브 × 3회, `tests/reconcile_vanishing_entries.rs`의 Phase E(victim 16 × 6라운드 × 4스레드)를 포함한 전 스위트가 `0 failed`**(= 아무도 그 팔을 실행하지 않는다. victim들은 전부 **`:252`(`read`)** 에서 탈출한다) | **B** | ⚠ **P16 ②를 낮추지 않는다 — 커버리지의 한계를 *정확히* 적는다.** 닫으려면 **격리 분기에 10번째 훅**(`pre_quarantine_rename`)이 필요하고 그것은 **프로덕션 코드 변경**이다 ⇒ **이 증분의 범위 밖**(B-1은 훅을 9개로 동결했다) ⇒ **F-43**으로 백로그에 올린다. **보상 통제**: ⑴ **행동**은 이미 덮여 있다 — `rename_source_checked`의 `SourceGone` 팔 자체는 **W5a**(단위)와 **W9b**(목적지발 `NotFound`는 무가공)가 판다. 덮이지 않은 것은 **그 팔의 *로그 침묵*뿐이다** ⑵ **B-5 diff 항목**: *"`:257` 팔에 tracing 호출이 새로 생겼는가"*(전수 grep — **`debug!`/`trace!`는 크레이트 전체 0건**이므로 눈에 띈다) ⑶ **폭발 반경**: 이 팔에 로그가 새로 생겨도 **관측 행동만** 바뀐다(데이터 손실 0) |
| **★ B-FT** *(★r26 · **도달 불가 — 정직한 죽은 팔**)* | **`file_type()`의 `Gone` 팔 둘**(`reconcile.rs:133` 무덤 루프 · `:227` 엔트리 루프)은 **실행되지 않는다** — tokio/std가 readdir 청크에서 **`d_type`을 캐시**하므로 소멸한 항목에도 `Ok`가 난다 ⇒ 그 팔의 로깅 뮤턴트가 **전 스위트를 통과한다**(★r26 실측: `170 passed; 0 failed` — **그러나 이것은 증인의 실패가 아니다**) | **B** | **`panic!` 프로브로 *도달 불가*를 실증했다**(전 스위트 × **3회** · `0 failed`) ∧ **대조군**(같은 프로브를 **`:244`** 에 걸면 **3/3 REACHED** — `w_log_d_…`가 RED) ⇒ **프로브가 작동함을 증명한 위에서의 "도달 불가"다**(*"확인 안 함"이 아니라 "확인했고 없음"*). **W2**(`every_fs_method_reports_gone_after_the_entry_vanishes`)가 `file_type()`의 **`Present`를 직접 특성화**한다 ⇒ **d_type 캐시가 깨지는 플랫폼이 나타나면 W2가 RED로 알려 준다**(조용한 표류 불가). 기존 **M-FT**와 같은 뿌리 |
| **B-REFS** *(★r14 신규 · **기존 구멍** · 데이터 손실 클래스)* | **`refs`는 참조 집합이 아니라 *하계*다** — `collect_referenced`(`reconcile.rs:74-79`)가 포인터 read/parse 실패를 **조용히 삼킨다**(EACCES · EIO · **EMFILE**) ⇒ 살아 있고 참조된 blob이 **미참조로 보이고** 그 패스에 put이 없으면 `landed`도 비어 **두 술어가 모두 눈이 먼다** ⇒ grace 경과 후 **회수 → 영구 404** | **B** | **red.sha에서 바이트 동일하게 재현된다**(실측: 포인터 `0o000` → `pass1 referenced:0 · gc_pending:1` → `pass2 gc_deleted:1` → **GET 404**) ⇒ **D안이 만든 것이 아니다.** ⚠ **그러나 F-14가 GC를 되살리면 도달성이 급증한다**(오늘의 GC는 Temp 소멸로 사실상 죽어 있다) ⇒ **F-34 등급 상향.** ⚠ **EMFILE을 "비현실적"이라고 쓰지 않는다** — 이 리뷰가 P-14로 이미 심각하게 다룬 실패 클래스다 |
| **B-2 / F-35** | **M5**(grave 호출부의 `NotFound` 흡수) — 창이 `spawn_blocking`의 동기 컨텍스트라 `AsyncHook` 배리어 불가 | **B** | `Absent` 타입 · W6b · F-35(`SyncHook`) |
| **B-3** | `grave()`의 소스 경로가 `layout.rs::is_sha_name`(scope 밖)에 **암묵 의존**(lossy == raw) · 격리 rename(A2)의 µs 창 | **B** | B-5 diff 항목(*"`is_sha_name`이 여전히 64자 ASCII hex인가"*) |
| **B-1** | **프로파일·env 편향**(`cfg!(debug_assertions)`) | **B** | acceptance의 **`--release` 2줄**(`Cargo.toml`에 `[profile]` 오버라이드 **없음** 확인) |
| **B-TESTBRIDGE** *(★r15 · 정직)* | **`#[cfg(test)] Vanished::new_for_test()`가 `cargo test --lib`에서 타입 봉인을 연다** — `pins::tests`의 **9개 호출부**(§Scope)가 `&Vanished`를 요구하므로 **불가피**하다. ⇒ **뮤턴트가 평가되는 바로 그 빌드에서 M-FRESH가 컴파일된다** ⇒ *"M-FRESH는 컴파일 불가"* 는 **프로덕션 빌드에서만 참**이다 | **B**(타입) / **A**(행동) | **`W-GRAVE-CD-A`가 행동으로 죽인다**(실측 RED). **B-5 diff 항목**: *"`new_for_test`가 `#[cfg(test)]`이고 `pins.rs`의 **프로덕션 영역에서 0회** 등장하는가"* · *"`grep -c 'Vanished::new()' == 1`"*. ⚠ **`pub(crate) fn new()`로 넓혀 다리를 없애는 것은 반려한다** — 그러면 `pins::grave`의 M-FRESH가 **프로덕션 빌드에서도 컴파일된다**(반사실 `p27-b`: BUILD OK) ⇒ **P-27의 구멍이 그대로 부활**한다 |
| **★ B-GATESELF** *(★r23/P-37 · **★r24/P-38·P-39에서 재봉인**)* | **게이트는 증인을 감사하지만 아무도 게이트를 감사하지 않는다.** 실측 이력 — **P-34**(발견 단언이 없었다) → **P-35**(앵커가 통합 증인 10개를 거짓 MISSING으로 죽였다) → **P-36**(`0 ignored`가 산문이었다) → **P-37**(그 수리가 부분문자열이라 `10 ignored`를 통과시켰다) → **★P-38**(`pipefail`+`grep -q` ⇒ **SIGPIPE 141** ⇒ **존재하는 증인이 거짓 MISSING**) → **★P-39**(**그 selftest가 selftest가 아니었다** — (d)·(f)가 rc 101을 넘겨 **다른 술어가 기대 실패를 대신 공급** ⇒ `failed` 검사와 결과-줄-0개 가드를 **지워도 6/6 초록**). ⇒ **여섯 라운드 연속으로 "증인을 지키는 장치" 자신이 무증인이었다** — 그리고 P-39는 **그 보상 통제(selftest)마저 무증인이었음**을 보였다 | **B** | **★ 직교 `--selftest`가 보상 통제다**(**§0-h가 정본이다 — 이 칸은 케이스 수·술어 수를 반복하지 않는다** · ★r25/P-40): 픽스처 = **`(출력, rc)` 쌍** ⇒ **케이스 하나가 술어 하나만 죽인다.** **§0-h 매트릭스에 열로 선 술어 전부**(DISC · LIST-RC · N0 · IGN · FAIL · RC) **+ M-SIGPIPE + M-OLDPARSER에 대해 "지우면 selftest가 RED가 되는가"를 실행으로 확인**했다 — **살아남은 술어 0**(원문 = §0-h의 킬 표). ⚠⚠ **P-39의 교훈은 §0-h가 규칙으로 박아 두었다**(*"확인하지 않은 술어는 핀되지 않은 것이다"* — **여기서 되풀이하지 않는다**). ⚠ **정직한 잔여 — 아래 셋은 selftest가 원리적으로 못 잡는다**: ⑴ **입력의 진위**(②의 `cargo test --tests`를 `--lib`로 좁히기) ⑵ **레지스트리 내용**(행 삭제·개명·플랫폼 강등) ⑶ **★ 게이트 스크립트가 아예 실행되지 않는 경우** — **selftest도 게이트도 스스로 호출되지 않는다.** ⇒ **§0-g가 이 셋의 정본이다** |
| **B-5** | **증인 자체의 약화** | **B** | **릴리스 게이트가 diff로 반드시 확인할 항목**: **가드가 `write_atomic` *앞*인가** · **가드가 `vanished > 0`로 게이트되는가** · **가드가 `metadata`(follow) + `is_dir()`인가** · **`bump()`/`share()`가 `absence` 모듈 private인가** · **`bump()` 호출부가 `entry_is_absent{,_blocking}` *두 곳*이고 둘 다 살아 있는가**(⚠ **M-NOBUMP는 단일 지점이 아니다** — r15) · **`Vanished`에 derive가 0개인가** · **`Vanished`가 `crate::store::reconcile` 서브트리에 사는가**(`atomic.rs`로 되돌리면 봉인이 **조용히** 풀린다 — `p27-a` BUILD OK) · **`mod absence`가 private이고 재수출이 §A-0의 4심볼(`rename_durable_source_checked`·`Absent`·`Renamed`·`Vanished`)로 최소인가**(`pub(crate) mod absence`로 넓히거나 `entry_is_absent`를 재수출하면 `pins`가 **집계를 올릴 수 있는 자유함수**에 닿는다 — r16/P-28) · **`grep -c 'Vanished::new()' == 1`** ∧ **`new_for_test`가 `#[cfg(test)]`인가** · **W-GRAVE-CD 무대에 비예약 항목이 정확히 하나인가**(둘 이상이면 M-FRESH/M-FRESH′가 산다) · **`atomic.rs:51`의 `write_atomic` → `mkdir_p_durable(parent)` 첫 줄이 살아 있는가**(사라지면 W-GRAVE-CD-A가 조용히 무력화된다) · **`entry_is_absent`가 `symlink_metadata`(no-follow)인가**(P-1) · **`SourceGone`이 `std::fs::rename`의 `Err` 팔에서만 태어나는가**(`rename_durable` 융합에 붙이면 M6 부활) · **`Entry`에 `dir` 필드가 없고 경로가 `de.path()`에서만 나오는가**(M46) · **모든 FS 호출이 `de.*`를 경유하는가**(M-B1) · **W10 계열 무대에 비트로트 blob·`.corrupt`·동시 put이 없는가**(자기무효화) · **`MIN_STEPS_E/G/T`가 낮아지지 않았는가** · `adversarial.rs`의 **`.expect(…)`** · **`--release` acceptance 라인** · **`layout.rs::is_sha_name`이 여전히 64자 ASCII hex인가** · **`with_hooks`가 `#[cfg(test)]`인가** · **`pre_entry` seam 한 줄이 살아 있는가** · **★ W-LOG의 `EventTap::enabled()`가 레벨로 거르지 않는가**(INFO 상한이 남으면 `debug!` skip 로그가 **보이지 않는다** — 실측) · **`target` 필터가 skip 로그를 숨기지 않는가**(`files*` 밖 target으로 낸 로그 · `spawn_blocking` 스레드의 로그는 `set_default` 시야 밖이다) · **★ W-LOG-D의 무대가 여전히 6개이고 `:236`·`:244`·`:252`·`:280`·`:149`·`:154`를 *실제로* 밟는가**(★r26 — 하나라도 무대를 잃으면 그 팔의 로깅 뮤턴트가 **살아난다**. 확인법은 산문이 아니라 **실행**이다: 그 `continue`를 `panic!`로 바꿔 **W-LOG-D가 RED가 되는지** 본다) **∧ 무대 ②의 α·β가 *둘 다* 살아 있는가**(β를 지우면 α가 `:236`을 밟고 있어도 **조용히 초록**이다 — β가 `de.metadata()`의 **dir-fd 의미론**을 실행으로 붙잡는 유일한 자물쇠다) **∧ 무대 ⑤의 무덤 내용이 여전히 *쓰레기*인가**(자기 sha와 정합하게 바꾸면 *"remove 분기를 탔다"*의 양성 증거가 사라진다) **∧ 무대 ④의 `post_grave` 0회 단언이 살아 있는가**(`SourceGone` 팔의 직접 증거다) · **★ `:257`(격리 rename)과 `:133`/`:227`(file_type)에 tracing 호출이 새로 생기지 않았는가**(**B-QUAR · B-FT** — 그 셋은 **무증인**이므로 **diff가 유일한 방벽이다**. `debug!`/`trace!`는 크레이트 전체 **0건**이라 눈에 띈다) · **★ 증인 게이트(P-34 · **★r22/P-35·P-36에서 재작성**) 4종**: **(가) `pins.rs`의 `mod` 줄이 §Scope의 표대로 *전부* 살아 있는가**(`log_witness` · `vanished_container_witnesses` · `recover_graves_production_seam` · 기존 2개 — **한 줄만 지워도 그 파일의 증인이 조용히 사라지고 스위트는 `0 failed`다**) · **(나) `scripts/f14-witness-gate.sh`가 살아 있고 acceptance 0단계가 그것을 부르는가**(*게이트를 지우는 것이 가장 값싼 공격이다 — 스크립트가 없으면 아무 테스트도 RED가 되지 않는다*) · **(다) 레지스트리에서 *삭제된* ID가 있는가**(사라진 ID = 사라진 증인) **∧ 타깃·플랫폼 칸이 낮아지지 않았는가**(`all` → `unix` → `linux`로 낮추면 개발기 또는 CI 한쪽에서 조용히 빠진다) · **(라) 앵커가 여전히 `(^\|::)<id>: test$`인가**(`::<id>`로 되돌리면 **통합 증인 10개가 거짓 MISSING**이고, 그것을 잠재우려 목록을 깎는 순간 **증인이 정말로 사라진다** — P-35) **∧ ★② 결과 게이트가 여전히 *숫자를 뽑아 정수 0과 비교*하는가**(**`grep -vc '0 ignored'` 같은 *부분문자열* 매칭으로 되돌리면 `10 ignored`가 통과한다** — **P-37 · 실측**) **∧ `failed`·결과 줄 수·cargo exit을 함께 보는가** **∧ ★`--selftest`가 살아 있고 acceptance 0단계가 그것을 부르는가** **∧ ★ selftest가 §0-h가 정의하는 *모든* 케이스에 대해 PASS인가**(⚠ **케이스 수·술어 목록의 정본은 §0-h 매트릭스다 — 이 칸은 숫자를 반복하지 않는다**(★r25/P-40). **케이스가 하나라도 빠졌으면 계획 위반이다** — 특히 **(g)·(h)·(i)** 는 **수용된 P-38·P-39 봉인을 핀하는 유일한 케이스**이고, 빠지면 **목록-상태와 SIGPIPE 회귀가 미검증으로 나간다**) **∧ ★ §0-h의 술어 전부**(**DISC · LIST-RC · N0 · IGN · FAIL · RC**) **+ M-SIGPIPE + M-OLDPARSER가 여전히 뮤테이션-킬되는가**(*"각 술어를 지우면 selftest가 RED"* — **구현자가 실제로 돌려 실증**한다. 프로토타입 실측 **8/8 RED · 살아남은 술어 0** — §0-h의 킬 표가 원문이다) **∧ 옛 파서(`grep -vc '0 ignored'`)가 `old_parser()`로 살아 있고 (b)·(d)에서 *그것이 통과함*을 단언하는 회귀 핀이 남아 있는가**(**M-OLDPARSER** — 지우면 파서를 부분문자열로 되돌리는 리팩터가 조용히 통과한다) **∧ selftest의 `SELFTEST:` 요약이 케이스 수를 *세는가*(하드코딩이 아니라)**(P-40 — 박아 넣은 숫자는 다음 개정에서 표류한다) **∧ ②의 명령이 여전히 `cargo test --tests`인가**(`--lib`로 좁히면 통합 타깃의 ignored가 **보이지 않는다** — selftest는 이것을 못 잡는다) |
| **B-IGNORE** *(★r21 → **r22/P-36에서 봉인**)* | **`#[ignore]`가 붙은 증인은 여전히 *등록되고 `--list`에 나온다*** ⇒ **발견 단언을 통과한다**. 스위트는 **`0 failed; N ignored`** 를 내고 **초록으로 보인다** — 실측: `--list`에 **그대로 등장** · 발견 단언 **DISCOVERY OK** · **`cargo test --lib` exit 0 · `131 passed; 0 failed; 1 ignored`**. ⚠⚠ **r21은 보상 통제로 *"acceptance가 `0 ignored`를 게이트한다"*고 적었지만 그것은 *산문*이었다 — 파싱해 실패시키는 명령이 하나도 없었다**(P-36) | **A** *(구 **B**)* | **봉인됨 — 킬러 = `scripts/f14-witness-gate.sh` ②**: 전 스위트(`cargo test --tests` = lib·bins·통합 전부)를 돌려 **모든 `test result:` 줄에서 `ignored`·`failed` 수를 뽑아 합산하고 *정수 0과 비교***한다(+ 결과 줄 수 · cargo exit). **⚠⚠ *부분문자열 매칭이 아니다* — r23이 그것으로 썼다가 `10 ignored`를 통과시켰다**(P-37 · 실측: 증인 10개 전부 재갈 · **옛 게이트 exit 0**). **실측(새 파서)**: 1개 재갈 → `FAIL: ignored=1` · **exit 1** / 10개 재갈 → `FAIL: ignored=10` · **exit 1** / 원복 → **exit 0**. **실행 결과의 숫자를 파므로** `#[cfg_attr(…, ignore)]`·매크로 판본도 함께 죽는다(소스 grep은 그것을 놓친다). **거짓 불변식이 아니다**: 저장소·프로토타입 전체의 기존 `#[ignore]` = **0건**(실측). **잔여**: W5e′·W6b(root 프로브) · W17(`cfg` 게이트)의 skip은 **`#[ignore]`가 아니다** ⇒ 충돌하지 않는다. **파서 자체의 퇴행**은 **`--selftest`(b)** 가, **스크립트의 삭제·약화**는 **B-5 diff 리뷰**가 막는다 |
| **B-DISCOVERY** *(★r21 · **게이트가 못 잡는 것 — r22에서 갱신**)* | **② 이름 변경**: 증인을 개명하고 **레지스트리까지 같이 고치면** 게이트는 통과한다(그것이 *의도된* 동작이다 — 스크립트가 정본이므로). 게이트가 강제하는 것은 **"레지스트리와 바이너리가 일치한다"** 이지 *"증인의 내용이 여전히 강하다"* 가 **아니다.** **③ 빈 본문**: `mod`도 있고 이름·타깃도 맞는데 본문이 `assert!(true)`면 **게이트는 통과한다** — **발견은 *존재*를 증명할 뿐 *내용*을 증명하지 않는다.** **④ 플랫폼 강등**: `all` → `unix` → `linux`로 낮추면 그 그룹 밖에서 **조용히 빠진다**(W17이 정확히 그 경우다 — B-12). **⑤ 레지스트리 행 삭제**: 지우면 그 증인은 **존재하지 않는 것이 된다** | **B** | **내용은 뮤턴트 표가 맡는다**(게이트는 **뮤턴트 표의 전제**를 지킬 뿐이다 — *"증인이 컴파일되고 등록됐고 재갈이 물리지 않았다"*). **B-5 diff 항목**: *"개명된 증인의 본문이 여전히 같은 것을 단언하는가"* · *"레지스트리에서 **삭제된** 행이 있는가(있다면 그 증인은 어디로 갔나)"* · *"플랫폼 칸이 낮아진 행이 있는가"* |
| **B-12** | **W17이 개발기(APFS)에서 안 돈다** | **B** (Linux: **A**) | CI·홈랩 = Linux · **B-5 diff 항목** |
| **F-41** *(**기존 구멍** · **F-14와 인과 없음**)* | **핀을 거치지 않고 만들어진 커밋 포인터**는 `refs`(수집 **이후** 생성)에도 `landed`(**핀이 없다**)에도 잡히지 않는다 ⇒ 만료 tombstone을 가진 blob이 **회수 → 영구 404**. 도달 경로 = **① 같은 데이터 루트의 두 번째 프로세스/레플리카**(D-3 · → F-32) **② 운영자의 수동 `.meta.json` 복원** | **B** | ⚠ **이 픽스가 만든 손실이 아니다** — **대조군**(사라진 항목 **0개** = 오늘의 코드)에서 **똑같이 죽는다**(`gc_deleted: 2` · `S GET = Err(NotFound)`). 홈랩은 **단일 replica + RWO PVC** ⇒ ①은 현재 닫혀 있다. 증거: `docs/reviews/reconcile-vanished-entry-aborts-pass/evidence-p21-refutation.md` ⇒ **F-41**(백로그) |
| **C-1** *(기존 해저드 · **픽스 이후 도달성 급증**)* | **아웃오브밴드 복원 + 만료 tombstone** — 복원된 blob이 **옛 스냅샷에 없었으면** 아무도 못 막는다 | **B** | **이 픽스가 만든 경로가 아니다**(오늘도 소멸 항목이 없는 패스는 완주하며 똑같이 회수한다). ⚠ 픽스가 GC를 되살리면 **실무적으로 새로 도달 가능해진다.** **근본 해결은 tombstone에 blob의 *관측 세대*를 묶는 별도 파이프라인** → **F-41이 흡수한다** |
| **W6b · W5e′ (root)** | root CI에서는 권한 검사가 우회되어 전제가 사라진다 | **B** | 프로브 후 **명시적 skip**(사유 출력 — 조용한 GREEN 금지) |
| ~~**C-2 · C-3 · C-4 · C-5 · B-16 · B-7~B-15**~~ | ~~핀 순서(M-ORDER) · virtiofs 핀 기전 · fd +1 · fd 압박 밴드 · 정체성 위조~~ | — | **소멸 — 핀이 코드에서 사라져 전부 표현 불가.** ino 재사용 계측의 요약(한 줄 잔존): *ext4 199/199 · xfs distinct 64/200 · virtiofs·APFS·btrfs 0/199 — **D안은 정체성을 보지 않으므로 이 수치가 어떤 판정에도 들어가지 않는다.*** |

**6) `tests/adversarial.rs` 강화 — 눈가리개 제거 (단조: 제거된 단언 0)**

`concurrent_nested_puts_with_reconcile_loop_preserve_all`의 **`:89-92` 한 곳뿐**이다 —
`let _ = reconcile::run_once(…).await;` → `reconcile::run_once(…).await.expect("PASS ABORTED — …")`.
이 루프는 동시 put의 `.tmp-<uniq>` → `<sha>` rename과 **매 실행 경합한다**. 결과를 버리면 패스가 `Err`로
중단돼도 테스트는 초록이다 — **그것이 이 버그가 살아남은 구조적 이유다.**

**anti-cheat 논증**: ① 패닉 배선은 **이미 있다**(`rec.await.unwrap()` `:112`) ⇒ **새 배선 0** ·
② `let _ = e;`와 `e.expect(m)`은 **똑같이 평가**하고 후자만 `Err`에서 패닉한다 ⇒ **제거된 단언 0 · 삭제된
테스트 0 · `#[ignore]` 0 · 실패 모드 +1** · ③ **락과 무모순**(`--verify-flip`은 각 sha의 **자기 트리**에서
돈다) · ④ **B4 표면 밖**(`isTestPath`) · ⑤ `pins.rs` 봉인 체크리스트 **10번**(*"`let _ = <async fn>(..)`
금지"*)의 정확한 위반을 고치는 것 · ⑥ 회귀 증인을 **대신하지 않는다**.

**정직한 잔여**: `.expect()`는 이 테스트를 **FS 민감**하게 만든다 — `CommitPointerWalk`(`layout.rs:294,312`,
**scope 밖**)가 `DT_UNKNOWN` FS에서 같은 ENOENT 중단을 낼 수 있다(→ **F-31**). **W13은 포인터를 만들지
않으므로 이 민감도가 없다** ⇒ **두 증인이 서로의 약점을 덮는다.**

**7) 문서·계수 개정** (**빠뜨리면 코드와 문서가 어긋난 채 머지된다**)

> ★ **훅 계수(8 → 9) 개정 위치의 정본은 이 표다**(★r25/P-40 — SSOT) ⇒ 다른 곳은 *"9번째 훅을 신설한다"*
> 는 **사실**만 적고 **위치를 열거하지 않는다.** ⚠ **red.sha에 이미 스테일 "7개"가 2곳**(7-c) — **같은 표류가
> 코드에서도 일어났다는 증거다.**

| # | 대상 (red.sha 기준 grep 실측) | 무엇을 |
|---|---|---|
| **7-a** | **`src/store/pins.rs:62`** — 현재 `/// 필드는 정확히 8개다. 늘리지 마라.` | → **`/// 필드는 정확히 9개다.`** + **9번째 훅이 왜 봉인을 깨지 않는지**(프로덕션 `None` ⇒ no-op · `AsyncHook`은 `()`를 반환)를 doc으로 박는다 |
| **7-b** | *"8개"* → *"9개"*: `pins.rs:1277` · `pins.rs:2777` · `reconcile.rs:283` · `reconcile.rs:291` | 파생 계수 4곳(**코드는 안 바뀐다 — doc만**) |
| **7-c** | ⚠ **red.sha에 남은 *스테일* "7개" 2곳**: `pins.rs:2297` · `pins/tests/vanished_entry_regression.rs:43` | *"7개"* → *"9개"*. **red.sha가 8번째 훅을 열면서 놓쳤다 — 숨기지 않는다** |
| **7-d** | **`docs/adr/0002-…md`** 봉인 체크리스트 | **`pre_entry`(8번째)·`pre_recover_grave`(9번째)의 존재**와 **P4 봉인 불변 논증**(P14)을 **기록**한다. ⚠ ADR은 훅 필드를 **세지 않는다** ⇒ **정정이 아니라 추가**다 |
| **7-e** *(★r22)* | **`scripts/f14-witness-gate.sh`** 신설 + **`bugfix-lock.json`의 `scope[]`에 그 정확 경로 추가** | 증인 레지스트리의 **단일 권위**. ⚠ **스크립트는 이식 커밋에서 태어나고**, `scope[]` 개정은 **지휘자가 한다**(아래) |

⚠⚠ **`bugfix-lock.json`의 `scope[]` 개정은 지휘자가 한다 — ★r22에서 항목이 하나 늘었다**:
**`"scripts/f14-witness-gate.sh"`**(정확 경로 · 와일드카드 아님).
`red.sha`(`ac58bd7`) · `flips[]`(2행) · `characterizationCmd` · `regressionCmd`는 **동결**이다.

---

## W13 — `tests/` 통합 증인 (W8/W12의 대체물)

> **가드의 영향: 없다.** W13의 세 페이즈는 **`.objects`를 지우지 않는다**(항목만 지운다) ⇒ 루프-후 가드는
> `metadata` = `Ok(dir)`를 보고 **통과**한다(게다가 `vanished == 0`인 라운드에서는 **돌지도 않는다**) ⇒ 모든
> 단언·항등식·`MIN_STEPS_*`가 **불변**이다. 같은 이유로 기존 증인·characterization 138개도 **무영향**이다
> (저장소 전체에서 `.objects`를 삭제·이름변경·chmod하는 테스트는 **0건** — grep 확인).

**W13-0. 핵심 통찰 — `tests/`는 `cfg(test)` *없이* lib를 링크한다** (스크래치 크레이트로 확정:
`tests/it.rs`에서 `cfg!(test) = false`) ⇒ **통합 증인은 모든 조건부 뮤턴트의 *프로덕션 팔*을 탄다**:
`if cfg!(test) {올바름} else {legacy}` · **훅-존재 가드**(`with_hooks`가 `#[cfg(test)]` ⇒ `tests/`는 훅을
**심을 수 없다** ⇒ legacy 팔 — **W11이 자기를 눈멀게 하던 성질의 역이용**) · `#[path]`/`include!`/스캔 밖
파일(**컴파일된 산출물의 행동**을 보므로 미끼가 무의미) ⇒ 전부 **RED**. **남는 편향 술어는 정확히 하나**
(`cfg!(debug_assertions)`/프로파일/env) ⇒ **보상 통제 = `--release` 실행**(**B-1**). **실증**:
`NotFound => continue`에 `&& cfg!(test)`를 붙인 뮤턴트 8곳 → `cargo test --lib` **118 passed** / 통합 증인
**3 FAILED**.

**W13-1. 위치·가시성.** 신규 `tests/reconcile_vanishing_entries.rs` — 독립 바이너리, 3함수(Phase E/G/T).
**프로덕션 공개 API만** 쓴다(`Store` · `reconcile::run_once` · `ReconcileStats`). `run_once_at`·`Hooks`·
`with_hooks`는 **부를 수 없다 — 그것이 약점이 아니라 무기다.**

**W13-2. 랑데부 — 훅 없이 "스냅샷 이후"를 만든다(E·T에 한함).** 프로덕션이 스스로 만드는 **온디스크
관측치**를 쓴다: `.corrupt/<name>` 등장(⇒ 엔트리 루프가 **이미 돌고 있다**) · `.gc-grave-*` 개수 감소
(⇒ 복구 루프가 돌고 있다). **절차**: 카나리아·victim을 **spawn 전에** 심고 → 패스 spawn → 관측치까지
busy-spin(유한 deadline) → **아직 디스크에 있는 victim을 지운다** → join → `Ok(stats)` + 사후-디스크 항등식
+ 전수 `assert_eq!` + 자기검증 하한.

> ⚠ **W13-2-G. Phase G는 이 랑데부를 *쓰지 않는다* — 결정적 훅 park로 재작성됐고 lib로 옮겼다.** 구 통합
> Phase G는 `.gc-grave-*` 감소를 busy-spin으로 기다리는 **동시성 랑데부**였는데, 그 조율이 신뢰성이 없어
> green.sha에서 **5/5 결정적 RED**(`K_KEEP의 무덤이 남아 있다` · 하이젠버그: 계측을 붙이면 초록으로
> 뒤집혔다)였다. ⇒ **9번째 훅 `pre_recover_grave`** 로 **결정적 park**를 걸어 랑데부를 대체한다(SPIN·`ROUNDS_G`
> ·`MIN_STEPS_G` 없음). 훅은 `PassGuard::begin → recover_graves`라는 진짜 프로덕션 경로에서 무덤 항목 하나당
> 정확히 한 번 발화한다. 훅을 심는 `Store::with_hooks`가 `#[cfg(test)]` **crate-private**이라 통합 바이너리로는
> 닿지 못한다 ⇒ **lib 테스트**(`src/store/pins/tests/recover_graves_production_seam.rs`)로 이전하고 게이트
> 레지스트리·증인 ID 표를 **같은 커밋에서** 갱신했다. E·T 두 페이즈는 랑데부 그대로 `tests/`에 남는다.

> ⚠⚠ **회계는 *우리 `remove_file`의 반환값*이 아니라 *패스 종료 후 디스크 상태*로 한다**(실측이 강제했다 —
> 초안대로 짠 Phase E는 `--release`에서 **~6% RED**였다: 우리의 `unlink`와 패스의 격리 `rename`이 **둘 다
> 성공할 수 있다**). **수리**: `escaped = { v ∈ VICTIM : .corrupt/v 부재 }` ⇒
> `quarantined == CANARY + (VICTIM_BLOB − ‖escaped‖)`. **unlink vs unlink 짝만 반환값을 그대로 써도 된다.**
> ⚠⚠ **심는 순서가 load-bearing이다**(tmpfs·ext4는 **삽입 순서**로 돌려준다) ⇒ **카나리아 ≺ victim**(E) ·
> **카나리아 ≺ ballast ≺ temp**(T). 그래서 **`MIN_STEPS_*`가 반드시 함께 있어야 한다**(못 밟으면 **RED로
> 소리친다**).

| Phase | 무대 | 단언 (+ 자기검증) |
|---|---|---|
| **E** (엔트리 루프) | `BALLAST` 48 × 256 KiB · `CANARY` 4(비트로트 = 루프 진입 신호) · `VICTIM_BLOB` 16(비트로트) · `VICTIM_TEMP` 8 · `ROUNDS_E = 6` · `gc_grace = 3600` · **포인터 0개**(⇒ **F-31 도달 불가** ⇒ W13의 GREEN은 **FS 무관**) | `Ok` · `quarantined == CANARY + (VICTIM_BLOB − ‖escaped‖)` · `gc_pending == BALLAST` · 전수 `assert_eq!` · `.gc-pending.json` 파싱 성공 · **`Σ‖escaped‖ ≥ MIN_STEPS_E (=6)`** |
| **G** (`recover_graves`) *(★재작성 · **lib 이전** — 아래 W13-2-G 참조)* | `R = 3`(정본 blob **부재** ⇒ **rename 분기**) · `K_KILL = 2` + `K_KEEP = 2`(정본 blob **무손상** ⇒ **remove 분기**) · **K의 무덤 내용 = *쓰레기***(정본 sha와 다르다) · **결정적 훅 park**(9번째 훅 `pre_recover_grave` — 랑데부·busy-spin·`ROUNDS_G`·`MIN_STEPS_G` **없음**): 첫 발화에서 park ⇒ 프로덕션은 grave[0] 파일 연산 **직전**에 서고 스냅샷은 고정·모든 무덤이 디스크에 있다 ⇒ park 창의 삭제는 **readdir 순서 무관 100% 결정적**. park 중 **`K_KILL` 무덤 전부 + `R` survivor 뺀 나머지를 우리가 `remove_file`** — ⚠ **`K_KEEP` 무덤은 *절대* 건드리지 않는다** | `Ok` · 전수 `assert_eq!`(`gc_pending == K_KILL + K_KEEP + 1`(복원된 R survivor) · `quarantined == 0` · 나머지 0) · **훅이 무덤 *전부*에 발화**(= 루프 완주 — `file_type()` 캐시가 소멸 무덤에도 `Ok`를 줘 삭제된 것도 발화) · **`K_KEEP` 무덤 *전부* 소멸**(우리가 안 건드렸다 ⇒ 없앨 수 있는 건 **프로덕션의 remove 분기뿐** = **날조 불가능한 프로덕션 증거** = **M-REMOVE-NOOP 킬**: 무덤 루프를 no-op으로 만드는 뮤턴트는 여기서 `K_KEEP` 무덤이 남아 **RED**) · **K 정본 *바이트 동일***(remove는 무덤을 *지우기만* 한다 ⇒ **rename으로 잘못 가는 뮤턴트는 쓰레기 무덤을 정본에 덮어써** 바이트 동일성·`quarantined==0`이 **둘 다 RED**) · **R survivor는 rename 분기로 정본 복원** · 지운 R은 **escaped**(정본·무덤 둘 다 부재) · 무덤 잔재 0. ⚠ **결정성**: green.sha에서 **20/20 GREEN**(구 통합 랑데부 무대는 5/5 RED 하이젠버그였다) |
| **T** (temp 삭제) | `gc_grace = 0` · **CANARY(4) ≺ BALLAST_T(32) ≺ TEMPS(16)** · `ROUNDS_T = 3` · **벽시계 슬립 0**(mtime 백데이트) | `Ok` · **`temps_deleted == TEMPS − stepped_t`** · `quarantined == CANARY` · 전수 `assert_eq!` · **`Σ stepped_t ≥ MIN_STEPS_T (=1)`** ⇒ **`Mut-Count` 킬**(뮤턴트는 사라진 temp도 센다) |

> ⚠ **정직 — Phase E는 Temp 창(`Entry::metadata()`)을 자기검증하지 못한다**(`gc_grace = 3600`). **그 창을
> 자기검증하는 유일한 페이즈는 Phase T**다 — 그리고 **계측상 프로덕션에서 실제로 발화하는 F-14 지점이
> `metadata()`**다(태그된 버그 코드로 12라운드: 관측된 Err **22건 전부 `E_TEMP_META`**).

**W13-3. 결정성.** `Err`를 볼 수 있는 `?`를 전수 열거하면 **전부 양성**이다: `try_exists(.objects)`(지우지
않는다) · `read_dir`/`next_entry`(항목 변경은 getdents 에러가 아니다) · `collect_referenced`(포인터 0개) ·
`mkdir_p_durable`/`fsync_dir`/`write_atomic`(`.corrupt`/`.objects`를 지우지 않는다) · **루프-후 가드**
(`.objects`가 살아 있다 ⇒ `Ok(dir)`) · `try_exists(blob_path)`(부재 = `Ok(false)`) · **소멸 항목**(= 정확히
픽스가 흡수하는 것). **실증**: 진짜 버그 코드에서 Phase E/G/T **3/3 모두 `PASS ABORTED`**. 실행 ≈ **1 s**.

---

## 락 무결성 논증 (F-1 ~ F-5)

**F-1. `flips[]`는 2행에서 동결이다.** W13·W10-G·W10c를 `flips[]`에 **넣지 않는다** — `red.sha = ac58bd7`이
**동결**이고 그 트리에 그 파일들이 **존재하지 않는다** ⇒ 그 sha에서 **RED verify-record를 만들 수 없다**.
⚠ **정직한 구별**: W10-G·W10c·W11은 red.sha에서 **RED이거나 컴파일조차 불가**하다. 그것은 **두 번째 플립이
아니라 *같은 하나의 플립*의 추가 증인**이다(하드룰 10 명시 허용 — `flips[]`의 2행이 이미 같은 근거로 선다).

**F-2. `characterizationCmd`는 red.sha에서 GREEN인 채로 남는다 — 문자열도 결과도 불변.**
`bugfix-status.mjs`의 `runInWorktree`(:549-566)는 **그 sha의 트리에서** 커맨드를 실행한다 ⇒ red.sha 트리에는
여전히 `let _ =`가 있고 새 파일이 **없다** ⇒ 이미 커밋된 RED verify-record가 **그대로 유효**하다.
⚠ **그러나 `.expect()`는 characterization을 `src/layout.rs`(scope 밖)에 FS-의존하게 만든다** —
`CommitPointerWalk::step`이 `entry.file_type().await?`를 **이름 필터 이전에** 부른다 ⇒ `DT_UNKNOWN` FS에서
동시 put의 소멸 포인터에 걸릴 수 있다. 개발기·CI·홈랩 PVC는 **전부 `d_type`을 채운다**(계측: 관측된 Err
22건 전부 `E_TEMP_META` · `[[WALK]]` **0건**) ⇒ **F-31**이 이 민감도를 없앤다.

**F-3. B4 표면 바운드.** `isTestPath`(`bugfix-status.mjs:89`)가 `^tests/`를 테스트 경로로 판정한다 ⇒
`tests/adversarial.rs`와 신규 `tests/reconcile_vanishing_entries.rs` **둘 다** `scopeViolationsOf`에서
필터아웃된다 ⇒ **비-테스트 표면 밖 · `scope[]` 편집 0**(`docs/adr/**` 추가는 별개 사유).

**F-4. 하드룰 10(단일 플립) 불변.** 두 `flips[]` 행은 **같은 하나의 관측 행동**에 대한 두 증인이고
symptomToken을 공유한다. **`ReconcileStats` 필드 추가 0 · B7 불변 · O1/O2 순서 불변.**
**F-5.** acceptance에 추가되는 명령은 **문서 레벨**이다 — `regressionCmd`·`characterizationCmd`는 **B-1이
고치지 않는다.**

---
## Scope

- **비-테스트 변경 = `src/store/**`의 5파일. 파일별로 *무엇을 하는지*까지 못박는다**(신설 / 수정 /
  **가시성만**):

  | 파일 | 이 픽스가 하는 일 |
  |---|---|
  | **신규** `src/store/reconcile/absence.rs` | **부재 관련 심볼을 전부 여기에 *신설*한다**: `Absent`(필드 `()` private) · `Vanished`(**derive 0** · `new`/`get` = `pub(super)` · `bump`/`share` = 모듈 private · `#[cfg(test)] pub(crate) new_for_test`) · `entry_is_absent{,_blocking}` · `Renamed` · `rename_checked_blocking` · `rename_{source,durable_source}_checked`. ⚠ **`atomic.rs`에서 *옮겨오는* 것이 아니다 — red.sha의 `atomic.rs`에는 이 심볼이 하나도 없다**(§A-0) |
  | **신규** `src/store/reconcile/entry.rs` | `Seen<T>` · `Entry<'v>`(`snapshot` + 6개 FS 메서드)를 **신설**(둘 다 `pub(super)`) — §구현 ② |
  | `src/store/reconcile.rs` (**수정**) | **`mod absence;`(private)** + **`pub(crate) use absence::{rename_durable_source_checked, Absent, Renamed, Vanished};`** — ⚠⚠ **r16/P-28: *"타입만 재수출"* 은 `E0425`다.** `pins::grave`가 쓰는 **자유함수 `rename_durable_source_checked`를 반드시 함께 재수출**한다(연관함수는 타입을 타고 따라오지만 자유함수는 아니다 — §A-0 실컴파일) · **`let vanished = Vanished::new()` = 크레이트 유일 호출부** · 엔트리 루프를 `Entry::snapshot`/`Seen`으로 감싼다(⚠ **`pre_entry` 한 줄은 *유지***) · **★ 루프-후 컨테이너 가드** · **`recover_graves(&layout, hooks, &vanished)`** |
  | `src/store/pins.rs` (**수정**) | `Grave` enum 신설 + **`grave(sha, &Vanished)`** · **`PassGuard::begin(store, settle, &vanished)`** · **`Hooks`에 9번째 필드 `pre_recover_grave`**(⚠ **8번째 `pre_entry`는 red.sha에 이미 있다 — 보존한다**) · **`use super::reconcile::{rename_durable_source_checked, Absent, Renamed, Vanished}`** — §A-0 |
  | `src/store/atomic.rs` (**가시성 1건. 그 외 무변경**) | **`fsync_dir_blocking`: 모듈 private → `pub(crate)`** — `absence.rs`가 rename+fsync를 **한 무취소 클로저**에 유지하려면 필요하다(M6 봉인). **본문·시그니처·시퀀스는 불변이고, 이 파일의 기존 API는 *전부 그대로 남는다***: `write_atomic` · `fsync_dir` · `mkdir_p_durable` · `rename_durable{,_blocking}` · `unique_suffix` · **F-1의 취소불가 커밋 파이프라인 `stage_blocking`/`Staged`/`Staged::commit_blocking`** · `mkdir_p_durable_blocking`. ⚠ **삭제 0 · 이동 0**(red.sha 호출부: `pins.rs:369-371` · `objects.rs:75` — 지우면 **컴파일이 깨진다**) |

  **전부 `src/store/**` 안**이다.
- ⚠⚠ **`begin`/`grave`의 호출부 전수 — 반드시 함께 고친다**(r15 적대적 반증이 실컴파일로 잡았다: 빠뜨리면
  **`cargo test`가 빌드조차 안 된다**):
  **`PassGuard::begin` = 8곳** — **프로덕션 1곳**(`reconcile.rs`의 `run_once_at` ⇒ `begin(store, settle, &vanished)`) ·
  **`pins::tests` 7곳**(`pins.rs:665·691·713·736·813·2536·2724`, 현재 `begin(&s, SETTLE)`) ⇒ **전부 3인자로**.
  **`grave` = 3곳** — **프로덕션 1곳**(`run_once_at` ⇒ `pass.grave(&name, &vanished)`) · **`pins::tests` 2곳**
  (`:2547`(T-B5③) · `:2728`) ⇒ `&Vanished` 추가 + `GraveOutcome::Moved` 언랩(단조 강화).
  **테스트 쪽 9곳**(`begin` 7 · `grave` 2)은 **`Vanished::new()`를 부를 수 없다**(`pins::tests`는
  `crate::store::reconcile`의 후손이 아니다 ⇒ **`E0624`**) ⇒ **`#[cfg(test)] pub(crate) fn Vanished::new_for_test()`
  다리가 필수**다(⚠ **연관함수는 타입 재수출을 타고 따라오므로 `absence` 모듈을 열 필요는 없다** — §A-0 실컴파일) ⇒
  **대가는 M-FRESH의 Class 강등**(**B-TESTBRIDGE**).
- **신규 외부 API = 0.** `nix`·`O_DIRECTORY`·`fstatat`·`AsRawFd`·`MetadataExt` **전부 불필요**(부재 판정이
  `symlink_metadata` 하나 · 가드가 `metadata` 하나) ⇒ **`Cargo.toml` 무변경 · `unsafe` 0.**
- **문서 변경 (⚠ `scope[]` 개정 필요 — `docs/adr/**`)**: ADR-0002 봉인 체크리스트에 **9번째 훅의 존재와
  P4 봉인 논증(P14)** 을 **기록**한다(정정이 아니라 **추가**).
- **테스트 변경 — ⚠⚠ 파일과 `mod` 등록을 *전수* 못박는다** (★r21/P-34. 이전 개정판은 **`log_witness.rs`와
  그 `mod` 줄을 빠뜨린 채** W-LOG-D를 *차단 요건*이라고 선언했다 — **선언만 하고 컴파일에 넣지 않았다**):

  > ⚠⚠ **`src/store/pins/tests/` 아래 파일은 자동 발견되지 않는다.** Rust는 **`mod` 선언이 있는 파일만**
  > 컴파일한다 — 파일을 만들고 `mod` 줄을 빠뜨리면 그 파일의 **모든 증인이 조용히 사라지고** 전 스위트가
  > **`0 failed`** 를 보고한다(**M-NOMOD** — 프로토타입 실측: `mod log_witness;` 한 줄을 지우면 lib
  > **132 passed → 129 passed; 0 failed** · `cargo` **exit 0**). **`tests/*.rs`(통합 바이너리)만 cargo가
  > 자동 발견한다** — 이 비대칭이 P-34의 뿌리다. ⇒ **§B-1 0단계의 `scripts/f14-witness-gate.sh`가 이것을
  > 죽인다**(★r22: **하네스 *밖*이라야 한다** — 크레이트 안의 검사(구 W-REG)는 `#[ignore]` 한 줄로 꺼진다).
  > ⚠ **그리고 `tests/*.rs`의 최상위 함수는 `--list`에 `::` 없이 나온다** ⇒ **앵커는 `(^|::)<id>: test$`**
  > 여야 한다(P-35 — 옛 `::<id>` 앵커는 **통합 증인 10개를 거짓 MISSING으로 죽였다**).

  | 파일 | 상태 | **`mod` 등록 (정확한 줄)** | 담는 증인 |
  |---|---|---|---|
  | `src/store/pins/tests/vanished_entry_regression.rs` | red.sha에 **이미 존재** | `mod vanished_entry_regression;` — **이미 있다**(`pins.rs`의 `#[cfg(test)] mod tests` 말미) | 회귀 증인 ①(Blob) + 대조군 ① |
  | `src/store/pins/tests/vanished_temp_regression.rs` | red.sha에 **이미 존재** | `mod vanished_temp_regression;` — **이미 있다** | 회귀 증인 ②(Temp) + 대조군 ② |
  | **신규** `src/store/pins/tests/vanished_container_witnesses.rs` | **신설** | **`mod vanished_container_witnesses;` — 신규 1줄** | W3 · W6 · W6b · W10 · W10-TEMP · W10-G · W10b · W-GRAVE-CD-A/B |
  | **신규** `src/store/pins/tests/log_witness.rs` | **신설** (★P-34가 빠뜨렸던 것) | **`mod log_witness;` — 신규 1줄** | **W-LOG-A/B/C/D** (`EventTap`) |
  | **신규** `src/store/pins/tests/recover_graves_production_seam.rs` | **신설** | **`mod recover_graves_production_seam;` — 신규 1줄** | W11 |
  | `src/store/reconcile/absence.rs` (**인라인**) | 신설 파일 안 | **`#[cfg(test)] mod tests { … }`** — 파일이 `mod absence;`로 이미 등록되므로 **추가 등록 없음** | W5(a~e′) |
  | `src/store/reconcile/entry.rs` (**인라인**) | 신설 파일 안 | **`#[cfg(test)] mod tests { … }`** — 〃(`mod entry;`) | W1 · W2 |
  | `tests/e2e.rs` (**수정**) | 기존 통합 바이너리 | **등록 불필요**(cargo 자동 발견) | W4 · W7 · W9a/W9b · W10c · W10c′ · W17 |
  | **신규** `tests/reconcile_vanishing_entries.rs` | **신설** 통합 바이너리 | **등록 불필요**(cargo 자동 발견) | W13 (Phase E/G/T) |
  | `tests/adversarial.rs` (**수정**) | 기존 | — | `let _ =` → `.expect(…)` |
  | **`src/store/pins.rs`의 `#[cfg(test)] mod tests`** | **수정** | ★ **위 3개의 `mod` 줄을 여기에 추가**(기존 2줄 옆). ⚠ **W-REG는 넣지 않는다 — r22/P-36이 폐기했다**(§B-1 0-e: 하네스 안의 검사는 `#[ignore]` 한 줄로 꺼진다) | — |
  | **신규** `scripts/f14-witness-gate.sh` (⚠ **비-테스트 경로**) | **신설** | — (셸 스크립트 · cargo와 무관) | **증인 레지스트리의 단일 권위** — 발견 단언 ① + `0 ignored` 게이트 ② (§B-1 0단계). ⚠⚠ **`scope[]` 개정 필요** ↓ |

  ⚠ **`pins::tests`의 9개 호출부**(위 `begin` 7 · `grave` 2)도 함께 고친다.
  ⚠ **`src/store/pins/tests/log_probe.rs`(프로토타입의 S1~S5 진단 프로브)는 이식하지 않는다** — 증인이
  아니라 계측 도구다(r20이 *"프로브 수치는 계획에서 내린다"*고 판정했다).
  **red.sha의 두 회귀 증인은 RED → GREEN으로 뒤집힐 뿐 코드가 바뀌지 않는다.**
- **프로덕션 공개 표면 확대 0**(**전수 맵 = §A-0**): `Absent`/`Vanished`/`Renamed`/`entry_is_absent*`/
  `rename_*_source_checked`/`Grave`는 `pub(crate)` · **`Vanished::{new,get}`는 `pub(super)`**(= `pub(in …::reconcile)`
  — **부모는 `reconcile`이지 `store`가 아니다** ⇒ `pins`에서 **`E0624`**) · `bump`/`share`/`entry_is_absent_blocking`/
  `rename_checked_blocking`은 **absence 모듈 private** · `Seen`/`Entry`/`recover_graves{,_from}`은 `pub(super)`
  (`reconcile.rs`의 `pub(super)` = **`store`** ⇒ **`pins`가 `recover_graves`를 부를 수 있다** — 의도된 것이다).
  ⇒ **`pins`는 집계를 *짓지도 읽지도 올리지도* 못하고 오직 *빌려서 전달*만 한다**(실컴파일: `new` `E0624` ·
  `get` `E0624` · `Absent(())` `E0423`) ⇒ **라운드 16의 봉인이 `pins`에서 그대로 성립한다 — 모순 없다.**
  `ReconcileStats` **필드 추가 0**. 증분은 **B-1 단일**.

---

## 정직한 부수 행동 변화 (관측 계약 밖 — 그래도 적는다)

1. **로깅 — *호출부는 불변, 런타임 스트림은 아니다*** (★r20/P-33 · **로그 스트림을 실제로 캡처해 측정한 뒤
   썼다**). **신규 tracing 이벤트는 0이고**(skip 시 **침묵**) 기존 이벤트의 **호출부·레벨·target·필드는 한
   글자도 안 바뀐다**(발화 지점 **13곳 = 13곳** · 줄번호만 이동 · `debug!`/`trace!`는 크레이트 전체 **0건**).
   **`ReconcileStats`도 불변**(P10). ⚠⚠ **그러나 *"로깅 행동이 동일하다"* 고 쓰지 않는다 — 그것은 런타임
   스트림에 대해 거짓이다**: 패스가 완주하므로 **기존 하류 이벤트가 새로 도달되고**(격리 WARN · `grave
   recovered` · **`graves recovered from a previous pass`** · GC 복원 INFO · settle 타임아웃 ERROR ·
   `main.rs:52`의 INFO `reconcile`) **오늘 매 패스마다 뜨던 기존 WARN(`main.rs:53` `reconcile failed`)이
   사라진다.** **7곳 · 두 방향 — 전수 목록은 §Single-Flip Contract의 "하류 범위" 표**이고, 그것은
   **단일 플립의 하류 결과**다.
   **★ 두 번째 플립이 없다는 것을 실측으로 못박는다 — S1(소멸 0)에서 로그 스트림이 *바이트 동일*하다**:
   red.sha와 D안 프로토타입에서 같은 무대(무덤 1 + 비트로트 1)를 돌려 이벤트 스트림을 캡처하면 **3 이벤트 ·
   레벨·target·메시지·필드·순서까지 동일**하고 **`diff`는 한 줄도 내지 않는다**(적대적 반증이 **독립 재현**:
   양쪽 S1 블록 md5 = `a97d09428200a406eb4dbc0c4b5c12b8`). **소멸이 0이면 로그는 오늘과 같다** — **그게
   아니었다면 그것이 두 번째 플립이었을 것이다.** 기존 `CaptureSubscriber` 4개 테스트(이벤트 **개수**를 정확히
   단언한다)도 **전부 GREEN**이다. 관측성 카운터/텔레메트리를 여는 일은 여전히 **별도 파이프라인**(→ **F-29**).
   *(**r19/P-32의 서술은 틀렸다** — *"스위트가 tracing subscriber를 설치하지 않아 증인이 원리적으로 없다"*는
   **확인 없이 단언한 거짓**이었다: `pins.rs:995-1037`의 `CaptureSubscriber`를 **4개 테스트가 설치한다**.
   ⇒ 증인 **W-LOG**를 세웠다.)*
2. **`Seen`/`Entry`/`Vanished`는 private이다**(`pub(super)`/`pub(crate)`) — 프로덕션 공개 표면은 한 글자도
   넓어지지 않는다(**증인 = §A-0의 실컴파일**: `pins`에서 `Vanished::new()`/`.get()` = **`E0624`** ·
   `Absent(())` = **`E0423`**). ⚠ **정직**: `#[cfg(test)] pub(crate) new_for_test` 다리는 **`cargo test --lib`에서만**
   그 봉인을 연다 ⇒ **Class B-TESTBRIDGE**(킬러 = **W-GRAVE-CD-A**).
3. **에러 경로에만 syscall이 는다** — `NotFound`가 났을 때에**만** `symlink_metadata(path)` 1회이고,
   **소멸이 1건 이상인 패스에서만** 루프 뒤 `metadata(objects)` **1회**. ⇒ **오늘 완주하는 모든 패스의
   syscall 트레이스는 *패스 전체까지* 바이트 동일하다**(P11 — C안이 포기했던 문언이 복원된다).
   **fd는 하나도 늘지 않는다.** (**증인 = W2 · W13의 전수 `assert_eq!`**.)

---

## 설계 논점에 대한 판정 (요약 — 근거는 위 본문)

| # | 논점 | 판정 |
|---|---|---|
| ① | `tests/adversarial.rs`의 `let _ =` | **벗긴다.** `.expect(...)`. 락과 무모순 · B4 테스트-경로 제외 · 단조적 강화 · 봉인 체크리스트 10번 준수. **`flips[]`에는 넣지 않는다** |
| ② | 잠복 범인 `file_type()` | **고치되 tokio의 `file_type()`을 그대로 쓴다** ⇒ 시점·`d_type` 캐시·`.ok()` 삼킴이 **정의상 동일** ⇒ **P-18은 존재하지 않는다.** ⚠ **정직**: `Gone` 팔은 **도달 불가**다 ⇒ **W2 명세에서 뺐고** raw `?`로 되돌리는 뮤턴트(**M-FT**)는 **Class B**다 |
| ③ | 좁은 창들(`remove_file` · 격리 `rename` · `grave` rename · `recover_graves`) | **전부 범위에 넣는다.** 근본 원인은 syscall이 아니라 **루프의 전제**이며, `Entry` 타입 아래에서는 **넣는 것이 빼는 것보다 작은 변경**이다 |
| ④ | 증분 분해 | **단일 증분(B-1)이 맞다** |
| ⑤ | `Entry` 래퍼를 없애고 분류 정책만 채택 | **부분 채택.** 분류 정책은 채택. **`Entry` 래퍼는 유지** — 제거하면 루프가 raw `DirEntry`를 다시 쥐고 **M10/M11이 되살아난다** |
| ⑥ | **A안(핀 + 손으로 짠 리더) · B안(값 포착) · C안(핀 + `DirEntry`) · **D안(핀 없음 + 루프-후 가드)** | **D안.** **핀·`fstatat`·컨테이너 정체성이라는 기계장치 전체가 오직 P5 하나를 위해 존재했다** — 그리고 **P5는 설계된 계약이 아니라 `?`의 부수효과**였다. 실험이 증명했다: **봉인 0**인 픽스로도 **120 passed / 0 failed · 새 데이터 손실 0**. D안은 **시끄러움을 값싸게 되찾는다**(루프-후 `metadata` 1회) — **현실적 파국은 여전히 시끄럽고**, 포기하는 것은 **적대적 ABA뿐이며 거기에도 데이터 손실이 없다** → Review Decision Log r14 |
| ⑦ | **`nlink > 0` · `is_dir()` · `de.ino()` 합취** | **셋 다 반려한다 — 되살리지 마라.** 근거는 §The fix 0의 "되살리기 금지 못"에 있다 |

---

## 남은 위험 (§5의 Class 표가 정본 — 여기는 색인이다)

**B-TESTBRIDGE**(테스트 다리가 `cargo test --lib`에서 타입 봉인을 연다 ⇒ **M-FRESH가 A(타입) → A(행동)** · 킬러 = **W-GRAVE-CD-A**) ·
**B-ABA**(적대적 ABA — 두 얼굴 · 손실 0 · 인간이 채택한 포기) · **B′-SELFINVAL**(격리 분기의
`mkdir_p_durable`가 가드를 무효화하는 µs 창 · 증인 불가) · **B″-SYNTH**(가드의 `Ok(_)` 팔은 errno 없는
합성 에러 — *"무가공"이라고 쓰지 않는다*) · **B-6′**(가드의 `Ok(non-dir)` 팔에 증인 없음) ·
**B-REFS**(`refs`는 하계다 — `reconcile.rs:74-79`의 조용한 삼킴 · **기존 구멍** · 픽스가 GC를 되살리면
도달성 급증 → **F-34**) · **B-2/F-35**(M5) · **B-3**(`is_sha_name` 암묵 의존) · **B-1**(프로파일 편향) ·
**B-5**(증인 자체의 약화 — ⚠ **가장 값싼 공격은 게이트 스크립트를 지우는 것이다**) · **B-12**(W17이 APFS에서
안 돈다) · ~~**B-IGNORE**~~(★r21 → **r22/P-36 · ★r23/P-37에서 봉인** — `#[ignore]`는 발견 단언을 **통과하지만**
`scripts/f14-witness-gate.sh` ②가 **`ignored` 수를 뽑아 정수 0과 비교**해 nonzero exit한다 ⇒ **Class A**.
⚠ **부분문자열 매칭이 아니다** — 그것으로 썼다가 **`10 ignored`를 통과시켰다**(P-37 · 실측)) ·
**★ B-GATESELF**(★r23/P-37 — **게이트 스크립트 자체가 결함을 가질 수 있다**: P-34→P-35→P-36→P-37로 **네
라운드 연속** · 보상 통제 = **`--selftest`** — 게이트가 자기를 증명한다) ·
**B-DISCOVERY**(★r21 · r22 갱신 — 게이트는 증인의 **존재**를 증명할 뿐 **내용**을 증명하지 않는다) ·
**F-41 · C-1**(기존 데이터 손실 구멍).

**§5에 없는 두 가지 — 여기에만 적는다:**

1. **재생성 레이스(확인 `symlink_metadata`의 창)** — `NotFound`(t1)와 확인(t2) 사이에 **같은 이름이
   재생성**되면 확인이 "존재"를 보고 **원본 ENOENT를 전파**한다 → **패스 중단 = 오늘의 행동**. 방향이
   **보수적**이다. ⚠ *"`.tmp-<uniq>`는 pid+카운터라 재생성 불가"*는 **거짓**이다(`atomic.rs:210-214` —
   카운터는 프로세스마다 0으로 리셋되고 PID는 재사용된다). **안전성은 유일성이 아니라 실패 방향이
   status quo라는 데서 나온다.**
2. **`.objects` 루프 밖의 삼킴 3곳**(`:74` · `:115` · `:166`) — **축자 보존**하지만 **오늘의 코드는 P1의
   문언대로 동작하지 않는다**(EIO를 삼킨다). 고치면 **두 번째 플립**이다 → **F-33 · F-34. 둘 다 데이터 손실
   클래스이므로 우선순위 높음.**

## Follow-up Backlog

| id | 내용 |
|---|---|
| **★ F-43** *(★r26 신규 — 낮음 · **증인 커버리지**)* | **격리 분기에 배리어 훅이 없어 `:257`(`rename_into`의 `SourceGone`) 팔에 결정적 증인을 지을 수 없다**(**Class B-QUAR**). `e.read()`(`:251`)와 `e.rename_into()`(`:256`) 사이에 발화하는 훅이 **하나도 없고**, 그 창은 µs 단위라 **랑데부로도 못 밟는다**(실측: `panic!` 프로브 × 3회 → Phase E 포함 전 스위트 `170 passed; 0 failed`). ⇒ **P16 ②의 침묵 계약이 그 팔에서만 무증인이다**(행동 자체는 W5a·W9b가 판다 — 무증인인 것은 **로그 침묵뿐**이다). 최소 픽스: **10번째 훅 `pre_quarantine_rename`**(`pre_grave`와 같은 모양 · prod = `None` ⇒ 관측 행동 변화 0)을 `mkdir_p_durable` **뒤** · `rename_into` **앞**에 꽂고 **W-LOG-D에 무대 ⑦을 추가**한다. ⚠ **이 증분에서 하지 않는 이유**: B-1은 훅을 **9개로 동결**했고(§7), 훅 추가는 **프로덕션 코드 변경**이라 단일-플립 계약의 diff 표면을 넓힌다 ⇒ **별도 증분이 정직하다** |
| **F-42** *(★r14 신규 — 중간)* | **꼬리 파괴에서 GC가 `.objects`를 되살리고 tombstone 원장을 `{}`로 덮어쓴다** — `write_atomic`이 원장을 쓰기 **전에** `mkdir_p_durable`로 컨테이너를 만든다(실측 T1: 심어 둔 `{"deadbeef":1}`이 `{}`가 됐다). **오늘의 버그이고 D안은 이것을 보존한다**(고치면 두 번째 플립). 최소 픽스: *"`write_atomic`이 원장을 쓰기 전에 컨테이너를 만들지 않는다"*. 고치면 §C의 자기무효화 벡터 ②도 함께 사라진다. ⚠⚠ **판정 — 적대적 ABA(파괴 → 재생성) 자체는 백로그로 신설하지 않는다**: **데이터 손실이 0**이고, 닫으려면 **핀/`(dev,ino)` 정체성 = C안**이 필요한데 **인간이 그것을 반려했다** ⇒ 백로그에 올리면 *"언젠가 C안을 한다"*는 **거짓 약속**이 된다. **Class B-ABA로 공개하는 것이 정직한 처리다** |
| **F-41** *(높음 · 데이터 손실 클래스 · **기존 구멍 · F-14와 인과 없음**)* | **핀을 우회해 만들어진 커밋 포인터가 GC의 두 술어(`refs` ∨ `landed`)에 둘 다 안 잡힌다** ⇒ 만료 tombstone을 가진 그 blob이 회수되어 **영구 404**. 도달 경로 = **같은 데이터 루트의 두 번째 프로세스/레플리카**(D-3 → **F-32**) · **운영자의 수동 `.meta.json` 복원**. **오늘의 코드에서 이미 열려 있다**(대조군: 사라진 항목 **0개**에서도 재현 — `gc_deleted: 2` · 영구 404). 근본 해결은 **tombstone에 blob의 *관측 세대*를 결박**하는 것(구 F-40을 **흡수한다**) + **F-32**(단일-패스 강제). 증거: `docs/reviews/reconcile-vanished-entry-aborts-pass/evidence-p21-refutation.md` |
| **F-34** *(높음 — 데이터 손실 · ★r14 등급 상향)* | **`collect_referenced`/pending read의 조용한 fallback.** ① `:74-79`의 `if let Ok(raw)` — 커밋 포인터 read가 EACCES/EIO/**EMFILE**이면 그 포인터가 참조하는 blob이 **GC 후보로 떨어진다**(⇒ **`refs`는 하계다** — §The fix 0의 정정 · **Class B-REFS**. **실측**: 포인터 `0o000` → `pass1 referenced:0` → `pass2 gc_deleted:1` → **영구 404**. **red.sha에서 바이트 동일하게 재현**) ② `:166`의 `Err(_) => HashMap::new()` — `.gc-pending.json` read가 EIO면 **tombstone 원장이 전멸**한다. ⚠ **F-14가 GC를 되살리면 도달성이 급증한다** |
| **F-33** *(높음 — 데이터 손실)* | **`recover_graves`의 `blob_intact`가 read EIO를 "blob이 썩었다"로 오독한다**(`reconcile.rs:115-118`) ⇒ 일시적 EIO에서 **무덤이 정본 blob을 덮어쓴다**(파괴적 전이). 이 픽스는 그 줄을 **축자 보존**한다 |
| **F-35** | **M5의 행동 증인 부재.** rename과 부모 fsync **사이**에 배리어가 필요한데 그 둘은 같은 `spawn_blocking` 클로저 **안**이다 ⇒ **`SyncHook` 계열 배리어를 `rename_checked_blocking` 안에** 꽂아야 한다(`in_commit_pre_rename`이 선례). 열리면 **M5를 Class B에서 행동 증인으로 승격**할 수 있다 |
| **F-31** | **`CommitPointerWalk`의 같은 잠복 클래스** — `layout.rs:294,312`의 `entry.file_type().await?`가 **이름 필터 이전에** 호출된다 ⇒ `DT_UNKNOWN` FS에서 패스가 중단될 수 있다. **`scope[]` 밖** ⇒ **자기 파이프라인으로 낸다.** 최소 픽스: **이름 필터를 `file_type()` 앞으로 당긴다** |
| **F-32** | **D-3 해저드의 런타임 방어** — 같은 데이터 루트에 `Store::new`를 두 번 하면 `pass_lock`이 갈라져 **패스 2개가 동시에** 돈다. 프로세스-전역 등록부 또는 온디스크 패스 락 |
| **F-29** | reconcile 관측성 카운터(`recovered`/`restored`/`deferred`/**`skipped`**)를 stats/metrics로 여는 파이프라인. **`ReconcileStats` 계약을 여는 일**이므로 별도 플립이다 |
| **F-25** | 비트로트 격리 분기는 여전히 **핀·무덤을 거치지 않는다.** 이 픽스는 그 사실을 **바꾸지 않는다** |
| ~~**F-38 · F-40 · F-39**~~ | **폐기** — F-38(virtiofs 핀 계측)은 **핀이 코드에서 사라져 잴 것이 없다** · F-40(tombstone 관측 세대)은 **F-41이 흡수한다** · F-39(musl 발산 증인)는 **`DirEntry`를 그대로 부르므로 발산이 정의상 0이다** |

---
## Review Decision Log

> ⚠⚠ **읽기 전 — 계수의 지시대상 (r10)**. 아래 r2~r9의 기록은 `pre_recover_grave`를 **"8번째 훅"**이라
> 부른다. **그 라운드들 시점에는 맞았다.** r10에서 **P-15를 수용해 RED를 재포착하면서 새 red.sha
> `ac58bd7`이 8번째 슬롯을 `pre_entry`로 채웠으므로**, `pre_recover_grave`는 **지금 9번째**다.
> **기록은 고치지 않는다**(그때의 판단을 그때의 언어로 남긴다) — **지시대상만 이렇게 읽어라.**
> 본문의 **모든 규범적 서술은 이미 9번째로 정정했다.**

### Codex Plan Review — r1: needs-attention (3 findings)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-1** | critical | *Dangling symlinks are a second observable flip* — 원안의 `Seen::of`가 **모든** `NotFound`를 `Gone`으로 바꾸므로, 댕글링 심링크(항목은 **있고** 타깃만 없음)에서 오늘의 `Err`가 `Ok`로 뒤집힌다 | **Accept** | `Gone`의 의미를 *"어떤 NotFound가 났다"*에서 **"소스 디렉터리 항목 자체가 부재하다"**(lstat 확인 ∧ 부모 생존)로 바꿨다. 댕글링 심링크는 항목이 **있으므로** 오늘의 행동(패스 `Err` 중단)을 **바이트 동일하게 보존**한다(**W3 · W1(b) · W4**). 원안의 **T-V1/T-V2는 폐기**하고 결정적 소스-제거 증인(**W1~W10**)으로 교체했다 |
| **P-2** | critical | *Whole-operation conversion can swallow durability failures* — 원안의 `Seen::of(pass.grave(..).await)`는 rename과 부모 fsync를 **하나의 `io::Result`로** 덮으므로, rename 성공 후 fsync 실패(= 내구성 실패)가 `Gone`으로 오보된다 | **Accept** | `rename`과 부모 `fsync`를 **타입 경계에서 분리**했다(`Renamed { Done, SourceGone(Absent) }` · `rename_checked_blocking`). **`SourceGone`은 `std::fs::rename`의 `Err` 팔에서만 태어난다** ⇒ rename 성공 후 fsync 실패는 **무가공 `io::Error`**다(post-rename 경로에서 구조적으로 도달 불가). 증인 **W5c · W6b** |
| **P-3** | high | *The claimed type-level enforcement is bypassable* — `Seen`/`Entry`가 `reconcile.rs`와 **같은 모듈**이면 루프가 언제든 raw `DirEntry`/inline 분류로 돌아갈 수 있고, `From<io::Error> for Seen<T>` **미구현**은 아무것도 강제하지 못한다 | **Accept** | 자식 모듈 `src/store/reconcile/entry.rs`로 경계를 긋고 **readdir(`Entry::snapshot`)까지 그 안에** 넣어 `DirEntry`가 `reconcile.rs`에 **한 번도 등장하지 않게** 했다. **`atomic::Absent(())` 위조 불가 토큰** — 부모는 `Seen::Gone`/`Renamed::SourceGone`/`GraveOutcome::SourceGone`을 **합성할 수 없다** ⇒ 우회 뮤턴트(M8/M10)가 **컴파일되지 않는다**. `include_str!` 소스 규율 증인(**W8**)이 나머지를 기계화한다. **무효한 `From<io::Error> for Seen<T>` 주장(원안 M8)은 문서에서 완전히 삭제했다** |
| **alt** | — | *Simpler alternative*: `Entry` 래퍼를 버리고 분류 정책만 헬퍼로 두라 | **Partially accept** | 분류 정책(부재 확인 + 목적지·fsync raw)은 **채택**했다. **`Entry` 래퍼는 유지**한다 — 제거하면 루프가 raw `DirEntry`를 다시 쥐어 **P-3 우회 뮤턴트(M10/M11)가 무증인으로 되살아난다**. 래퍼 + `Entry::snapshot` + **W8(b)/(d)** 로 그 문을 닫는다 |

**개정 후 라운드 2 재실행** — 이 개정판(P-1/P-2/P-3 봉인 + 3-렌즈 적대적 사전검증: symlink 80 ·
durability 0.55 · bypass 6, 치명 0/3)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 걸었다**.

### Codex Plan Review — r2: needs-attention (escalated)

아티팩트 `docs/reviews/reconcile-vanished-entry-aborts-pass/plan-r2.json` · reviewedSha `793d474`.
**P-1 · P-2는 수리 확인**("P-1/P-2 are repaired") · **새 critical 없음**. 잔여 1건:

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-4** | high | *P-3 acceptance can pass while `recover_graves` remains broken* — 계획은 W2가 "`recover_graves`를 raw `?`로 되돌리는 뮤턴트"를 죽인다고 주장하지만, **W2는 `Entry::rename_durable_to`를 직접 호출하는 유닛 테스트일 뿐이다**. 구현자가 `entry.name()` + `Layout`으로 **무덤 경로를 재구성**해 기존 `atomic::rename_durable(...).await?`를 그대로 쓰면, **W8이 금지한 문자열은 하나도 등장하지 않고**(`rename_durable`은 금지 목록에 없다) 기존 테스트 중 **스냅샷과 rename 사이에 무덤을 사라지게 하는 것도 없다**. ⇒ 선언된 acceptance가 **전부 통과하는데도 사라진 무덤이 여전히 패스 전체를 중단시킨다** | **Accept** | **지적이 정확하다 — r1의 주장이 거짓이었다.** ① **거짓 주장 삭제**: 뮤턴트 표의 M12에서 `recover_graves`를 **분리**해 **M14**(신규)로 세웠다. W2는 `Entry::rename_durable_to`를 **직접** 호출하는 유닛 증인이라 **호출부가 그것을 우회하는 뮤턴트를 원리적으로 못 잡는다**. ② **호출부 증인 W11 추가**: `recover_graves`를 **스냅샷 이음매에서 분해**했다(`recover_graves` 껍데기 + `recover_graves_from(layout, entries)` — 순수 extract-function · 행동/syscall 동일 · `pub(super)`). 증인은 `Entry::snapshot` → **스냅샷된 무덤 A(rename 분기)·C(remove 분기)를 삭제** → `recover_graves_from` → **`Ok(1)`** 을 단언한다. **park 0 · spawn 0 · 동시성 0 · 100% 결정적.** ⚠ **왜 park이 아닌가(정직)**: `recover_graves`는 `PassGuard::begin`에서 **`Hooks`를 인자로 받지도 않고** 돌며, 7개 훅 중 **그 구간에 발화하는 것이 하나도 없다**(`during_collect`는 **그 다음**) · `pass_lock`이 패스를 직렬화하므로 두 번째 패스를 끼워 넣을 수도 없다 ⇒ **8번째 훅 없이는 park이 불가능**(= 별도 플립 · 금지)하다. 그래서 **동시성이 아니라 함수 이음매**로 창을 열었다. ③ **W8 강화(구조적 봉인)**: **(h)** `rename_durable(` **0회** · **(i)** `grave_path`/`grave_name`/`".gc-grave-"` **0회** · **(j)** `.join(`/`PathBuf`/`Path::new` **0회** · **(k)** `blob_path` **정확히 2회** ∧ `blob_path(&sha)` **정확히 2회**. ⇒ `reconcile.rs`는 **무덤 경로를 지을 수단이 없고**(`Entry`에는 경로 접근자가 없다) **fail-CLOSED rename을 부를 수도 없다** ⇒ Codex가 대안으로 제시한 *"`Entry::rename_durable_to` 우회를 구조적으로 불가능하게 만들라"*를 **문자 그대로 달성**했다. ⚠ **`blob_path` 0회는 거짓 불변식이다** — grep으로 확인했다(`:114` blob_intact · `:265` pending 정리 = **둘 다 축자 보존**) → **정확히 2회**로 못박고 **인자까지**(`&sha`) 고정했다. 나머지 신규 금지 문자열은 **전부 픽스 후 실제로 0이 된다**(현재값: `rename_durable(` 1 · `.join(` 1 · 나머지 0) |

**Recommendation(Codex)**: 스냅샷된 무덤을 복구 **전에** 제거하고 패스가 계속됨을 단언하는 **호출부 증인**을 추가하라. 그리고 W8이 `reconcile.rs`의 **직접 `atomic::rename_durable` 호출을 거부**하게 하거나, `Entry::rename_durable_to` 우회를 **구조적으로 불가능**하게 만들라.

**하드룰 4 도달** — 2라운드 상한. 게이트는 **BLOCKED**이며, 인간이 (a) 잔여 리스크 면제 (b) 수동 라운드 3 승인 (c) 중단 중 하나를 명시할 때까지 executing으로 넘어가지 않는다.

**인간 판정 (하드룰 4)**: 수동 **라운드 3 승인**(2026-07-13). 잔여 리스크 면제가 아니라 **봉인 후 재심사**를 택했다 — P-4의 수정은 작고 강제된 것이며(호출부 증인 + W8 강화) 설계 모델 변경도 트레이드오프도 없다.

**라운드 3 실행 예정** — 이 개정판(P-4 봉인: **W11** 호출부 증인 + **W8(h)~(k)** 구조적 봉인 + **M14** 신설 · 거짓 주장 삭제)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r3: needs-attention (1 finding)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-5** | **high** | ***W11 can pass while production recovery remains broken.*** W11은 `recover_graves_from`을 **직접** 호출하는데 W8은 **`reconcile.rs`만 스캔한다**. ⇒ 올바른 헬퍼가 **W11을 만족시키기 위해서만 존재**하고, 프로덕션 `recover_graves`는 **old-style raw 루프를 `reconcile/entry.rs`로 옮겨** 유지할 수 있다. **W8은 금지 문자열을 하나도 보지 못하고, W11과 정상 복구 characterization은 통과하는데, `PassGuard::begin`은 스냅샷된 무덤이 사라지면 여전히 중단한다.** ⇒ **P-4/M14의 봉인이 진짜 프로덕션 진입점에서 강제되지 않는다.** **Recommendation**: `recover_graves`를 *"스냅샷 → `recover_graves_from` 호출 정확히 1회"*로 못박는 소스/AST 불변식을 추가하고, dead-helper/raw-production 변종을 **뮤턴트로 검증**하거나 **W11을 실제 프로덕션 진입점의 결정적 이음매로 통과**시켜라 | **Accept** | **지적이 정확하다.** ① **W11 전면 재설계 (옵션 B — 행동)**: **8번째 훅 `pre_recover_grave`**를 열고(`Hooks` 8필드 · 프로덕션 **항상 `None`** ⇒ no-op ⇒ **관측 행동 변화 0 · 두 번째 플립 아님** → **P14**), `recover_graves(&Layout)` → **`recover_graves(&Layout, &Hooks)`** 로 넓힌다(호출부는 `PassGuard::begin` **한 곳** — `pins.rs:427`). W11은 이제 **`recover_graves_from`을 직접 부르지 않는다** — `run_once_at` → `PassGuard::begin` → `recover_graves`라는 **진짜 프로덕션 경로**를 타고, 훅에서 park한 뒤 **스냅샷된 무덤을 삭제**하고 재개해 **패스가 완주함**을 단언한다. 훅은 **분기 판정 이전**에 발화하므로 **rename/remove 두 분기의 공통 조상**이고, **계급당 무덤 2개**(R×2 · K×2)를 심어 파킹된 1개를 뺀 victim 3개에 **R·K가 반드시 각각 ≥1** 포함되게 했다 ⇒ **두 분기 모두** 소멸 항목을 만난다. **첫 발화에서만 park**하므로 파킹된 무덤은 **스냅샷의 첫 무덤**이고 나머지는 **전부 미처리** ⇒ **readdir 순서 무관 100% 결정적**. **자기검증 4종**(파킹 sha가 심은 무덤임 · victim이 두 분기를 덮음 · victim이 park 시점에 **디스크에 있음**(삭제 성공이 미처리의 증거) · **훅이 무덤 4개 전부에서 발화했음**을 채널 드레인으로 확인). ② **W8 강화 (옵션 A — 구조 · 벨트와 멜빵)**: **(l)** `recover_graves` **본문 슬라이스**를 못박는다(`for `/`while ` **0회** ∧ `Entry::snapshot(` **1회** ∧ `recover_graves_from(` **정확히 1회** ∧ raw syscall **0회** — Codex의 recommendation을 **문자 그대로** 구현) · **(m)~(q)** **`entry.rs`까지 스캔**해 raw 루프가 **거기로 이사 오는 것**을 막는다(`Layout` **0** ∧ `grave_sha`/`grave_path`/`blob_path`/`".gc-grave-"` **0** ∧ `Sha256`/`hex::` **0** ∧ `rename_durable(` **0** ∧ `recover_graves` **0** — 무덤 복구 루프에 **반드시 필요한 원시 요소 넷을 전부 없앤다**). **거짓 불변식이 아님을 각각 논증했다**(`classify_objects_entry`가 **`&str`을 받는 자유 함수**임을 실제로 확인 ⇒ `entry.rs`는 `Layout`이 **필요 없다**). ③ **M15 신설**(dead-helper) — 사망 방식은 **W11 2중 킬**(패스가 `Err` → RED · raw 루프는 훅을 발화시키지 않아 **park 도착 신호가 안 와** 타임아웃 → RED)  **∧ W8(l)(m)~(q)**. **M14의 사망 방식도 갱신**(W11이 이제 프로덕션 경로를 탄다). ④ **r2의 거짓 논거 3곳을 정정했다**(숨기지 않는다): *"8번째 훅 = 별도 플립"*(→ 거짓 · P14) · *"M5의 창이 안 열리는 이유는 Hooks 동결"*(→ 거짓 · **진짜 이유는 `spawn_blocking`의 동기 경계**) · *"`recover_graves` 안의 park은 Class C"*(→ **해소됨**). ⑤ **`scope[]`에 `docs/adr/**` 추가 필요**(ADR-0002 개정 — §Single-Flip Contract의 B4 근거) |

**하드룰 4 재도달** — r2에서 이미 2라운드 상한에 걸렸고 인간이 수동 라운드 3을 승인했다. r3이 **새 high 1건**을 냈으므로 다시 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **옵션 B + A 병행** 및 **수동 라운드 4 승인**(2026-07-13). 구조적 불변식만으로는 "헬퍼는 초록인데 프로덕션이 그 헬퍼를 안 쓴다"를 잡지 못한다는 것이 이 파이프라인의 반복된 교훈이므로, **행동 증인을 프로덕션 진입점에 꽂는 쪽**(8번째 훅)을 택했다. 훅은 프로덕션에서 `None`이므로 관측 행동 변화가 0이고 두 번째 플립이 아니다. ADR-0002의 "정확히 7필드" 계수는 8로 개정한다(P4 봉인 논증은 불변).

> ⚠ **위 인간 판정문의 사실 정정 (구현자·리뷰어가 읽어야 한다)**: *"ADR-0002의 '정확히 7필드' 계수"*는
> **ADR-0002에 존재하지 않는다** — grep 확인 결과 그 파일에 문자열 `Hooks`는 **0회** 등장한다. 실제 계수는
> **`src/store/pins.rs:62`**(권위) 외 5곳이며 **전부 이미 `scope[]` 안**이다. ⇒ **ADR-0002 편집은 "틀린
> 계수의 정정"이 아니라 "8번째 훅과 P4 봉인 논증의 *기록*"**(추가)이다. **판정의 실질**(8필드로 간다 ·
> P4 봉인 논증은 불변 · `docs/adr/**`를 scope에 넣는다)은 **그대로 유효하다** — 바뀌는 것은 **ADR 편집의
> 성격**뿐이다(→ B-1 acceptance **7-a/7-b/7-c**).

**라운드 4 실행 예정** — 이 개정판(P-5 봉인: **8번째 훅** + **W11 프로덕션 진입점 재설계** + **W8(l)(m)~(q)** 구조 강화 + **M15** 신설 · r2의 거짓 논거 3곳 정정)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r4: needs-attention (1 finding)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-6** | **high** | ***Test-only configuration can conceal the broken production recovery path.*** W11은 `PassGuard::begin`을 타지만 **`pre_recover_grave = Some`**으로 돈다 — **프로덕션은 항상 `None`**이다. 그리고 **W8(l)은 스냅샷 1회 · `recover_graves_from` 호출 1회를 *세고* 선택된 루프/syscall 토큰을 금지할 뿐, 그 위임이 *무조건적*일 것을 요구하지도, 다른 호출을 금지하지도 않는다.** ⇒ 구현자는 **`cfg(test)` 아래에서 또는 훅이 심어졌을 때에만** 올바른 헬퍼를 쓰고 **프로덕션은 스캔되지 않는 raw 헬퍼에 위임**할 수 있다. **W8 · W11 · characterization이 전부 초록인데, 프로덕션은 스냅샷된 무덤이 사라지면 여전히 중단한다 — P-5와 똑같은 실패다.** **Recommendation**: W8(l)이 래퍼를 **파싱/정규화해 본문 전체가 정확히 "무조건적 `Entry::snapshot` + `recover_graves_from` 꼬리 호출"임을 요구**하게 하라. **조건부 컴파일 · 훅-존재 분기 · 추가 호출 · 재위임을 거부**하라. **`cfg(test)`와 훅-존재 변종을 뮤턴트로 검증**하라. **Simpler alternative (Codex)**: *"승인된 B+A 설계는 유지하되 W8(l)이 래퍼의 정확한 AST를 강제하게 하라"* | **Accept** | **지적이 정확하다 — r3의 (l)은 계수였고, 계수는 블랙리스트다.** ① **W8(l)을 계수 → 본문 전체 동일성으로 교체**했다: 정규화(**줄 주석 제거 → 공백 전부 제거**) 후 **기대 문자열과 `assert_eq!`**. 기대 문자열은 **§⑤의 설계 본문 그 자체**이므로 `cfg` 분기 · 훅-존재 분기 · 추가 호출 · 재위임은 **무엇을 넣든 문자열을 바꿔 전부 RED**다 ⇒ **우회 여지가 열거의 완전성에 의존하지 않고 원리적으로 0**이다(블랙리스트 → **화이트리스트**). **선언(시그니처)도 함께 못박는다.** ② **못을 세 곳에 박는다** — Codex의 recommendation은 **래퍼만** 지목했지만, **래퍼만 잠그면 똑같은 분기가 한 층 아래(`recover_graves_from`)와 한 층 위(`PassGuard::begin`의 호출문)에서 그대로 부활한다**(직접 확인했다: 두 변종 모두 (l)·W11·characterization을 **전부 통과**한다) ⇒ **(l₁)** 호출 문장(`pins.rs` — **연속 부분문자열**로 무조건적 위임을 강제) · **(l₂)** 래퍼 · **(l₃)** 헬퍼 루프. ③ **제3-파일 구멍이 닫혔다**: r3은 *"raw 루프를 스캔되지 않는 새 파일로 옮기면 W8이 못 본다"*를 **정직한 잔여**로 남겼는데, **그 파일을 부르는 행위는 (l₁)(l₂)(l₃) 중 하나 *안에서* 일어나야 하므로** 동일성에 걸린다 ⇒ **열거의 완전성이 더 이상 필요 없다.** ④ **뮤턴트 M16(`cfg(test)`) · M17(훅-존재) 신설** — 둘 다 **W8(l) 단독**으로 죽는다. **W11은 둘 다 죽이지 못한다**(M16: W11은 **`cfg(test)`가 켜진 채** 돈다 · M17: **W11이 훅을 심는 행위 자체가 뮤턴트를 올바른 팔로 보낸다**) — **정직하게 적었고** §5에 **등급 B(구조 증인 단독 · SPOF)**로 분류했다. **거짓 안심을 주지 않는다.** ⑤ **부수 사실 공개**: (g)의 *"`pins.rs` 프로덕션 영역 = 첫 `#[cfg(test)]` 이전"*은 **`pins.rs:145`(`with_hooks`)에서 끊겨 `begin`(:415)에 닿지 못한다**(실측) ⇒ (l₁)은 **전문**을 본다. **r1부터 있던 약점이고 이번 픽스가 만든 것이 아니다 — 숨기지 않는다** |

**하드룰 4 재도달** — 새 critical **0** · high **1**. r2에서 이미 2라운드 상한에 걸렸으므로 다시 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **수동 라운드 5 승인**(2026-07-13). Codex 자신의 simpler alternative대로 **추가 훅 없이 W8(l)만 조인다** — 라운드 3의 B+A 설계(8번째 훅 `pre_recover_grave` · `recover_graves(&Layout, &Hooks)` · W8의 `entry.rs` 스캔)는 **불변**이다. 설계 모델 변경도 트레이드오프도 없고, 바뀌는 것은 **증인 하나의 판정식**(계수 → 동일성)뿐이다.

**라운드 5 실행 예정** — 이 개정판(P-6 봉인: **W8(l)을 계수 → 본문 전체 동일성으로 교체** · **(l₁)(l₂)(l₃) 세 텍스트** · **M16/M17** 신설 · 제3-파일 구멍 완결)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r5: needs-attention (1 finding)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-7** | **high** (confidence 0.99) | ***W8(l₁) permits a test-only outer branch around the approved call.*** (l₁)은 `recover_graves(`를 **세고**, 정규화된 `pins.rs`가 승인된 호출 문장을 **포함**하는지만 본다. 그 문장이 **바깥의 `if cfg!(test)` 또는 훅-존재 분기 *안*에 들어앉아 있어도** 개수와 부분문자열은 통과한다(다른 팔은 이름이 다른 raw 헬퍼를 부른다). **W11은 올바른 팔을 고르고 프로덕션은 망가진 팔을 고른다** ⇒ 주장된 **무조건적 위임이 강제되지 않고 M16/M17이 살아 있다.** **Recommendation**: `PassGuard::begin`의 **선언과 본문 전체**를 정규화 후 **완전 동일성**으로 못박고, **바깥-분기 변종**을 M16/M17에 추가하라 | **Accept** | **지적이 정확하다 — 실행으로 재현했다.** 두 뮤턴트(`begin` 안의 `if cfg!(test) {…} else {…}` · `if hooks().has_pre_recover_grave() {…} else {…}`)에서 **`recover_graves(` 개수는 여전히 1**이고 **승인된 호출 문장도 부분문자열로 그대로 들어 있다** ⇒ r5의 (l₁)은 **둘 다 통과시킨다**. ① **(l₁)을 `PassGuard::begin`의 *선언 + 본문 전체 동일성*으로 승격**했다(앵커 `pub(crate) async fn begin(` — 원본·주석제거본 모두 **정확히 1회**(실측) · 시그니처에 `{` 없음 · 중괄호 균형 슬라이싱 성공 · **기대 본문 = 정규화 후 408자 · 선언 = 83자**, 문서에 **전문 수록** · **픽스 전 본문은 392자**라 **red 트리에서 실제로 RED**다). 두 뮤턴트는 **offset 218에서 갈려 `assert_eq!` RED**다(실행 확인). `#[cfg(test)]` **속성 이중 본문** 판본은 **앵커 개수 2 → RED**. ② **취약성은 의도된 것이다** — 이 단언은 셋을 동시에 못박는다: **무조건적 위임** · **P5(Drop 가드가 fallible op 이전)** · **순서 제약 ③(`recover_graves` ≺ `collect_referenced`)**. 뒤의 둘은 `pins.rs:412-414`가 *"이것만 사람이 지켜야 한다"* 고 적어 둔, **지금까지 증인이 없던** 불변식이다. 실패 메시지가 **"begin을 바꿨다면 기대 문자열도 같은 커밋에서 함께 갱신하고 무조건적 위임이 여전한지 확인하라"** 와 *"리터럴을 뮤턴트에 맞춰 고치는 것은 테스트 약화이며 anti-cheat 리뷰가 본다"* 를 **함께** 말한다. ③ **바깥-분기 변종 4종 신설**(M16″ `cfg!(test)`@begin · M16‴ 속성 이중 본문 · M17′ 훅-존재@begin · M16⁗ 슬라이서 속이기) — 각각의 사망 단언·오프셋을 표에 적었다. ④ **슬라이서 순서 결함 수정(부수 발견 — 숨기지 않는다)**: r4가 *"(g)의 영역이 `pins.rs:145`에서 끊긴다"* 고 적은 것은 **틀렸다**. 실제 첫 리터럴 `#[cfg(test)]`는 **:56의 `//` 주석 안**이고 `reconcile.rs`도 **:285의 문서 주석 안**이다 ⇒ **주석 한 줄로 영역이 붕괴하는 거짓 불변식**이었다. **주석 제거 → 영역 절단**으로 순서를 뒤집고 **네 파일 전부에 영역 위생 단언**을 붙였다. ⑤ **⚠ 자기 반증에서 나온 치명적 발견 — 사다리는 *아래*로도 있었다 (P-7의 진짜 크기)**: r5의 완결 논증이 쓴 *"① `Entry`의 FS 메서드는 전부 `Seen`을 돌려주므로 **중단하지 않는다**"* 는 **거짓**이다 — 반환형은 `io::Result<Seen<T>>`이고 **`Err`가 곧 중단 채널**이다. `Absent`는 **위조**를 막을 뿐 **주조**를 강제하지 않는다. ⇒ `Entry::seen`에 `&& cfg!(test)` 한 조각(**M19**)만 붙이거나, **스캔조차 되지 않던 `atomic.rs`**의 `entry_is_absent`가 프로덕션에서 `None`을 돌려주게 하면(**M20**), **(l₁)~(l₄)·(m)~(q)·W1~W11·characterization·`adversarial.rs`가 전부 초록인데 프로덕션은 그대로 중단한다.** **거짓 문장을 삭제하고 (l₅)(l₆)(r) + `atomic.rs`를 W8의 네 번째 스캔 대상으로 추가**해 **아래쪽 사다리를 바닥(`std::fs`)까지 닫았다.** ⑥ **잔여를 정직하게 등재했다 — M18(Class B)**: 남는 우회는 **`run_once_at`/`run_once`/`main.rs`의 바깥-분기 + `pins.rs`의 두 번째 `PassGuard` 생성자**다. **선택적 (l₄)** 로 한 칸 밀어내되 **사다리가 끝나지 않음을 명시**한다. **보상 통제: structure gate의 anti-cheat diff 리뷰 + conductor-side `/code-review` + release gate.** **거짓 안심을 주지 않는다 — 이것은 테스트가 아니라 리뷰가 막는다** |

**하드룰 4 재도달** — 새 critical **0** · high **1**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **수동 라운드 6 승인 + 정지 규칙**(2026-07-13). (l₁)을 `PassGuard::begin` 본문 전체 동일성으로 올린다 — `begin`이 `recover_graves`의 유일한 호출자이므로 이것이 의미 있는 마지막 계층이다. 소스-문자열 불변식은 언제나 한 겹 바깥에서 우회 가능하므로, **잔여 사다리는 테스트가 아니라 structure gate의 anti-cheat diff 리뷰가 막는다**고 명시하고 Class B로 공개한다. 라운드 6이 또 한 겹 위의 같은 클래스를 내면 **면제**하고 executing으로 간다.

> ⚠ **판정문에 대한 실행 기반 보정 (r6 — 숨기지 않는다)**: 위 판정의 전제 *"`begin`이 `recover_graves`의
> 유일한 호출자이므로 **이것이 의미 있는 마지막 계층**"* 은 **호출 사슬의 *위쪽*에 대해서만 참**이다.
> **플립이 실제로 구현되는 계층은 `begin`이 아니라 `Entry::seen`과 `atomic::entry_is_absent`이며**, r5까지
> **그 두 본문을 못박는 증인은 하나도 없었다**(그리고 `atomic.rs`는 W8의 스캔 대상조차 아니었다).
> ⇒ **정지 규칙의 면제 조항("또 한 겹 *위*의 같은 클래스")에 해당하지 않는 결함**(**M19/M20** — 인간 판정이
> 면제하지 **않겠다**고 못박은 *"타입 봉인의 구멍"* 클래스)이 **한 겹 *아래*에 있었다** ⇒ **면제하지 않고
> 봉인했다**((l₅)(l₆)(r)). **판정의 실질은 유효하다**: **위쪽** 사다리(M18)는 Class B로 공개하고 리뷰 통제로
> 덮으며, 라운드 6이 **또 한 겹 위**의 같은 클래스를 내면 **면제하고 executing으로 간다.** 바뀌는 것은
> *"마지막 계층"*의 **위치**뿐이다 — 아래쪽 사다리는 **`std::fs`에서 유한하게 끝나므로 실제로 닫았다.**

**라운드 6 실행 예정** — 이 개정판(P-7 봉인: **(l₁) = `begin` 선언+본문 전체 동일성**(408자 · 실행 대조) · **바깥-분기 뮤턴트 M16″/M16‴/M17′/M16⁗** · **아래쪽 사다리 봉인 (l₅)(l₆)(r) + `atomic.rs` 스캔 추가 · M19/M20 신설 · 거짓 완결 논증 삭제** · **슬라이서 순서 결함 수정 + 4파일 영역 위생** · **M18을 Class B로 등재 + 보상 통제 명시**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r6: needs-attention (1 critical)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-8** | **critical** (confidence 0.99) | ***`Entry::snapshot` remains an unsealed production-only escape hatch.*** W8(l₅)는 `seen()`과 여섯 FS 메서드를 exact-match 하면서 **`Entry::snapshot`을 빠뜨렸다** — **잠긴 `recover_graves` 래퍼가 잠긴 복구 루프 바로 앞에서 그것을 부르는데도**. 구현자는 `snapshot` **안에** `#[cfg(not(test))] super::legacy::raw(dir).await?;` 를 넣고 **스캔되지 않는 새 모듈**에 옛 무덤 워커를 둘 수 있다. 그 호출은 **(m)~(q)의 금지 토큰을 하나도 포함하지 않고**, **(r)은 `cfg!(`만 막을 뿐 `#[cfg(not(test))]`는 막지 않는다.** ⇒ **(l₁)~(l₆) 동일성이 전부 초록**이고, W11·유닛 증인은 `cfg(test)`로 컴파일돼 raw prepass를 **제외**하며, 통합 characterization은 **무덤을 사라지게 하지 않는다** ⇒ **선언된 모든 증인이 통과하는데 프로덕션은 그대로 중단된다.** **Recommendation**: `Entry::snapshot`의 **선언 + 정규화 본문 전체를 (l₅)에 추가**하고, 방어 심화로 **프로덕션 영역에서 `#[cfg(` 자체를 금지**하며, `#[cfg(not(test))]` prepass **뮤턴트를 등재**하라 | **Accept** | **지적이 정확하다 — 그리고 P-8은 *증상*이었다. 병은 "폐포를 손으로 열거했다"는 것이다.** ① **(l₅)를 7 → 8앵커로 승격**: `Entry::snapshot`의 **선언(60자) + 본문(280자)** 을 동일성으로 못박았다(실행 대조 — 앵커 1회 · 중괄호 균형 왕복 · 꼬리 표현식 `Ok(entries)` ⇒ 조기 절단 공간 0 · (m)~(q) 0회 ⇒ 거짓 불변식 아님). ② **`#[cfg(` 프로덕션 금지를 *실측 기반*으로 넣었다**: 전 파일(주석 제거) 계수 `reconcile.rs`=2 · `pins.rs`=2 · `atomic.rs`=1 · `mod.rs`=3 · `locks.rs`=3 · 나머지 0 — **전부 `#[cfg(test)]`**이고 `#[cfg(not(`·`cfg!(`·`cfg_attr` 는 **어디에도 0회**다 ⇒ **거짓 불변식이 아니다**. ⚠ **영역 기반 금지는 쓰지 않는다**(`pins.rs`의 영역은 :145에서 끊겨 **2,415자 / 151KB** — 거의 공허하다). ⚠ **허용 목록은 `cfg(test)` + `cfg(unix)` 둘**이다(심링크 증인 W1(b)(c)·W5(d)가 이 저장소의 관행(`tests/layout_tree.rs:103`)을 따를 수 있어야 한다 — `cfg(unix)`는 **타깃 조건**이라 테스트/프로덕션을 **가를 수 없다**). ③ **★ 폐포를 기계로 계산했다 (인간 판정 2의 핵심 요구)**: 잠긴 본문을 루트로 호출 그래프를 **고정점까지** 전개 — **4회 반복 · 고유 노드 76 = 잠김 16 + 외부 리프 40 + 미봉인 20 · 정의 미상 0**. **미봉인 20 중 에러 채널 보유 = 4**: `Entry::snapshot`(P-8) · **`collect_referenced`** · **`fsync_dir`** · **`fsync_dir_blocking`** — **뒤의 셋은 P-8이 지목하지도 않은 같은 클래스의 노드였다.** ⇒ **(l₇)(l₈) 신설 · (l₆)은 5 → 7함수**. **에러 채널 논증**으로 나머지 16노드가 **F-14를 보존하는 뮤턴트를 호스팅할 수 없음**을 증명했다(반환형에 에러 채널이 없으면 잠긴 `?`에 `Err`를 주입할 수 없다 · 삼키면 버그가 고쳐지고 · `panic!`은 다른 플립이며 characterization 137이 죽인다). ④ **뮤턴트 M21~M27 등재** — M21(cfg prepass@snapshot) · M22(@collect_referenced) · M23(@fsync_dir[_blocking]) · M24(`cfg_attr`+`path`) · M25(런타임 훅-존재 가드 · **cfg 없음**) · M26(**무조건 `#[path]`**) · M26′(`include!`) · M27(스캔 밖 파일의 prepass). ⑤ **적대적 반증이 낸 두 결함을 그대로 반영했다(숨기지 않는다)**: **P-9 (CRITICAL · rustc로 재현)** — `#[path]` 한 줄이 **모든 텍스트 동일성을 미끼에 대고 초록으로 만든다**(`include_str!`는 **디스크의 바이트**를 고정할 뿐 **무엇이 링크되는지**를 고정하지 못한다) ⇒ **(r″) 링크 폐포** 신설(**`SELF_PATH = file!()`** + **신뢰 사슬 귀납**: 뿌리 `src/lib.rs`가 `scope[]` 밖이라 B4가 HEAD-wide로 차단 ⇒ `mod.rs`가 기저 ⇒ 파일마다 `#[path`/`include!(` 0회 ⇒ **컴파일되는 파일 집합 = 워크가 읽는 파일 집합**). r6이 **M24로 조건부 판본만 막고 무조건 `#[path]`를 열어 뒀다** — **또 열거였다.** **P-10 (HIGH)** — (r′)의 **파일 열거**는 `objects.rs`·`locks.rs`·**새 파일**을 보지 못한다 ⇒ **열거를 버리고 `src/store/**` 디렉터리 워크**로 갔다(W12). ⑥ **거짓 주장 3개를 정정했다**: *"`collect_referenced`도 글자 그대로 같은 탈출구"*(**거짓** — 잠긴 `begin`이 `recover_graves`를 **먼저** 부르므로 무덤이 이미 소진됐다 ⇒ (l₇)은 **구멍 봉인이 아니라 심층방어**다) · *"`scope[]` 안의 미봉인 에러 채널 = 0"*(**거짓** — 참인 것은 **"폐포 안의 미봉인 에러 채널 = 0"**이다) · **폐포 계수 중복**(`Entry::snapshot`을 잠김·미봉인 양쪽에 적었다 → 고유 노드는 **75가 아니라 76**이 되도록 잠김 쪽에만 적는다). ⑦ **(l₇)(l₈)의 기대 리터럴을 실측해 박았다**(손으로 적으면 반드시 틀린다 — `DECL_COLLECT`는 **다중행 파라미터의 끝 쉼표**가 정규화 후에도 살아 있다: 99자) |

**하드룰 4 재도달** — 새 **critical 1**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **수동 라운드 7 승인**(2026-07-13). 정지 규칙은 발동하지 않았다 — P-8은 "한 겹 위"가 아니라 **우리가 닫았다고 선언한 계층 안의 누락**(잠긴 래퍼가 부르는 `Entry::snapshot`을 안 잠갔다)이다. 봉인은 완전하고 값싸다: snapshot 본문 동일성 + **`#[cfg(` 프로덕션 전면 금지**(cfg-편향 클래스 전체를 한 번에 죽인다) + 전이적 폐포 증명. 라운드 7이 **위쪽 사다리(M18)** 계열을 내면 면제하고 executing으로 간다.

**라운드 7 실행 예정** — 이 개정판(P-8 봉인: **(l₅) 8앵커**(+`Entry::snapshot` 60자/280자) · **(l₇) `collect_referenced`** · **(l₈) `fsync_dir` + `fsync_dir_blocking`** · **W12 = (r′) `src/store/**` 디렉터리 워크**(`#[cfg(not(`·`cfg!(`·`cfg_attr`·`#[path`·`include!(` **0회** ∧ 허용 목록 `cfg(test)`/`cfg(unix)`) **+ (r″) 링크 폐포**(`SELF_PATH = file!()` + 신뢰 사슬 귀납) · **전이적 폐포 표**(76노드 · 미봉인 20 · 에러 채널 4) · **뮤턴트 M21~M27** · **거짓 주장 3개 정정** · **M18만 Class B로 남기고 보상 통제 명시**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.


### Codex Plan Review — r7: needs-attention (1 critical)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-11** | **critical** (confidence 1.0) | ***The source-string discipline witnesses (W8/W12) are self-defeating and trivially bypassable.*** W8/W12는 `src/store/**`를 `include_str!`/디렉터리 워크로 스캔해 `cfg!(` · `#[cfg(not(` · `#[path` · `include!(` **0회**를 단언한다. **그런데 그 증인 자신이 `src/store/**` 안에 산다** — W12는 `src/store/mod.rs`의 인라인 `#[cfg(test)] mod struct_gate`다 ⇒ **자기 needle을 자기 소스에 담고 있어 자기 검사에 걸린다** ⇒ **GREEN에 도달할 수 없다**. 그리고 부분문자열 검사는 **띄어쓴 철자**(`#[cfg (not(test))]` · `cfg ! (test)` · `#[ path = ".."]` · `include !("..")`)를 **전부 통과시킨다**. 제대로 하려면 **테스트 안에 Rust 렉서**가 필요하다 | **Accept** | **지적이 정확하다 — 그리고 이것은 소스-문자열 접근법의 *자멸 증명*이다.** ① **W8/W12를 전면 폐기**하고 관련 서술((l₁)~(l₈) · (m)~(r) · (r′)(r″) · 링크 폐포 · 영역 위생 · 전이적 폐포 표(76노드))을 **문서에서 삭제**했다. ② **`tests/` 통합 증인 W13으로 대체**했다 — **핵심 통찰(실험으로 확정)**: `tests/`의 통합 테스트는 **`cfg(test)` 없이 lib를 링크한다** ⇒ **cfg-편향·훅-존재 가드·dead-helper·`#[path]` 미끼가 전부 *행동으로* 죽는다**(실증: `&& cfg!(test)` 뮤턴트 8곳 → `cargo test --lib` **118 passed** / 통합 증인 **3 FAILED**). 랑데부는 훅이 아니라 **프로덕션이 스스로 만드는 온디스크 관측치**로 한다. ③ **뮤턴트 표 전수 재작성** — W8/W12가 죽이던 18개 중 **16개가 A로 옮겨졌고**, W8/W12에만 의존하던 나머지는 **Class B로 강등**하고 **보상 통제를 명시**했다(B-1 프로파일 편향 → `--release` acceptance · B-2 M5 · B-3 격리 rename µs 창 · B-5 증인 약화). ④ **적대적 반증 2건을 그대로 반영했다(숨기지 않는다)**: **(가)** 초안의 항등식은 *"모든 인터리빙에서 성립"*이 **아니었다** — 우리 `unlink`와 패스의 격리 `rename`이 **둘 다 성공**해 이중 계상이 나고 고쳐진 코드가 `--release`에서 **~6% RED**였다 ⇒ **회계를 사후-디스크 판정으로 재설계**했다. **(나)** readdir 순서는 **이름 해시가 아니라 FS 의존**이다(tmpfs·작은 ext4 디렉터리 = **삽입 순서**) ⇒ 초안의 Phase T는 **리눅스에서 결정적으로 창을 못 밟고 조용히 초록**이었고 `Mut-Count`가 살아남았다 ⇒ **심는 순서를 뒤집고**(카나리아 ≺ ballast ≺ temp) **`MIN_STEPS_T ≥ 1`을 넣었다**(문서 내부 모순도 해소). 그리고 **Phase T는 `Entry::metadata()` 창을 자기검증하는 유일한 페이즈**이므로(계측: 프로덕션에서 실제 발화하는 F-14 지점이 거기다) *"떼도 되는 곁가지"*라던 문장을 **삭제**했다. ⑤ **락 무결성 논증(§F-1~F-5)을 절로 승격**했다 — `flips[]` 1행 불변 · `red.sha` 동결 · `characterizationCmd` 문자열·결과 불변 · **`.expect()`가 만드는 `layout.rs` F-31 FS-의존을 락 리스크로 명시** |

**하드룰 4 재도달** — 새 **critical 1**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **계획 단순화 + 수동 라운드 8 승인**(2026-07-13). P-11은 소스-문자열 접근법의 **자멸 증명**이다(W12는 자기 needle에 걸려 GREEN에 도달할 수 없고, 띄어쓴 철자가 부분문자열 검사를 우회한다). 렉서를 테스트에 넣는 것은 **악의적 구현자**라는 존재하지 않는 위협 모델을 위한 기계장치이므로 기각한다. 대신 **W8/W12를 전면 폐기**하고, `tests/`가 **`cfg(test)` 없이 라이브러리를 링크한다**는 사실을 이용해 **cfg-편향·dead-helper 클래스를 행동으로** 죽이는 통합 증인(W13)으로 대체한다. 남는 구멍(위쪽 사다리 M18 등)은 Class B로 공개하고 **structure gate의 anti-cheat diff 리뷰**가 보상 통제다. 라운드 8이 또 같은 클래스를 내면 면제하고 executing으로 간다.

**라운드 8 실행 예정** — 이 개정판(P-11 봉인: **W8/W12 전면 폐기** · **W13 통합 증인 신설**(Phase E/G/T · 사후-디스크 회계 · 심는 순서 불변식 · `MIN_STEPS_E/G/T`) · **`tests/adversarial.rs` 단조 강화** · **뮤턴트 표 전수 재작성** · **Class B 5종 + 보상 통제 명시** · **락 무결성 §F-1~F-5**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r8: needs-attention (1 critical)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-12** | **critical** (confidence 0.99) | ***Replacement directory can forge the parent-liveness proof.*** `metadata(parent).is_dir()`는 **"지금 어떤 디렉터리가 그 경로명을 차지한다"만 증명**할 뿐 **스냅샷 당시의 부모가 살아남았음을 증명하지 않는다.** `.objects`가 스냅샷 이후 삭제/이름변경되고 **동시 쓰기가 그것을 재생성**하면(`write_atomic` → `mkdir_p_durable`), `symlink_metadata(p)`는 `NotFound`인데 **새 부모가 `container_alive`를 통과**한다 → `Absent`를 발행하고 **파괴된 컨테이너의 항목들을 건너뛰고 `Ok(stats)` + 새 pending 원장을 발행**할 수 있다. **선언된 단일 플립과 P5 약속을 위반한다. W10은 재생성 없는 삭제만 테스트하므로 이 ABA를 잡지 못한다** | **Accept** | **지적이 정확하다 — 그리고 계측이 지적보다 더 나빴다.** ① **봉인**: `Entry::snapshot`이 **`read_dir` 이전에** 부모의 **안정적 정체성**을 포착하고(`atomic::DirGuard` = **열린 fd(핀) + `(dev,ino)`**), `Absent`는 **`still_at(dir)`가 참일 때에만** 발행된다. 다르면 **`None` → 원본 `NotFound` 무가공 전파 = 오늘의 행동**(보수적). **발행 지점 A1~A4를 전수 열거**했고(다섯 번째는 **없다**), **A4(`pins::grave()`)가 열린 문이었다** — `Entry`를 쥐지 않으므로 **호출부가 정체성을 넘긴다**. ② **⚠ 승인된 방향 1번(`(dev,ino)` 포착)만으로는 불충분하다 — 계측이 반증했다**: **ext4는 `rmdir` 후 같은 ino를 60/60 결정적으로 재사용**한다 ⇒ **값만으로는 정체성이 위조된다**. **열린 핸들은 "더 강한 대안"이 아니라 필수다**(핀 보유 시 **0/60**) — 핸들의 역할은 디렉터리를 **읽는 것이 아니라 inode를 해제 불가로 못박는 것**이다. 부수 계측: `rmdir` 후 fd의 `nlink`는 **Linux 0 / APFS 2** ⇒ **nlink 생존 판정은 금지**. ③ **적대적 반증이 낸 blocking 2건을 반영했다(숨기지 않는다)**: 맨 **`File::open(dir)`은 `read_dir`와 등가가 아니다** — **FIFO에서 영구 블록**(= 패스가 `pass_lock`을 쥔 채 교착 = **이 픽스가 막으려던 증상의 영구화** = **두 번째 플립**)이고 **일반 파일에서 성공**한다(⇒ ENOTDIR을 손으로 합성하게 되어 `raw_os_error`·메시지가 바뀐다 = **B7 위반**). ⇒ **`custom_flags(O_DIRECTORY)`** 로 연다(`nix`는 **이미 직접 의존성** ⇒ `Cargo.toml` 무변경) — 둘 다 사라지고 **`open` ≡ `read_dir`가 주장에서 *구조적 사실*로 승격**된다(**W5h · M35**). ④ **증인**: **W10-ABA-G**(park-at-**last** ⇒ `grave()`가 루프의 마지막 FS 접촉 ⇒ **M30을 죽이는 유일한 증인**; 재생성 blob은 **심은 orphan 밖**이어야 한다) · **W10-ABA-E**(park-at-first ⇒ `seen`) · **W11-ABA**(복구 루프) · **W14**(핀 속성) · **W5g/W5h**. 재생성은 **프로덕션 경로 그대로**(`write_atomic` → `mkdir_p_durable`) 일으키고, self-verify ③④가 *"정말 다른 inode가 같은 경로에 앉았다"* ∧ *"이 순간 `is_dir()`는 `true`다"*를 **명시적으로 단언**한다. 둘 다 **red/green 양쪽 GREEN** ⇒ **`flips[]` 1행 · `red.sha` · `characterizationCmd` 불변**. ⑤ **거짓 주장 2개를 정정했다**: **W5e는 M4를 죽이지 못한다**(`regfile/child`에서는 부모 검사가 먼저 실패해 뮤턴트에서도 `None` — 실행으로 반증) ⇒ **W5e′**(살아 있는·정체성 일치 부모 + **EACCES lstat** · root skip 프로브)로 교체 · *"핀이 있으면 **모든** 오판이 불일치 쪽으로 떨어진다"*는 **거짓**이다(부재 판정은 **경로를 두 번 해석**하고 핀은 그 둘을 원자화하지 못한다) ⇒ **B-8**로 등재하고 **F-36**(`fstatat`)을 진짜 봉인으로 기록. ⑥ **잔여를 정직하게 등재**: **B-6**(M31 · 포착 순서 — 창이 `snapshot` 안이라 배리어 불가) · **B-7**(M34 핀 드롭 **+ 원격 FS에서 fd가 서버 inode를 핀하지 않는다** ⇒ 코드 변경 없이 퇴화 · **전제 = 로컬 POSIX FS**) · **B-8**(A-B-A 복원) · **B-9**(꼬리 파괴 — **P5는 전면적이지 않다**. red와 바이트 동일이므로 새 플립은 아니지만 *"모든 ABA 창을 닫았다"*는 **과장이다**) · **승인 방향 4번(`#[cfg(unix)]` 팔)은 명시적으로 폐기**(비-unix 팔은 이 저장소에서 컴파일되지도 검사되지도 않는다) |

**하드룰 4 재도달** — 새 **critical 1**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **수동 라운드 9 승인**(2026-07-13). P-12는 8라운드 만에 처음으로 **증인 기계장치가 아니라 픽스 자체**의 정합성 결함이다 — 그리고 진짜다(`write_atomic`의 `mkdir_p_durable`이 `.objects`를 실제로 재생성하므로, 이 시나리오는 가상의 악의적 구현자가 아니라 **평범한 동시 업로드**가 만든다). 게이트가 군비경쟁에서 빠져나와 실제 버그를 짚었다 = 라운드 7의 단순화가 옳았다는 신호. 봉인: **부모 디렉터리의 안정적 정체성**을 스냅샷 시점에 포착하고, **동일한 디렉터리일 때만** `Absent`를 발행한다(다르면 원본 `NotFound` 무가공 전파 = 오늘의 행동).

**라운드 9 실행 예정** — 이 개정판(P-12 봉인: **`DirGuard`**(핀 fd + `(dev,ino)` · **`O_DIRECTORY`**) · **포착 ≺ readdir** · **`Absent` 발행 4지점(A1~A4) 전수 봉인 — A4(`grave`) 포함** · **W10-ABA-E/G · W11-ABA · W14 · W5e′/W5g/W5h** · **뮤턴트 M28~M35** · **잔여 B-6~B-9 + 전제(로컬 POSIX FS) 명시** · **거짓 주장 2개 정정**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r9: needs-attention (1 critical + 1 high)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-13** | **critical** (confidence 0.98) | ***B-7 is not a hypothetical migration residual — it is a current deployment risk.*** 프로덕션 `/data`는 **virtiofs 바인드**인데(`docs/plans/2026-06-30-files-design.md:139`) `DirGuard`의 inode-핀 계측은 **ext4/tmpfs/overlayfs/APFS뿐**이다. **하중을 받는 정체성 증명이 문서화된 프로덕션 FS에서 미검증**이다. 열린 핸들이 inode 재사용을 막지 못하면 **교체된 `.objects`가 `(dev,ino)`를 위조**해 `Absent`를 발행하고 **`Ok` + pending 원장 발행**을 허용한다. **Recommendation**: 마운트된 **프로덕션 PVC에서 pin/no-pin ABA·inode 재사용 계측을 반복**하고 결과를 **acceptance/배포 게이트**로 만들라 | **Accept** | **지적이 정확하다 — 그리고 계측하니 공포는 재현되지 않았지만 *다른 것*이 틀렸다.** ① **프로덕션 백킹 경로(`/mnt/mac/Volumes/homelab/k3s-bulk` = virtiofs)에서 직접 계측했다**: ino 재사용 **0/60 · 0/500(500 distinct)** · `still_at` 교체 거부 **60/60** · `drop_caches=2`로 **FUSE `FORGET`을 강제**해도 **0/60** ⇒ **P-13의 핵심 공포(교체가 `(dev,ino)`를 위조한다)는 프로덕션에서 재현되지 않는다.** ② **⚠ 그러나 그 *이유*를 틀리게 적으면 안 된다(개정 도중 실제로 틀리게 적었다 — 숨기지 않는다)**: 초안은 *"virtiofs의 ino는 호스트 APFS에서 **패스스루**되고 APFS는 디렉터리 inode를 재사용하지 않는다"* 고 적었는데 **거짓이다** — 같은 디렉터리를 양쪽에서 stat하면 **호스트 `ino=21719664/dev=16777234`(APFS)** vs **게스트 `ino=2053665/dev=37`(virtiofs)** 로 **완전히 다르다**. 우리가 비교하는 값은 **OrbStack virtiofs 데몬의 자체 노드 id**이며, **"재사용 없음"은 그 데몬 id 할당기의 (문서화되지 않은·버전 종속의) 성질**이다. **삭제하고 실측을 그대로 적었다**(§계측된 FS 사실 II의 r10 각주 · **B-7**). ③ **핀의 *효능*은 virtiofs에서 반증 불가능하다**(핀 유무 양쪽 다 0) — 증명한 것은 *"정체성이 위조되지 않는다"*(= 정확성에 필요한 그것)이지 *"fd가 inode를 핀한다"*가 아니다. 추정 기전(게스트 inode 참조의 FORGET 억제)은 **미검증**이라고 적었다. ④ **M12 배포 게이트로 등재**(**§5 · B-7-G** — 실행 가능한 `kubectl` 절차 + G1~G6 통과/실패 기준). **G6(`nlink`가 파괴를 잡는가)는 실패**다 — virtiofs는 살아서도 죽어서도 `nlink=1`이다. ⑤ **`nlink > 0` 합취는 채택하되 문언을 강등했다**(§①-0) — **프로덕션·개발기 양쪽에서 상수 참 = no-op**이며, *"M34의 백스톱"*이라는 주장은 **자기모순이므로 철회했다**(nlink를 `fstat(dirfd)`에서 읽는 이상, fd를 삭제하는 M34에는 **읽을 dirfd가 없다**). **"벨트+멜빵"이라 부르지 않는다.** ⑥ **적대적 반증이 요구한 F-36을 B-1에 흡수**했다 — 부재 증명을 `fstatat(dirfd, name, AT_SYMLINK_NOFOLLOW)`로 바꾸면 **경로 해석이 0회**가 되어 **B-8(A-B-A 복원)이 원리적으로 죽고**(APFS 실측: r9 설계는 **존재하는 항목에 `Absent`를 위조**했다) 미룬 근거(*"변경면이 커진다"*)는 **`PinnedDir`가 이미 `dirfd()`를 쥐므로 소멸**했다 |
| **P-14** | **high** (confidence 0.99) | ***The guard creates a new EMFILE pass-abort path.*** `capture_dir`이 디렉터리 fd **하나를 쥔 채** `tokio::fs::read_dir`이 **또 하나**를 연다. RED 구현은 **후자만** 필요하다. `RLIMIT_NOFILE` 아래 **정확히 하나**의 디스크립터가 남았을 때 **옛 패스는 스냅샷에 성공**하는데 새 구현은 그것을 `DirGuard`가 먹고 **`read_dir`에서 `EMFILE`로 중단**한다. 계획의 *"새 실패 클래스 0"* 주장과 모순이고, **디스크립터 압박 하에서 GC를 또 멈추는 두 번째 관측 실패**다. **Recommendation**: **열린 디렉터리 핸들 하나로 열거와 정체성 핀을 둘 다** 하라. **낮은 `RLIMIT_NOFILE` 서브프로세스 증인**을 추가하라 | **Accept** | **지적이 정확하다 — 실측으로 재현했다.** `setrlimit(64)` + ballast + **정확히 1개** 반납: **`red: OK` · `two_fd: FAILED errno=24(EMFILE)` · `single: OK`.** ① **단일 fd로 봉인**: `PinnedDir`가 `open(O_DIRECTORY)` → `fstat` → **`fdopendir`(fd 인수인계 — 신규 생성 0)** 로 **열거·정체성 핀·재검증·부재 증명을 fd 하나로** 한다 ⇒ **peak 1 · held 1 = RED와 바이트 동일**(계측) ⇒ **새 실패 클래스 0**(§①-0b의 **fd 회계표 + 증명**). 부수 이득: **B-6(포착 순서)이 구성상 소멸**하고, 블로킹 홉이 `1+⌈N/32⌉` → **1**로 줄며(30k 항목: 25.2 ms → **12.5 ms**), r9의 *"동시 보유 ≤2 · syscall +2"* 가 **과대 계상이었음**이 드러났다(**RED도 이미 패스 내내 fd 1개를 쥔다** — `rd`가 `run_once_at`의 지역변수다 · 실제 델타는 **`fstat` 1회**). ② **⚠ 인간 판정 1번이 지목한 `nix::dir::Dir::from_fd`는 쓸 수 없다**(실컴파일 확정) — `dir`는 **`features=["fs"]`에 없고**(`Cargo.toml` 수정 = **scope 밖 = B4 위반**) 켜도 **`Dir: !Sync`** 라 `Arc`를 `spawn_blocking`에 넘길 수 없다 ⇒ **raw `nix::libc::fdopendir`** 로 간다(`nix`가 `libc`를 무조건 재수출 ⇒ **`Cargo.toml` 무변경 성립**). `unsafe impl Send + Sync`가 필요하며 **SAFETY 논증을 코드에 박았다**(readdir은 `&mut self`에서 1회 · 이후 `&self`는 안정된 fd에 대한 커널 호출뿐) — 잔여는 **B-11**. ③ **증인 W15 신설**(`tests/reconcile_fd_pressure.rs` — 서브프로세스 재실행 · **음성 대조군 `two_fd_shape`가 창의 실재를 자기검증** · **`raw_os_error() == Some(EMFILE)`로 단언**한다. `kind`는 `Uncategorized`라 `kind` 단언은 **조용히 공허해진다**). **M36** 등재. ④ **부산물 결함 2건을 반증이 잡았다(숨기지 않는다)**: **(가)** `std::fs::FileType`/`Metadata`는 **주조 불가** ⇒ `Seen<FileType>`/`Seen<Metadata>` 시그니처는 **컴파일되지 않는다** → **`Seen<bool>`/`Seen<SystemTime>`** 으로 낮췄다. **(나)** `readdir(3)`은 **`.`/`..`를 돌려준다**(세 FS 전부) ⇒ 거르지 않으면 `DT_UNKNOWN` FS에서 stat 2회가 늘어 **P11 위반** → **W16** 신설 |

**하드룰 4 재도달** — 새 **critical 1 · high 1**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **수동 라운드 10 승인**(2026-07-13). P-14는 순수 코드 결함이므로 **단일 fd**(`fdopendir`)로 봉인한다 — 열거·정체성·재검증을 fd 하나가 담당하므로 옛 코드(`read_dir` fd 1개)와 동수가 되어 **새 실패 클래스 0이 복원**된다. P-13은 **실질적 위험 교환**이므로 인간이 판정했다: ① `still_at()`에 **`nlink > 0`을 합취**로 추가(inode 재사용과 무관하게 파괴를 직접 잡는다) ② **virtiofs 계측을 M12 배포 게이트 항목으로 등재** ③ **계측이 실패해도 픽스는 켜 둔다** — 위험이 비대칭이기 때문이다: 위조는 `.objects` 파괴 + 재생성 + inode 재사용을 요구하고 그 파괴는 운영자 `rm -rf`/SSD 언마운트뿐이지만(후자는 readyz가 잡는다), F-14가 고치는 버그는 **평범한 동시 업로드가 매번** 만든다. 픽스를 끄면 교환이 거꾸로다. 잔여는 **Class B**로 공개한다.

> ⚠ **판정문에 대한 실행 기반 보정 (r10 — 숨기지 않는다. 판정의 *실질*은 전부 유효하다)**
> 1. **`nlink > 0`의 근거 문장이 거짓이다.** 판정문의 전제 *"언링크된 디렉터리는 `nlink == 0`"* 은
>    **프로덕션(virtiofs)과 개발기(APFS) 양쪽에서 거짓**이다 — virtiofs는 **살아서도 죽어서도 1**,
>    APFS는 **죽어도 2**다(계측). ⇒ 합취는 **거기서 아무것도 잡지 못한다**(상수 참 = no-op). 게다가
>    *"M34(핀 제거)의 백스톱"* 이라는 부수 주장은 **자기모순**이다(nlink를 `fstat(dirfd)`에서 읽으므로
>    **fd가 없는 M34에는 읽을 dirfd가 없다**) ⇒ **철회한다.** **그래도 채택한다** — 비용이 0에 가깝고
>    (이미 쥔 fd에 `fstat` 1회 · `NotFound` 경로에서만) 실패 방향이 **보수적**이며 **ext4/tmpfs(미래의
>    로컬 PVC)** 에서는 값이 있다. **문언만 강등한다: "벨트+멜빵"이 아니라 "Linux 로컬 백스톱"이다.**
> 2. **`nix::dir::Dir`는 쓸 수 없다**(판정 1번이 명시한 수단) — feature 미설정(= `Cargo.toml` = scope 밖)
>    + `!Sync`. **raw `nix::libc::fdopendir`** 로 간다. **판정의 실질(단일 fd)은 그대로 성립한다.**
> 3. **계측은 이미 통과했다**(백킹 경로 기준 G1~G5 ✅ · G6 ❌) ⇒ 판정 3번의 게이트는 **파드 내 재실행만**
>    남았다. 판정 4번(*"계측이 실패해도 픽스는 켜 둔다"*)과 **위험 비대칭 논거는 그대로 유효**하며
>    **§5 · B-7-G의 실패 절차에 그대로 기록**했다.
> 4. **추가로 봉인했다(판정이 요구하지 않은 것 — 그러나 적대적 반증이 요구했고 비용이 소멸했다)**:
>    **F-36**(`fstatat(dirfd, name)`)을 B-1에 흡수해 **B-8을 죽였고**, `Entry::metadata()`가 오늘 실제로
>    **`fstatat(dirfd, name)`** 임을 발견해(std 소스 확정) **경로 `lstat`으로 바꾸려던 개정안 초안을
>    폐기**했다 — 그대로 뒀으면 **절대 제약(항목별 트레이스 바이트 동일)을 깨고 두 번째 플립을 만들었다.**

**라운드 10 실행 예정** — 이 개정판(P-14 봉인: **단일 fd `PinnedDir`**(raw `fdopendir`) · **fd 회계표 + "새 실패 클래스 0" 증명** · **W15**(낮은 `RLIMIT_NOFILE` · 음성 대조군) · **W16/W16′** · **M36~M45** / P-13 봉인: **`nlink > 0` 합취(문언 강등)** · **B-7 재작성 = 현재의 배포 리스크** · **B-7-G(M12 배포 게이트: 실행 절차 + G1~G6)** · **virtiofs ino의 진짜 기전 정정** / 부수: **F-36 흡수 ⇒ B-8·B-6 소멸 · B-10/B-11 신설**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r10: needs-attention (2 critical + 1 high)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-15** | **critical** | ***The locked regression witness does not cover the branch production actually hits.*** 프론트매터의 증상과 §도달성 표는 **범인 ①(Temp 분기 `e.metadata()`)이 정상 프로덕션에서 상시 발화한다**고 말하는데, `flips[]`의 **유일한 증인은 Blob 분기(`read`)만 결정적으로 때린다.** 그 증인이 심는 temp 2개의 킬은 **readdir 순서 의존적**이라(계획이 §"결정적으로 핀하지 못하는 것"에서 **스스로 인정한다**) **범인 ①을 되돌리는 뮤턴트가 살아남는다.** ⇒ **Blob만 고치는 픽스가 잠긴 회귀를 GREEN으로 만들면서 프로덕션이 실제로 밟는 Temp 경로는 망가진 채 남을 수 있다** — **기계 배리어(B1/B2)가 헛돈다.** **Recommendation**: **Temp 분기의 결정적 RED 증인을 추가하고 `flips[]`에 넣어라**(= **RED 재포착 · `red.sha` 이동**) | **Accept** | **지적이 정확하다 — 그리고 계획은 이 구멍을 알면서 W2(유닛 증인)로 때우려 했다. 그것으로는 부족하다**(W2는 `Entry` 메서드를 **직접** 부를 뿐 **호출부**를 타지 않는다 — P-4가 이미 같은 클래스를 반증했다). ① **RED를 재포착했다**: 새 커밋 **`ac58bd7`** = 새 **`red.sha`**(종전 `33d05ca` 폐기). ② **Temp 분기 결정적 증인 신설** — `src/store/pins/tests/vanished_temp_regression.rs`의 `reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot` + 대조군. **결정성은 "표적 이름 park"에서 나온다**(그 temp 이름으로 발화할 때만 park · 다른 항목은 통과) ⇒ **readdir 순서 무관 100%**. **RED 20/20 · 대조군 GREEN 20/20.** ③ **그러려면 훅이 필요했다** — Temp 분기에 결정적으로 park를 걸 수 있는 훅이 **기존 7개 중 하나도 없었다**(`pre_grave`/`post_grave`는 **Blob 전용** · `during_collect`는 **스냅샷 이전**) ⇒ **8번째 훅 `pre_entry`**(인자 = 항목 이름 `&str` · 반환 `()` · 프로덕션 `None` ⇒ **no-op**)를 **red.sha에 함께 넣었다**. **그것은 픽스가 아니라 seam이다**: `?` 전파 **무변경** ⇒ **버그는 살아 있다**. ④ **`flips[]` 2행** — 두 증인은 **같은 하나의 관측 행동**(사라진 항목이 패스를 중단시킨다)에 대한 **N개 증인**이고 **symptomToken을 공유한다**(`PASS ABORTED`) ⇒ **하드룰 10 준수**. ⑤ **`--verify-red` 통과**: regression exit **101** · **2 failed** · symptomToken present / characterization exit **0** · **138 passed**. ⑥ **문서 전체의 훅 계수를 정정했다** — `pre_recover_grave`는 이제 **9번째**다 |
| **P-16** | **critical** (confidence 0.98) | ***A `String`-based `RawEntry` creates a second flip in filename handling.*** 제안된 `Entry`는 **`String` 이름만** 들고 `dir.join(&r.name)`으로 경로를 **재구성**한다. **오늘의 reconcile은 `DirEntry::path()`(원본 유닉스 파일명 바이트)를 보존**하고 lossy string은 **분류에만** 쓴다. **비-UTF-8이 든 옛 `.tmp-` 이름**에서는 스냅샷을 거부하거나 **대체문자 경로**를 만들게 되고, 이어지는 `fstatat`이 **틀린 이름**을 보아 **오늘은 stat되어 삭제되는 temp를 건너뛴다.** **stats가 바뀌고 오래된 파일이 샌다 — 선언된 플립 밖이며 어떤 증인도 덮지 않는다.** **Recommendation**: `RawEntry`가 **`OsString`/원시 바이트 이름을 정본 경로이자 `fstatat` 피연산자**로 들게 하고, **분류·로깅용 lossy `String`은 별도로** 두라. **비-UTF-8 `.tmp-` 항목의 characterization을 추가**하라 | **Accept** | **지적이 정확하다 — 그리고 계측하니 파국의 기전이 지적보다 더 미묘했다.** ① **실측(Linux tmpfs)**: `create(".tmp-\xff\xfeabc")` = **OK** · readdir이 **그 바이트를 그대로** 준다 · **lossy로 재구성한 경로의 `stat` = ENOENT** · `DirEntry::path()`의 `stat` = **Ok**. ⇒ **`dir.join(&lossy)`는 파일을 못 찾는다.** ② **⚠ 그런데 뮤턴트는 "에러가 나서" 죽는 게 아니다 — *부재 판정이 정당하게 성공해서* 죽는다**: `fstatat(dirfd, lossy)` = ENOENT → **그 lossy 이름은 그 디렉터리에 정말로 없다** → `entry_absent` = **true** ∧ `still_at` = **true** → **`Absent`가 위조가 아니라 *정당하게* 주조**된다 → `Seen::Gone` → **`continue`** ⇒ **old temp가 영구히 잔존하고 `temps_deleted`가 1 → 0으로 바뀐다.** **부재 증명 기계장치가 아무 잘못 없이 작동하는데 틀린 이름을 물었기 때문에 파일이 샌다** — 이것이 P-16의 정확한 크기다. ③ **봉인**: `RawEntry`/`Entry`가 **`raw: OsString`(정본)** 과 **`name: String`(lossy)** 을 **둘 다** 들고, **경로·`fstatat` 피연산자는 오직 `raw`에서** 나온다. **A1~A4가 전부 `&OsStr`를 요구**하므로 lossy 판본은 **컴파일되지 않는다**(§①-0c). ④ **lossy는 오늘과 똑같이 유지한다** — `classify_objects_entry`는 **`&str` 시그니처의 자유 함수**이고 **오늘도 이미 lossy를 먹고 있다**(`reconcile.rs:182,185`). 안전한 이유를 **논증했다**: 분류가 보는 리터럴은 **전부 ASCII**이고 lossy는 **유효 UTF-8 바이트를 건드리지 않는다** ⇒ **`classify(lossy) == classify(raw)`**. 목적지 이름·원장 키도 lossy 그대로 둔다 — **거기 도달하는 클래스(`Blob`·`Grave`)는 `is_sha_name`이 강제하는 64자 ASCII hex**라 **lossy == raw**다. ⇒ **비-UTF-8이 도달할 수 있는 클래스는 `Temp`(와 no-op `Other`)뿐 — 폭발 반경은 Temp 분기 하나다.** ⑤ **`Hooks::pre_entry`의 인자는 `&str`(lossy) 그대로 둔다** — **증인용 seam**이지 FS 피연산자가 **아니고**, 프로덕션 `None`이라 **호출되지도 않는다**(P14). ⑥ **증인 W17 + 뮤턴트 M46 신설**(old → `temps_deleted == 1` ∧ 삭제 / recent → 보존 · 전수 `assert_eq!` · 자기검증 3종). ⑦ **⚠ 정직한 한계 — 숨기지 않는다**: **APFS(개발기 macOS)는 비-UTF-8 파일명을 `EILSEQ`(errno 92)로 거부한다**(실측) ⇒ **W17은 `#[cfg(target_os = "linux")]`로 게이트할 수밖에 없다** ⇒ **개발기에서는 M46이 죽지 않는다.** **Class B-12로 등재**하고, 1차 자물쇠를 **타입(`&OsStr`)** 으로, 2차를 **B-5 diff 리뷰**로 둔다. **거짓 안심을 주지 않는다** |
| **P-17** | **high** (confidence 1.0) | ***The M12 deployment gate cannot be executed in the production container.*** 절차가 파이썬 스크립트를 앱 파드에 복사해 `python3`·`rm`을 실행한다. 최종 이미지는 **distroless static/nonroot**이고 `/files` 하나만 담으므로 **python도, `kubectl cp`가 요구하는 tar도, `rm`도 없다.** 스크립트도 레포에 없고 `scripts/**`는 잠긴 scope 밖이다. ⇒ **G1~G6을 실제 PVC에서 잴 수 없다.** **Recommendation**: **같은 PVC를 마운트하고 프로브를 담은 실행 가능한 one-shot Job/디버그 이미지**를 지정하고, 프로브를 **허용된 아티팩트 경로**에 포함시키며, distroless 앱 컨테이너 안의 도구에 의존하지 않는 **출력 캡처와 정리 절차**를 정의하라 | **Accept** | **지적이 정확하다 — `Dockerfile:11-12`를 읽고 확인했다**(`FROM gcr.io/distroless/static-debian12:nonroot` · `COPY --from=build … /files` · `USER nonroot`) ⇒ **셸도 `python3`도 `rm`도 `tar`도 없다.** r9의 절차는 **문서상으로만 실행 가능했다.** ① **§B-7-G를 전면 재작성**했다: **같은 PVC(`files-bulk-ssd`)를 마운트하는 one-shot `Job`**(`image: python:3-slim`)이 잰다. **앱 컨테이너를 건드리는 단계가 0이다** — `exec` 0 · `cp` 0 · `rm` 0. ② **프로브 소스**: **Job 매니페스트에 인라인**하고 매니페스트를 **홈랩 레포 `platform/files/prod/`** 에 둔다 ⇒ **이 레포에 파일이 하나도 안 생긴다** ⇒ **`scripts/**`(scope 밖) 문제가 소멸하고 `scope[]` 편집도 0**이다. ③ **출력 캡처**: 프로브는 **stdout에만** 쓰고 `kubectl logs job/…`으로 받아 verify 아티팩트에 넣는다(파드에서 파일을 꺼내오지 않는다). ④ **정리**: **`ttlSecondsAfterFinished: 600`** 이 Job/파드를 자동 소멸시키고, **PVC 위의 프로브 디렉터리는 프로브 자신이 `finally`에서 `shutil.rmtree`** 한다(앱 컨테이너에 `rm`이 없으므로 **정리를 밖에 맡길 수 없다**). ⑤ **`backoffLimit: 0`**(계측은 1회여야 한다). ⑥ **G1~G6 기준·판정은 한 글자도 바꾸지 않았다** — **바뀐 것은 *어떻게 재는가*이지 *무엇을 재고 어떻게 판정하는가*가 아니다.** ⑦ **잔여를 적었다**: PVC가 `ReadWriteOnce`면 앱을 잠깐 `--replicas=0`으로 내려야 할 수 있다 ⇒ **접근 모드를 배포 전에 확인하라**를 절차에 넣었다 |

**하드룰 4 재도달** — 새 **critical 2 · high 1**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **수동 라운드 11 승인 + red-capture 롤백**(2026-07-14). P-15가 가장 아프다 — 기계 배리어(B1/B2)가 프로덕션이 실제로 밟는 Temp 경로를 증명하지 못하고 있었다. 스테이지를 red-capture로 되돌려 **Temp seam 증인을 추가하고 flips[]를 2행으로** 만들었다(새 red.sha `ac58bd7`, `--verify-red` 통과: regression 2 failed + symptomToken · characterization 138 GREEN). 그러려면 Temp 분기에 결정적으로 park를 걸 훅이 필요했는데 기존 7개 중 하나도 없어서 **8번째 훅 `pre_entry`(prod None ⇒ no-op)** 를 red.sha에 함께 넣었다 — 픽스 코드가 아니라 **seam**이다(`?` 전파 무변경, 버그는 살아 있다). P-16(원시 파일명 바이트)과 P-17(distroless에서 실행 불가능한 배포 게이트)은 작고 강제된 수정이다.

**라운드 11 실행 예정** — 이 개정판(P-15 봉인: **RED 재포착**(새 `red.sha` `ac58bd7`) · **Temp 분기 증인 + `flips[]` 2행** · **8번째 훅 `pre_entry` = seam** · **`pre_recover_grave` → 9번째로 전면 재계수** / P-16 봉인: **`Entry`가 `OsString` 원시 이름을 정본으로** · **A1~A4 전부 `&OsStr`** · **lossy는 분류·로깅·원장 키 전용** · **W17/M46** · **B-12(APFS 한계) 정직 등재** / P-17 봉인: **B-7-G를 PVC 마운트 one-shot Job으로 재작성** · **앱 컨테이너 의존 0** · **프로브는 홈랩 레포/인라인** · **`kubectl logs` 캡처 + Job TTL 정리**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.
### Codex Plan Review — r11: needs-attention (1 critical + 1 high)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-18** | **critical** (confidence 0.99) | ***DT_UNKNOWN handling changes snapshot semantics.*** 계획은 `DT_UNKNOWN` 해석을 **`Entry::is_dir()`까지 미룬다**. 잠긴 **tokio 1.52.3은 read-dir 청크를 채우면서 `std::fs::DirEntry::file_type().ok()`를 즉시 부르고**(`read_dir.rs:146`), std는 `DT_UNKNOWN`에 대해 **그 자리에서 `metadata()`** 로 내려가며(`unix.rs:981`), tokio는 **성공한 결과를 캐시**한다(그 즉시 조회가 **실패했을 때만** 나중에 재시도 — `:344-349`). ⇒ **미루면 더 나중의 파일시스템 상태를 본다.** 예: 스냅샷 이후 **파일로 교체된 64-hex 디렉터리**는 베이스라인에선 **캐시된 타입 때문에 건너뛰어지지만** 계획된 구현에선 **처리되어 격리·회수될 수 있다**(반대 교체는 베이스라인의 `Err`를 `Ok` skip으로 바꾼다). **두 번째 관측 행동 변화**다. W11의 *"모든 무덤에서 훅 발화"* 단언도 이 즉시-캐시가 없다고 가정한다. **Recommendation**: `read_all`에서 **tokio를 정확히 미러**하라 — `DT_UNKNOWN`이면 **no-follow stat을 즉시** 하고, **성공한 타입은 캐시**하고, **초기 실패는 미캐시로 두고 `is_dir()`에서만 재시도**하라. **강제-`DT_UNKNOWN` 증인**을 추가하라 | **Accept** | **지적이 정확하다 — 그리고 적대적 반증이 그 위에서 *우리 std 인용이 프로덕션에서 거짓임*을 다시 잡아냈다.** ① **§①-0의 `read_all`을 tokio의 *정확한* 미러로 재작성**했다(**즉시 해석 · 성공 캐시 · 실패는 `.ok()`로 삼킴(`?`가 **아니다**) · `.`/`..` 필터는 **eager 이전** · `is_dir()`는 **미캐시만 재시도**) — **인용을 표로 박았다**(tokio `read_dir.rs:22,34-41,128-147,210,299-302,344-349` · std `unix.rs:769-771,972-983`). ② **`dtype: Option<DType>`의 *의미*를 재정의**했다: **`None` = *"DT_UNKNOWN이었고 스냅샷 시점의 stat도 실패했다"*** (= tokio의 `file_type: std.file_type().ok()`와 **글자 그대로 같다**). ③ **W18-a/b/c/d 신설 + M47~M54 등재.** **W18-a는 red.sha에서 실행되는 오라클**(타입 동결을 **소스 인용이 아니라 행동으로** 못박는다)이고 — ⚠ **뮤턴트를 하나도 죽이지 못한다는 사실을 명시했다.** ④ **주입 seam `DtypeProbe`는 프로세스-전역 `static`이 아니라 `Hooks`가 나르는 *인스턴스 스코프*다**(§D-2) — 반증(F3)이 *"전역 static이면 `cargo test`의 스레드 병렬에서 조용한 초록 + flaky 등호"* 임을 보였다. **등호 자기검증(`attempts == 심은 항목 수`)이 결정적이 되어 M52가 죽는다.** ⑤ **W11의 자기검증 ④는 즉시-캐시가 *깨는 것이 아니라 FS-독립으로 만든다*** — 지연 해석이면 `DT_UNKNOWN` FS에서 victim 3개가 `Gone → continue`로 **훅을 발화시키지 못해 ④가 RED**가 된다(§⑤의 표). ⑥ **⚠⚠ 반증이 낸 FATAL을 그대로 반영했다(숨기지 않는다)**: r10/개정안이 인용한 *"`DirEntry::metadata()` = `fstatat(dirfd)`"* 는 **`not(target_env="musl")`을 빠뜨린 인용**이었다(std `unix.rs:904-948`) ⇒ **프로덕션(`aarch64-unknown-linux-musl`)의 baseline은 경로 `lstat(root.join(name))`** 이다(rust:1.93-alpine + strace로 실행 확인 · `mkfs.ext4 -O ^filetype`의 진짜 `d_type=0` FS에서 `file_type()` 폴백도 동일). ⇒ **§계측된 FS 사실 · P8 · P11 · W16′ · §정직한 부수 행동을 전부 다시 썼고**, *"바이트 동일"* 을 **apple 한정**으로 강등했다. **핀-dirfd 미러가 musl baseline과 *기준계*가 다르다는 사실을 Class B-15로 등재**하고 **F-39**(musl 증인 또는 타깃 결정 — **인간 판정 필요**)를 냈다. **거짓 안심을 주지 않는다: `cargo test`는 musl에서 한 번도 돌지 않으므로 이 칸에 초록 불은 없다** |
| **P-19** | **high** (confidence 1.0) | ***The M12 deployment gate job still cannot run.*** PVC claimName이 실제로는 **`files-data`** 인데 `files-bulk-ssd`로 썼고, `files` 네임스페이스의 **Restricted PSS**를 `python:3-slim` 파드가 **위반**하며(admission 거부), 인라인 프로브는 **placeholder**이고 `shutil` import가 **없다** | **Accept** (**이관**) | **소유권을 정정한다 — 봉인이 아니라 이관이다.** **§B-7-G의 Job 매니페스트·`kubectl` 절차·인라인 프로브를 문서에서 통째로 삭제**하고, **리스크 서술(Class **B-7**)과 판정 기준(G1~G6)만** 남겼다. 그 자리에 *"**이 통제는 `M12`/`F-38`이 소유한다**"* 를 명시했다. **신규 F-38**: *"virtiofs 열린-fd inode 핀 계측을 실제 PVC에서 수행 — 홈랩 레포의 실제 매니페스트(claimName `files-data`, `files` 네임스페이스의 Restricted PSS)에 맞춰 one-shot Job을 작성·검증한다. G1~G6 기준은 F-14 계획의 B-7에 있다. **M12 배포 작업이 소유한다**."* ⚠ **왜 두 라운드 연속 틀렸는지 적는다**: **이 저장소에는 홈랩 레포가 없어 PVC 이름도 PSS 정책도 확인할 수 없다.** 확인할 수 없는 것을 계획에 적으면 **다음 사람이 틀린 것을 물려받는다**. **F-14는 코드 픽스이고, 배포 게이트는 배포 작업이다** |

**하드룰 4 재도달** — 새 **critical 1 · high 1**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **수동 라운드 12 승인**(2026-07-14). P-18은 진짜다 — tokio가 **스냅샷 시점에** `DT_UNKNOWN`을 `fstatat`으로 해석해 **캐시**한다는 사실을 설계가 놓쳤고, 미루면 **관측 시점이 이동**해 두 번째 플립이 된다. `read_all`이 tokio를 **정확히 미러**하도록 봉인한다(`.ok()`로 에러를 삼키는 것까지 포함 — 그것이 오늘의 행동이다). P-19는 **소유권을 정정**한다: virtiofs 계측 Job은 **홈랩 매니페스트**이고 이 저장소에는 홈랩 레포가 없어 PVC 이름도 PSS 정책도 확인할 수 없어 두 라운드 연속 틀렸다. F-14 계획에서 **통째로 들어내 F-38로 파일링**하고, 리스크 서술(Class B-7)과 소유권 포인터만 남긴다 — F-14는 코드 픽스이고 배포 게이트는 **배포 작업**이다.

> ⚠⚠ **판정문에 대한 실행 기반 보정 (r11 적대적 반증 — 숨기지 않는다. 판정의 *실질*은 유효하다).**
> 판정문의 *"tokio가 `DT_UNKNOWN`을 **`fstatat`** 으로 해석해 캐시한다"* 는 **개발기(apple)와 glibc-Linux에
> 대해서만 참**이다. **프로덕션(`aarch64-unknown-linux-musl`)에서 std는 `fstatat`이 아니라 경로
> `lstat(root.join(name))`** 을 낸다(`unix.rs:906`의 `not(target_env = "musl")` — 실행 확인).
> ⇒ **판정의 실질(*"시점*을 미루지 마라 — 스냅샷 시점에 해석·캐시하고 실패는 삼켜라")은 세 타깃 모두에서
> 그대로 성립한다**(시점·캐시·`.ok()` 의미론은 **tokio 쪽 성질**이고 타깃과 무관하다). 바뀌는 것은
> **그 자리에서 나가는 syscall의 *기준계*** 뿐이며, 그 발산은 **P-18이 아니라 B-15**로 등재했다(**F-39** —
> 인간 판정 필요). **P-18 봉인은 그대로 진행한다.**

**라운드 12 실행 예정** — 이 개정판(P-18 봉인: **`read_all`이 tokio를 정확히 미러**(즉시 해석 · 성공 캐시 · **실패는 `.ok()`로 삼킴** · 미캐시만 재시도) · **tokio/std 소스 인용(파일:줄)** · **W18-a/b/c/d** · **M47~M54** · **`DtypeProbe` = 인스턴스 스코프 주입 seam(§D-2)** · **W11 자기검증 ④의 근거 명시** / **std 인용 정정**: **musl = 경로 `lstat`** ⇒ **P8/P11/W16′ 문언 강등 + Class B-15 + F-39 신설** / P-19 이관: **B-7-G의 Job·프로브·`kubectl` 절차 전면 삭제 · 리스크 서술과 G1~G6만 존치 · **F-38** 신설**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r11 후속: 인간의 설계 판정 (B안)

> ⚠ **위 r11 항목의 "라운드 12 실행 예정"(A안 개정판)은 이 판정으로 *대체된다*.** r11의 triage 기록
> **P-18 · P-19는 남기되 결말을 여기 적는다**:
> * **P-18**(DT_UNKNOWN 즉시-해석 미러) — **B안에서 *정의상 소멸*한다.** 우리는 tokio의
>   `DirEntry::file_type()`을 **그대로 부른다** ⇒ 시점·`d_type` 캐시·`.ok()` 삼킴 의미론이 **오늘과 글자
>   그대로 같다.** `read_all` 미러 · **`DtypeProbe` 주입 seam(10번째 훅)** · W18-a~d · M47~M50 ·
>   B-13/B-13′/B-14가 **전부 문서에서 사라진다.** *(같은 이유로 **B-15**(musl std 발산 — **증인 원리적
>   0개**)와 **F-39**도 소멸한다.)*
> * **P-19**(M12 배포 게이트 Job) — **F-38로 이관**(소유권 정정: 홈랩 매니페스트는 이 저장소가 소유할 수
>   없다). ⚠ **B안에서 F-38의 내용이 바뀐다**: fd 핀이 없으므로 *"핀이 inode를 핀하는가"*(반증 불가능한
>   전제)는 **잴 이유가 사라졌고**, 남는 것은 **"이 FS가 inode를 재사용하는가"** 하나다 — **직접 측정
>   가능하다.** 등급도 **배포 블로커 → 관측 항목(B-16의 트립와이어)** 으로 내린다(§Follow-up F-38).

**인간 판정 (하드룰 4 · 설계 방향)**: **B안 채택**(2026-07-14). 11라운드 끝에 설계가 전이했다(2400줄 · 뮤턴트 65 · Class B 잔여 15). 전이의 단일 원인은 **P-12(ABA)를 봉인하려 `.objects`에 fd를 핀한 것**이고, 그 fd가 EMFILE(P-14)을 만들어 **`tokio::fs::read_dir`/`DirEntry`를 손으로 다시 짜게** 만들었으며, 그 결과 P-13(virtiofs) · P-16(OsString) · P-18(DT_UNKNOWN) · B-15(musl std 발산 — **증인 원리적 0개**)가 줄줄이 딸려 왔다. **`DirEntry`를 그대로 두면 이 다섯이 정의상 전부 사라진다.** 대신 **P-12를 부분 봉인**한다: `.objects`가 패스 도중 **파괴 → 재생성 → inode 재사용**까지 겹치면 그 파국이 깨끗한 패스로 보고될 수 있다 — **데이터 손실은 없고**(파괴된 blob은 이미 운영자가 지운 것), 파괴 경로는 운영자 `rm -rf`나 SSD 언마운트뿐이며(후자는 readyz가 잡는다), 잃는 것은 **시끄러움**이지 무결성이 아니다. **검증할 수 없는 기계장치로 운영자 실수 시나리오의 시끄러움을 사는 것은 교환이 거꾸로다.** 잔여는 Class B로 공개한다.

**설계 총계 (A안 → B안 → **C안**)** — ⚠ **B안 열은 r12(P-20)에서 폐기된 기록이다. 규범은 C안 열이다.**

| | A안 (r11) | ~~B안 (r12에서 폐기)~~ | **C안 (규범)** |
|---|---|---|---|
| 뮤턴트 | 65 | ≈ 40 | **≈ 40** |
| Class 잔여 | **15** | 7 (B-16 · B-6 · …) | **8** (**C-1 · C-2 · C-3 · C-4 · C-5 · F-41** · B-2 · B-3 · B-5 · B-12 중 **픽스가 *새로* 만드는** 데이터 손실 **0건** — **F-41은 기존 구멍**) |
| 증인 | W1~W18 + 통합 2 | W8/W12/W14/W15/W16/W18 **삭제** | **B안 + W15′ · W10-ABA-RB · W-FIFO** |
| 신규 외부 API | `nix::libc` 12심볼 + `OsStrExt` + `AtomicUsize` | `MetadataExt` 하나 | **`MetadataExt` + `OpenOptionsExt` + `nix::sys::stat::{fstatat,fstat}` + `AsRawFd`** |
| `unsafe` | 2 (`Send`/`Sync`) | 0 | **0** (`nix` 안전 래퍼) |
| 보유 fd | 1 (`fdopendir`) | 1 (`read_dir`) | **2** (핀 + `DIR*`) — **오늘도 `DIR*`는 패스 수명만큼 산다** |
| 훅 | 10 필드 | 9 필드 | **9 필드** |
| 항목별 정상 트레이스 | **apple에서만** 바이트 동일 | 모든 타깃 동일 | **모든 타깃 동일** |
| **픽스가 *새로* 만드는 데이터 손실 경로** | 0 | **⚠ 1건 (P-20)** | **0** *(⚠ **기존 구멍 F-41**은 별개 — **오늘의 코드에도 열려 있다**)* |

> ⚠⚠ **적대적 반증(2026-07-14 · B안 초안에 대해)이 잡은 것 — 전부 반영했다. 숨기지 않는다.**
> 1. **[FATAL] "데이터 손실 없음"이 거짓이었다.** 위조 skip으로 **완주한** 패스가 **만료된 옛 tombstone
>    원장을 새 세대 컨테이너에 durable하게 *재발행*한다**(`reconcile.rs:271-277`의 `try_exists`가 복원된
>    blob에 `true`를 준다) ⇒ 다음 패스가 그 blob을 grave → `settle()`의 cohort/landed가 비어 있으므로
>    **Reaped → 복원된 blob 삭제** ⇒ **blob 없는 포인터 = 영구 404**. RED는 `?`로 죽어 원장을 발행하지
>    **못하므로** grace가 새로 시작되어 살아남는다. ⇒ **§D-2의 tombstone 드롭(`pending.remove`)을
>    채택했다.** 그 한 줄이 있어야 "데이터 손실 없음"이 **실제로 참이 된다.**
> 2. **W10의 문언이 자기 코드에 의해 무효화된다** — 비트로트 blob이 있는 패스에서는 우리 코드의
>    `mkdir_p_durable(&corrupt_dir)`가 **스스로 `.objects`를 재생성**한다(ext4에서 같은 ino) ⇒ *"컨테이너
>    소멸(재생성 없음)"* 은 **안정적 상태가 아니다.** ⇒ **B-16의 세 번째 얼굴로 등재**(§D-1).
> 3. **패스-스코프 "바이트 동일"은 거짓이었다**(문서가 §A-3와 §H에서 **자기모순**이었다) ⇒ **"항목별"로
>    한정**하고 `+2 stat`를 명시했다(§정직한 부수 행동 변화 3 · P11).
> 4. **`file_type()`의 `Gone` 팔은 도달 불가다**(tokio가 readdir 시점에 캐시한다 — 실행으로 확정) ⇒
>    **W2 명세에서 뺐고**, `file_type()`을 raw `?`로 되돌리는 뮤턴트(**M-FT**)를 **Class B로 신설 등재**했다.
> 5. **M46의 심각도를 격상했다** — lossy 재구성은 *"조용한 skip"*이 아니라 **live temp의 영구 누수 +
>    `Absent`의 *정당한* 주조**다(= 이 픽스가 막으려던 누수 그 자체). **`Entry`에서 `dir` 필드를 제거**해
>    타입 자물쇠를 **부분 복원**했다(경로는 `Container` 안 private).
> 6. **`grave()`의 소스 경로가 `layout.rs::is_sha_name`에 암묵 의존한다**(scope 밖) ⇒ **B-3에 못을 박고
>    B-5 diff 항목으로 올렸다.**
> 7. **반려한 것 둘 (근거를 적는다)**: **① `capture` before/after 등가 검사** — **ext4의 ino 재사용도
>    rename-복귀도 `before == after`** 이므로 **B-16의 두 얼굴을 하나도 못 잡으면서** *"열거 창 안 교체 +
>    새 컨테이너 선충전"* 이라는 단일 케이스에 **Ok → Err 라는 새 실패 클래스**를 만든다 ⇒ **교환이
>    거꾸로다**(§C-4). **② `de.ino()` 합취** — 부재 팔에서는 `symlink_metadata`가 `Err`라 **비교할 ino가
>    없고**, 존재 팔에서 ino 불일치를 "부재"로 읽는 것은 **합취가 아니라 이접**이므로 **살아 있는 항목을
>    skip하는 비보수적 확장**이다.
> 8. **반증이 확인해 준 것**: `Absent`/`Seen::Gone`/`Renamed::SourceGone`/`GraveOutcome::SourceGone`의 **위조가
>    타입으로 강제 차단됨**(컴파일러: `E0603 tuple struct constructor Absent is private`) ·
>    `Entry{de: DirEntry, …}`가 **await를 가로질러 `Send`로 산다**(실컴파일 · `unsafe` 0) ·
>    **새 `Err` 클래스 0**(`capture` = `metadata`가 `read_dir`보다 **구성상 약하다**) ·
>    **두 번째 플립 없음**(`Seen::Gone` 팔은 카운터·삭제·rename을 하나도 하지 않는다 —
>    `reconcile.rs:210/221/241`) · **회귀 증인 2개가 B안에서 GREEN이 된다.**

**라운드 12 실행 예정** — 이 **B안 개정판**(A안 전면 폐기: `PinnedDir`/`fdopendir`/raw `readdir(3)`/fd
회계표/`DtypeProbe`/`OsString` 재설계/DT_UNKNOWN 미러/musl 발산/W14·W15·W16·W16′·W18-a~d/F-39 **전부
삭제** · `tokio::fs::read_dir`+`DirEntry`+`e.path()` **보존** · 컨테이너 정체성 = **`(dev,ino)` 값 포착 ∧
`is_dir()`** · **`nlink > 0` 반려** · **§D-2 tombstone 드롭으로 데이터 손실 봉인** · **B-16 신설 · B-6
부활 · B-7/B-8/B-10/B-11/B-13/B-13′/B-14/B-15 소멸**)을 `pipeline-stage: design`인 채로 plan 게이트에
**다시 건다**.

### Codex Plan Review — r12: needs-attention (1 critical)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-20** | **critical** (confidence 0.99) | ***Forged `Gone` lets a restored blob be reaped — `pending.remove` does not seal it.*** 동일-inode 컨테이너 ABA에서 **앞선 항목 X가 위조된 `Gone`**을 내면 패스가 **중단되지 않고 계속**되고, **만료 tombstone을 가진 복원 blob S**가 ⓐ 새 컨테이너에서 읽혀 **같은 패스에서 회수**되거나 ⓑ 옛 스냅샷에 없었다면 최종 정리(`reconcile.rs:272`의 `try_exists`)에서 **옛 타임스탬프를 단 채 살아남아 다음 패스에 0-grace로 회수**된다. `pending.remove`는 **스스로 `Gone`을 내는 Blob 항목만** 덮으므로 **둘 다 보호하지 못한다**. 베이스라인은 **X에서 중단**하므로 S에 도달조차 않는다 ⇒ **픽스가 만든 새로운 영구-404 경로**. **Open question(Codex)**: *"is data loss under B-16 acceptable? If not, option B needs redesign."* | **Accept** | **지적이 정확하다 — B안의 "데이터 손실 없음"이 두 번째로 거짓이었다.** ① **설계를 C안으로 전이했다**(아래 인간 판정). **`.objects`에 fd를 핀하면 ino 재사용 위조가 원리적으로 불가능해지고**(실측 ext4 **199/199 → 0/199** · 값-포착 술어는 같은 조건에서 **200/200 위조**), **`fstatat(pin_fd, raw_name)`로 부재를 판정하면 경로 재해석이 0회**가 되어 **rename-away → 복귀 위조**(**FS-무관 · virtiofs 재현 — B안이 과소평가한 진짜 이빨**)도 닫힌다 ⇒ **위조 `Gone` = 0 ⇒ P-20 ⓐ의 연쇄가 시작조차 못 한다**(§D-3에 `pins.rs:479-495` · `reconcile.rs:164,272`를 코드로 추적해 논증했다). ② **ⓑ(S가 옛 스냅샷에 없는 경우)는 C안도 못 닫는다 — 숨기지 않는다.** 그것은 **기존 아웃오브밴드 복원 해저드**이지만, **문서 스스로 ①(Temp 소멸)이 상시 발화한다고 적었으므로 오늘의 GC는 사실상 죽어 있고, 픽스가 GC를 되살리면 도달성이 급증한다** ⇒ *"기존 해저드 · **픽스 이후 도달성 급증**"* 으로 **Class C-1** 등재 + **F-40**(tombstone에 관측 세대를 묶는 별도 파이프라인) 신설. ③ **`pending.remove`는 재판정 결과 *유지*한다 — 근거만 바뀐다**(§D-2): 위조 봉인 논거는 소멸하지만, **정직한 소멸 → 부활 창**에서 `:272`의 `try_exists`가 **살아 있는 blob에 만료 tombstone을 재발행**하므로 **0-grace 회수**가 난다. 그 한 줄이 **full-grace로 되돌린다.** ④ **적대적 반증이 낸 FATAL을 그대로 반영했다(숨기지 않는다)**: 개정 초안이 P-14 봉인으로 제시한 **"2단계 핀 반납"은 명세된 타입에서 작동하지 않는다** — `Arc<File>`을 **항목마다** 나르므로 mid-loop 반납은 **0바이트 no-op**이고(`IntoIter` 안의 미방출 클론에 손이 닿지 않는다 — 실행 확인), 반납이 **꼬리(`write_atomic`)·`recover_graves`를 덮지 못하며**, **`rename_durable`은 멱등하지 않다**(fsync의 `open`이 EMFILE이면 rename은 **이미 커밋됐다**). ⇒ **2단계 반납을 폐기하고 naive fallback만 채택**하며, **fd 압박 밴드(여유 fd 1~2에서 오늘 `Ok` → C안 `Err`)를 Class C-5로 공개**한다. **"P-14를 봉인했다"고 쓰지 않는다.** ⑤ **B안이 `is_dir()` 합취를 넣은 근거가 무효였음**을 정정했다(파일이 ino를 탈취해도 `lstat`은 **ENOTDIR** ⇒ 부재 판정 ①이 핀 없이도 실패한다) ⇒ **`is_dir()`도 `nlink`와 같은 이유로 반려**(§C-2). ⑥ **nix 0.29의 실제 시그니처를 정정**했다(`fstatat(dirfd: Option<RawFd>, …)` — **`AsFd`가 아니다**) ⇒ 신규 외부 API에 **`AsRawFd`** 추가 · **`fstatat(None, …)` = `AT_FDCWD`** 이므로 **한 토큰 뮤턴트 M-ATFDCWD를 명시 등재**. ⑦ **blocking-in-async를 명시**했다(핀 `open`/`fstat`/`fstatat`은 전부 동기 syscall ⇒ **`spawn_blocking` 경유** · `entry_is_absent`의 async/blocking 쌍 유지) |

**하드룰 4 재도달** — 새 **critical 1**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4 · 설계 방향)**: **C안 채택**(2026-07-14). P-20은 B안의 `pending.remove` 봉인이 불충분함을 증명했다 — 위조된 `Gone` 뒤에 복원된 blob이 만료 tombstone으로 회수되어 **영구 404**가 난다. **데이터 손실은 수용 불가**다(직전 파이프라인 F-1의 존재 이유). 그러나 A안(손으로 짠 리더)으로 돌아갈 필요는 없다 — **손으로 짠 리더와 fd 핀은 분리할 수 있다**. C안: `DirEntry`를 그대로 두고(P-16·P-18·B-15 소멸 유지) **`.objects`에 fd만 핀한다**(inode 해제 불가 ⇒ ino 재사용 위조 불가 ⇒ B-16 데이터 손실 소멸). P-14(EMFILE)는 **Codex가 r9에서 직접 제시한 fallback**으로 닫는다: 핀 `open`이 `EMFILE`/`ENFILE`이면 fail-closed로 퇴화 = **정확히 오늘의 행동**(패스 중단 = 지금의 버그, 데이터 손실 없음) ⇒ 두 번째 플립 없음. 비용은 **fd 1개**뿐이다.

> ⚠⚠ **판정문에 대한 실행 기반 보정 (r12 적대적 반증 — 숨기지 않는다. 판정의 *실질*은 유효하다).**
> 1. **지휘자가 사용자에게 *"B안은 데이터 손실이 없다"* 고 잘못 말했다.** 그 전제 위에서 r11 후속의 B안
>    채택 판정이 내려졌고, **P-20이 그 전제를 반증했다.** 위조 `Gone`으로 완주한 패스가 만료 tombstone을
>    재발행하고, 그 다음 패스가 **복원된 blob을 회수**한다 ⇒ **영구 404**. `pending.remove`는 **스스로
>    `Gone`을 내는 항목만** 덮으므로 **불충분했다.** **기록을 고치지 않고 여기에 정정을 남긴다** — 설계
>    전이(B → C)의 **직접 원인**이 이 오류다.
> 2. **판정문의 *"두 번째 플립 없음"* 은 *핀 획득 실패*에 대해서만 참이다.** 핀이 **마지막 fd를
>    가져가는 경우**(여유 fd = 2)에는 오늘 `Ok`인 패스가 **errno 24**로 죽는다(실측). 초안이 이것을
>    "2단계 반납"으로 닫으려 했으나 **반증이 실행으로 죽였다**(§C-5의 세 결함) ⇒ **Class C-5로 공개한다.**
>    판정의 실질(*"fallback으로 fail-closed = 오늘의 행동"*)은 **핀 실패 11케이스 × 4 FS에서 그대로
>    성립한다**(§핀 획득 실패 대조표).

**라운드 13 실행 예정** — 이 **C안 개정판**(P-20 봉인: **`Container` = 열린 fd(`O_DIRECTORY`) + `(dev,ino)`** ·
**부재 판정 = `fstatat(pin_fd, raw_name, AT_SYMLINK_NOFOLLOW)` + 정체성** · **핀 실패 ⇒ `None` ⇒ fail-closed** ·
**B-16 소멸(데이터 손실 경로 0)** · **P-20 ⓐ가 코드로 닫혔음을 논증 · ⓑ는 Class C-1 + F-40으로 등재** ·
**`pending.remove` 유지(근거 교체)** · **W15′ · W10-ABA-RB · W-FIFO 신설 · W10-ABA의 ext4 skip 삭제** ·
**M-PIN/M-FAILOPEN/M-NOODIR/M-NOIDENT/M-ATFDCWD/M-ORDER/M-PENDING** · **2단계 반납 폐기 + Class C-5 공개** ·
**`is_dir()`·`nlink` 둘 다 반려** · **nix 시그니처 정정 + `spawn_blocking` 명시**)을 `pipeline-stage: design`인
채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r13: needs-attention (1 critical)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-21** | **critical** (confidence 0.99) | ***A genuine vanished entry can still unlock same-pass deletion of a restored blob.*** 계획은 복원된 blob S의 같은-패스 회수가 **위조된 `Gone`**을 요구한다고 주장하지만 그렇지 않다 — **의도된 진짜 `Gone` 플립만으로 충분하다.** 참조(`collect_referenced`)와 pending tombstone은 항목 루프 **이전에** 포착된다. 첫 스냅샷 blob X가 **진짜로** 사라지면 baseline은 X에서 중단하지만 C안은 **계속한다**. 그러면 **만료 tombstone을 갖고 참조 수집 이후에 포인터가 복원된** 뒤쪽 blob S가 grave/reap된다. **`pending.remove(X)`는 S를 보호할 수 없다.** ⇒ **영구 404**이며 *"데이터 손실 경로 0"* 결론을 무효화한다 | **Reject** (**실험으로 반증**) | **P-21의 전제는 전부 성립했는데(refs 비어 있음 · 무덤 실제로 파임) 결론만 안 났다.** 지적은 **보호 술어를 `refs` 하나로 가정**했으나 **F-1이 그 자리에 두 번째 술어 `landed`를 심어 뒀다**(**`pins.rs:376`** — 커밋 rename `Ok` 직후 착지 흔적이 선다). *"`pending.remove(X)`가 S를 못 지킨다"*는 **옳다** — 그러나 **S를 지키는 것은 pending이 아니라 `landed`다**: `settle()`이 `Settlement::Landed`(`:250`) → `Settled::Restored`(`:586`) → `restore_io` + `rename(무덤 → 정본)`(`:610`)으로 **무덤을 되돌린다**(`fn landed()` = *"유일한 보호 술어"* · `:266`). **① 프로덕션 경로**(X 진짜 소멸 + `Store::put`으로 S 포인터 복원): 패스가 계속돼 **S의 무덤이 실제로 파이지만**(`post_grave` 발화) **`landed(S) == true`** ⇒ `gc_deleted: 0` · **`S GET = Ok`**. **② 핀 우회**(`.meta.json` 직접 기록): S가 죽는다 — **그러나 대조군이 결정적이다**: **X를 지우지 않아도**(= 플립 미적용 = **오늘의 코드**) **똑같이 죽는다**(`gc_deleted: 2` · `S GET = Err(NotFound)`) ⇒ **픽스가 만든 손실이 아니라 기존 구멍**이고 플립은 그것을 **가려 주던 우연한 중단**을 치울 뿐이다 ⇒ **F-41 신설**(§5 · Follow-up). **③ 프로덕션 도달성**: 커밋 포인터를 만드는 프로덕션 코드는 **`objects.rs:44`(`put`) · `:110`(`put_stream`) 둘뿐이고 둘 다 `pin.commit_pointer`를 지난다** ⇒ `landed`가 **반드시** 선다 ⇒ **핀 우회는 인-프로세스에서 도달 불가**. ⇒ **문서 반영**: **§The fix 0**(두 술어 `refs` ∨ `landed` 명문화 — **이 사실이 문서에 없어서 P-21이 나왔다**) · **§5/Backlog의 F-41** · *"데이터 손실 경로 0"* → ***"F-14가 **새로** 만드는 데이터 손실 경로가 0"*** 으로 정정(**거짓 안심 금지**) · **봉인 장치의 정체 공개**(§The fix 0의 ⚠ 블록). 증거(원문·대조군·20/20 결정성): `docs/reviews/reconcile-vanished-entry-aborts-pass/evidence-p21-refutation.md` |

**하드룰 4 재도달** — 새 **critical 1**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **P-21 Reject**(2026-07-14). 논증이 아니라 **실험으로 판정했다** — Codex가 요구한 증인을 그대로 만들어(만료 blob 2개 · 첫 `pre_entry`에서 park · X 진짜 소멸 · 참조 수집 이후 S 포인터 복원 · **20/20 결정적**) **봉인 장치 0인 "가장 위험한 픽스"** 로 돌렸다. **프로덕션 경로에서 S는 죽지 않는다** — 무덤은 실제로 파이지만 `landed(S)`가 참이라 `settle()`이 복원한다. P-21은 보호 술어를 `refs` 하나로 가정했으나 **F-1이 두 번째 술어 `landed`를 심어 뒀다**. 핀을 **우회**하는 경로(`.meta.json` 직접 기록)에서는 S가 죽지만, **대조군**에서 **사라진 항목이 하나도 없어도**(= 오늘의 코드) **똑같이 죽는다** ⇒ **픽스가 만든 손실이 아니라 기존 구멍**이다(→ **F-41**). 증거(원문·대조군 포함): `docs/reviews/reconcile-vanished-entry-aborts-pass/evidence-p21-refutation.md`(커밋 `865b11e`).

> ⚠ **실험의 한계 (숨기지 않는다)**: macOS/APFS에서 돌았다. **`DT_UNKNOWN` FS에서의 행동을 직접 관측한 것은 아니다** — 다만 근사 픽스가 `file_type`/`metadata`/`read` **세 지점을 모두** 덮었으므로 결론(**`landed`가 S를 지킨다**)은 바뀌지 않는다.

**라운드 14 실행 예정** — 이 개정판(P-21 **Reject**: **§The fix 0 — 보호 술어 `refs` ∨ `landed` 명문화**(`pins.rs:376 · :250 · :586 · :610`) · **F-41 신설**(§5 · Follow-up — **기존 구멍 · F-14와 인과 없음**) · ***"데이터 손실 경로 0"* → *"F-14가 새로 만드는 데이터 손실 경로 0"*** · **봉인 장치의 정체 공개**(운영자 시나리오 전용 — 봉인 0으로도 전 스위트 120/0))을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r14: needs-attention (1 critical + 1 high)

> **r14는 P-21 반증을 수용했다**(*"the P-21 refutation is persuasive"*) — 그리고 **핀 기계장치 자체**에 대해
> critical 1 · high 1을 냈으며 **인간에게 직접 물었다**:
> *"whether **P5's noisy-failure guarantee is mandatory**; if it is, enumeration must use the pinned handle."*
> Simpler alternative: *"if **container replacement is explicitly unsupported**, use a **path-based
> source-absence check without fd sealing**."*

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-22** | **critical** | ***The pin does not pin what is enumerated.*** `Container::pin`은 `open(O_DIRECTORY)`로 **자기 fd**를 잡지만, 항목 열거는 `tokio::fs::read_dir`가 **따로 여는 `DIR*`** 에서 나온다 ⇒ **핀된 inode와 열거된 inode가 같다는 보장이 없다**(두 open 사이의 창). 부재 판정은 `fstatat(pin_fd, …)`로 **핀된 쪽**을 보므로, 열거가 **다른** 디렉터리에서 왔다면 `Absent`가 **위조**된다 ⇒ **P5가 실제로는 보장되지 않는다.** 시끄러움을 진짜로 보장하려면 **열거 자체가 핀된 핸들에서 나와야 한다**(= `fdopendir` = **A안** ⇒ P-16/P-18/B-15/B-11 부활) | **Accept** — ⚠ **단, D안에서 *정의상 소멸*한다** | **지적이 정확하다. 그리고 그것이 설계 전이의 방아쇠다.** C안은 *"`pin ≺ read_dir` 순서가 load-bearing"*(§C-4 · **M-ORDER** · Class C-2)이라고 적으면서 **그 순서를 지켜도 두 핸들이 다른 inode를 볼 수 있다**는 사실을 **행동 증인 없이** 주석으로만 막고 있었다. 봉인하려면 **A안으로 되돌아가야 하고**, 그러면 11라운드에 걸쳐 죽인 P-16/P-18/B-15/B-11이 전부 되살아난다 ⇒ **교환이 거꾸로다.** ⇒ **D안에서 `Container`·핀 fd·`fstatat`가 코드에서 사라진다** ⇒ **이 결함은 표현 불가**가 된다(**M-PIN/M-NOODIR/M-NOIDENT/M-ATFDCWD/M-ORDER/M-FAILOPEN이 전부 소멸**). 대신 **P5를 재정의한다**(아래 인간 판정) |
| **P-23** | **high** | ***`still_at()` re-interprets the path it claims never to re-interpret.*** 계획은 *"`fstatat`이 경로를 **한 번도** 재해석하지 않는다"*고 적지만, 정체성 술어 ②(`still_at`)는 **`stat(self.path)`** 를 부른다(§C-3 판정식) ⇒ **경로 재해석이 판정식 안에 남아 있다.** `.objects`의 경로 교체(rename-away → 복귀 포함)는 **두 syscall 사이의 창**에서 판정을 흔들 수 있고, `Container::path`를 private으로 둔 **타입 자물쇠는 이 재해석을 막지 못한다** | **Accept** — ⚠ **단, D안에서 *정의상 소멸*한다** | **지적이 정확하다.** C안 §C-3의 판정식은 ①(`fstatat` — 재해석 0)과 ②(`(dev,ino)` 비교 — **재해석 1**)의 **합취**였고, 문서는 ①의 성질을 **판정식 전체의 성질로 승격**해 적고 있었다(**거짓 안심**). ⇒ **D안에는 `still_at`도 `(dev,ino)`도 없다** ⇒ **표현 불가.** D안의 부재 판정은 **`symlink_metadata(e.path())` 하나**이고, 그 경로 해석의 창은 **오늘의 코드가 이미 갖고 있는 그 창**이다(재생성 레이스 → **방향이 보수적** = `Err` = status quo · §남은 위험 6) |

**하드룰 4 재도달** — 새 **critical 1 · high 1**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4 · 설계 방향)**: **D안(최소 픽스) 채택**(2026-07-14). Codex가 던진 open question — *"P5의 시끄러운-실패 보장이 필수인가?"* — 에 답한다: **아니다.** P5는 **설계된 계약이 아니라 `?`의 부수효과**였고, 그것을 지키려고 넣은 컨테이너 생존 합취가 **P-12 → P-13 → P-14 → P-20 → P-22 · P-23**을 줄줄이 낳았다. 그리고 **실험이 증명했다**(`evidence-p21-refutation.md`): **봉인 장치 0**인 픽스로도 전 스위트 **120 passed / 0 failed** · **새 데이터 손실 경로 0** ⇒ **핀·`fstatat`·컨테이너 정체성이라는 기계장치 전체가 오직 P5 하나를 위해 존재했다.** D안은 Codex의 simpler alternative(*"path-based source-absence check without fd sealing"*)를 채택하되, **시끄러움을 값싸게 되찾는다**: 루프 **이후** `.gc-pending.json` 발행 **전에** `metadata(.objects)` **1회** ⇒ **현실적 파국**(파괴 후 **미재생성** = SSD 언마운트 · 운영자 `rm -rf`)은 **여전히 시끄럽다.** 포기하는 것은 **파괴-후-재생성이라는 적대적 ABA**뿐이며, 거기에도 **데이터 손실은 없다**(Class B로 공개).

> ⚠⚠ **D안 초안에 대한 적대적 반증(2026-07-14)이 잡은 것 — 전부 반영했다. 숨기지 않는다.**
> **보존성 렌즈 (a) 자기무효화 · (b) 두 번째 플립 · (c) 댕글링/목적지/EACCES 보존 · (d) 가드 에러 동일성 —
> 네 개 전부 실행으로 통과했다**(D안 구현본 전 스위트 GREEN · 회귀 증인 2개 RED→GREEN). 그러나 **증인·뮤턴트
> 회계가 두 군데 뚫려 있었다:**
> 1. **[critical] `M-COUNT`가 살아 있었다** — 초안의 `vanished += 1`은 **5개 팔**에 흩어져 있었고 W10(blob
>    무대)은 **그중 하나만** 발화시킨다. **Temp-only 파괴 무대에서 실측**: 뮤턴트가 **조용한 `Ok` + `.objects`
>    부활 + 원장 발행**(= M-NOGUARD와 **같은 실패 모드**)을 냈는데 **W10은 GREEN**이었다. ⇒ **타입으로 닫았다**:
>    **계수를 `entry_is_absent` 안으로 넣어**(`atomic::Vanished` · `bump()`는 private) **`vanished += 1`이라는
>    문장이 코드에서 사라졌다** ⇒ **M-COUNT는 표현 불가**(§A). 잔재인 **M-NOBUMP**(단일 지점)는 **W10 ∧
>    W10-TEMP**(★신규 무대)가 죽인다.
> 2. **[high] `W10c`는 아무 뮤턴트도 죽이지 못했다** — 초안의 무대(*"심링크 `.objects`인 정상 배포 · 소멸 0"*)
>    에서는 **`vanished == 0`이라 가드가 아예 돌지 않는다**(실측: D안·`M-GUARD-LSTAT`·`M-NOGUARD`가 **전부
>    `Ok`**). ⇒ **W10c를 재설계했다**: 무대 = **심링크 `.objects` ∧ 소멸 1건** ⇒ D안 `Ok` · `M-GUARD-LSTAT`
>    **`Err(NotADirectory)`** ⇒ **green-only 증인으로 죽는다.** 소멸 0 무대는 **W10c′**(특성화 · **뮤턴트 킬
>    0**)로 **정직하게 강등**했다.
> 3. **[medium] 가드의 `Ok(_)` 팔은 무가공이 아니다** — `ErrorKind::NotADirectory` 합성 에러는
>    `raw_os_error() = None`이라 오늘의 `ENOTDIR/20`과 다르다. 현실적 ABA에서는 **도달 불가**(B7이 먼저
>    무가공 전파한다 — 실측)이나 **Class B″-SYNTH로 등재**했다. *"가드 에러는 무가공"*이라고 **쓰지 않는다.**
> 4. **[high] `refs`는 참조 집합이 아니라 *하계*다** — `collect_referenced`(`reconcile.rs:74-79`)가 포인터
>    read/parse 실패를 **조용히 삼킨다**(EACCES · EIO · **EMFILE**) ⇒ **§0의 두-술어 완전성 주장이 거짓이었다.**
>    **실측**(포인터 `0o000`): `pass1 referenced:0 · gc_pending:1` → `pass2 gc_deleted:1` → **영구 404**.
>    **red.sha에서 바이트 동일하게 재현된다 ⇒ D안이 만든 것이 아니다** ⇒ **§0을 정정하고 Class B-REFS 신설 ·
>    F-34 등급 상향.**
> 5. **[medium] 가드는 `recover_graves` 안의 파괴에 *구조적으로 눈이 멀다*** — 무덤 루프에서 파괴 후
>    **재생성**되면 `read_dir` = `Ok(빈 dir)` ⇒ 엔트리 루프 0회 ⇒ **`vanished == 0`** ⇒ **가드 미발화** ⇒
>    `{}` 원장 + `Ok`(오늘은 `Err`). ⇒ **§D-②의 조건절을 명시**하고 **§B-3 표에 행 7 추가** · **B-ABA의 두
>    번째 얼굴로 등재**(손실 0).
> 6. **[medium] §B-3의 *"바이트 동일"*이 과장이었다** — 가드의 `metadata`가 EACCES/EIO/ELOOP을 낼 수 있는
>    세계에서는 **오늘 `Err`인 패스의 kind가 달라진다**(극성은 같고 전파는 무가공) ⇒ **문언을 약화**했다.
> 7. **[low] §A 행 16의 함정** — `atomic::rename_durable`은 rename+fsync를 **융합**하므로 거기에 부재 확인을
>    붙이면 **rename 성공 후의 fsync ENOENT가 `SourceGone`으로 위조**된다(프로브로 실증) ⇒ **M6 부활.**
>    ⇒ **§A 행 16 · §③에 `rename_checked_blocking` / `rename_durable_source_checked`를 이름으로 못박았다.**
> 8. **반증이 확인해 준 것 (되살리지 마라)**: **`vanished > 0` 게이트는 진짜로 load-bearing이다**(게이트를
>    제거하면 **꼬리 파괴가 `Ok` → `Err`로 뒤집힌다** — 실측) · **M-NOGUARD는 W10이 실제로 죽인다**(조용한
>    `Ok` + 부활 + 원장) · **§D의 "무덤 루프에 가드를 넣지 않는다"는 옳다**(`reconcile.rs:174`의 `read_dir`이
>    오늘과 같은 `Err`를 낸다) · **§E의 `pending.remove` 폐기는 안전하다**(`try_exists`가 파괴된 세계에서
>    `Ok(false)` ⇒ 동어반복. 그리고 D안이 **오히려 더 보수적**이다 — 파괴→빈 재생성 후 다음 패스에서
>    blob 3/3 생존) · **위조된 `Gone`으로는 아무것도 삭제할 수 없다**(모든 파괴 연산이 `Present` 팔 뒤에 있다).

**라운드 15 실행 예정** — 이 **D안 개정판**(P-22/P-23 소멸: **`Container`/핀 fd/`O_DIRECTORY`/`fstatat`/`(dev,ino)`/EMFILE fallback/fd 회계 · 증인 W15′·W10-ABA-E/G/RB·W-FIFO·W11-ABA · 뮤턴트 M-PIN/M-FAILOPEN/M-NOODIR/M-NOIDENT/M-ATFDCWD/M-ORDER/M30/M30′/M32/M33 · 잔여 B-16/C-2/C-3/C-4/C-5 · 백로그 F-38/F-40 **전부 삭제** ⇒ **신규 외부 API 0 · fd +0 · `unsafe` 0**) · **경로 기반 부재 판정**(`symlink_metadata` — P-1 봉인) · **★ 루프-후 컨테이너 가드**(`vanished > 0` 게이트 · `write_atomic` **이전** · `metadata`+`is_dir`) · **`atomic::Vanished` 타입 자물쇠**(M-COUNT 표현 불가) · **W10 재설계**(W10 · **W10-TEMP** · **W10-G** · **W10b** · **W10c**(재설계) · **W10c′**) · **§E `pending.remove` 폐기 ⇒ P6 무변경** · **P5 재작성**(현실적 파국에 한해 보존) · **P11 복원**(패스 전체 트레이스 바이트 동일) · **Class B-ABA · B′-SELFINVAL · B″-SYNTH · B-6′ · B-REFS 정직 등재** · **F-42 신설 + 적대적 ABA는 백로그로 올리지 않는다는 판정**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r15: needs-attention (1 medium)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-27** | **medium** (confidence 0.98) | ***The grave path can still divert bumps into a fresh tally.*** `Vanished`가 **`Default`를 derive**하므로 `grave()`가 **넘겨받은 패스 집계를 무시하고** `Vanished::default()`를 새로 지어 `rename_durable_source_checked`에 넘겨도 **합법**이다. `SourceGone`은 **진짜**이고 항목은 정상 skip되지만 **패스 집계가 0으로 남아** 루프-후 가드가 **건너뛰어진다** ⇒ 무덤 rename 시점에 `.objects`가 사라지면 `write_atomic`이 **컨테이너를 되살리고 `{}` 원장을 발행하며 `Ok`** 로 끝난다(베이스라인은 **중단**한다). 선언된 증인이 못 잡는다: **W5**는 로컬 집계만 보고 · **W6**는 컨테이너가 살아 있으며 · **W10/W10-TEMP**는 `grave`가 아니라 `Entry::seen`을 탄다 ⇒ *"표현 불가한 계수 자물쇠"* 와 *"파국 탐지"* 주장이 **미증명** | **Accept** | **지적이 정확하다 — 그리고 봉인하려다 *두 가지를 더* 실컴파일로 발견했다(숨기지 않는다).** ① **`Vanished`를 `atomic.rs`에서 들어냈다** — 거기서는 *"reconcile만 만들 수 있다"* 가 **표현 불가**다(`pub(in crate::store::reconcile)` = **`E0433`**(조상 모듈만 허용) · `pub(super)`(=store)나 `pub(crate)`면 **`pins`가 대체 집계를 짓는다** — 반사실 `p27-a`/`p27-b` 둘 다 **BUILD OK**) ⇒ **신규 `src/store/reconcile/absence.rs`**(§구현 ①). **derive 0개**(`Default`·`Clone`·`Copy` 전부 삭제 — 복제본이 곧 대체 집계다) · `new()`/`get()` = **`pub(super)`**(reconcile 서브트리) · `bump()`/`share()` = **absence 모듈 private** ⇒ `pins::grave`의 대체 집계는 **`E0624`/`E0599`/`E0423`** (실컴파일). ② **집계가 둘이었다(부수 결함)** — 구 §⑤가 무덤 루프에 **지역 `Vanished::default()`** 를 만들어 **A2·A4의 발행이 버려지는 집계**를 올렸다 ⇒ **패스 집계 하나를 `run_once_at → PassGuard::begin → recover_graves`로 관통**시켰다(§구현 ④⑤ · **`Vanished::new()` 호출부 = 크레이트 전체 1개**). 행동 델타 0(무덤 루프의 bump 후보 둘 다 **오늘 `?`로 죽는 지점** ⇒ P11 보존) · **§B-3 행 7/§D-②의 *"가드가 아예 안 돈다"* 는 거짓이 됐다**(결론 불변). ③ **★ 봉인이 `pins::tests`도 잠갔다 (적대적 반증이 실컴파일로 잡았다 — `cargo test`가 *빌드조차 안 됐다*)** — `PassGuard::begin` **7곳** + `pass.grave` **2곳** = **호출부 9개**(`pins.rs:665,691,713,736,813,2536,2547,2724,2728`)가 `&Vanished`를 요구하는데 `pins::tests`는 `crate::store::reconcile`의 **후손이 아니다** ⇒ **`E0624`** ⇒ **`#[cfg(test)] pub(crate) fn new_for_test()` 다리가 필수**다. **그 대가를 정직하게 등재한다**: 뮤턴트가 평가되는 빌드가 **바로 그 `cargo test --lib` 빌드**이므로 **M-FRESH는 거기서 컴파일된다** ⇒ **Class A(타입) → A(행동)으로 강등**하고 **`W-GRAVE-CD-A`를 유일한 킬러로 세운다**(**B-TESTBRIDGE**). ⚠ `pub(crate) fn new()`로 다리를 없애는 탈출구는 **반려**한다 — **P-27의 구멍이 그대로 부활**한다. ④ **★ *"M-COUNT 표현 불가"* 를 철회한다** — `bump()` 호출부는 **둘**이고(`entry_is_absent`(async) · `entry_is_absent_blocking`) private `bump()`는 **위조**를 막을 뿐 **한 채널에서의 누락**을 못 막는다 ⇒ **M-NOBUMP를 2행으로 쪼개고 킬러를 각각 붙였다**(async = **W10 ∧ W10-TEMP** · blocking = **W-GRAVE-CD-A**. **서로를 못 덮는다** — 실측: async 쪽 삭제는 W-GRAVE-CD-A 아래에서 **GREEN**). ⑤ **증인 W-GRAVE-CD 신설**(§C-A · 증인 표) — **A**(파괴 · 재생성 없음 ⇒ `Err(NotFound/2)` ∧ 부활 0 ∧ 원장 0 · 양쪽 GREEN = 특성화) · **B**(파괴 → 빈 재생성 ⇒ `Ok` ∧ `post_grave` 0회 ⇒ **`SourceGone`이었음의 직접 증거** · green-only). **둘 다 `flips[]` 미등재**(A는 애초에 플립이 아니고 B는 *같은 하나의 플립*의 추가 증인). **자기무효화 0**(격리 분기·`settle` 복원 rename 전부 도달 불가 — 실행 확인) · **무대 규율이 load-bearing**(비예약 항목이 둘 이상이면 다른 항목이 스스로 집계를 올려 **M-FRESH′를 가린다** — 실측). ⑥ **부수**: `absence.rs`가 rename+fsync를 한 무취소 클로저에 유지하려면 **`atomic::fsync_dir_blocking`을 `pub(crate)`로 넓혀야** 한다(§Scope) |

**하드룰 4 재도달** — 새 critical **0** · high **0** · **medium 1**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **수동 라운드 16 승인**(2026-07-14). 라운드 15가 **14라운드 만에 처음으로 critical/high 0**을 냈고 RED 증명을 sound로 판정했다. P-27은 강제된 국소 수정이다 — `Vanished`에서 `Default`(및 `Clone`/`Copy`) 유도를 떼고 **패스 밖에서 생성할 수 없게** 만들어 `grave()`가 대체 집계를 **짓지 못하게** 한다. Codex의 simpler alternative(*"P5가 필수가 아니니 `Vanished`와 가드를 아예 제거하라"*)는 **반려**한다 — SSD 언마운트가 "깨끗한 GC 패스"로 보고되는 것은 나쁘고, 가드의 비용은 `vanished > 0`일 때의 `metadata()` **1회**뿐이다.

**라운드 16 실행 예정** — 이 개정판(P-27 봉인: **`Vanished` → 신규 `src/store/reconcile/absence.rs`**(derive 0 · `new`/`get` = `pub(super)` · `bump`/`share` = 모듈 private) · **패스 집계 관통**(`Vanished::new()` 호출부 1개) · **`Absent` 발행 5지점 × 집계 연결 전수표(A1~A5)** · **증인 W-GRAVE-CD-A/B** · **`pins::tests` 9개 호출부 + 테스트 다리 명시** · **M-FRESH 강등(A(타입) → A(행동)) · M-COUNT 철회 · M-NOBUMP 2행 분할** · **B-TESTBRIDGE 정직 등재** · **`fsync_dir_blocking` `pub(crate)`**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r16: needs-attention (2 high · 설계 결함 0)

> **라운드 16은 critical 0 · 설계 결함 0**을 냈고 **RED 락과 프로덕션 호출부 증인을 `sound`로 판정**했다.
> 남은 2건은 **규범 의사코드가 라운드 16의 이사(`atomic.rs` → `reconcile/absence.rs`)를 따라가지 못한
> *문서의 낡은 서술*** 뿐이다. Codex 자신이 못박았다: *"Repeat the plan review after these **document-only
> corrections**; **the committed RED record need not be regenerated**."*

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-28** | **high** (confidence 0.99) | ***The grave pseudocode still uses the old `atomic` API.*** 규범 스니펫이 **`atomic::Absent` · `atomic::Vanished` · `atomic::rename_durable_source_checked` · `atomic::Renamed`** 를 참조하는데, **라운드 16이 그것들을 `reconcile/absence.rs`로 옮겼고** `atomic.rs`에는 평범한 durability 헬퍼만 남는다. 또한 **철회된 M-COUNT 주장**(*"bump 하나를 빠뜨리는 것은 표현 불가"*)이 `grave()` doc에 남아 있다. 그 텍스트를 따르면 **컴파일이 실패하거나**, 타입을 `atomic`으로 되돌려 **봉인된 가시성 경계를 무너뜨린다**. **Recommendation**: 살아 있는 **모든 `atomic::` 참조를 `reconcile::absence`** 로 바꾸고, **`pins.rs`가 필요로 하는 헬퍼까지 포함한 import/re-export 맵 하나**를 명시하며, **W5의 위치**를 갱신하고, **낡은 M-COUNT 주장을 두 개의 M-NOBUMP 행동 킬러로 대체**하라 | **Accept** (순수 문서 정합성) | **지적이 정확하다 — 그리고 맵을 실제로 컴파일해 보니 *지적보다 하나 더* 나왔다(숨기지 않는다).** ① **살아 있는 규범 참조 6곳을 전부 이전**했다(§The fix 0 · §A 행 16 · §③의 `Seen`/`Grave`/`grave()` 시그니처·본문 · §③의 "분류하는 코드 두 곳" 문장). **Review Decision Log r1~r15의 `atomic::` 언급은 *역사*이므로 건드리지 않았다.** ② **★ §A-0 신설 — import/re-export 맵을 한 곳에 못박고 스크래치 크레이트로 실컴파일했다.** **문서가 적어 둔 *"`reconcile.rs`는 **타입만** 재수출한다"* 는 거짓이었다**: `mod absence`가 **private**이므로 `pins`는 `reconcile::absence::…` 경로를 지나갈 수 없고, 타입만 재수출하면 `pins::grave`가 **`E0425`(cannot find function `rename_durable_source_checked` in module `super::reconcile`)** 로 **죽는다**(실컴파일). ⇒ **자유함수 `rename_durable_source_checked`를 재수출 목록에 넣어야 한다.** **비대칭이 원인이다**: **연관함수는 타입 재수출을 타고 따라오지만**(`Vanished::new_for_test()`가 `pins::tests`에서 **부를 수 있음**을 `cargo test --lib`로 확인) **자유함수는 스스로 재수출돼야 한다.** ③ **가시성 봉인은 그대로 성립한다 — 모순 없다**(실컴파일): `absence`의 **`pub(super)`는 `pub(in …::reconcile)`이지 `store`가 아니다** ⇒ `pins`에서 **`Vanished::new()` = `E0624`** · **`.get()` = `E0624`** · **`Absent(())` = `E0423`** ⇒ **`pins`는 집계를 짓지도 읽지도 올리지도 못하고 오직 빌려서 전달만 한다.** 반면 `reconcile.rs`의 `pub(super)`는 **`store`** 이므로 **`PassGuard::begin`이 `recover_graves`를 부르는 것은 정상**이다(의도된 비대칭). ④ **정직한 잔여 — 최소 수정 제안**: `entry_is_absent`/`rename_source_checked`의 **`pub(crate)` 선언은 실효보다 넓다**(private 모듈에 갇혀 재수출되지 않으므로 실제 도달 범위 = reconcile 서브트리). **봉인에는 무해**하나 문언이 실제보다 넓다 ⇒ **`pub(super)`로 좁힐 것을 제안**하고 **B-5 diff 항목**에 올렸다(*"재수출이 §A-0의 4심볼로 최소인가"*). **설계 변경이 아니라 선언의 정직화다.** ⑤ **W5의 위치를 `atomic.rs` unit → `reconcile/absence.rs` unit**으로 갱신했다. ⑥ **철회된 M-COUNT 주장 2곳을 제거**하고(`Entry::seen`의 인라인 주석 · `grave()`의 doc) **M-NOBUMP 2행**(async 채널 = **W10 ∧ W10-TEMP** · blocking 채널 = **W-GRAVE-CD-A** · **서로를 못 덮는다**)으로 대체했다. 뮤턴트 표의 `~~M-COUNT~~` 철회 행과 M-NOBUMP-ASYNC/-BLOCKING 2행은 **r15에서 이미 정본이다** ⇒ 본문이 그것을 따라왔다. ⑦ **부수 정정**: M8의 컴파일 에러 코드를 **`E0603` → `E0423`** 으로 고쳤다(실컴파일 — 필드가 private인 튜플 구조체는 **초기화**할 수 없다) |
| **P-29** | **high** (confidence 0.99) | ***The concrete `PassGuard::begin` diff omits the sealed pass tally.*** 그 diff가 여전히 `recover_graves`를 **`&Vanished` 없이** 호출한다 — 요구된 시그니처(§⑤)와 **단일 패스 집계**(§구현 ④)에 **모순**이다. 그리고 **§"왜 두 번째 플립이 없는가" 표의 *무덤 루프 안에서 파괴 → 재생성* 행**이 아직도 *"무덤 루프 소멸은 `vanished == 0`이라 가드를 건너뛴다"* 고 말한다 — **집계 관통 후 거짓**이다. **Recommendation**: `PassGuard::begin(..., vanished: &Vanished)`가 **그 같은 참조를** `recover_graves(..., vanished)`에 넘기는 것을 보여라. 그 행을 *"집계가 양수이고, **가드가 돌며**, 재생성된 디렉터리를 보고 **통과한다**"* 로 정정하라 | **Accept** (순수 문서 정합성) | **지적이 정확하다.** ① **§⑥의 diff를 선언 + 호출부 둘 다로 확장**했다: `begin(store, settle_timeout, vanished: &Vanished)` → **받은 그 참조를 그대로** `recover_graves(&me.layout, me.pins.hooks(), vanished)`에 넘긴다. ⚠ **`pins`는 대신 지을 수 없다**(`E0624` — §A-0) ⇒ **관통 외에는 선택지가 없다.** ② **`begin`의 호출부를 전수로 적었다 — 8개**(프로덕션 `run_once_at` **1** + `pins::tests` **7**: `pins.rs:665·691·713·736·813·2536·2724`). **`grave`는 3개**(프로덕션 1 + `pins::tests` 2). **§Scope의 *"`pins::tests` 9개 호출부"* 는 테스트만 센 것이었다** ⇒ **프로덕션 호출부를 명시**해 총계를 닫았다. ③ **낡은 행을 정정**했다: *"`vanished == 0` → 가드 미발화"* → **"무덤 루프의 부재가 패스 집계를 올린다(A2·A4) → `read_dir` = `Ok(빈 dir)` → 엔트리 루프 0회 → **`vanished > 0` ⇒ 가드가 돈다** → **재생성된 살아 있는 dir**을 보고 **통과**"**. **결론(Class B-ABA · 손실 0)은 불변**이다. ④ **같은 취지의 다른 곳을 전수 확인했다 — 이미 r15가 고쳐 뒀다**: **§B-3 표 행 7** ✔ · **§D-②** ✔ · **P5** ✔ · **§5 B-ABA** ✔ · **Single-Flip Contract 잔여** ✔ ⇒ **낡은 행은 이 한 곳뿐이었다** |

**하드룰 4 재도달** — 새 critical **0** · high **2** · **설계 결함 0**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **수동 라운드 17 승인**(2026-07-14). 라운드 16은 **critical 0 · 설계 결함 0**이고 RED 락과 프로덕션 호출부 증인을 **sound**로 판정했다. 남은 두 건은 **문서의 낡은 서술**뿐이다 — 라운드 16의 사전검증이 `Vanished`를 `atomic.rs` → `reconcile/absence.rs`로 옮기도록 **컴파일 증거로 강제**했는데 규범 의사코드 몇 곳이 따라가지 않았다. Codex 자신이 *"document-only corrections; the committed RED record need not be regenerated"*라고 못박았다. 기계적 정규화로 닫는다.

**라운드 17 실행 예정** — 이 개정판(P-28 봉인: **살아 있는 `atomic::` 규범 참조 6곳 → `reconcile::absence`** · **★ §A-0 import/re-export 맵**(스크래치 실컴파일: *"타입만 재수출"* = **`E0425`** ⇒ **자유함수 `rename_durable_source_checked` 재수출 필수** · 봉인은 `pins`에서 **`E0624`/`E0624`/`E0423`으로 그대로 성립**) · **W5 위치 → `reconcile/absence.rs` unit** · **M-COUNT 잔재 2곳 제거 → M-NOBUMP 2행** · **M8 = `E0423`** · **`entry_is_absent*` `pub(super)` 축소 제안 → B-5** / P-29 봉인: **`begin` diff = 선언 + `&Vanished` 관통** · **호출부 전수(begin 8 · grave 3)** · **무덤-루프 ABA 행 정정("가드가 돌고 통과한다")**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r17: needs-attention (1 high · 설계 결함 0)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-30** | **high** (confidence 0.99) | ***The atomic keep-map deletes the existing commit pipeline.*** §A-0(≈:237)과 §Scope(≈:1233)가 *"`atomic.rs`에는 `write_atomic`·`rename_durable`·`fsync_dir{,_blocking}`·`mkdir_p_durable`**만** 남는다"* 고 **배타적 목록**으로 썼다. 그런데 red.sha에서 **`pins.rs:369`가 `atomic::stage_blocking` → `Staged::commit_blocking`** 을 부르고 **`objects.rs:75`가 `atomic::unique_suffix`** 를 부르며, `rename_durable_blocking`·`mkdir_p_durable_blocking`도 내부에서 필요하다. 그 텍스트를 따르면 **컴파일이 실패하거나**, **F-1의 취소불가 포인터-커밋과 durable-directory 기계장치를 근거 없이 재작성**하게 되어 **F-14와 무관한 취소·내구성 회귀**를 부른다. **Recommendation**: 배타적 keep-list를 **포함적 서술**(*"기존 `atomic` API는 **전부 그대로** 남는다"* + **부재 관련 심볼만 `absence.rs`에 신설** + **`fsync_dir_blocking`의 가시성만 변경**)로 교체하고 **두 곳을 모두** 고쳐라 | **Accept** (순수 문서 정합성) | **지적이 정확하다 — 그리고 그 문장은 지휘자의 지시(*"`atomic.rs`에 무엇이 남는지 적어라"*)가 **배타적 목록**으로 쓰인 사고였다. 설계는 한 글자도 걸려 있지 않다.** ① **`atomic.rs`의 pub 표면을 red.sha에서 전수 확인**했다: `write_atomic`(`pub`) · `fsync_dir`(`pub`) · `mkdir_p_durable`(`pub`) · `rename_durable`·`rename_durable_blocking`·`stage_blocking`·`Staged`·`Staged::commit_blocking`·`unique_suffix`(`pub(crate)`) · 모듈 private `fsync_dir_blocking`·`mkdir_p_durable_blocking`. **호출부 확인**: `pins.rs:369-371`(`stage_blocking`+`commit_blocking` — **T-C1/T-C2가 지키는 무취소 커밋**) · `objects.rs:75`(`unique_suffix`) · `pins.rs:480,611` / `reconcile.rs:123`(`rename_durable`) ⇒ **하나도 지울 수 없다.** ② **배타적 목록 2곳을 포함적 서술로 교체**했다(§A-0 · §Scope) — *"기존 API는 **전부 그대로** 남는다(삭제 0 · 이동 0)"* + *"이 픽스가 `atomic.rs`에 가하는 변경은 **`fsync_dir_blocking`의 가시성 확대 단 하나**"*. ③ **★ *"이동(move)"* 이라는 표현을 정정했다(문서가 사실과 달랐다)** — 부재 관련 심볼(`Absent`·`Vanished`·`Renamed`·`entry_is_absent*`·`rename_*_source_checked`)은 **red.sha의 `atomic.rs`에 존재하지 않는다**(F-14가 **새로 만드는** 심볼이다) ⇒ **`atomic.rs` → `absence.rs` 이사가 아니라 `reconcile/absence.rs` *신설*** 이다. 옮겨진 것은 코드가 아니라 **초기 개정판의 계획**이고, 그 계획을 **r15/P-27이 컴파일로 반려**했다(라운드 16의 컴파일 증거). ④ **§Scope를 파일별 표로 다시 썼다** — 5파일 각각에 **신설/수정/가시성** 라벨을 붙여 **`atomic.rs`가 가시성 1건 외에는 무변경**임이 표면에서 바로 읽히게 했다. ⑤ **부수 정정**: Single-Flip Contract의 `scope[]` 산문이 **신규 `reconcile/absence.rs`를 빠뜨리고** 있었다(§구현 ①이 신설하는 파일인데 경로 열거에 없었다) ⇒ **추가**했다. **설계 변경 0 · 증인 0 · 뮤턴트 0 · `flips[]`/`red.sha`/`characterizationCmd` 불변.** |

**하드룰 4 재도달** — 새 critical **0** · high **1** · **설계 결함 0**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **수동 라운드 18 승인**(2026-07-14). 라운드 17도 **critical 0 · 설계 결함 0**이다(설계에 대한 결함은 **3라운드 연속 0**). P-30은 지휘자가 라운드 17에서 *"`atomic.rs`에 무엇이 남는지 적어라"*라고 지시한 것이 **배타적 목록**으로 쓰인 데서 왔다 — 그 문장은 **F-1의 취소불가 커밋 파이프라인(`stage_blocking`·`Staged`·`unique_suffix`)을 지운다**. **포함적 서술**로 교체한다: `atomic.rs`의 기존 API는 **전부 그대로** 남고, 이번 픽스가 그 파일에 가하는 변경은 **`fsync_dir_blocking`의 가시성 확대 단 하나**다. 부재 관련 심볼은 `atomic.rs`에서 *옮기는* 것이 아니라 **`reconcile/absence.rs`에 신설**하는 것이다(라운드 16의 컴파일 증거). 설계는 한 글자도 바뀌지 않는다.

**라운드 18 실행 예정** — 이 개정판(P-30 봉인: **배타적 keep-list 2곳(§A-0 · §Scope) → 포함적 서술**(`atomic.rs` 기존 API **전수 열거** · **삭제 0 · 이동 0** · 변경은 **`fsync_dir_blocking` 가시성 1건**) · **"이동" → "`absence.rs` 신설"** 로 정정 · **§Scope = 파일별 신설/수정/가시성 표** · **`scope[]` 산문에 `reconcile/absence.rs` 추가**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r18: needs-attention (1 high · 설계 결함 0)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-31** | **high** (confidence 0.99) | ***The `Entry` helpers are unreachable from the reconcile loop.*** §구현 ②의 규범 스니펫이 **FS 메서드 6개(`file_type`·`metadata`·`read`·`remove`·`rename_into`·`rename_durable_to`)를 private으로** 선언한다. 그런데 그것들을 부르는 것은 **`reconcile.rs`**(= `entry`의 **부모**)다 ⇒ 그 텍스트를 그대로 옮기면 호출부 전부가 **`E0624`(private method)** 로 죽는다. 같은 스니펫의 **`impl Entry`** 도 `Entry<'v>`와 맞지 않는다. **Recommendation**: FS 메서드 6개(+`snapshot`/`name`/`class`)를 **`pub(super)`**(= reconcile 서브트리)로 올리고 **`seen()`만 private**으로 유지하라 — 봉인은 그대로 선다 | **Accept** | **지적이 정확하다. 그리고 이번에는 논쟁하지 않고 *컴파일했다*(P-21에서 통한 수).** ① **D안 설계 *전체*를 red.sha 복제본(스크래치)에 실제로 구현**했다 — `cargo build` **경고 0** · `cargo test --lib --bins --tests` **전부 GREEN**(lib **123 passed**) · **회귀 증인 2개 RED → GREEN**. ② **P-31 봉인**: `Entry`의 **FS 메서드 6개 + `snapshot`/`name`/`class` = `pub(super)`** · **`seen()`만 모듈 private** · **`impl<'v> Entry<'v>`**(`snapshot`은 자체 `<'v>`를 재선언하지 않는다). ③ **§구현의 모든 규범 스니펫을 프로토타입의 실제 소스로 교체**했다 — **의사코드가 아니라 실코드다**(그 사실을 §구현 머리말에 명시). ④ **★ §E-COMPILE 신설** — *"의사코드를 그대로 쓰면 죽는 곳"* **5건 전수**: **1** `impl Entry` + `snapshot<'v>` = lifetime 불일치 · **2** `rename_durable_source_checked`를 `pub(super)`로 좁히면 `pub(crate) use` 재수출이 **`E0364`**(⇒ **`pub(crate)` 유지** · 좁힌 것은 `entry_is_absent`·`rename_source_checked` **둘뿐**) · **3** `Seen::Gone(Absent)`/`GraveOutcome::SourceGone(Absent)`는 **`#[allow(dead_code)]` 필수**(경고 0 정책) · **4** `grave` 매치는 **`let g = match …`로 분리**해야 한다(arm의 `?`/`continue`로 타입이 갈린다) · **5** Temp의 `remove`는 **let-else 불가**(`match`여야 `Gone`에서 `temps_deleted`가 안 오른다). ⑤ **봉인을 컴파일러로 실증**: `pins`에서 `Vanished::new()`/`.get()` = **`E0624`** · `Absent(())` = **`E0423`** · 자유함수 재수출 누락 = **`E0425`**. **`Vanished::new()` 코드 호출부 1개** · `new_for_test` 호출부 **9개**(begin 7 · grave 2) — **계획의 계수와 정확히 일치**. ⑥ **행동 보존 6렌즈를 실행으로 반증 시도했고 전부 살아남았다**(두 트리에 동일 바이트 프로브 11 시나리오 → 출력 **완전 동일**) · **뮤턴트 킬 3건 실측**(M-NOBUMP-BLOCKING → `W-GRAVE-CD-A`**만** RED ⇒ *"두 채널은 서로를 못 덮는다"*가 **실행으로 확인**). ⑦ **★ 치명 아닌 결함 2건을 정직하게 기록한다.** **(가) 허위 수치를 폐기했다** — 프로토타입의 최초 보고가 characterization을 *"118 passed … 합계 **138** ← red.sha와 정확히 동일"* 이라고 적었으나 **재현 불가(허위)** 다: 픽스가 lib를 120 → **123**으로 늘리고 `--skip`은 2개뿐이므로 **121 / 합계 141**이 실제 출력이다(적대적 검증이 실행으로 반증). ⇒ **acceptance §2와 P13을 트리별 표로 다시 썼고 *"138과 동일"* 이라는 문장을 폐기**했으며 **게이트를 합계가 아니라 `0 failed`로** 바꿨다. **요약을 원문으로 위장하는 실패 양식이 18라운드째 재현됐다 — 숨기지 않는다.** **(나) W10b 부재가 차단 요건이다** — 프로토타입은 증인을 **3개만**(W10 · W-GRAVE-CD-A · W3) 구현했고, **W10b가 없으면 `vanished.get() > 0` 게이트를 지우는 뮤턴트(M-GUARD-ALWAYS)가 `--lib --bins --tests` 전부 GREEN으로 살아남는다**(실측) ⇒ **두 번째 플립을 막는 유일한 방벽이 증인 0으로 출하된다.** **M-GUARD-ALWAYS 행과 acceptance에 차단 요건으로 못박았다.** ⑧ **미확인 항목을 acceptance에 전수 등재**했다(미구현 증인 목록 · 9번째 훅의 증인 W11 부재 · §7-a~7-d 문서 개정 미수행 · `--release` 2줄 미실행 · B-ABA 잔존). **설계 변경 0 · 증인 명세 0 · `flips[]`/`red.sha`/`characterizationCmd` 불변.** |

**하드룰 4 재도달** — 새 critical **0** · high **1** · **설계 결함 0**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **프로토타입 후 라운드 19 승인**(2026-07-14). r16·r17·r18이 연속으로 **의사코드의 컴파일 오류**를 하나씩 잡아냈다(잘못된 `atomic::` 경로 · 배타적 keep-list · `E0624`). 전부 진짜지만 **하나씩 잡는 것은 낭비다.** P-21에서 통한 수를 다시 쓴다 — **논쟁 말고 컴파일**: D안 설계 **전체**를 스크래치 복제본에 **실제로 구현해 컴파일과 스위트를 통과시키고**, 계획의 규범 코드를 **실제로 컴파일되는 소스에서 그대로 옮겨 적는다**. 프로토타입 코드는 **저장소에 넣지 않는다**(B1 배리어 — 구조 게이트 전 fix 코드 금지). 계획에 **사실만** 옮긴다.

**라운드 19 실행 예정** — 이 개정판(P-31 봉인: **§구현 전체를 프로토타입의 실컴파일 소스로 교체**(`Entry`의 FS 메서드 6개 = `pub(super)` · `seen` = private · `impl<'v> Entry<'v>`) · **★ §E-COMPILE 5건 전수** · **characterization 수치 정정(픽스 트리 = 141 · `0 failed`로 게이트)** · **W10b = 이식 차단 요건으로 등재** · **acceptance = 확인/미확인 분리**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r19: needs-attention (1 high · 설계 결함 0)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-32** | **high** (confidence 0.99) | ***The plan mandates an uncharacterized second observable behavior.*** *"부수 행동 변화"* 절(≈:1731-1734)이 **skip된 항목 이름에 대해 새 `tracing::debug!` 이벤트**가 난다고 선언하는데, **실컴파일이 증명된 규범 구현(§구현 ④·⑤의 축자 전사)에도, 파일별 Scope에도 그런 이벤트가 없다.** 이 요구를 따르면 **잠긴 단일 플립(`Err`→`Ok`) 밖의 로그 볼륨과 항목명 텔레메트리**가 생기고, 규범 코드를 따르면 **선언된 요구를 빠뜨린다.** 어떤 acceptance 증인도 그 이벤트를 특성화하지 않는다. **Recommendation**: **로깅 주장을 삭제**하고 **기존 로깅 행동을 그대로 보존한다**고 명시하라. 그 이벤트가 의도된 것이라면 **자기 자신의 파이프라인과 acceptance 계약을 요구하는 별도의 관측 행동 변화**로 다루라. **Open question**: *"is the debug event intentional or stale?"* | **Accept** (순수 문서 정합성) | **지적이 정확하다 — 그리고 open question의 답은 *stale*이다.** ① **스크래치 프로토타입(실컴파일 · 전 스위트 통과)은 그 이벤트를 내지 않는다**: 엔트리 루프(§구현 ④)와 무덤 루프(§구현 ⑤)의 **모든 skip 경로(`continue`)가 침묵**이고, §⑤의 *"루프 본문의 변경은 넷뿐이다"* 열거에도 로그가 **없다**. 계획의 **낡은 서술이 남은 것**이다(설계는 한 글자도 걸려 있지 않다). ② **로깅 주장을 삭제**하고 그 자리에 **명시적 보존 선언**을 넣었다: *"**로깅 행동은 오늘과 동일하다** — 이 픽스는 `tracing` 이벤트를 **추가하지도 제거하지도 변경하지도 않는다.** 사라진 항목을 건너뛸 때 **아무 로그도 내지 않는다.**"* ③ **기존 `tracing` 사용을 실코드 grep으로 전수 확인해 축자 보존을 명시**했다 — **수정 대상 2파일의 5곳이 전부**다: `reconcile.rs:127` *"grave recovered"* · `:155` *"graves recovered from a previous pass"* · `:222` *"quarantined corrupt blob (bit rot)"* · `:245` *"GC restored: landed commit"* · `pins.rs:597` settle `TimedOut → Deferred`의 *"gc settle timed out …"*. **정정**: `LOCK_WARN_AFTER`의 `tracing::error!`는 **`pins.rs`가 아니라 `locks.rs:116`**에 있고 **`locks.rs`는 이 픽스가 열지 않는 파일**이다(`pins.rs:33·329·1079`의 언급은 **문서 주석**이다). **크레이트 전체에 `tracing::debug!`는 0건**이다. ④ **`Preserved Contract`에 `P16`(로깅 보존) 신설** — **핀하는 증인은 정직하게 *없음*으로 등재**했다(스위트가 `tracing` subscriber를 설치하지 않아 **로그를 관측하는 증인이 원리적으로 없다**) ⇒ **diff 리뷰가 막는다**(**B-5**). ⑤ **"부수 행동 변화" 절을 전수 재검토**했다 — 나머지 2항목은 **규범 구현과 일치하고 증인도 있다**(항목 2 = 가시성 봉인 → **§A-0의 `E0624`/`E0423` 실컴파일** · 항목 3 = 에러 경로 syscall → **P11 · W2 · W13**) ⇒ **같은 종류의 자기모순은 더 없다.** 증인 포인터를 각 항목에 명시하고, 항목 2에는 **B-TESTBRIDGE**(테스트 다리가 `--lib`에서 봉인을 연다)를 정직하게 붙였다. ⑥ **로그 볼륨/항목명 텔레메트리를 여는 일은 `F-29`**(관측성 카운터 파이프라인)로 **이미 등재되어 있다** — 별도 플립이다. **설계 변경 0 · 증인 명세 0 · 뮤턴트 0 · `flips[]`/`red.sha`/`characterizationCmd`/`scope[]` 불변.** |

**하드룰 4 재도달** — 새 critical **0** · high **1** · **설계 결함 0**. r2에서 이미 2라운드 상한에 걸렸으므로 인간 판정이 필요하다.

**인간 판정 (하드룰 4)**: **수동 라운드 20 승인**(2026-07-14). Codex의 open question(*"is the debug event intentional or stale?"*)에 답한다: **stale이다.** 실컴파일 프로토타입은 그 이벤트를 내지 않는다 — 계획의 낡은 서술이 남았을 뿐이다. 로깅 주장을 **삭제**하고 *"로깅 행동은 오늘과 동일하다"*를 **명시적 보존 선언**으로 못박는다. 설계 결함은 **4라운드 연속 0**이다.

**라운드 20 실행 예정** — 이 개정판(P-32 봉인: **skip 시 새 로그 주장 삭제 → 명시적 보존 선언**(*"`tracing` 이벤트를 추가·제거·변경하지 않는다 · 건너뛸 때 아무 로그도 내지 않는다"*) · **기존 `tracing` 5곳의 축자 보존을 실코드 grep으로 명시**(+ `LOCK_WARN_AFTER`는 `locks.rs:116` — 정정) · **`P16`(로깅 보존) 신설 — 증인 없음으로 정직 등재** · **"부수 행동 변화" 절 전수 재검토**(나머지 2항목은 규범 구현·증인과 일치))을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r20: needs-attention (1 high · 설계 결함 0)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-33** | **high** (confidence 0.99) | ***P16 conflates unchanged trace call sites with unchanged runtime logging.*** P16이 *"로깅 행동이 동일하다"* 고 쓰고 *"스위트가 tracing subscriber를 설치하지 않는다"* 는 이유로 증인을 면제한다. **그러나 픽스는 사라진 항목 뒤에도 계속 돌므로 베이스라인이 중단하던 자리에서 기존 이벤트(격리 WARN · GC 복원)가 발화할 수 있다 — 호출부가 안 바뀐다고 로그 스트림이 같지는 않다.** 게다가 **근거 전제가 거짓이다**: red.sha의 **`pins.rs:995-1037`이 `CaptureSubscriber`를 정의하고 4개 테스트가 그것을 설치해 이벤트 수를 정확히 단언한다.** **Recommendation**: P16을 *"호출부·스키마 보존 + skip 전용 이벤트 없음"* 으로 좁히고 **완주로 도달되는 하류 이벤트를 허용된 플립의 일부로 명시**하라 · **`CaptureSubscriber`를 재사용·확장**해 증인을 세우라. **Open question**: *"패스가 성공적으로 계속된 뒤 새로 도달되는 하류 로그는 단일 플립의 일부로 명시되어 있는가?"* | **Accept** | **지적이 정확하다 — 그리고 근거 전제가 거짓이라는 지적도 정확하다.** ① **로그 스트림을 실제로 캡처해 측정한 뒤 계약을 다시 썼다**(두 트리 동일 프로브 · 레벨-무관 구독자): **S1(소멸 0) = 바이트 동일**(3 이벤트 · diff 무출력 — 적대적 반증이 md5 `a97d0942…`로 **독립 재현**) · **S2(소멸 1) = red `Err`+0 이벤트 → D안 `Ok`+격리 WARN ×2** · **S3(skip) = 모든 레벨 0건**. ⇒ **P16을 3항(호출부·스키마 보존 / skip 침묵 / 하류는 플립의 하류)으로 재작성**하고 **§Single-Flip Contract에 "하류 범위" 절**을 신설해 *"완주한 패스가 하는 모든 일(기존 로그 포함)이 그 플립의 하류"* 임을 명시했다(B7·stats·격리·GC 보존과 **무모순**임을 논증: 그것들은 **연산의 의미론**이고 하류 도달성은 **그 연산이 실행되느냐**의 문제다). ② **★ 적대적 반증이 하류 열거의 불완전을 fatal로 잡았고 그대로 반영했다** — 초안이 *"엔트리 루프 안의 넷"* 이라 못박은 것은 **틀렸다**: **(가) `reconcile.rs:190` INFO `graves recovered from a previous pass`** 는 **무덤 루프 종료 직후**에 나므로 그 넷에 없었다(실측 S4: 무덤 소멸 시 red는 `rename_durable`의 `?`에서 죽어 **못 낸다**) · **(나) 이벤트는 *사라지기도* 한다** — `main.rs`의 호출부가 `Err`→`Ok`로 뒤집혀 **오늘 매 패스마다 뜨던 WARN `reconcile failed`가 침묵하고 INFO `reconcile`(`?stats`)이 새로 도달한다**(운영자 알람이 멈춘다). ⇒ **하류 목록을 4곳 → 7곳 · 두 방향 표**로 다시 썼다. ③ **증인 W-LOG 신설** — `CaptureSubscriber`를 **재사용·확장**(`EventTap`: `enabled()`의 **INFO 상한 해제** + `target` 캡처. ⚠ 그대로 쓰면 `debug!` 뮤턴트를 **못 본다** — 실측). **W-LOG-A**(특성화 · 소멸 0 스트림 전수·순서) · **W-LOG-B**(green-only · 하류 WARN 정확히 N−1) · **W-LOG-C**(green-only · skip 시 모든 레벨 0건). **red RED / D안 GREEN · 10회 반복 flaky 0 · 기존 `CaptureSubscriber` 4개 테스트 무손상**(`--test-threads=16` 스트레스 포함 — `set_default`는 스레드-로컬이라 전역 간섭 0). **뮤턴트 2종 실측 사망**: M-LOG-DEBUG(킬러 = W-LOG-C) · M-LOG-SUPPRESS-DOWNSTREAM(킬러 = **W-LOG-B뿐**) — **둘 다 기존 스위트 123개는 전부 GREEN으로 통과한다** ⇒ **W-LOG가 유일한 킬러다.** ④ **★ 정직 — 증인이 계약보다 좁다.** 적대적 반증이 실행으로 보였다: **W-LOG-C가 밟는 skip 팔은 `:252`(Blob `read`) 하나뿐**이고, **Temp 팔(`:236`)에만** 로그를 넣는 뮤턴트(**M-LOG-DEBUG-TEMP**)와 **무덤 루프 팔**에 INFO를 넣는 뮤턴트(**M-LOG-INFO-GRAVE**)가 **전 스위트를 통과해 살아남는다**. ⇒ **P16 ②의 문언을 실측이 뒷받침하는 범위로 좁히고** **W-LOG-D**(Temp `:236` + 무덤 `:133`/`:154`)를 **W10b와 같은 등급의 이식 차단 요건**으로 등재했다. ⑤ **정직 — 프로브 수치는 계획에서 내렸다**: 폐기용 진단 프로브의 md5·테스트 합계는 두 세션이 동시에 트리를 만지며 **재현 불가**가 됐다(반증 실측) ⇒ 계획에는 **재현된 것만** 남긴다(S1 md5 · 뮤턴트 생존/사망 · `0 failed` 게이트). **합계로 게이트하지 않는다.** ⑥ **시야 한계 등재**: `EventTap`은 **`files*` target · `set_default` 스레드**만 본다(무필터 구독자로 **그 밖의 발화 0건** 확인) ⇒ **B-5 diff 항목**. **설계 변경 0 · `flips[]`/`red.sha`/`characterizationCmd`/`scope[]` 불변.** |

**하드룰 4 재도달** — 새 critical **0** · high **1** · **설계 결함 0**.

**인간 판정 (하드룰 4)**: **수동 라운드 21 승인**(2026-07-14). Codex의 open question에 답한다: **그렇다 — 명시해야 한다.** 플립이 *"패스가 완주한다"*이므로 **완주한 패스가 하는 모든 일(기존 로그 포함)은 그 플립의 하류**이며, 계획이 그것을 명시하지 않은 것이 결함이었다. 그리고 **라운드 19에서 내가 쓴 근거가 거짓이었다** — *"스위트가 tracing subscriber를 설치하지 않는다"*는 **확인 없이 단언**한 것이고, 실제로는 `pins.rs:995-1037`의 `CaptureSubscriber`를 4개 테스트가 설치해 이벤트 수를 단언한다. **20라운드째 같은 실패 양식**(요약을 원문으로 위장 · 확인 없이 단언)이다. 이번엔 **로그 스트림을 실제로 캡처해** 무엇이 달라지는지 측정한 뒤 계약을 썼고, `CaptureSubscriber`를 재사용해 **skip 경로의 침묵**과 **하류 이벤트**를 증인으로 만들었다.

**라운드 21 실행 예정** — 이 개정판(P-33 봉인: **P16 = 3항 재작성**(호출부·스키마 보존 / skip 침묵 — **증인 범위까지 정직하게** / 하류는 단일 플립의 하류) · **§하류 범위 신설**(7곳 · **두 방향** — `graves recovered from a previous pass`와 `main.rs`의 **WARN 소멸**을 포함 · B7·stats·격리·GC 보존과의 무모순 논증) · **증인 W-LOG-A/B/C 추가 + W-LOG-D를 차단 요건으로 등재** · **뮤턴트 4행 추가**(2종 실측 사망 · **2종 실측 생존** — 정직) · **§부수 행동 변화 1항 = S1 바이트 동일 실측으로 재작성**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r21: needs-attention (1 high · 설계 결함 0)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-34** | **high** (confidence 0.99) | ***Blocking W-LOG witnesses can remain undiscovered.*** 전수 테스트-변경 scope가 **`src/store/pins/tests/log_witness.rs`와 그 `mod log_witness;` 등록을 빠뜨렸다** — **W-LOG-D를 다른 곳에서 *차단 요건*으로 선언해 놓고도.** **Rust는 `src/store/pins/tests` 아래 파일을 자동 발견하지 않는다** ⇒ **모든 광범위 cargo 명령이 W-LOG-A/B/C/D를 컴파일조차 하지 않고 `0 failed`를 보고할 수 있다** ⇒ 문서화된 로깅 뮤턴트가 **그대로 살아남는다.** **Recommendation**: `log_witness.rs`와 `mod log_witness;`를 **B-1과 Scope에 명시**하고, **acceptance가 스위트를 돌리기 전에 모든 W-LOG 테스트 ID가 *발견*되는지 검증**하게 하라 — **0개 발견이 통과가 될 수 없도록.** | **Accept** — **그리고 일반화한다**(인간 판정) | **지적이 정확하다 — 그리고 W-LOG만의 문제가 아니었다.** ① **실측으로 재현했다**(프로토타입): `mod log_witness;` **한 줄**을 지우면 **`cargo test --lib --tests`가 exit 0** · lib **131 passed → 128 passed; 0 failed** · 다른 타깃 전부 `0 failed` · **경고 0**(컴파일되지 않으니 `dead_code`조차 안 난다) ⇒ **W-LOG-A/B/C가 통째로 증발하는데 스위트는 초록이다.** ② **§Scope의 테스트-변경 항목을 *파일 × `mod` 등록* 전수표로 재작성**했다 — 빠져 있던 것은 `log_witness.rs` **하나가 아니었다**: `vanished_container_witnesses.rs`(W10/W10-TEMP/W10-G/W10b/W-GRAVE-CD-A/B/W6/W6b) · `recover_graves_production_seam.rs`(W11)의 **`mod` 줄도 명시되지 않았고**, **W3·W4·W-LOG 계열은 Scope의 열거에 아예 없었다.** ⇒ **파일 11개 × 등록 방식**(인라인 `mod tests` / `mod` 1줄 / cargo 자동 발견)을 표로 못박고, **`tests/*.rs`만 자동 발견된다**는 비대칭을 **경고로** 박았다. ③ **★ 발견 단언을 acceptance의 0단계로 신설**(스위트보다 **먼저** 돈다): `cargo test --lib --tests -- --list`의 출력에 **ID 표의 모든 증인 ID가 존재해야 하고, 하나라도 없으면 exit 1**. **실행 확인**: `--tests`가 **lib 타깃을 포함**하므로 한 번의 `--list`가 **lib(131) + 통합(20) = 151개 ID**를 전부 열거한다(원문 수치). 뮤턴트 아래에서 **`MISSING WITNESS: w_log_a…/b…/c…` · exit 1** ⇒ **M-NOMOD 사망**(실측). ④ **★ 증인 ID 표를 정본으로 세웠다** — 34행(회귀 2 · 대조군 2 · W1~W17 · W-LOG-A~D · W13 3페이즈 · W-REG). **7개는 프로토타입에서 실행된 확정 이름**이고 나머지는 **규범 이름**(구현 시 이 이름으로 만들고, 바꾸려면 표와 ID 목록을 **같은 커밋에서** 갱신한다). *"이 표에 없는 증인은 존재하지 않는 것으로 간주한다."* ⑤ **크레이트 내부 절반 — W-REG 신설**(Codex가 제안하지 않은 것 · 더 강하다): `pins.rs`의 인라인 테스트가 **`current_exe() --list`로 자기 바이너리의 테스트 레지스트리**를 물어 lib 증인의 등록을 단언한다 ⇒ **`cargo test --lib` 한 줄만 돌려도 M-NOMOD가 RED**(실측 **exit 101**). **소스 문자열을 읽지 않는다** ⇒ 라운드 7이 폐기한 W8/W12(소스-문자열 규율)의 부활이 **아니다** — 보는 것은 **컴파일된 산출물의 레지스트리**다. **한계(정직)**: `current_exe()`는 **lib 바이너리만** 본다 ⇒ 통합 증인(e2e · W13)은 **0단계 셸이 덮는다.** ⑥ **⚠ 플랫폼 분할이 load-bearing임을 실측으로 발견했다** — **`#[cfg(target_os="linux")]` 테스트는 macOS의 `--list`에 *나오지 않는다***(주입 실험: **0건**) ⇒ ID 목록을 **`IDS_ALL` / `IDS_UNIX` / `IDS_LINUX`로 분할**하지 않으면 개발기에서 **거짓 RED**가 나고, 그것을 잠재우려 목록을 깎는 순간 **단언 자체가 약화**된다(W17이 정확히 그 경우다 — B-12). ⑦ **⚠⚠ 발견 단언이 *못* 잡는 것을 정직하게 등재했다**: **B-IGNORE** — **`#[ignore]`가 붙어도 테스트는 여전히 `--list`에 나온다**(실측) ⇒ 발견 단언 **통과** · 스위트 `0 failed; 1 ignored` ⇒ **스킵된 red = 위조된 red = 하드룰 9 위반** ⇒ 보상 통제로 **`0 ignored` 게이트**를 acceptance에 넣었다(**거짓 불변식이 아님을 실측**: 저장소 전체의 기존 `#[ignore]` = **0건**). **B-DISCOVERY** — 개명(표를 함께 고치면 통과) · **빈 본문**(`assert!(true)`도 통과) · ID 목록에서의 **삭제**는 못 잡는다 ⇒ **발견은 *존재*를 증명할 뿐 *내용*을 증명하지 않는다**(내용은 뮤턴트 표가, 삭제는 **B-5 diff 리뷰**가 맡는다). ⑧ **뮤턴트 M-NOMOD · M-NOMOD′ 신설**(킬러 = 발견 단언 ∧ W-REG). **설계 변경 0 · 프로덕션 코드 변경 0 · `flips[]`/`red.sha`/`characterizationCmd`/`scope[]` 불변.** |

**하드룰 4 재도달** — 새 critical **0** · high **1** · **설계 결함 0**(5라운드 연속).

**인간 판정 (하드룰 4)**: **수동 라운드 22 승인**(2026-07-14). P-34는 이 파이프라인이 21라운드 동안 잡아 온 것의 **극한**이다 — *"증인이 아무것도 증명하지 못한다"* → *"증인이 프로덕션을 안 탄다"* → *"증인이 테스트 빌드에서만 산다"* → **"증인이 아예 컴파일되지 않는다."** W-LOG만 고치지 않고 **일반화**한다: **계획이 선언한 모든 증인**에 대해 **발견 단언**을 acceptance의 **첫 단계**로 건다 — `cargo test -- --list`에 **선언된 모든 증인 테스트 ID가 존재해야** 하고, 하나라도 없으면 실패한다. **0개 발견이 통과가 될 수 없다.** 이 한 줄이 미등록 증인 클래스 전체를 죽인다.

**라운드 22 실행 예정** — 이 개정판(P-34 봉인: **§Scope = 테스트 파일 × `mod` 등록 전수표**(+ *"`pins/tests/` 아래는 자동 발견되지 않는다"* 경고) · **★ acceptance 0단계 = 발견 단언**(`-- --list` 원문 · 플랫폼 3분할 · `0 ignored` 게이트) · **★ 증인 ID 표 = 정본**(확정 7 + 규범 27) · **W-REG**(크레이트 내부 레지스트리 자기검사 — `cargo test --lib`에서 M-NOMOD를 **exit 101**로 죽인다) · **뮤턴트 M-NOMOD/M-NOMOD′ 신설** · **B-IGNORE · B-DISCOVERY 정직 등재**(하드룰 9 연결))을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r22: needs-attention (2 high · 설계 결함 0)

> **라운드 22는 설계 결함 0**(6라운드 연속)이다. 두 건 모두 **라운드 21이 새로 세운 발견 단언 그 자체의
> 결함**이다 — *"게이트가 게이트가 아니었다."*

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-35** | **high** (confidence 0.99) | ***The discovery assertion's anchor rejects valid integration witnesses.*** 여러 필수 증인이 **통합 테스트 크레이트의 최상위 함수**인데 libtest는 그것을 **`<id>: test`**(`::` **없이**)로 열거한다. 그런데 발견 루프는 **`::<id>: test`** 를 요구한다 ⇒ **올바로 구현된 W4·W7·W9a·W9b·W10c·W10c′·W17과 W13-E/G/T가 스위트가 돌기도 전에 MISSING으로 보고된다**(거짓 RED). **Recommendation**: 앵커를 **`(^\|::)<id>: test$`** 로 바꾸거나 **정본 표에 타깃 한정 이름**을 저장해 정확히 매칭하라 | **Accept** | **지적이 정확하다 — 프로토타입에서 실행으로 재현했다.** ① **타깃별 `--list` 형태를 원문으로 확정**했다(§B-1 0-a): **lib** = `store::pins::tests::log_witness::w_log_a_…: test`(모듈 경로 전체) · **통합 최상위** = `phase_e_…: test`(**`::` 없음**) · **통합 중첩 모듈** = `nested_probe::…: test`(**모듈-상대** — 크레이트·타깃 이름은 **붙지 않는다**) ⇒ **세 형태가 공존한다.** ② **거짓 MISSING을 실행으로 보였다**: 옛 앵커는 `w_log_a…`(lib 중첩)는 찾고 `phase_e_…`(통합 최상위)에서 **`MISSING WITNESS` · exit 1**을 낸다 · 새 앵커 `(^\|::)<id>: test$`는 **둘 다 찾는다**(exit 0). ③ **⚠ 피해가 선언된 증인만이 아니었다** — 저장소의 **기존** 테스트도 **22개가 `::` 없는 최상위**이고 그중 `put_stream_midflight_temp_observed_and_preserved`(**P8**이 핀한다) · `symlinked_commit_pointer_current_behavior`(**P12**) · `on_disk_layout_golden_tree`(**P10**)가 포함된다. ④ **⚠ 타깃 경계는 stdout에 없다**(실측: `Running …` 줄은 **stderr**) ⇒ `--lib --tests`를 한 번에 리다이렉트하면 **어느 ID가 어느 바이너리에서 왔는지 사라진다** ⇒ **Codex의 두 번째 대안(타깃 한정 이름)을 채택**해 **타깃별로 따로 `--list`를 묻는다.** ⑤ **⚠ 부수 발견 — 셸 판본 자체가 취약했다**: r22의 `for id in $REQUIRED`는 **워드분할에 의존**하는데 **zsh에서는 분할되지 않아** 루프가 *"여러 줄 패턴 = OR"* 로 붕괴해 **아무거나 하나만 맞아도 통과**한다(실측) ⇒ **스크립트에 `#!/usr/bin/env bash` 셔뱅을 박고 `while IFS='\|' read`로 바꿨다** |
| **P-36** | **high** (confidence 0.99) | ***The `0 ignored` gate is prose only.*** **ignored 수를 파싱해 실패시키는 실행 가능한 명령이 없다** ⇒ 계획이 스스로 문서화한 **`1 ignored` 공격(B-IGNORE)에 대해 cargo가 성공으로 종료한다.** `0 ignored`를 게이트라고 써 놓고도 **B-IGNORE가 열린 채로 남는다.** **Recommendation**: **모든 스위트 결과를 캡처해 ignored가 0이 아니면 nonzero exit**하라. 또는 **`#[ignore]` 추가를 거부**하거나 **`--include-ignored`로 실제로 실행**하라 | **Accept** | **지적이 정확하다 — r21이 *"보상 통제 = `0 ignored` 게이트"* 라고 적어 놓고 그 게이트를 **만들지 않았다**.** ① **실증**(뮤턴트 **M-IGNORE** — W-LOG-C(lib)와 W13-E(통합)에 `#[ignore]` 한 줄씩): `--list`에 **그대로 등장** → 발견 단언 **DISCOVERY OK** → **`cargo test --lib` exit 0** · `131 passed; 0 failed; 1 ignored`. **r22의 acceptance 어디에도 이것을 RED로 만드는 명령이 없다.** ② **실행 가능한 게이트로 만들었다**: `cargo test --tests`(= **lib·bins·통합 전부**)의 출력에서 **모든 `test result:` 줄**을 파싱해 **`0 ignored`가 아니면 nonzero exit**(실측: `IGNORED GATE FAILED — ignored != 0 인 결과 줄 2개` · **exit 1** · 원복하면 **exit 0**). ③ **`--include-ignored`를 택하지 않은 이유**: 그것은 **ignored를 *실행*할 뿐 ignore를 *금지*하지 않는다** ⇒ 릴리스 게이트가 그 플래그 없이 돌면 재갈이 그대로 산다. **소스 `grep '#\[ignore\]'`도 택하지 않았다** — **`#[cfg_attr(…, ignore)]`·매크로 판본을 놓친다.** **실행 결과를 파는 것이 유일하게 표기-독립적이다.** ④ **뮤턴트 표에 M-IGNORE 신설**(킬러 = 게이트 ②) · **§5의 B-IGNORE를 Class B → A로 승격.** |
| **alt** | — | *Simpler alternative (Codex)*: *"셸과 W-REG에 basename 목록을 **중복**시키지 말고, **타깃을 아는 발견 스크립트 하나**를 레지스트리의 **권위**로 삼아라"* | **Accept**(인간 채택) | **채택했다 — 그리고 W-REG를 폐기하는 *더 강한* 이유를 실행으로 찾았다.** ① **`scripts/f14-witness-gate.sh` 하나가 단일 권위다**(§B-1 0-b): 정본 레지스트리 = `<target>\|<id>\|<platform>` 35행 · **타깃별 `--list`** · 앵커 `(^\|::)<id>: test$` · **`0 ignored` 게이트**. ② **★ W-REG 폐기 — 그것은 자기가 막겠다던 공격에 스스로 당한다.** **실측**(복합 공격: W-REG에 **`#[ignore]`** + `mod log_witness;` **한 줄 삭제**): `cargo test --lib` = **`128 passed; 0 failed; 1 ignored` · exit 0** — **W-LOG-A/B/C가 증발했는데 W-REG는 재갈이 물려 침묵하고 스위트는 초록이다.** 같은 트리에서 **하네스 밖의 스크립트**는 `MISSING WITNESS ×3` · **exit 1**. ⇒ **레지스트리 게이트는 그것이 감사하는 하네스 *밖*에 있어야 한다.** ③ **그래서 `tests/` 안의 Rust 테스트(안 (c))도 반려했다** — ⑴ 발견 절반은 **실제로 된다**(통합 테스트에서 `Command::new(env!("CARGO"))`로 `cargo test --lib -- --list`를 부르는 것이 **작동한다**: 웜 트리 **178 ms** · 내부 cargo가 **컴파일까지 하는** stale 트리에서도 **1.59 s · exit 0** ⇒ cargo는 테스트 바이너리 **실행 중에 빌드 락을 쥐지 않는다** — 실측) ⑵ **그러나 `0 ignored` 절반이 원리적으로 불가능하다**(ignored 수를 알려면 스위트를 **실행**해야 하는데 그 스위트 안에 자기가 있다 ⇒ **무한 재귀**) ⑶ **치명적으로, `#[ignore]` 한 줄로 꺼진다**(②와 같은 결함). ④ **인라인 셸(안 (a))도 반려했다** — 파일이 없으면 **아무도 실행하지 않는다**. **P-34가 죽인 실패 양식과 정확히 같은 클래스**(*"선언만 하고 실행에 넣지 않았다"*). ⑤ **⚠⚠ 대가 = `scope[]` 개정**(지휘자): `isTestPath`는 **`scripts/…`를 테스트로 판정하지 않는다**(실측 `false`) ⇒ **정확 경로** `"scripts/f14-witness-gate.sh"` **한 줄**을 `scope[]`에 넣는다(와일드카드 금지 — 실측으로 정확 경로 glob이 그 파일만 매칭함을 확인). **B4 근거**: 셸 스크립트는 **컴파일·링크되지 않는다** ⇒ **관측 행동을 만들 수 없다.** **설계 변경 0 · 프로덕션 코드 변경 0 · `flips[]`/`red.sha`/`characterizationCmd`/`regressionCmd` 불변.** |

**하드룰 4 재도달** — 새 critical **0** · high **2** · **설계 결함 0**(6라운드 연속).

**인간 판정 (하드룰 4)**: **수동 라운드 23 승인**(2026-07-14). 아이러니하지만 정확한 지적이다 — *"증인이 아무것도 증명하지 못한다"*를 죽이려고 만든 발견 단언이 **(a) 올바른 통합 증인을 거짓 RED로 죽이고 (b) 자기가 막겠다던 `#[ignore]` 공격을 그대로 통과시키는** 상태였다. **게이트가 게이트가 아니었다** — 22라운드 동안 잡아 온 바로 그 클래스다. Codex의 simpler alternative를 채택한다: 셸과 W-REG에 목록을 중복시키지 않고 **타깃을 아는 발견 스크립트 하나를 단일 권위**로 삼는다. 앵커는 `(^|::)<id>: test$`로 바로잡고, `0 ignored`는 **산문이 아니라 실행 가능한 단언**으로 만든다.

**라운드 23 실행 예정** — 이 개정판(P-35 봉인: **타깃별 `--list` 형태 실측 3종** · **앵커 `(^|::)<id>: test$`** · **타깃 한정 매칭**(타깃별로 따로 묻는다 — stdout에는 타깃 경계가 없다) / P-36 봉인: **`0 ignored`를 실행 가능한 게이트로**(전 스위트 `test result:` 파싱 · nonzero exit) · **뮤턴트 M-IGNORE 신설** · **B-IGNORE → Class A 승격** / alt 채택: **`scripts/f14-witness-gate.sh` = 증인 레지스트리의 단일 권위** · **W-REG 폐기**(하네스 안의 게이트는 `#[ignore]`로 꺼진다 — 실측) · **안 (a)/(c) 반려 근거 실행으로 등재** · **⚠ `scope[]` 개정 필요**(`scripts/f14-witness-gate.sh` 정확 경로))을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r23: needs-attention (1 high · confidence 1.0 · 설계 결함 0)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-37** | **high** (confidence 1.0) | ***Ignored gate accepts counts ending in zero.*** `grep -vc '0 ignored'`는 **`10 ignored` · `20 ignored` · `100 ignored`를 전부 유효로 본다** — 각각이 부분문자열 `0 ignored`를 **포함**하기 때문이다. `--list`는 ignored 테스트를 여전히 열거하므로, **선언된 증인 10개(차단 증인 포함)를 전부 비활성화해도** 스위트가 성공 종료하고 **이 게이트가 위반 0을 보고한다.** 이는 *"0이 아닌 모든 ignored 수를 잡는다"*는 바로 다음 문장과 **모순**이다. **Recommendation**: **구분된 필드**(`; 0 ignored;`)로 매칭하거나 **숫자를 파싱해 0과 비교**하라. **정확히 10개를 ignore한 뮤턴트로 실증**하라. | **Accept** | **지적이 정확하다 — 그리고 실증했다.** ① **결과 줄 원문·구분자를 먼저 쟀다**(§0-d): `test result: ok. 132 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.83s` — **구분자 = `; `** · 각 필드 = `<수> <이름>` · 줄은 **행 선두**에서 시작한다. ② **함정을 grep의 실제 판정으로 확인**: `grep -c '0 ignored'`가 **`10 ignored` 줄에 1을 낸다**(그리고 `grep -c '0 failed'`도 **`10 failed` 줄에 1**). ③ **★ 정확히 10개 뮤턴트(M-IGNORE-10)로 실증**했다 — 프로토타입 lib에 구현된 증인 **10개 전부**(회귀 ①·② + 대조군 ①·② = **`flips[]`의 차단 증인** · W10 · W-GRAVE-CD-A · W3 · W-LOG-A/B/C)에 `#[ignore]`: **`--list`에 그대로 등장 → DISCOVERY OK** · `122 passed; 0 failed; 10 ignored` · **cargo exit 0** · **옛 파서: 위반 결과 줄 = 0 ⇒ 게이트 PASS(exit 0)**. **차단 증인까지 열 개를 전부 재갈 물렸는데 게이트가 초록이었다.** ④ **파서를 다시 썼다** — 부분문자열 매칭을 버리고 `test result:` 줄을 **토큰으로 쪼개 `ignored`·`failed` 수를 합산하고 정수 0과 비교**한다(+ **결과 줄 수 0 검사** — 컴파일 실패·타깃 증발을 잡는다 · **+ cargo exit도 함께** — ⚠ **exit code만 믿으면 안 된다: cargo는 ignored가 있어도 0으로 끝난다**, 실측). **실측**: 1 ignored → **exit 1** · **10 ignored → exit 1** · 0 ignored → **exit 0**. ⑤ **`failed` 필드 전수 확인** — r22/r23에는 **`failed` 파싱이 아예 없었다**(`suite_rc`에만 의존) ⇒ 부분문자열 버그는 *없었으나* 필드가 **무방비**였다. 새 파서가 **숫자로** 검사하고, **M-FAILED-10**을 뮤턴트 표에 **예방적으로 등재**했다. ⑥ **앵커 매칭도 같은 클래스인지 실제 ID로 검사** — 접두사 충돌(`w_log_a` ⊂ `w_log_ab` ⊂ `w_log_a_no_vanish_stream_is_identical`)을 프로토타입에 **실제로 심어** `--list`를 다시 받았다: **짧은 증인이 삭제되고 긴 것만 남으면 `MISSING WITNESS`** ⇒ **접미사 앵커 `: test$`가 막는다**(거짓 양성 0). 정본 35행에 **접두사 충돌 쌍 0 · ID 중복 0**(전수 스캔). ⇒ **앵커는 안전하다.** ⑦ **★ `--selftest` 신설 — 게이트가 자기를 증명한다**(§0-h): **캡처된 cargo 원문 픽스처 6종**을 본선과 **같은 판정 함수**에 먹인다 — (a) 1 ignored→FAIL · **(b) 10 ignored→FAIL** · (c) 0 ignored→PASS · (d) 10 failed→FAIL · (e) exit≠0→FAIL · (f) 결과 줄 0개→FAIL. **⚠ 옛 파서를 스크립트에 남겨 두고 (b)·(d)에서 *그것이 통과함*을 단언한다** ⇒ **파서를 부분문자열로 되돌리면 selftest가 RED**다. **실행: 6/6 PASS · exit 0 · cargo 미호출(밀리초)**. ⑧ **§5에 Class **B-GATESELF** 신설** — *"게이트 스크립트 자체가 결함을 가질 수 있다"*(P-34→P-35→P-36→P-37: **네 라운드 연속으로 증인을 지키는 장치가 무증인이었다**). **보상 통제 = `--selftest`** · acceptance 0단계 = **`--selftest` → 본 게이트**(두 줄). ⑨ **뮤턴트 M-IGNORE를 M-IGNORE-1 / M-IGNORE-10 두 행으로 분할**하고 **옛 파서가 후자를 통과했다는 실측**을 게이트 자신의 회귀 증인으로 박았다. **설계 변경 0 · 프로덕션 코드 변경 0 · `flips[]`/`red.sha`/`characterizationCmd`/`scope[]` 불변**(⚠ `scope[]`의 `scripts/f14-witness-gate.sh` 개정 요구는 r22에서 이미 제기됐고 **여전히 유효**하다). |

**하드룰 4 재도달** — 새 critical **0** · high **1** · **설계 결함 0**.

**인간 판정 (하드룰 4)**: **수동 라운드 24 승인**(2026-07-14). 고전적인 부분문자열 버그다 — `grep -vc '0 ignored'`가 `10 ignored`를 통과시킨다. 하필 **10**은 선언된 통합 증인의 수이므로, **차단 증인을 포함해 열 개를 전부 재갈 물려도 게이트가 위반 0을 보고**했다. 파서를 **숫자 추출 + 정수 비교**로 바꾸고, **`failed` 필드에도 같은 함정이 없는지 전수 확인**했다. 그리고 게이트가 스스로를 증명하도록 **`--selftest`**를 넣는다(1-ignored · 10-ignored · 0-ignored 3종) — **게이트 자체가 게이트를 통과해야 한다.**

**라운드 24 실행 예정** — 이 개정판(P-37 봉인: **결과 줄 원문·구분자 실측** · **부분문자열 매칭 폐기 → 숫자 추출 + 정수 비교**(`ignored`·`failed`·결과 줄 수·cargo exit **넷 다**) · **M-IGNORE → M-IGNORE-1 / M-IGNORE-10 분할 + M-FAILED-10 신설** · **옛 파서가 10-ignored를 통과했다는 실측을 게이트의 회귀 증인으로 등재** · **앵커 부분문자열 함정 실증 검사 — 안전 확인** · **★ `--selftest` 신설**(픽스처 6종 · cargo 미호출 · 옛 파서 회귀 핀) · **★ §5 Class B-GATESELF 신설**(*"게이트 스크립트 자체가 결함을 가질 수 있다"*))을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.


### Codex Plan Review — r24: needs-attention (2 high · 설계 결함 0)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-38** | **high** (confidence 0.99) | ***`pipefail` turns a successful match into `MISSING WITNESS`.*** 발견 검사가 `pipefail`이 켜진 채 `list_for`를 `grep -qE`에 파이프한다. **조기 매치 후 `grep -q`가 종료하면 상류가 SIGPIPE를 받고** 파이프라인이 **141**을 반환해 **거짓 `MISSING WITNESS` 분기**를 탄다(Codex 재현). 목록이 커질수록 **존재하는 필수 증인이 비결정적으로 acceptance를 막는다.** **Recommendation**: **타깃별 목록을 파일로 캐시**하고 **cargo listing의 종료 상태를 검사**한 뒤 **파이프라인이 아니라 캐시 파일에 직접 `grep -qE`** 하라. **조기 매치 + 큰 목록 selftest**를 추가하라 | **Accept** | **재현했다 — 141 = 128+13(SIGPIPE)**(§0-d (f)): 매치가 **첫 줄**이고 목록이 **1.1 MB** → **20/20** · **57 KB** → **4/30**(비결정) · 매치가 **마지막 줄**이면 **0/20**(grep이 끝까지 읽으니 `cat`도 끝까지 쓴다). **bash 3.2·5.3 동일** ⇒ 셸 판본 문제가 아니다. ⚠ **정직**: 오늘의 lib 목록은 **8.5 KB**라 **아직 발화하지 않는다**(0/30) — **잠복**이다. 그러나 계획은 증인을 **24개 더 이식**하고 파이프 용량은 플랫폼·부하 의존이다 ⇒ **방어적으로 봉인한다.** **봉인**: `list_file()`이 목록을 **`$TMP/list.<target>`에 캐시**하고 **cargo 종료 상태를 먼저 검사**하며(`rc≠0` ⇒ **`LIST FAILED`** — 빌드 실패를 *"증인 없음"*으로 **오진하지 않는다**. r23은 오진했다 — 실측), `has_witness()`가 **캐시 파일을 직접 `grep -qE`** 한다 ⇒ **파이프 0개**. **selftest (h)**(조기 매치 + 1.1 MB 목록 → **PASS 이어야 한다**)가 **M-SIGPIPE**(파이프라인 복원)를 죽인다(실측 RED). |
| **P-39** | **high** (**confidence 1.0**) | ***Two advertised predicates are not actually regression-tested.*** selftest의 **(d)와 (f)가 둘 다 cargo exit 101**을 넘긴다 ⇒ **독립적인 nonzero-exit 검사가 기대 실패를 대신 공급한다** ⇒ **숫자 `failed` 검사와 결과-줄-0개 가드를 지워도 selftest 6/6이 전부 초록이다** ⇒ **M-FAILED-10과 no-results 가드는 핀되어 있지 않다.** **Recommendation**: **10-failed와 zero-result 픽스처를 합성 cargo exit 0으로** 돌리거나 **파싱된 집계를 직접 단언**하라. **(e)는 별도의 nonzero-exit 증인으로 유지**하라 | **Accept** | **재현했다 — 원문**(§0-h): r23 스크립트에서 `failed` 숫자 검사 한 줄 삭제 → **`SELFTEST: PASS (6/6)` · exit 0** · 결과-줄-0개 가드 삭제 → **6/6 · exit 0** · **둘 다 삭제** → **여전히 6/6 · exit 0**. ⇒ **두 술어가 무핀이었다.** ⑴ **픽스처를 `(출력, rc)` 쌍으로 분리**했다 — **(d) 10-failed = rc 0** · **(f) 결과 줄 0개 = rc 0** · **(e)만 rc 101**(nonzero-exit **전용** 증인). ⑵ **발견 술어는 selftest가 아예 안 건드리고 있었다** ⇒ **발견 케이스 3종 신설**((g) 증인 누락 · (h) 조기매치+큰목록 · (i) 목록 rc≠0) — `discover()`에 **목록-해결자를 주입**해 cargo 없이 돈다. ⑶ **술어 6개**(PRED-DISC · PRED-LIST-RC · PRED-N0 · PRED-IGN · PRED-FAIL · PRED-RC) **전부에 대해 "지우면 RED가 되는가"를 실행으로 확인**했다 — **6/6 RED**(+ M-SIGPIPE · M-OLDPARSER 도 RED) ⇒ **살아남은 술어 0.** **selftest = 9/9**(bash 3.2 · 5.3 · zsh 동일 · cargo 미호출). **설계 변경 0 · 프로덕션 코드 변경 0 · `flips[]`/`red.sha`/`characterizationCmd`/`scope[]` 불변.** |

**하드룰 4 재도달** — 새 critical **0** · high **2** · **설계 결함 0**(8라운드 연속).

**인간 판정 (하드룰 4)**: **수동 라운드 25 승인**(2026-07-14). P-39는 이 파이프라인의 시그니처 결함이 **한 겹 더 안쪽**에서 반복된 것이다 — *"증인이 아무것도 증명하지 못한다"* → *"게이트가 게이트가 아니다"* → **"게이트의 selftest가 selftest가 아니다."** 픽스처가 **다른 술어로도 실패**하기 때문에, 검사하려던 술어를 **지워도 초록**이었다. 봉인은 **직교화**다: 픽스처의 cargo 종료코드를 **합성 파라미터**로 분리해 (d)·(f)를 **rc=0**으로 돌리고 (e)만 nonzero-exit 전용 증인으로 남긴다. 그리고 **모든 술어에 대해 "그것을 지우면 selftest가 RED가 되는가"를 실증**한다 — 하나라도 살아남으면 그 술어는 핀되지 않은 것이다. P-38(`pipefail` + `grep -q` → SIGPIPE 141 → **거짓 MISSING WITNESS**)은 목록을 **파일로 캐시**하고 파이프라인을 없애 봉인한다.

### Codex Plan Review — r25: needs-attention (1 medium · critical/high 0)

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **P-40** | **medium** (confidence 0.99) | ***Normative checklist still specifies the obsolete six-case selftest.*** **B-1 증분**(증분 표)과 **B-5 릴리스 체크리스트**가 아직 **"픽스처 6종"** selftest를 요구하는데, 개정된 게이트는 **9케이스**를 요구한다. 그중 **(g) 증인 누락 · (h) 조기 매치 + 큰 목록 · (i) 목록 실패**가 바로 **수용된 P-38/P-39 봉인을 핀하는 케이스**다 ⇒ **체크리스트를 따르는 엔지니어가 그 셋을 빠뜨리고도 "계획 준수"를 주장할 수 있고**, **목록-상태와 SIGPIPE 회귀가 미검증으로 남는다.** **Recommendation**: 모든 규범적 "6케이스" 참조를 **9로 갱신**하고 **(a)~(i)를 명시적으로 요구**하며, **6개 파서 술어 + M-SIGPIPE + M-OLDPARSER가 전부 뮤테이션-킬되어야 함**을 못박아라. *Simpler alternative*: **임베드된 9케이스 스크립트를 유일한 규범 정의로 삼고, 나머지 참조는 전부 상호참조로 바꿔라** | **Accept** | **지적이 정확하다 — 그리고 이것은 r22~r25가 반복해서 잡아낸 *표류 클래스* 그 자체다.** ① **중복을 전수 색출했다**: 게이트 명세(케이스 수·술어 목록·픽스처 목록)가 **B-1 증분 표 · B-5 · §5의 B-GATESELF · 뮤턴트 표(M-PRED-\*) · 스크립트의 `SELFTEST:` 요약** 다섯 곳에 흩어져 있었고 **그중 둘이 이미 6에서 굳어 있었다.** ② **★ 인간이 simpler alternative를 채택했다 — 숫자를 9로 고치지 않고 중복 자체를 없앴다**: **§0-b의 임베드된 스크립트 + §0-h의 술어×케이스 매트릭스를 유일한 규범 정의(SSOT)로 선언**하고, **다른 모든 곳은 숫자를 반복하지 않고 상호참조**한다(*"§0-h가 정의하는 전 케이스"*). **숫자만 고치면 다음 개정에서 또 표류한다 — r22~r25가 정확히 그 표류였다.** ③ **명시적 요구는 유지했다**(Codex의 원 권고): **B-1 acceptance 0단계**와 **B-5**에 **⑴ `--selftest`가 §0-h의 *모든* 케이스에 대해 PASS**(하나라도 빠지면 **계획 위반**) **⑵ §0-h의 술어 전부(DISC·LIST-RC·N0·IGN·FAIL·RC) + M-SIGPIPE + M-OLDPARSER의 뮤테이션-킬을 구현자가 실제로 실증**(프로토타입 **8/8 RED · 살아남은 술어 0**) **⑶ 게이트 = acceptance의 0단계**(`--selftest` → 본 게이트 **두 줄**)를 **케이스 수를 하드코딩하지 않고** 박았다. ④ **★ 스크립트 자신의 숫자도 없앴다** — `SELFTEST: PASS (9/9 · 술어 6개 …)`가 **박아 넣은 리터럴**이라 케이스를 더하면 그것도 표류한다 ⇒ **`res()`/`dis()` 호출을 세도록** 고쳤다(**분모가 저절로 는다**). **실행 확인**: 원본 **9/9 · exit 0**(출력 문자열은 기존 실측 원문과 동일) · **8개 뮤턴트 전부 RED**(M-PRED-DISC/LIST-RC/N0/IGN/FAIL/RC · M-SIGPIPE · M-OLDPARSER → 각각 `SELFTEST: FAIL (8/9)` 또는 `(7/9)` · exit 1) ⇒ **계수 리팩터가 킬 성질을 깨지 않았다.** ⑤ **같은 표류 클래스를 전수 소탕했다**: 레지스트리 **"35행"**(4곳 — §0-b가 정본 · 측정 기록은 *"실측 당시 35행"*으로 보존) · 위 증인 표의 **"characterization 138"**(§B-1 acceptance 2)의 표가 정본 · **합계가 아니라 `0 failed`가 게이트다**) · **§5 B-GATESELF의 "술어 6개 · 6/6 RED"** · **뮤턴트 표 M-PRED-\*의 술어→케이스 대응 재서술**. ⚠ **이미 SSOT가 선언돼 있던 것은 건드리지 않았다**(증인 ID = 스크립트 레지스트리 · 파일/`mod` = §Scope · 잔여 위험 = §5 · 훅 계수 = §7). **설계 변경 0 · 프로덕션 코드 변경 0 · `flips[]`/`red.sha`/`characterizationCmd`/`regressionCmd`/`scope[]` 불변.** |

**하드룰 4 재도달** — 새 critical **0** · high **0** · medium **1** · **설계 결함 0**(9라운드 연속).

**인간 판정 (하드룰 4)**: **수동 라운드 26 승인**(2026-07-14). 숫자를 9로 고치기만 하면 **다음 개정에서 또 표류한다** — r22~r25가 반복해서 잡아낸 것이 정확히 그 표류다. Codex의 simpler alternative를 채택해 **근본을 봉인한다**: **§0-h의 임베드된 스크립트와 술어×케이스 매트릭스를 유일한 규범 정의(SSOT)로 선언**하고, B-1·B-5를 포함한 **다른 모든 곳은 숫자를 반복하지 않고 상호참조**한다. 명시적 요구는 유지한다 — `--selftest`가 **§0-h의 모든 케이스**에 대해 PASS해야 하고, **여섯 파서 술어 + M-SIGPIPE + M-OLDPARSER가 전부 뮤테이션-킬**되어야 하며(프로토타입 실측 8/8 RED), 게이트는 **acceptance의 0단계**다.

**라운드 26 실행 예정** — 이 개정판(P-40 봉인: **§0-b 스크립트 + §0-h 매트릭스 = SSOT 선언** · **B-1 증분 표 · B-1 acceptance · B-5 · §5 B-GATESELF · 뮤턴트 표 M-PRED-\* · 레지스트리 행 수 · characterization 합계를 전부 *상호참조*로 전환**(규범 본문의 하드코딩된 케이스 수·술어 수 **0개**) · **`--selftest` 요약이 케이스를 *세도록* 스크립트 수정**(하드코딩 리터럴 제거 · **9/9 · 8 뮤턴트 전부 RED 재확인**) · **B-1·B-5에 명시적 요구 3종 박음**(전 케이스 PASS · 술어+M-SIGPIPE+M-OLDPARSER 뮤테이션-킬 실증 · 게이트 = 0단계))을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

**라운드 25 실행 예정** — 이 개정판(P-38 봉인: **SIGPIPE 141 재현 원문** · **목록 파일 캐시 + cargo 종료 상태 검사 + 파이프 없는 `grep -qE`** · **뮤턴트 M-SIGPIPE 신설** / P-39 봉인: **픽스처 = `(출력, rc)` 쌍** · **(d)·(f) → rc 0** · **발견 케이스 (g)/(h)/(i) 신설** · **술어 × 케이스 직교 매트릭스** · **뮤턴트 M-PRED-\* 6종 신설 — 전부 킬 실증**)을 `pipeline-stage: design`인 채로 plan 게이트에 **다시 건다**.

### Codex Plan Review — r26: clean — **approve · 0 findings**

> *"**Ship the plan.** The lock's red SHA/tree matches the committed RED record; **both assertions pin the exact
> pass-abort symptom through the production call-site seam.** The round-26 SSOT refactor leaves **no materially
> stale normative gate count.** No open questions."*
> **Simpler alternative(기각 아님 — 기록)**: *"기존 호출부에 소스-인지 `NotFound` 헬퍼 하나 + 루프-후 가드를
> 쓰는 것이 더 작다. 다만 그것은 **미래의 raw-`?` 회귀를 막는 `Entry`/`Absent` 경계를 희생한다.**"*
> ⇒ 그 경계를 지키는 쪽을 택한다(F-14의 근본 원인이 바로 raw `?`였다).

아티팩트: `docs/reviews/reconcile-vanished-entry-aborts-pass/plan-r26.json` (reviewedSha `348190a`).
**plan 게이트 종료 — 26라운드 · 결함 40건(critical 12 · high 22 · medium 2 · 기각 1 = P-21).**
`pipeline-stage: design → executing`.

### Codex Structure Review — s1: needs-attention (1 critical)

> *"the **root-cause seam matches the approved plan** and **no test weakening was found**, but the branch
> **violates the locked single-flip scope**."*

| id | severity | finding | triage | 근거 |
|---|---|---|---|---|
| **S-1** | critical (1.0) | *Out-of-scope domain contract change breaks the single-flip lock* — 브랜치가 `Vanished`·`Absent`·`Entry/Seen`의 **프로젝트 전역 용어집 계약**을 추가하는데 `bugfix-lock.json`의 `scope[]`가 `CONTEXT.md`를 **선언하지 않았다**. 산문의 정확성과 무관하게 **미선언 변경 표면은 Blocker** | **Accept → 락 개정** | conductor-side `/code-review`의 **Standards 축이 `CONTEXT.md` 미갱신을 하드 위반**으로 잡았다 — F-14의 도메인 개념이 `pins`·`reconcile`·`atomic`을 관통하고 `PassGuard::begin`의 시그니처에까지 올라왔다. **빼면 하드 위반이 되살아나고, 선언하지 않으면 미선언 표면이다** ⇒ **선언이 유일한 답**이다. 선례: 직전 파이프라인 `reconcile-gc-dedup-race`의 릴리스 게이트 **R-4**가 같은 이유로 `CONTEXT.md`·`docs/adr/**`를 추가했다 |

**인간 판정**: `scope[]`에 **`CONTEXT.md` 추가**(커밋 `760e517`). 설계·행동 변경 0 — **선언만** 넓힌다.
아티팩트: `docs/reviews/reconcile-vanished-entry-aborts-pass/structure-r1.json` (reviewedSha `6089361`).
**라운드 2 실행 예정.**

### Codex Structure Review — s2: clean — **approve · 0 findings**

> *"**Ship**: S-1 is resolved. `CONTEXT.md` is now explicitly declared in `bugfix-lock.json` scope, and the
> round-2 commits introduce no new critical issue."*

아티팩트: `docs/reviews/reconcile-vanished-entry-aborts-pass/structure-r2.json` (reviewedSha `582d2b0`).
**B3 frontier 열림** → `pipeline-stage: executing → verification`.

### Codex Release Review — r1~r6

릴리스 게이트는 **6라운드**가 걸렸다(수동 라운드 3~6 승인). 라운드별:
- **r1** (4건): R-1(release 프로파일) · R-2(원 repro null) · R-3(게이트 뮤테이션 감사) · R-4(Phase G 공허) 전부 **Accept**.
- **r2** (4건): R-2를 봉인하려 **RED 4차 재포착**(원 안무 = 정확히 40 puts, 1000-put은 stress로 분리) · R-3'/R-4-noop'(지휘자가 head -N으로 자른 증거 절단 실수) · R-4-plan(계획 계약 갱신) 전부 **Accept**.
- **자체 발견(게이트 밖)**: 게이트를 직접 재확인하다 **phase_g 증인이 green.sha에서 5/5 결정적 실패**함을 잡았다 — R-4를 고친 서브에이전트의 *"20/20 GREEN"*이 거짓이었다(`verify-flip`이 못 잡은 이유: `characterizationCmd`에 `reconcile_vanishing_entries` 없음). 근본 원인은 프로덕션이 아니라 구 통합 무대의 **동시성 랑데부 하이젠버그**. **9번째 훅 결정적 park + lib 이전**으로 봉인, 지휘자가 clean 재빌드로 20/20 직접 확인.
- **r3** (R-1만): 잘못된 release 타깃. **r4** (R-1만): 수동 편집이라 감사 불가. **r5** (R-1만): grep 필터로 raw가 아님. **r6**: **approve** — SHA-스탬프 완전 raw capture로 봉인.

> **r6**: *"**Ship**: R-1 is closed with no regression. The SHA-stamped artifact contains complete output and exit 0
> for both canonical release commands, matches verification.md byte-for-byte, and all post-green commits affect
> only review evidence; executable and configuration inputs remain unchanged."*

아티팩트: `docs/reviews/reconcile-vanished-entry-aborts-pass/release-r{1..6}.json`.
**릴리스 게이트 종료 → `pipeline-stage: release-gate → finishing`.**
