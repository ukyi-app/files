---
id: B-2
title: the flip — GC 삭제 분기를 pre_grave → grave() → settle()로 교체 (유일한 관측 행동 플립)
status: open
blocked-by: [B-1]
plan: docs/bugfixes/reconcile-gc-dedup-race.md
created: 2026-07-13
closed:
---

# B-2 — the flip (**유일한 관측 행동 플립**)

> ## ⚠ 이 문서는 **자기완결적**이다
>
> 계획서(`docs/bugfixes/reconcile-gc-dedup-race.md`, 2646줄)를 **열지 않아도** 이 증분을 완수할 수 있다 —
> 필요한 설계·코드·근거·acceptance·**배리어 테스트 안무 전문**이 **전부 아래에 축자 발췌**돼 있다.
> 계획서는 **Codex plan gate r8에서 approve · 0 findings**를 받은 확정 설계이며, 이 문서와 어긋나는 것이
> 있으면 **계획서가 정본**이다.
>
> **⚠ 이 문서의 §4(삭제 분기 자기검증)와 §5(랑데부 규율)를 건너뛰지 마라.**
> **배리어 테스트를 잘못 짜면 증인이 아무것도 증명하지 못한다** — 그것이 **8라운드에 걸친 게이트의 핵심 교훈**이다.
> 게이트가 잡아낸 것은 **버그가 아니라 거짓 증인**이었다: r4(순서) · r5(스케줄링) · r6(취소) · r7(폴링·teardown).
> **`src/` 설계는 여섯 라운드 내내 한 글자도 바뀌지 않았다. 매번 틀린 것은 테스트였다.**

---

## 0. 공통 계약 — 반드시 먼저 읽어라

### 0.1 불변식 — 뒤집히는 관측 행동은 **정확히 하나**다

**이 증분이 그 하나를 뒤집는다.** 최종 진술:

> reconcile 패스 P가 blob X를 GC 삭제 후보로 확정했을 때(무참조 ∧ tombstone 만료), P는 X를 무덤 이름으로 옮기고,
> **그 순간 살아 있던 핀들(= 코호트)이 전부 종료될 때까지 — 단 `settle_timeout`까지만 — 기다린 다음**, 오직
> 하나의 술어를 평가한다:
>
> > **P가 시작된 이후 X에 대한 커밋 rename이 `Ok`를 반환한 적이 있는가**(= 커밋 포인터가 VFS에 실재하는가).
>
> **그렇다면** — 그리고 **그 경우에 한해서만** — X는 삭제 대신 정본 이름으로 복원되고, `gc_deleted`는 증가하지
> 않으며 tombstone은 유지된다(D-2). (술어가 **참으로 확정되는 즉시** 대기를 끊고 복원한다 — 기다림은 **답을
> 모를 때만** 한다.)
>
> 그 외 **모든** 경우 — 실패한 put, 취소된 put, ENOSPC로 죽은 put, 커밋 rename이 `Err`를 반환한 put, 패스
> 이전에 끝난 put, **그리고 X를 관측했으나 결국 아무 포인터도 남기지 못한 put** — 에서 X는 오늘과 **바이트
> 동일하게** 삭제되고 `gc_deleted += 1`이다.
>
> **단 하나의 예외 — degraded 경로**: 코호트 멤버의 **파일시스템 연산이 `settle_timeout` 안에 돌아오지
> 않으면**, P는 술어를 **평가할 수 없다**(결말을 모른다). 이때 P는 **fail-CLOSED**로 간다 — X를 정본 이름으로
> **복원**하고(보존), tombstone을 유지하고, `gc_deleted`를 **증가시키지 않고**, `tracing::error!`를 내고,
> **패스를 정상 종료**한다(같은 패스의 다른 blob은 오늘과 똑같이 회수된다). 이 경로는 **정상 입력에서 도달
> 불가능하다**.

- **characterization 105개는 전부 초록을 유지**한다. **골든 stats·골든 트리 비트 동일.**
- 회귀 테스트 `tests/regression_reconcile_gc_dedup_race.rs`는 **B-2에서 GREEN 20/20**이 돼야 한다
  (B-1에서는 **RED**였다).
- **두 번째 플립이 생기면 그것이 곧 실패다.**

### 0.2 anti-cheat

**테스트를 약화·삭제·스킵하지 마라.** 스위트가 red면 **구현을 고친다.**
단언을 느슨하게(`assert_eq!` → `assert!`, `==` → `>=`) 바꾸거나 `#[ignore]`를 다는 것은 **즉시 실패**다.

### 0.3 scope — 비-테스트 표면

```
src/store/**      src/main.rs      src/layout.rs
```

이 밖의 **프로덕션 파일을 건드리면 배리어 B4 위반**이다.
**`Hooks` 필드는 7개다 — 하나도 늘리지 마라.** 랑데부 신호는 **전부 테스트 쪽 채널**이며 **기존 훅 클로저
안에서만** 산다. Codex가 **네 라운드 연속으로** 못박았다: *"no production hook or fix-model change."*

### 0.4 ⚠ `ReconcileStats`에 필드를 **추가하지 마라**

`tests/layout_tree.rs:71,137,198`이 **구조체 전수 `assert_eq!`**로 stats를 핀한다 → 필드를 하나라도 늘리면
**그 3개가 깨진다 = 두 번째 관측 행동 플립**(하드룰 10 위반). 복구·복원·**연기** 카운트는 **전부 tracing으로만**
낸다. 연기 카운터가 필요하면 **후속 파이프라인**(F-29)이다.

### 0.5 뮤턴트 킬은 **실증하라**

acceptance가 요구하는 **각 뮤턴트**에 대해 **실제로 코드를 임시 변형**해 테스트가 **RED가 되는지 확인**하고
**원복**한 뒤, **그 출력을 보고하라**. **주장은 증거가 아니다.**

### 0.6 B-1이 이미 깔아 둔 것 (전제)

`atomic.rs`의 `Staged`/`stage_blocking`/`commit_blocking`/`rename_durable` · `layout.rs`의 `.gc-grave-<sha>`
평면 이름공간 + `ObjectsEntry::Grave` · **`pins.rs` 전문**(`BlobPins`/`PinGuard`/`PassGuard`/`Graved`/`Settled`/
`Settlement`/`Hooks` — **코드는 이미 있다**) · `objects.rs`의 `pin → blob_intact → commit_pointer` ·
`mod.rs`의 `pins` 필드 + `with_hooks` · `reconcile.rs`의 `recover_graves` + `PassGuard::begin` 배선 +
`Grave => {}` arm + **D-1 `&Store` 전환** + `settle_timeout` 인자 + `settle_timeout_from` · `main.rs` 배선.

**B-1에서 핀·landed는 기록되지만 아무도 읽지 않았다. B-2가 그것을 읽는다.**

**⚠ B-2의 첫 작업**: `git grep -n 'allow(dead_code)' -- src/store/pins.rs` → B-1이 단 attribute를
**전부 제거**한다. **그 attribute가 사라지는 것이 곧 배선의 증거다.** acceptance가 0건을 요구한다.

---

## 1. 이 증분이 바꾸는 것 — GC 삭제 분기 **하나**

```rust
let pass = PassGuard::begin(store, settle_timeout).await?;   // ① 등록 → 무덤 복구 → 참조 스냅샷
let refs = pass.referenced();
stats.referenced = refs.len();
// ... pending 로드 / now_secs / .objects 엔트리 스냅샷 / Reserved continue / is_dir continue: 기존 그대로
match class {
    ObjectsEntry::Temp  => { /* 기존 grace 로직 그대로 */ }
    ObjectsEntry::Grave => { /* 도달 불가(복구가 비웠다). **아무것도 하지 않는다** — 절대 삭제 금지 */ }
    ObjectsEntry::Blob  => {
        let content = tokio::fs::read(&p).await?;
        if hex::encode(Sha256::digest(&content)) != name {                    // 비트로트
            // ⚠ D-4: 격리 분기는 **현행 그대로** — 핀·무덤을 거치지 않고 rename(blob → .corrupt).
            //    F4 유실 레이스는 **미봉인**으로 남는다(F-25).
            /* 기존 코드 무변경: mkdir_p(.corrupt) → rename(blob → .corrupt/<name>)
               → pending.remove(&name) → stats.quarantined += 1 */
            continue;
        }
        if refs.contains(&name) { pending.remove(&name); }
        else { match pending.get(&name) {
            Some(&first) if now_secs.saturating_sub(first) > grace_secs => {
                pass.pins().hooks().pre_grave(&name).await;                   // 결정적 배리어(= 모델링된 사전확인 지점)
                // ↑ `reconcile.rs`가 `BlobPins`에서 얻을 수 있는 것은 **훅뿐**이다(P4). `Inner`의 필드는
                //   `pins.rs` private이라 `live`/`landed`를 **읽을 방법이 아예 없다**.
                // `settle()`은 `Graved`의 메서드이고 `Graved`는 `grave()`의 rename이 성공해야만 태어난다
                // → 이 두 호출을 **뒤바꾸는 뮤턴트는 컴파일되지 않는다**. 그리고 `reconcile.rs`에는
                //   sha로 물어볼 수 있는 보호 술어가 **존재하지 않는다**.
                match pass.grave(&name).await?.settle().await? {
                    Settled::Reaped   => { pending.remove(&name); stats.gc_deleted += 1; }
                    Settled::Restored => { /* D-2: tombstone 유지, 무카운트 */
                                           tracing::info!(sha=%name, "GC restored: landed commit"); }
                    // **degraded 경로**. 무덤은 이미 정본으로 복원됐다(데이터 보존).
                    // tombstone **유지**(D-2) → 다음 패스가 **새 스냅샷으로 재판정**한다.
                    // `gc_deleted` **무증가**. 에러 로그는 `settle()`이 이미 냈다(중복 로깅 금지).
                    // ⚠ **`?`로 패스를 중단하지 않는다** — 멈춘 핀 **하나**가 다른 blob들의 GC를
                    //    막으면 안 된다. 루프는 **계속 돈다**.
                    Settled::Deferred => {}
                }
            }
            Some(_) => {}
            None => { pending.insert(name.clone(), now_secs); }
        }}
    }
    ObjectsEntry::Reserved | ObjectsEntry::Other => {}
}
```

**바뀌는 것은 `Some(&first) if 만료` arm의 본문 하나다.**
개정 전:

```rust
Some(&first) if now_secs.saturating_sub(first) > grace_secs => {
    tokio::fs::remove_file(&p).await?;
    atomic::fsync_dir(&objects).await?;
    pending.remove(&name);
    stats.gc_deleted += 1;
}
```

**무덤이 생기기 시작하므로 `recover_graves`가 실효**한다(B-1에서는 clean 트리에서 no-op이었다).

⚠ **`ObjectsEntry::Grave` arm은 절대 삭제하지 않는다.** `recover_graves`가 `collect_referenced` **이전에**
돌아 무덤을 비우므로 이 arm은 **도달 불가**지만, 도달했다면 그것은 **복구가 실패했다는 뜻**이고 **그때 지우면
데이터가 사라진다.**

⚠ **비트로트 격리 분기는 diff 0줄이다**(D-4 · 하드룰 10 — 고치면 **두 번째 플립**이다 → F-25).

---

## 2. `Settled` — 세 변종과 그 의미

```rust
pub(crate) enum Settled { Restored, Reaped, Deferred }
```

| 변종 | 언제 | 디스크 | `gc_deleted` | tombstone | 로그 |
|---|---|---|---|---|---|
| **`Reaped`** | 코호트 드레인 ∧ `landed` **없음** | 무덤 **unlink** | **+1** | `pending.remove` | — |
| **`Restored`** | `landed` **확정**(즉시) 또는 드레인 후 `landed` **있음** | 무덤 → **정본 복원** | **0** | **유지**(D-2) | `tracing::info!("GC restored: landed commit")` |
| **`Deferred`** | **`settle_timeout` 소진**(결말 불명) | 무덤 → **정본 복원**(fail-CLOSED) | **0** | **유지**(D-2) | `tracing::error!("gc settle timed out — grave restored, reclamation deferred")` (`settle()`이 이미 낸다 — **중복 로깅 금지**) |

> `Restored`/`Deferred`는 **디스크 전이가 동일**하다(무덤 → 정본). 갈라지는 것은 **왜**뿐이다:
> `Restored` = **보호가 확정**됐다(landed) · `Deferred` = **결말을 알아내지 못했다**(타임아웃).
> **변이를 나누는 이유는 정직성이다** — 타임아웃 복원을 `"GC restored: landed commit"`으로 로깅하면 **거짓말**이다.

### P-4 봉인 — `settle()`이 **유한·fail-CLOSED**여야 하는 다섯 가지

`settle()`은 다음 셋 중 **먼저 오는 것**에서 깨어난다(**무한 대기가 표현 불가**하다):

1. **`landed(sha)` 확정 → 대기 0 · 즉시 복원.**
   보호가 확정이므로 **나머지 코호트를 기다리지 않는다.** 더 기다리는 것은 **순손해**다 — **그 창 내내 실재하는
   포인터가 404**다. `settled: Notify`를 **`landed` 삽입에서도** 울려 대기 **도중** 착지도 **즉시** 깨운다.
   ⇒ **증인: T-P4b-1**(이미 landed) · **T-P4b-2**(대기 도중 착지 — `notify_waiters()`가 깨운다).
2. **코호트 드레인 → 결말 확정 → `landed`를 읽어 판정.** (정상 경로. 코호트가 비어 있으면 **첫 검사에서 즉시
   `Drained`** — await 0회 · **오늘과 동일한 실행시간**.)
3. **`settle_timeout` 소진 → fail-CLOSED.** **무덤을 정본으로 복원**(데이터 보존 우선) · tombstone **유지**(D-2)
   · `gc_deleted` **무증가** · `tracing::error!`.
   ⇒ **증인: T-P4a**.
4. **패스는 반드시 해제된다.** `settle()`이 반환하면 `Graved`는 소비되고 `PassGuard`는 `run_once_at`의 끝에서
   drop된다 → **`pass_lock` 해제.** 멈춘 핀은 `settle()`을 **한 번** 지연시킬 뿐, 그 패스의 **나머지 blob 회수를
   막지 못하고**(루프가 계속 돈다) **이후 패스를 막지도 못한다**(락이 풀린다).
   ⇒ **증인: T-P4a 단언 ③·④**.
5. **관측성은 `tracing::error!`로만 낸다** — ⚠ **`ReconcileStats` 필드 추가 금지**.

**왜 타임아웃을 `io::Error`로 합성하지 않는가**(B7 계약): **어떤 syscall도 실패하지 않았다.**
`io::Error::new(ErrorKind::TimedOut, …)`로 **합성**하는 것이야말로 B7이 금지하는 **가공**이다 — 커널이 낸 적
없는 에러를 발명해 io 표면에 얹는 짓이다. ⇒ **이 개정은 `io::Error`를 하나도 새로 만들지 않고 하나도 삼키지
않는다.** `settle()` 안의 **진짜** io 실패(복원 `rename`의 EIO/ENOSPC · `remove_file` · `fsync_dir` ·
`restore_io` 주입)는 **전부 `?`로 무가공 전파**되며 그 행동은 **개정 전과 바이트 동일**하다.

**왜 중단(`?`)이 아니라 로그 + 계속인가**: `settle` 타임아웃을 `Err`로 올리면 GC 루프의 `?`가 **패스 전체를
중단**시킨다 → **멈춘 핀 하나가 나머지 모든 blob의 회수를 막는다.** 이는 P-4가 지적한 병을 **`?`로 갈아입힌
것**에 지나지 않는다. **봉인의 목표는 격리다**: 병든 blob 하나만 연기하고, **나머지는 오늘과 똑같이 회수**한다.

---

## 3. 왜 창이 닫히는가 (완전성 정리 — 유실 불가 ∧ 연기 불가)

X를 무참조·만료 blob, **R**을 `grave(X)`의 rename Ok(= **코호트 스냅샷 시각**), **W**를 코호트가 **전부
drain된** 시각, **S**를 `settle`의 `landed` read라 하자. 구성상 **R ≺ W ≺ S**이다. 커밋 포인터 → X를 남기는
**모든** put P에 대해, P의 커밋 rename이 `Ok`를 반환한 사건을 M(= `landed` 삽입 임계구역)이라 하면:

| # | 순서 | 결과 |
|---|---|---|
| **(1)** | **M ≺ enter_pass** | 포인터는 `collect_referenced`의 readdir 이전에 이미 VFS에 있다 → `refs ∋ X` → GC는 **삭제 분기에 진입조차 안 한다** |
| **(2)** | **enter_pass ≺ M ≺ S** | `pass_live=true`였으므로 `landed ∋ X` → S가 본다 → **Restore** (M이 R 이전이든 이후든 무관 — `landed`는 sticky다) |
| **(3a)** | **S ≺ M** ∧ P의 핀 **∈ 코호트** | **이 칸은 비어 있다.** W ≺ S이므로 S 시점에 P의 핀은 **이미 죽었다**. 핵심 사실 A에 의해 핀의 죽음 ⇒ P의 rename이 **`Ok`를 반환했거나**(⇒ M ≺ S — 가정과 **모순**) **`Err`를 반환했다**(⇒ **커밋 포인터 부재** — 전제와 **모순**). ⇒ **모순** |
| **(3b)** | **S ≺ M** ∧ P의 핀 **∉ 코호트** | P는 **R 이후에 pin**했다 ⇒ **핵심 사실 D**(자급자족)에 의해 P는 무덤 inode에 의존하지 않는다. Reap은 무덤 이름만 지우므로 P의 blob은 **살아남는다** → **유실 0** |
| **(4)** | **M이 영원히 없다** | rename이 `Ok`를 반환한 적이 없다 ⇒ **커밋 포인터가 존재하지 않는다** → **정당한 Reap**, `gc_deleted += 1` |

**유실 시퀀스는 존재하지 않는다.** 그리고 **연기 시퀀스도 존재하지 않는다** — 결말이 (4)인 put(실패·취소·
ENOSPC·rename `Err`)은 `landed`에 **흔적을 남기지 않는다**. `settle`은 코호트가 죽을 때까지 **기다린 다음**
판정하므로 그런 put은 X를 보호하지 **못하고**, X는 **바로 그 패스에서** Reap된다. **연기가 누적될 자리가
구조적으로 없다.**

**`Landed`** 로 깨어난 경우는 (2)와 판정이 같다(**Restore**). **`TimedOut`** 은 W가 **존재하지 않는 경우**이며
정리를 **적용하지 않는다** — 대신 **fail-CLOSED**로 무조건 복원한다. ⇒ **유실 불가는 자명하다**(파괴 연산
`remove_file(grave)`가 **실행되지 않기 때문**).

---

## 4. ⚠⚠ §삭제 분기 자기검증 — **"이 테스트가 정말 `grave()`까지 갔는가"**

> **이 섹션은 계획서에서 통째로 가져온 것이다. 한 줄도 건너뛰지 마라.**

r4가 T-P4b에서 잡은 실패 유형은 **뮤턴트가 아니라 테스트 자신의 결함**이었고, **모든 배리어 테스트에 대해
반복될 수 있다**. 병의 이름을 붙여 둔다:

> **참조됨 분기 누수(referenced-branch leak).** `collect_referenced`는 `PassGuard::begin` 안에서 **블롭 루프보다
> 먼저** 돈다(`reconcile.rs:55` — 오늘도 그렇다). 그러므로 테스트의 put이 **패스 시작 전에** 포인터를 착지시키면
> 그 sha는 **`refs`에 들어가고**, 블롭은 `if refs.contains(&name) { pending.remove(&name); }`로 빠져
> **`pre_grave`도 `grave()`도 `settle()`도 실행되지 않는다.** 테스트는 GC 경로를 **한 줄도 실행하지 않은 채**
> "복원 로그가 없다"는 이유로 RED가 되고 — **봉인을 통째로 제거해도 초록으로 남을 수 있다.**

**규율(모든 배리어 테스트에 예외 없이 적용)**: 각 테스트는 **자신이 삭제 분기에 실제로 들어갔음을 스스로
단언한다.** 두 가지를 **함께** 쓴다(하나는 사전조건, 하나는 사후증거):

1. **`stats.referenced`의 정확한 값을 `assert_eq!`로 못박는다**(`>=`나 `!=` **금지**).
   이 값은 `refs.len()` **그 자체**다(`reconcile.rs:56`) → **테스트의 put이 만든 포인터가 스냅샷에 새어
   들어왔다면 값이 1 커진다** → **시끄럽게 깨진다.** 대상 sha X가 스냅샷에 **없어야** 한다는 것이 이
   테스트들의 **전제**이므로, 그 전제를 **단언으로 승격**한다.
2. **`post_grave` 훅으로 "무덤이 실제로 파였다"를 관측한다.** `grave()`는 **blob→무덤 rename이 성공한 뒤에만**
   `post_grave(sha)`를 부른다 → 이 훅이 X를 봤다는 것은 **`Graved`가 태어났다 = 삭제 분기에 들어갔다**의
   **직접 증거**다. 각 테스트는 `Arc<Mutex<Vec<String>>>`에 sha를 모으고 마지막에 **`graved == vec![X_sha]`**를
   단언한다.
   · **새 훅이 아니다** — `post_grave`는 이미 존재하며(`Hooks` 필드 7개 불변) T-B5 ①이 이미 쓰고 있다.
   · 훅은 **기록 + 신호**를 겸할 수 있다(랑데부가 필요한 테스트는 같은 클로저에서 둘 다 한다).

**왜 `gc_deleted`로는 부족한가**: reap 테스트(T-C1/T-C3)는 `gc_deleted == 1`이 곧 삭제 분기의 증거다.
그러나 **복원 테스트**(T-B1/T-B2/T-B4/T-C2/T-P4a/T-P4b-1/T-P4b-2)의 기대값은 **`gc_deleted == 0`**이고,
**참조됨 분기로 샌 경우에도 `gc_deleted == 0`이다** — **두 세계가 구별되지 않는다.** 정확히 이 구멍으로
T-P4b가 빠졌다. **`referenced`와 `graved`가 그 둘을 가른다.**

**결정성의 열쇠**: `pointers_all`의 `SeedRoot`가 첫 `next()`에서 루트를 readdir해 **버킷 목록을 확정**한다
(`layout.rs:257-274`) → **패스 시작 시 존재하지 않던 버킷**의 포인터는 그 패스의 `collect_referenced`가
**구조적으로 볼 수 없다.** 워커 yield 순서에 기대지 않는다.

**배리어 테스트는 `src/store/{reconcile,pins}.rs`의 in-module `#[cfg(test)] mod tests`에 산다**
(통합 테스트 크레이트에서는 crate-private 훅이 안 보인다).

---

## 5. ⚠⚠ §랑데부 규율 — **개시(initiation)는 완료(completion)가 아니다**

> **이 섹션은 계획서에서 통째로 가져온 것이다. 한 줄도 건너뛰지 마라.**

> ### 규칙 0 (규율의 첫 줄) — **비동기 연산의 *개시*를 그것의 *완료*로 착각하지 마라.**
> **`spawn`했다 ≠ 폴링됐다** · **`abort()`했다 ≠ 취소가 끝났다** · **`drop`했다 ≠ Drop이 관측 가능해졌다** ·
> **`send`했다 ≠ 상대가 받았다** · **`timeout`이 `Err` ≠ 안쪽 퓨처가 끝났다(드롭됐을 뿐이다)**.
> **다음 단계로 넘어가려면, 넘어가도 되는 이유를 *관측*해야 한다.** 논증은 근거가 아니다 — **신호가 근거다.**
> **그리고 테스트는 마지막 단언에서 끝나지 않는다 — teardown도 코드다.**
> 새 배리어 테스트를 쓰는 사람은 **§5.2의 10개 함정 항목을 1:1로 대조**하라.

이 규칙은 **네 라운드에 걸쳐 다섯 번 물렸다** — 매번 다른 옷을 입고 왔다:

| 라운드 | 변종 | 무엇을 완료로 착각했나 |
|---|---|---|
| **r4/P-5** | *(순서 — 이 클래스가 **아니다**)* | 포인터가 `collect_referenced` **이전에** 착지 → 참조됨 분기 누수(§4) |
| **r5/P-6** | **`tokio::spawn` ≠ 폴링됨** | *"put을 spawn했다"*를 *"put이 `pin()`했다"*로 착각 → **빈 코호트** |
| **r6/P-7** | **`JoinHandle::abort()` ≠ 취소 완료** | *"abort를 불렀다"*를 *"caller가 소유한 것이 드롭됐다"*로 착각 → **뮤턴트가 GREEN으로 생존** |
| **r7/P-8** | **async 호출 ≠ 폴링된 퓨처** | *"`grave()`를 불렀다"*를 *"무덤이 파였다"*로 착각 → **`let _ =`가 퓨처를 폴링도 않고 버렸다** → 증인이 **아무것도 증명하지 못한다** |
| **r7/P-9** | **park ≠ 영원한 정지** | *"park 이후 실행되는 코드가 없다"*로 착각 → **sender 드롭(= teardown)이 곧 재개**다 → **teardown의 패닉·에러가 조용히 삼켜진다** |

**spawn ≠ polled**: `tokio::spawn`은 태스크를 **큐에 넣을 뿐 동기적으로 폴링하지 않는다.** 테스트가 spawn 직후
**곧바로** 다음 단계로 넘어가면 GC가 **핀이 생기기도 전에** 무덤을 파고 **빈 코호트**를 캡처해 **즉시 reap**할
수 있다 → 증인이 **셋업 스케줄링 때문에** 실패하거나 — **더 나쁘게는 기대값과 우연히 일치해 조용히 GREEN으로
남는다**(§T-C3: 빈 코호트 reap의 `gc_deleted == 1`이 **정답과 같다**. **가장 위험한 형태다**).

**abort ≠ cancelled**: `JoinHandle::abort()`는 취소를 **스케줄만 한다.**

> `pub fn abort(&self) { self.raw.remote_abort(); }` (`tokio-1.52.3/src/runtime/task/join.rs:227-229`) ·
> `is_finished()`는 *"can return `false` even if `abort` has been called… **the cancellation process may take
> some time**"*(`:231-236`). ⇒ **`abort()`가 반환한 시점에 그 태스크의 퓨처는 아직 드롭되지 않았을 수 있다** —
> 따라서 **그 퓨처가 소유하던 값(예: caller-owned `PinGuard`)도 아직 살아 있을 수 있다.**
> tokio 자신의 doctest가 처방을 보여 준다(`:214-220`):
> `handle.abort(); … assert!(handle.await.unwrap_err().is_cancelled());` — **abort 뒤에 await한다.**

### 5.1 규율 — 모든 배리어 테스트에 **예외 없이** 적용

1. **park하는 훅 클로저는 park하기 *전에* 자신의 도착을 알린다.** 클로저 안의 순서는 반드시
   **`send(arrival)` → `park`**이다(뒤집으면 신호가 **영영 오지 않는다**).
2. **테스트는 다음 단계로 넘어가기 전에 그 도착을 `await`한다.** ⇒ **spawn만 하고 넘어가는 지점이 0개**여야 한다.
   *(JoinHandle을 **완주까지** await하는 것은 spawn 지점이 **아니다** — 완주가 곧 도착이다.)*
3. **⚠ `abort()` 뒤에는 반드시 그 `JoinHandle`을 유한 타임아웃으로 await하고 `JoinError::is_cancelled()`를
   단언한다.** 그 await가 **반환한 뒤에야** 다음 단계로 간다.
   **취소 완료 = 그 퓨처가 드롭됐다 = 그 퓨처가 소유하던 가드·락이 드롭됐다.** 이것을 관측하지 않으면:
   · **뮤턴트가 경합으로 살아남는다**(T-C2 — caller-owned 가드가 **아직 안 죽어서** GC가 코호트로 잡는다) ·
   · **테스트가 hang한다**(T-B5① — 아직 안 죽은 `PassGuard`가 **`pass_lock`을 쥐고 있어** 새 패스가 못 들어간다).
   ⚠ **취소 완료는 `spawn_blocking` 클로저의 종료를 뜻하지 *않는다*** — 시작된 blocking 태스크는 **abort 불가**이고
   `JoinHandle` 드롭은 **detach일 뿐**이다(`blocking.rs:107-120`). **그 비대칭이 바로 T-C2가 증명하려는 것이다.**
4. **모든 park은 해제 경로를 갖거나, "끝까지 잡아 둔다"가 *의도*임을 명시한다**(§5.3 park 함정 — 테스트가 `tx`를
   쥔 채 끝나고 unwind가 풀어 준다. 그 park의 **도착 신호는 여전히 필수**다: 도착을 확인해야 *"핀이 살아 있다"*가
   **단언**이 된다).
5. **"해제를 *언제* 하는가"도 관측 대상이다.** 해제 시점이 *"settle이 **대기에 들어간 뒤**"*여야 하는
   테스트(**T-B4 · T-C2 · T-C3 · T-P4b-2**)는 `post_grave`의 도착(`graved_reached`)을 await한 뒤
   **pending 프로브**(`timeout(200ms, &mut gc)` = **`Err`**)로 그 상태를 **관측하고서** 해제한다.
   ⚠ **너무 일찍 해제하면 코호트-대기 뮤턴트가 *경합으로* 살아남는다** — 해제된 put이 mutant-settle의 `landed`
   첫 검사보다 **먼저** 착지하면 그 뮤턴트도 **Restore**를 내고 **GREEN**이 된다.
   ⚠ **프로브는 반드시 `&mut gc`로 건다**(값으로 넘기면 `timeout`이 `Err`일 때 **`JoinHandle`이 드롭돼** GC가
   detach된다 — 함정 6). `&mut JoinHandle`은 `Future + Unpin`이므로 **빌림만 드롭되고 태스크는 그대로 산다.**
6. **⚠ 해제 `send()`의 반환은 "훅이 재개했다"가 아니다.** 해제 직후에는 **어떤 상태도 단언하지 않는다** —
   반드시 **다음 관측 가능한 사건**(도착 신호 · put 완주 · GC 완주)까지 await한 뒤에 단언한다.
   T-P4b-2가 정확히 그렇게 한다(`park_A` 해제 → **`post_landed_reached` await** → 단언).
7. **⚠ 버려진 `JoinHandle`은 패닉을 조용히 삼킨다.** 완주를 await하는 모든 핸들은 **`JoinError`를 언랩**하고
   (패닉 = 즉시 RED) **안쪽 `Result`까지 단언**한다(`let _ = h.await;` **금지**).
   ⚠ **"의도적으로 await하지 않는 핸들"이라는 면제는 *폐기됐다*.** **모든** `JoinHandle`은 **await된다.**
   영구 park된 태스크는 **teardown에서** await한다(규율 8).
8. **⚠ 영구 park의 *해제*도 안무의 일부다 — teardown에서 실제 코드가 돈다.**
   테스트가 `tx`를 쥔 채 끝나는 park은 *"영원히 멈춘다"*가 **아니다**: `tx`가 드롭되는 **그 순간** `recv()`가
   `Err(RecvError)`로 풀리고 **훅이 반환하며 커밋 클로저가 완주한다**(rename → `landed` 삽입 →
   `notify_waiters()` → fsync → `PinGuard::drop` → `spawn_blocking`의 **`.await.expect("join")`**).
   ⇒ **패닉·에러가 그 자리에서 태어난다.** 핸들을 버리면 **테스트는 초록이고 아무도 모른다.**
   **규율(T-P4a · T-P4b-1 · T-P4b-2에 예외 없이 적용)**:
   ① **핸들을 보유한다**(`let put = tokio::spawn(…)` — `let _ = …` **금지**) ·
   ② **영구 stall 증인 단언을 *전부* 마친다**(먼저 해제하면 핀이 drop되고 포인터가 착지해 **시나리오 자체가
      사라진다**) · ③ **park sender를 *명시적으로* 드롭한다**(`drop(tx);` — **스코프 종료에 기대지 않는다**) ·
   ④ **유한 타임아웃으로 핸들을 await하고 `JoinError`와 안쪽 `put()` 결과를 *둘 다* 언랩한다**
      (`timeout(5s, put).await.expect("put must finish after park release").expect("put task must not panic")`
      → `Ok` 단언).
   *(패닉으로 인한 RED에서는 unwind가 `tx`와 핸들을 함께 드롭하므로 teardown await에 **도달하지 않는다** —
   RED는 여전히 **hang이 아니라 깔끔한 실패**다.)*
9. **⚠ async 호출은 그 자체로 아무 일도 하지 않는다 — 폴링되어야 일어난다.**
   `let _ = pass.grave(&sha);`는 **rename을 수행하지 않는다** — **폴링되지 않은 퓨처를 드롭할 뿐이다.**
   `#[must_use]`도 `let _ =`가 **삼켜 버린다.** ⇒ **모든 async 표현식은 `.await`되고, 그 결과는 단언된다.**
   **"부작용을 노리고 호출한 async 함수"는 반드시 await한다.**
   *(clippy에 `let_underscore_future` 린트가 있지만 — **계획 문서의 스니펫은 clippy가 읽지 않는다.**)*

### 5.2 함정 10개 (이 저장소에서의 구체적 표현)

| # | 함정 | 무엇을 완료로 착각하나 | 이 저장소에서 |
|---|---|---|---|
| **1** | `tokio::spawn(...)` 후 **도착 신호 없이 진행** | 큐 삽입 = 실행 | 핀이 생기기 전에 GC가 **빈 코호트**를 캡처 → **즉시 reap** |
| **2** | `JoinHandle::abort()` 후 **취소 완료 await 없이 진행** | 스케줄 = 드롭 완료 | caller-owned `PinGuard` 뮤턴트가 **아직 안 죽어** 코호트에 잡힌다 → **뮤턴트 생존** |
| **3** | `JoinHandle` **드롭(detach) 후 그 태스크의 상태를 가정** | 드롭 = 취소 | **시작된 `spawn_blocking`은 abort 불가이며 detach될 뿐 계속 실행된다**(`blocking.rs:107-120`) — 이 설계가 **의존하는** 성질이다 |
| **4** | `drop(guard)`/`drop(store)` 후 **효과가 관측 가능해지기 전에** 진행 | 드롭 호출 = 효과 발생 | `PinGuard::drop`(live 제거 + notify) · `PassGuard::drop`(**`pass_lock` 해제**) — 후자를 안 기다리면 **다음 패스가 hang한다** |
| **5** | `JoinHandle`을 **await하지 않고 버려** 패닉·에러가 **조용히 삼켜진다** | 태스크 종료 = 성공 | 배리어 테스트가 **패닉을 못 보고** "흔적 없음"을 **엉뚱한 이유로** 관측한다 |
| **6** | `tokio::time::timeout(...)`이 `Err`일 때 **안쪽 퓨처가 어떻게 되는지 가정** | 타임아웃 = 안쪽이 끝났다 | **드롭될 뿐이다.** `run_once_at`을 드롭하면 `PassGuard`가 → **`pass_lock`이 풀린다**. `timeout(_, gc_handle)`을 **값으로** 넘기면 `Err`일 때 **핸들이 드롭돼 GC가 detach**된다 |
| **7** | **채널 send/recv의 완료를 가정** | `send` 반환 = 상대가 받음 | 해제 `send()` 반환은 **훅이 재개했다는 뜻이 아니다.** `Notify::notify_waiters()`는 **permit을 저장하지 않는다**(대기자 0이면 유실). `oneshot::Receiver::await`는 `self`를 **소비**해 `Fn` 훅에 **못 들어간다** |
| **8** | **파일시스템 연산의 가시성 가정** | fsync = 관측 가능 | rename `Ok` ⇒ **즉시 가시**(핵심 사실 C) · `SeedRoot` 스냅샷은 **가시성이 아니라 시점**의 문제 |
| **9** | **park된 태스크의 *teardown*에서 실패가 나는데 핸들을 await하지 않는다** | park = 영원한 정지 (*"이후 실행되는 코드가 없다"*) | **`tx` 드롭 = 재개다.** `recv()`가 `Err(RecvError)`로 풀리고 커밋 클로저가 **rename·`landed` 삽입·notify·fsync·`PinGuard::drop`을 완주**한다. `commit_pointer`는 **`spawn_blocking(…).await.expect("join")`** 으로 끝나므로 **그 안의 패닉은 put 태스크의 패닉**이 된다 → 버려진 핸들이 **삼킨다** → **테스트는 초록** |
| **10** | **async 표현식의 결과를 *await 없이* 버린다** | 호출 = 실행 | **`let _ = pass.grave(&sha);` 는 rename을 하지 않는다** — **폴링되지 않은 퓨처를 드롭**할 뿐이다. `#[must_use]`도 `let _ =`가 **삼킨다**. ⇒ 무덤이 **아예 없고**, blob은 **멀쩡하며**, `recover_graves`가 **깨져 있어도 GREEN**이다 |

> #### 보조정리 L — **"put 완주 await" = "핀 사망 + 알림 발사"의 관측** (함정 3·4의 뿌리)
>
> **`put()`이 반환하면(`Ok`든 `Err`든) 그 put의 `PinGuard`는 이미 drop됐고 `notify_waiters()`도 이미 울렸다.**
> - **(a) 커밋에 도달한 경우** — 가드는 `commit_pointer`의 **blocking 클로저 안 지역변수**(`let me = self;`)이고,
>   지역변수는 **클로저가 반환하기 전에** drop된다(`?`로 조기 반환해도 마찬가지). 호출자의 `.await`는 그 blocking
>   태스크가 **완료된 뒤에만** 깨어난다 ⇒ **`drop(PinGuard)` ≺ `commit_pointer` 반환 ≺ `put` 반환.**
> - **(b) 커밋 이전에 실패·반환한 경우** — 가드는 **호출자 퓨처의 지역변수**이고 퓨처 완료와 함께 drop된다.
>
> ⇒ **T-B1·T-B2**(*"무덤 시점 코호트는 비어 있다"*) · **T-C1**(*"그 핀은 이미 죽어 있다"*) ·
> **T-B4·T-C3**(*"해제하면 코호트가 드레인된다"*)의 **전제가 전부 이 보조정리다.** 논증이 아니라 **기계 사실**이다.
>
> ⚠ **역은 성립하지 않는다 — 그리고 그 비대칭이 T-C2의 명제 그 자체다.** **취소**(= 호출자 퓨처 드롭)는 (a)의
> 가드를 **죽이지 못한다**(클로저가 소유한다). ⇒ **완주 ⇒ 핀 사망** · **취소 ⇏ 핀 사망**.
> **caller-owned 뮤턴트에서는 취소가 곧 핀 사망이다** — T-C2는 정확히 그 차이를 관측한다.

### 5.3 ⚠ 영구 park 훅은 런타임 셧다운을 걸지 않는다 (**park 함정**)

⚠ **tokio는 시작된 `spawn_blocking`을 abort하지 못하고, 런타임 셧다운은 그것들이 끝날 때까지 무한정 기다린다**
(`blocking.rs:107-120` — 이 설계가 **의존하는 바로 그 성질**이다). 따라서 커밋 클로저 안의 동기 훅에서
**영원히** park하면 **테스트 런타임이 drop에서 영영 멈춘다** — 픽스가 옳아도 테스트 바이너리가 hang한다.

**해법**: park은 **`std::sync::mpsc`의 `recv()`**로 한다. 훅 클로저가 `Mutex<mpsc::Receiver<()>>`를 쥐고,
**테스트 함수가 `Sender`를 쥔다**.

```rust
let (tx, rx) = std::sync::mpsc::channel::<()>();
let rx = std::sync::Mutex::new(rx);
let park: SyncHook = Arc::new(move |_sha: &str| { let _ = rx.lock().unwrap().recv(); });
// … 테스트 본문: tx는 **단언이 끝날 때까지 살아 있다** → recv()는 블록 → **핀이 절대 풀리지 않는다**
// (`recv()`는 **동기** 호출이다 — 퓨처가 아니므로 함정 10과 무관하고, 버리는 `Err(RecvError)`가 **곧 해제 신호**다)
//
// **⚠ teardown**: tx drop → recv() = Err(RecvError) → 훅 반환 → **클로저 완주**
//   (rename → landed 삽입 → notify → fsync → PinGuard::drop → `spawn_blocking`의 `.await.expect("join")`)
//   ⇒ **teardown에서 실제 코드가 돈다.** 그러므로:
drop(tx);                                             // ① **명시적** 해제 — 스코프 종료에 기대지 않는다
let r = tokio::time::timeout(Duration::from_secs(5), put).await
    .expect("put must finish after park release")     // ② 유한 대기
    .expect("put task must not panic");               // ③ **JoinError 언랩** — 패닉 = 즉시 RED
assert!(r.is_ok());                                   // ④ **안쪽 put() 결과까지 단언**
// 패닉 unwind 경로에서는 tx와 put 핸들이 **함께** drop된다 → 훅이 풀려 런타임은 **정상 종료**(RED는 hang이 아니다)
```

- **패스가 도는 동안 핀은 절대 풀리지 않는다** — 뮤턴트가 요구하는 조건이 그대로 성립한다.
- **패닉에도 안전하다** — unwind가 로컬 `tx`를 drop하므로 훅이 풀린다. 단언 실패가 **hang이 아니라 RED**가 된다.
- **⚠ "park 이후 실행되는 코드가 없다"는 *거짓*이다** — 위 teardown 블록이 그 증거다. **해제는 재개다.**
- **`Fn` 제약을 만족한다** — `mpsc::Receiver::recv(&self)`는 `&self`를 받는다
  (`oneshot::blocking_recv(self)`는 `FnOnce`라 `SyncHook = Arc<dyn Fn(&str)>`에 **들어가지 않는다**).

### 5.4 신호 채널 — **전부 테스트 쪽이다. 프로덕션 훅은 하나도 늘지 않는다**

(`Hooks` 필드 **7개 불변**; 신호는 테스트가 그 7개 필드에 꽂는 **클로저 안**에서만 산다)

| 용도 | 채널 | 왜 이것이어야 하는가 |
|---|---|---|
| **도착** (sync·async 훅 **공통**) | `tokio::sync::mpsc::unbounded_channel::<String>()` — 훅이 `tx.send(sha)`, 테스트가 `rx.recv().await` | `send(&self)`가 **논블로킹 · 런타임 컨텍스트 불필요** → **blocking 클로저 안에서도 안전**하고 `Fn`의 `&self` 제약도 만족한다(`oneshot::Sender::send(self)`는 `self`를 소비해 `Fn`에 **못 들어간다**). ⚠ **테스트 쪽은 반드시 `await`로 기다린다** — `std::sync::mpsc::recv()`로 기다리면 **current-thread 런타임이 그 자리에서 멈춰** spawn된 put이 **영영 폴링되지 않는다**(P-6을 **직접 재현**하는 자충수) |
| **해제 — sync 훅** (`in_commit_pre_rename` · `in_commit_post_landed`) | `std::sync::mpsc` + `recv()` (§5.3 그대로) | 여기는 **blocking 클로저 안**이므로 **블로킹이 옳다**(async park은 표현조차 안 된다). `recv(&self)`라 `Fn` 제약도 만족 |
| **해제 — async 훅** (`pre_grave` · `post_grave` · `post_observe` · `during_collect`) | `Arc<tokio::sync::Notify>` — 훅이 `notified().await`, 테스트가 **`notify_one()`** | `notified()`는 `&self`(`Fn` OK) · **`notify_one()`은 대기자가 없어도 permit을 저장한다** → **lost wakeup 불가**. ⚠ **`notify_waiters()`를 쓰지 마라** — 그건 permit을 **저장하지 않는다**(프로덕션 `settled`가 그것을 쓸 수 있는 이유는 `await_settlement`가 **검사 이전에** `enable()`로 등록하기 때문이다. 테스트 훅에는 그 보증이 없다) · ⚠ **`oneshot`도 쓰지 마라** — `Receiver::await`가 `self`를 **소비**하므로 `Fn` 훅에 **들어가지 않는다** |
| **⚠ 취소 완료** | `tokio::time::timeout(2s, &mut handle).await` → **`Ok(Err(e))`** ∧ **`e.is_cancelled()`** | **`abort()`는 스케줄만 한다**(`join.rs:227-229`). 이 await가 반환해야 **퓨처가 드롭됐다 = caller가 소유하던 가드·락이 드롭됐다**가 **확정**된다. tokio doctest와 **동형**(`join.rs:214-220`). ⚠ **`is_cancelled()` 단언이 패닉 탐지기도 겸한다** — 태스크가 abort 이전에 **패닉**했다면 `is_panic()`이라 이 단언이 **RED**가 된다 |

### 5.5 체크리스트 — **전 배리어 테스트 × 랑데부**

> **"모든 park에 도착 신호가 있다 · 모든 abort에 취소 완료 await가 있다 · 모든 핸들이 (teardown을 포함해)
> await된다 · 모든 async 퓨처가 폴링된다"**
> **다음 개정이 이 표의 행을 지우지 않고서는 안무를 약화시킬 수 없다.**
> (park·abort가 **없는** 테스트는 그 사실 자체를 적어 둔다 — "확인 안 함"과 "확인했고 없음"을 구별한다.)

| 테스트 | park 지점 (훅) | **도착 신호** | 해제 | **취소 완료 await** | **teardown await** | **async 퓨처 폴링** | **테스트가 다음 단계 이전에 await하는 것** (spawn-후-진행 = **0**) |
|---|---|---|---|---|---|---|---|
| **T-B1** | `during_collect` (GC) | `collect_reached` | `Notify` | — (abort 없음) | **—** 잔여 태스크 0 | ✔ 전부 `.await` | GC **spawn** → **`collect_reached` await**. put은 **spawn하지 않는다 — 완주를 await**한다(→ **`Ok` 단언**) → 그 다음 해제 |
| **T-B2** | `pre_grave` (GC) | `gc_at_pre_grave` | `Notify` | — (abort 없음) | **—** 잔여 태스크 0 | ✔ 전부 `.await` | GC **spawn** → **`gc_at_pre_grave` await** → *그제서야* putter 시작 → **putter 완주 await**(→ **`Ok` 단언** · 핀 drop) → 해제 |
| **T-B4** | `post_observe` (put) · `post_grave` (GC, **기록+신호**) | `observed` · `graved_reached` | `Notify` · 없음(통과) | — (abort 없음) | **—** 잔여 태스크 0 | ✔ 전부 `.await` | put **spawn** → **`observed` await** → GC **spawn** → **`graved_reached` await** → **pending 프로브(`&mut gc`)** → put 해제 → **put 완주 await**(→ **`Ok` 단언**) → `timeout(5s, gc)` |
| **T-C1** (B-1) | **없음** (동시 put 0) | — | — | — | **—** park·spawn 0 | ✔ 전부 `.await` | **spawn 지점 0.** put은 reconcile **이전에** 완주(`Err`)한다 → 함정이 **구조적으로 없다** |
| **T-C2** | `in_commit_pre_rename` (put) | `pre_rename_reached` | `std::sync::mpsc` | **✅ 필수 — `abort()` → `timeout(2s, &mut put)` = `Ok(Err(e))` ∧ `e.is_cancelled()`** | ⚠ **await할 핸들이 구조적으로 없다** — abort가 커밋 클로저를 **detach**시킨다(**그것이 이 테스트의 명제다**). **대리 관측**: GC의 **Restore + 포인터 실재 + blob 존재**가 *"클로저가 rename·`landed` 삽입까지 완주했다"*를 증명한다. **잔여(정직)**: 착지 **이후**(fsync·핀 drop)의 패닉만 **미관측** | ✔ 전부 `.await` | put **spawn** → **`pre_rename_reached` await** → abort → **⚠ 취소 완료 await** → *그제서야* GC **spawn** → **`graved_reached` await** → **pending 프로브(`&mut gc`)** → 해제 → `timeout(5s, gc)` |
| **T-C3** | `in_commit_pre_rename` (put) | `pre_rename_reached` | `std::sync::mpsc` | — (abort 없음) | **—** 잔여 태스크 0(해제 후 **put 완주 await**로 본문에서 닫는다) | ✔ 전부 `.await` | put **spawn** → **`pre_rename_reached` await**(⚠ **없으면 조용히 GREEN**) → GC **spawn** → **`graved_reached` await** → **pending 프로브(`&mut gc`)** → 해제 → **put 완주 await**(→ **`Err(Internal)` 단언** · 핀 drop) → `timeout(5s, gc)` |
| **T-P4a** | `in_commit_pre_rename` (put) | `pre_rename_reached` | **teardown에서만**(영구 park) | — (abort 없음) | **🔧 필수.** 단언 ①~⑤ **전부** 끝난 뒤 → **`drop(tx)`**(명시) → **`timeout(5s, put)`** → **`JoinError` 언랩** + 안쪽 **`Ok` 단언** | ✔ 전부 `.await` | put **spawn**(핸들 **보유**) → **`pre_rename_reached` await** → *그제서야* `timeout(5s, run_once_at(…))` ×3 → 단언 → **teardown** |
| **T-P4b-1** | `pre_grave` (GC) · `in_commit_post_landed` (put) | `gc_arrived` · `landed_reached` | **`Notify`** · **teardown에서만**(영구 park) | — (abort 없음) | **🔧 필수.** 단언 ①~⑤ 뒤 → **`drop(tx_put)`** → **`timeout(5s, put)`** → **`JoinError` 언랩** + **`Ok` 단언** | ✔ 전부 `.await` | GC **spawn** → **`gc_arrived` await** → put **spawn**(핸들 **보유**) → **`landed_reached` await** → `pre_grave` 해제 → `timeout(2s, gc)` → 단언 → **teardown** |
| **T-P4b-2** | `pre_grave` (GC) · `in_commit_pre_rename`(`park_A`) · `in_commit_post_landed`(`park_B`) | `gc_arrived` · `pre_rename_reached` · `post_landed_reached` | `Notify` · `std::sync::mpsc`(6단계) · **teardown에서만**(`park_B`) | — (abort 없음) | **🔧 필수.** 단언 ①~⑤ 뒤 → **`drop(tx_B)`** → **`timeout(5s, put)`** → **`JoinError` 언랩** + **`Ok` 단언**(`tx_A`는 6단계에서 이미 해제됐다) | ✔ 전부 `.await` | GC **spawn** → **`gc_arrived` await** → put **spawn**(핸들 **보유**) → **`pre_rename_reached` await** → `pre_grave` 해제 → **`graved_reached` await** → pending 프로브(**`&mut gc`**) → `park_A` 해제 → **`post_landed_reached` await** → `timeout(2s, gc)` → 단언 → **teardown** |
| **T-B5 ①**(취소) | `post_grave` (GC) | `graved_reached` | **없음** — abort가 곧 해제 | **✅ 필수 — `abort()` → `timeout(2s, &mut gc)` = `Ok(Err(e))` ∧ `e.is_cancelled()`** | **—** teardown에 재개할 park이 **없다**(park은 abort로 **드롭**됐다 · in-flight `spawn_blocking` **0**) | ✔ 전부 `.await` | GC **spawn** → **`graved_reached` await** → abort(⚠ 도착 전에 abort하면 **무덤이 안 파여** 단언이 **엉뚱한 이유로** RED) → **⚠ 취소 완료 await**(**없으면 `PassGuard`가 `pass_lock`을 쥔 채라 새 `run_once`가 hang한다**) → *그제서야* 디스크 단언 + 새 `run_once` |
| **T-B5 ②③** | **없음** (동시 put 0) | — | — | — | **—** park·spawn 0 | ✔ 전부 `.await` | **spawn 지점 0** — 전부 순차 await |
| **T-B5 ④**(`Graved` 누수) | **없음** | — | — | — | **—** park·spawn 0 | **🔧 `pass.grave(&sha)`를 `.await`한다**(`let _ = pass.grave(..)`는 **폴링되지 않은 퓨처를 드롭**해 **rename이 아예 일어나지 않았다**) | **spawn 지점 0.** `grave().await` → **복구 이전 디스크 단언** → **`drop(graved)`**(settle 없음 = 누수) → **`drop(pass)`**(명시 — 안 하면 다음 패스가 `pass_lock`에서 **hang**한다) → 복구 패스 |
| **T-Q2 · T-Q3** | **없음** (동시 put 0) | — | — | — | **—** park·spawn 0 | ✔ 전부 `.await` | **spawn 지점 0** |
| **회귀**(`tests/regression_…`) | ⚠ **의도적 확률 창** | — | — | — | **—** park 0 | ✔ **이미 올바름** | 확률적 창은 **의도**다(`sleep(PUT_DELAY)`) — **이 규율의 적용 대상이 아니다.** 적용하면 증상 재현 자체가 불가능해진다. **함정 5는 이미 올바르다**: `rec.await.unwrap().unwrap()` · `h.await.unwrap()` — ⚠ **B-1의 기계 치환이 이 언랩들을 지워서는 안 된다** |
| **adversarial**(characterization) | ⚠ 배리어 아님 | — | — | — | **—** park 0 | ✔ **폴링은 된다** — `let _ = run_once(…).await`는 **await가 있다**(결과만 버린다) ⇒ **함정 5**이지 **함정 10이 아니다** | **기존 계약 — 손대지 않는다** |

> **⚠ 왜 T-C3가 가장 위험했는가**(정직하게 적는다): 위 표가 없었다면 T-C3는 **조용히 무해해질 수 있었다.**
> put이 폴링되기 전에 GC가 무덤을 파면 **코호트가 비고** → `settle()`이 **첫 검사에서 `Drained`** → `landed` 없음
> → **Reap** → **`gc_deleted == 1`** — 이것은 **T-C3가 기대하는 바로 그 값이다.** 즉 **테스트는 GREEN인데
> "겹치는 실패 put"이라는 시나리오는 한 번도 재현되지 않는다.** r2/P-2가 명시 요구한 증인이 **아무것도 지키지
> 않는 채 초록으로 남는다.**

---

## 6. B-2 acceptance (**유일한 플립**)

- [ ] `tests/regression_reconcile_gc_dedup_race.rs` **GREEN 20/20**
- [ ] `cargo test` **105 green** (골든 stats·골든 트리 **비트 동일**), `tests/adversarial.rs` 40객체 불변
- [ ] `git grep -n 'allow(dead_code)' -- src/store/pins.rs` → **0건** (B-1이 단 attribute 전부 제거)

### T-C1 — **B-1에서 이미 착지했다. 여기서 깨지면 안 된다** (두 번째 플립 회귀 가드)

**새로 쓰지 마라 — B-1의 것이 그대로 초록이어야 한다.** B-2가 `settle()`을 배선하면서 이 증인이 깨지면
**두 번째 플립이 생겼다는 뜻**이다.

`b/k.meta.json` 위치에 **디렉터리**를 심어 `rename`을 결정적으로 EISDIR 실패시킨다 → `put()` = `Err(Internal)`
∧ `landed` **무흔적** ∧ (만료·미참조 blob에 대해) `run_once_at` → **`gc_deleted == 1`**.

- **뮤턴트 킬**: `on_landed`를 rename **앞**으로 이동(= "커밋을 **시도**했다"는 흔적) → 흔적 발생 → Restore →
  `gc_deleted == 0` → **결정적 RED**. (ENOSPC 무한연기의 기계 증인)
- **랑데부**: **park 0 · spawn 0.** put은 reconcile **시작 전에** 완주(`Err`)하고 그 핀은 **이미 죽어 있다** →
  **spawn ≠ polled 함정이 구조적으로 없다.** *"확인 안 함"이 아니라 "확인했고 없음"이다.*
- ⚠ **T-C1의 한계(정직하게)**: 이 테스트는 **실패한 put이 이미 반환되고 그 핀이 죽은 뒤에** reconcile을 돌린다
  → **겹치는(overlapping) 실패 put**을 전혀 재현하지 못한다. 그 창의 증인은 **T-C3**이며, T-C1은 `landed`
  흔적의 **위치**만 지킨다. **두 테스트는 다른 것을 지킨다 — T-C1을 "겹치는 실패 put"의 증인으로 제시하지 마라.**

### T-B1 — put이 참조 수집 도중 완료

**셋업**: ① 만료·미참조 blob **X**(정상 put → 포인터 삭제 → tombstone 만료) · ② **디코이 객체 D**(다른 내용 ·
포인터 **살아 있음**). **D는 장식이 아니라 필수다** — `during_collect`는 **포인터를 1개 낼 때마다** 발화하므로,
포인터가 **하나도 없으면 훅이 영영 발화하지 않아** 랑데부가 걸린다.

**랑데부(도착/해제 쌍)**: `during_collect` = **도착 `collect_reached` 송신 → `Notify` park**.
**단계 순서**: ⓐ GC를 **spawn** → ⓑ **`collect_reached`를 await**(= 패스가 `collect_referenced` **안에** 있음이
확정된다 — **여기서 기다리지 않으면** putter의 포인터가 `SeedRoot`의 루트 readdir **이전에** 착지해 `refs`에
새어 든다) → ⓒ **그 park 동안** putter를 **spawn하지 않고 완주까지 await**한다(**패스 시작 시 존재하지 않던
버킷** `fresh`에 **X와 같은 내용**으로 put → dedup 분기(바이트 재기록 없음) → 커밋 → 핀 drop) → ⓓ `Notify`로
해제 → ⓔ `timeout(5s, gc)` = `Ok`.
⇒ **spawn만 하고 넘어가는 지점 0개**(putter는 spawn조차 하지 않는다 — 완주 await가 곧 도착이다).

- **함정 5**: GC의 `JoinHandle`은 `timeout(5s, gc)` → **`JoinError` 언랩**(패닉 = 즉시 RED) → **`io::Result`
  언랩**까지 간다(`let _ = gc.await;` **금지**). putter는 태스크가 아니라 **직접 await**하므로 `JoinError`가
  없다 — 대신 **`put()`의 `Ok`를 단언**한다(put이 실패하면 `landed`가 서지 않아 **엉뚱한 이유로 RED**가 된다).
  **함정 4**: putter 완주 = **핀 drop 확정**(보조정리 L).

**단언**: `stats == ReconcileStats{referenced:1, gc_deleted:0, gc_pending:1, temps_deleted:0, quarantined:0}`
∧ `get_bytes("fresh","v.bin").is_ok()` ∧ **무덤 잔재 0**.

- **삭제 분기 자기검증**: **`referenced == 1`** = **D 하나뿐**이다. putter의 포인터가 스냅샷에 새어 들어왔다면
  **2**가 된다 → **참조됨 분기 누수를 시끄럽게 잡는다**. ∧ **`graved == vec![X_sha]`**(`post_grave` 훅 관측 =
  무덤이 **실제로 파였다** → 삭제 분기 진입 증명). `gc_pending == 1`은 X의 tombstone이 **복원 뒤에도 유지**됨
  (D-2)을 함께 못박는다.

**뮤턴트**:
- **M1 `enter_pass()`를 `collect_referenced` 뒤로** → put 착지 시 `pass_live=false` → 흔적 0, refs에도 없음 →
  Reap → `get_bytes` 404 → **RED**
- **`PassGuard::drop`의 `landed.clear()` 제거** → 관측 동일(GREEN) = **equivalent 뮤턴트**로 **정직하게 분류**
  (다음 패스가 시작 시 clear한다)

### T-B2 — "사전확인 ↔ 무덤 rename 창"의 결정적 증인

GC를 **모델링된 사전확인 지점**(= `pre_grave` 훅)에서 park한다. **그 park 동안**, putter가 **비로소 시작**해
X를 dedup 관측(`blob_intact == true` — 무덤은 아직 없다)하고 **완전히 착지**한다(핀 drop, 포인터 on-disk).
그 다음 GC 재개 → `grave()` → `settle()`.

**랑데부**: `pre_grave` = **도착 `gc_at_pre_grave` 송신 → `Notify` park**.
**단계 순서**: ⓐ GC **spawn** → ⓑ **`gc_at_pre_grave` await** → ⓒ *그제서야* putter 시작 → **완주까지 await**
(핀 drop · 포인터 on-disk) → ⓓ 해제 → ⓔ `timeout(5s, gc)` = `Ok`.

- **함정 4·5**: putter는 **완주까지 await**된다(spawn하든 직접 await하든 — **완주가 곧 도착**이므로 spawn 지점이
  **아니다**). 그 완주가 **핀 drop을 확정**한다(**보조정리 L**) ⇒ *"무덤 시점 코호트는 비어 있다"*는 이 테스트의
  전제가 **논증이 아니라 관측**이 된다. **`put()`의 `Ok`를 단언**하고, 태스크로 spawn했다면 **`JoinError`도
  언랩**한다. GC 핸들은 **`JoinError` + `io::Result` 둘 다 언랩**한다.

⚠ **ⓑ를 빼면(= spawn 직후 곧바로 putter)** 패스가 아직 폴링되지 않았을 수 있고, 그러면 putter가 `fresh` 버킷을
**`SeedRoot`의 루트 readdir 이전에** 만들어 **포인터가 `refs`에 새어 든다** → **참조됨 분기 누수가 이 테스트에서
재발한다.** 아래 "구조적으로 먼저"라는 논증은 **패스가 실제로 시작한 뒤에만** 성립한다 — **`gc_at_pre_grave`가
그 전제를 기계로 못박는다.**

**결정성의 근거(시간에 기대지 않는다)**: `PassGuard::begin`의 `collect_referenced`는 `pre_grave`보다 **구조적으로
먼저** 끝난다 → putter의 포인터는 **`refs`에 절대 들어갈 수 없다**. putter는 패스 시작 시 존재하지 않던 버킷
`fresh`에 쓴다(`SeedRoot` 성질) → 이중 보증.

**삭제 분기 자기검증**: 위 두 줄은 **논증**일 뿐이다 — 이제 **단언으로 승격**한다:
**`stats.referenced == 0`**(패스 시작 시 포인터가 **하나도 없다** — X의 포인터는 tombstone 시드 때 지웠다.
putter의 포인터가 스냅샷에 새면 **1**이 된다) ∧ **`graved == vec![X_sha]`**.

**정상**: 무덤 시점 **코호트는 비어 있다**(핀이 이미 drop됐다) → 대기 0 → 그러나 `landed ∋ sha` → **Restore** →
`get_bytes` Ok(**바이트까지 비교**), `gc_deleted == 0`, 무덤 잔재 0.

**뮤턴트**:
- **① `landed` 삽입 삭제**(또는 `PinGuard::drop`에서 `landed.remove`) → 코호트도 비고 `landed`도 비었다 →
  Reap → 404 → **RED**
- **② 사전확인(load-bearing)** — `pins.rs`에 손으로 lock-and-peek을 추가해 **`pre_grave` 시점에** 보호 여부를
  판정하고 미보호면 무덤 없이 즉시 reap: 그 시점엔 putter가 **아직 시작조차 안 했다** → `live`도 `landed`도
  **비어 있다** → 미보호 판정 → **Reap** → 그 사이 putter가 dedup으로 착지 → **포인터 + blob 부재** → 404 →
  **RED**
- ⚠ **그런데 이 뮤턴트가 "컴파일 불가"라면 왜 테스트가 필요한가?**(정직한 답) — 컴파일 불가한 것은 **`settle()`을
  `grave()` 앞으로 옮기는 재배치**뿐이다(`Graved` 없이는 `settle`을 호출할 방법이 없다). `pins.rs`를 **편집해**
  새 lock-and-peek 코드를 **추가**하는 것은 재배치가 아니라 **새 API 추가**이며 **컴파일된다**. 봉인은 **모듈
  경계**이지 타입 마법이 아니다 → **T-B2가 2차 방어선이다.** 또한 타입은 "Restore가 **반드시** 일어난다"는
  **양성 방향**을 강제하지 못한다 — 그건 오직 테스트가 한다.

### T-B4 — 관측 후·커밋 전 park (**코호트 대기 킬**)

putter를 `post_observe`에서 park(intact=true, 핀 live, 미커밋) → GC 패스를 그 사이에 실행 → 무덤 시점 코호트 =
{그 핀} → `settle()`이 **대기에 들어간다** → putter 해제 → 착지 → 핀 drop → settle 깨어남 → `landed ∋ sha` →
**Restore 필수** → `get_bytes` Ok.

**랑데부**: `post_observe` = **도착 `observed` 송신 → `Notify` park** · `post_grave` = **기록(`graved`) + 도착
`graved_reached` 송신**(park 없음 — 통과한다).
**단계 순서**: ⓐ put **spawn** → ⓑ **`observed` await**(⚠ **여기서 기다리지 않으면** put이 아직 폴링되지 않아
**핀이 없을 수 있다** → 무덤 시점 **코호트가 비고** → `settle()`이 첫 검사에서 `Drained` → `landed` 없음 →
**Reap** → `get_bytes` 404 → **엉뚱한 이유로 RED**) → ⓒ GC **spawn** → ⓓ **`graved_reached` await**(= 코호트가
**{그 핀}으로 확정된 뒤**임을 못박는다 — **이 순서가 M4 뮤턴트를 죽이는 힘의 원천이다**: 더 일찍 해제하면 put이
무덤 **이전에** 착지해 `landed`가 서고, M4 뮤턴트도 **Restore**로 살아남는다) → ⓓ′ **대기 진입 프로브**:
`timeout(200ms, &mut gc)` = **`Err`**(pending) — *"settle이 실제로 대기에 들어갔다"*를 **관측**한다 → ⓔ putter
해제 → **put 완주 await**(핀 drop) → ⓕ `timeout(5s, gc)` = `Ok`.

- **함정 3·4·5·6**: ⓓ′의 프로브는 **반드시 `&mut gc`**로 건다 — 값으로 넘기면 `Err`일 때 **`JoinHandle`이
  드롭돼 GC가 detach**된다. ⓔ의 put 핸들은 **`JoinError` 언랩 + `Ok` 단언**까지 간다 — 그 완주가 **핀 drop을
  확정**한다(**보조정리 L**). ⚠ **데드락 부재 sanity의 "무관한 put"도 spawn이 아니라 완주 await**로 건다
  (`timeout(5s, put_other)` = `Ok`).

**삭제 분기 자기검증**: putter는 `post_observe`(= **커밋 이전**)에 park돼 있으므로 GC의 `collect_referenced`
시점에 **포인터가 디스크에 없다** → **`stats.referenced == 0`**을 단언한다(누수하면 **1**) ∧
**`graved == vec![X_sha]`**.

**뮤턴트**:
- **M4 — 코호트 대기 제거**(`settle`이 즉시 `landed`만 본다): 판정 시점에 putter는 아직 park 중 → `landed`
  비었음 → **Reap** → 해제된 putter가 dedup으로 커밋(바이트 재기록 없음) → **포인터 + blob 부재** → 404 →
  **RED**
- **M4′ — 코호트를 무덤 rename 시점이 아니라 `settle` 진입 시점에 스냅샷**: 관측 동일(GREEN) = **equivalent
  뮤턴트**로 **정직하게 분류**. 코호트를 늦게 뜨면 **더 많이 기다릴 뿐** 안전 측이므로 결함이 아니다. 무덤
  **직후**로 고정하는 이유는 **성능**(자급자족 핀을 안 기다림)이지 안전이 아니다.

### T-C2 — 커밋 도중 호출자 취소 (crash 렌즈 FATAL의 결정적 증인)

put을 spawn → `in_commit_pre_rename`(blocking 클로저 **내부** 동기 훅)에서 park → **바깥 퓨처를 abort**(=
`upload_timeout` 시뮬레이션) → **⚠ 그 취소가 *완료*될 때까지 await하고 `JoinError::is_cancelled()`를 단언** →
*그 다음에* GC 패스 실행 → 무덤 시점 코호트 = {그 핀}(가드는 **클로저 소유**이므로 취소가 **완료된 뒤에도 살아
있다** — 뮤턴트에서는 **죽어 있다**) → `settle()`이 **대기** → 훅 해제 → 클로저가 rename·마킹·fsync·drop 완주 →
settle 깨어남 → `landed ∋ sha` → **Restore**.

**랑데부**: `in_commit_pre_rename` = **도착 `pre_rename_reached` 송신 → `std::sync::mpsc` park**(= `park_A`) ·
`post_grave` = **기록 + 도착 `graved_reached`**.

**최종 단계 순서 (spawn-후-진행 0 · abort-후-진행 0)**:
- ⓐ put을 **spawn**(`let mut put = tokio::spawn(async move { s2.put(b,"cancelled",X_bytes).await })`)
- ⓑ **`pre_rename_reached` await** — 이 시점 **확정 사실**: `blob_intact == true`(dedup) ∧ `stage` 성공 ∧
  **blocking 클로저가 *시작*됐다** ∧ 핀 live ∧ `landed` 무흔적 ∧ 포인터 부재
- ⓒ **`put.abort()`** (= `upload_timeout` 발화 · 클라이언트 disconnect 시뮬레이션)
- ⓓ **⚠ 취소 *완료*를 await한다**:
  **`let e = timeout(2s, &mut put).await.expect("abort must complete").expect_err("put must be cancelled");`**
  ∧ **`assert!(e.is_cancelled())`**
- ⓔ *그제서야* GC **spawn** → ⓕ **`graved_reached` await** → ⓖ **대기 진입 프로브** `timeout(200ms, &mut gc)`
  = **`Err`**(pending)
- ⓗ `tx_A`로 `park_A` **해제** → ⓘ `timeout(5s, gc)` = **`Ok`**(⚠ 해제 직후에는 **아무것도 단언하지 않는다** —
  함정 6: 해제 `send()`의 반환은 *"클로저가 재개했다"*가 아니다. **GC 완주가 그 관측이다**)

⚠ **ⓑ와 ⓓ는 서로 다른 것을 증명한다 — 둘 다 없으면 이 테스트는 아무것도 봉인하지 못한다.**
- **ⓑ = "blocking 클로저가 *시작*됐다".** 도착 **이전에** abort하면 클로저가 **시작조차 하지 않을 수 있고**
  (`join.rs:189-193`: *"The exception is if the task has not started running yet; in that case, calling `abort`
  may prevent the task from starting"*) → 핀이 **caller 퓨처와 함께 즉시 소멸** → *"가드는 클로저가 소유하므로
  취소에도 살아 있다"*는 **명제가 검증되지 않은 채** 테스트만 RED가 된다.
- **ⓓ = "호출자 취소가 *완료*됐다".** `abort()`는 취소를 **스케줄만 한다**. ⓓ 없이 GC를 spawn하면 **caller-owned
  뮤턴트에서 가드가 아직 살아 있을 수 있고**, 그러면 GC가 **그 가드를 코호트로 포착**해 settlement를 park했다가 →
  풀린 클로저가 포인터를 착지시키면 → **복원** → **뮤턴트가 GREEN으로 생존한다.** `graved_reached`·pending
  프로브는 **GC의 상태**를 증명할 뿐 **취소 완료를 증명하지 않는다.**
- ⚠ **ⓓ는 blocking 클로저의 *종료*를 뜻하지 않는다** — 그것은 **detach된 채 `park_A`에서 계속 살아 있다**.
  **이 비대칭이 T-C2의 명제 그 자체다.** 그래서 ⓓ의 await는 **막히지 않는다**: 바깥 태스크는 안쪽 blocking
  `JoinHandle`을 **드롭(detach)**하고 즉시 취소로 완료된다.
- ⚠ **`is_cancelled()` 단언은 패닉 탐지기도 겸한다**: put이 abort 이전에 **패닉**했다면 `is_panic()`이므로 이
  단언이 **RED**가 된다.

**단언**: `get_bytes(b,"cancelled")` = **`Ok`** ∧ **바이트 동일** ∧ `.objects/<sha>` **존재** ∧
`.gc-grave-<sha>` **부재** ∧ `gc_deleted == 0`.
**삭제 분기 자기검증**: put은 `park_A`(= **rename 이전**)에 있으므로 `collect_referenced` 시점에 **포인터가
없다** → **`stats.referenced == 0`** ∧ **`graved == vec![X_sha]`**.

**뮤턴트**:
- **① "가드를 클로저로 옮기지 않고 caller가 보유"**(= **이 테스트의 표적**) — **어떻게 죽는가**: 가드가 **바깥
  퓨처의 지역변수**다 → ⓒ의 abort가 그 퓨처를 드롭하면 **가드도 드롭된다** → **ⓓ가 그 드롭이 *끝났음*을 기계로
  확정한다** ⇒ ⓔ에서 GC가 무덤을 팔 때 **코호트가 비어 있다** → `settle()`이 첫 검사에서 **`Drained`** →
  `landed` 무흔적 → **Reap**(`gc_deleted == 1`) → ⓗ 해제 후 detach된 클로저가 rename을 **뒤늦게 착지**시킨다 →
  **포인터 존재 ∧ blob 부재** → `get_bytes` **404** → **RED**.
  ⚠ **ⓓ가 없으면 이 뮤턴트는 경합으로 살아남는다.**
  (독립 RED 신호 **3개**: `gc_deleted == 1` ∧ `get_bytes` 404 ∧ 프로브 ⓖ가 **`Ok`**가 된다)
- **② `commit_pointer`를 `tokio::fs` async 체인으로 되돌림**(= 취소 가능한 커밋) → 바깥 퓨처가 rename 체인을
  **소유**한다 → ⓒⓓ의 취소가 **rename을 통째로 취소**하거나, `tokio::fs::rename`의 내부 `spawn_blocking`이
  **뒤늦게 착지**한다(`fs/mod.rs:312`) → 그때는 **핀이 이미 죽었으므로** 뮤턴트 ①과 **동일하게** Reap → 뒤늦은
  착지 → **404** → **RED**
- **③ 코호트 대기 제거**(`settle`이 즉시 `landed`만 본다) → 판정 시 `landed` 비었음(put은 `park_A`) → **Reap** →
  해제된 클로저가 뒤늦게 착지 → 포인터 + blob 부재 → **404** → **RED** (ⓖ의 pending 프로브도 **`Ok`**가 되어
  함께 깨진다)

### T-C3 — 겹치는 실패 put의 결정적 증인 (`live`가 보호 술어가 **아님**을 못박는다)

1. 만료·미참조 blob X를 심는다. `b/poisoned.meta.json` **위치에 디렉터리를 심어** 커밋 rename을 **결정적으로
   EISDIR 실패**시킬 준비를 한다(T-C1과 동일한 기법).
2. `put(b, "poisoned", X_bytes)`를 **spawn** → `blob_intact == true`(dedup) → `stage` 성공 →
   **`in_commit_pre_rename` 훅에서 park**(park **이전에** 도착 `pre_rename_reached`를 송신한다).
2′. **⚠ `pre_rename_reached`를 `await`한다 — 여기서 기다리지 않으면 이 테스트는 *조용히* 무의미해진다.**
   `tokio::spawn`은 **폴링을 보장하지 않는다** → put이 아직 `pin()`도 못 했는데 3번이 무덤을 파면 **코호트가
   비고** → `settle()`이 첫 검사에서 **`Drained`** → `landed` 없음 → **Reap** → **`gc_deleted == 1`** — 이것은
   **6번이 기대하는 바로 그 값이다.** ⇒ 테스트는 **GREEN인데** *"겹치는 실패 put"* 시나리오는 **한 번도
   재현되지 않고**, "`live`를 보호 술어로 되돌리는" 뮤턴트도 **살아남는다**(핀이 없으니 되돌려도 Reap이다).
   **이 도착 신호가 T-C3의 킬 파워 전부를 지탱한다.**
   이 시점 확정 사실: **핀 live · 미착지(`landed` 무흔적) · 포인터 부재**.
3. GC(`run_once_at`)를 spawn → `pre_grave` 통과 → **무덤 rename** → 코호트 = {그 핀} → **`settle()`이 대기에
   들어간다**.
4. **대기 진입의 증인**: `post_grave` 훅의 도착 신호(`graved_reached`)를 **await한 뒤**
   `timeout(200ms, &mut gc_handle)`이 **Err(pending)** 임을 단언한다.
   *(보조 단언 — 주 단언은 6번이며 시간에 의존하지 않는다. ⚠ 이 pending 단언은 2′의 **대체재가 아니다**:
   2′가 없으면 이 단언은 **경합에 기대게 되고**, 통과할 때조차 **우연**이다.)*
5. 훅을 해제 → 클로저의 `rename(tmp → b/poisoned.meta.json)`이 **EISDIR로 실패** → `on_landed`는 **절대 호출되지
   않는다** → `commit_pointer` = `Err` → `put` = **`Err(Internal)`** → **핀 drop** → `notify_waiters()` →
   **`landed` 무흔적**.
5′. **⚠ put 핸들을 완주까지 await한다 — `timeout(5s, put)` = `Ok(Err(AppError::Internal))`**.
   · **함정 4**: 해제 `send()`의 반환은 *"클로저가 재개했다"*가 아니다. **완주 await만이** *"핀이 drop됐다 ·
     코호트가 드레인됐다 · `landed`가 비었다"*를 **관측**으로 만든다(**보조정리 L**).
   · **함정 5**: `JoinError`를 **언랩**한다 → put 태스크가 **패닉**했다면 **즉시 RED**다. 언랩하지 않으면
     패닉으로 인한 `landed` 무흔적을 **EISDIR 때문이라고 오독**하고 **GREEN**이 된다.
   · **단언은 `Err(Internal)`이어야 한다** — `Ok`면 EISDIR 셋업이 깨진 것이고 시나리오가 재현되지 않았다.
6. `settle()`이 깨어나 판정 → `landed(X)` = **false** → **Reap**.
   **주 단언**: **`gc_deleted == 1`** ∧ `.objects/<sha>` **부재** ∧ `.objects/.gc-grave-<sha>` **부재** ∧
   `get_bytes(b,"poisoned")` **404**(포인터 무흔적).

**삭제 분기 자기검증**: put은 `in_commit_pre_rename`에 park돼 있고 그 rename은 **끝내 EISDIR로 실패**하므로
포인터는 **한 번도 존재하지 않는다** → **`stats.referenced == 0`** ∧ **`graved == vec![X_sha]`**.

**뮤턴트(개정 전으로 되돌림 — `live`를 보호 술어로 복원: `restore ⇔ live ∨ landed`, 코호트 대기 없음)**
→ 판정 시점에 그 핀은 **park된 채 live** → **Restore** → **`gc_deleted == 0`** → **RED**
(4번의 pending 단언도 함께 깨진다 — **두 개의 독립 RED 신호**)

> **T-C1은 이 창을 열지조차 못한다** — 실패한 put이 **이미 반환된 뒤** reconcile을 돌리기 때문이다.
> **T-C3만이 연다.**

### T-P4a — 포인터 rename *이전*에 **영원히 멈춘 핀**

*T-C3와 형제지만 정반대를 친다*: T-C3의 핀은 **결국 죽는다**(rename이 EISDIR로 실패) → 결말이 확정된다.
**T-P4a의 핀은 죽지 않는다** → 결말이 **영원히 불명**이다.

1. 만료·미참조 blob X를 심는다.
2. `Hooks{ in_commit_pre_rename: park, post_grave: recorder }`. `park` = **도착 `pre_rename_reached` 송신 →
   mpsc park**(§5.3 — 테스트가 `tx`를 **모든 단언이 끝날 때까지** 쥔다 → **본문이 도는 동안 절대 풀리지
   않는다**. 해제는 **teardown에서 명시적으로** 한다 — 5단계).
3. `put(b, "stuck", X_bytes)`를 **spawn하고 핸들을 보유한다**(`let put = tokio::spawn(…)` — **`let _ =` 금지**)
   → `blob_intact == true`(dedup) → `stage` 성공 → **`in_commit_pre_rename`에서 park**.
3′. **⚠ `pre_rename_reached`를 `await`한다**(spawn ≠ polled). 이것 없이 4번으로 넘어가면 put이 **아직 `pin()`도
   안 한 채** GC가 무덤을 파 **빈 코호트**를 캡처하고 **즉시 reap**할 수 있다 → **단언 ①·②가 셋업 스케줄링
   때문에 RED**가 되고, *"영원히 멈춘 핀"*이라는 **시나리오 자체가 재현되지 않는다**(무한 대기 뮤턴트도
   **살아남는다** — 기다릴 코호트가 없다).
4. `timeout(5s, run_once_at(&s, now, gc_grace, /*settle_timeout*/ 200ms))` → **`Ok`여야 한다**
   → `pre_grave` 통과 → 무덤 rename → 코호트 = {그 핀} → `settle()` 대기 → **200ms 타임아웃**
   → **fail-CLOSED 복원** → `Settled::Deferred`.

- **단언 ① (유실 0)**: `.objects/<sha>` **존재** ∧ **바이트 동일** ∧ `.objects/.gc-grave-<sha>` **부재**
- **단언 ② (무회수)**: `stats.gc_deleted == 0`
- **단언 ②′ (삭제 분기 자기검증)**: put은 `in_commit_pre_rename`에 **영원히** park돼 있다 → 포인터가 **한 번도
  착지하지 않는다** → **`stats.referenced == 0`**(세 패스 **전부**) ∧ **`graved`가 X를 패스마다 1회 기록**
  (매 패스가 무덤을 **다시** 판다 — 타임아웃 복원으로 매번 정본으로 되돌아가므로).
- **단언 ③ (GC가 영구 정지하지 않는다 — `pass_lock` 해제)**: **후속** `timeout(5s, run_once_at(…))`가 **`Ok`**
  ∧ 역시 `gc_deleted == 0` ∧ blob 여전히 존재. *(핀은 **아직도** park돼 있다 — 그런데도 패스가 **완주한다**.)*
- **단언 ④ (격리 — 다른 blob은 오늘과 똑같이 회수된다)**: 만료·미참조 blob **Y**(핀 없음)를 심고 **세 번째**
  패스 → `timeout(5s, …)` = `Ok` ∧ **`gc_deleted == 1`**(Y가 회수됐다) ∧ X는 **여전히 존재**.
- **단언 ⑤ (관측 가능한 에러)**: 캡처된 tracing 출력에 **`"gc settle timed out"`** 이벤트가 **패스마다 1건**
  (레벨 ERROR · `sha`·`cohort_size=1`·`waited_ms` 필드 포함).
  *캡처*: `tracing_subscriber::fmt().with_writer(Arc<Mutex<Vec<u8>>>).finish()` +
  `tracing::subscriber::set_default(...)` 가드. `#[tokio::test]`는 **current-thread**이고 `settle()`의 `error!`는
  **reconcile 태스크(= 테스트 스레드)**에서 나므로 스레드-로컬 구독자가 잡는다.
  (`tracing-subscriber`는 이미 **정규 의존성**이다 — 새 dev-dep 0.)

5. **⚠ teardown (함정 9)**: **단언 ①~⑤가 전부 끝난 뒤**에만 실행한다.
   ① **`drop(tx);`** — park sender를 **명시적으로** 드롭한다 →
   ② **`let r = timeout(5s, put).await.expect("put must finish after park release")
      .expect("put task must not panic");`** → ③ **`assert!(r.is_ok());`**
   · **왜 `Ok`인가**: X의 정본 blob은 fail-CLOSED 복원으로 **디스크에 있고**(단언 ①), `b/stuck.meta.json`
     자리에는 **아무것도 없다**(EISDIR 함정은 **T-C3의 장치**이지 여기에는 **없다**) ⇒ 재개된 rename은
     **성공**하고 `put`은 **`Ok`**를 낸다. **이 단언은 픽스의 관측 계약을 건드리지 않는다** — *"teardown이
     조용히 깨져 있지 않다"*만 말한다.
   · ⚠ **순서 엄수**: **반드시 모든 단언 뒤**에 해제한다. 먼저 해제하면 핀이 drop되고 포인터가 착지해
     **"영원히 멈춘 핀"이라는 시나리오 자체가 사라진다.**

**뮤턴트**:
- **무한 대기**(= `await_settlement`를 `await_cohort_drained`로 되돌림): 코호트가 **영영 드레인되지 않는다**
  (park된 핀) → `settle()`이 **영영 깨어나지 않는다** → **4단계의 `run_once_at`이 반환하지 않는다** →
  **4단계의 `timeout(5s, …)`가 `Err`** → **패닉 = RED**.
  ⚠ **park를 절대 해제하지 않는다** → 뮤턴트에 **탈출구가 없다**. 패닉 unwind가 `tx`를 drop해 훅을 풀어 주므로
  **RED는 hang이 아니라 깔끔한 실패**로 뜬다.
  · ⚠ **정정 (함정 6)**: *"`pass_lock`을 쥔 채이므로 후속 패스도 전부 막힌다"*는 **거짓**이다.
    `tokio::time::timeout`이 `Err`를 내면 **안쪽 퓨처가 드롭될 뿐**이고, 그 드롭이 `run_once_at`의 지역변수인
    **`PassGuard`를 → `OwnedMutexGuard`를 → `pass_lock`을 해제한다.** ⇒ 후속 패스는 **락에서 막히지 않는다.**
    그것들은 **스스로 같은 이유로 hang한다**. **RED 신호는 여전히 2개지만 메커니즘이 다르다.**
    실제로 테스트는 **4단계에서 먼저 패닉하므로** ③에 도달하지 않는다.
- **fail-OPEN (타임아웃 시 Reap)**: 무덤을 지운다 → 그 뒤 park가 풀리면 rename이 착지 → **포인터 + blob 부재 →
  404** → **단언 ①이 RED**. *(fail-CLOSED가 load-bearing임을 못박는다.)*

> ### ⚠ T-P4b는 **두 증인으로 분리**됐다 — 역할이 다르다
>
> 두 테스트 모두 **reconcile을 먼저 시작해 `pre_grave`/무덤 rename까지 진행시킨 뒤**(= `collect_referenced`가
> 포인터를 **놓친 뒤**) put을 진행시킨다.
> **T-P4b-1** = *"`landed`가 이미 true면 **대기 0**"* · **T-P4b-2** = *"대기 **도중** 착지하면 **알림이 깨운다**"*.
> 한 테스트가 둘 다 증명할 수는 없다 — 전자는 settle이 **시작 전에** 이미 landed여야 하고, 후자는 settle이
> **이미 대기 중이어야** 한다. **상호배타적 순서다.**
> **두 증인의 골격은 동일하다**: *reconcile spawn → `pre_grave` 도착 await → put spawn → **put의 도착 await*** →
> 해제. 갈라지는 곳은 **put을 어디까지 진행시키느냐**뿐이다 — **T-P4b-1은 `in_commit_post_landed`까지**
> (착지 **완료**), **T-P4b-2는 `in_commit_pre_rename`까지**(착지 **이전**).

### T-P4b-1 — 무덤 시점에 `landed`가 이미 true (핀은 live) → **대기 0 · 즉시 복원**

1. 만료·미참조 blob **X**를 심는다. **포인터는 0개**다.
2. `Hooks{ pre_grave: gc_park, in_commit_post_landed: put_park, post_grave: recorder }`.
   · `gc_park` = **async** 훅 — 도달을 알리고(`gc_arrived`) **`Arc<Notify>`로 park**(5단계에서 **`notify_one()`**
     으로 해제).
     ⚠ **`oneshot`을 쓰지 마라 — 컴파일되지 않는다**: `AsyncHook = Arc<dyn Fn(&str) -> BoxFuture<'static,()>>`는
     **`Fn`**인데 `oneshot::Receiver::await`는 **`self`를 소비**한다(`FnOnce`). `notify_one()`은 **대기자가
     없어도 permit을 저장**하므로 **lost wakeup도 불가**하다(`notify_waiters()`를 쓰면 **유실된다**).
   · `put_park` = **sync** 훅 — 도달을 알리고(`landed_reached`) **mpsc park**(테스트가 `tx_put`을 **단언이 끝날
     때까지** 쥔다 → **본문이 도는 동안 절대 풀리지 않는다** → **핀이 살아 있다**).
     ⇒ put의 `JoinHandle`은 **보유한다**(`let put = tokio::spawn(…)`) — **7단계에서 await한다.**
   · **`settle_timeout` = 30s** — *이 테스트의 핵심 장치: 픽스는 그 30초를 **한 번도 건드리지 않아야** 한다.*
3. **reconcile을 먼저 spawn**(`gc = tokio::spawn(run_once_at(&s2, now, gc_grace, 30s))`) → `PassGuard::begin` →
   `recover_graves` → **`collect_referenced`**(포인터 **0개** → `refs = {}`) → 블롭 루프 → X의 tombstone
   **만료** → **`pre_grave`에서 park**. **`gc_arrived`를 기다려 이 상태를 확인한다.**
   · **사전조건 확인**: `.objects/<sha>` **존재**(무덤 아직 없음) ∧ `b/landed_then_stuck.meta.json` **부재**.
     ⇒ **`collect_referenced`는 포인터를 볼 수 없었다** — 참조됨 분기 누수가 **구조적으로 배제된다.**
4. **그 park 동안** put을 spawn: `put(b, "landed_then_stuck", X_bytes)` → `pin()`(무대기) →
   **`blob_intact == true`**(blob은 **아직 무덤으로 안 갔다**) → **dedup 분기**(바이트 재기록 **없음**) →
   `stage` → 커밋 **`rename`이 `Ok`** → **`landed` 삽입 + `notify_waiters()`**(⚠ 대기자 **0명** — settle은
   **아직 시작조차 안 했다**) → **`in_commit_post_landed`에서 park**(fsync 직전). **`landed_reached`를 기다린다.**
   · 이 시점: **포인터가 VFS에 실재**(핵심 사실 C) ∧ **`landed ∋ sha`** ∧ **핀은 여전히 live**(클로저 소유).
5. **`gc_park`을 푼다** → GC 재개 → **`grave()`** = blob→무덤 rename → **코호트 = {그 핀}**(⚠ **살아 있다**) →
   `post_grave` → **`settle()`** → `await_settlement`의 **첫 검사 ①**에서 `landed ∋ sha` →
   **`Settlement::Landed` 즉시 반환(await 0회)** → **즉시 복원**(무덤 → 정본).
6. **`timeout(2s, gc)` → `Ok`여야 한다.** ⚠ 이 2초 창은 **5단계(해제) 이후에만** 돈다 → **settle 구간만** 잰다.
7. **⚠ teardown**: **단언 ①~⑤가 전부 끝난 뒤** → **`drop(tx_put);`**(명시) → **`timeout(5s, put)`** →
   **`JoinError` 언랩** → **안쪽 `put()` = `Ok`** 단언. ⚠ **반드시 단언 이후**: 먼저 해제하면 **핀이 drop되어
   코호트가 드레인**되고, *"핀이 live인데도 즉시 복원됐다"*(단언 ②)는 **이 테스트의 요지가 사라진다.**

- **단언 ① (삭제 분기 자기검증)**: **`stats.referenced == 0`**(포인터는 **3단계 이후에** 착지했으므로 스냅샷에
  **없다**. 누수가 재발하면 **1**이 되어 **시끄럽게** 깨진다) ∧ **`graved == vec![X_sha]`**.
- **단언 ② (핀이 live인데도 즉시 복원 — 이 테스트의 요지)**: **단언 시점에 put은 여전히 `put_park`에 갇혀
  있다** ⇒ **코호트는 드레인되지 않았다.** 그런데도 `get_bytes(b, "landed_then_stuck")` = **`Ok`** ∧
  **바이트 동일** ∧ `.objects/<sha>` **존재** ∧ 무덤 잔재 0.
- **단언 ③ (무회수)**: `stats.gc_deleted == 0`.
- **단언 ④ (타임아웃을 안 태웠다 — 시간 무관, 주 단언)**: 캡처된 tracing에 **`"GC restored: landed commit"`이
  1건** ∧ **`"gc settle timed out"`이 0건**.
- **단언 ⑤ (시간 기반, 보조)**: 6단계의 `timeout(2s, gc)` = **`Ok`**(예산 **30s**의 1/15 — **15× 분리**).

**뮤턴트**:
- **landed 즉시복원 제거**(`await_settlement`의 검사 ① 삭제 → 무조건 코호트 드레인 대기): 코호트 = {**park된 핀**}
  → **영영 드레인되지 않는다** → settle이 **30s 예산을 전부 태운다** → **그 창 내내 `.objects/<sha>`가 부재 =
  실재하는 포인터가 404** → **단언 ⑤가 `Err`(RED)** ∧ **단언 ④의 두 문자열이 정확히 뒤바뀜(RED)**.
  **독립 RED 신호 2개.**
- **`landed` 삽입 자체 제거**: 보호 술어가 **false** → 검사 ①이 발화하지 않고 코호트도 드레인되지 않는다 →
  **30s 타임아웃** → `Deferred`(fail-CLOSED 복원 — **유실은 없다**) → 그러나 로그가 **`"gc settle timed out"`
  ×1 / `"GC restored: landed commit"` ×0**으로 **뒤바뀌고** 단언 ⑤도 `Err` → **RED ×2**.
- **`notify_waiters()` 제거** → **이 테스트는 GREEN이다**(settle이 **첫 검사에서** `landed`를 본다 — 깨울 필요가
  **없다**). **정직하게 적는다: T-P4b-1은 그 뮤턴트를 죽이지 못한다.** **그것이 T-P4b-2가 존재하는 이유다.**

### T-P4b-2 — 대기 **도중**에 착지 → `landed` 알림(`notify_waiters`)이 대기를 깨운다

1. 만료·미참조 blob **X**를 심는다(tombstone 만료). **포인터는 0개**다.
2. `Hooks{ pre_grave: gc_park, in_commit_pre_rename: park_A, in_commit_post_landed: park_B,
   post_grave: recorder+신호 }` — **전부 기존 훅이다**(`Hooks` 필드 **7개 불변 · 프로덕션 훅 0개 추가**).
   **모든 park이 「도착 신호 + 해제 신호」를 쌍으로 갖는다**:
   · `gc_park` = **async** 훅 — 도착 **`gc_arrived`** 송신 → **`Notify` park**(**5단계**에서 해제).
   · `park_A` = **sync** 훅 — 도착 **`pre_rename_reached`** 송신 → **mpsc park**(**6단계**에서 해제).
   · `park_B` = **sync** 훅 — 도착 **`post_landed_reached`** 송신 → **mpsc park**, **본문에서는 해제하지
     않는다**(테스트가 `tx_B`를 **모든 단언이 끝날 때까지** 쥔다 → **핀이 착지 이후에도 살아 있다**).
     ⇒ put의 `JoinHandle`은 **보유하고 8단계(teardown)에서 await한다**.
   · `post_grave` = 기록(`graved`) + 도착 **`graved_reached`** 송신(park 없음 — 통과한다).
   · **`settle_timeout` = 30s**.
3. **reconcile을 먼저 spawn**(`gc = tokio::spawn(run_once_at(&s2, now, gc_grace, 30s))`) → `PassGuard::begin` →
   `recover_graves` → **`collect_referenced`**(포인터 **0개** → `refs = {}`) → 블롭 루프 → X의 tombstone
   **만료** → **`pre_grave`에서 park**. **⇒ `gc_arrived`를 `await`한다**(다음 단계로 넘어가기 전에 **반드시**).
   · **사전조건 확인**: `.objects/<sha>` **존재**(무덤 **아직 없음**) ∧ `b/settle_wakeup.meta.json` **부재**.
4. **그 park 동안** put을 **spawn**: `put(b, "settle_wakeup", X_bytes)` → `pin()`(무대기) →
   **`blob_intact == true`** → **dedup 분기**(바이트 재기록 **없음** — 레이스의 전제) → `stage` →
   **`park_A`에서 park**(rename **직전**). **⇒ `pre_rename_reached`를 `await`한다**(**반드시**).
   · **⚠ 이 await가 봉인 그 자체다.** `tokio::spawn`은 **폴링을 보장하지 않는다** — 이 신호가 없으면 put이
     **아직 `pin()`도 못 한 채** GC가 재개돼 **빈 코호트**를 캡처하고 **즉시 reap**할 수 있다 → 5단계의 pending
     단언이 **`notify_waiters()` 제거가 아니라 셋업 스케줄링 때문에** 깨진다. **이 증인은 그때 아무것도 봉인하지
     못한다.**
   · 이 await 이후 **확정 사실**: **핀 live** ∧ **미착지**(`landed` **무흔적**) ∧ **포인터 부재**.
5. **`gc_park`을 푼다**(`Notify::notify_one()`) → GC 재개 → **`grave()`** = blob→무덤 rename →
   **코호트 = {그 핀}**(⚠ **4단계가 살아 있음을 확정했다**) → `post_grave` → **`settle()`** →
   `await_settlement`: 검사 ① `landed` **false**(put은 `park_A`에 있다) · 검사 ② 코호트 **미드레인**
   → **`notified.await`로 진입한다(= 대기 중).**
   **⇒ `graved_reached`를 `await`한 뒤 `timeout(200ms, &mut gc)`가 `Err`(pending)임을 단언한다.**
   · **왜 이것이 "settle이 `notified`에 park했다"를 함의하는가**(결정적 논증 — 우연에 기대지 않는다):
     `await_settlement`의 루프 몸통은 **동기**다(`Mutex` 검사뿐) — **유일한 await 지점이
     `timeout_at(deadline, notified)`**이다. 그리고 이 순간 세 종료 조건이 **전부 거짓**이다(`landed` 비었음 ∵
     put이 `park_A` · 코호트 살아 있음 ∵ **4단계의 도착 신호** · 30s 예산 남음) ⇒ **패스가 200ms 동안 반환하지
     않았다는 사실 자체가 "settle이 그 await에 있다"는 뜻이다.** 게다가 `notified.as_mut().enable()`이 **검사
     이전에** 호출되므로 **등록은 이미 끝나 있다**(lost wakeup 불가).
     ⚠ **이 논증의 두 전제**(*핀이 살아 있다* · *`landed`가 비었다*)는 **4단계의 `pre_rename_reached`가 없으면
     성립하지 않는다** — 그래서 그 신호가 **load-bearing**이다.
6. **그제서야** `tx_A`로 `park_A`를 **해제**한다 → 커밋 클로저 재개 → **`rename`이 `Ok`** → **`landed` 삽입 +
   `notify_waiters()`** → **`park_B`에서 park** — ⚠ **핀은 drop되지 않는다.**
   **이것이 이 테스트의 핵심 장치다**: `park_B`가 **핀을 착지 이후에도 살려 둠**으로써 **`PinGuard::drop`의
   알림이라는 대체 기상 수단을 제거한다.** ⇒ 이제 settlement를 깨울 수 있는 것은 **`landed` 삽입의
   `notify_waiters()` 하나뿐**이다(그 외에는 **30s 타임아웃**뿐).
   **⇒ `post_landed_reached`를 `await`한다** — *"착지했고, 핀은 **아직 살아 있으며**, 그 상태로 갇혔다"*가
   **논증이 아니라 관측**이 된다.
7. settlement가 **깨어나** 검사 ①에서 `landed ∋ sha` → **`Settlement::Landed`** → **즉시 복원**.
   **`timeout(2s, gc)` = `Ok`**(⚠ 이 2초 창은 **6단계(해제) 이후에만** 돈다).
8. **⚠ teardown**: **단언 ①~⑤가 전부 끝난 뒤** → **`drop(tx_B);`**(명시. `tx_A`는 **6단계에서 이미 해제**됐다) →
   **`timeout(5s, put)`** → **`JoinError` 언랩** → **안쪽 `put()` = `Ok`** 단언.
   ⚠ **반드시 단언 이후**: 먼저 해제하면 **핀이 drop되어 코호트가 드레인**되고, 그러면 *"깨운 것은
   `notify_waiters()` **하나뿐**"* 이라는 이 테스트의 **핵심 장치가 무너진다**(드레인이라는 **대체 기상 수단**이
   되살아나 `notify_waiters()` 제거 뮤턴트가 **살아남는다**).

- **단언 ① (삭제 분기 자기검증)**: **`stats.referenced == 0`** ∧ **`graved == vec![X_sha]`**.
- **단언 ② (대기 진입)**: 5단계의 `timeout(200ms, &mut gc)` = **`Err`**(pending).
- **단언 ③ (핀이 아직도 live인 채로 복원됐다)**: **6단계의 `post_landed_reached`가 도착했고**(= 착지 완료)
  **put은 `park_B`에 갇혀 있다**(→ **핀 미drop · 코호트 미드레인**) → 그런데도 `get_bytes` = **`Ok`** ∧
  **바이트 동일** ∧ `.objects/<sha>` 존재 ∧ 무덤 잔재 0 ∧ `gc_deleted == 0`.
- **단언 ④ (시간 무관, 주 단언)**: `"GC restored: landed commit"` **×1** ∧ `"gc settle timed out"` **×0**.
- **단언 ⑤ (시간 기반, 보조)**: 7단계의 `timeout(2s, gc)` = **`Ok`**(예산 30s — **15× 분리**).

**뮤턴트**:
- **`landed` 삽입의 `notify_waiters()` 제거** — ⚠ **이제 죽는다**: settlement는 6단계 **이전에 이미**
  `notified.await`에 park했다(**단언 ②가 그것을 못박는다**). 알림이 사라지면 **깨울 것이 아무것도 없다** — 핀은
  `park_B`에 갇혀 **drop되지 않으므로** `PinGuard::drop`의 `notify_waiters()`도 **오지 않는다**. ⇒ settlement가
  **30s 예산을 전부 태운다** → `TimedOut` → **단언 ⑤가 `Err`(RED)** ∧ **단언 ④의 두 문자열이 뒤바뀜(RED)**.
  **독립 RED 신호 2개.**
- **코호트 대기 제거** → 판정 시점에 `landed`가 비어 있다 → **Reap** → 해제된 put이 dedup으로 착지 →
  **포인터 + blob 부재 → 404** → **단언 ③이 RED**.

> ⚠ **정직하게 — `notify_waiters()` 제거 뮤턴트는 안전성 결함이 아니라 지연(latency) 결함이다.**
> 알림이 없어도 settlement는 **결국** 깨어나고(핀 drop **또는** `settle_timeout`) **어느 쪽이든 복원한다**
> (`Landed` 또는 fail-CLOSED `Deferred` — **디스크 전이가 같다**) ⇒ **유실 0 · 판정 동일.**
> 바뀌는 것은 **실재하는 포인터가 404를 내는 창의 길이**뿐이다. **T-P4b-2는 그 창을 관측 가능하게 만들어
> 뮤턴트를 죽인다** — 핀을 착지 이후에도 park해 **드레인이라는 대체 기상 수단을 제거**하면 지연이 **30s로
> 증폭되어 단언에 걸린다.** 다만 **결함의 등급은 그대로다**: 이것은 **가용성(404 창) 회귀**이지 **유실**이
> 아니다. **과장하지 않는다.**

### T-B5 — fault injection **4종**

#### ① 취소
`post_grave` 훅이 **도착 `graved_reached`를 송신한 뒤 park**한다 → reconcile을 **spawn** →
**`graved_reached`를 await** → *그제서야* 퓨처 abort(park한 async 훅째로 드롭된다 = 해제) →
**⚠ 취소 *완료*를 await한다** → `.gc-grave-<sha>` **정확히 1개** ∧ `<sha>` **부재** → **새 `run_once`** →
`recover_graves` 복원 → `get_bytes` Ok, 잔재 0.

- **랑데부(도착)**: **도착을 기다리지 않고 abort하면 무덤이 아직 안 파여 있다** → `.gc-grave-<sha>`가 **0개** →
  **엉뚱한 이유로 RED**(`recover_graves` 삭제 뮤턴트도 **살아남는다** — 복구할 무덤이 애초에 없다).
- **⚠ 랑데부(취소 완료). 이것은 T-C2와 *같은 함정의 두 번째 사례*다**: `abort()`는 취소를 **스케줄만 한다**.
  그 상태에서 곧바로 **새 `run_once`를 시작하면**, 아직 드롭되지 않은 `PassGuard`가 **`pass_lock`을 쥐고 있어**
  새 패스가 `lock_owned().await`에서 **막힌다** → 라이브니스 timeout에 걸려 **엉뚱한 이유로 RED**(또는 hang).
  **수정**: `gc.abort()` → **`let e = timeout(2s, &mut gc).await.expect("abort must complete")
  .expect_err("pass must be cancelled"); assert!(e.is_cancelled());`** → *그제서야* 디스크 단언 + 새 `run_once`.
  ⇒ **취소 완료 = `PassGuard` drop = `pass_lock` 해제**가 **관측**이 된다.
  *(함정 3 확인: abort 시점에 in-flight `spawn_blocking`은 **없다** — `grave()`의 rename은 `post_grave`
  **이전에** 이미 반환했다.)*

#### ② 크래시/재시작
`Store`를 드롭하고 무덤이 심어진 root에 **새 `Store`**를 만들어 `run_once` → 복원 + 포인터 관측 → 보존.
- **함정 4 ("확인했고 없음")**: `drop(store)`는 **디스크에 아무 효과도 없다**(`PassGuard::drop`은 디스크 무접촉).
  ②는 그 드롭의 효과에 **의존하지 않는다** — 전제는 **디스크에 놓인 무덤**뿐이고, 재시작 시뮬레이션의 동력은
  **새 `Store::new`가 새(빈) 핀 등록부를 만든다**는 사실이다(**D-3의 해저드를 의도적으로 쓴다**).

#### ③ 복원 실패
`restore_io` 훅으로 EIO 주입 → `run_once` = `Err`(io::Error **무가공**) ∧ 무덤 잔존 ∧ **unlink 0회** →
다음 패스 복구 → 유실 0.

#### ④ `Graved` 누수 (**fail-CLOSED by construction**)
무덤을 **실제로 판 뒤** `settle()`을 **부르지 않고** `Graved`를 **버린다** → 무덤 잔존 → **다음 패스가 복구한다**.

> ⚠ **`let _ = pass.grave(..)`는 *아무 일도 하지 않는다*.** `grave`는 **`async fn`** 이다 ⇒ **폴링되지 않은
> 퓨처를 드롭**할 뿐이고 **blob→무덤 rename이 *아예 일어나지 않는다*.** 그러면 `drop(pass)` 이후의 패스는
> **원래의 멀쩡한 blob**을 발견하고, **`recover_graves`가 통째로 깨져 있어도 테스트가 GREEN이다.**
> (`#[must_use]`조차 `let _ =`가 **삼킨다** — **컴파일러는 침묵한다**.)

**최종 안무 (5단계 — 순서가 곧 증명이다)**:

1. 만료·미참조 blob **X**를 심는다. **동시 put 0 · spawn 0 · park 0.** `Hooks{ post_grave: recorder }`.
2. **`let pass = PassGuard::begin(&s, settle_timeout).await.expect("begin");`**
   · **삭제 분기 자기검증**: **`assert!(pass.referenced().is_empty())`** — 포인터가 **하나도 없다**
     (`stats`가 없는 경로이므로 `referenced()`로 **같은 규율**을 건다).
3. **⚠ `grave()`를 `await`한다**:
   **`let graved = pass.grave(&x_sha).await.expect("grave rename must succeed");`**
   → **성공을 단언**한다(`Graved`는 **rename이 성공했을 때만** 태어난다).
4. **⚠ 복구 *이전* 디스크 상태를 단언한다** (이 단언들은 **복구 패스보다 먼저** 실행되어야 한다):
   · **`.objects/.gc-grave-<sha>` 존재** ∧ **무덤 정확히 1개** ∧ **정본 `.objects/<sha>` 부재**
   · **`graved == vec![X_sha]`**(`post_grave` 관측)
   ⇒ **이 네 줄이 P-8이 없앴던 바로 그 관측이다.** 개정 전에는 **넷 다 거짓**이었고(무덤 0개 · blob 존재)
   **아무도 그것을 묻지 않았다.**
5. **누수 시뮬레이션 → 복구**:
   · **`drop(graved);`** — **`settle()`을 부르지 않는다**(= **누수**). `Graved`에는 **파괴적 Drop이 없다** ⇒
     **디스크는 그대로**여야 한다 → **재확인**: 무덤 **여전히 1개** ∧ blob **여전히 부재**.
   · **`drop(pass);`** — **명시적으로** 드롭한다(⚠ **함정 4**). `PassGuard`가 살아 있으면 **`pass_lock`을 쥔
     채**이므로 **다음 `run_once`가 hang한다**. 스코프 종료에 기대지 않는다.
     *(순서는 **타입이 강제**한다: `Graved<'p>`가 `&'p PassGuard`를 빌리므로 `drop(graved)` ≺ `drop(pass)`.)*
   · **복구 패스**: **`timeout(5s, run_once_at(&s, t_before_expiry, gc_grace, settle_timeout))` = `Ok`** →
     `PassGuard::begin`의 **`recover_graves`가 무덤을 정본으로 되돌린다** → 블롭 루프는 X의 tombstone을
     **미만료**로 보고 **Skip**한다.
     ⚠ **`now`를 만료 이전으로 되돌리는 이유**(정직하게): 같은 `now`로 돌리면 그 패스가 복원 **직후** X를
     **정당하게 다시 파묻고 reap**한다(X는 진짜 가비지다) → *"복구됐다"*가 `gc_deleted == 1`로부터의 **간접
     추론**으로 약해진다. `now`를 되돌리면 **복원 그 자체를 직접 관측**한다. 이는 **테스트 안무**이며
     (`run_once_at`의 `now`는 **이미 주입형 인자**다) **프로덕션 경로를 한 줄도 건드리지 않는다.**

**단언 (복구 이후)**: `.objects/<sha>` **존재** ∧ **바이트 동일** ∧ **무덤 잔재 0** ∧ `gc_deleted == 0`.

**뮤턴트**:
- **`recover_graves` 삭제 → 이제 죽는다**: 복구 패스가 무덤을 되돌리지 못한다 → **blob 부재** ∧ **무덤 잔존 1**
  → **RED ×2**. ⚠ **P-8 이전에는 이 뮤턴트가 ④에서 GREEN이었다** — **④의 킬 파워 전부가 3·4단계에서 나온다.**
  (①②에서도 RED — 무덤 영구 잔존 → `get_bytes` 404)
- **`Graved`에 파괴적 Drop 추가**(drop 시 `remove_file(grave)` = fail-OPEN) → **5단계의 재확인 단언이 RED**
  (무덤이 사라졌다) ∧ 복구할 것이 없어 blob **영구 유실** → **RED ×2**.
- **rename 없이 `Graved`를 낳는다** → **4단계가 RED**(무덤 0개 ∧ blob 존재 ∧ `graved`가 **비어 있다**).

**삭제 분기 자기검증**: 이 네 시나리오에는 **동시 put이 아예 없다** → **참조됨 분기 누수의 여지가 구조적으로
없다.** 그래도 규율을 맞춰 **`stats.referenced == 0`**을 단언한다(④는 `run_once`를 거치지 않으므로
**`pass.referenced().is_empty()`** 로 같은 규율을 건다).

### T-Q2 · T-Q3 — `recover_graves`의 두 가드

- [ ] **T-Q2 — `recover_graves` 내용 검증**: `<sha>` 내용이 손상 ∧ `.gc-grave-<sha>`에 **정상** 사본 →
      `recover_graves` → **무덤이 정본을 덮어쓴다** → `get_bytes` Ok
      · **뮤턴트(`blob 존재 → remove_file(grave)` 무검증)** → 좋은 사본 소멸 → 격리 → 404 → **RED**
- [ ] **T-Q3 — `is_dir` 가드**: `.gc-grave-<64hex>`라는 **디렉터리**를 심는다 → `recover_graves`가 `is_dir`로
      스킵 → `<sha>`가 디렉터리가 되지 않음 → 이후 put 정상(500 영구화 없음)

### 컴파일 불가 뮤턴트 (**정직한 목록**)

**`settle` 이전의 사전확인.** 무엇이 정확히 컴파일 불가인가:

1. 보호 판정 API는 **`Graved::settle(self)` 하나뿐**이다. `BlobPins`에는 sha로 물어볼 수 있는 **공개 술어가
   존재하지 않는다**(`landed()`/`cohort_at_grave()`/`await_settlement()`/`Settlement`은 `pins.rs` **private**).
   ⇒ **`reconcile.rs`는 사전확인을 표현할 방법이 아예 없다.**
2. `Graved`를 만드는 **유일한 길은 `PassGuard::grave()`이고, 그것은 blob→무덤 rename을 실제로 수행한다.**
   ⇒ `pass.grave(&name).await?.settle().await?`에서 **`settle()`을 `grave()` 앞으로 옮기는 재배치는 컴파일되지
   않는다.**
3. `settle`은 `self`를 **소비**하므로 "판정만 미리 얻어 두고 나중에 쓴다"도 표현 불가다.

- ⚠ **경계(정직 — 과장하지 않는다)**: 이 봉인은 여전히 **모듈 경계**다. **`pins.rs`를 편집해** 새 술어 API를
  **추가**하면 풀린다. 그건 **재배치가 아니라 새 API 추가**이므로 뮤턴트 클래스 **밖**이다. **"타입이 모든 걸
  막는다"고 주장하지 않는다.**
- **그래서 2차 방어선이 필요하다**: **T-B2**가 그 뮤턴트를 **행동으로** 죽인다. **타입 + 테스트 이중 봉인이며,
  문서는 둘 중 어느 하나도 단독으로 충분하다고 주장하지 않는다.**

### Sanity 3종

- [ ] **성능 sanity**: reap당 fsync **+2**, restore당 **+1** — adversarial 루프 실행시간 회귀 없음.
      **코호트 대기의 실행시간 영향 = 0임을 실측으로 못박는다**: 105개 characterization + `tests/adversarial.rs`
      (40객체)에는 GC와 동시에 같은 sha를 dedup-put하는 시나리오가 **없다** → 코호트는 **항상 비어 있고**
      `await_settlement`가 **첫 검사에서 `Drained`를 반환**한다(await 0회 · fast path). 회귀가 보이면 fast path가
      깨진 것이다. **`settle_timeout`은 이 스위트들에서 단 한 번도 발화하지 않아야 한다** —
      `"gc settle timed out"` 로그가 **0건**임을 함께 확인한다(발화했다면 정상 경로에 연기가 생긴 것 = P-2 재발)
- [ ] **데드락 부재 sanity**: `settle()`이 대기하는 동안 **다른 키에 대한 put이 정상 완료**됨을 단언(T-B4의
      park 중 무관한 put 1건 → Ok, timeout 5s). GC→put 단방향 대기 · put은 `pass_lock`을 잡지 않음을 못박는다
- [ ] **라이브니스 sanity**: **모든** `run_once_at` 호출을 테스트에서 `tokio::time::timeout`으로 감싼다.
      **패스가 반환하지 않는 것은 hang이 아니라 실패여야 한다.**
      · **⚠ 함정 6**: `timeout`의 `Err`는 **안쪽 퓨처를 드롭할 뿐이다.** `run_once_at`을 드롭하면 그 지역변수인
        **`PassGuard`가 → `pass_lock`이 해제**되고 **무덤은 디스크에 남는다**(의도된 fail-CLOSED). ⇒ **`Err`는
        반드시 그 자리에서 패닉시켜야 한다**(`.expect(...)`). 조용히 넘어가면 **다음 단언이 "패스가 중간에 잘린"
        오염된 상태**를 보게 되고, 그 RED/GREEN은 **아무 의미가 없다.**
      · **⚠ GC `JoinHandle`을 프로브할 때는 `&mut`로 건다**(`timeout(200ms, &mut gc)`). **값으로 넘기면** `Err`일
        때 **핸들이 드롭돼 GC 태스크가 detach**된다 → 이후 단언이 **아직 끝나지도 않은 패스**를 읽는다.

---

## 7. 보고 (완료 시 반드시 포함)

1. `cargo test` 출력 — **105 passed** ∧ 회귀 테스트 **GREEN 20/20**.
2. `git diff`에서 **비트로트 격리 분기 0줄 변경**임을 보여라(D-4).
3. `git grep -n 'allow(dead_code)' -- src/store/pins.rs` → **0건**.
4. **뮤턴트 킬 실증 — 아래 각각을 실제로 적용해 RED 출력을 캡처하고 원복한 뒤 그 출력을 붙여라.**
   **주장은 증거가 아니다.**
   | 테스트 | 뮤턴트 |
   |---|---|
   | T-B1 | `enter_pass()`를 `collect_referenced` 뒤로 (M1) |
   | T-B2 | ① `landed` 삽입 삭제 · ② `pins.rs`에 lock-and-peek 사전확인 추가 |
   | T-B4 | M4 — 코호트 대기 제거 |
   | T-C2 | ① caller-owned `PinGuard` · ② `commit_pointer`를 async 체인으로 · ③ 코호트 대기 제거 |
   | T-C3 | `restore ⇔ live ∨ landed` 복원(코호트 대기 없음) |
   | T-P4a | ① 무한 대기(`await_settlement` → `await_cohort_drained`) · ② fail-OPEN(타임아웃 시 Reap) |
   | T-P4b-1 | ① `await_settlement`의 검사 ① 삭제 · ② `landed` 삽입 삭제 |
   | T-P4b-2 | ① `notify_waiters()` 제거 · ② 코호트 대기 제거 |
   | T-B5④ | ① `recover_graves` 삭제 · ② `Graved`에 파괴적 Drop · ③ rename 없이 `Graved` 생성 |
   | T-Q2 | `blob 존재 → remove_file(grave)` 무검증 |
5. **equivalent로 분류한 뮤턴트**(죽지 않는 것)를 **정직하게** 보고하라:
   `PassGuard::drop`의 `landed.clear()` 제거(T-B1) · 코호트를 `settle` 진입 시점에 스냅샷(M4′, T-B4) ·
   `notify_waiters()` 제거는 **T-P4b-1에서는 GREEN**이고 **T-P4b-2에서만 죽는다**.
6. `"gc settle timed out"` 로그가 characterization + adversarial 스위트에서 **0건**임을 확인한 출력.
7. `cargo clippy -D warnings` 출력.
