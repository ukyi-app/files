---
bugfix: reconcile-gc-dedup-race
invariant-class: bugfix
entry-track: bug
review-track: full
pipeline-stage: release-gate
issue-tracker: local
symptom: "reconcile가 참조 스냅샷을 뜬 뒤 동시 put이 dedup 경로로 그 블롭을 커밋하면, GC가 살아있는 블롭을 삭제한다 — 커밋 포인터는 남고 블롭만 사라져 객체가 영구 non-servable이 된다(GET 404 / list 제외). 데이터 손실."
red-baseline: 65458082b6692acd0345763da96ef9a811ae745e
bugfix-lock: green
first-increment: [B-1]
increments: [B-1, B-2, B-3]
spike-1:
---

# reconcile GC ↔ dedup-put 레이스로 인한 블롭 유실

> **개정 이력**: 이 문서는 Codex plan gate **r1**(`needs-attention`, 치명 1 · 중대 2 — 전부 인간 triage Accept)
> 이후 **전면 개정**됐다. 개정 1차안은 다시 3렌즈 적대적 반증(crash / flip / mutant)에 걸려 **3건 전부 fatal**로
> 죽었고, 그 반증을 봉인한 안이 Codex plan gate **r2**(`needs-attention`, 중대 2)에 다시 걸렸다 — **P-1은 해소**,
> **P-2·P-3은 잔존**. 인간이 **수동 3라운드를 승인**(하드룰 4 (b) 경로)했고, P-2·P-3을 정밀 봉인한 안이
> **r3**에 제출됐다. **r3에서 P-1·P-2·P-3과 커밋된 RED 증인은 전부 sound 판정**을 받았으나, **P-2를 봉인하며
> 도입한 코호트 대기 자체가 새 결함(P-4)을 낳았다**. 인간이 **round 4를 승인**했고, P-4를 정밀 봉인한 안이
> **r4**에 제출됐다. **r4는 P-4의 fix model 자체를 sound로 판정**했다(*"No other new critical P-4 issue found.
> **Simpler alternative: none; repair the test choreography without changing the fix model.**"*) — 남은 지적은
> **단 하나, T-P4b의 테스트 안무 결함**이었다. 인간이 **round 5를 승인**했고, 그 개정은 T-P4b를
> **T-P4b-1 / T-P4b-2** 두 증인으로 분리했다. **r5는 T-P4b-1을 sound로 판정**했으나 **T-P4b-2에서 새 결함
> (P-6)**을 잡았다 — *"**This needs no new production hook or fix-model change.**"* 인간이 **round 6을 승인**했고,
> 그 개정이 바꾼 것은 배리어 테스트의 랑데부 안무 하나뿐이었다(→ **§랑데부 규율**). **r6은 P-6을 해소로
> 판정**했으나 **T-C2에서 같은 질병의 세 번째 변종(P-7)**을 잡았다 — 다시 *"no production hook or fix-model
> change"*. 인간이 **round 7을 승인**하면서 **한 테스트가 아니라 함정 클래스 전체의 전수 점검을 요구**했다
> (→ **§「개시 ≠ 완료」 클래스 전수 점검**).
> **`src/` 설계는 여섯 라운드 내내 한 글자도 바뀌지 않았다.** 판정 근거는 전부 `## Review Decision Log`에 있다.
> 컨덕터 판정 **D-4**로 최종안에서 **B-3의 격리(quarantine) 분기 봉인은 제외**됐다(→ F-25). 이 픽스는
> "포인터만 남고 blob 부재" 증상 클래스에 대해 **부분 해결**이다. §Preserved Contract·§남은 위험 참조.
>
> **r2 봉인 요약**(r3에서 **sound 판정** — 이 개정에서 **건드리지 않는다**):
> - **P-2** — `live`는 더 이상 **보호 술어가 아니다**. `Graved`가 **무덤 rename 시점의 핀 코호트**(단조 id 집합)를
>   품고, `settle()`이 그 **고정·유한 집합이 전부 종료(drop)될 때까지 await**한 뒤 **`landed(sha)` 하나만** 본다
>   → 결말을 **알고 나서** 판정한다. 유실 0 · **연기 0**. 증인: **T-C3**.
> - **P-3** — **`RenameReceipt` 삭제**. 보호 판정 API는 **`Graved::settle(self)` 하나뿐**이고 `Graved`는
>   **`PassGuard::grave()`의 rename이 성공했을 때만** 태어난다 → 판정이 **전이·sha에 바인딩**된다. 증인: **T-B2(개정)**.
>
> **r3 봉인 요약 (P-4 — 이 개정에서 바뀐 것 전부)**:
> - **P-4** — **코호트 대기가 무한정이었다.** r2안이 주장한 상계 `< gc_grace`는 **거짓**이다: `upload_timeout`은
>   **호출자 퓨처를 드롭할 뿐**인데, 계획은 의도적으로 `PinGuard`를 **abort 불가능한 `spawn_blocking` 클로저**로
>   옮겼다 → 멈춘 파일시스템 연산이 코호트 멤버를 **영원히 살려 둘 수 있다** → 무덤이 이미 파인 상태로 GC가
>   **영구 정지**하고(`pass_lock` 보유) 실재하는 포인터가 **무한정 404**를 낸다.
>   **봉인**: `Graved::settle()`을 **유한·fail-closed**로 만든다 — ① `landed`가 true면 **대기 0**(즉시 복원,
>   `Notify`를 `landed` 삽입에서도 울린다) · ② 그 외에는 **명시적 `settle_timeout`**(= `upload_timeout` **파생**,
>   `main.rs`가 계산해 주입)까지만 대기 · ③ 타임아웃 시 **무덤을 정본으로 복원**(데이터 보존 우선)하고 tombstone
>   유지(D-2)·`gc_deleted` 무증가 · ④ **패스는 반드시 해제**(다른 blob의 GC가 막히지 않는다) · ⑤ `tracing::error!`로
>   **관측 가능한 에러**(⚠ `ReconcileStats` 필드 **추가 금지**). 증인: **T-P4a**(rename **이전** 영구 스톨) ·
>   **T-P4b-1**(rename **이후** 스톨 = `landed` **즉시 복원**) · **T-P4b-2**(`landed` **알림**이 대기를 깨운다).
> - 그 외 **아무것도 바꾸지 않았다** — P-1 봉인(무덤 이름공간·복구), 코호트 모델·`landed` 단일 보호 술어·
>   `Graved::settle(self)` 단일 API·무덤 이름공간·복구 경로, D-1~D-4, F-25~F-28, "부분 해결" 선언 **불변**.
>
> **r4 봉인 요약 (테스트 안무만, `src/` 무변경 — r5에서 **T-P4b-1은 sound 판정**, 그대로 유지)**:
> - **T-P4b가 참조 스냅샷 *이전에* 포인터를 착지시켰다.** `collect_referenced`는 `PassGuard::begin` 안에서
>   블롭 루프보다 **먼저** 돈다 → 포인터가 그 전에 착지하면 **`refs`에 들어가고**, 블롭은
>   **참조됨 분기**(`refs.contains(&name)` → `pending.remove`)로 새어 **`grave()`도 `settle()`도 호출되지 않는다.**
>   기대한 복원 로그가 없으니 T-P4b는 **엉뚱한 이유로 RED**였고 — **landed-우선 복원을 제거해도 초록으로 남을 수
>   있었다.** 아무것도 증명하지 못하는 증인이었다.
> - **봉인**: T-P4b를 **역할이 다른 두 증인으로 분리**한다. 둘 다 **reconcile을 먼저 시작**해
>   `collect_referenced`가 포인터를 **놓친 뒤** put을 진행시킨다.
>   **T-P4b-1** = `landed` **즉시복원(대기 0)** · **T-P4b-2** = `landed` **알림(`notify_waiters`)이 실제로 대기를
>   깨운다**(이전 개정이 *"죽일 수 있는 테스트가 없다"*고 **정직하게 equivalent로 분류**했던 뮤턴트 —
>   **이제 죽는다**. §T-P4b-2).
> - **전수 점검**: 다른 배리어 테스트(T-B1/T-B2/T-B4/T-C1/T-C2/T-C3/T-B5)에 **같은 함정이 있는지 전부 확인**했다.
>   **순서 결함은 없었다**(전부 포인터가 collect **이후**에 착지하거나 아예 착지하지 않는다) — 그러나 **그 사실을
>   테스트가 스스로 증명하지는 않았다.** 이제 **모든** 배리어 테스트가 **"삭제 분기에 실제로 들어갔다"를
>   자기검증**한다(§삭제 분기 자기검증).
>
> **r5 봉인 요약 (이 개정에서 바뀐 것 **전부** — 랑데부 안무만, `src/` 무변경 · **훅 0개 추가**)**:
> - **P-6 — `tokio::spawn`은 폴링을 보장하지 않는다.** T-P4b-2는 put을 spawn하고 **곧바로** reconcile을 spawn했다.
>   `park_A`에는 **도착 신호가 없었다** → GC가 **put이 X를 핀하기도 전에** 무덤을 파고 **빈 코호트**를 캡처해
>   즉시 reap할 수 있다 → 증인이 **셋업 스케줄링 때문에** RED가 된다(= `notify_waiters()` 제거를 못 잡는다).
>   r4가 잡은 **"참조됨 분기 누수"와는 다른 병**이다(그건 *순서*의 문제, 이건 *스케줄링*의 문제).
> - **봉인**: **모든 park에 「도착 신호 + 해제 신호」의 쌍**을 의무화한다(§랑데부 규율). 신호는 **전부 테스트 쪽
>   채널**이며(`tokio::sync::mpsc` 도착 · `std::sync::mpsc`/`Notify` 해제) **기존 훅 클로저 안에서만** 산다 →
>   **프로덕션 훅 0개 추가**(`Hooks` 필드 **7개 불변**) · **fix model 무변경**(Codex: *"This needs no new production
>   hook or fix-model change."*).
> - **T-P4b-2를 승인된 순서로 재작성**: reconcile spawn → **`pre_grave` 도달 await** → put spawn →
>   **`pre_rename_reached` await** → `pre_grave` 해제 → **`post_grave` await + pending 프로브** → `park_A` 해제 →
>   **`post_landed_reached` await** → `timeout(2s, gc)`. **spawn만 하고 넘어가는 지점이 0개다.**
> - **전수 재점검(이 렌즈로 다시)**: **T-B2 · T-B4 · T-C2 · T-C3 · T-P4a · T-B5①에서 같은 함정을 발견**해
>   **전부 고쳤다**(도착 신호 추가). 특히 **T-C3는 조용히 GREEN으로 남을 수 있었다** — 빈 코호트 reap의
>   `gc_deleted == 1`이 **기대값과 우연히 일치**하기 때문이다(가장 위험한 형태). **T-P4b-1은 이미 두 도착 신호를
>   갖고 있었다**(r5 sound 판정과 일치) · **T-B1/T-C1/T-Q2/T-Q3는 park·spawn 지점이 없거나 완주 await뿐**이다.
> - **역할·단언·뮤턴트 분석은 하나도 바꾸지 않았다.** 바뀐 것은 **"언제 다음 단계로 넘어가는가"** 뿐이다.
>
> **r6 봉인 요약 (이 개정에서 바뀐 것 **전부** — 테스트 안무만, `src/` 무변경 · **훅 0개 추가**)**:
> - **P-7 — `abort()`는 취소 완료가 아니다.** T-C2는 `pre_rename_reached`(= blocking 클로저가 **시작**됐다)를
>   기다린 뒤 **곧바로** `abort()`를 부르고 **즉시 GC를 spawn**했다. `JoinHandle::abort()`는 취소를 **스케줄만
>   한다**(`tokio-1.52.3/src/runtime/task/join.rs:227-229` — `remote_abort()`; `:231-236` — *"the cancellation
>   process may take some time"*). ⇒ **caller-owned `PinGuard` 뮤턴트**(= T-C2가 죽이겠다고 선언한 바로 그것)에서
>   가드가 **아직 드롭되지 않은 채** GC가 무덤을 파면 **코호트에 그 핀이 잡히고** → settle이 park했다가 → 풀린
>   클로저가 포인터를 착지시키면 → **복원** → **테스트가 GREEN으로 남는다.** 취소로 인한 데이터 손실 경로가
>   **그대로 출하된다.** `graved_reached`·pending 프로브는 **GC의 상태**를 증명할 뿐 **취소 완료**를 증명하지 않는다.
> - **봉인**: `abort()` **직후 바깥 put `JoinHandle`을 유한 타임아웃으로 await**하고 **`JoinError::is_cancelled()`
>   를 단언**한 **뒤에야** GC를 spawn한다. `park_A`는 계속 막아 둔다. (tokio 자신의 doctest가 정확히 이 형태다 —
>   `join.rs:214-220`: `handle.abort(); … assert!(handle.await.unwrap_err().is_cancelled());`)
> - **질병에 이름을 붙였다** — r5/P-6(`spawn` ≠ 폴링됨)과 r6/P-7(`abort()` ≠ 취소 완료)은 **같은 병의 두 변종**이다:
>   > **비동기 연산의 *개시*를 그것의 *완료*로 착각한다.**
>   그래서 이번 라운드는 **한 테스트를 고치지 않고 클래스를 쓸었다** — **8개 함정 항목 × 전 배리어 테스트**를
>   1:1로 대조했다(→ **§「개시 ≠ 완료」 클래스 전수 점검**). **T-C2 외에 4건을 더 찾아 고쳤다**:
>   **T-B5①**(같은 P-7 — abort 후 취소 완료를 안 기다리고 새 패스를 시작 → **`pass_lock`에서 hang 가능**) ·
>   **T-B5④**(`drop(pass)` 누락 → 다음 패스가 `pass_lock`에서 **hang**) · **T-P4a**(뮤턴트 RED 논증이 **거짓**이었다
>   — `timeout`의 Err는 안쪽 퓨처를 **드롭**해 `pass_lock`을 **푼다**) · **T-P4b-1**(`oneshot` park은 `Fn` 훅에
>   **들어가지 않는다** → `Notify`) · 그리고 **T-B1/T-B2/T-B4/T-C3**에 **JoinError·put 결과 단언**을 의무화했다
>   (완주 await 없이 버린 핸들은 **패닉을 조용히 삼킨다**).
> - **역할·단언·뮤턴트 분석은 이번에도 바꾸지 않았다**(T-P4a의 **거짓 논증 1건 정정** 제외 — 그건 결함이지 설계가 아니다).
>
> **r7 봉인 요약 (이 개정에서 바뀐 것 **전부** — 테스트 안무만, `src/`·`tests/` 무변경 · **훅 0개 추가**)**:
> - **P-8 (critical) — 호출은 폴링이 아니다.** T-B5④의 `let _ = pass.grave(..)`는 **`async fn`의 퓨처를 폴링도
>   하지 않고 드롭**했다 ⇒ **blob→무덤 rename이 일어나지 않았다** ⇒ 다음 패스는 **멀쩡한 blob**을 보고,
>   **`recover_graves`를 통째로 삭제해도 GREEN**이었다. **fail-closed 증인이 아무것도 증명하지 못했다.**
>   (`#[must_use]`조차 `let _ =`가 삼킨다 — **컴파일러는 침묵했다**.)
> - **봉인**: `grave()`를 **await**하고 → **복구 이전 디스크 상태를 단언**(무덤 **정확히 1개** ∧ 정본 blob
>   **부재** ∧ `graved == vec![X_sha]`) → **`Graved`를 `settle()` 없이 버리고**(누수 시뮬레이션) → **`drop(pass)`**
>   → 복구 패스. **`recover_graves` 삭제 뮤턴트가 이제 죽는다**(+ 파괴적 Drop 뮤턴트 · rename 없는 `Graved` 뮤턴트).
> - **P-9 (high) — park은 영원한 정지가 아니다. `tx` 드롭이 곧 재개다.** r6의 전수 점검이 T-P4a·T-P4b-1·T-P4b-2를
>   *"park 이후 실행되는 코드가 없다"*며 함정 5에서 **면제**했다. **거짓이었다** — teardown에서 클로저가 재개해
>   **rename·`landed` 삽입·fsync·`PinGuard::drop`을 완주**하고, `commit_pointer`의 **`.await.expect("join")`**
>   때문에 **그 구간의 패닉이 put 태스크의 패닉**이 된다 → **버려진 핸들이 삼킨다** → **초록.**
>   ⚠ **전수 점검이 스스로 면제 사유를 발명해 자기를 통과시켰다** — *"코드가 없다"*는 **논증**이고,
>   **규칙 0이 금지하는 바로 그것**이다.
> - **봉인**: 세 테스트 모두 **핸들을 보유** → **증인 단언을 전부 마친 뒤** → **park sender를 명시적으로 드롭** →
>   **`timeout(5s, put)`** → **`JoinError`와 안쪽 `put()` 결과를 둘 다 언랩**. **"의도적 미await" 예외는 폐기됐다.**
> - **두 함정 클래스를 다시 전수로 쓸었다** — 함정 항목이 **8개 → 10개**(**9 teardown** · **10 async 폴링**).
>   **새로 찾은 것**: **T-C2의 teardown 잔여 1건**(abort로 detach된 클로저는 **await할 핸들이 구조적으로 없다** —
>   **그것이 T-C2의 명제 그 자체다**. rename·`landed`까지는 **대리 관측**으로 봉인되고, **fsync 이후 패닉만
>   미관측**으로 **기록**한다 — 훅을 늘리지 않는다). 그 외에는 **없다**: `let _ = <async>`로 퓨처를 흘리는 곳은
>   **T-B5④ 하나뿐이었고**(`src/`·`tests/`의 `let _ = …`는 **전부 `.await`가 붙어 있다**), teardown에 재개될
>   것이 남는 테스트는 **위 셋 + T-C2뿐**이다.
> - **역할·단언·뮤턴트 표적은 이번에도 바꾸지 않았다.** 바뀐 것은 **"무엇을 관측하고 나서 끝내는가"** 뿐이다.

## Root cause

diagnosing-bugs Phases 1–4로 확정(재현율 **20/20**, 결정적 루프 3.4초).

`reconcile::run_once_at`은 패스 시작에 `collect_referenced()`로 **참조 sha 집합의
스냅샷**을 뜨고, 그 스냅샷을 기준으로 `.objects` 항목마다 2단계 tombstone GC를
집행한다(`.gc-pending.json`에 grace를 넘긴 항목이면 `remove_file`).

한편 `Store::put`의 **dedup 분기**는 기존 블롭이 온전하면(`if !intact`) **바이트를
다시 쓰지 않고** 커밋 포인터만 원자적으로 기록한다 — 즉 **기존 블롭에 새 참조를
추가하면서 그 블롭의 유일한 사본에 아무 흔적도 남기지 않는다.**

put은 `KeyLocks`(bucket/key 입도)를 잡지만 **reconcile은 어떤 락도 잡지 않는다.**
게다가 경합 자원은 **블롭(sha)**이지 키가 아니고, `reconcile::run_once(root: &Path, …)`는
경로만 받는 자유함수라 **구조적으로 그 락을 잡을 수 없다**(`main.rs`는 `Store`를 들고
있지도 않다).

→ **스냅샷 이후 참조를 얻은 블롭이 같은 패스 안에서 삭제된다.** 커밋 포인터만 남고
유일한 사본이 사라져 객체가 영구 non-servable이 된다.

**프로덕션 도달 가능**: `main.rs`가 reconcile을 백그라운드 주기로 돌리고, 블롭 루프는
매 블롭을 전량 재독·재해시하므로 실제 창은 **초~분** 단위. 트리거는 평범하다 — 객체를
지우거나 덮어쓴 뒤 grace가 지나고 **같은 내용을 재업로드**(CI 재시도, 동일 아티팩트 재푸시).

**진단이 실증적으로 기각한 두 가지**(둘 다 창을 닫지 못한다):
- "put이 항상 바이트를 재기록" — GC가 put의 기록 **이후**에 지우면 여전히 유실.
- "삭제 직전에 refs를 재확인" — 재확인과 `remove_file` 사이에 put이 커밋 가능. 여전히 TOCTOU.

## The fix

**무취소 커밋(uncancellable commit) + 착지 흔적(landed) + 무덤 코호트 정산(cohort settle).**

r1 이후의 개정 1차안은 "핀 + 되돌릴 수 있는 삭제"에 `arm()`(= "커밋을 **시도**했다"는 흔적)을
얹었다. 세 렌즈의 fatal은 전부 **같은 뿌리**를 가리켰다:

> **`arm()`은 흔적을 남기지만 흔적의 수명은 `PinGuard::drop`(취소 시 즉시 동기 실행)에 묶여 있고,
> 정작 커밋(`rename`)은 `spawn_blocking`이라 취소를 뚫고 착지한다.** 흔적과 커밋이 **다른 스레드에서
> 다른 시각에** 결정된다 — 이 비대칭이 crash 렌즈의 유실 시퀀스도, flip 렌즈의 ENOSPC 무한연기도 낳았다.

봉인은 그 비대칭을 없애는 것이다. **커밋 rename과 핀의 수명을 하나의 `spawn_blocking` 클로저 안에
가둔다.** 그러면 "시도"라는 불확실한 프록시가 필요 없고, **"착지(landed) = rename이 `Ok`를 반환했다"는
확정 사실**을 흔적으로 쓸 수 있다. 그 결과 P-2가 요구한 프로토콜 축소가 완성되고, crash 유실 창이 닫히며,
뮤턴트 표면(`armed` 맵·P3 시드·M3/M5 클래스)이 **통째로 소멸해 설계가 작아진다**.

**코드로 확인한 근거(전부 재확인함):**

| 사실 | 출처 |
|---|---|
| `tokio::fs::rename`은 `asyncify = spawn_blocking(f).await` — **퓨처를 드롭해도 blocking 클로저는 끝까지 실행된다** | `tokio-1.52.3/src/fs/mod.rs:312` |
| **"`spawn_blocking` tasks cannot be aborted once they start running… runtime shutdown will wait indefinitely for all started `spawn_blocking` to finish"** — 시작된 blocking 태스크는 **abort 불가**, `JoinHandle` 드롭은 detach일 뿐 | `tokio-1.52.3/src/task/blocking.rs:107-120` |
| 저장소는 이미 `spawn_blocking` + `std::fs` 관행을 쓴다 | `src/store/atomic.rs:16`(rename) · `:24`(fsync_dir) |
| 취소는 **상시 경로**다(가정이 아니다) | `src/http/internal/files.rs:87` `tokio::time::timeout(upload_timeout, put_stream_fut)` |

같은 사실이 **양날**이다 — 개정 1차안을 죽인 칼(취소 뒤에도 rename이 착지한다)이 최종안의 **봉인 도구**다
(시작된 커밋은 아무도 못 끊으므로 핀이 커밋보다 먼저 죽을 수 없다).

> ### ⚠ **그 칼의 세 번째 날 — r3/P-4** (이 표를 적어 놓고도 **보지 못한 것**)
>
> *"시작된 blocking 태스크는 **abort 불가**"*는 **유실 창을 닫아 주는 동시에, 대기의 상계를 파괴한다.**
> **아무도 그 클로저를 끊을 수 없다**는 것은 곧 **`PinGuard`가 영원히 살 수 있다**는 뜻이다 — `upload_timeout`은
> **호출자 퓨처를 드롭할 뿐** blocking 클로저를 죽이지 못한다. 멈춘 파일시스템 연산(NFS 정지 · EBS 열화 ·
> dm-thin 고갈) 하나면 **코호트가 영영 드레인되지 않고**, GC는 **무덤을 판 채 영구 정지**한다.
> r2안은 위 표에 이 인용을 **직접 적어 놓고** 상계를 `upload_timeout`으로 계산했다 — **자기 코드가 반증하는
> 주장**이었다. **무취소는 공짜가 아니다: 유실 창을 닫은 대가로 대기에 상계가 사라진다.**
> ⇒ **그것을 기다리는 쪽(GC)이 반드시 자기 벽시계 예산을 가져야 한다** → **`settle_timeout`**(§settle_timeout).

### r2 봉인의 핵심 통찰 — **무덤을 판 뒤에는 이미 안전하다**

r2의 P-2는 "**무덤 rename 시점에 살아있는 핀**이 있으면 복원한다"는 계약을 정면으로 때렸다. 그 시점에는 그 put의
**결말을 모른다** — put이 X를 관측하고, GC가 settle·복원하는 내내 살아있다가, 그 뒤 포인터 rename이 **실패**해
포인터를 하나도 만들지 않을 수 있다. 매 패스에 겹치는 실패가 반복되면 **회수가 무한정 연기**된다.

봉인의 열쇠는 **무덤 rename 이후의 비대칭**이다:

> **blob 이름(`.objects/<sha>`)이 치워진 뒤에 X를 보는 put은 `blob_intact`에서 ENOENT를 관측한다** → dedup 분기에
> 들어가지 못하고 **바이트를 스스로 재기록**한다(`write_atomic(blob_path)`). 즉 **무덤 이후에 생긴 핀은
> 자급자족(self-sufficient)이며, GC가 그 put을 기다릴 이유가 전혀 없다.**

그러므로 GC가 결말을 알아야 하는 put은 **무덤 rename 시점에 이미 살아 있던 핀들 — 고정되고 유한한 집합 — 뿐이다.**
`settle()`은 그 집합(**코호트**)이 **전부 종료(drop)될 때까지 기다린 다음** 판정한다. 기다림이 끝난 시점에는 모든
코호트 멤버의 **종료 결과(terminal outcome)가 확정**돼 있고, 그 결과는 **`landed(sha)`에 정확히 반영**돼 있다
(핵심 사실 A: 핀은 rename이 `Ok`/`Err`를 반환하고 마킹까지 끝난 **뒤에만** 죽는다).

> ⚠ **r3/P-4 — 이 문단의 "기다린다"는 유한하다.** 코호트가 **고정·유한 집합**이라는 것은 **유한 시간에
> 종료된다**는 뜻이 **아니다**: 핀은 **abort 불가능한 blocking 클로저**가 소유하므로(그게 무취소 커밋의 대가다)
> 멈춘 FS 연산이 멤버를 **영원히** 살릴 수 있다. 그래서 대기에는 **`settle_timeout`이라는 명시적 상계**가 있고,
> **`landed`가 확정되면 그 즉시 대기를 끊는다**(더 기다릴 이유가 없다). 상계를 넘기면 **fail-CLOSED**로 복원하고
> 패스를 정상 해제한다. §`settle_timeout` · §degraded-path 연기 · **P7**.

- **착지한 커밋이 하나라도 있으면** → 포인터가 VFS에 있다 → **복원**.
- **하나도 없으면** → 어떤 포인터도 존재하지 않는다 → **reap**(`gc_deleted += 1`, tombstone 제거 — **오늘과 동일**).

**결말을 알고 나서 판정하므로 유실도 없고 연기도 없다.** `live`는 **보호 술어의 지위를 잃고 대기 조건**이 된다.
대기하는 동안 무덤은 안전하게 보존되며(파괴 연산은 판정 이후에만 일어난다), 프로세스가 죽어도 무덤이 잔존해
다음 패스의 `recover_graves`가 복원한다.

### 1. `src/store/atomic.rs` — 커밋을 두 단계로 쪼갠다

> **r2 / P-3**: `RenameReceipt`는 **삭제됐다.** 범용 `rename_durable`이 **임의의 경로**에 대해 발급하는 unit 토큰은
> "blob→무덤 전이"에 **아무 것도 바인딩하지 못했다** — `pins.rs` 안의 코드가 **무관한 rename에서 영수증을 얻어**
> 전이 이전에 위험한 사전확인을 할 수 있었고, 그건 `atomic.rs`를 건드리지 않고도 **컴파일됐다**. 증거는 토큰이
> 아니라 **`Graved` 그 자체**여야 한다(§3).

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

### 2. `src/layout.rs` — 무덤 이름공간 (**P-1 봉인**)

```rust
const GRAVE_PREFIX: &str = ".gc-grave-";              // `.objects` 직속 **평면** 이름 (mkdir 0)
fn is_sha_name(s:&str)->bool { s.len()==64 && s.bytes().all(|b| b.is_ascii_hexdigit()) }
pub(crate) fn grave_sha(name:&str)->Option<&str> { name.strip_prefix(GRAVE_PREFIX).filter(|s| is_sha_name(s)) }
pub(crate) fn grave_name(sha:&str)->String { format!("{GRAVE_PREFIX}{sha}") }
impl Layout { pub(crate) fn grave_path(&self, sha:&str)->PathBuf { self.objects_dir().join(grave_name(sha)) } }

pub enum ObjectsEntry { Reserved, Temp, Blob, Grave, Other }   // payload 없는 Copy 유지
// classify_objects_entry: Reserved → Grave(grave_sha 재사용) → Temp(.tmp-) → Blob(64hex) → Other
```

이름이 **sha를 품는다** → 복구가 가능하다. `.tmp-` 접두가 아니다 → **temp로 오분류되지 않는다**
(P-1의 사인이 정확히 그것이었다: 원안의 `.tmp-<unique>` 무덤은 rename이 mtime을 보존하므로 다음 패스가
**만료 temp로 보고 즉시 지우고 `temps_deleted`로 세었다**). 분류 서로소성 · `segment_ok`가 `.` 시작 세그먼트를
금지(`layout.rs:19`) · `pointers_all`이 `.objects`를 스킵(`layout.rs:262`) — 3개 렌즈가 **반증에 실패**한 항목.

### 3. `src/store/pins.rs` (신규, crate-private)

```rust
//! ## 불변식
//! P1 `pin()`은 절대 블록하지 않는다(상호배제 0 — put은 GC를 기다리지 않는다).
//! P2 **보호 술어는 `landed` 하나뿐이다.**  (r2/P-2 봉인 — `live`는 술어가 **아니다**)
//!      landed(sha) = 이 패스 동안 **커밋 rename이 Ok를 반환한** sha (sticky)
//!      live(sha)   = 지금 존재하는 핀 = **결말이 아직 확정되지 않은** put → **대기 조건**이지 보호가 아니다
//!    GC 보호 술어: restore ⇔ landed(sha)   ← 코호트 대기가 끝난 **뒤에만** 평가된다
//! P3 **커밋은 취소 불가다.** PinGuard는 커밋 클로저가 **소유**하며, Drop은 rename·마킹·fsync가
//!    모두 끝난 뒤 그 클로저 안에서 실행된다 → "핀이 죽었는데 rename이 나중에 착지"는 **불가능**.
//!    ⇒ **핀의 죽음 = 그 put의 종료 결과(terminal outcome) 확정**이며, 결과는 landed에 이미 반영돼 있다.
//! P4 보호 판정 API는 **`Graved::settle(self)` 하나뿐**이다. `Graved`는 **`PassGuard::grave()`의
//!    blob→무덤 rename이 성공했을 때만** 태어나고(private 필드·같은 모듈 외 생성자 0·derive 0),
//!    자기 `sha`와 **무덤 시점 코호트**를 품는다 → 판정이 **그 전이·그 sha에** 바인딩된다.
//!    `BlobPins`에 sha로 조회하는 **공개 술어는 존재하지 않는다**(`protected()` 삭제 — r2/P-3 봉인).
//!    ⚠ **`pins.rs`가 밖으로 내보내는 것의 전부**(이 목록을 늘리면 봉인이 풀린다):
//!      `pub(crate)`: `BlobPins::{new, pin}` · `BlobPins::hooks() -> &Hooks`(**배리어 전용**) ·
//!                    `PinGuard::{blob_intact, commit_pointer}` ·
//!                    `PassGuard::{begin, referenced, recovered, pins, grave}` ·
//!                    `Graved::settle(self)` · `Settled` · `Hooks`
//!      **private (pins.rs 전용)**: `Inner`의 **모든 필드**(`next_id`/`live`/`landed`/`pass_live`) ·
//!                    `cohort_at_grave` · `await_settlement` · `Settlement` · `landed` · `enter_pass`
//!    ⇒ `reconcile.rs`는 **훅과 `grave()`만** 볼 수 있고, **보호 상태는 읽을 수단이 아예 없다**
//!      → 사전확인 뮤턴트는 `reconcile.rs`에서 **표현 불가**다(`pins.rs`를 편집해야만 가능 — §정직한 경계).
//! P5 pass_live 플래그는 `PassGuard`(Drop 보유)가 **fallible op 이전에** 획득한다 → `?` 누수 0.
//! P6 핀에는 **단조 증가 id**가 붙는다. 무덤 rename **직후** 그 sha의 live id를 스냅샷한 것이 **코호트**다.
//!    코호트는 **고정·유한**하며, 무덤 **이후**에 생긴 핀은 코호트에 **들어오지 않는다**
//!    (그 put은 blob_path에서 ENOENT를 보고 바이트를 재기록한다 → **자급자족** → 기다릴 이유가 없다).
//! P7 **대기는 유한하며 fail-CLOSED다.** (r3/P-4 봉인)
//!    코호트가 **고정·유한**하다는 것과 **유한 시간에 종료된다**는 것은 **다른 명제다.** `PinGuard`는
//!    **abort 불가능한 `spawn_blocking` 클로저가 소유**하므로(P3 — 그게 무취소 커밋의 대가다), 멈춘
//!    파일시스템 연산은 코호트 멤버를 **영원히 살려 둘 수 있다**. `upload_timeout`은 **호출자 퓨처를 드롭할 뿐**
//!    blocking 클로저를 **죽이지 못한다** → **`upload_timeout`은 대기의 상계가 아니다**(r2안의 거짓 주장).
//!    ⇒ `settle()`은 다음 셋 중 **먼저 오는 것**에서 깨어난다(무한 대기 **불가**):
//!      (a) **`landed(sha)` 확정** → 보호가 확정이므로 **나머지 코호트를 기다리지 않는다**(대기 0 · 즉시 복원)
//!      (b) **코호트 드레인** → 모든 멤버의 종료 결과 확정 → `landed`를 읽어 판정
//!      (c) **`settle_timeout` 소진** → **fail-CLOSED**: **무덤을 정본으로 복원**(데이터 보존 우선) ·
//!          tombstone **유지**(D-2) · `gc_deleted` **무증가** · `tracing::error!` · **패스는 정상 해제**
//!    `settled: Notify`는 **핀 drop**과 **`landed` 삽입** **양쪽에서** 울린다 → (a)가 **즉시** 발화한다.

#[derive(Clone, Default)]
pub(crate) struct BlobPins {
    inner: Arc<Mutex<Inner>>,                 // 동기 Mutex — 임계구역이 await를 걸치지 않는다
    settled: Arc<tokio::sync::Notify>,        // **두 곳에서** 울린다(r3/P-4):
                                              //   ① PinGuard::drop      → 코호트 드레인 진행
                                              //   ② landed 삽입(신규)   → **보호 확정 → 즉시 깨움**
    pass_lock: Arc<tokio::sync::Mutex<()>>,   // 프로세스 내 라이브 패스 ≤ 1
    hooks: Hooks,                             // 결정적 배리어. prod = 전부 None (§증분 분해)
}
#[derive(Default)]
struct Inner {
    next_id: u64,                             // 단조 증가 핀 id (P6)
    live: HashMap<String, HashSet<u64>>,      // sha → 살아있는 핀 id 집합  (usize 카운터 → id 집합)
    landed: HashSet<String>,                  // 커밋 rename이 Ok를 반환한 sha (sticky, 패스 스코프)
    pass_live: bool,
}
// ※ `armed` 맵도, `touched := armed 스냅샷` 시드도 **없다**(§Single-Flip Contract에서 불필요함을 증명).

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

    /// **유한 대기**(r3/P-4). 셋 중 **먼저 오는 것**에서 깨어난다 — 무한 대기가 **표현 불가**하다.
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
                //    → 더 기다리는 것은 **순손해**다(그 객체가 그동안 404다 — r3/P-4의 후반부 스톨).
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
            me.pins.hooks.in_commit_pre_rename(&me.sha);             // 동기 훅(T-C2 · **T-P4a**)
            staged.commit_blocking(|| {                              // ← rename Ok 직후에만
                let landed_now = {
                    let mut g = me.pins.inner.lock().unwrap();
                    // **착지 흔적**. `insert`의 반환값 = 이번에 처음 들어갔는가(전이에서만 깨운다)
                    g.pass_live && g.landed.insert(me.sha.clone())
                };  // ← 락을 **먼저 놓고** 깨운다(PinGuard::drop과 같은 규율)
                // **r3/P-4 ①**: 보호가 **확정**됐다 → 코호트 대기 중인 settle()을 **즉시** 깨운다.
                // 이게 없으면 settle은 나머지 코호트가 죽을 때까지(또는 타임아웃까지) 기다리고,
                // **그 창 내내 실재하는 포인터가 404다**(= Codex P-4의 "rename 이후 스톨" 시나리오).
                // notify_waiters()는 동기·논블로킹이고 런타임 컨텍스트를 요구하지 않는다
                // → blocking 클로저 안에서 안전하다(PinGuard::drop이 이미 같은 호출을 한다).
                if landed_now { me.pins.settled.notify_waiters(); }
                me.pins.hooks.in_commit_post_landed(&me.sha);        // 동기 훅(**T-P4b-1 · T-P4b-2**) — rename **이후**
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
```

> ⚠ **`in_commit_post_landed`는 r3/P-4 개정이 추가한 유일한 `Hooks` 필드다**(prod = `None`). Codex가 **명시 요구**한
> T-P4b("포인터 rename **이후**에 멈춘 핀")의 park 지점이 **rename `Ok` ∧ `landed` 삽입 이후 ∧ 핀 drop 이전**
> 이어야만 하는데, 기존 훅에는 그 지점이 **없다**(`in_commit_pre_rename`은 rename **이전**이다).
> **r4 개정은 훅을 하나도 더 늘리지 않는다** — T-P4b-1/T-P4b-2는 **이미 있는 훅 7개**(`pre_grave` ·
> `post_grave` · `in_commit_pre_rename` · `in_commit_post_landed` …)만으로 짜인다. `notify_waiters()`가
> **`in_commit_post_landed` 훅보다 먼저** 호출된다는 이 순서가 **T-P4b-2의 load-bearing 지점**이다:
> 알림이 나간 **뒤에** 클로저가 park하므로, **핀이 살아있는 채로** settlement가 깨어나는지를 관측할 수 있다.
> **`atomic.rs`는 건드리지 않는다** — 훅은 `pins.rs`가 이미 넘기는 `on_landed` 클로저 **안에서** 호출되므로
> `Staged::commit_blocking(self, on_landed: impl FnOnce())` 시그니처는 **불변**이다.

```rust
pub(crate) struct PassGuard { pins: BlobPins, _pass: OwnedMutexGuard<()>, layout: Layout,
                              refs: HashSet<String>, recovered: usize,
                              settle_timeout: Duration }   // r3/P-4 — 호출자가 **명시**한다

impl PassGuard {
    /// **패스 순서의 유일한 소유자.** P5: 플래그를 든 가드를 **fallible op 이전에** 만든다.
    /// `settle_timeout`은 **주입**된다(기본값 없음) → 테스트가 짧은 값을 넣어 degraded 경로를 **결정적으로**
    /// 친다(T-P4a). prod 값은 `main.rs`가 `cfg.upload_timeout_secs`에서 파생한다(§settle_timeout).
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
    /// **유한·fail-CLOSED**(P7 / r3-P-4): 멈춘 핀 하나가 GC를 **영구 정지시킬 수 없다**.
    pub(crate) async fn settle(self) -> io::Result<Settled> {
        let began = tokio::time::Instant::now();

        // ① **결말을 기다린다 — 단, 유한하게.** 무덤은 그동안 안전하게 보존된다(파괴 연산은 ③에서만).
        //    · landed 확정 → **대기 0**(이미 true였거나, 삽입이 즉시 깨운다)
        //    · 코호트 드레인 → 결말 확정
        //    · settle_timeout 소진 → TimedOut
        let outcome = self.pass.pins
            .await_settlement(&self.sha, &self.cohort, self.pass.settle_timeout).await;

        let (g, b, o) = (self.pass.layout.grave_path(&self.sha),
                         self.pass.layout.blob_path(&self.sha),
                         self.pass.layout.objects_dir());

        // ② **판정.** 보호 술어는 여전히 `landed` 하나뿐이다(P2 — r2 봉인 불변).
        let (protect, verdict) = match outcome {
            // 보호 확정. (코호트 잔여 멤버의 결말은 판정을 바꿀 수 없다 — landed는 sticky·단일 술어)
            Settlement::Landed  => (true, Settled::Restored),
            // 결말을 **알고 나서** 판정한다(r2/P-2 봉인의 핵심 — 그대로 유지).
            Settlement::Drained => match self.pass.pins.landed(&self.sha) {
                true  => (true,  Settled::Restored),
                false => (false, Settled::Reaped),          // ← 실패·취소·ENOSPC put: 오늘과 동일하게 회수
            },
            // **fail-CLOSED.** 결말을 알아내지 **못했다** → 보호 여부를 알 수 없다 → **보존을 택한다**.
            // 무덤을 정본으로 되돌리고, tombstone은 **유지**(D-2) → 다음 패스가 **새 스냅샷으로 재판정**한다.
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

**`settle()`이 반환하면 `Graved`는 소비되고, `PassGuard`는 `run_once_at`의 끝에서 drop된다 → `pass_lock` 해제.**
멈춘 핀은 `settle()`을 **한 번** 지연시킬 뿐, 그 패스의 **나머지 blob 회수를 막지 못하고**(루프가 계속 돈다)
**이후 패스를 막지도 못한다**(락이 풀린다). 이것이 P-4의 "`pass_lock`이 이후의 모든 복구·GC 패스를 막는다"에 대한
직접적인 봉인이며, **T-P4a의 3번 단언**(후속 `run_once`가 **완료**된다)이 그 기계 증인이다.

### `settle_timeout` — 상계를 무엇으로 잡는가 (**그리고 왜 `upload_timeout`이 아닌가**)

```rust
// src/store/reconcile.rs — pub(라이브러리 밖 main.rs가 쓴다)
/// 무취소 커밋 **꼬리**의 여유분. 이 꼬리는 `commit_pointer`의 blocking 클로저가 rename 전후로 수행하는
/// **고정 크기 작업**이다: mkdir_p + create + write_all(**메타 JSON 수백 바이트**) + sync_all(file)
/// + rename + sync_all(parent). 업로드 **크기에 비례하지 않는다** → 여유분은 **상수**가 맞다(비율 아님).
/// 건강한 디스크에서 한 자릿수 ms · blocking 풀이 대형 스크럽으로 포화돼도 1초 미만.
/// **60초 = 그 위로 두 자릿수 배의 헤드룸**이다.
pub const GC_SETTLE_MARGIN: Duration = Duration::from_secs(60);

/// **명시적 상계.** `upload_timeout`에서 **파생**하되 — ⚠ **`upload_timeout`은 상계가 아니다**(아래).
pub fn settle_timeout_from(upload_timeout: Duration) -> Duration { upload_timeout + GC_SETTLE_MARGIN }
```

`main.rs`가 `cfg`에서 계산해 **주입**한다(`settle_timeout_from(Duration::from_secs(cfg.upload_timeout_secs))`
→ 기본값 `600s + 60s = 660s`). `run_once`/`run_once_at`의 **명시 인자**이므로 테스트는 짧은 값(200ms)을 넣어
degraded 경로를 **결정적으로** 친다.

> #### ⚠ **왜 `upload_timeout`이 상계가 아닌가 — r2안이 여기서 거짓말을 했다**
>
> r2안은 "각 코호트 멤버의 수명은 `tokio::time::timeout(upload_timeout, …)`(`files.rs:89`)로 **잘리며**,
> `cfg.validate()`가 `upload_timeout < gc_grace`를 강제하므로 **sha당 상계 < `gc_grace`**"라고 적었다.
> **그 진술은 틀렸고, 틀린 이유가 바로 이 설계의 핵심 도구다.**
>
> `tokio::time::timeout`은 **호출자 퓨처를 드롭할 뿐**이다. 그런데 이 설계는 **의도적으로** `PinGuard`를
> **abort 불가능한 `spawn_blocking` 클로저 안으로 옮겼다**(핵심 사실 A — 그게 crash 렌즈의 유실 창을 닫은
> 물건이다). tokio 문서 그대로: *"`spawn_blocking` tasks cannot be aborted once they start running"*
> (`tokio-1.52.3/src/task/blocking.rs:107-120`). ⇒ **`upload_timeout`이 발화해도 커밋 클로저는 계속 돈다.**
> 큐에 걸리거나 **멈춘 파일시스템 연산**(NFS 정지 · EBS 열화 · dm-thin 고갈)은 그 클로저를 — 따라서 `PinGuard`를 —
> **무한정 살려 둔다**. **무취소 커밋의 대가가 정확히 이것이다.**
>
> **그러므로 상계는 `upload_timeout`에서 *파생*될 뿐 그것이 *강제*하는 것이 아니다.** `settle_timeout`은
> **GC가 스스로 거는 벽시계 예산**이며, 그것만이 유일한 상계다. 그 위에 `GC_SETTLE_MARGIN`을 얹는 이유는
> **`upload_timeout`이 코호트 멤버의 *취소 가능한* 부분(= blob 재기록 · `blob_intact`의 재해시, 최대
> `max_file_bytes`)을 실제로 자르기 때문**이다 — 그 부분은 잘리고 나면 핀이 **즉시** drop된다(가드는 호출자
> 퓨처가 들고 있다). 남는 것은 무취소 **꼬리**뿐이고, 마진은 그 꼬리를 덮는다.
> ⇒ **정상적으로 진행 중인 put은 절대 타임아웃되지 않는다**(그래야 정상 경로의 연기가 0으로 유지된다 — P-2 봉인
> 불변). 타임아웃은 **파일시스템 연산이 돌아오지 않을 때에만** 발화한다.
>
> **`settle_timeout` vs `gc_grace`**: 기본값은 `660s ≪ 3600s`이다. 운영자가 `upload_timeout`을 `gc_grace`
> 바로 밑까지(예: 3599s) 올리면 `settle_timeout`이 `reconcile_interval`(= `gc_grace`)을 넘을 수 있다 —
> 그래도 **안전하다**: `pass_lock`이 라이브 패스 ≤ 1을 강제하고 `MissedTickBehavior::Skip`(`main.rs:42`)이
> 틱을 쌓지 않는다 → 패스는 **밀릴 뿐 겹치지 않는다**. **클램프하지 않는다** — 클램프하면 정상적으로 느린
> put이 타임아웃돼 정상 경로에 연기가 생긴다(P-2 재발). §남은 위험 12에 명시.

**대기의 상계 (`settle()`이 await를 하게 된 대가 — 정직하게. ⚠ r3/P-4로 전면 개정)**

| 질문 | 답 |
|---|---|
| 대기가 무한정 늘어날 수 있나? | **없다 — 그러나 r2안의 이유는 거짓이었다.** 코호트가 **고정·유한 집합**인 것은 맞지만(그 뒤 생긴 핀은 무시 — 자급자족), **유한 집합이 유한 시간에 종료된다는 보장은 없다.** 핀은 **abort 불가능한 blocking 클로저**가 소유하므로 멈춘 FS 연산이 멤버를 **영원히** 살릴 수 있다. **무한 대기를 실제로 막는 것은 `settle_timeout` 하나뿐이다**(P7) |
| 한 sha의 상계는? | **`settle_timeout`**(기본 `upload_timeout + 60s` = `660s`). ⚠ **`upload_timeout`이 아니다** — 그것은 **호출자 퓨처를 드롭할 뿐** blocking 클로저를 죽이지 못한다(`blocking.rs:107-120`). r2안의 "**상계 < `gc_grace`**"는 **거짓 주장이었고 r3/P-4가 그것을 깼다.** §`settle_timeout` 참조 |
| 그 상계에 **실제로** 도달하나? | **정상 경로에서는 절대 도달하지 않는다.** 도달하려면 코호트 멤버의 **파일시스템 연산이 `settle_timeout` 동안 돌아오지 않아야** 한다(정상적으로 느린 put은 `upload_timeout`에 잘리고 **핀이 즉시 drop**된다 — 가드는 호출자 퓨처 소유). 도달 = **병리적 스톨**이며 `tracing::error!`가 뜬다 |
| `landed`가 이미 true면? | **대기 0.** 보호가 확정이므로 코호트를 기다리지 않고 **즉시 복원**한다(P7 (a)). 증인: **T-P4b-1** |
| 대기 **도중에** 착지하면? | **그 즉시 깨어난다.** `Notify`를 **`landed` 삽입에서도** 울리기 때문이다 → **실재하는 포인터가 404인 창을 최소화**한다(= P-4의 "rename 이후 스톨"). 증인: **T-P4b-2**. ⚠ **정직하게**: 이 알림은 **안전이 아니라 지연(latency)** 장치다 — 없어도 settlement는 결국 **핀 drop** 또는 **`settle_timeout`**에서 깨어나 **복원**한다(**유실 0**, 판정 동일). 없앨 때 **바뀌는 것은 404 창의 길이뿐**이다. **T-P4b-2는 바로 그 창을 관측 가능하게 만들어 뮤턴트를 죽인다**(핀을 착지 **이후에도** park해 두어 drop-알림이라는 **대체 기상 수단을 제거**한다) — r3 개정에서 *"죽이는 테스트가 없다"*고 분류했던 것이 **r4에서 죽는다** |
| 한 패스의 상계는? | GC 루프는 **순차**다 → 최악은 `N_stalled × settle_timeout` + 나머지 blob의 정상 처리 시간. **정직히**: 병리적으로 여러 sha가 동시에 스톨하면 패스가 길어진다. **그러나 패스는 반드시 끝나고**(각 항이 유한) **락은 반드시 풀린다** — 이것이 r2안과의 결정적 차이다(r2안: **끝나지 않는다**) |
| 실제로는? | 코호트가 **비어 있지 않으려면** 바로 그 sha를 **동시에 dedup-put 중**이어야 한다. 정상 스크럽에서 코호트는 비고 → `await_settlement`가 **첫 검사에서 `Drained` 반환**(await 0회) → **오늘과 동일한 실행시간** |
| 패스가 길어지면 다음 패스는? | `pass_lock`이 **프로세스 내 라이브 패스 ≤ 1**을 강제한다 → 패스는 **겹치지 않고 밀릴 뿐** 쌓이지 않는다(`MissedTickBehavior::Skip`, `main.rs:42`). GC는 백그라운드 스크럽이라 감당 가능하다 |
| 멈춘 핀이 **GC를 영구 정지**시키나? | **아니다 — 이것이 P-4 봉인의 요점이다.** 타임아웃 → 복원 → `settle()` 반환 → 루프가 **다음 blob으로 진행**(같은 패스의 다른 blob은 정상 회수된다) → `PassGuard` drop → **`pass_lock` 해제** → 다음 패스·복구 패스가 **정상 실행**된다. 증인: **T-P4a** |
| 대기 중 프로세스가 죽으면? | 무덤 잔존(디스크 상태 **(B)**) → 다음 패스의 `recover_graves`가 `rename(grave → blob)` → **안전**. §크래시 논증을 **그대로 재사용**한다(새 논증이 필요 없다) |
| 대기 중 reconcile 퓨처가 취소되면? | 동일 — `Graved`에 파괴적 Drop이 없다 → 무덤 잔존 → 다음 패스 복구. **unlink는 한 번도 일어나지 않았다** |
| put이 GC를 기다리게 되나? | **아니다.** 대기는 **GC → put** 단방향이다. `pin()`은 여전히 무대기(P1)이고 put은 `pass_lock`을 **잡지 않는다** → 순환 대기 불가 → **데드락 불가**. 기각된 "블롭 락"으로 되돌아가지 **않는다** |

> 최종안에는 `Graved::sift_corrupt`(+ `enum Sifted`)가 하나 더 있었다 — 격리 분기의 유실 레이스(F4)를
> 봉인하는 물건이다. **컨덕터 판정 D-4로 이 픽스에서 제외**했다(두 번째 관측 행동 플립 → 하드룰 10).
> 설계 전문은 **F-25에 청사진으로 보존**한다. `pins.rs`는 `sift_corrupt`/`Sifted` **없이** 착지한다.

### 4. `src/store/objects.rs` — pin → blob_intact → commit_pointer

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

`put_stream` 동형: `stream_to_temp` 반환 직후(sha 확정) `pin()` → 기존 `existing_intact` 표현식을
`pin.blob_intact(&self.layout).await`로 치환 → temp→blob rename 분기 전체가 핀 아래 → 마지막
`write_atomic(meta_target)`을 `pin.commit_pointer(...)`로 치환. **스트리밍 본문은 취소 가능한 채로 남는다**
(`upload_timeout` 예산 불변). 무취소가 되는 것은 **메타 커밋(수백 바이트)뿐**이다.

### 5. `src/store/mod.rs` / `src/main.rs`

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

`main.rs`: `cfg`가 `build_state`로 **move되기 전에** `settle_timeout`을 계산(오늘 `gc_grace`를 그렇게 뽑는 것과
**동형**) → `build_state`를 **먼저**(그것이 `.objects`를 만든다) → 부트
`reconcile::run_once(&state.store, gc_grace, settle_timeout)` → 주기 루프는 `state.store.clone()`을 move
(**같은 Arc 등록부**).

```rust
let gc_grace = Duration::from_secs(cfg.gc_grace_secs);
let reconcile_interval = Duration::from_secs(cfg.gc_grace_secs);           // 기존 그대로
// r3/P-4 — **유일한 상계**. cfg가 move되기 전에 뽑는다(gc_grace와 동형).
let settle_timeout = reconcile::settle_timeout_from(Duration::from_secs(cfg.upload_timeout_secs));

let state = http::build_state(cfg)?;                                        // ← 여기서 cfg가 move된다
```

> **왜 `Config`에 새 env 노브를 안 만드는가**(`FILES_GC_SETTLE_TIMEOUT`): `bugfix-lock.json`의 `scope`는
> `["src/store/**", "src/main.rs", "src/layout.rs"]`이고 이 개정은 거기에 **아무것도 더하지 않는다**.
> `src/config.rs`(+ `validate()` 규칙 + `src/http/state.rs` 배선)를 열면 **설계가 커지고**(새 env·새 불변식·
> 새 검증 테스트) 컨덕터의 "국소 수정만" 제약을 깬다. **`upload_timeout`에서의 파생은 순수 함수**이고
> `main.rs`(**scope 안**)가 그것을 호출한다 → 노브는 **`FILES_UPLOAD_TIMEOUT` 하나로 유지**되며 운영자가
> 그것을 올리면 `settle_timeout`이 **자동으로 따라 올라간다**. 독립 노브가 필요해지면 → **F-29**.

### 6. `src/store/reconcile.rs`

```rust
/// ⚠ r3/P-4: `settle_timeout`은 **명시 인자**다. 기본값을 숨기지 않는다 — 이 값이 **유일한 상계**이므로
///    호출자가 그것을 **알고 정해야** 한다. prod = `settle_timeout_from(cfg.upload_timeout)`(§settle_timeout).
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

GC 루프 본문(변경분만):

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
            //    F4 유실 레이스는 **미봉인**으로 남는다(F-25). §Preserved Contract·§남은 위험 참조.
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
                //   sha로 물어볼 수 있는 보호 술어가 **존재하지 않는다**(`protected()` 삭제).
                match pass.grave(&name).await?.settle().await? {
                    Settled::Reaped   => { pending.remove(&name); stats.gc_deleted += 1; }
                    Settled::Restored => { /* D-2: tombstone 유지, 무카운트 */
                                           tracing::info!(sha=%name, "GC restored: landed commit"); }
                    // r3/P-4 — **degraded 경로**. 무덤은 이미 정본으로 복원됐다(데이터 보존).
                    // tombstone **유지**(D-2) → 다음 패스가 **새 스냅샷으로 재판정**한다.
                    // `gc_deleted` **무증가**. 에러 로그는 `settle()`이 이미 냈다(중복 로깅 금지).
                    // ⚠ **`?`로 패스를 중단하지 않는다** — 멈춘 핀 **하나**가 다른 blob들의 GC를
                    //    막으면 안 된다(그게 P-4의 병이다). 루프는 **계속 돈다**. §에러 표면 논증.
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

`ReconcileStats`는 **필드 한 개도 늘리지 않는다**(복구·복원·**연기**는 tracing만) — 골든 전수 `assert_eq!` 보존.

#### 에러 표면 — **로그 + 계속**(중단 아님)이 B7을 깨지 않는 이유

**선택**: 타임아웃은 **`io::Result`로 전파하지 않는다.** `tracing::error!` + `Settled::Deferred` + **루프 계속**.

1. **중단은 P-4를 다른 자리로 옮길 뿐이다.** `settle` 타임아웃을 `Err`로 올리면 GC 루프의 `?`가 **패스 전체를
   중단**시킨다 → **멈춘 핀 하나가 나머지 모든 blob의 회수를 막는다.** 이는 P-4가 지적한 병(`pass_lock`이 모든
   후속 패스를 막는다)을 **`?`로 갈아입힌 것**에 지나지 않는다. 봉인의 목표는 **격리**다: 병든 blob 하나만 연기하고,
   **나머지는 오늘과 똑같이 회수**한다.
2. **B7("reconcile은 `io::Result`를 무가공 전파")을 깨지 않는다.** B7이 규율하는 것은 **`io::Error`를 어떻게
   surface하는가**다 — *가공하지 말고, 삼키지 말고, 그대로 `?`로 올려라*. 타임아웃은 **`io::Error`가 아니다**:
   **어떤 syscall도 실패하지 않았다.** 이걸 `io::Error::new(ErrorKind::TimedOut, …)`로 **합성**하는 것이야말로
   B7이 금지하는 **가공**이다 — 커널이 낸 적 없는 에러를 발명해 io 표면에 얹는 짓이다.
   ⇒ **이 개정은 `io::Error`를 하나도 새로 만들지 않고 하나도 삼키지 않는다.** `settle()` 안의 **진짜** io 실패
   (복원 `rename`의 EIO/ENOSPC · `remove_file` · `fsync_dir` · `restore_io` 주입)는 **전부 `?`로 무가공 전파**되며
   그 행동은 **개정 전과 바이트 동일**하다(§복원 실패).
3. **선례가 이미 있다.** `collect_referenced`의 정책은 *"워커가 낸 포인터의 read/파싱 실패는 **조용히 skip**(B7)"*
   (`reconcile.rs:27`)이다 — B7 아래에서 **패스를 죽이지 않는 국소 결정**은 이미 계약의 일부다. 우리 쪽은 그보다
   **엄격하다**: 조용하지 않고 **`tracing::error!`로 시끄럽다**.
4. **상위에서 달라지는 게 없다.** `main.rs`는 `run_once`의 `Err`를 **`warn!`만 하고 다음 틱으로 넘어간다**
   (`main.rs:49`). 즉 중단을 택해도 **운영자에게 더 잘 보이지 않고**, 그 패스의 남은 blob 회수만 잃는다.
   `tracing::error!`는 `warn!`보다 **더 높은 레벨**로, **sha·cohort_size·waited_ms**까지 실어 나른다.

> **⚠ 왜 `ReconcileStats` 필드가 아닌가**(예: `deferred: usize`): `tests/layout_tree.rs:71,137,198`이 **구조체
> 전수 `assert_eq!`**로 stats를 핀한다 → 필드를 하나라도 늘리면 **그 3개가 깨진다 = 두 번째 관측 행동 플립**
> (하드룰 10). 관측성은 **tracing으로만** 낸다 — 이 제약은 `recovered`/`restored`에 이미 적용된 것과 **동일한
> 규율**이며 이번에도 예외를 두지 않는다. 연기 카운트가 필요하면 **후속**에서 stats 계약을 여는 파이프라인으로
> 간다(→ **F-29**).

### 왜 락 계열이 아닌가 (기각 근거 — 유지)

블롭 락(sha 뮤텍스) 설계들은 GC가 락을 **전 포인터 트리 워크 내내** 쥐고, put은 그 락을 **KeyLocks를 쥔 채**
기다린다. 그 대기가 `upload_timeout` 예산(`src/http/internal/files.rs:89`)과 `cap.reserve` 전역 누산(`:82`)에
그대로 계상되어 부하 하에서 **400 `upload_timeout` / 무관한 업로드의 507**이 새로 발생한다 — characterization
105개가 **절대 못 잡는 두 번째 관측 행동**이며 하드룰 10 위반이다. 승자는 **put이 블록되지 않으므로 그 표면이
존재하지 않는다**(P1).

## 왜 창이 닫히는가

**핵심 사실 A (무취소).** `commit_pointer`의 클로저가 `PinGuard`를 **소유**한다. tokio에서 **시작된 blocking
태스크는 abort 불가**(`tokio-1.52.3/src/task/blocking.rs:107-120`)이고 `JoinHandle` 드롭은 detach일 뿐이다. 따라서:

> **핀 id가 `live[sha]`에서 사라지는 시점 = stage 실패로 rename에 도달조차 못 했거나, rename이 `Err`를
> 반환했거나, rename이 `Ok`를 반환하고 `landed`가 이미 삽입된 이후.**
> 즉 **핀의 죽음 = 그 put의 종료 결과(terminal outcome) 확정**이며, 그 결과는 **이미 `landed`에 반영돼 있다.**
> "핀이 죽었는데 rename이 나중에 착지한다"는 crash 렌즈의 유실 시퀀스 3단계가 **표현 불가능**해졌다.
> **이것이 코호트 대기가 성립하는 이유다** — 기다림이 끝나면 결말을 **안다**.

**핵심 사실 B (happens-before).** `landed` 삽입, `pass_live=true`(=`enter_pass`), **코호트 스냅샷**, **코호트
drain 검사**, `settle`의 `landed` read는 **모두 같은 `Mutex<Inner>` 임계구역**이다 → 전순서가 존재한다.

**핵심 사실 C (rename Ok ⇒ 즉시 가시).** POSIX rename이 `Ok`를 반환하면 디렉터리 엔트리가 VFS에 존재한다
(fsync는 *내구성*이지 *가시성*이 아니다). 부모 버킷 디렉터리는 `stage_blocking`의 `mkdir_p`가 rename 이전에
만든다 → `pointers_all`의 `SeedRoot`(`layout.rs:257-274`)가 루트를 readdir할 때 그 버킷이 보인다.

**핵심 사실 D (무덤 이후의 자급자족).** R = blob→무덤 rename이 `Ok`를 반환한 사건이라 하자. `settle`의 파괴 연산은
`remove_file(grave_path)` **하나뿐**이고 — **무덤 이름만** 지운다 — R 이후에 `pin()`한 put은 `blob_intact`에서
`blob_path(X)`를 읽는데 **그 이름은 더 이상 무덤 inode를 가리키지 않는다.** 따라서 그 put은 (a) **ENOENT**를 보고
dedup 분기에 못 들어가 **바이트를 재기록**하거나, (b) 다른 post-R put이 이미 재기록해 둔 **치유본**을 dedup한다.
**어느 쪽이든 무덤 inode에 의존하지 않는다** → Reap이 그 blob을 건드릴 수 없고, Restore가 일어나도
`rename(grave → <sha>)`는 **내용주소 동일 바이트**로 덮어쓸 뿐이다.
⇒ **post-R 핀은 자급자족이며, GC가 기다릴 이유가 없다.** (이것이 코호트를 R 시점으로 **닫는** 근거다.)

### 정상 — 완전성 정리 (유실 불가 ∧ **연기 불가**)

X를 무참조·만료 blob, **R**을 `grave(X)`의 rename Ok(= **코호트 스냅샷 시각**), **W**를 코호트가 **전부 drain된**
시각, **S**를 `settle`의 `landed` read라 하자. 구성상 **R ≺ W ≺ S**이고, B에 의해 S는 전 사건과 비교 가능하다.
커밋 포인터 → X를 남기는 **모든** put P에 대해, P의 커밋 rename이 `Ok`를 반환한 사건을 M(= `landed` 삽입
임계구역)이라 하면:

> **⚠ r3/P-4 — 이 정리의 전제**: 아래 표는 `settle`이 **`Drained`로 깨어난 경우**(= `W`가 실제로 존재하는 경우)의
> 정리다. `settle`은 이제 **세 가지로 깨어난다**(P7). 나머지 둘은 이 정리에 **흡수되거나 이 정리를 필요로 하지
> 않는다**:
> - **`Landed`** — `landed(X)`가 참임을 **직접 관측**했다. 케이스 **(2)**와 판정이 같다(**Restore**). C에 의해
>   포인터는 이미 VFS에 있다 → **복원이 정답**이고, 나머지 코호트의 결말은 **판정을 바꿀 수 없다**(`landed`는
>   sticky이고 **유일한 보호 술어**다). 더 기다리는 것은 **그 객체를 404로 두는 순손해**일 뿐이다.
> - **`TimedOut`** — `W`가 **존재하지 않는다**(핀이 안 죽는다). 정리를 **적용하지 않는다.** 대신 **fail-CLOSED**:
>   무조건 **복원**한다. ⇒ **유실 불가는 자명하다** — `settle`의 파괴 연산(`remove_file(grave)`)이 **실행되지
>   않기 때문**이다. 대가는 **회수 연기**이며 §degraded-path 연기에서 특성화한다.
>
> ⇒ **유실 불가는 세 경우 모두에서 성립한다.** 아래 표가 증명하는 것은 **`Drained` 경우의 유실 불가 ∧ 연기 불가**다.

| # | 순서 | 결과 |
|---|---|---|
| **(1)** | **M ≺ enter_pass** | C에 의해 포인터는 `collect_referenced`의 readdir 이전에 이미 VFS에 있다 → `refs ∋ X` → GC는 **삭제 분기에 진입조차 안 한다** |
| **(2)** | **enter_pass ≺ M ≺ S** | `pass_live=true`였으므로 `landed ∋ X` → S가 본다 → **Restore** (M이 R 이전이든 이후든 무관 — `landed`는 sticky다) |
| **(3a)** | **S ≺ M** ∧ P의 핀 **∈ 코호트** | **이 칸은 비어 있다.** W ≺ S이므로 S 시점에 P의 핀은 **이미 죽었다**. A에 의해 핀의 죽음 ⇒ P의 rename이 **`Ok`를 반환했거나**(⇒ M ≺ S — 가정 `S ≺ M`과 **모순**) **`Err`를 반환했다**(⇒ M이 영원히 없다 ⇒ **커밋 포인터 부재** — "P가 포인터를 남긴다"는 전제와 **모순**). ⇒ **모순** |
| **(3b)** | **S ≺ M** ∧ P의 핀 **∉ 코호트** | P는 **R 이후에 pin**했다 ⇒ **D**에 의해 P는 무덤 inode에 의존하지 않는다. Reap은 무덤 이름만 지우므로 P의 blob(`<sha>`)은 **살아남는다** → **유실 0** |
| **(4)** | **M이 영원히 없다** | rename이 `Ok`를 반환한 적이 없다 ⇒ (rename은 원자적) **커밋 포인터가 존재하지 않는다** ⇒ 회수해도 참조하는 포인터가 없다 → **정당한 Reap**, `gc_deleted += 1` |

(1)·(2)·(3a)·(3b)·(4)가 전부다(mutex 전순서 + rename 원자성 + D). **유실 시퀀스는 존재하지 않는다.**

**그리고 — r2/P-2가 요구한 것 — 연기(deferral) 시퀀스도 존재하지 않는다.** 결말이 (4)인 put(실패·취소·ENOSPC·
rename `Err`)은 `landed`에 **흔적을 남기지 않는다**. `settle`은 코호트가 죽을 때까지 **기다린 다음** 판정하므로
그런 put은 X를 보호하지 **못하고**, X는 **바로 그 패스에서** Reap된다(`gc_deleted += 1`). 겹치는 실패가 매 패스
반복돼도 **매 패스 회수된다** — 연기가 누적될 자리가 **구조적으로 없다**.

> **개정 전 계약**(`restore ⇔ live ∨ landed`)에서는 바로 이 put이 **`live` 항으로 X를 무기한 보호**했다.
> GC는 무덤 rename 시점에 그 put의 **결말을 모르면서** 복원해 버렸고, 겹치는 실패가 반복되면 회수가 **영영**
> 밀렸다. **그것이 r2의 P-2였다.** 봉인은 `live`를 술어에서 **제거**하고 **대기 조건으로 강등**하는 것이다.
> 결정적 증인: **T-C3**.

> ⚠ **정직하게**: 이 "파괴 연산은 하나뿐" 진술은 **GC 삭제 분기에 한정**된다. D-4로 격리 분기를 손대지 않으므로
> **reconcile에는 두 번째 파괴 연산 `rename(blob → .corrupt)`가 봉인되지 않은 채 남아 있다**(F-25).

### 크래시 (SIGKILL / 전원 / k8s SIGTERM — `main.rs:14`에 SIGTERM 핸들러가 **없다**)

모든 전이가 rename + `fsync_dir`이므로 무덤 상태는 세 스냅샷 중 하나: **(A)** `<sha>` 존재·무덤 없음 /
**(B)** 무덤 존재·`<sha>` 없음 / **(C)** settle 완료. **"둘 다 없음"은 rename 원자성으로 불가능.**
(B)에서 **inode는 살아 있다** → 재시작 후 첫 패스의 `recover_graves`가 `rename(grave → blob)` → **그 다음에**
`collect_referenced`가 돈다 → 크래시 창에 커밋된 포인터도 정상 refs로 관측된다.

인메모리 흔적(live/landed/코호트) 소실은 무해하다: 크래시 시점에 P의 핀이 살아 있었다면(=커밋 진행 중) **그
순간까지 Reap은 일어날 수 없었고**(코호트 대기 중이었거나 아직 무덤도 안 팠다), 재시작 후엔 디스크가 유일
진실이다 — 포인터가 내구화됐으면 `refs`가 잡고, 안 됐으면 참조가 없으니 회수가 정당하다.

**코호트 대기 중 사망도 같은 논증에 흡수된다** — 대기 중 디스크 상태는 정확히 **(B)**(무덤 존재·`<sha>` 없음)이고,
파괴 연산은 **아직 한 번도 일어나지 않았다.** 다음 패스의 `recover_graves`가 `collect_referenced` **이전에**
복원하므로 그 사이에 착지한 포인터도 정상 `refs`로 잡힌다. **새 논증이 필요 없다.** (put이 결국 실패했다면
복원된 blob은 여전히 무참조·만료 → 그 패스가 다시 무덤을 파고 이번엔 코호트가 비어 있으므로 **즉시 Reap** →
회수가 **한 패스 밀릴 뿐 누적되지 않는다**.)

### 취소 (`upload_timeout` 발화 / 클라이언트 disconnect — **실제 취소원**)

- **커밋 진입 전 취소**(스트리밍·blob 쓰기 중): 핀 drop(→ 코호트 drain·notify), `landed` 무흔적, 포인터 없음 →
  GC가 **오늘과 똑같이 회수**하고 `gc_deleted == 1`. (두 번째 플립 소멸의 핵심 — **이제 GC가 그 결말을 보고 나서
  판정한다**)
- **커밋 도중 취소**: `JoinHandle`이 드롭돼도 blocking 클로저는 **끝까지 실행된다** → 핀은 rename·마킹·fsync가
  끝난 뒤에 풀린다 → 결말이 `Ok`면 (2) **Restore**, `Err`면 (4) **Reap**. 어느 쪽이든 **GC는 그 결말을 기다렸다가**
  본다. crash 렌즈가 지목한 유실 시퀀스가 여기서 죽는다.
- **클로저가 시작되기 전에 런타임 셧다운**: 클로저가 아예 안 돌 수 있다 → rename 없음 → 포인터 없음 → (4). 안전.
  (`JoinHandle`이 드롭돼도 `PinGuard`는 클로저와 함께 드롭되므로 **코호트는 반드시 drain된다** — 대기가 영원히
  걸리지 않는다.)
- **reconcile 퓨처 취소**(코호트 대기 중 포함): `Graved`에 파괴적 Drop이 없다 → 무덤 잔존 → (B) → 다음 패스 복구.
  `PassGuard::drop`은 `pass_live=false`·`landed.clear()`만 한다(디스크 무접촉).

### 복원 실패 (restore rename EIO/ENOSPC)

`settle`이 `Err`를 무가공 전파 → `run_once` `Err` → main은 `warn!`만. 디스크는 (B). **unlink는 한 번도 안
일어났다.** 해당 객체는 **일시적** non-servable(404/list 제외 — `objects.rs:117`, `listing.rs:23`)이며 다음
패스가 (A)로 되돌린다. 오늘의 **영구** 유실과 질적으로 다르다.

⚠ **정직하게**: 정상 Restore 경로에도 **fsync 2회 폭의 transient non-servable 창이 새로 생긴다**(오늘 없던
상태). 복원이 실패하면 최대 `gc_grace`(prod: `reconcile_interval == gc_grace`) 동안 404. §Preserved Contract에
계약 항목으로, §남은 위험에 위험으로 올린다.

### 재시작 / 롤백

**재시작**: 위 크래시 절과 동일 — `recover_graves`가 `collect_referenced` **이전에** 돈다.

**롤백(구 바이너리)**: `.gc-grave-<sha>`는 구 `classify_objects_entry`(`layout.rs:162-173`)에서 `Other`로
떨어진다 → temp 분기(`.tmp-` 접두)도 blob 분기(64hex)도 안 걸린다 → **구 코드는 절대 지우지 않는다.** 이름이
sha를 품으므로 수동 복구는 `mv .objects/.gc-grave-<sha> .objects/<sha>` 한 줄이다(런북: B-3).
원안의 `.tmp-<unique>` 이름은 **신·구 양쪽이 삭제**했다 — 그게 P-1의 사인이었다.

## Single-Flip Contract

### 뒤집히는 관측 행동 (최종 진술 — **r2/P-2로 개정**)

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
> **단 하나의 예외 — degraded 경로(r3/P-4)**: 코호트 멤버의 **파일시스템 연산이 `settle_timeout` 안에 돌아오지
> 않으면**, P는 술어를 **평가할 수 없다**(결말을 모른다). 이때 P는 **fail-CLOSED**로 간다 — X를 정본 이름으로
> **복원**하고(보존), tombstone을 유지하고, `gc_deleted`를 **증가시키지 않고**, `tracing::error!`를 내고,
> **패스를 정상 종료**한다(같은 패스의 다른 blob은 오늘과 똑같이 회수된다). **X의 회수는 다음 패스로 연기된다.**
> 이 경로는 **정상 입력에서 도달 불가능하다** — 아래 §degraded-path 연기 참조.

> **개정 전과 무엇이 다른가**: 개정 전에는 보호 술어가 **`live ∨ landed`** 두 항이었다. `live` 항은 GC가 그 put의
> **결말을 알기도 전에** X를 보호하게 만들었다 — 그 put이 뒤늦게 rename에 실패해 **포인터를 하나도 만들지 않아도**
> X는 이미 복원된 뒤였고, 겹치는 실패가 반복되면 회수가 **무기한 연기**됐다(r2 P-2). 개정안에서 **`live`는 보호
> 술어가 아니라 대기 조건**이다. 보호 술어는 **`landed` 하나뿐**이고, 그것은 **결말이 확정된 뒤에** 평가된다.
> **"살아 있음"은 더 이상 "성공할 것임"의 프록시가 아니다 — 프록시를 쓰지 않고 결말을 기다린다.**

### "착지(landed) 흔적"이 P-2의 두 번째 플립을 없애는 논증

P-2(중대)는 정확했다. 개정 1차안의 `arm()` = "커밋을 **시도**한다"는 `write_atomic(meta)`의 **5개 실패 지점 중
4개**(`mkdir_p` / `File::create` / `write_all` / `sync_all`)를 **rename 이전임에도** sticky 흔적으로 만들었다.
그 4개는 **커밋 포인터가 존재할 수 없음이 확실한** 영역이다. 그리고 **ENOSPC는 정확히 그 4개에서 터진다**
→ dedup 대상 blob이 보호됨 → **GC가 공간을 회수해야 하는 바로 그 상태에서 회수를 못 한다**(자기강화 루프).
이것이 P-2가 지목한 "capacity/statistics 두 번째 행동"이다.

최종안은 흔적의 정의를 **"rename이 `Ok`를 반환했다"**로 옮긴다(`Staged::commit_blocking`이 `on_landed`를
rename의 `?` **뒤에서만** 호출한다). **그리고 r2 개정으로, GC는 그 흔적을 put의 결말이 확정된 뒤에 읽는다.**

**죽는 지점별 표 (개정 — 두 번째 플립이 실제로 0이 되는가?)**

| 죽는 지점 | 코호트 대기 | 커밋 포인터 | `landed` 흔적 | GC 판정 | 오늘과 동일한가 |
|---|---|---|---|---|---|
| `mkdir_p` / `create` / `write_all` / `sync_all` (**ENOSPC 포함**) | 그 핀이 drop될 때까지 | **존재 불가** | **없음** | **Reap**, `gc_deleted += 1` | ✅ 동일 |
| **커밋 `rename` `Err`** (ENOSPC/EIO — 원자적, 타깃 불변) | 그 핀이 drop될 때까지 | **존재 불가** | **없음** | **Reap**, `gc_deleted += 1` | ✅ 동일 ← **r2 P-2의 정면 케이스** |
| 커밋 진입 전 취소(`upload_timeout`/disconnect) | 핀 즉시 drop | 존재 불가 | 없음 | **Reap**, `gc_deleted += 1` | ✅ 동일 |
| 커밋 **진행 중** 취소 | 클로저 완주까지(무취소) | 결말에 따름 | 결말에 따름 | 위/아래 줄 중 하나로 **귀결** | ✅/🔁 (귀결에 따름) |
| **커밋 `rename` `Ok` 이후**(fsync_dir 실패 포함) | (그 핀은 마킹 뒤 drop) | **존재** | **landed** | **Restore** | 🔁 **이것이 유일한 플립** |
| **코호트가 비어 있음**(무덤 시점에 핀 0) | **대기 0**(fast path) | — | 없음 | **Reap**, `gc_deleted += 1` | ✅ 동일 |
| **무덤 이후에 생긴 put** | **기다리지 않는다**(코호트 밖) | 무관 | (착지했으면 landed → Restore = **정답**) | 핵심 사실 **D** — 자급자족 | ✅ 동일(**회수 연기 0**) |
| **rename `Ok` 직후 스톨**(fsync가 안 돌아옴) | **대기 0** — `landed` 삽입이 **즉시 깨운다**(r3/P-4 ①) | **존재** | **landed** | **Restore**(즉시) | 🔁 위와 같은 **그 하나의 플립** |
| **rename 이전 영구 스톨**(FS가 안 돌아옴) | `settle_timeout`까지 | **아직 없음**(결말 불명) | 없음 | **Deferred** — 복원·무카운트·`error!`·**패스 해제** | ⚠ **degraded 전용**(아래) |

**"두 번째 플립"의 잔량 = 0**, 그리고 **정상 경로의 "연기" 잔량도 = 0**(degraded 경로는 아래에서 따로 특성화).

- 병리적 재시도 폭풍(ENOSPC 루프)에서 흔적은 **한 번도 생기지 않는다** → **매 패스 Reap된다.**
- **겹치는 실패 put**(r2 P-2가 지목한 시퀀스: put이 X를 관측 → GC가 무덤을 팜 → put이 rename에 **실패**)도
  **대기 후 흔적 0**이므로 **같은 패스에서 Reap**된다. 개정 전에는 `live` 항 때문에 Restore였다.
- **starvation 불가** — 결말을 **알고** 판정하기 때문이다. 프록시가 없으므로 프록시의 오차도 없다.

> **`live` 항을 없앤 대가로 무언가 잃었는가?** 아니다. `live` 항이 덮던 유일한 실질 케이스는 "put이 X를 관측했고
> **결국 착지할** 상태"였는데, 그 케이스는 **대기 후 `landed`가 그대로 잡는다**(케이스 (2)). `live` 항은 그 위에
> **"결국 실패할 put"까지 얹어서** 보호하던 것이고 — 그것이 정확히 **P-2가 지목한 과보호**였다. 대기는 그 둘을
> **결말로 분리**한다.

#### degraded-path 연기 — 특성화, 그리고 **왜 두 번째 플립이 아닌가** (r3/P-4)

**무엇이 일어나는가.** 코호트 멤버의 FS 연산이 돌아오지 않으면 그 핀은 영원히 산다. `settle()`은
`settle_timeout`에서 fail-CLOSED로 빠지고 X를 복원한다. **핀이 계속 살아 있으면 매 패스가 같은 일을 반복한다** —
X를 무덤으로 옮기고, `settle_timeout`을 태우고, 복원하고, `error!`를 낸다. **X의 회수는 스톨이 풀릴 때까지
연기된다.** 정직하게 적는다: **이것은 실재하는 연기이며, 오늘(=버그 있는 코드)에는 없는 행동이다.**

**정상 경로의 연기(P-2가 때린 것)와는 다른 물건이다:**

| | **P-2의 연기**(개정 전 — 봉인됨) | **P-4의 degraded 연기**(이번 개정이 **남기는** 것) |
|---|---|---|
| **트리거** | **평범한 실패 put**(ENOSPC · rename `Err` · 취소) — **정상 운영에서 흔하다** | **파일시스템 연산이 영영 돌아오지 않음**(NFS 정지 · EBS 열화 · dm-thin 고갈) — **병리** |
| **왜 연기되나** | GC가 결말을 **모른 채** `live`를 "성공할 것"의 **프록시**로 오독 → 실패할 put이 X를 보호 | GC가 결말을 **알아낼 수 없다**(FS가 답을 안 준다) → **모르면 보존**(fail-CLOSED) |
| **누적** | **무한정.** 겹치는 실패가 반복되면 X는 **영영** 회수 안 됨. **ENOSPC에서 자기강화**(공간을 회수해야 할 때 회수 불가) | 스톨이 **풀리는 즉시** 종료(핀 drop → 다음 패스 코호트 비어 있음 → 정상 판정). **자기강화 없음** |
| **관측성** | **없음.** 조용히 안 지운다 | **`tracing::error!`**(sha · cohort_size · waited_ms) — **매 패스 시끄럽다** |
| **다른 blob 영향** | 없음 | **없음** — 루프가 계속 돈다. 같은 패스의 다른 blob은 **오늘과 똑같이 회수**된다(T-P4a 3번 단언) |
| **GC 정지** | 없음 | **없음** — `pass_lock` 해제, 후속 패스 정상(**그게 P-4 봉인의 목적**) |
| **결말을 아는가** | **모르면서 판정했다**(과보호) | **모르는 걸 안다**(보존을 택하고 **말한다**) |

**왜 두 번째 관측 행동 플립이 아닌가 — 세 가지 근거:**

1. **정상 입력에서 도달 불가능하다.** 이 분기는 "**syscall이 `settle_timeout`(기본 660s) 동안 반환하지 않는다**"
   는 조건에서만 발화한다. 정상적으로 **느린** put은 여기에 걸리지 않는다 — `upload_timeout`이 호출자 퓨처를
   자르면 **핀이 즉시 drop되고**(가드는 caller 소유) 코호트가 드레인된다. 즉 `settle_timeout`은 **느림**이 아니라
   **정지**를 잡는다. 관측 계약(105 characterization + 골든 3종 + adversarial)의 **어떤 입력도** FS 연산을
   정지시키지 않는다 → **stats·골든 트리·서빙 계약이 한 비트도 변하지 않는다**(B-2 acceptance의 성능 sanity가
   이미 "코호트는 항상 비어 있다"를 못박는다).
2. **오늘 그 자리에는 "정의된 행동"이 없다 — 스토어가 사실상 죽는다.** FS가 멈춘 상태에서 **오늘의** reconcile은
   같은 FS에 대해 `tokio::fs::read`(blob 전량 재독) → 역시 **abort 불가능한 `spawn_blocking`** → **패스가 그
   자리에서 무한정 멈춘다.** 그 사이 put들은 `upload_timeout`에 잘려 400을 뱉고, blocking 풀이 고갈된다.
   비교 대상은 "**연기 vs 즉시 회수**"가 아니라 "**연기 + 시끄러운 에러 + 나머지 blob 정상 회수**" vs
   "**패스 자체가 멈춤 / 또는 (스톨이 커밋 클로저에서 나면) 포인터만 남고 blob 부재 = 지금 고치는 바로 그 유실**"
   이다. **degraded 경로는 오늘보다 엄격하게 낫다.**
3. **플립은 `flips[]`의 그 한 줄에 **포함**된다 — 반대 방향이 아니라 **같은 방향의 약화**다.** 선언한 플립은
   "*무참조·만료 X를 GC가 지운다 → **커밋 포인터가 실재할 수 있으면 지우지 않는다***"이다. degraded 분기는 그
   술어를 **알 수 없을 때 보수적으로 참으로 취급**하는 것 — 즉 **같은 보호 방향으로 한 걸음 더 갈 뿐**이며,
   **새로운 삭제를 만들지 않는다**(fail-CLOSED는 `gc_deleted`를 **늘릴 수 없다**). 두 번째 플립이 되려면 **오늘
   보존되던 것이 삭제되거나**, **오늘과 다른 응답/통계가 정상 경로에서 관측**되어야 하는데 **둘 다 없다.**

> **⚠ 정직하게 — 과장하지 않는다.** 이것을 "행동 변화 0"이라고 주장하지 **않는다.** 병리적 상황에서 **회수가
> 연기되는 것은 사실**이고, 그 상황이 **오래 지속되면 디스크가 안 비워진다**. 우리가 주장하는 것은 딱 이만큼이다:
> **(a)** 그 경로는 **정상 입력에서 도달 불가능**하고, **(b)** 오늘 같은 상황에서는 **스토어가 사실상 죽는다**
> (패스 정지 또는 영구 유실), **(c)** 연기는 **시끄럽고**(`error!`) **국소적이며**(그 blob 하나) **자기치유적**
> (스톨이 풀리면 즉시 정상)이다. **릴리스 게이트가 이것을 두 번째 플립으로 판정한다면**, 제거할 방법은
> **대기를 무한정으로 되돌리는 것**(= P-4 부활)뿐이므로 — **연기를 없애는 것이 아니라 GC를 영구 정지시키는 것과
> 맞바꾸는 것**이다. 그래서 **연기를 택하고, 특성화하고, 증인을 붙인다**(T-P4a). 이것이 "characterize"의 의미다.

#### 왜 이것이 P-2가 금지한 "커밋 성공을 묻는 질문"이 아닌가

인간이 확정한 P-2 문구는 "무장은 **시도**이지 성공이 아니다"였다. 그 근거는 **"성공 여부를 신뢰성 있게 알 수
없다"**였고 — 그것은 **커밋 경로가 취소 가능하다는 전제** 위의 명제였다(탈락한 4안의 claim 게이트가 rename↔fsync
사이 취소에서 fail-OPEN으로 죽은 이유). 최종안은 **그 전제를 제거한다**: 커밋 경로에 취소점이 **없다**(단일
blocking 클로저, rename과 마킹 사이에 `await`가 없다). **알 수 없던 것을 알 수 있게 만들었으므로 프록시가 필요
없다.**

- **4안과의 차이**: 4안은 **디스크에 "커밋됐냐"고 되물었다**(TOCTOU + 취소). 최종안은 **rename의 반환값을
  in-memory에 즉시 기록**한다. 되묻지 않는다.
- **fail-OPEN 잔량 0**: 마킹을 건너뛸 수 있는 유일한 경로는 (a) rename `Err`(⇒ 포인터 없음), (b) 프로세스
  사망(⇒ 인메모리 상태 자체가 무의미), (c) 클로저 미시작(⇒ rename 없음). **셋 다 "포인터 없음"이다.**
- P-2의 **정신**(= 흔적을 "포인터를 남길 수 있는 put"으로 정확히 좁혀라)은 이행되고, **문구**(무장 위치 =
  `write_atomic` 직전)는 상위 제약(관측 행동 플립 정확히 1개)에 의해 **초과 이행**된다. 문구를 지키면 제약이
  깨진다는 것이 flip 렌즈 FATAL-1의 증명이다. **제약을 택했다.**
- **r2 개정 이후**: 이제 GC는 "커밋이 성공했냐"를 **묻지 않는다** — 그 put이 **끝나기를 기다렸다가**, 성공했다면
  put 스스로가 남긴 **확정 사실**(`landed`)을 읽을 뿐이다. **질문이 아니라 관측이다.** 취소·실패는 흔적을 남길
  수 없고, 흔적이 있다는 것은 **포인터가 VFS에 있다는 것과 동치**다(핵심 사실 A + C).

#### `armed` 맵과 P3 시드가 사라진 이유 (뮤턴트 클래스 소멸)

개정 1차안의 "패스 시작 시 `touched := armed 스냅샷`"(P3)은 **불필요하다.** 패스 이전에 착지한 커밋은 케이스
(1)이 `refs`로 잡고, 패스 이전에 시작됐지만 아직 착지하지 않은 커밋은 **코호트 대기**가 결말까지 붙잡았다가
케이스 (2)의 `landed`로 잡는다(핀이 커밋 클로저 소유이므로 착지 전에 죽지 않는다 — 핵심 사실 A). 시드가 덮으려던
케이스가 **구조적으로 소멸**했다 → `armed` 맵·시드·M3/M5 뮤턴트 클래스가 통째로 사라진다. **설계가 작아졌다.**

### flips[]

1행 — `dedup_put_during_reconcile_window_must_not_lose_blob` / symptomToken `DATA LOSS`. (불변)

## Preserved Contract

`characterizationCmd`(105 passed)가 핀하는 것 전부. 항목별 근거:

| 보존 항목 | 근거 |
|---|---|
| **2단계 tombstone GC 의미론** | pending 로드/삽입/삭제/최종 정리 로직 무변경. `Restored`·**`Deferred`** 시 `pending.remove()` 안 함(**D-2**) → 다음 패스에서 여전히 만료 상태로 재판정. 복원은 "이 패스에서는 판단 보류"지 "참조됨 확정"이 아니다. **`Deferred`는 그 "판단 보류"의 가장 정직한 사례다** — 판단할 정보를 **못 얻었다**(r3/P-4) |
| **temp grace 보존** | Temp 분기 무변경. `.gc-grave-`는 `.tmp-` 접두가 아니므로 Temp 분기에 **절대 들어오지 않는다**(문자열 서로소 — P-1이 지적한 원안의 오분류가 여기서 죽는다). mid-flight 테스트의 `tmp_entries`(`.tmp-` 접두만 카운트) → "정확히 1개" 불변 |
| **비트로트 격리** | **분기 자체를 손대지 않는다**(D-4). `.corrupt/<sha>`에 손상 바이트 · `quarantined += 1` · `pending.remove` — 코드가 그대로다 → `corrupt_blob_quarantined` 초록이 **자명하게** 유지된다. ⚠ **대가**: F4 유실 레이스가 **미봉인**으로 남는다(아래 굵은 항목) |
| **목록/서빙 계약** | `head`/`get_bytes`/`list`/`open` 무변경. ⚠ **정직한 부수 변화**: 무덤 창(grave rename ~ settle) 동안 그 sha를 가리키는 객체는 **일시적으로** 404/list 제외다. 오늘 이 자리는 **영구 유실**이었으므로 순개선이나, **오늘 없던 transient 상태**다. **r2 개정으로 이 창이 코호트 대기만큼 늘어난다** — §정직한 부수 변화 1 참조 |
| **에러 표면 (B7)** | `io::Result` 무가공 `?` 전파. `spawn_blocking(..).await.expect("join")`은 기존 `atomic::fsync_dir:26` 관행 축자 동일. put의 실패는 여전히 `AppError::Internal` → 500. ⚠ **r3/P-4**: settle 타임아웃은 **`io::Error`를 합성하지 않는다**(어떤 syscall도 실패하지 않았다 — 합성이야말로 B7이 금지하는 **가공**이다). `tracing::error!` + `Deferred` + **루프 계속**이며, `settle()` 안의 **진짜** io 실패(복원 rename EIO/ENOSPC · `remove_file` · `fsync_dir`)는 **여전히 `?`로 무가공 전파**된다 → **개정 전과 바이트 동일**. §에러 표면 논증 |
| **`ReconcileStats` 필드 정의** | 필드 **0개 추가**(추가하면 `layout_tree.rs:71,137,198`의 전수 구조체 `assert_eq!`가 깨진다 = 두 번째 플립). `recovered`/`restored`/**`deferred`(r3/P-4)** 전부 **tracing 전용**. 연기 카운터가 필요하면 stats 계약을 여는 **별도 파이프라인**(→ F-29) |
| **GC 패스의 종료성 (r3/P-4 — 신규 항목)** | **패스는 반드시 끝나고 `pass_lock`은 반드시 풀린다.** `settle()`의 모든 대기 경로에 **유한 상계**(`settle_timeout`)가 있다 → 멈춘 핀 하나가 **그 패스의 다른 blob 회수도**, **이후의 복구·GC 패스도** 막지 못한다. ⚠ **오늘의 코드에는 `pass_lock`이 없으므로 이것은 "보존"이 아니라 "새 잠금장치가 스스로를 잠그지 않음"의 증명이다.** 증인: **T-P4a**(단언 ③·④) |
| **`pin()` 무대기 (P1)** | 동기 `Mutex`, 임계구역이 await를 걸치지 않는다(코호트 drain 검사도 락을 **await 앞에서 놓는다**). put은 GC를 **절대 기다리지 않는다** → 기각된 "블롭 락"(`upload_timeout` 예산 오염 · `cap.reserve` 누산 → 400/507)으로 되돌아가지 않는다. ⚠ **r2 개정의 대기는 GC → put 단방향**이다: put은 `pass_lock`을 잡지 않고 GC는 `KeyLocks`를 잡지 않는다 → **순환 대기 불가 = 데드락 불가**. `upload_timeout`·`cap.reserve` 예산은 **한 바이트도 변하지 않는다**(느려지는 것은 백그라운드 GC 패스뿐) |
| **골든 트리 잔재 0 — 구조적 사실** | ① 무덤은 `.objects` **직속 평면 파일**이다 — **`mkdir`이 코드에 없다** → 빈 디렉터리 잔재가 **불가능**하다(`rel_files`가 디렉터리를 수집하지 않는다는 `layout_tree.rs:20-36`의 **허점에 기대지 않는다**). ② 골든 스크립트의 reconcile은 `gc_deleted:0, gc_pending:1` — **첫 관측 → `pending.insert`**이므로 `grave()`에 **진입조차 하지 않는다**. ③ 정상 완료 패스는 무덤을 남길 수 없다(`grave()`의 결과는 같은 표현식에서 `settle()`에 소비되고 `?` 외 탈출 경로가 없다) → `expected` 리스트 **바이트 동일** |
| **`RESERVED_SUFFIXES` / `valid_key`** | 불변. `segment_ok`(`layout.rs:19`)가 `.` 시작 세그먼트를 금지 → 사용자 키와 무덤 이름 **충돌 불가**. `pointers_all`은 `.objects` 스킵(`layout.rs:262`), `is_commit_pointer_name`은 `.meta.json` 요구 |
| **`write_atomic` 공개 시그니처** | **불변**(`pub async fn write_atomic(&Path, &[u8]) -> io::Result<()>`). 내부만 단일 blocking 클로저로 위임 — **syscall 시퀀스 축자 동일**, 취소 입도만 "부분 → 전무"로 **좁아진다**(부분 상태의 순감소) |
| **같은 키의 쓰기 직렬화 (B8)** | `KeyLocks`·`lock_key`·가드 획득 지점 **무변경**. 바뀐 것은 **가드의 수명**뿐이다 — 커밋 클로저로 **이전**된다(S-1 수리). ⚠ **그 수리가 새 degraded-path 행동을 낳는다**: **멈춘 fs에서 그 `bucket/key`는 프로세스 재시작까지 쓰기 불가**다. **오늘 없던 상태**이므로 아래 §재시작-필요 복구 계약에 **정직하게** 편다 — 릴리스 게이트 제출물 |
| **`KeyLocks::lock`의 대기 의미론** | **무한정 대기·에러 반환 없음 — 불변.** B-1이 더한 것은 `LOCK_WARN_AFTER`(30s) 초과 시의 **`tracing::error!` 한 줄**뿐이다. 대기 퓨처를 **드롭하지 않고** 계속 await하므로 tokio Mutex의 **FIFO 대기열 위치도 보존**된다 → 공정성 포함 **행동 delta 0**. 반환 타입에 실패가 없다(`KeyGuard`, `Result` 아님) → *"타임아웃으로 가드를 포기한다"*가 **표현 불가**하다(S-1 부활 차단). 증인: **T-S2** ⑤ |

### ⚠ 재시작-필요 복구 계약 (S-2 — **새 degraded-path 행동**, 릴리스 게이트 제출물)

> **무엇이 바뀌는가.** 커밋 클로저는 `PinGuard`와 `KeyGuard`를 **함께 소유**하며 rename·fsync가 끝난 뒤에야
> 놓는다. **시작된 `spawn_blocking`은 취소할 수 없다.** 따라서 계획서가 **스스로 모델링한** "반환하지 않는
> 파일시스템 연산"(P7의 존재 이유) 하에서는, `upload_timeout`이 호출자 퓨처를 드롭해도 detach된 클로저가
> 그 키를 **syscall이 반환하거나 프로세스가 재시작될 때까지** 붙들고 있다 →
> **그 `bucket/key`는 재시작까지 쓰기 불가**다. 같은 키의 DELETE는 **타임아웃이 없고**(`objects.rs`의
> `delete`), PUT 재시도는 **대기 전에 전역 capacity를 예약**하므로(`http/internal/files.rs`) 그 키로
> 재시도가 쌓이면 **전역 capacity를 갉아** 장애가 번질 수 있다.
>
> **이것은 의도된 교환이다.** 가드를 타임아웃으로 놓는 것은 **금지**다 — S-1이 되살아난다.

**왜 두 번째 관측 행동 플립이 아닌가 (정직한 논증)**

1. **오늘, 같은 입력에서 무슨 일이 일어나는가.** 오늘의 커밋도 `tokio::fs::rename` = `spawn_blocking`이다
   (§크래시 렌즈가 이미 코드로 증명). 오늘 그 rename이 반환하지 않으면 — 키 가드가 **호출자 퓨처**에 있으므로
   `upload_timeout`이 그것을 **풀어버린다** → 같은 키의 DELETE가 진행돼 **성공**하고 → 뒤늦게 깨어난 낡은
   rename이 **삭제된 키를 되살린다**. 즉 **오늘의 행동은 "쓰기 가능 + 조용한 되살아나기"**이고, 새 행동은
   **"쓰기 불가 + 되살아나기 없음"**이다. **정상 → 비정상의 플립이 아니라, 병리적 입력 하에서 두 실패 모드 중
   무엇을 택하느냐**다. 그리고 그 선택은 **잠김(가용성) < 되살아나기(무결성)** — 삭제된 키의 부활은 **조용한
   데이터 손상**이고, 잠김은 시끄러운 가용성 손실이다.
2. **그 입력에서는 오늘도 스토어가 사실상 죽어 있다.** reconcile도 **같은 fs**를 읽는다
   (`recover_graves`·`collect_referenced`). 멈춘 fs는 이 스토어의 **전역 장애**이지 이 키의 국소 장애가 아니다.
   `settle_timeout`(P7)이 애초에 존재하는 이유가 바로 그 상황의 모델링이다 — **계획서가 스스로 인정한 전제**다.
3. **blast radius.** 홈랩 **단일 replica + RWO PVC**라 잠기는 것은 **그 키 하나**다(락은 `bucket/key` 단위 —
   `lock_key`). 다른 키의 PUT/DELETE는 영향받지 않는다.
4. **관측 계약의 표면.** 이 픽스가 세는 플립은 characterization·regression이 **실제로 구동하는** 관측 표면의
   행동이다. "반환하지 않는 syscall"은 **프로덕션 fs에서 어떤 테스트도 구동할 수 없는 입력**이며(T-S2는 훅으로
   그 대역을 **모형화**할 뿐이다), 그 입력에서 오늘의 결과는 "정의된 행동"이 아니라 **조용한 손상**이다.

**⚠ 그럼에도 정직하게 — 반론을 숨기지 않는다.** 이것은 **오늘 없던 상태**다("그 키가 재시작까지 쓰기 불가").
"관측 불가한 입력"이라는 4번 논거는 **약하다** — 멈춘 NFS·풀린 PVC는 홈랩에서 실재하는 입력이다.
릴리스 게이트가 이것을 **숨겨진 두 번째 플립**으로 판정하면 **Blocker로 받아들인다.** 우리는 숨기지 않는다 —
계약은 **네 곳**에 박혀 있다: `PinGuard::commit_pointer` doc · `KeyGuard` doc · **T-S2**(기계 증인) ·
`KeyLocks::lock`의 **`tracing::error!`**(운영자가 그 상황을 **본다**).

**관측성 — 그 상황은 침묵하지 않는다.** `KeyLocks::lock`은 획득이 `LOCK_WARN_AFTER`(**30s**)를 넘기면
**한 번** `error!`를 내고 **계속 기다린다**(행동 delta 0 · `ReconcileStats` 필드 0개 추가):

```text
key lock held beyond threshold — an uncancellable commit may be wedged on a stalled filesystem;
this key stays unwritable until the syscall returns or the process restarts (deliberate:
releasing the guard would let a detached commit resurrect a deleted key)
    bucket=<b> key=<k> waited_ms=30000
```

첫 절은 **사실**("락이 임계를 넘겨 잡혀 있다")이고 해석절은 **유보**("**may** be wedged")다 — 알려진 오탐
클래스(`put_stream`이 스트리밍 본문 내내 락을 쥐므로 같은 키의 동시 writer가 `upload_timeout`(600s)까지
**정당하게** 대기할 수 있다)에서도 **거짓말하지 않는다**.

**→ F-30 (후속 파이프라인)**: **키-바인드 펜싱 / 버전화된 포인터 발행으로 잠김 **없이** 되살아나기를 막는다.**
커밋이 자기 키의 **펜스 토큰**(또는 포인터 세대 번호)을 들고 rename하고, 발행 시점보다 낡은 토큰의 rename은
**커밋 자체가 거부**한다 → 가드를 일찍 놓아도 낡은 커밋이 이길 수 없다 ⇒ **잠김 0 · 되살아나기 0**.
Codex의 첫 번째 권고이며 **설계가 커져서**(원자적 세대 발행 + 크래시 복구 시 세대 재구성 + 무덤/복구 경로와의
상호작용) 이번 범위에서 뺐다. 그때까지의 계약이 위의 **재시작-필요 복구**다.

### ⚠ 이 픽스는 부분 해결이다 (D-4)

> **격리 분기의 유실 경로는 미해결로 남는다.** `reconcile.rs`의 비트로트 격리 분기는 여전히 핀·무덤을 거치지
> 않고 `read → rename(blob → .corrupt)`를 한다. 손상 blob을 동시 put이 `write_atomic`으로 **치유한 직후**
> 패스가 그 **치유된 inode**를 `.corrupt`로 옮기면 — **오늘 고치는 것과 같은 증상**(커밋 포인터만 남고 blob
> 부재 → 영구 404)이 재현된다. 선행 비트로트가 필요하므로 이번 플립과 **직교**하지만, **"포인터만 남고 blob
> 부재" 증상 클래스는 이 픽스 이후에도 완전히 닫히지 않는다.**
>
> **왜 여기서 안 고치는가**: 고치면 두 번째 관측 행동 플립이다("치유된 blob이 격리되어 404가 된다" → "안 된다").
> gated-bugfix **하드룰 10**: *"두 번째 관측 행동 플립은 근본 원인을 공유하거나 first-increment diff 안에
> 들어오더라도 **항상 별도 파이프라인**."* → **F-25**로 분리한다(설계 청사진 포함).
>
> **릴리스 게이트에 이 문장을 그대로 제시한다.** 이 사실을 숨기고 "증상 클래스 해결"이라 주장하면 Blocker다.

### 정직한 부수 변화 (관측 계약 밖, 테스트 없음)

1. **정상 Restore 경로의 transient non-servable 창** — 무덤 rename ~ 복원 rename 사이 404. **오늘 없던 상태**다
   (오늘은 그 자리가 영구 유실). 창의 폭(⚠ **r3/P-4로 개정 — r2안의 상계는 거짓이었다**):
   - **코호트가 비면**(정상 스크럽) — 대기 0 → 창 = **fsync 2회 폭**. 개정 전과 동일.
   - **코호트에 put이 하나면** — 그 put이 착지하는 즉시 핀이 drop되고 settle이 깨어난다 → 창 ≈ **notify + fsync 2회**.
   - **코호트에 put이 둘 이상이면** — ⚠ **r2안에서는** 먼저 착지한 put의 객체가 **나중 put이 끝날 때까지** 404였다.
     **r3에서 이 창이 닫힌다**: `landed` 삽입이 `Notify`를 울리고 `settle()`이 **그 즉시 깨어나 복원**한다
     (P7 (a)) → **나머지 코호트를 기다리지 않는다** → 창 ≈ **notify + fsync 2회**. **이것이 P-4 권고 ①의 효과다.**
   - **코호트 멤버가 rename에 **도달하지 못한 채** 스톨하면**(degraded) — `landed`가 없으므로 깨울 것이 없다 →
     창 = **`settle_timeout`**(기본 660s). 그 뒤 **fail-CLOSED 복원**. ⚠ **정직히**: 이 창 동안 그 sha를 가리키는
     **기존** 객체들이 404다. **오늘 같은 상황에서는 그 자리가 영구 유실이거나 패스 자체가 멈춘다** — §degraded-path 연기.
   - 복원이 **실패**하면 최대 `gc_grace`(prod: `reconcile_interval == gc_grace`) 동안 지속 — 개정 전과 동일.
   - **무덤 이후에 생긴 put은 영향 없다** — 바이트를 재기록하므로 `<sha>`가 즉시 존재한다(핵심 사실 D).
2. **`upload_timeout` 발화 시 커밋 클로저가 이미 시작됐으면 객체가 커밋된다.** **오늘도** `tokio::fs::rename`이
   `spawn_blocking`이라 취소 후 착지한다(crash 렌즈가 코드로 증명) → 새 상태가 아니라 **확률 분포의 이동**이다.
   HTTP 응답은 불변(`400 upload_timeout`).
3. **`write_atomic` 전체가 무취소가 되며** 다른 호출부(`delete`의 fsync, gc-pending 쓰기)의 부분 상태가
   사라진다. 순개선.

> ⚠ **네 번째 부수 변화는 이 목록에 넣지 않는다** — **재시작-필요 복구 계약**(S-2)은 "테스트 없음"이 아니다.
> **T-S2가 기계 증인**이고 `tracing::error!`가 운영 증인이다. §재시작-필요 복구 계약 참조.

## Regression test (already RED at red.sha)

- **seam**: 진짜 호출부(`Store::put` + `reconcile::run_once`)를 동시에 구동하고,
  HTTP 핸들러가 쓰는 것과 같은 `get_bytes`/`list`로 판정한다. 얕은 단위 stub이 아니다.
- **regressionCmd**: `cargo test --test regression_reconcile_gc_dedup_race`
- **symptomToken**: `DATA LOSS`
- **red.sha**: `6545808` — RED verify-record
  `docs/reviews/reconcile-gc-dedup-race/bugfix-verify-red-cc8704f.json` **커밋됨**
  (스크립트가 red.sha를 throwaway 워크트리에 체크아웃해 직접 재실행: regression exit
  101 / symptomTokenPresent true / characterization exit 0).

⚠ **이 회귀 테스트는 B-1에서 기계적으로 편집된다** — 그 `tokio::spawn`이 `root`를
move하므로 `&Store` 공유를 하려면 불가피하다. **단언은 한 글자도 바뀌지 않으며**,
변경은 `reconcile::run_once(&root, g)` → `reconcile::run_once(&s2, g)`(+ **`Store::clone()`** 캡처)뿐이다.
anti-cheat 정면 지점이므로 diff를 릴리스 게이트에 제시한다.

⚠ **회귀 테스트는 확률적 창을 친다.** r1의 P-3(중대)이 정확히 이걸 때렸다 — **이 테스트는 load-bearing 순서
뮤턴트를 죽이지 못한다.** 그래서 아래 acceptance는 **결정적 배리어 테스트**(훅 기반, T-*)로 뮤턴트를 죽인다.
회귀 테스트는 "증상이 실제로 사라졌다"의 증인일 뿐, **뮤턴트 킬의 증거가 아니다.**

## Increment plan

| id | what | blocked-by | notes |
|---|---|---|---|
| **B-1** | **fix-seam** — `atomic.rs`(stage/commit 분리·`write_atomic` 위임 — **영수증 없음**) · `pins.rs` 신설(**핀 id·코호트·`settled: Notify`** 포함) · `layout.rs` 무덤 이름공간 · `objects.rs`(pin → blob_intact → commit_pointer) · `mod.rs`(`pins` 필드) · `reconcile.rs`(`recover_graves` + `PassGuard::begin` 배선 + `Grave => {}` arm + **D-1 `&Store` 전환** + **`settle_timeout` 인자·`settle_timeout_from`**) · `main.rs`(**cfg에서 파생·주입**) · **호출부 전수 치환**. **GC 삭제/격리 분기는 아직 기존 그대로.** 핀·landed는 **기록되지만 아무도 읽지 않는다**(→ `settle_timeout`도 **아직 아무도 안 본다**) → **관측 행동 플립 0** | none | **first-increment** — structure 게이트가 이 diff를 심사 |
| **B-2** | **the flip** — GC 삭제 분기를 `pre_grave → pass.grave(sha) → settle()`로 교체. `settle()`이 **유한 대기**(landed 확정 → 즉시 / 코호트 드레인 / **`settle_timeout` → fail-CLOSED**) 후 **`landed`만** 보고 판정한다. 무덤이 생기기 시작하므로 `recover_graves`가 실효. `Settled::Restored` = tombstone 유지(D-2)·무카운트. **`Settled::Deferred`**(r3/P-4) = 복원·tombstone 유지·무카운트·`tracing::error!`·**루프 계속** | B-1 | **관측 행동 1개 뒤집기** |
| **B-3** | **위생·관측성·문서만**(행동 무변경) — tracing 필드 · Drop poison 봉인(`unwrap_or_else(into_inner)`) · `shrink_to_fit` · 세 순서 제약의 doc 고정 · `gc_deleted` doc 정정 · **ADR 0002** · **CONTEXT.md Language(Pin / Landed / Grave / Cohort)** · `Store::new` **D-3 doc + 테스트** · **롤백 런북** · **`Graved` 봉인 체크리스트** | B-2 | **D-4: F4(격리) 봉인은 여기서 뺐다 → F-25** |

### 배리어는 프로덕션 코드와 같은 경로를 지난다

배리어는 **`BlobPins`가 소유**한다(`hooks: Hooks`, prod = 전부 `None`). put 경로와 GC 경로가 **같은 등록부의
같은 훅**을 본다 → `#[cfg(test)]` 코드 경로 분기가 **없다**.

```rust
type AsyncHook = Arc<dyn Fn(&str) -> BoxFuture<'static, ()> + Send + Sync>;
type SyncHook  = Arc<dyn Fn(&str) + Send + Sync>;
type FailHook  = Arc<dyn Fn(&str) -> io::Result<()> + Send + Sync>;
#[derive(Clone, Default)]
pub(crate) struct Hooks { post_observe: Option<AsyncHook>, during_collect: Option<AsyncHook>,
                          pre_grave: Option<AsyncHook>, post_grave: Option<AsyncHook>,
                          in_commit_pre_rename: Option<SyncHook>,
                          in_commit_post_landed: Option<SyncHook>,   // r3/P-4 — **T-P4b-1 · T-P4b-2 전용**
                          restore_io: Option<FailHook> }
```

훅 배선은 **실제로 존재한다**: `collect_referenced(layout, &hooks)` · `PinGuard::blob_intact` 끝의
`post_observe` · `commit_pointer` 클로저 **안**의 동기 `in_commit_pre_rename` · **`on_landed` 클로저 안,
`landed` 삽입·notify **직후**의 동기 `in_commit_post_landed`** · GC 루프의 `pre_grave` · `grave()`의
`post_grave` · `settle()`의 `restore_io`.

> **`in_commit_post_landed`가 r3/P-4 개정이 늘린 표면의 전부다**(prod = `None`). 그 park 지점은 **rename `Ok` ∧
> `landed` 삽입 이후 ∧ 핀 drop 이전** — Codex가 T-P4b로 **명시 요구**한 상태이며, 기존 훅 6개 중 **어느 것도**
> 그 지점에 없다. `atomic.rs`는 **무변경**이다(훅은 `pins.rs`가 넘기는 `on_landed` 클로저 안에서 호출된다).
> **r4·r5는 훅을 하나도 더 늘리지 않는다** — `Hooks` 필드는 **7개 그대로**이고, T-P4b-1/T-P4b-2는 이 7개
> (특히 `pre_grave` · `post_grave` · `in_commit_pre_rename` · `in_commit_post_landed`)**만으로** 짜인다.
> **T-P4b-2가 기대는 순서**: `notify_waiters()`가 **`in_commit_post_landed` 훅보다 먼저** 호출된다 →
> 알림이 나간 **뒤에** 클로저가 park하므로 **핀이 살아있는 채로** settlement가 깨어나는지 관측할 수 있다.
> **r5의 랑데부 신호도 프로덕션 표면이 아니다** — 도착 신호는 테스트가 **이 7개 필드에 꽂는 클로저 안에서**
> `tokio::sync::mpsc`로 내보낼 뿐이다(§랑데부 규율). Codex r5: *"**This needs no new production hook or
> fix-model change.**"*

#### 영구 park 훅은 런타임 셧다운을 걸지 않는다 (T-P4a/T-P4b-1/T-P4b-2의 **함정**)

⚠ **tokio는 시작된 `spawn_blocking`을 abort하지 못하고, 런타임 셧다운은 그것들이 끝날 때까지 무한정 기다린다**
(`blocking.rs:107-120` — 이 설계가 **의존하는 바로 그 성질**이다). 따라서 커밋 클로저 안의 동기 훅에서
**영원히** park하면 **테스트 런타임이 drop에서 영영 멈춘다** — 픽스가 옳아도 테스트 바이너리가 hang한다.

**해법(두 테스트 공통)**: park은 **`std::sync::mpsc`의 `recv()`**로 한다. 훅 클로저가 `Mutex<mpsc::Receiver<()>>`를
쥐고, **테스트 함수가 `Sender`를 쥔다**.

```rust
let (tx, rx) = std::sync::mpsc::channel::<()>();
let rx = std::sync::Mutex::new(rx);
let park: SyncHook = Arc::new(move |_sha: &str| { let _ = rx.lock().unwrap().recv(); });
// … 테스트 본문: tx는 **단언이 끝날 때까지 살아 있다** → recv()는 블록 → **핀이 절대 풀리지 않는다**
// (`recv()`는 **동기** 호출이다 — 퓨처가 아니므로 함정 10과 무관하고, 버리는 `Err(RecvError)`가 **곧 해제 신호**다)
//
// **⚠ teardown (r7/P-9)**: tx drop → recv() = Err(RecvError) → 훅 반환 → **클로저 완주**
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
- **⚠ "park 이후 실행되는 코드가 없다"는 *거짓*이다**(r7/P-9) — 위 teardown 블록이 그 증거다. **해제는 재개다.**
- **`Fn` 제약을 만족한다** — `mpsc::Receiver::recv(&self)`는 `&self`를 받는다(`oneshot::blocking_recv(self)`는
  `FnOnce`라 `SyncHook = Arc<dyn Fn(&str)>`에 **들어가지 않는다**).

배리어 테스트는 **`src/store/{reconcile,pins}.rs`의 in-module `#[cfg(test)] mod tests`**에 산다(통합 테스트
크레이트에서는 crate-private 훅이 안 보인다). **결정성의 열쇠**: `pointers_all`의 `SeedRoot`가 첫 `next()`에서
루트를 readdir해 **버킷 목록을 확정**한다(`layout.rs:257-274`) → **패스 시작 시 존재하지 않던 버킷**의 포인터는
그 패스의 `collect_referenced`가 **구조적으로 볼 수 없다.** 워커 yield 순서에 기대지 않는다.

#### ⚠ 삭제 분기 자기검증 — **"이 테스트가 정말 `grave()`까지 갔는가"** (r4/P-5 봉인)

r4가 T-P4b에서 잡은 실패 유형은 **뮤턴트가 아니라 테스트 자신의 결함**이었고, **모든 배리어 테스트에 대해
반복될 수 있다**. 병의 이름을 붙여 둔다:

> **참조됨 분기 누수(referenced-branch leak).** `collect_referenced`는 `PassGuard::begin` 안에서 **블롭 루프보다
> 먼저** 돈다(`reconcile.rs:55` — 오늘도 그렇다). 그러므로 테스트의 put이 **패스 시작 전에** 포인터를 착지시키면
> 그 sha는 **`refs`에 들어가고**, 블롭은 `if refs.contains(&name) { pending.remove(&name); }`로 빠져
> **`pre_grave`도 `grave()`도 `settle()`도 실행되지 않는다.** 테스트는 GC 경로를 **한 줄도 실행하지 않은 채**
> "복원 로그가 없다"는 이유로 RED가 되고 — **봉인을 통째로 제거해도 초록으로 남을 수 있다.**

**규율(모든 배리어 테스트에 예외 없이 적용)**: 각 테스트는 **자신이 삭제 분기에 실제로 들어갔음을
스스로 단언한다.** 두 가지를 **함께** 쓴다(하나는 사전조건, 하나는 사후증거):

1. **`stats.referenced`의 정확한 값을 `assert_eq!`로 못박는다**(`>=`나 `!=` 금지).
   이 값은 `refs.len()` **그 자체**다(`reconcile.rs:56`) → **테스트의 put이 만든 포인터가 스냅샷에 새어
   들어왔다면 값이 1 커진다** → **시끄럽게 깨진다.** 대상 sha X가 스냅샷에 **없어야** 한다는 것이
   이 테스트들의 **전제**이므로, 그 전제를 **단언으로 승격**한다.
2. **`post_grave` 훅으로 "무덤이 실제로 파였다"를 관측한다.** `grave()`는 **blob→무덤 rename이 성공한 뒤에만**
   `post_grave(sha)`를 부른다(§3) → 이 훅이 X를 봤다는 것은 **`Graved`가 태어났다 = 삭제 분기에 들어갔다**의
   **직접 증거**다. 각 테스트는 `Arc<Mutex<Vec<String>>>`에 sha를 모으고 마지막에
   **`graved == vec![X_sha]`**를 단언한다.
   · **새 훅이 아니다** — `post_grave`는 이미 존재하며(`Hooks` 필드 7개 불변) T-B5 ①이 이미 쓰고 있다.
   · 훅은 **기록 + 신호**를 겸할 수 있다(랑데부가 필요한 테스트는 같은 클로저에서 둘 다 한다).

**왜 `gc_deleted`로는 부족한가**: reap 테스트(T-C1/T-C3)는 `gc_deleted == 1`이 곧 삭제 분기의 증거다.
그러나 **복원 테스트**(T-B1/T-B2/T-B4/T-C2/T-P4a/T-P4b-1/T-P4b-2)의 기대값은 **`gc_deleted == 0`**이고,
**참조됨 분기로 샌 경우에도 `gc_deleted == 0`이다** — **두 세계가 구별되지 않는다.** 정확히 이 구멍으로
T-P4b가 빠졌다. `referenced`와 `graved`가 **그 둘을 가른다.**

#### ⚠ 랑데부 규율 — **개시(initiation)는 완료(completion)가 아니다** (r5/P-6 · **r6/P-7** 봉인)

> ### 규칙 0 (규율의 첫 줄) — **비동기 연산의 *개시*를 그것의 *완료*로 착각하지 마라.**
> **`spawn`했다 ≠ 폴링됐다** · **`abort()`했다 ≠ 취소가 끝났다** · **`drop`했다 ≠ Drop이 관측 가능해졌다** ·
> **`send`했다 ≠ 상대가 받았다** · **`timeout`이 `Err` ≠ 안쪽 퓨처가 끝났다(드롭됐을 뿐이다)**.
> **다음 단계로 넘어가려면, 넘어가도 되는 이유를 *관측*해야 한다.** 논증은 근거가 아니다 — **신호가 근거다.**
> **그리고 테스트는 마지막 단언에서 끝나지 않는다 — teardown도 코드다**(r7/P-9).
> 새 배리어 테스트를 쓰는 사람은 **§「개시 ≠ 완료」 클래스 전수 점검의 10개 항목을 1:1로 대조**하라.

이 규칙은 **네 라운드에 걸쳐 다섯 번 물렸다** — 매번 다른 옷을 입고 왔다:

| 라운드 | 변종 | 무엇을 완료로 착각했나 |
|---|---|---|
| **r4/P-5** | *(순서 — 이 클래스가 **아니다**)* | 포인터가 `collect_referenced` **이전에** 착지 → 참조됨 분기 누수(§삭제 분기 자기검증) |
| **r5/P-6** | **`tokio::spawn` ≠ 폴링됨** | *"put을 spawn했다"*를 *"put이 `pin()`했다"*로 착각 → **빈 코호트** |
| **r6/P-7** | **`JoinHandle::abort()` ≠ 취소 완료** | *"abort를 불렀다"*를 *"caller가 소유한 것이 드롭됐다"*로 착각 → **뮤턴트가 GREEN으로 생존** |
| **r7/P-8** | **async 호출 ≠ 폴링된 퓨처** | *"`grave()`를 불렀다"*를 *"무덤이 파였다"*로 착각 → **`let _ =`가 퓨처를 폴링도 않고 버렸다** → 증인이 **아무것도 증명하지 못한다** |
| **r7/P-9** | **park ≠ 영원한 정지** | *"park 이후 실행되는 코드가 없다"*로 착각 → **sender 드롭(= teardown)이 곧 재개**다 → **teardown의 패닉·에러가 조용히 삼켜진다** |

> ⚠ **r7/P-9가 무효화한 면제 사유**: 지난 라운드의 전수 점검은 T-P4a·T-P4b-1·T-P4b-2를
> *"park 이후 실행되는 코드가 없다"*며 함정 5에서 **면제**했다. **거짓이다.** 계획 자신이
> §park 함정에 *"테스트 종료 → `tx` drop → `recv()` = `Err(RecvError)` → 훅 반환 → **클로저 완주**"*라고
> 적어 놓았다. ⇒ **teardown 중에 rename·`landed` 삽입·fsync·`PinGuard::drop`이 실제로 돈다.**
> 그리고 `commit_pointer`는 `spawn_blocking(...).await.expect("join")`으로 끝나므로
> **그 안의 패닉은 put 태스크의 패닉이 된다** — 핸들을 버리면 **초록인 채로 삼켜진다.**
> **면제는 폐기됐다. 모든 핸들은 await된다 — 영구 park도 예외가 아니다**(아래 규율 8).

**r5가 붙인 이름(그대로 유지)** — **spawn ≠ polled**: `tokio::spawn`은 태스크를 **큐에 넣을 뿐 동기적으로
폴링하지 않는다.** 테스트가 spawn 직후 **곧바로** 다음 단계로 넘어가면 GC가 **핀이 생기기도 전에** 무덤을 파고
**빈 코호트**를 캡처해 **즉시 reap**할 수 있다 → 증인이 **셋업 스케줄링 때문에** 실패하거나 — **더 나쁘게는
기대값과 우연히 일치해 조용히 GREEN으로 남는다**(§T-C3: 빈 코호트 reap의 `gc_deleted == 1`이 **정답과 같다**.
**가장 위험한 형태다**).

**r6이 붙인 이름(신규)** — **abort ≠ cancelled**: `JoinHandle::abort()`는 취소를 **스케줄만 한다.**

> `pub fn abort(&self) { self.raw.remote_abort(); }` (`tokio-1.52.3/src/runtime/task/join.rs:227-229`) ·
> `is_finished()`는 *"can return `false` even if `abort` has been called… **the cancellation process may take
> some time**"*(`:231-236`). ⇒ **`abort()`가 반환한 시점에 그 태스크의 퓨처는 아직 드롭되지 않았을 수 있다** —
> 따라서 **그 퓨처가 소유하던 값(예: caller-owned `PinGuard`)도 아직 살아 있을 수 있다.**
> tokio 자신의 doctest가 처방을 보여 준다(`:214-220`):
> `handle.abort(); … assert!(handle.await.unwrap_err().is_cancelled());` — **abort 뒤에 await한다.**

**규율(모든 배리어 테스트에 예외 없이 적용)**:

1. **park하는 훅 클로저는 park하기 *전에* 자신의 도착을 알린다.** 클로저 안의 순서는 반드시
   **`send(arrival)` → `park`**이다(뒤집으면 신호가 **영영 오지 않는다**).
2. **테스트는 다음 단계로 넘어가기 전에 그 도착을 `await`한다.** ⇒ **spawn만 하고 넘어가는 지점이 0개**여야 한다.
   *(JoinHandle을 **완주까지** await하는 것은 spawn 지점이 **아니다** — 완주가 곧 도착이다.)*
3. **⚠ (r6/P-7 — 신규) `abort()` 뒤에는 반드시 그 `JoinHandle`을 유한 타임아웃으로 await하고
   `JoinError::is_cancelled()`를 단언한다.** 그 await가 **반환한 뒤에야** 다음 단계로 간다.
   **취소 완료 = 그 퓨처가 드롭됐다 = 그 퓨처가 소유하던 가드·락이 드롭됐다.** 이것을 관측하지 않으면:
   · **뮤턴트가 경합으로 살아남는다**(T-C2 — caller-owned 가드가 **아직 안 죽어서** GC가 코호트로 잡는다) ·
   · **테스트가 hang한다**(T-B5① — 아직 안 죽은 `PassGuard`가 **`pass_lock`을 쥐고 있어** 새 패스가 못 들어간다).
   ⚠ **취소 완료는 `spawn_blocking` 클로저의 종료를 뜻하지 **않는다*** — 시작된 blocking 태스크는 **abort 불가**이고
   `JoinHandle` 드롭은 **detach일 뿐**이다(`blocking.rs:107-120`). **그 비대칭이 바로 T-C2가 증명하려는 것이다.**
4. **모든 park은 해제 경로를 갖거나, "끝까지 잡아 둔다"가 *의도*임을 명시한다**(§park 함정 — 테스트가 `tx`를
   쥔 채 끝나고 unwind가 풀어 준다. 그 park의 **도착 신호는 여전히 필수**다: 도착을 확인해야 *"핀이 살아 있다"*가
   **단언**이 된다).
5. **"해제를 *언제* 하는가"도 관측 대상이다.** 해제 시점이 *"settle이 **대기에 들어간 뒤**"*여야 하는
   테스트(**T-B4 · T-C2 · T-C3 · T-P4b-2**)는 `post_grave`의 도착(`graved_reached`)을 await한 뒤
   **pending 프로브**(`timeout(200ms, &mut gc)` = **`Err`**)로 그 상태를 **관측하고서** 해제한다.
   ⚠ **너무 일찍 해제하면 코호트-대기 뮤턴트가 *경합으로* 살아남는다** — 해제된 put이 mutant-settle의 `landed`
   첫 검사보다 **먼저** 착지하면 그 뮤턴트도 **Restore**를 내고 **GREEN**이 된다.
   ⚠ **프로브는 반드시 `&mut gc`로 건다**(값으로 넘기면 `timeout`이 `Err`일 때 **`JoinHandle`이 드롭돼** GC가
   detach된다 — 함정 6). `&mut JoinHandle`은 `Future + Unpin`이므로 **빌림만 드롭되고 태스크는 그대로 산다.**
6. **⚠ (r6 — 신규) 해제 `send()`의 반환은 "훅이 재개했다"가 아니다.** 해제 직후에는 **어떤 상태도 단언하지 않는다** —
   반드시 **다음 관측 가능한 사건**(도착 신호 · put 완주 · GC 완주)까지 await한 뒤에 단언한다.
   T-P4b-2가 정확히 그렇게 한다(`park_A` 해제 → **`post_landed_reached` await** → 단언).
7. **⚠ (r6 — 신규) 버려진 `JoinHandle`은 패닉을 조용히 삼킨다.** 완주를 await하는 모든 핸들은
   **`JoinError`를 언랩**하고(패닉 = 즉시 RED) **안쪽 `Result`까지 단언**한다(`let _ = h.await;` **금지**).
   ⚠ **(r7/P-9 — 개정) "의도적으로 await하지 않는 핸들"이라는 면제는 *폐기됐다*.** **모든** `JoinHandle`은
   **await된다.** 영구 park된 태스크는 **teardown에서** await한다(규율 8).
8. **⚠ (r7/P-9 — 신규) 영구 park의 *해제*도 안무의 일부다 — teardown에서 실제 코드가 돈다.**
   테스트가 `tx`를 쥔 채 끝나는 park(§park 함정)은 *"영원히 멈춘다"*가 **아니다**: `tx`가 드롭되는 **그 순간**
   `recv()`가 `Err(RecvError)`로 풀리고 **훅이 반환하며 커밋 클로저가 완주한다**(rename → `landed` 삽입 →
   `notify_waiters()` → fsync → `PinGuard::drop` → `spawn_blocking`의 **`.await.expect("join")`**).
   ⇒ **패닉·에러가 그 자리에서 태어난다.** 핸들을 버리면 **테스트는 초록이고 아무도 모른다.**
   **규율(T-P4a · T-P4b-1 · T-P4b-2에 예외 없이 적용)**:
   ① **핸들을 보유한다**(`let put = tokio::spawn(…)` — `let _ = …` 금지) ·
   ② **영구 stall 증인 단언을 *전부* 마친다**(먼저 해제하면 핀이 drop되고 포인터가 착지해 **시나리오 자체가
      사라진다**) · ③ **park sender를 *명시적으로* 드롭한다**(`drop(tx);` — **스코프 종료에 기대지 않는다**.
      암묵적 드롭은 **핸들을 await할 기회 없이** 해제를 일으킨다) ·
   ④ **유한 타임아웃으로 핸들을 await하고 `JoinError`와 안쪽 `put()` 결과를 *둘 다* 언랩한다**
      (`timeout(5s, put).await.expect("put must finish after park release").expect("put task must not panic")`
      → `Ok` 단언).
   *(패닉으로 인한 RED에서는 unwind가 `tx`와 핸들을 함께 드롭하므로 teardown await에 **도달하지 않는다** —
   RED는 여전히 **hang이 아니라 깔끔한 실패**다. §park 함정 그대로.)*
9. **⚠ (r7/P-8 — 신규) async 호출은 그 자체로 아무 일도 하지 않는다 — 폴링되어야 일어난다.**
   `let _ = pass.grave(&sha);`는 **rename을 수행하지 않는다** — **폴링되지 않은 퓨처를 드롭할 뿐이다.**
   `#[must_use]`도 `let _ =`가 **삼켜 버린다.** ⇒ **모든 async 표현식은 `.await`되고, 그 결과는 단언된다**
   (`Result`는 `expect`/`assert`로, 값은 검사로). **"부작용을 노리고 호출한 async 함수"는 반드시 await한다.**
   *(clippy에 `let_underscore_future` 린트가 있지만 — **계획 문서의 스니펫은 clippy가 읽지 않는다.**
   코드가 되기 전에 죽여야 한다. 그것이 이 전수 점검이 1차 방어선인 이유다.)*

**신호 채널 — 전부 테스트 쪽이다. 프로덕션 훅은 하나도 늘지 않는다**(`Hooks` 필드 **7개 불변**; 신호는 테스트가
그 7개 필드에 꽂는 **클로저 안**에서만 산다):

| 용도 | 채널 | 왜 이것이어야 하는가 |
|---|---|---|
| **도착** (sync·async 훅 **공통**) | `tokio::sync::mpsc::unbounded_channel::<String>()` — 훅이 `tx.send(sha)`, 테스트가 `rx.recv().await` | `send(&self)`가 **논블로킹 · 런타임 컨텍스트 불필요** → **blocking 클로저 안에서도 안전**하고 `Fn`의 `&self` 제약도 만족한다(`oneshot::Sender::send(self)`는 `self`를 소비해 `Fn`에 **못 들어간다**). ⚠ **테스트 쪽은 반드시 `await`로 기다린다** — `std::sync::mpsc::recv()`로 기다리면 **current-thread 런타임이 그 자리에서 멈춰** spawn된 put이 **영영 폴링되지 않는다**(P-6을 **직접 재현**하는 자충수) |
| **해제 — sync 훅** (`in_commit_pre_rename` · `in_commit_post_landed`) | `std::sync::mpsc` + `recv()` (§park 함정 그대로) | 여기는 **blocking 클로저 안**이므로 **블로킹이 옳다**(async park은 표현조차 안 된다). `recv(&self)`라 `Fn` 제약도 만족 |
| **해제 — async 훅** (`pre_grave` · `post_grave` · `post_observe` · `during_collect`) | `Arc<tokio::sync::Notify>` — 훅이 `notified().await`, 테스트가 **`notify_one()`** | `notified()`는 `&self`(`Fn` OK) · **`notify_one()`은 대기자가 없어도 permit을 저장한다** → **lost wakeup 불가**(테스트가 먼저 깨워도 안전). ⚠ **`notify_waiters()`를 쓰지 마라** — 그건 permit을 **저장하지 않는다**(프로덕션 `settled`가 그것을 쓸 수 있는 이유는 `await_settlement`가 **검사 이전에** `enable()`로 등록하기 때문이다. 테스트 훅에는 그 보증이 없다) · ⚠ **`oneshot`도 쓰지 마라** — `Receiver::await`가 `self`를 **소비**하므로 `Fn` 훅에 **들어가지 않는다** |
| **⚠ 취소 완료** (**r6/P-7 — 신규**) | `tokio::time::timeout(2s, &mut handle).await` → **`Ok(Err(e))`** ∧ **`e.is_cancelled()`** | **`abort()`는 스케줄만 한다**(`join.rs:227-229`). 이 await가 반환해야 **퓨처가 드롭됐다 = caller가 소유하던 가드·락이 드롭됐다**가 **확정**된다. tokio doctest와 **동형**(`join.rs:214-220`). ⚠ **`is_cancelled()` 단언이 패닉 탐지기도 겸한다** — 태스크가 abort 이전에 **패닉**했다면 `is_panic()`이라 이 단언이 **RED**가 된다(함정 5) |

**체크리스트 — "모든 park에 도착 신호가 있다 · 모든 abort에 취소 완료 await가 있다 · **모든 핸들이 (teardown을
포함해) await된다** · **모든 async 퓨처가 폴링된다**"**. **다음 개정이 이 표의 행을 지우지 않고서는 안무를
약화시킬 수 없다.** (park·abort가 **없는** 테스트는 그 사실 자체를 적어 둔다 — "확인 안 함"과 "확인했고 없음"을
구별한다.)

| 테스트 | park 지점 (훅) | **도착 신호** | 해제 | **취소 완료 await** (r6/P-7) | **teardown await** (r7/P-9) | **async 퓨처 폴링** (r7/P-8) | **테스트가 다음 단계 이전에 await하는 것** (spawn-후-진행 = **0**) |
|---|---|---|---|---|---|---|---|
| **T-B1** | `during_collect` (GC) | `collect_reached` | `Notify` | — (abort 없음) | **—** 잔여 태스크 0(해제·완주 모두 본문에서 끝난다) | ✔ 전부 `.await` | GC **spawn** → **`collect_reached` await**. put은 **spawn하지 않는다 — 완주를 await**한다(→ **`Ok` 단언**) → 그 다음 해제 |
| **T-B2** | `pre_grave` (GC) | `gc_at_pre_grave` | `Notify` | — (abort 없음) | **—** 잔여 태스크 0 | ✔ 전부 `.await` | GC **spawn** → **`gc_at_pre_grave` await** → *그제서야* putter 시작 → **putter 완주 await**(→ **`Ok` 단언** · 핀 drop) → 해제 |
| **T-B4** | `post_observe` (put) · `post_grave` (GC, **기록+신호**) | `observed` · `graved_reached` | `Notify` · 없음(통과) | — (abort 없음) | **—** 잔여 태스크 0(put·GC·무관한 put **전부** 완주 await) | ✔ 전부 `.await` | put **spawn** → **`observed` await** → GC **spawn** → **`graved_reached` await** → **pending 프로브(`&mut gc`)** → put 해제 → **put 완주 await**(→ **`Ok` 단언**) → `timeout(5s, gc)` |
| **T-C1** | **없음** (동시 put 0) | — | — | — | **—** park·spawn 0 | ✔ 전부 `.await` | **spawn 지점 0.** put은 reconcile **이전에** 완주(`Err`)한다 → 함정이 **구조적으로 없다** |
| **T-C2** | `in_commit_pre_rename` (put) | `pre_rename_reached` | `std::sync::mpsc` | **✅ 필수 — `abort()` → `timeout(2s, &mut put)` = `Ok(Err(e))` ∧ `e.is_cancelled()`** | ⚠ **await할 핸들이 구조적으로 없다** — abort가 커밋 클로저를 **detach**시킨다(**그것이 이 테스트의 명제다**). **대리 관측**: GC의 **Restore + 포인터 실재 + blob 존재**가 *"클로저가 rename·`landed` 삽입까지 완주했다"*를 증명한다. **잔여(정직)**: 착지 **이후**(fsync·핀 drop)의 패닉만 **미관측** — 아래 §새로 찾은 것 3번 | ✔ 전부 `.await` | put **spawn** → **`pre_rename_reached` await** → abort → **⚠ 취소 완료 await** → *그제서야* GC **spawn** → **`graved_reached` await** → **pending 프로브(`&mut gc`)** → 해제 → `timeout(5s, gc)` |
| **T-C3** | `in_commit_pre_rename` (put) | `pre_rename_reached` | `std::sync::mpsc` | — (abort 없음) | **—** 잔여 태스크 0(해제 후 **put 완주 await**로 본문에서 닫는다) | ✔ 전부 `.await` | put **spawn** → **`pre_rename_reached` await**(⚠ **없으면 조용히 GREEN** — 아래 박스) → GC **spawn** → **`graved_reached` await** → **pending 프로브(`&mut gc`)** → 해제 → **put 완주 await**(→ **`Err(Internal)` 단언** · 핀 drop) → `timeout(5s, gc)` |
| **T-P4a** | `in_commit_pre_rename` (put) | `pre_rename_reached` | **teardown에서만**(영구 park) | — (abort 없음) | **🔧 P-9 — 필수.** 단언 ①~⑤ **전부** 끝난 뒤 → **`drop(tx)`**(명시) → **`timeout(5s, put)`** → **`JoinError` 언랩** + 안쪽 **`Ok` 단언**. *"park 이후 코드가 없다"는 면제는 **거짓**이었다* | ✔ 전부 `.await` | put **spawn**(핸들 **보유**) → **`pre_rename_reached` await** → *그제서야* `timeout(5s, run_once_at(…))` ×3 → 단언 → **teardown** |
| **T-P4b-1** | `pre_grave` (GC) · `in_commit_post_landed` (put) | `gc_arrived` · `landed_reached` | **`Notify`** · **teardown에서만**(영구 park) | — (abort 없음) | **🔧 P-9 — 필수.** 단언 ①~⑤ 뒤 → **`drop(tx_put)`** → **`timeout(5s, put)`** → **`JoinError` 언랩** + **`Ok` 단언**(해제 시 fsync·핀 drop이 **실제로 돈다**) | ✔ 전부 `.await` | GC **spawn** → **`gc_arrived` await** → put **spawn**(핸들 **보유**) → **`landed_reached` await** → `pre_grave` 해제 → `timeout(2s, gc)` → 단언 → **teardown** |
| **T-P4b-2** | `pre_grave` (GC) · `in_commit_pre_rename`(`park_A`) · `in_commit_post_landed`(`park_B`) | `gc_arrived` · `pre_rename_reached` · `post_landed_reached` | `Notify` · `std::sync::mpsc`(6단계) · **teardown에서만**(`park_B`) | — (abort 없음) | **🔧 P-9 — 필수.** 단언 ①~⑤ 뒤 → **`drop(tx_B)`** → **`timeout(5s, put)`** → **`JoinError` 언랩** + **`Ok` 단언**(`tx_A`는 6단계에서 이미 해제됐다) | ✔ 전부 `.await` | GC **spawn** → **`gc_arrived` await** → put **spawn**(핸들 **보유**) → **`pre_rename_reached` await** → `pre_grave` 해제 → **`graved_reached` await** → pending 프로브(**`&mut gc`**) → `park_A` 해제 → **`post_landed_reached` await** → `timeout(2s, gc)` → 단언 → **teardown** |
| **T-B5 ①**(취소) | `post_grave` (GC) | `graved_reached` | **없음** — abort가 곧 해제(park한 async 훅째로 드롭된다) | **✅ 필수 — `abort()` → `timeout(2s, &mut gc)` = `Ok(Err(e))` ∧ `e.is_cancelled()`** | **—** teardown에 재개할 park이 **없다**(park은 abort로 **드롭**됐다 · in-flight `spawn_blocking` **0** — rename은 `post_grave` **이전에** 반환했다) | ✔ 전부 `.await` | GC **spawn** → **`graved_reached` await** → abort(⚠ 도착 전에 abort하면 **무덤이 안 파여** `.gc-grave-*` 단언이 **엉뚱한 이유로** RED) → **⚠ 취소 완료 await**(**없으면 `PassGuard`가 `pass_lock`을 쥔 채라 새 `run_once`가 hang한다**) → *그제서야* 디스크 단언 + 새 `run_once` |
| **T-B5 ②③** | **없음** (동시 put 0) | — | — | — | **—** park·spawn 0 | ✔ 전부 `.await` | **spawn 지점 0** — 전부 순차 await |
| **T-B5 ④**(`Graved` 누수) | **없음** | — | — | — | **—** park·spawn 0 | **🔧 P-8 — `pass.grave(&sha)`를 `.await`한다**(`let _ = pass.grave(..)`는 **폴링되지 않은 퓨처를 드롭**해 **rename이 아예 일어나지 않았다**) | **spawn 지점 0.** `grave().await` → **복구 이전 디스크 단언** → **`drop(graved)`**(settle 없음 = 누수) → **`drop(pass)`**(명시 — 안 하면 다음 패스가 `pass_lock`에서 **hang**한다 · 함정 4) → 복구 패스 |
| **T-Q2 · T-Q3** | **없음** (동시 put 0) | — | — | — | **—** park·spawn 0 | ✔ 전부 `.await` | **spawn 지점 0** |

> **왜 T-C3가 가장 위험했는가**(정직하게 적는다): 위 표가 없었다면 T-C3는 **조용히 무해해질 수 있었다.**
> put이 폴링되기 전에 GC가 무덤을 파면 **코호트가 비고** → `settle()`이 **첫 검사에서 `Drained`** → `landed` 없음
> → **Reap** → **`gc_deleted == 1`** — 이것은 **T-C3가 기대하는 바로 그 값**이다. 즉 **테스트는 GREEN인데
> "겹치는 실패 put"이라는 시나리오는 한 번도 재현되지 않는다.** r2/P-2가 명시 요구한 증인이 **아무것도 지키지
> 않는 채 초록으로 남는다.** (4번의 pending 단언이 *때때로* 잡아 주지만, 그건 **경합에 기댄 보조 단언**이다 —
> 규율은 **결정성**이어야 한다.)

#### ⚠ 「개시 ≠ 완료」 클래스 전수 점검 (r6/P-7 · **r7/P-8·P-9** — **한 테스트가 아니라 클래스를 쓸었다**)

r5(P-6)·r6(P-7)·r7(P-8·P-9)은 **같은 질병의 변종들**이다. 다음 변종이 또 다른 옷을 입고 오지 못하도록,
**10개 함정 항목**을 정의하고 **모든 배리어 테스트와 1:1로 대조**했다. **"이전 라운드에서 safe 판정"은 이
렌즈로는 근거가 아니다** — 전부 다시 봤다. **r7에서 항목 2개가 늘었고**(**9 teardown** · **10 async 폴링**),
**지난 라운드의 면제 사유 하나가 무효화됐다**(*"park 이후 실행되는 코드가 없다"* — **거짓**).

**함정 10개 (이 저장소에서의 구체적 표현)**

| # | 함정 | 무엇을 완료로 착각하나 | 이 저장소에서 |
|---|---|---|---|
| **1** | `tokio::spawn(...)` 후 **도착 신호 없이 진행** | 큐 삽입 = 실행 | 핀이 생기기 전에 GC가 **빈 코호트**를 캡처 → **즉시 reap**(r5/P-6) |
| **2** | `JoinHandle::abort()` 후 **취소 완료 await 없이 진행** | 스케줄 = 드롭 완료 | caller-owned `PinGuard` 뮤턴트가 **아직 안 죽어** 코호트에 잡힌다 → **뮤턴트 생존**(r6/P-7) |
| **3** | `JoinHandle` **드롭(detach) 후 그 태스크의 상태를 가정** | 드롭 = 취소 | **시작된 `spawn_blocking`은 abort 불가이며 detach될 뿐 계속 실행된다**(`blocking.rs:107-120`) — 이 설계가 **의존하는** 성질이다 |
| **4** | `drop(guard)`/`drop(store)` 후 **효과가 관측 가능해지기 전에** 진행 | 드롭 호출 = 효과 발생 | `PinGuard::drop`(live 제거 + notify) · `PassGuard::drop`(**`pass_lock` 해제**) — 후자를 안 기다리면 **다음 패스가 hang한다** |
| **5** | `JoinHandle`을 **await하지 않고 버려** 패닉·에러가 **조용히 삼켜진다** | 태스크 종료 = 성공 | 배리어 테스트가 **패닉을 못 보고** "흔적 없음"을 **엉뚱한 이유로** 관측한다 |
| **6** | `tokio::time::timeout(...)`이 `Err`일 때 **안쪽 퓨처가 어떻게 되는지 가정** | 타임아웃 = 안쪽이 끝났다 | **드롭될 뿐이다.** `run_once_at`을 드롭하면 `PassGuard`가 → **`pass_lock`이 풀린다**. `timeout(_, gc_handle)`을 **값으로** 넘기면 `Err`일 때 **핸들이 드롭돼 GC가 detach**된다 |
| **7** | **채널 send/recv의 완료를 가정** | `send` 반환 = 상대가 받음 | 해제 `send()` 반환은 **훅이 재개했다는 뜻이 아니다.** `Notify::notify_waiters()`는 **permit을 저장하지 않는다**(대기자 0이면 유실). `oneshot::Receiver::await`는 `self`를 **소비**해 `Fn` 훅에 **못 들어간다** |
| **8** | **파일시스템 연산의 가시성 가정** | fsync = 관측 가능 | rename `Ok` ⇒ **즉시 가시**(핵심 사실 C — 논증됨) · `SeedRoot` 스냅샷은 **가시성이 아니라 시점**의 문제(§결정성의 열쇠) |
| **9** | **⚠ (r7/P-9 — 신규) park된 태스크의 *teardown*에서 실패가 나는데 핸들을 await하지 않는다** | park = 영원한 정지 (*"이후 실행되는 코드가 없다"*) | **`tx` 드롭 = 재개다.** `recv()`가 `Err(RecvError)`로 풀리고 커밋 클로저가 **rename·`landed` 삽입·notify·fsync·`PinGuard::drop`을 완주**한다. `commit_pointer`는 **`spawn_blocking(…).await.expect("join")`** 으로 끝나므로 **그 안의 패닉은 put 태스크의 패닉**이 된다 → 버려진 핸들이 **삼킨다** → **테스트는 초록** |
| **10** | **⚠ (r7/P-8 — 신규) async 표현식의 결과를 *await 없이* 버린다** | 호출 = 실행 | **`let _ = pass.grave(&sha);` 는 rename을 하지 않는다** — **폴링되지 않은 퓨처를 드롭**할 뿐이다. `#[must_use]`도 `let _ =`가 **삼킨다**. ⇒ 무덤이 **아예 없고**, blob은 **멀쩡하며**, `recover_graves`가 **깨져 있어도 GREEN**이다. **증인이 아무것도 증명하지 못한다** |

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
> **caller-owned 뮤턴트에서는 취소가 곧 핀 사망이다** — T-C2는 정확히 그 차이를 관측한다. **그러므로 T-C2는
> "취소가 완료됐다"를 반드시 기계로 확정해야 한다**(함정 2). 그러지 않으면 **두 세계가 구별되지 않는다.**

**전수 매트릭스** — `—` 구조적 부재(확인했고 없음) · `✔` 해당하나 **이미 봉인됨**(근거) ·
**🔧 r6** 지난 라운드에 걸려서 고친 것 · **🔧 r7** **이번 라운드에 걸려서 고친 것** · **⚠** 남겨 둔 잔여(근거 명시)

| 테스트 | 1 spawn | 2 abort | 3 detach | 4 drop 효과 | 5 삼킨 패닉 | 6 timeout Err | 7 채널 | 8 FS 가시성 | **9 teardown** | **10 async 폴링** |
|---|---|---|---|---|---|---|---|---|---|---|
| **T-B1** | ✔ r5 | — | — | ✔ 보조정리 L | **🔧 r6** | ✔ | ✔ | ✔ | **—** 잔여 태스크 0 | ✔ |
| **T-B2** | ✔ r5 | — | — | ✔ 보조정리 L | **🔧 r6** | ✔ | ✔ | ✔ | **—** 잔여 태스크 0 | ✔ |
| **T-B4** | ✔ r5 | — | ✔ 프로브 `&mut` | ✔ 보조정리 L | **🔧 r6** | ✔ | ✔ | ✔ | **—** 잔여 태스크 0 | ✔ |
| **T-C1** | — | — | — | ✔ 보조정리 L | ✔ | ✔ | — | ✔ | **—** park·spawn 0 | ✔ |
| **T-C2** | ✔ r5 | **🔧 r6 (P-7 본체)** | ✔ **비대칭이 명제** | ✔ (핀이 **안** 죽어야 한다) | ✔ `is_cancelled()`가 겸함 | ✔ | ✔ | ✔ | ⚠ **잔여** — 핸들이 **구조적으로 없다**(abort = detach = **명제 그 자체**). 대리 관측으로 rename·`landed`까지 봉인 · **fsync 이후 패닉만 미관측**(3번) | ✔ |
| **T-C3** | ✔ r5 | — | — | **🔧 r6** | **🔧 r6** | ✔ | ✔ | ✔ | **—** 잔여 태스크 0(put 완주 await) | ✔ |
| **T-P4a** | ✔ r5 | — | ✔ **보유**(드롭 안 함 → detach **없음**) | — | **🔧 r7** — *"의도적 미await"* **면제 무효** | **🔧 r6 (거짓 논증)** | ✔ | ✔ | **🔧 r7 (P-9 본체)** `drop(tx)` → `timeout(5s, put)` → 언랩 ×2 | ✔ |
| **T-P4b-1** | ✔ r5 | — | ✔ **보유** | — | **🔧 r7** — 면제 무효 | ✔ | **🔧 r6 (`oneshot`)** | ✔ | **🔧 r7 (P-9)** `drop(tx_put)` → `timeout(5s, put)` → 언랩 ×2 | ✔ |
| **T-P4b-2** | ✔ r5 | — | ✔ **보유** | — | **🔧 r7** — 면제 무효 | ✔ 프로브 `&mut` | ✔ **모범** | ✔ | **🔧 r7 (P-9)** `drop(tx_B)` → `timeout(5s, put)` → 언랩 ×2 | ✔ |
| **T-B5 ①** | ✔ r5 | **🔧 r6 (P-7 2차)** | ✔ | ✔ (`pass_lock`) | ✔ | ✔ | ✔ | ✔ | **—** park이 abort로 **드롭**됐다 · in-flight blocking 0 | ✔ |
| **T-B5 ②** | — | — | — | ✔ (아래 8번) | ✔ | ✔ | — | ✔ | **—** park·spawn 0 | ✔ |
| **T-B5 ③** | — | — | — | — | ✔ | ✔ | — | ✔ | **—** park·spawn 0 | ✔ |
| **T-B5 ④** | — | — | — | **🔧 r6 (`drop(pass)`)** | ✔ | ✔ | — | ✔ | **—** park·spawn 0 | **🔧 r7 (P-8 본체)** — `grave()`를 **await**한다 |
| **T-Q2 · T-Q3** | — | — | — | — | ✔ | ✔ | — | ✔ | **—** park·spawn 0 | ✔ |
| **회귀**(`tests/regression_…`) | ⚠ **의도적 확률 창** | — | — | — | ✔ **이미 올바름** | — | — | ✔ | **—** park 0 | ✔ **이미 올바름** |
| **adversarial**(characterization) | ⚠ 배리어 아님 | — | — | — | ⚠ **기존 계약** | — | — | ✔ | **—** park 0 | ✔ **폴링은 된다** — `let _ = run_once(…).await`는 **await가 있다**(결과만 버린다) ⇒ **함정 5**이지 **함정 10이 아니다** |

**새로 찾은 것 — 전부 (r6: 🔧 6건 + ⚠ 2건 · **r7: 🔧 4건 + ⚠ 1건**)**

> ### r7에서 새로 찾은 것 (P-8 · P-9 · 두 클래스 전수 재점검)
>
> 1. **T-B5④ / 함정 10 — P-8 본체(critical).** `let _ = pass.grave(&sha)`는 **async fn을 호출만 하고
>    퓨처를 폴링 없이 드롭**한다 ⇒ **blob→무덤 rename이 아예 일어나지 않는다.** 따라서 `drop(pass)` 후의
>    복구 패스는 **원래의 멀쩡한 blob**을 발견하고, **`recover_graves`가 통째로 삭제돼 있어도** 테스트가
>    **GREEN**이다. *"fail-CLOSED by construction"*의 증인이 **아무것도 증명하지 못했다.**
>    **수정**: `grave()`를 **await**하고 **복구 이전 디스크 상태를 단언**한 뒤 `Graved`를 **버린다**(→ §T-B5④).
> 2. **T-P4a · T-P4b-1 · T-P4b-2 / 함정 9 — P-9 본체(high).** 지난 라운드의 전수 점검이 이 셋을
>    *"park 이후 실행되는 코드가 없다"*며 함정 5에서 **면제**했다. **그 면제가 틀렸다** — 계획 자신이
>    §park 함정에 *"테스트 종료 → `tx` drop → `recv()` = `Err` → 훅 반환 → **클로저 완주**"*라고 적어 놓았다.
>    ⇒ **teardown에서 rename·`landed` 삽입·fsync·`PinGuard::drop`·`expect("join")`이 전부 실행된다.**
>    패닉이나 `Err`가 나도 **버려진 핸들이 삼킨다** → **초록.** **수정**: 핸들을 **보유** → 단언 **전부** 완료 →
>    **명시적 `drop(tx)`** → **`timeout(5s, put)`** → **`JoinError` + 안쪽 `put()` 결과 둘 다 언랩**(→ `Ok`).
>    ⚠ **이것은 "전수 점검이 없애겠다던 바로 그 함정"이었다** — 점검표가 **면제 사유를 발명해** 스스로를 통과시켰다.
>    **교훈**: *"코드가 없다"*는 **논증**이다. 규칙 0이 금지하는 바로 그것이다. **신호(= await된 핸들)가 근거다.**
> 3. **⚠ T-C2 / 함정 9 — 잔여(구조적 · 고치지 않는다 · 기록한다).** abort된 put의 커밋 클로저는
>    **detach**돼 있고(그것이 **T-C2의 명제 그 자체**다 — 함정 3), 테스트에는 **await할 핸들이 남아 있지 않다.**
>    · **어디까지 관측되는가**: 해제 후 클로저가 rename·`landed` 삽입·`notify_waiters()`까지 갔다는 것은
>      **GC의 Restore + `get_bytes` Ok + `.objects/<sha>` 존재**가 **증명한다**(그 경로 없이는 settle이
>      `Landed`를 볼 수 없다) — **대리 관측이 실재한다.**
>    · **잔여**: 그 **이후**(fsync · `PinGuard::drop`)에서 나는 패닉은 **미관측**이다. 이 잔여를 없애려면
>      **핸들이 필요한데, 핸들을 없애는 것이 이 테스트의 시나리오다** ⇒ **구조적으로 제거 불가.**
>      새 훅으로 관측하는 것은 **프로덕션 표면을 늘리는 일**이므로 **하지 않는다**(설계 동결).
>      **"확인 안 함"이 아니라 "확인했고, 알고 남긴다."**
> 4. **함정 10 전수 — 다른 곳에는 없다(확인했고 없음).** `let _`을 **전수로 훑었다**(`docs/` 스니펫 + `src/` + `tests/`):
>    · **설계 스니펫**: `pass.grave(sha).await?.settle().await?` · `hooks.post_grave(sha).await` ·
>      `rename_durable(…).await?` · `blob_intact(…).await` · `commit_pointer(…).await` — **전부 await된다.**
>      `hooks.restore_io(&sha)?`는 **동기** fallible이고 `?`로 전파된다.
>    · **`src/`**: `let _ = tokio::fs::remove_file(…).await`(`objects.rs:77,88` · `health.rs:20`) ·
>      `let _ = tokio::signal::ctrl_c().await`(`main.rs:14`) — **전부 `.await`가 붙어 있다**(폴링됨. 버리는 것은
>      **결과**뿐이며 **의도된 best-effort**다) → **함정 10 해당 없음.**
>    · **`tests/adversarial.rs:29,33,91`**: 셋 다 `.await`가 있다 → **폴링됨.** `:91`의 결과 무시는
>      **함정 5(기존 계약 · B-1 무변경)**로 이미 기록돼 있다 — **함정 10이 아니다.**
>    · **park 훅의 `let _ = rx.lock().unwrap().recv();`**: `recv()`는 **동기 blocking 호출**이지 퓨처가 아니다
>      → 함정 10 **무관**. 버리는 `Err(RecvError)`가 **곧 해제 신호**다(**의도**).
>    ⇒ **`let _ = <async>`로 퓨처를 흘리는 곳은 T-B5④ *하나뿐*이었다.**
> 5. **함정 9 전수 — 다른 곳에는 없다(확인했고 없음).** teardown에 살아남는 태스크·park이 있는 테스트는
>    **T-P4a · T-P4b-1 · T-P4b-2**(영구 park put)와 **T-C2**(detach된 클로저)뿐이다. 나머지 전 테스트는
>    **모든 park을 본문에서 해제하고 모든 핸들을 본문에서 완주 await**한다(T-B1·T-B2·T-B4·T-C3·T-B5①) 또는
>    **park·spawn이 아예 없다**(T-C1·T-B5②③④·T-Q2·T-Q3·회귀·adversarial) ⇒ **teardown에 재개될 코드가 없다.**

**r6에서 찾은 것 — 전부 (🔧 6건 + ⚠ 2건)**

1. **T-C2 / 함정 2 — P-7 본체.** `abort()` 뒤 **취소 완료를 기다리지 않고** GC를 spawn했다.
   **수정**: `abort()` → **`timeout(2s, &mut put)` = `Ok(Err(e))` ∧ `e.is_cancelled()`** → *그 다음에* GC spawn.
   (→ **§T-C2**)
2. **T-B5① / 함정 2 — P-7의 두 번째 사례(같은 병, 다른 테스트).** reconcile 퓨처를 `abort()`한 **직후** 디스크를
   단언하고 **새 `run_once`를 시작**한다. 취소가 아직 완료되지 않았다면 **`PassGuard`가 `pass_lock`을 쥔 채**이므로
   새 패스가 **`lock_owned().await`에서 막힌다** → 라이브니스 timeout에 걸려 **엉뚱한 이유로 RED**(또는 hang).
   **수정**: `abort()` → **취소 완료 await(`is_cancelled()`)** → *그 다음에* 디스크 단언 + 새 패스.
   ⚠ **이것이 "클래스를 쓸어라"의 배당금이다** — Codex는 T-C2만 지적했지만 **같은 함정이 T-B5①에도 있었다.**
3. **T-B5④ / 함정 4 — `drop(pass)` 누락.** `Graved`를 흘린 뒤 **`PassGuard`가 살아
   있으면 `pass_lock`을 쥔다** → 다음 `run_once`가 **hang**한다. **스코프 종료에 기대지 말고 명시적 `drop(pass)`.**
   ⚠ **그러나 r6은 그 옆의 더 큰 결함을 놓쳤다** — r6이 채택한 `let _ = pass.grave(..)`는 **퓨처를 폴링도 하지
   않는다**(→ **r7/P-8**, 함정 10). r6은 *"`Graved`를 흘린다"*를 **의도**로 읽었지만, 그 코드는 **`Graved`를
   만들지도 않았다.** **같은 줄을 두 라운드가 다른 렌즈로 봤고, 두 번째 렌즈가 더 나쁜 것을 찾았다.**
4. **T-P4a / 함정 6 — 뮤턴트 RED 논증이 *거짓*이었다(정정).** r3안은 무한대기 뮤턴트에서 *"`pass_lock`을 쥔 채이므로
   후속 패스도 전부 막힌다 — 단언 ③도 함께 죽는다"*고 적었다. **거짓이다**: `timeout`의 `Err`는 **안쪽 퓨처를
   드롭**하고, 그 드롭이 `PassGuard`를 → **`pass_lock`을 해제**한다. 후속 패스는 **막히지 않는다** — 대신 **스스로
   같은 이유로 hang한다**(핀이 여전히 park돼 있다). **RED 신호가 2개라는 결론은 유지되지만 *메커니즘이 다르다*.**
   **단언은 한 글자도 바꾸지 않았다** — 틀린 것은 *왜 RED인가*의 설명이었다.
5. **T-P4b-1 / 함정 7 — `oneshot` park은 `Fn` 훅에 들어가지 않는다.** 2단계가 `gc_park`을 *"해제 가능한
   대기(oneshot)"*라고 적었는데, `AsyncHook = Arc<dyn Fn(&str) -> BoxFuture<'static, ()>>`는 **`Fn`**이고
   `oneshot::Receiver::await`는 **`self`를 소비**한다 → **컴파일되지 않는다.** §랑데부 규율의 채널 표와도 **모순**
   이었다. **수정**: **`Arc<Notify>` + `notify_one()`**(permit 저장 → lost wakeup 불가).
6. **T-B1/T-B2/T-B4/T-C3 / 함정 5 — 버려진 핸들이 패닉을 삼킨다.** 완주를 await하는 모든 핸들에 대해
   **`JoinError` 언랩 + 안쪽 `Result` 단언**을 **의무화**했다(T-B1/T-B2/T-B4 = `Ok` · T-C3 = `Err(Internal)`).
   특히 **T-C3**는 함정 4까지 걸렸다 — 해제 후 **put 완주를 await하지 않아** *"핀 drop · `landed` 무흔적"*이
   **관측이 아니라 기대**였다. **수정**: 해제 → **put 완주 await(`Err(Internal)` 단언)** → `timeout(5s, gc)`.
7. **⚠ 회귀 테스트 / 함정 1 — 확률적 창은 *의도*다.** `tokio::spawn(reconcile)` 후 **도착 신호 없이
   `sleep(PUT_DELAY)`**로 창을 넓힌다(`multi_thread` 런타임 · 재현 20/20). **이 규율의 적용 대상이 아니다** —
   적용하면 확률적 창이 사라져 **증상 재현 자체가 불가능해진다.** 계획은 이미 *"회귀 테스트는 뮤턴트 킬의
   증거가 아니다"*라고 선언했고(§Regression test), **그 선언이 이 예외의 값을 지불한다.**
   **함정 5는 이미 올바르다**: `rec.await.unwrap().unwrap()`(JoinError + `io::Result` **둘 다** 언랩) ·
   put 핸들도 `h.await.unwrap()`. ⚠ **B-1의 기계 치환이 이 언랩들을 지워서는 안 된다**(치환은 `&root` → `&s2` 뿐).
8. **⚠ `tests/adversarial.rs` / 함정 5 — 기존 계약(무변경).** 루프가 `let _ = reconcile::run_once(…).await;`로
   **io 에러를 삼킨다**(`adversarial.rs:91`). **배리어 증인이 아니라 characterization**이고 B-1은 그 줄의
   **인자만** 치환한다 → **행동 무변경 계약상 손대지 않는다.** 그러나 **이 렌즈로 보면 결함이므로 기록해 둔다**
   ("확인 안 함"이 아니라 "확인했고, 알고 남긴다"). 핸들 자체는 `rec.await.unwrap()`으로 언랩되므로 **패닉은 잡힌다.**
   **T-B5② / 함정 4**도 같은 칸에서 답한다: `drop(store)`는 **디스크에 아무 효과가 없고**(`PassGuard::drop`은
   디스크 무접촉) `BlobPins`는 `Arc` 공유라 **클론이 살아 있으면 등록부가 죽지도 않는다** → ②는 그 어느 효과에도
   **의존하지 않는다**. ②가 쓰는 것은 **디스크에 놓인 무덤**과 **새 `Store::new`가 새 등록부를 만든다**는 사실
   (= D-3의 해저드를 **의도적으로** 재시작 시뮬레이션에 쓴다)뿐이다.

**걸리지 않은 것의 근거(요약 — "확인했고 없음")**: T-C1·T-B5②③④·T-Q2·T-Q3에는 **`spawn`·`abort`·park가 아예
없다**(전부 순차 await) → 함정 1·2·3이 **구조적으로 표현 불가**다. T-P4b-2는 **해제 후 `post_landed_reached`를
await**하므로 함정 6·7의 **모범 사례**다(다른 테스트가 이 형태를 따른다). 함정 8은 **핵심 사실 C**(rename `Ok` ⇒
즉시 가시)와 **`SeedRoot` 스냅샷 성질**로 전 테스트에서 이미 논증돼 있다 — **이 라운드에서 새로 걸린 곳은 없다.**

### 호출부 전수 (B-1의 기계 치환 대상)

`git grep -n 'run_once' -- src tests`로 **재검증한 실측**이다:

| 파일 | 라인 | 비고 |
|---|---|---|
| `src/main.rs` | `:35`, `:48` | 부트 1회 + 주기 루프 |
| `src/store/reconcile.rs` | `:21` | `run_once` → `run_once_at` **내부 위임** |
| `src/store/reconcile.rs` (유닛 테스트) | `:175`, `:194`, `:196`, `:211`, `:212`, `:227`, `:241`, `:246` | `run_once`×2 + `run_once_at`×6 = **8곳 / 테스트 함수 5개**. **5개 중 4개는 `Store`를 아예 안 만든다** → `Store::new(root.to_path_buf())` 생성 추가 필요 |
| `tests/layout_tree.rs` | `:71`, `:137`, `:198` | 골든 트리 3종 |
| `tests/adversarial.rs` | `:91` | |
| `tests/regression_reconcile_gc_dedup_race.rs` | `:150` | 회귀 |

**합계 15곳**(+ 정의 2곳: `:20`, `:43`). 최종안 초안이 "**13곳**"이라 적은 것은 **오산이다** — reconcile 유닛
테스트를 *테스트 함수 수*로 세다 *호출 라인 수*와 섞었다. 위 표가 정본이다(grep 재현 가능).

> **r3/P-4 — 같은 15곳이 `settle_timeout` 인자도 함께 받는다**(치환은 **한 번**에 끝난다 — B-1의 기계 치환에
> 흡수된다). 값: `main.rs` = `settle_timeout_from(cfg.upload_timeout)` · **테스트** = 시나리오가 요구하는 명시값
> (골든/adversarial/회귀는 **발화하지 않을 넉넉한 값**, T-P4a는 **200ms**, **T-P4b-1·T-P4b-2는 30s**).
> **기본값을 숨긴 편의 오버로드를 만들지 않는다** — 이 값이 **유일한 상계**이므로 호출자가 **알고 정해야** 한다.

> ### ⚠ **D-3 함정 — 여기서 미끄러지면 영구 RED다**
>
> `regression:148-151`과 `adversarial.rs:88-95`는 `root.clone()`을 `tokio::spawn`에 넘긴다. 이걸
> **`Store::new(root)`로 재구성하면 등록부가 갈라진다** — spawn된 reconcile이 **다른 Store의 put을 절대 보지
> 못한다** → 핀도 landed도 안 보임 → **회귀 테스트가 영구 RED**로 남고, 원인은 프로덕션 버그가 아니라
> 테스트 배선이다(디버깅에 하루를 태울 자리다).
>
> **반드시 `let s2 = (*s).clone();`로 같은 `Store`를 클론해 넘겨라.** `layout_tree.rs:198`(mid-flight)도
> 동일한 `s`를 써야 한다. 이 함정은 D-3(같은 root에 `Store` 둘 = 버그 부활)의 **테스트 코드 판본**이다.

### B-1 acceptance (**플립 0**)

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
- [ ] 신규 유닛(reconcile — **r3/P-4**): `settle_timeout_from(600s) == 660s`(= `upload_timeout + GC_SETTLE_MARGIN`)
      ∧ **파생이 단조**(`upload_timeout`을 올리면 `settle_timeout`도 오른다) — 운영자가 `FILES_UPLOAD_TIMEOUT`을
      올렸을 때 **정상적으로 느린 put이 타임아웃되지 않음**을 못박는다(정상 경로 연기 = 0 유지)
- [ ] **T-C1 — 두 번째 플립 회귀 가드**(이 증분에서 이미 걸 수 있다): `b/k.meta.json` 위치에 **디렉터리**를 심어
      `rename`을 결정적으로 EISDIR 실패시킨다 → `put()` = `Err(Internal)` ∧ `landed` **무흔적** ∧ (만료·미참조
      blob에 대해) `run_once_at` → **`gc_deleted == 1`**
      · **뮤턴트 킬**: `on_landed`를 rename **앞**으로 이동(= 개정 1차안의 `arm()`) → 흔적 발생 → Restore →
      `gc_deleted == 0` → **결정적 RED**. (ENOSPC 무한연기 = flip FATAL-1의 기계 증인)
      · **랑데부(r5)**: **park 0 · spawn 0.** put은 reconcile **시작 전에** 완주(`Err`)하고 그 핀은 **이미 죽어
        있다** → **spawn ≠ polled 함정이 구조적으로 없다**(§랑데부 규율 체크리스트). *"확인 안 함"이 아니라
        "확인했고 없음"이다.*
      · ⚠ **r2/P-2가 정확히 지적한 T-C1의 한계**: 이 테스트는 **실패한 put이 이미 반환되고 그 핀이 죽은 뒤에**
        reconcile을 돌린다 → **겹치는(overlapping) 실패 put**을 전혀 재현하지 못한다. 그 창의 증인은
        **T-C3**(B-2)이며, T-C1은 `landed` 흔적의 **위치**만 지킨다. 두 테스트는 **다른 것을 지킨다** —
        T-C1을 P-2의 증인으로 제시하지 **않는다**
- [ ] **D-3 테스트**: `store.clone()`은 등록부 공유(`Arc::ptr_eq`) ∧ 같은 root의 `Store::new` 2개는
      **공유하지 않음**을 단언 — 해저드를 테스트로 못박는다
- [ ] `cargo clippy -D warnings` (미사용 훅 필드 0 — 훅은 전부 실제 호출부가 있다)

### B-2 acceptance (**유일한 플립**)

- [ ] `tests/regression_reconcile_gc_dedup_race.rs` **GREEN 20/20**
- [ ] `cargo test` **105 green** (골든 stats·골든 트리 **비트 동일**), `tests/adversarial.rs` 40객체 불변
- [ ] **T-B1 — put이 참조 수집 도중 완료** (r1 P-3이 요구한 배리어 ①):
      **셋업(r4에서 명시)**: ① 만료·미참조 blob **X**(정상 put → 포인터 삭제 → tombstone 만료) ·
      ② **디코이 객체 D**(다른 내용 · 포인터 **살아 있음**). **D는 장식이 아니라 필수다** — `during_collect`는
      **포인터를 1개 낼 때마다** 발화하므로(§6), 포인터가 **하나도 없으면 훅이 영영 발화하지 않아** 랑데부가 걸린다.
      **랑데부(r5 — 도착/해제 쌍)**: `during_collect` = **도착 `collect_reached` 송신 → `Notify` park**.
      **단계 순서**: ⓐ GC를 **spawn** → ⓑ **`collect_reached`를 await**(= 패스가 `collect_referenced` **안에**
      있음이 확정된다 — **여기서 기다리지 않으면** putter의 포인터가 `SeedRoot`의 루트 readdir **이전에** 착지해
      `refs`에 새어 든다 = r4 결함의 재발) → ⓒ **그 park 동안** putter를 **spawn하지 않고 완주까지 await**한다
      (**패스 시작 시 존재하지 않던 버킷** `fresh`에 **X와 같은 내용**으로 put → dedup 분기(바이트 재기록 없음)
      → 커밋 → 핀 drop) → ⓓ `Notify`로 해제 → ⓔ `timeout(5s, gc)` = `Ok`.
      ⇒ **spawn만 하고 넘어가는 지점 0개**(putter는 spawn조차 하지 않는다 — 완주 await가 곧 도착이다).
      · **함정 5 (r6 — 버려진 핸들이 패닉을 삼킨다)**: GC의 `JoinHandle`은 `timeout(5s, gc)` → **`JoinError`
        언랩**(패닉 = 즉시 RED) → **`io::Result` 언랩**까지 간다(`let _ = gc.await;` **금지**). putter는 태스크가
        아니라 **직접 await**하므로 `JoinError`가 없다 — 대신 **`put()`의 `Ok`를 단언**한다(put이 실패하면
        `landed`가 서지 않아 **엉뚱한 이유로 RED**가 된다). **함정 4**: putter 완주 = **핀 drop 확정**(보조정리 L).
      단언: `stats == ReconcileStats{referenced:1, gc_deleted:0, gc_pending:1, temps_deleted:0, quarantined:0}`
      ∧ `get_bytes("fresh","v.bin").is_ok()` ∧ **무덤 잔재 0**.
      · **삭제 분기 자기검증(r4)**: **`referenced == 1`** = **D 하나뿐**이다. putter의 포인터가 스냅샷에 새어
        들어왔다면 **2**가 된다 → **참조됨 분기 누수를 시끄럽게 잡는다**(`SeedRoot` 성질이 바뀌면 여기서 깨진다).
        ∧ **`graved == vec![X_sha]`**(`post_grave` 훅 관측 = 무덤이 **실제로 파였다** → 삭제 분기 진입 증명).
        `gc_pending == 1`은 X의 tombstone이 **복원 뒤에도 유지**됨(D-2)을 함께 못박는다
      · **뮤턴트 M1 `enter_pass()`를 `collect_referenced` 뒤로** → put 착지 시 `pass_live=false` → 흔적 0,
      refs에도 없음 → Reap → `get_bytes` 404 → **RED**
      · **뮤턴트 `PassGuard::drop`의 `landed.clear()` 제거** → 관측 동일(GREEN) = **equivalent 뮤턴트**로
      정직하게 분류(다음 패스가 시작 시 clear한다)
- [ ] **T-B2 (개정 — r2/P-3이 요구한 "사전확인↔무덤 rename 창"의 결정적 증인)**:
      GC를 **모델링된 사전확인 지점**(= `pre_grave` 훅)에서 park한다. **그 park 동안**, putter가 **비로소 시작**해
      X를 dedup 관측(`blob_intact == true` — 무덤은 아직 없다)하고 **완전히 착지**한다(JoinHandle await → 핀 drop,
      포인터 on-disk). 그 다음 GC 재개 → `grave()` → `settle()`.
      · **랑데부(r5 — 도착/해제 쌍)**: `pre_grave` = **도착 `gc_at_pre_grave` 송신 → `Notify` park**.
        **단계 순서**: ⓐ GC **spawn** → ⓑ **`gc_at_pre_grave` await** → ⓒ *그제서야* putter 시작 →
        **완주까지 await**(핀 drop · 포인터 on-disk) → ⓓ 해제 → ⓔ `timeout(5s, gc)` = `Ok`.
        · **함정 4·5 (r6)**: putter는 **완주까지 await**된다(spawn하든 직접 await하든 — **완주가 곧 도착**이므로
          spawn 지점이 **아니다**). 그 완주가 **핀 drop을 확정**한다(**보조정리 L**) ⇒ *"무덤 시점 코호트는 비어
          있다"*는 이 테스트의 전제가 **논증이 아니라 관측**이 된다. **`put()`의 `Ok`를 단언**하고, 태스크로
          spawn했다면 **`JoinError`도 언랩**한다(버려진 핸들은 **패닉을 삼킨다**). GC 핸들은 **`JoinError` +
          `io::Result` 둘 다 언랩**한다.
        ⚠ **ⓑ를 빼면(= spawn 직후 곧바로 putter)** 패스가 아직 폴링되지 않았을 수 있고, 그러면 putter가 `fresh`
        버킷을 **`SeedRoot`의 루트 readdir 이전에** 만들어 **포인터가 `refs`에 새어 든다** → **참조됨 분기 누수
        (r4 결함)가 이 테스트에서 재발한다.** 아래 "구조적으로 먼저"라는 논증은 **패스가 실제로 시작한 뒤에만**
        성립한다 — **`gc_at_pre_grave`가 그 전제를 기계로 못박는다.**
      · **결정성의 근거(시간에 기대지 않는다)**: `PassGuard::begin`의 `collect_referenced`는 `pre_grave`보다
        **구조적으로 먼저** 끝난다 → putter의 포인터는 **`refs`에 절대 들어갈 수 없다**. putter는 패스 시작 시
        존재하지 않던 버킷 `fresh`에 쓴다(`SeedRoot` 성질, `layout.rs:257-274`) → 이중 보증.
      · **삭제 분기 자기검증(r4)**: 위 두 줄은 **논증**일 뿐이다 — 이제 **단언으로 승격**한다:
        **`stats.referenced == 0`**(패스 시작 시 포인터가 **하나도 없다** — X의 포인터는 tombstone 시드 때
        지웠다. putter의 포인터가 스냅샷에 새면 **1**이 된다) ∧ **`graved == vec![X_sha]`**(`post_grave` 관측).
      · **정상**: 무덤 시점 **코호트는 비어 있다**(핀이 이미 drop됐다) → 대기 0 → 그러나 `landed ∋ sha` →
        **Restore** → `get_bytes` Ok(**바이트까지 비교**), `gc_deleted == 0`, 무덤 잔재 0
      · **뮤턴트 ① `landed` 삽입 삭제**(또는 `PinGuard::drop`에서 `landed.remove`) → 코호트도 비고 `landed`도
        비었다 → Reap → 404 → **RED**
      · **뮤턴트 ② 사전확인(load-bearing)** — `pins.rs`에 손으로 lock-and-peek을 추가해 **`pre_grave` 시점에**
        보호 여부를 판정하고 미보호면 무덤 없이 즉시 reap: 그 시점엔 putter가 **아직 시작조차 안 했다** →
        `live`도 `landed`도 **비어 있다** → 미보호 판정 → **Reap** → 그 사이 putter가 dedup으로 착지 →
        **포인터 + blob 부재** → 404 → **RED**
      · **왜 개정 전 T-B2는 이 뮤턴트를 못 죽였나**(r2/P-3의 정확한 지적): 개정 전에는 putter가 `post_observe`에서
        park돼 있었으므로 `pre_grave` 시점에 이미 **`live`에 있었다** → 어떤 사전확인이든 "보호됨"을 보고
        rename을 건너뛰었고 → blob이 살아남아 **GREEN이 유지됐다**. 개정판은 putter의 **시작 자체를** `pre_grave`
        park **안쪽으로** 옮겨 그 창을 **비운다**
      · ⚠ **그런데 이 뮤턴트가 "컴파일 불가"라면 왜 테스트가 필요한가?**(정직한 답) — 컴파일 불가한 것은
        **`settle()`을 `grave()` 앞으로 옮기는 재배치**뿐이다(`Graved` 없이는 `settle`을 호출할 방법이 없다).
        `pins.rs`를 **편집해** 새 lock-and-peek 코드를 **추가**하는 것은 재배치가 아니라 **새 API 추가**이며
        **컴파일된다**. 봉인은 **모듈 경계**이지 타입 마법이 아니다 → **T-B2가 2차 방어선**이다.
        또한 타입은 "Restore가 **반드시** 일어난다"는 **양성 방향**을 강제하지 못한다 — 그건 오직 테스트가 한다
- [ ] **T-B4 — 관측 후·커밋 전 park (코호트 대기 킬)**: putter를 `post_observe`에서 park(intact=true, 핀 live,
      미커밋) → GC 패스를 그 사이에 실행 → 무덤 시점 코호트 = {그 핀} → `settle()`이 **대기에 들어간다** →
      putter 해제 → 착지 → 핀 drop → settle 깨어남 → `landed ∋ sha` → **Restore 필수** → `get_bytes` Ok
      · **랑데부(r5 — 도착/해제 쌍)**: `post_observe` = **도착 `observed` 송신 → `Notify` park** ·
        `post_grave` = **기록(`graved`) + 도착 `graved_reached` 송신**(park 없음 — 통과한다).
        **단계 순서**: ⓐ put **spawn** → ⓑ **`observed` await**(⚠ **여기서 기다리지 않으면** put이 아직 폴링되지
        않아 **핀이 없을 수 있다** → 무덤 시점 **코호트가 비고** → `settle()`이 첫 검사에서 `Drained` → `landed`
        없음 → **Reap** → `get_bytes` 404 → **엉뚱한 이유로 RED**) → ⓒ GC **spawn** → ⓓ **`graved_reached` await**
        (= 코호트가 **{그 핀}으로 확정된 뒤**임을 못박는다 — **이 순서가 M4 뮤턴트를 죽이는 힘의 원천이다**:
        더 일찍 해제하면 put이 무덤 **이전에** 착지해 `landed`가 서고, M4 뮤턴트도 **Restore**로 살아남는다) →
        ⓓ′ **대기 진입 프로브**: `timeout(200ms, &mut gc)` = **`Err`**(pending) — *"settle이 실제로 대기에
        들어갔다"*를 **관측**한다(T-C3·T-P4b-2와 **같은 규율**. 이것 없이 해제하면 M4 뮤턴트가 **경합으로
        살아남을 수 있다**: 해제된 put이 mutant-settle의 `landed` 첫 검사보다 **먼저** 착지하면 Restore가 되어
        **GREEN**이다) → ⓔ putter 해제 → **put 완주 await**(핀 drop) → ⓕ `timeout(5s, gc)` = `Ok`.
      · **함정 3·4·5·6 (r6)**: ⓓ′의 프로브는 **반드시 `&mut gc`**로 건다 — 값으로 넘기면 `Err`일 때
        **`JoinHandle`이 드롭돼 GC가 detach**된다(함정 6; `&mut JoinHandle`은 `Future + Unpin`이라 **빌림만
        드롭되고 태스크는 그대로 산다**). ⓔ의 put 핸들은 **`JoinError` 언랩 + `Ok` 단언**까지 간다(함정 5) —
        그 완주가 **핀 drop을 확정**한다(**보조정리 L** · 함정 4). ⚠ **데드락 부재 sanity의 "무관한 put"도
        spawn이 아니라 완주 await**로 건다(`timeout(5s, put_other)` = `Ok`).
      · **삭제 분기 자기검증(r4)**: putter는 `post_observe`(= **커밋 이전**)에 park돼 있으므로 GC의
        `collect_referenced` 시점에 **포인터가 디스크에 없다** → **`stats.referenced == 0`**을 단언한다
        (누수하면 **1**) ∧ **`graved == vec![X_sha]`**(`post_grave` 관측).
      · **뮤턴트 M4 — 코호트 대기 제거**(`settle`이 즉시 `landed`만 본다): 판정 시점에 putter는 아직 park 중 →
        `landed` 비었음 → **Reap** → 해제된 putter가 dedup으로 커밋(바이트 재기록 없음) → **포인터 + blob 부재**
        → 404 → **RED**
      · **뮤턴트 M4' — 코호트를 무덤 rename 시점이 아니라 `settle` 진입 시점에 스냅샷**: 관측 동일(GREEN) =
        **equivalent 뮤턴트**로 정직하게 분류(이 테스트에서는 두 시점 사이에 새 핀이 없다). **T-C3가 이 차이를
        가른다**(거기서는 무덤 이후 핀이 등장하지 않으므로 마찬가지 — 정직히 말해 **이 뮤턴트는 어느 테스트도
        죽이지 못한다.** 코호트를 늦게 뜨면 **더 많이 기다릴 뿐** 안전 측이므로 결함이 아니다. 무덤 **직후**로
        고정하는 이유는 **성능**(자급자족 핀을 안 기다림)이지 안전이 아니다 — §대기의 상계에 그대로 적어 뒀다)
- [ ] **T-C2 — 커밋 도중 호출자 취소** (crash 렌즈 FATAL의 결정적 증인 · **r6/P-7로 안무 개정**):
      put을 spawn → `in_commit_pre_rename`(blocking 클로저 **내부** 동기 훅)에서 park → **바깥 퓨처를
      abort**(= `upload_timeout` 시뮬레이션) → **⚠ 그 취소가 *완료*될 때까지 await하고 `JoinError::is_cancelled()`를
      단언**(r6/P-7 — **`abort()`는 스케줄만 한다**) → *그 다음에* GC 패스 실행 → 무덤 시점 코호트 = {그 핀}
      (가드는 **클로저 소유**이므로 취소가 **완료된 뒤에도 살아 있다** — 뮤턴트에서는 **죽어 있다**) →
      `settle()`이 **대기** → 훅 해제 → 클로저가 rename·마킹·fsync·drop 완주
      → settle 깨어남 → `landed ∋ sha` → **Restore** → `get_bytes` Ok ∧ blob 존재 ∧ 무덤 잔재 0
      · **랑데부 (r5 도착/해제 쌍 + ⚠ r6/P-7 취소 완료 await)**: `in_commit_pre_rename` = **도착
        `pre_rename_reached` 송신 → `std::sync::mpsc` park**(= `park_A`) · `post_grave` = **기록 + 도착
        `graved_reached`**.
        **최종 단계 순서 (r6 — spawn-후-진행 0 · abort-후-진행 0)**:
        ⓐ put을 **spawn**(`let mut put = tokio::spawn(async move { s2.put(b,"cancelled",X_bytes).await })`) →
        ⓑ **`pre_rename_reached` await** — 이 시점 **확정 사실**: `blob_intact == true`(dedup) ∧ `stage` 성공 ∧
           **blocking 클로저가 *시작*됐다** ∧ 핀 live ∧ `landed` 무흔적 ∧ 포인터 부재 →
        ⓒ **`put.abort()`** (= `upload_timeout` 발화 · 클라이언트 disconnect 시뮬레이션) →
        ⓓ **⚠ 취소 *완료*를 await한다 (r6/P-7 — 이 한 줄이 이번 라운드의 전부다)**:
           **`let e = timeout(2s, &mut put).await.expect("abort must complete").expect_err("put must be cancelled");`**
           ∧ **`assert!(e.is_cancelled())`** →
        ⓔ *그제서야* GC **spawn** → ⓕ **`graved_reached` await** → ⓖ **대기 진입 프로브**
           `timeout(200ms, &mut gc)` = **`Err`**(pending — T-B4·T-C3·T-P4b-2와 **같은 규율**) →
        ⓗ `tx_A`로 `park_A` **해제** → ⓘ `timeout(5s, gc)` = **`Ok`**(⚠ 해제 직후에는 **아무것도 단언하지 않는다** —
           함정 6: 해제 `send()`의 반환은 *"클로저가 재개했다"*가 아니다. **GC 완주가 그 관측이다**).
      · ⚠ **ⓑ와 ⓓ는 서로 다른 것을 증명한다 — 둘 다 없으면 이 테스트는 아무것도 봉인하지 못한다.**
        · **ⓑ = "blocking 클로저가 *시작*됐다".** 도착 **이전에** abort하면 클로저가 **시작조차 하지 않을 수 있고**
          (`join.rs:189-193`: *"The exception is if the task has not started running yet; in that case, calling
          `abort` may prevent the task from starting"*) → 핀이 **caller 퓨처와 함께 즉시 소멸** → *"가드는 클로저가
          소유하므로 취소에도 살아 있다"*는 **명제가 검증되지 않은 채** 테스트만 RED가 된다(빈 코호트 → Reap → 404).
          **시작된 `spawn_blocking`은 abort 불가**라는 성질은 **시작한 뒤에만** 성립한다.
        · **ⓓ = "호출자 취소가 *완료*됐다".** `abort()`는 취소를 **스케줄만 한다**(`join.rs:227-229`
          `remote_abort()`; `:231-236` *"the cancellation process may take some time"*). ⓓ 없이 GC를 spawn하면
          **caller-owned 뮤턴트에서 가드가 아직 살아 있을 수 있고**, 그러면 GC가 **그 가드를 코호트로 포착**해
          settlement를 park했다가 → 풀린 클로저가 포인터를 착지시키면 → **복원** → **뮤턴트가 GREEN으로 생존한다.**
          `graved_reached`·pending 프로브는 **GC의 상태**를 증명할 뿐 **취소 완료를 증명하지 않는다.**
          ⇒ **ⓓ가 반환한 뒤에야 "caller가 소유하던 것은 전부 드롭됐다"가 확정**되고, 그제서야 **두 세계가 갈린다.**
        · ⚠ **ⓓ는 blocking 클로저의 *종료*를 뜻하지 않는다** — 그것은 **detach된 채 `park_A`에서 계속 살아 있다**
          (`blocking.rs:107-120`: 시작된 blocking 태스크는 **abort 불가**, `JoinHandle` 드롭은 **detach일 뿐**).
          **이 비대칭이 T-C2의 명제 그 자체다**(함정 3). 그래서 ⓓ의 await는 **막히지 않는다**: 바깥 태스크는
          안쪽 blocking `JoinHandle`을 **드롭(detach)**하고 즉시 취소로 완료된다 — 클로저를 기다리지 **않는다**.
        · ⚠ **`is_cancelled()` 단언은 패닉 탐지기도 겸한다**(함정 5): put이 abort 이전에 **패닉**했다면
          `is_panic()`이므로 이 단언이 **RED**가 된다 → *"흔적이 없다"*를 **엉뚱한 이유로** 관측하는 일이 없다.
      · **올바른 코드에서 무슨 일이 벌어지는가**: ⓒⓓ로 **바깥 퓨처는 죽었지만** `PinGuard`는 `commit_pointer`가
        `spawn_blocking`을 부른 순간 **클로저 안으로 이동했다**(`let me = self;`) → 바깥 퓨처는 **`JoinHandle`만
        들고 있었고** 그것의 드롭은 **detach**다 ⇒ **핀은 살아 있다.** GC는 무덤을 파고 **코호트 = {그 핀}**을
        캡처 → `settle()`이 **대기에 들어간다**(ⓖ가 관측) → ⓗ 해제 → 클로저가 rename·`landed` 삽입·
        `notify_waiters()`·fsync·**drop(핀)**을 완주 → settlement가 깨어나 `landed ∋ sha` → **Restore**.
        **단언**: `get_bytes(b,"cancelled")` = **`Ok`** ∧ **바이트 동일** ∧ `.objects/<sha>` **존재** ∧
        `.gc-grave-<sha>` **부재** ∧ `gc_deleted == 0`.
      · **삭제 분기 자기검증(r4)**: put은 `park_A`(= **rename 이전**)에 있으므로 `collect_referenced` 시점에
        **포인터가 없다** → **`stats.referenced == 0`** ∧ **`graved == vec![X_sha]`**
      · **뮤턴트 ① "가드를 클로저로 옮기지 않고 caller가 보유"**(= 개정 1차안 · **이 테스트의 표적**) —
        **어떻게 죽는가**: 가드가 **바깥 퓨처의 지역변수**다 → ⓒ의 abort가 그 퓨처를 드롭하면 **가드도 드롭된다** →
        **ⓓ가 그 드롭이 *끝났음*을 기계로 확정한다**(취소 완료 = 퓨처 드롭 완료 = **가드 드롭 완료**) ⇒ ⓔ에서
        GC가 무덤을 팔 때 **코호트가 비어 있다** → `settle()`이 첫 검사에서 **`Drained`** → `landed` 무흔적 →
        **Reap**(`gc_deleted == 1`) → ⓗ 해제 후 detach된 클로저가 rename을 **뒤늦게 착지**시킨다 →
        **포인터 존재 ∧ blob 부재** → `get_bytes` **404** → **RED**.
        ⚠ **ⓓ가 없으면 이 뮤턴트는 경합으로 살아남는다** — 가드 드롭이 무덤 rename **이후**로 밀리면 코호트에
        그 핀이 잡히고, settle이 park했다가 착지 후 **복원**해 버린다 → **GREEN**. **ⓓ가 그 경합을 제거한다.**
        (독립 RED 신호 **2개**: `gc_deleted == 1` ∧ `get_bytes` 404 — 프로브 ⓖ도 함께 깨진다: 빈 코호트라
        settle이 **대기하지 않으므로** `timeout(200ms, &mut gc)`가 **`Ok`**가 된다 ⇒ **3개**)
      · **뮤턴트 ② `commit_pointer`를 `tokio::fs` async 체인으로 되돌림**(= 취소 가능한 커밋) → 바깥 퓨처가
        rename 체인을 **소유**한다 → ⓒⓓ의 취소가 **rename을 통째로 취소**하거나(포인터 없음 → Reap → 그러나
        착지도 없다 → **404 아님**) 아니면 `tokio::fs::rename`의 내부 `spawn_blocking`이 **뒤늦게 착지**한다
        (`fs/mod.rs:312`) → 그때는 **핀이 이미 죽었으므로**(가드가 바깥 퓨처 소유) 뮤턴트 ①과 **동일하게**
        Reap → 뒤늦은 착지 → **404** → **RED**
      · **뮤턴트 ③ 코호트 대기 제거**(`settle`이 즉시 `landed`만 본다) → 판정 시 `landed` 비었음(put은 `park_A`)
        → **Reap** → 해제된 클로저가 뒤늦게 착지 → 포인터 + blob 부재 → **404** → **RED**
        (ⓖ의 pending 프로브도 **`Ok`**가 되어 함께 깨진다)
- [ ] **T-C3 — 겹치는 실패 put의 결정적 증인 (r2/P-2가 명시 요구 — `live`가 보호 술어가 아님을 못박는다)**:
      1. 만료·미참조 blob X를 심는다(정상 put → 포인터 삭제 → tombstone 만료). `b/poisoned.meta.json` **위치에
         디렉터리를 심어** 커밋 rename을 **결정적으로 EISDIR 실패**시킬 준비를 한다(T-C1과 동일한 기법).
      2. `put(b, "poisoned", X_bytes)`를 **spawn** → `blob_intact == true`(dedup) → `stage` 성공 →
         **`in_commit_pre_rename` 훅에서 park**(park **이전에** 도착 `pre_rename_reached`를 송신한다).
      2′. **⚠ `pre_rename_reached`를 `await`한다 — 여기서 기다리지 않으면 이 테스트는 *조용히* 무의미해진다**
         (r5/P-6). `tokio::spawn`은 **폴링을 보장하지 않는다** → put이 아직 `pin()`도 못 했는데 3번이 무덤을
         파면 **코호트가 비고** → `settle()`이 첫 검사에서 **`Drained`** → `landed` 없음 → **Reap** →
         **`gc_deleted == 1`** — 이것은 **6번이 기대하는 바로 그 값이다.** ⇒ 테스트는 **GREEN인데** *"겹치는
         실패 put"* 시나리오는 **한 번도 재현되지 않고**, "`live`를 보호 술어로 되돌리는" 뮤턴트도 **살아남는다**
         (핀이 없으니 되돌려도 Reap이다). **이 도착 신호가 T-C3의 킬 파워 전부를 지탱한다.**
         이 시점 확정 사실: **핀 live · 미착지(`landed` 무흔적) · 포인터 부재**.
      3. GC(`run_once_at`)를 spawn → `pre_grave` 통과 → **무덤 rename** → 코호트 = {그 핀} →
         **`settle()`이 대기에 들어간다**.
      4. **대기 진입의 증인**: `post_grave` 훅의 도착 신호(`graved_reached`)를 **await한 뒤**
         `timeout(200ms, &mut gc_handle)`이 **Err(pending)** 임을 단언한다.
         *(보조 단언 — 주 단언은 아래 6번이며 시간에 의존하지 않는다. ⚠ 이 pending 단언은 2′의 **대체재가
         아니다**: 2′가 없으면 이 단언은 **경합에 기대게 되고**, 통과할 때조차 **우연**이다.)*
      5. 훅을 해제 → 클로저의 `rename(tmp → b/poisoned.meta.json)`이 **EISDIR로 실패** → `on_landed`는
         **절대 호출되지 않는다** → `commit_pointer` = `Err` → `put` = **`Err(Internal)`** → **핀 drop** →
         `notify_waiters()` → **`landed` 무흔적**.
      5′. **⚠ put 핸들을 완주까지 await한다 — `timeout(5s, put)` = `Ok(Err(AppError::Internal))`** (r6 — 함정 4·5).
         · **함정 4**: 해제 `send()`의 반환은 *"클로저가 재개했다"*가 아니다. **완주 await만이**
           *"핀이 drop됐다 · 코호트가 드레인됐다 · `landed`가 비었다"*를 **관측**으로 만든다(**보조정리 L**).
           그것 없이 6번의 판정을 기대하는 것은 **GC가 알아서 깨어나기를 바라는 것**이다 — 결과는 같지만
           **테스트가 그 사실을 보지 못한다.**
         · **함정 5**: `JoinError`를 **언랩**한다 → put 태스크가 **패닉**했다면 **즉시 RED**다.
           언랩하지 않으면 패닉으로 인한 `landed` 무흔적을 **EISDIR 때문이라고 오독**하고 **GREEN**이 된다.
         · **단언은 `Err(Internal)`이어야 한다** — `Ok`면 EISDIR 셋업이 깨진 것이고 시나리오가 재현되지 않았다.
      6. `settle()`이 깨어나 판정 → `landed(X)` = **false** → **Reap**.
         **주 단언**: **`gc_deleted == 1`** ∧ `.objects/<sha>` **부재** ∧ `.objects/.gc-grave-<sha>` **부재** ∧
         `get_bytes(b,"poisoned")` **404**(포인터 무흔적).
      · **삭제 분기 자기검증(r4)**: put은 `in_commit_pre_rename`에 park돼 있고 그 rename은 **끝내 EISDIR로
        실패**하므로 포인터는 **한 번도 존재하지 않는다** → **`stats.referenced == 0`** ∧
        **`graved == vec![X_sha]`**. *(reap 테스트는 `gc_deleted == 1` 자체가 삭제 분기의 증거지만 — 참조됨
        분기로 샜다면 `gc_deleted == 0`이 된다 — 두 단언을 **동일한 규율**로 함께 건다.)*
      · **뮤턴트(개정 전으로 되돌림 — `live`를 보호 술어로 복원: `restore ⇔ live ∨ landed`, 코호트 대기 없음)**
        → 판정 시점에 그 핀은 **park된 채 live** → **Restore** → **`gc_deleted == 0`** → **RED**
        (4번의 pending 단언도 함께 깨진다 — **두 개의 독립 RED 신호**)
      · **이것이 r2/P-2가 지목한 "미특성화 capacity/statistics 플립"의 기계 증인이다.** 이 시나리오가 매 패스
        반복되면 개정 전 설계는 X를 **영영 회수하지 못한다**. T-C1은 실패한 put이 **이미 반환된 뒤** reconcile을
        돌리므로 이 창을 **열지조차 못한다** — T-C3만이 연다
- [ ] **T-P4a — 포인터 rename *이전*에 영원히 멈춘 핀** (**r3/P-4의 결정적 증인 — Codex 명시 요구**).
      *T-C3와 형제지만 정반대를 친다*: T-C3의 핀은 **결국 죽는다**(rename이 EISDIR로 실패) → 결말이 확정된다.
      **T-P4a의 핀은 죽지 않는다** → 결말이 **영원히 불명**이다. r2안은 이 창에서 **영영 깨어나지 못한다**.
      1. 만료·미참조 blob X를 심는다(정상 put → 포인터 삭제 → tombstone 만료).
      2. `Hooks{ in_commit_pre_rename: park, post_grave: recorder }`. `park` = **도착 `pre_rename_reached`
         송신 → mpsc park**(위 §park 함정 — 테스트가 `tx`를 **모든 단언이 끝날 때까지** 쥔다 → **본문이 도는
         동안 절대 풀리지 않는다**. 해제는 **teardown에서 명시적으로** 한다 — 5단계 · r7/P-9).
      3. `put(b, "stuck", X_bytes)`를 **spawn하고 핸들을 보유한다**(`let put = tokio::spawn(…)` — **`let _ =` 금지**)
         → `blob_intact == true`(dedup) → `stage` 성공 → **`in_commit_pre_rename`에서 park**.
         ⚠ 이 put은 **본문이 도는 동안에는** 끝나지 않는다 — **그러나 teardown에서 끝난다**(5단계에서 **await한다**).
      3′. **⚠ `pre_rename_reached`를 `await`한다**(r5/P-6 — **spawn ≠ polled**). 이것 없이 4번으로 넘어가면 put이
         **아직 `pin()`도 안 한 채** GC가 무덤을 파 **빈 코호트**를 캡처하고 **즉시 reap**할 수 있다 →
         `gc_deleted == 1` ∧ blob 부재 → **단언 ①·②가 셋업 스케줄링 때문에 RED**가 되고, *"영원히 멈춘 핀"*
         이라는 **시나리오 자체가 재현되지 않는다**(무한 대기 뮤턴트도 **살아남는다** — 기다릴 코호트가 없다).
         ⇒ 이 await 이후에야 **핀 live · rename 미도달 · `landed` 무흔적**이 **확정 사실**이 된다.
      4. `timeout(5s, run_once_at(&s, now, gc_grace, /*settle_timeout*/ 200ms))` → **`Ok`여야 한다**
         → `pre_grave` 통과 → 무덤 rename → 코호트 = {그 핀} → `settle()` 대기 → **200ms 타임아웃**
         → **fail-CLOSED 복원** → `Settled::Deferred`.
      · **단언 ① (유실 0)**: `.objects/<sha>` **존재** ∧ **바이트 동일** ∧ `.objects/.gc-grave-<sha>` **부재**
      · **단언 ② (무회수)**: `stats.gc_deleted == 0`
      · **단언 ②′ (삭제 분기 자기검증 — r4)**: put은 `in_commit_pre_rename`에 **영원히** park돼 있다 →
        포인터가 **한 번도 착지하지 않는다** → **`stats.referenced == 0`**(세 패스 **전부**) ∧
        **`graved`가 X를 **패스마다 1회** 기록**(`post_grave` 관측 — 매 패스가 무덤을 **다시** 판다. 무덤은
        타임아웃 복원으로 매번 정본으로 되돌아가므로 다음 패스가 또 판다). *단언 ⑤의 `"gc settle timed out"`
        로그도 같은 사실의 독립 증인이다 — `settle()`이 실행되지 않았다면 그 로그는 **존재할 수 없다**.*
      · **단언 ③ (GC가 영구 정지하지 않는다 — `pass_lock` 해제)**: **후속** `timeout(5s, run_once_at(…))`가
        **`Ok`** ∧ 역시 `gc_deleted == 0` ∧ blob 여전히 존재. *(핀은 **아직도** park돼 있다 — 그런데도 패스가
        **완주한다**. 이것이 "멈춘 핀 하나가 GC를 영구 정지시키지 못한다"의 기계 증인이다.)*
      · **단언 ④ (격리 — 다른 blob은 오늘과 똑같이 회수된다)**: 만료·미참조 blob **Y**(핀 없음)를 심고
        **세 번째** 패스 → `timeout(5s, …)` = `Ok` ∧ **`gc_deleted == 1`**(Y가 회수됐다) ∧ X는 **여전히 존재**
        (X만 연기된다). *"한 blob의 멈춘 핀이 **전체** GC를 세우지 못한다"의 양성 증인.*
      · **단언 ⑤ (관측 가능한 에러)**: 캡처된 tracing 출력에 **`"gc settle timed out"`** 이벤트가 **패스마다 1건**
        (레벨 ERROR · `sha`·`cohort_size=1`·`waited_ms` 필드 포함).
        *캡처*: `tracing_subscriber::fmt().with_writer(Arc<Mutex<Vec<u8>>>).finish()` +
        `tracing::subscriber::set_default(...)` 가드. `#[tokio::test]`는 **current-thread**이고 `settle()`의
        `error!`는 **reconcile 태스크(= 테스트 스레드)**에서 나므로 스레드-로컬 구독자가 잡는다.
        (`tracing-subscriber`는 이미 **정규 의존성**이다 — 새 dev-dep 0.)
      · **뮤턴트 (무한 대기 = r2안 그대로 — `await_settlement`를 `await_cohort_drained`로 되돌림)**:
        코호트가 **영영 드레인되지 않는다**(park된 핀) → `settle()`이 **영영 깨어나지 않는다** →
        **4단계의 `run_once_at`이 반환하지 않는다** → **4단계의 `timeout(5s, …)`가 `Err`** → **패닉 = RED**.
        ⚠ **park를 절대 해제하지 않는다** → 뮤턴트에 **탈출구가 없다**. 패닉 unwind가 `tx`를 drop해 훅을 풀어
        주므로 **RED는 hang이 아니라 깔끔한 실패**로 뜬다(위 §park 함정).
        · ⚠ **정정 (r6 / 함정 6 — r3안의 논증이 *거짓*이었다)**: r3안은 여기에 *"그리고 `pass_lock`을 쥔 채이므로
          후속 패스도 전부 막힌다 — 단언 ③도 함께 죽는다"*고 적었다. **틀렸다.** `tokio::time::timeout`이 `Err`를
          내면 **안쪽 퓨처가 드롭될 뿐**이고(§랑데부 규율 함정 6), 그 드롭이 `run_once_at`의 지역변수인
          **`PassGuard`를 → `OwnedMutexGuard`를 → `pass_lock`을 해제한다.** ⇒ 후속 패스는 **락에서 막히지
          않는다.** 그것들은 **스스로 같은 이유로 hang한다**(핀이 여전히 park돼 있고 무덤을 다시 파므로) →
          단언 ③의 `timeout(5s, …)`도 `Err`가 된다. **"독립 RED 신호 2개"라는 결론은 유지되지만 메커니즘이
          다르다**(락 봉쇄가 아니라 **각 패스의 자체 hang**). 실제로 테스트는 **4단계에서 먼저 패닉하므로**
          ③에 도달하지 않는다 — **RED 신호 하나로 이미 충분하다.**
          **단언 ①~⑤는 한 글자도 바뀌지 않았다** — 정정된 것은 *왜 RED인가*의 설명뿐이다.
      5. **⚠ teardown — 영구 park의 *해제*도 안무다 (r7/P-9 — 함정 9. r6의 "무해" 판정은 *거짓*이었다)**:
         **단언 ①~⑤가 전부 끝난 뒤**에만 실행한다.
         ① **`drop(tx);`** — park sender를 **명시적으로** 드롭한다(**스코프 종료에 기대지 않는다**) →
         ② **`let r = timeout(5s, put).await.expect("put must finish after park release")
            .expect("put task must not panic");`** → ③ **`assert!(r.is_ok());`**
         · **왜 필요한가 (r6의 면제가 무효인 이유)**: r6은 *"park 이후 실행되는 코드가 없다"*며 이 핸들을
           함정 5에서 **면제**했다. **그러나 `tx` 드롭이 곧 재개다** — §park 함정이 이미 그렇게 적어 놓았다:
           *"테스트 종료 → `tx` drop → `recv()` = `Err(RecvError)` → 훅 반환 → **클로저 완주**"*. 즉 teardown에서
           **`staged.commit_blocking(...)`이 실제로 돈다**: `rename` → `on_landed`(`landed` 삽입 ·
           `notify_waiters()`) → **fsync** → **`PinGuard::drop`** → 그리고 `commit_pointer`의
           **`.await.expect("join")`**. ⇒ **그 구간의 패닉은 put 태스크의 패닉이 되고, 버려진 핸들은 그것을
           조용히 삼킨다** → **테스트는 초록인 채로 아무것도 모른다.** *"코드가 없다"는 **논증**이었다.
           **await된 핸들이 신호다.**(규칙 0)*
         · **왜 `Ok`인가 (새 명제가 아니다)**: X의 정본 blob은 fail-CLOSED 복원으로 **디스크에 있고**
           (단언 ①), `b/stuck.meta.json` 자리에는 **아무것도 없다**(EISDIR 함정은 **T-C3의 장치**이지 여기에는
           **없다**) ⇒ 재개된 rename은 **성공**하고 `put`은 **`Ok`**를 낸다. **이 단언은 픽스의 관측 계약을
           건드리지 않는다** — *"teardown이 조용히 깨져 있지 않다"*만 말한다.
         · ⚠ **순서 엄수**: **반드시 모든 단언 뒤**에 해제한다. 먼저 해제하면 핀이 drop되고 포인터가 착지해
           **"영원히 멈춘 핀"이라는 시나리오 자체가 사라진다**(단언 ①~⑤가 **다른 세계를 관측**하게 된다).
         · ⚠ **뮤턴트 RED 경로는 그대로다**: 무한대기 뮤턴트는 **4단계에서 이미 패닉**하고, unwind가 `tx`와
           핸들을 **함께** 드롭하므로 teardown await에 **도달하지 않는다**(훅은 풀려서 런타임은 정상 종료 —
           §park 함정). **RED는 여전히 hang이 아니라 깔끔한 실패다.**
      · **함정 3 (r6 — "확인했고 무해", 유지)**: 핸들을 **드롭하지 않으므로 detach도 없다**. (`let _ = …`로
        즉시 드롭해도 태스크는 detach되어 계속 살지만 — **드롭 ≠ 취소** — 그 형태는 **금지한다**: 위 5단계의
        await 대상이 사라진다.)
      · **뮤턴트 (fail-OPEN — 타임아웃 시 Reap)**: 무덤을 지운다 → 그 뒤 park가 풀리면 rename이 착지 →
        **포인터 + blob 부재 → 404** → **단언 ①이 RED**. *(fail-CLOSED가 load-bearing임을 못박는다.)*

> ### ⚠ T-P4b는 **두 증인으로 분리**됐다 (r4 — Codex critical)
>
> **r3의 단일 T-P4b는 참조 스냅샷 *이전에* 포인터를 착지시켰다**(메타 rename과 `landed` 삽입이 `run_once_at`
> **시작 전에** 끝나도록 짜여 있었다). `collect_referenced`는 블롭 처리보다 **먼저** 도므로 그 스냅샷이
> **포인터를 포함**한다 → reconcile이 **참조됨 분기**로 새고 **`grave()`도 `settle()`도 부르지 않는다** →
> 복원 로그가 없어 **엉뚱한 이유로 RED**였다. **즉시 복원도, no-404 봉인도 아무것도 증명하지 못했다.**
> ⇒ 두 테스트 모두 **reconcile을 먼저 시작해 `pre_grave`/무덤 rename까지 진행시킨 뒤**(= `collect_referenced`가
> 포인터를 **놓친 뒤**) put을 진행시킨다. **역할 분담**:
> **T-P4b-1** = *"`landed`가 이미 true면 **대기 0**"* · **T-P4b-2** = *"대기 **도중** 착지하면 **알림이 깨운다**"*.
> 한 테스트가 둘 다 증명할 수는 없다 — 전자는 settle이 **시작 전에** 이미 landed여야 하고, 후자는 settle이
> **이미 대기 중이어야** 한다. **상호배타적 순서다.**
>
> ⚠ **r5/P-6 — 그 약속을 T-P4b-2는 지키지 않았다.** *"둘 다 reconcile을 먼저 시작한다"*고 적어 놓고 T-P4b-2는
> **put을 먼저 spawn**했고, 심지어 **도착을 기다리지도 않았다**(`tokio::spawn` ≠ polled → GC가 **핀이 생기기 전에**
> 무덤을 파고 **빈 코호트**를 reap할 수 있었다). **이제 두 증인의 골격이 동일하다**: *reconcile spawn →
> `pre_grave` 도착 await → put spawn → **put의 도착 await*** → 해제. 갈라지는 곳은 **put을 어디까지 진행시키느냐**
> 뿐이다 — **T-P4b-1은 `in_commit_post_landed`까지**(착지 **완료** → settle은 첫 검사에서 `Landed`),
> **T-P4b-2는 `in_commit_pre_rename`까지**(착지 **이전** → settle이 **대기에 들어간 뒤** 착지시킨다).
> **§랑데부 규율** 참조.

- [ ] **T-P4b-1 — 무덤 시점에 `landed`가 이미 true (핀은 live) → 대기 0 · 즉시 복원** (**Codex 명시 요구**):
      1. 만료·미참조 blob **X**를 심는다(정상 put → 포인터 삭제 → tombstone 만료). **포인터는 0개**다.
      2. `Hooks{ pre_grave: gc_park, in_commit_post_landed: put_park, post_grave: recorder }`.
         · `gc_park` = **async** 훅 — 도달을 알리고(`gc_arrived`) **`Arc<Notify>`로 park**(5단계에서
           **`notify_one()`**으로 해제).
           ⚠ **정정 (r6 / 함정 7)**: r4안은 이 park을 *"해제 가능한 대기(**oneshot**)"*라고 적었으나 **컴파일되지
           않는다** — `AsyncHook = Arc<dyn Fn(&str) -> BoxFuture<'static,()>>`는 **`Fn`**인데
           `oneshot::Receiver::await`는 **`self`를 소비**한다(`FnOnce`). §랑데부 규율의 채널 표가 **처음부터
           `Notify`를 지정**하고 있었다 — 그 표와 **모순된 서술**이었다. `notify_one()`은 **대기자가 없어도 permit을
           저장**하므로 **lost wakeup도 불가**하다(`notify_waiters()`를 쓰면 **유실된다** — 쓰지 마라).
         · `put_park` = **sync** 훅 — 도달을 알리고(`landed_reached`) **mpsc park**(§park 함정: 테스트가
           `tx_put`을 **단언이 끝날 때까지** 쥔다 → **본문이 도는 동안 절대 풀리지 않는다** → **핀이 살아 있다**).
           ⇒ put의 `JoinHandle`은 **보유한다**(`let put = tokio::spawn(…)`) — ⚠ **r7/P-9로 개정**: r4안은 이 핸들을
           *"의도적으로 await하지 않는다"*고 적었으나 **그 면제는 무효다**. `tx_put`을 드롭하는 **순간 클로저가
           재개해 fsync·`PinGuard::drop`까지 완주하고**, 그 구간의 패닉은 `commit_pointer`의 `expect("join")`을
           거쳐 **put 태스크의 패닉**이 된다 ⇒ **버려진 핸들이 삼킨다.** **7단계에서 await한다.**
         · **`settle_timeout` = 30s** — *이 테스트의 핵심 장치: 픽스는 그 30초를 **한 번도 건드리지 않아야** 한다.*
      3. **reconcile을 먼저 spawn**(`gc = tokio::spawn(run_once_at(&s2, now, gc_grace, 30s))`) →
         `PassGuard::begin` → `recover_graves` → **`collect_referenced`**(포인터 **0개** → `refs = {}`) →
         블롭 루프 → X의 tombstone **만료** → **`pre_grave`에서 park**. `gc_arrived`를 기다려 이 상태를 확인한다.
         · **이 시점의 사전조건 확인**: `.objects/<sha>` **존재**(무덤 아직 없음) ∧ `b/landed_then_stuck.meta.json`
           **부재**. ⇒ **`collect_referenced`는 포인터를 볼 수 없었다** — r4가 잡은 결함이 **구조적으로 배제된다.**
      4. **그 park 동안** put을 spawn: `put(b, "landed_then_stuck", X_bytes)` → `pin()`(무대기) →
         **`blob_intact == true`**(blob은 **아직 무덤으로 안 갔다** — GC가 `pre_grave`에 멈춰 있다) →
         **dedup 분기**(바이트 재기록 **없음** — 레이스의 전제) → `stage` → 커밋 **`rename`이 `Ok`** →
         **`landed` 삽입 + `notify_waiters()`**(⚠ 대기자 **0명** — settle은 **아직 시작조차 안 했다**) →
         **`in_commit_post_landed`에서 park**(fsync 직전). `landed_reached`를 기다린다.
         · 이 시점: **포인터가 VFS에 실재**(핵심 사실 C) ∧ **`landed ∋ sha`** ∧ **핀은 여전히 live**(클로저 소유).
      5. **`gc_park`을 푼다** → GC 재개 → **`grave()`** = blob→무덤 rename → **코호트 = {그 핀}**(⚠ **살아 있다**) →
         `post_grave` → **`settle()`** → `await_settlement`의 **첫 검사 ①**에서 `landed ∋ sha` →
         **`Settlement::Landed` 즉시 반환(await 0회)** → **즉시 복원**(무덤 → 정본).
      6. **`timeout(2s, gc)` → `Ok`여야 한다.** ⚠ **이 2초 창은 5단계(해제) 이후에만 돈다** → **settle 구간만**
         잰다(셋업 시간이 섞이지 않는다).
      7. **⚠ teardown (r7/P-9 — 함정 9)**: **단언 ①~⑤가 전부 끝난 뒤** → **`drop(tx_put);`**(명시 — 스코프 종료에
         기대지 않는다) → **`timeout(5s, put)`** → **`JoinError` 언랩**(패닉 = 즉시 RED) → **안쪽 `put()` 결과가
         `Ok`** 임을 단언. 해제 시 클로저가 재개해 **fsync → `PinGuard::drop` → `commit_blocking` 반환**까지
         **실제로 돈다**(rename과 `landed` 삽입은 **park 이전에 이미** 끝났다) — **그 구간이 조용히 깨져 있지
         않음을 관측한다.** ⚠ **반드시 단언 이후**: 먼저 해제하면 **핀이 drop되어 코호트가 드레인**되고,
         *"핀이 live인데도 즉시 복원됐다"*(단언 ②)는 **이 테스트의 요지가 사라진다.**
      · **단언 ① (삭제 분기 자기검증 — r4)**: **`stats.referenced == 0`**(포인터는 **3단계 이후에** 착지했으므로
        스냅샷에 **없다**. r4 결함이 재발하면 **1**이 되어 **시끄럽게** 깨진다) ∧ **`graved == vec![X_sha]`**
        (`post_grave` 관측 = **무덤이 실제로 파였다**. 참조됨 분기로 샜다면 **빈 벡터**다).
      · **단언 ② (핀이 live인데도 즉시 복원 — 이 테스트의 요지)**: **단언 시점에 put은 여전히 `put_park`에 갇혀
        있다**(테스트가 `tx`를 쥐고 있다) ⇒ **코호트는 드레인되지 않았다.** 그런데도
        `get_bytes(b, "landed_then_stuck")` = **`Ok`** ∧ **바이트 동일** ∧ `.objects/<sha>` **존재** ∧ 무덤 잔재 0.
      · **단언 ③ (무회수)**: `stats.gc_deleted == 0`.
      · **단언 ④ (타임아웃을 안 태웠다 — **시간 무관**, 주 단언)**: 캡처된 tracing에
        **`"GC restored: landed commit"`이 1건** ∧ **`"gc settle timed out"`이 0건**.
      · **단언 ⑤ (타임아웃을 안 태웠다 — 시간 기반, 보조)**: 6단계의 `timeout(2s, gc)` = **`Ok`**
        (예산 **30s**의 1/15 — **15× 분리**).
      · **뮤턴트 (landed 즉시복원 제거 = `await_settlement`의 검사 ① 삭제 → **무조건 코호트 드레인 대기**)**:
        코호트 = {**park된 핀**} → **영영 드레인되지 않는다** → settle이 **30s 예산을 전부 태운다** →
        **그 창 내내 `.objects/<sha>`가 부재 = 실재하는 포인터가 404**(= Codex P-4 후반부 시나리오 **그 자체**) →
        **단언 ⑤가 `Err`(RED)** ∧ **단언 ④의 두 문자열이 정확히 뒤바뀜(RED)**. **독립 RED 신호 2개.**
      · **뮤턴트 (`landed` 삽입 자체 제거)**: 보호 술어가 **false** → 검사 ①이 발화하지 않고 코호트도 드레인되지
        않는다 → **30s 타임아웃** → `Deferred`(fail-CLOSED 복원 — **유실은 없다**) → 그러나 로그가
        **`"gc settle timed out"` ×1 / `"GC restored: landed commit"` ×0**으로 **뒤바뀌고** 단언 ⑤도 `Err`
        → **RED ×2**.
      · **뮤턴트 (`notify_waiters()` 제거)** → **이 테스트는 GREEN이다**(settle이 **첫 검사에서** `landed`를 본다 —
        깨울 필요가 **없다**). **정직하게 적는다: T-P4b-1은 그 뮤턴트를 죽이지 못한다.**
        **그것이 T-P4b-2가 존재하는 이유다** — 두 증인의 역할은 **겹치지 않는다.**
- [ ] **T-P4b-2 — 대기 **도중**에 착지 → `landed` 알림(`notify_waiters`)이 대기를 깨운다** (**Codex r4 명시 요구**:
      *"settlement가 이미 대기 중일 때 rename이 착지하는 순서를 추가해 `notify_waiters()` 제거가 결정적으로
      RED가 되게 하라"* — **안무는 r5/P-6의 승인된 순서로 재작성**: *"spawn reconciliation and await `pre_grave`;
      spawn the put and await an explicit `pre_rename_reached` signal emitted by `park_A`; release `pre_grave`,
      observe `post_grave` and the pending settlement probe, then release `park_A` into rename/notify while
      `park_B` keeps the pin live."*):
      1. 만료·미참조 blob **X**를 심는다(tombstone 만료). **포인터는 0개**다.
      2. `Hooks{ pre_grave: gc_park, in_commit_pre_rename: park_A, in_commit_post_landed: park_B,
         post_grave: recorder+신호 }` — **전부 기존 훅이다**(`Hooks` 필드 **7개 불변 · 프로덕션 훅 0개 추가**).
         **모든 park이 「도착 신호 + 해제 신호」를 쌍으로 갖는다**(§랑데부 규율):
         · `gc_park` = **async** 훅 — 도착 **`gc_arrived`** 송신 → **`Notify` park**(**5단계**에서 해제).
         · `park_A` = **sync** 훅 — 도착 **`pre_rename_reached`** 송신 → **mpsc park**(**6단계**에서 해제).
         · `park_B` = **sync** 훅 — 도착 **`post_landed_reached`** 송신 → **mpsc park**,
           **본문에서는 해제하지 않는다**(테스트가 `tx_B`를 **모든 단언이 끝날 때까지** 쥔다 → **핀이 착지
           이후에도 살아 있다**). ⇒ put의 `JoinHandle`은 **보유하고 8단계(teardown)에서 await한다**
           (⚠ **r7/P-9** — *"의도적 미await"* 면제는 **무효**다: `tx_B` 드롭이 곧 **재개**이고, 재개된 클로저의
           fsync·`PinGuard::drop`에서 나는 패닉을 **버려진 핸들이 삼킨다**).
         · `post_grave` = 기록(`graved`) + 도착 **`graved_reached`** 송신(park 없음 — 통과한다).
         · **`settle_timeout` = 30s**.
      3. **reconcile을 먼저 spawn**(`gc = tokio::spawn(run_once_at(&s2, now, gc_grace, 30s))`) →
         `PassGuard::begin` → `recover_graves` → **`collect_referenced`**(포인터 **0개** → `refs = {}`) →
         블롭 루프 → X의 tombstone **만료** → **`pre_grave`에서 park**.
         **⇒ `gc_arrived`를 `await`한다**(다음 단계로 넘어가기 전에 **반드시**).
         · **이 시점의 사전조건 확인**: `.objects/<sha>` **존재**(무덤 **아직 없음**) ∧
           `b/settle_wakeup.meta.json` **부재**. ⇒ **`collect_referenced`는 포인터를 볼 수 없었다** —
           **r4의 참조됨 분기 누수가 구조적으로 배제된다**(T-P4b-1과 **동일한 보증**. *"두 증인 모두 reconcile을
           먼저 시작한다"*는 이 계획의 약속이 **이제 실제로 지켜진다** — r5/P-6이 지적한 모순이 사라진다).
      4. **그 park 동안** put을 **spawn**: `put(b, "settle_wakeup", X_bytes)` → `pin()`(무대기) →
         **`blob_intact == true`**(blob은 **아직 무덤으로 안 갔다** — GC가 `pre_grave`에 멈춰 있다) →
         **dedup 분기**(바이트 재기록 **없음** — 레이스의 전제) → `stage` → **`park_A`에서 park**(rename **직전**).
         **⇒ `pre_rename_reached`를 `await`한다**(다음 단계로 넘어가기 전에 **반드시**).
         · **⚠ 이 await가 r5/P-6의 봉인 그 자체다.** `tokio::spawn`은 **폴링을 보장하지 않는다** — 이 신호가
           없으면 put이 **아직 `pin()`도 못 한 채** GC가 재개돼 **빈 코호트**를 캡처하고 **즉시 reap**할 수 있다
           → 5단계의 pending 단언(또는 후속 단언)이 **`notify_waiters()` 제거가 아니라 셋업 스케줄링 때문에**
           깨진다. **이 증인은 그때 아무것도 봉인하지 못한다.**
         · 이 await 이후 **확정 사실**: **핀 live** ∧ **미착지**(`landed` **무흔적**) ∧ **포인터 부재**.
      5. **`gc_park`을 푼다**(`Notify::notify_one()`) → GC 재개 → **`grave()`** = blob→무덤 rename →
         **코호트 = {그 핀}**(⚠ **4단계가 살아 있음을 확정했다**) → `post_grave` → **`settle()`** →
         `await_settlement`: 검사 ① `landed` **false**(put은 `park_A`에 있다) · 검사 ② 코호트 **미드레인**
         → **`notified.await`로 진입한다(= 대기 중).**
         **⇒ `graved_reached`를 `await`한 뒤 `timeout(200ms, &mut gc)`가 `Err`(pending)임을 단언한다.**
         · **왜 이것이 "settle이 `notified`에 park했다"를 함의하는가**(결정적 논증 — 우연에 기대지 않는다):
           `await_settlement`의 루프 몸통은 **동기**다(`Mutex` 검사뿐) — **유일한 await 지점이
           `timeout_at(deadline, notified)`**이다. 그리고 이 순간 세 종료 조건이 **전부 거짓**이다
           (`landed` 비었음 ∵ put이 `park_A` · 코호트 살아 있음 ∵ **4단계의 도착 신호** · 30s 예산 남음)
           ⇒ **패스가 200ms 동안 반환하지 않았다는 사실 자체가 "settle이 그 await에 있다"는 뜻이다.**
           게다가 `notified.as_mut().enable()`이 **검사 이전에** 호출되므로 **등록은 이미 끝나 있다**
           (lost wakeup 불가). ⚠ **이 논증의 두 전제**(*핀이 살아 있다* · *`landed`가 비었다*)는 **4단계의
           `pre_rename_reached`가 없으면 성립하지 않는다** — 그래서 그 신호가 **load-bearing**이다.
         · ⚠ **이 전제가 깨지면 테스트는 조용히 약해지지 않고 시끄럽게 실패한다** — pending 단언 **그 자체가 RED**다.
      6. **그제서야** `tx_A`로 `park_A`를 **해제**한다 → 커밋 클로저 재개 → **`rename`이 `Ok`** →
         **`landed` 삽입 + `notify_waiters()`** → **`park_B`에서 park** — ⚠ **핀은 drop되지 않는다.**
         **이것이 이 테스트의 핵심 장치다**: `park_B`가 **핀을 착지 이후에도 살려 둠**으로써
         **`PinGuard::drop`의 알림이라는 대체 기상 수단을 제거한다.** ⇒ 이제 settlement를 깨울 수 있는 것은
         **`landed` 삽입의 `notify_waiters()` 하나뿐**이다(그 외에는 **30s 타임아웃**뿐).
         **⇒ `post_landed_reached`를 `await`한다** — *"착지했고, 핀은 **아직 살아 있으며**, 그 상태로 갇혔다"*가
         **논증이 아니라 관측**이 된다(단언 ③의 전제를 기계로 못박는다).
      7. settlement가 **깨어나** 검사 ①에서 `landed ∋ sha` → **`Settlement::Landed`** → **즉시 복원**.
         **`timeout(2s, gc)` = `Ok`**(⚠ 이 2초 창은 **6단계(해제) 이후에만** 돈다 → **settle 구간만** 잰다.
         셋업·park 시간이 섞이지 않는다).
      8. **⚠ teardown (r7/P-9 — 함정 9)**: **단언 ①~⑤가 전부 끝난 뒤** → **`drop(tx_B);`**(명시. `tx_A`는
         **6단계에서 이미 해제**됐다) → **`timeout(5s, put)`** → **`JoinError` 언랩** → **안쪽 `put()` = `Ok`** 단언.
         재개된 클로저가 **fsync → `PinGuard::drop` → 반환**까지 **실제로 돈다** — 그 구간의 패닉·에러를
         **관측한다**(버리면 **초록인 채로 삼켜진다**). ⚠ **반드시 단언 이후**: 먼저 해제하면 **핀이 drop되어
         코호트가 드레인**되고, 그러면 *"깨운 것은 `notify_waiters()` **하나뿐**"* 이라는 이 테스트의 **핵심 장치가
         무너진다**(드레인이라는 **대체 기상 수단**이 되살아나 `notify_waiters()` 제거 뮤턴트가 **살아남는다**).
      · **단언 ① (삭제 분기 자기검증 — r4)**: **`stats.referenced == 0`**(포인터는 **3단계 이후에** 착지한다 —
        `collect_referenced`는 3단계에서 **이미 끝났다**) ∧ **`graved == vec![X_sha]`**.
      · **단언 ② (대기 진입)**: 5단계의 `timeout(200ms, &mut gc)` = **`Err`**(pending).
      · **단언 ③ (핀이 **아직도** live인 채로 복원됐다)**: **6단계의 `post_landed_reached`가 도착했고**(= 착지 완료)
        **put은 `park_B`에 갇혀 있다**(테스트가 `tx_B`를 쥐고 있다 → **핀 미drop · 코호트 미드레인**) → 그런데도
        `get_bytes` = **`Ok`** ∧ **바이트 동일** ∧ `.objects/<sha>` 존재 ∧ 무덤 잔재 0 ∧ `gc_deleted == 0`.
      · **단언 ④ (시간 무관, 주 단언)**: `"GC restored: landed commit"` **×1** ∧ `"gc settle timed out"` **×0**.
      · **단언 ⑤ (시간 기반, 보조)**: 7단계의 `timeout(2s, gc)` = **`Ok`**(예산 30s — **15× 분리**).
      · **뮤턴트 (`landed` 삽입의 `notify_waiters()` 제거) — ⚠ 이제 죽는다(r3에서는 죽일 수 없었다)**:
        settlement는 6단계 **이전에 이미** `notified.await`에 park했다(**단언 ②가 그것을 못박는다**). 알림이
        사라지면 **깨울 것이 아무것도 없다** — 핀은 `park_B`에 갇혀 **drop되지 않으므로** `PinGuard::drop`의
        `notify_waiters()`도 **오지 않는다**. ⇒ settlement가 **30s 예산을 전부 태운다** → `TimedOut` →
        **단언 ⑤가 `Err`(RED)** ∧ **단언 ④의 두 문자열이 뒤바뀜(RED)**. **독립 RED 신호 2개.**
      · **뮤턴트 (코호트 대기 제거)** → 판정 시점에 `landed`가 비어 있다 → **Reap** → 해제된 put이 dedup으로
        착지 → **포인터 + blob 부재 → 404** → **단언 ③이 RED**.
      > ⚠ **정직하게 — 이 뮤턴트는 안전성 결함이 아니라 지연(latency) 결함이다.**
      > 알림이 없어도 settlement는 **결국** 깨어나고(핀 drop **또는** `settle_timeout`) **어느 쪽이든 복원한다**
      > (`Landed` 또는 fail-CLOSED `Deferred` — **디스크 전이가 같다**) ⇒ **유실 0 · 판정 동일.**
      > 바뀌는 것은 **실재하는 포인터가 404를 내는 창의 길이**뿐이다(= P-4 "rename 이후 스톨"의 잔여분).
      > **T-P4b-2는 그 창을 관측 가능하게 만들어 뮤턴트를 죽인다** — 핀을 착지 이후에도 park해
      > **드레인이라는 대체 기상 수단을 제거**하면 지연이 **30s로 증폭되어 단언에 걸린다.**
      > **r3 개정은 이 뮤턴트를 *"죽이는 테스트는 없다"*며 equivalent로 분류했다 — r4에서 그 분류는 철회된다.**
      > 다만 **결함의 등급은 그대로다**: 이것은 **가용성(404 창) 회귀**이지 **유실**이 아니다. **과장하지 않는다.**
- [ ] **T-B5 — fault injection 4종** (P-1이 요구한 취소/복원실패/재시작/롤백):
      ① **취소**: `post_grave` 훅이 **도착 `graved_reached`를 송신한 뒤 park**한다 → reconcile을 **spawn** →
      **`graved_reached`를 await** → *그제서야* 퓨처 abort(park한 async 훅째로 드롭된다 = 해제) →
      **⚠ 취소 *완료*를 await한다**(r6/P-7 — 아래) → `.gc-grave-<sha>` 정확히 1개 ∧ `<sha>` 부재 →
      **새 `run_once`** → `recover_graves` 복원 → `get_bytes` Ok, 잔재 0
      · **랑데부(r5 — 도착)**: **도착을 기다리지 않고 abort하면 무덤이 아직 안 파여 있다** → `.gc-grave-<sha>`가
        **0개** → **엉뚱한 이유로 RED**(`recover_graves` 삭제 뮤턴트도 **살아남는다** — 복구할 무덤이 애초에 없다)
      · **⚠ 랑데부(r6/P-7 — 취소 완료). 이것은 T-C2와 *같은 함정의 두 번째 사례*다**(전수 점검이 찾아냈다):
        `abort()`는 취소를 **스케줄만 한다**(`join.rs:227-229`). 그 상태에서 곧바로 **새 `run_once`를 시작하면**,
        아직 드롭되지 않은 `PassGuard`가 **`pass_lock`을 쥐고 있어** 새 패스가 `lock_owned().await`에서
        **막힌다** → 라이브니스 `timeout`에 걸려 **엉뚱한 이유로 RED**(또는 hang). 디스크 단언(`.gc-grave-*`)도
        **취소가 끝나기 전에** 읽게 된다.
        **수정**: `gc.abort()` → **`let e = timeout(2s, &mut gc).await.expect("abort must complete")
        .expect_err("pass must be cancelled"); assert!(e.is_cancelled());`** → *그제서야* 디스크 단언 + 새 `run_once`.
        ⇒ **취소 완료 = `PassGuard` drop = `pass_lock` 해제**가 **관측**이 된다(함정 4).
        *(함정 3 확인: abort 시점에 in-flight `spawn_blocking`은 **없다** — `grave()`의 rename은 `post_grave`
        **이전에** 이미 반환했다. 따라서 이 await는 blocking 클로저를 기다리지 않는다.)*
      ② **크래시/재시작**: `Store`를 드롭하고 무덤이 심어진 root에 **새 `Store`**를 만들어 `run_once` →
      복원 + 포인터 관측 → 보존
      · **함정 4 (r6 — "확인했고 없음")**: `drop(store)`는 **디스크에 아무 효과도 없다**(`PassGuard::drop`은
        디스크 무접촉). ②는 그 드롭의 효과에 **의존하지 않는다** — 전제는 **디스크에 놓인 무덤**뿐이고, 재시작
        시뮬레이션의 동력은 **새 `Store::new`가 새(빈) 핀 등록부를 만든다**는 사실이다(D-3의 해저드를 **의도적으로**
        쓴다). *(`BlobPins`는 `Arc` 공유라 클론이 하나라도 살아 있으면 등록부는 드롭되지 않는다 — ②는 그것도
        **가정하지 않는다**.)*
      ③ **복원 실패**: `restore_io` 훅으로 EIO 주입 → `run_once` = `Err`(io::Error 무가공) ∧ 무덤 잔존 ∧
      **unlink 0회** → 다음 패스 복구 → 유실 0
      ④ **`Graved` 누수** (**안무 확정 — r7/P-8**): 무덤을 **실제로 판 뒤** `settle()`을 **부르지 않고** `Graved`를
      **버린다** → 무덤 잔존 → **다음 패스가 복구한다**(**fail-CLOSED by construction**)
      · **⚠ r7/P-8 (critical — 함정 10): r6안의 `let _ = pass.grave(..)`는 *아무 일도 하지 않았다.***
        `grave`는 **`async fn`** 이다 ⇒ `let _ = pass.grave(&sha)`는 **폴링되지 않은 퓨처를 드롭**할 뿐이고
        **blob→무덤 rename이 *아예 일어나지 않는다*.** 그러면 `drop(pass)` 이후의 패스는 **원래의 멀쩡한 blob**을
        발견하고, **`recover_graves`가 통째로 깨져 있어도 테스트가 GREEN이다** — 의도한 fail-closed 증인이
        **아무것도 증명하지 못한다.** (`#[must_use]`조차 `let _ =`가 **삼킨다** — 컴파일러는 침묵한다.)
      · **최종 안무 (5단계 — 순서가 곧 증명이다)**:
        1. 만료·미참조 blob **X**를 심는다(정상 put → 포인터 삭제 → tombstone 만료). **동시 put 0 · spawn 0 · park 0.**
           `Hooks{ post_grave: recorder }`(기록만 — park 없음).
        2. **`let pass = PassGuard::begin(&s, settle_timeout).await.expect("begin");`**
           · **삭제 분기 자기검증(r4)**: **`assert!(pass.referenced().is_empty())`** — 포인터가 **하나도 없다**
             (`stats`가 없는 경로이므로 `referenced()`로 **같은 규율**을 건다).
        3. **⚠ `grave()`를 `await`한다 (P-8의 전부)**:
           **`let graved = pass.grave(&x_sha).await.expect("grave rename must succeed");`**
           → **성공을 단언**한다(`Graved`는 **rename이 성공했을 때만** 태어난다 — §3).
        4. **⚠ 복구 *이전* 디스크 상태를 단언한다** (이 단언들은 **복구 패스보다 먼저** 실행되어야 한다 —
           `recover_graves`가 돌면 무덤은 사라진다):
           · **`.objects/.gc-grave-<sha>` 존재** ∧ **무덤 정확히 1개** ∧ **정본 `.objects/<sha>` 부재**
           · **`graved == vec![X_sha]`**(`post_grave` 관측 = *"무덤이 실제로 파였다"*의 직접 증거 — r4 규율)
           ⇒ **이 네 줄이 P-8이 없앴던 바로 그 관측이다.** r6안에서는 **넷 다 거짓**이었고(무덤 0개 · blob 존재)
           **아무도 그것을 묻지 않았다.**
        5. **누수 시뮬레이션 → 복구**:
           · **`drop(graved);`** — **`settle()`을 부르지 않는다**(= **누수**. 이것이 이 시나리오의 정의다).
             `Graved`에는 **파괴적 Drop이 없다** ⇒ **디스크는 그대로**여야 한다 → **재확인**: 무덤 **여전히 1개**
             ∧ blob **여전히 부재**.
           · **`drop(pass);`** — **명시적으로** 드롭한다(**⚠ 함정 4 — r6에서 걸렸다**). `PassGuard`가 살아 있으면
             **`pass_lock`을 쥔 채**이므로 **다음 `run_once`가 hang한다**. 스코프 종료에 기대지 않는다
             (`OwnedMutexGuard::drop`은 **동기·즉시** → 그 뒤의 패스는 곧바로 들어간다).
             *(순서는 **타입이 강제**한다: `Graved<'p>`가 `&'p PassGuard`를 빌리므로 `drop(graved)` ≺ `drop(pass)`.)*
           · **복구 패스**: **`timeout(5s, run_once_at(&s, t_before_expiry, gc_grace, settle_timeout))` = `Ok`**
             → `PassGuard::begin`의 **`recover_graves`가 무덤을 정본으로 되돌린다** → 블롭 루프는 X의 tombstone을
             **미만료**로 보고 **Skip**한다.
             ⚠ **`now`를 만료 이전으로 되돌리는 이유**(정직하게): 같은 `now`로 돌리면 그 패스가 복원 **직후** X를
             **정당하게 다시 파묻고 reap**한다(X는 진짜 가비지다) → *"복구됐다"*가 `gc_deleted == 1`로부터의
             **간접 추론**으로 약해진다. `now`를 되돌리면 **복원 그 자체를 직접 관측**한다. 이는 **테스트 안무**이며
             (`run_once_at`의 `now`는 **이미 주입형 인자**다) **프로덕션 경로를 한 줄도 건드리지 않는다.**
      · **단언 (복구 이후)**: `.objects/<sha>` **존재** ∧ **바이트 동일** ∧ **무덤 잔재 0** ∧ `gc_deleted == 0`.
      · **뮤턴트 `recover_graves` 삭제 → 이제 죽는다**: 복구 패스가 무덤을 되돌리지 못한다 → **blob 부재** ∧
        **무덤 잔존 1** → **RED ×2**. ⚠ **P-8 이전에는 이 뮤턴트가 ④에서 GREEN이었다**(무덤이 애초에 없었고 blob은
        멀쩡했다) — **④의 킬 파워 전부가 3·4단계에서 나온다.**
      · **뮤턴트 `Graved`에 파괴적 Drop 추가**(drop 시 `remove_file(grave)` = fail-OPEN) → **5단계의 재확인 단언이
        RED**(무덤이 사라졌다) ∧ 복구할 것이 없어 blob **영구 유실** → **RED ×2**.
      · **뮤턴트 rename 없이 `Graved`를 낳는다**(= `Graved`가 더는 rename 성공의 증거가 아니다) → **4단계가 RED**
        (무덤 0개 ∧ blob 존재 ∧ `graved`가 **비어 있다**).
      · **랑데부(r5 — spawn · r7/P-9 — teardown)**: **②③④는 park 0 · spawn 0**(전부 순차 await) →
        **spawn ≠ polled 함정도, teardown 함정(9번)도 구조적으로 없다** — teardown에 재개될 park·태스크가
        **하나도 없다**. park·abort가 있는 것은 **①뿐**이고(그 park은 **abort가 통째로 드롭**한다 → 역시
        teardown 잔여 0), 위에서 **도착 신호 + 취소 완료 await**로 봉인했다.
      · **삭제 분기 자기검증(r4)**: 이 네 시나리오에는 **동시 put이 아예 없다**(X는 만료·미참조로 심어진다)
        → **참조됨 분기 누수의 여지가 구조적으로 없다.** 그래도 규율을 맞춰 **`stats.referenced == 0`**을
        단언한다(④는 `run_once`를 거치지 않으므로 **`pass.referenced().is_empty()`** 로 같은 규율을 건다).
        ①의 **`.gc-grave-<sha>`가 정확히 1개**라는 단언이 이미 *"무덤이 실제로 파였다"*의 직접 증거다
        (참조됨 분기로 샜다면 무덤이 **0개**다) — ②③은 그 무덤을 **전제**로 하므로 동일하게 보호되고,
        **④는 이제 스스로 그 증거를 만든다**(3·4단계의 `grave().await` + 디스크 단언 + `graved == vec![X_sha]`
        — **r7/P-8 이전에는 없던 것이다**).
      · **뮤턴트 `recover_graves` 삭제** → ①②에서 무덤 영구 잔존 → `get_bytes` 404 → **RED**
        (**④에서도 RED** — blob 부재 ∧ 무덤 잔존. **P-8 이전에는 ④가 이 뮤턴트를 놓쳤다**)
- [ ] **T-Q2 — `recover_graves` 내용 검증**: `<sha>` 내용이 손상 ∧ `.gc-grave-<sha>`에 **정상** 사본 →
      `recover_graves` → **무덤이 정본을 덮어쓴다** → `get_bytes` Ok
      · **뮤턴트(`blob 존재 → remove_file(grave)` 무검증)** → 좋은 사본 소멸 → 격리 → 404 → **RED**
- [ ] **T-Q3 — `is_dir` 가드**: `.gc-grave-<64hex>`라는 **디렉터리**를 심는다 → `recover_graves`가 `is_dir`로
      스킵 → `<sha>`가 디렉터리가 되지 않음 → 이후 put 정상(500 영구화 없음)
- [ ] **컴파일 불가 뮤턴트 (정직한 목록 — r2/P-3으로 개정)**: **`settle` 이전의 사전확인**.
      **무엇이 정확히 컴파일 불가인가**:
      ① 보호 판정 API는 **`Graved::settle(self)` 하나뿐**이다. `BlobPins`에는 sha로 물어볼 수 있는 **공개 술어가
         존재하지 않는다**(`protected()` **삭제**, `landed()`/`cohort_at_grave()`/`await_settlement()`/
         `Settlement`은 `pins.rs` **private**). ⇒ **`reconcile.rs`는 사전확인을 표현할 방법이 아예 없다** —
         GC 루프 문장을 어떻게 재배치해도 컴파일되지 않는다.
      ② `Graved`를 만드는 **유일한 길은 `PassGuard::grave()`이고, 그것은 blob→무덤 rename을 실제로 수행한다.**
         필드는 전부 private, 생성자는 `pins.rs` 안에만, `Default`/`Clone`/`Copy` **유도 없음**. ⇒
         `pass.grave(&name).await?.settle().await?`에서 **`settle()`을 `grave()` 앞으로 옮기는 재배치는
         컴파일되지 않는다**(`Graved` 값이 존재하지 않으므로).
      ③ `settle`은 `self`를 **소비**하므로 "판정만 미리 얻어 두고 나중에 쓴다"도 표현 불가다. 판정은 **이 sha ·
         이 무덤 전이**에 **바인딩**된다 — r2가 지적한 "**무관한 rename에서 얻은 unit 영수증**"의 우회로가
         **구조적으로 사라졌다**(`RenameReceipt` 자체가 없다).
      · ⚠ **경계(정직 — 과장하지 않는다)**: 이 봉인은 여전히 **모듈 경계**다. **`pins.rs`를 편집해** 새 술어
        API(예: `pub(crate) fn protected(&self, sha)` 또는 raw `inner.lock()` peek)를 **추가**하면 풀린다.
        그건 **재배치가 아니라 새 API 추가**이므로 뮤턴트 클래스 **밖**이다. **"타입이 모든 걸 막는다"고 주장하지
        않는다.** 개정 1차안이 바로 그 과장을 했다가 mutant 렌즈 F1에 죽었고, r2가 `RenameReceipt`로 같은 과장을
        다시 잡아냈다.
      · **그래서 2차 방어선이 필요하다**: **T-B2(개정)** 가 그 뮤턴트를 **행동으로** 죽인다(사전확인 시점에는
        `live`도 `landed`도 비어 있다 → Reap → 404 → RED). **타입 + 테스트 이중 봉인이며, 문서는 둘 중 어느
        하나도 단독으로 충분하다고 주장하지 않는다.**
- [ ] 성능 sanity: reap당 fsync **+2**, restore당 **+1** — adversarial 루프 실행시간 회귀 없음.
      **코호트 대기의 실행시간 영향 = 0임을 실측으로 못박는다**: 105개 characterization + `tests/adversarial.rs`
      (40객체)에는 GC와 동시에 같은 sha를 dedup-put하는 시나리오가 **없다** → 코호트는 **항상 비어 있고**
      `await_settlement`가 **첫 검사에서 `Drained`를 반환**한다(await 0회 · fast path). 회귀가 보이면 fast path가
      깨진 것이다. **`settle_timeout`은 이 스위트들에서 단 한 번도 발화하지 않아야 한다** —
      `"gc settle timed out"` 로그가 **0건**임을 함께 확인한다(발화했다면 정상 경로에 연기가 생긴 것 = P-2 재발)
- [ ] **데드락 부재 sanity**: `settle()`이 대기하는 동안 **다른 키에 대한 put이 정상 완료**됨을 단언(T-B4의
      park 중 무관한 put 1건 → Ok, timeout 5s). GC→put 단방향 대기 · put은 `pass_lock`을 잡지 않음을 못박는다
- [ ] **라이브니스 sanity (r3/P-4)**: **모든** `run_once_at` 호출을 테스트에서 `tokio::time::timeout`으로 감싼다.
      **패스가 반환하지 않는 것은 hang이 아니라 실패여야 한다** — 이것이 P-4가 재발했을 때의 **조기 경보**다.
      (r2안에서는 T-P4a 시나리오가 **영영 반환하지 않았고**, 그 사실을 잡는 테스트가 **하나도 없었다.**)
      · **⚠ 함정 6 (r6 — `timeout`의 `Err`가 무엇을 하는가)**: `Err`는 **안쪽 퓨처를 드롭할 뿐이다.**
        `run_once_at`을 드롭하면 그 지역변수인 **`PassGuard`가 → `pass_lock`이 해제**되고 **무덤은 디스크에
        남는다**(`Graved`에 파괴적 Drop이 없다 — 의도된 fail-CLOSED). ⇒ **`Err`는 반드시 그 자리에서
        패닉시켜야 한다**(`.expect(...)`). 조용히 넘어가면 **다음 단언이 "패스가 중간에 잘린" 오염된 상태**를
        보게 되고, 그 RED/GREEN은 **아무 의미가 없다.** *(이 사실이 T-P4a 무한대기 뮤턴트의 RED 메커니즘을
        정정한 근거이기도 하다 — §「개시 ≠ 완료」 클래스 전수 점검 4번.)*
      · **⚠ GC `JoinHandle`을 프로브할 때는 `&mut`로 건다**(`timeout(200ms, &mut gc)`). **값으로 넘기면**
        `Err`일 때 **핸들이 드롭돼 GC 태스크가 detach**된다 → 이후 단언이 **아직 끝나지도 않은 패스**를 읽는다.

### B-3 acceptance (**위생·관측성·문서 — 행동 무변경**)

- [ ] `cargo test` 105 green + 회귀 GREEN 유지. **`ReconcileStats` 정의 무변경**(필드 추가 금지)
- [ ] **격리 분기 diff 0줄**(D-4) — `git diff`로 증명. `corrupt_blob_quarantined` **불변 초록**
- [ ] tracing: `GC restored` / `grave recovered` 필드(`sha`) · Drop poison 봉인(`unwrap_or_else(into_inner)`) ·
      `shrink_to_fit`
- [ ] **ADR 0002** + **CONTEXT.md Language**: **Pin / Landed / Grave / Cohort / Settle** (특히 *"landed = 커밋
      rename이 `Ok`를 반환했다"* — **유일한 보호 술어** — 와 *"cohort = 무덤 rename 시점에 살아있던 핀 집합"* —
      **대기 조건이지 보호가 아니다** — 와 *"settle = **유한·fail-CLOSED** 정산: landed 확정 → 즉시 복원 /
      코호트 드레인 → 판정 / **`settle_timeout` 초과 → 복원 + 연기 + `error!`**"*). 용어가 코드 식별자와 **1:1**.
      **ADR에 P-4의 교훈을 남긴다**: *"무취소 커밋은 유실 창을 닫는 대신 **대기의 상계를 파괴한다**. 
      `upload_timeout`은 **호출자 퓨처만** 자른다 — abort 불가능한 blocking 클로저를 기다리는 코드는 **반드시
      자기 벽시계 예산을 가져야 한다.**"*
- [ ] `Store::new` **D-3 doc** ("데이터 루트 하나당 Store 하나 — 공유는 `Store::clone()`")
- [ ] **롤백 런북**: 구 바이너리는 `.gc-grave-*`를 `Other`로 무시한다(절대 안 지운다) → 수동 복구는
      `mv .objects/.gc-grave-<sha> .objects/<sha>`. 무덤 개수 세는 원라이너 포함
- [ ] **`Graved` 봉인 체크리스트**(리뷰 항목 — r2/P-3 + **r3/P-4**): ① `Graved`의 필드는 **전부 private** · ②
      `Default`/`Clone`/`Copy` **유도 금지** · ③ **`PassGuard::grave()` 밖에 생성자를 만들지 말 것** · ④
      `BlobPins`에 sha로 조회하는 **공개 보호 술어를 추가하지 말 것**(`landed()`는 `pins.rs` private 유지) · ⑤
      **보호 판정 API는 `Graved::settle(self)` 하나로 유지**(판정만 따로 얻는 메서드 **금지**) ·
      **⑥ (r3/P-4) `settle()`의 모든 대기 경로는 유한해야 한다** — `await_settlement`의 `timeout_at` **제거 금지** ·
      `settle_timeout`에 **기본값을 숨긴 오버로드 금지**(호출자가 **알고 정한다**) · 타임아웃 분기는 **fail-CLOSED**
      (**복원**)여야 하며 **절대 `remove_file(grave)`로 가지 않는다** · **`landed` 삽입의 `notify_waiters()`
      제거 금지**(제거하면 착지한 객체가 코호트 잔여 멤버를 기다리며 404가 된다 — **유실은 아니지만 가용성
      회귀다**. **증인 = T-P4b-2**. r3에서는 *"죽이는 테스트가 없다"*였으나 **r4에서 죽는다**) ·
      **⑦ (r4) 배리어 테스트는 `stats.referenced`와 `post_grave` 관측으로 *삭제 분기 진입*을 자기검증한다** —
      이 두 단언을 **약화하지 말 것**(§삭제 분기 자기검증. 없애면 테스트가 **참조됨 분기로 새고도 초록**일 수 있다) ·
      **⑧ (r5/P-6) 배리어 테스트의 모든 park에는 「도착 신호 + 해제 신호」가 쌍으로 있다** — **spawn만 하고 다음
      단계로 넘어가는 지점을 만들지 말 것**(`tokio::spawn`은 **폴링을 보장하지 않는다** → 핀이 생기기도 전에
      GC가 **빈 코호트**를 캡처한다). §랑데부 규율의 **체크리스트 표를 함께 갱신하지 않고는** 배리어 테스트의
      안무를 바꿀 수 없다(⚠ **T-C3는 이 함정에서 *조용히 GREEN*이 된다** — `gc_deleted == 1`이 기대값과 같다) ·
      **⑨ (r6/P-7) 「개시 ≠ 완료」 — 비동기 연산의 *개시*를 *완료*로 쓰지 말 것.**
      **`abort()` 뒤에는 반드시 그 `JoinHandle`을 유한 타임아웃으로 await하고 `JoinError::is_cancelled()`를
      단언한다**(⚠ **`abort()`는 취소를 *스케줄만* 한다** — `join.rs:227-229`. 이것을 빠뜨리면 **T-C2의
      caller-owned 뮤턴트가 경합으로 GREEN이 되고**, **T-B5①은 `pass_lock`에서 hang한다**) ·
      **`timeout`의 `Err`는 안쪽 퓨처를 *드롭*할 뿐이다**(`&mut handle`로 프로브할 것 — 값으로 넘기면 태스크가
      detach된다) · **완주를 await하는 모든 핸들은 `JoinError`를 언랩한다**(버려진 핸들은 **패닉을 삼킨다**) ·
      **⚠ (r7/P-9 — 개정) "의도적으로 await하지 않는 핸들"이라는 예외는 *없다*.** **park된 태스크도 teardown에서
      재개된다**(sender 드롭 = 재개) ⇒ **park sender를 *명시적으로* 드롭하고, 핸들을 유한 타임아웃으로 await하며,
      `JoinError`와 안쪽 결과를 *둘 다* 언랩한다**(**단언을 전부 마친 뒤에** — 먼저 해제하면 시나리오가 사라진다).
      **⑩ (r7/P-8) async 표현식은 반드시 `.await`한다.** `let _ = <async fn>(..)`는 **폴링되지 않은 퓨처를
      드롭**할 뿐 **아무 일도 하지 않는다**(`#[must_use]`도 `let _ =`가 **삼킨다**). **부작용을 노린 async 호출을
      결과째 버리지 말 것** — T-B5④의 `let _ = pass.grave(..)`가 **무덤을 파지 않아** `recover_graves` 뮤턴트를
      **통째로 놓쳤다**. **rename·복원 같은 파괴/복구 연산은 await하고 *디스크 상태로* 확인한다.**
      **새 배리어 테스트를 쓸 때는 §「개시 ≠ 완료」 클래스 전수 점검의 10개 함정 항목을 1:1로 대조하고
      그 매트릭스에 행을 추가한다** — *"이전 라운드에서 safe 판정"은 근거가 아니다.*
      **이 열 줄 중 하나라도 어기면 봉인이 풀린다**(①~⑤: 사전확인 뮤턴트가 컴파일된다 · ⑥: **P-4가 부활한다** ·
      ⑦⑧⑨⑩: **증인이 아무것도 증명하지 못한다**) — 리뷰에서 반드시 확인
- [ ] **부분 해결 명시**: doc에 F-25(격리 분기 유실 경로 미해결)를 굵게 남긴다 — 릴리스 게이트 제출물
- [ ] `cargo clippy -D warnings`, doc 링크, `#[must_use]` 경고 0

## Scope — 최종 비-테스트 표면

```
src/layout.rs            GRAVE_PREFIX, is_sha_name, grave_name, grave_sha, Layout::grave_path,
                         ObjectsEntry::Grave, classify_objects_entry
src/store/atomic.rs      rename_durable(_blocking) → io::Result<()>, Staged, stage_blocking,
                         Staged::commit_blocking, write_atomic(위임 — **시그니처 불변**)
                         ※ RenameReceipt는 **없다**(r2/P-3 — 삭제)
src/store/pins.rs        [신규] BlobPins{pin, +private: cohort_at_grave/await_settlement/landed},
                         Inner{next_id, live: HashMap<String,HashSet<u64>>, landed, pass_live},
                         settled: Notify (핀 drop **+ landed 삽입** 양쪽에서 울린다 — r3/P-4),
                         PinGuard{blob_intact, commit_pointer, Drop(+notify)},
                         PassGuard{begin(store, **settle_timeout**),referenced,recovered,grave,Drop},
                         Graved{sha, cohort, settle(self)}, Settled{Restored,Reaped,**Deferred**},
                         **Settlement{Landed,Drained,TimedOut}**(private), Hooks(+in_commit_post_landed)
                         ※ protected() 는 **없다**(r2/P-3 — 보호 판정 API = Graved::settle 하나뿐)
                         ※ sift_corrupt / Sifted 는 **포함하지 않는다**(D-4 → F-25)
src/store/objects.rs     put, put_stream (pin → blob_intact → commit_pointer)
src/store/reconcile.rs   run_once(&Store, gc_grace, **settle_timeout**)[D-1],
                         run_once_at(&Store, now, gc_grace, **settle_timeout**), collect_referenced
                         (pub(super), hooks 인자), recover_graves, GC 삭제 분기(+ **Deferred arm**),
                         Grave arm, **GC_SETTLE_MARGIN / settle_timeout_from()**(pub — main.rs가 쓴다)
                         ※ **비트로트/격리 분기는 무변경**(D-4)
src/store/mod.rs         Store{pins}, Store::pins/layout, Store::with_hooks(cfg(test)), D-3 doc
src/main.rs              build_state 우선, **settle_timeout_from(cfg.upload_timeout)**(cfg move 이전),
                         run_once(&state.store, gc_grace, settle_timeout), state.store.clone() 루프
docs/adr/0002-*.md       [신규]  ·  CONTEXT.md (Language: Pin/Landed/Grave/Cohort/Settle, 롤백 런북)
```

⚠ **`src/config.rs`는 무변경이다**(scope 밖 유지 — 새 env 노브를 만들지 않는다). `settle_timeout`은
`FILES_UPLOAD_TIMEOUT`에서 **파생**되며 그 파생은 `reconcile::settle_timeout_from`(순수 함수)이 한다.
**`bugfix-lock.json`의 `scope[]`는 r2안에서 한 글자도 바뀌지 않는다** — 이 개정은 scope를 **넓히지 않는다**.

`src/http/state.rs` · `src/http/internal/files.rs` · `src/store/{locks,listing,buckets}.rs` ·
`src/capacity.rs` — **무변경**.

### `bugfix-lock.json`의 `scope[]` (개정 필요)

```json
"scope": ["src/store/**", "src/main.rs", "src/layout.rs"]
```

**`src/layout.rs` 추가가 이 개정의 전부**다(r1 P-1의 권고 그대로 — 무덤 이름공간이 `Layout` 소유이므로).
기존 값 `src/store/**`·`src/main.rs`는 유지. 테스트 경로(`tests/**`)는 배리어 B4의 `isTestPath()`가 scope
검사 **전에** 제외하므로 추가 개정은 불필요하다. `docs/**`도 scope 밖이다.

## 남은 위험

1. **⚠ 격리 분기 유실 경로 미해결 (F-25)** — **이 픽스는 "포인터만 남고 blob 부재" 증상 클래스에 대해 부분
   해결이다.** GC 삭제 분기는 봉인되지만 **비트로트 격리 분기의 `rename(blob → .corrupt)`는 핀·무덤을 거치지
   않은 채 남는다.** 동시 치유 put과 경합하면 **같은 증상**이 재현된다. 하드룰 10에 따라 별도 파이프라인(D-4).
   **릴리스 게이트에 명시 제출.**
2. **다중 프로세스/레플리카** — 핀은 **in-process**다. 같은 PVC에 replica ≥ 2면 등록부가 갈라져 **버그가
   부활한다.** `replicas:1 + RWO`가 **load-bearing 배포 불변식**이다(`locks.rs`가 이미 같은 전제 위에 있다 —
   새 위험이 아니라 **하나 더 얹혔다**). 완화: 배포 매니페스트 주석 + 향후 `.objects` 온디스크 패스 락파일.
   **[파일링: files#gc-pass-lockfile]**
3. **`Store::new` 2회 (D-3)** — doc + 테스트로 못박지만 **컴파일 강제는 아니다.** 후속: 프로세스 전역
   `root → BlobPins` 레지스트리. **[파일링]**
4. **뮤턴트 경계 (정직 — r2/P-3 개정)** — **`Graved` 봉인은 모듈 경계이지 타입 마법이 아니다.** `reconcile.rs`
   에서는 사전확인이 **표현 불가**하고(보호 술어가 공개돼 있지 않다), `settle()`을 `grave()` 앞으로 옮기는
   재배치는 **컴파일 불가**하다(`Graved` 없이는 호출할 수 없다). **그러나** `pins.rs`를 편집해 **새 술어 API를
   추가**하면 풀린다 — 그건 재배치가 아니라 **새 API 추가**이므로 뮤턴트 클래스 밖이다. B-3에 5줄 체크리스트로
   명시하고, **T-B2(개정)를 2차 방어선**으로 둔다. **타입이 모든 것을 막는다고 주장하지 않는다.**
5. **정상 Restore 경로의 transient non-servable 창** — 무덤 rename ~ 복원 rename 사이(fsync 2회 폭) 404/list
   제외. 복원이 **실패**하면 최대 `gc_grace`(prod: `reconcile_interval == gc_grace`) 동안 지속. 오늘의 영구
   유실 대비 순개선이나 **오늘 없던 상태**다.
6. **`settle()`의 코호트 대기 → reconcile 패스가 길어질 수 있다 (⚠ r3/P-4로 전면 개정 — r2안의 상계는 거짓이었다)** —
   `settle()`은 **무덤 시점 핀 코호트가 종료될 때까지 await**하되 **`settle_timeout`을 넘기지 않는다**.
   - **⚠ r2안이 무엇을 틀렸나**: "각 멤버의 수명은 `timeout(upload_timeout, …)`으로 **잘린다** → 상계 <
     `gc_grace`"라고 적었다. **거짓이다.** `tokio::time::timeout`은 **호출자 퓨처를 드롭할 뿐**인데, 이 설계는
     **의도적으로** `PinGuard`를 **abort 불가능한 `spawn_blocking` 클로저**로 옮겼다(핵심 사실 A). 멈춘 FS 연산은
     핀을 **무한정** 살린다 → **r2안의 대기는 무한정이었고, GC는 무덤을 판 채 영구 정지할 수 있었다**(P-4).
   - **실제 상계**: **`settle_timeout`**(= `upload_timeout + GC_SETTLE_MARGIN(60s)`, 기본 **660s**) — **GC가
     스스로 거는 벽시계 예산**이며 **이것만이 유일한 상계다**. `landed`가 확정되면 **대기 0**으로 끊는다.
   - **패스 상계(최악, 정직)**: GC 루프는 순차이므로 **`N_stalled × settle_timeout`**. 그러나 **패스는 반드시
     끝나고 락은 반드시 풀린다**(r2안과의 결정적 차이). `pass_lock` + `MissedTickBehavior::Skip`이 패스를
     **밀릴 뿐 쌓이지 않게** 한다.
   - **실제**: 코호트가 비어 있지 않으려면 **바로 그 sha를 동시에 dedup-put 중**이어야 한다. 정상 스크럽에서는
     **대기 0회**(fast path) → **실행시간 변화 없음**(B-2 acceptance의 성능 sanity가 이를 못박는다).
   - **정상 경로의 회수는 연기되지 않는다**(P-2 봉인 **불변**): 대기 후 판정이므로 실패·취소·ENOSPC put은
     **같은 패스에서** Reap된다. **`settle_timeout`은 "느림"이 아니라 "정지"를 잡으므로** 정상적으로 느린 put은
     여기 걸리지 않는다(`upload_timeout`이 caller를 자르면 **핀이 즉시 drop**된다).
   - **데드락 없음**: 대기는 **GC → put 단방향**(put은 `pass_lock`을 잡지 않고 GC는 `KeyLocks`를 잡지 않는다).
   - **대기 중 프로세스 사망**: 무덤 잔존 → 다음 패스 `recover_graves` 복원 → 안전(§크래시 논증 재사용).
   - ⚠ **미측정**: 코호트가 큰 병리적 부하(같은 sha를 수백 개 동시 dedup-put)에서의 패스 시간은 **벤치하지
     않았다**. 안전성 결함은 아니지만(대기에 상계가 있다) **지연 특성은 미실측**이다. **[파일링]**
12. **⚠ degraded-path 회수 연기 (r3/P-4가 남기는 것 — 정직하게)** — 코호트 멤버의 **파일시스템 연산이 영영
    돌아오지 않으면** 그 blob의 회수가 **스톨이 풀릴 때까지 매 패스 연기**된다(패스마다 `settle_timeout`을 태우고
    `tracing::error!`를 낸다). **이것은 실재하는 행동 변화이며 숨기지 않는다.**
    - **왜 받아들이는가**: 제거하는 유일한 방법은 **대기를 무한정으로 되돌리는 것**(= P-4 부활 = **GC 영구 정지**)
      이다. **연기를 없애는 게 아니라 GC를 세우는 것과 맞바꾸는 셈**이다.
    - **경계**: 정상 입력에서 **도달 불가능**(§degraded-path 연기) · **국소적**(그 blob 하나 — 다른 blob은 오늘과
      똑같이 회수, T-P4a 단언 ④) · **시끄럽다**(`error!`) · **자기치유적**(스톨이 풀리면 다음 패스에 정상 판정).
    - **오늘과의 비교**: 같은 상황에서 **오늘은 스토어가 사실상 죽는다** — reconcile 자신의 `tokio::fs::read`가
      같은 FS에서 멈추거나(패스 정지), 스톨이 커밋 클로저에서 나면 **포인터만 남고 blob 부재**(= 지금 고치는 유실).
    - **증인**: **T-P4a**. **[파일링: files#gc-settle-timeout-metric]**(연기 횟수를 stats/metric으로 노출 → F-29)
13. **`settle_timeout`은 `Config` 노브가 아니다** — `FILES_UPLOAD_TIMEOUT`에서 **파생**된다(`+60s`). 운영자가
    독립적으로 조정할 수 없다. **의도된 축소**다(scope를 넓히지 않기 위해 — §main.rs). `upload_timeout`을
    `gc_grace` 바로 밑까지 올리면 `settle_timeout > reconcile_interval`이 될 수 있으나 **안전하다**(`pass_lock` +
    `Skip`이 패스를 쌓지 않는다). 독립 노브가 필요해지면 → **F-29**.
7. **SIGTERM 핸들러 부재** — `main.rs:14` `shutdown_signal()`은 `ctrl_c()`만 await한다. k8s의 SIGTERM은 퓨처
   취소가 아니라 **프로세스 급사**다. 안전성 결함은 아니다(크래시 논증이 덮는다) — 그러나 graceful shutdown이
   SIGTERM에 반응하지 않는 것은 **별개 버그**다. **[파일링: files#sigterm-graceful-shutdown]** (본 fix의 전제 아님)
8. **`.corrupt`에 복구 경로 0** — 격리된 손상 blob을 되읽는 코드가 저장소 어디에도 없다. 운영 런북(수동 검사·
   삭제)이 유일한 출구. **[파일링]**
9. **`.gc-grave-<비-sha>` 쓰레기** — `Other`로 영구 무시(누구도 안 지운다). 의도된 보수성이지만 **누수**다.
   `ReconcileStats` 필드 불변 제약 때문에 카운트하지 않는다 → 디스크 사용량으로만 관측.
10. **fsync 비용** — reap당 +2, restore당 +1. 백그라운드 스크럽이라 SLO 영향은 없다고 보지만 **대량 회수 벤치는
    잡지 않았다.**
11. **`commit_pointer`의 blocking pool 점유** — 커밋은 수백 바이트 쓰기 + rename + fsync 2회다. reconcile의
    blob별 `tokio::fs::read`와 blocking pool을 공유한다(오늘도 그렇다). 대형 저장소 스크럽 중 커밋 지연이 ms
    단위로 늘 수 있으나, 그 지연은 이제 **유실이 아니라 지연일 뿐**이다(무취소 커밋). **[파일링]**

## Follow-up backlog

| id | 항목 | 라우팅 |
|---|---|---|
| **F-25** | **quarantine ↔ put 자가치유 경합** (아래 청사진 참조) — **이 픽스의 미해결 잔여물** | **별도 gated-bugfix (필수)** |
| F-26 | rename 앞의 사전 확인(순수 최적화 — rename churn과 일시적 404를 줄인다). **정확성은 전적으로 사후 확인에 있다**는 것을 코드에 못박아야 하며, 여기서 미끄러지면 기각된 '삭제 직전 재확인'으로 되돌아간다. ⚠ r2 개정 설계에서는 보호 판정 API가 **`Graved::settle(self)` 하나뿐**이고 `Graved`는 무덤 rename으로만 태어나므로, 이 최적화는 **`pins.rs`에 새 술어 API를 추가해야만** 가능하다 — **그 추가가 곧 P-3 봉인 해제다.** 기본 제외 | 선택적 최적화(**기본 제외 — 봉인을 푼다**) |
| F-27 | `.objects` **온디스크 패스 락파일** — 다중 replica에서 핀 등록부가 갈라지는 위험(§남은 위험 2) 완화 | **[파일링: files#gc-pass-lockfile]** |
| F-28 | **SIGTERM graceful shutdown** — `shutdown_signal()`이 `ctrl_c()`만 본다 | **[파일링: files#sigterm-graceful-shutdown]** |
| **F-29** | **settle 연기의 관측성·설정 (r3/P-4의 잔여)** — ① `ReconcileStats`에 `deferred: usize`(또는 metric) 추가 → **stats 전수 `assert_eq!` 3곳을 손대야 하므로 이 픽스에서는 금지**(두 번째 플립). ② `FILES_GC_SETTLE_TIMEOUT` **독립 env 노브**(현재는 `FILES_UPLOAD_TIMEOUT + 60s` 파생) → `src/config.rs` + `validate()` + `state.rs` 배선이 필요해 **scope 밖**. 오늘의 출구는 **`tracing::error!`** 하나다 | **[파일링: files#gc-settle-timeout-metric]** |

### F-25 — 청사진 (다음 파이프라인이 재발명하지 말 것)

**증상 클래스**: 오늘 고치는 것과 **동일**(커밋 포인터만 남고 blob 부재 → 영구 404). **트리거**: 선행 비트로트가
있는 blob을 put이 `write_atomic`으로 **치유한 직후**, 같은 패스의 격리 분기가 **stale read**를 근거로 그
**치유된 inode**를 `.corrupt`로 rename한다. **왜 이 픽스에서 못 고치는가**: 고치면 관측 행동이 하나 더 뒤집힌다
("치유된 blob이 격리되어 404가 된다" → "안 된다") — **하드룰 10**(D-4).

**설계(최종안 §1.3에서 그대로 이관 — `Graved`/`PassGuard`/무덤 이름공간은 이 픽스가 이미 깔아 둔다)**:

> ⚠ **r2 개정 반영**: `Graved`는 이제 **코호트**를 품는다. `sift_corrupt`는 **핀을 묻지 않으므로 코호트를 쓰지
> 않는다**(무덤 이름 아래에서 **내용을 재검증**하는 것이 봉인이다 — 대기가 필요 없다). F-25는 `Graved`의
> `cohort` 필드를 **무시**하면 된다. 그리고 `RenameReceipt`는 **더 이상 존재하지 않으므로** F-25 설계에서
> 영수증 인자를 기대하지 말 것.

```rust
pub(crate) enum Sifted { Quarantined, Healed }

impl Graved<'_> {
    /// 비트로트 격리. **핀을 묻지 않는다** — 무덤 이름 아래에서 내용을 **재검증**한다.
    /// 무덤 이름은 GC 사유이므로 put이 건드릴 수 없다 → TOCTOU 창이 구조적으로 0.
    pub(crate) async fn sift_corrupt(self) -> io::Result<Sifted> {
        let g = self.pass.layout.grave_path(&self.sha);
        let bytes = tokio::fs::read(&g).await?;
        let (b, o) = (self.pass.layout.blob_path(&self.sha), self.pass.layout.objects_dir());
        if hex::encode(Sha256::digest(&bytes)) == self.sha {
            atomic::rename_durable(&g, &b, &o).await?;        // 동시 put이 이미 치유했다 → 되돌린다
            return Ok(Sifted::Healed);                        // (내용주소 → 덮어써도 동일 바이트)
        }
        let corrupt = self.pass.layout.corrupt_dir();
        atomic::mkdir_p_durable(&corrupt).await?;
        atomic::rename_durable(&g, &corrupt.join(&self.sha), &o).await?;
        Ok(Sifted::Quarantined)
    }
}
```

reconcile 비트로트 분기는 `pass.grave(&name).await?.sift_corrupt().await?`로 교체되고,
`Sifted::Quarantined => { pending.remove(&name); stats.quarantined += 1; }` /
`Sifted::Healed => { tracing::warn!(sha=%name, "corrupt blob healed by concurrent put"); }`.

**테스트 설계(그대로 인용 — 재작성 금지)**:

- **T-Q1 (F4의 결정적 증인 — 신규, F-25가 가져간다)**: 손상 blob 심기 → **GC 쪽 훅**(`pre_grave`)에서 park →
  그 사이 동시 put이 같은 sha를 **치유**(intact=false → `write_atomic(blob)`) + 커밋 → GC 재개 → `grave()` →
  `sift_corrupt()`가 **무덤 아래에서 재검증** → 손상 확인 → `.corrupt`로.
  단언: `get_bytes` **Ok**(치유된 `<sha>`가 살아 있다) ∧ `.corrupt/<sha>` 존재 ∧ `quarantined == 1`.
  · **현행 코드(재검증 없이 `rename(blob → .corrupt)`)** → 치유된 inode가 격리됨 → 포인터 + blob 부재 → 404
  → **RED** ← **이것이 F-25의 RED baseline이다**
  · **뮤턴트(`sift_corrupt`의 재검증을 `grave()` 이전으로 이동)** → stale read 기반 → 동일 RED
  (이 뮤턴트는 `Graved` 없이 `blob_path`를 읽어야 하므로 **컴파일은 되지만 테스트가 죽인다**)
- **T-Q2 (recover_graves 내용 검증)**: `<sha>` 손상 ∧ `.gc-grave-<sha>`에 정상 사본 → 무덤이 정본을 덮어쓴다
  → `get_bytes` Ok. ※ **이 픽스의 B-2 acceptance에 이미 있다** — F-25는 **재사용**하라
- **T-Q3 (`is_dir` 가드)**: `.gc-grave-<64hex>` **디렉터리**를 심어도 `recover_graves`가 스킵한다.
  ※ **이 픽스의 B-2 acceptance에 이미 있다** — F-25는 **재사용**하라
- `corrupt_blob_quarantined` 유닛 테스트는 **불변 초록**이어야 한다(`quarantined == 1`, `.corrupt/<sha>` 존재,
  `<sha>` 부재) — 즉 F-25의 플립은 **"동시 치유 put이 있을 때"로 정확히 한정**된다.

## Review Decision Log

### 설계 판정단 (2026-07-12) — 독립 설계안 4개 × 3렌즈 적대적 심사 12건

| 순위 | 설계안 | 점수 | 판정 |
|---|---|---|---|
| **1** | **블롭 핀 + 되돌릴 수 있는 삭제** | **23/30**, 치명 0 | **채택** (→ Codex r1에서 3건 피격 → 아래 개정) |
| 2 | 블롭 스트라이프 락 + 락-내 재확인 | 22.5/30, 치명 0 | 기각 — GC가 락을 쥔 채 전 트리 워크 → put 대기가 `upload_timeout`·`cap.reserve`에 계상 → **두 번째 관측 행동**(400/507) |
| 3 | BlobLocks + lock-then-reverify | 22/30, 치명 0 | 기각 — 2안의 결함을 **더 크게** 가짐 + 취소 경로에서 **F-18 재발** |
| 4 | Store-owned GC + BlobClaims | 18/30, **치명 1** | **탈락** — `write_atomic`의 rename 완료 후 `fsync_dir` 사이 취소 시 커밋 포인터는 디스크에 있는데 claim은 철회 → **fail-OPEN** |

### Codex Plan Review — r1 (2026-07-12) — `needs-attention`, 3 findings

아티팩트: `docs/reviews/reconcile-gc-dedup-race/plan-r1.json` (reviewedSha `74a603f`).
**인간 triage 2026-07-12 — 3건 전부 Accept.**

| # | Finding (요지) | Severity | Decision | Reason | Action |
|---|---|---|---|---|---|
| **P-1** | **취소·실패한 되돌리기가 살아있는 blob을 영구 고립시킨다.** `.objects/.tmp-<unique>` 무덤은 (a) sha를 안 품어 **복구 불가**, (b) **다음 패스가 만료 temp로 오분류**한다(rename이 mtime 보존) → **즉시 삭제 + `temps_deleted`로 집계**. 크래시·복원실패·재시작·**롤백**에서 원래의 영구 유실이 부활 | **critical** | **Accept** | 반박 불가. 코드로 확인: `classify_objects_entry`의 Temp 분기가 `.tmp-` 접두만 본다. 지적한 **롤백 비안전성**은 설계가 아예 고려하지 않았던 축이다 | **`Layout` 소유 · sha를 품는 무덤 이름공간**(`.gc-grave-<sha>`) + 모든 전이 **fsync** + `collect_referenced` **이전**의 `recover_graves` + **fault-injection acceptance 4종**(T-B5: 취소/재시작/복원실패/누수). **scope에 `src/layout.rs` 추가**(권고 그대로 수용) |
| **P-2** | **선언한 after 상태에 두 번째 관측 행동 플립이 있다.** before는 "**참조를 얻은** dedup put"인데 after는 **실패·취소된 put까지** X를 보호한다. 무참조·만료 X에 대해 오늘은 `gc_deleted == 1`인데 개정안은 0 → **capacity/statistics 행동**이 뒤집히고, 반복 실패는 회수를 **무기한 연기**할 수 있다. `flips[]`에는 그 행동이 없다 | **high** | **Accept** | 정확하다. 특히 **ENOSPC**가 자기강화 루프를 만든다(공간을 회수해야 할 때 회수 불가) | **프로토콜 축소**: 흔적을 "**시도**(arm)"에서 "**착지**(landed = 커밋 rename이 `Ok` 반환)"로 좁혔다 → 실패·취소·ENOSPC put은 **흔적을 남기지 않는다** → 오늘과 **바이트 동일**하게 Reap. **T-C1**이 회귀 가드. ⚠ **r2에서 불충분 판정** — `live` 항이 **여전히 보호 술어**여서 겹치는 실패 put이 회수를 연기했다 → **r2/P-2로 재개정**(아래) |
| **P-3** | **제안된 테스트가 load-bearing 순서 뮤턴트 2개를 못 죽인다.** 회귀 테스트는 put을 `collect_referenced` **뒤에** 시작하므로 `begin_pass()`를 수집 뒤로 옮겨도 GREEN이 남는다. decoy 때문에 put이 victim 방문 **전에** 끝나므로 rename **앞의** 사전확인도 마커를 본다. 결정적 증인은 패스 내내 핀을 쥐고 있어 **두 케이스 모두 미커버** | **high** | **Accept** | 정확하다. "확률적 창"을 뮤턴트 킬의 증거로 쓴 것이 잘못이었다 | **결정적 배리어**를 프로덕션 경로에 심었다(`Hooks`, prod=None): **T-B1** · **T-B2** · **T-B4** · **T-C2**. 그리고 **구조적 강제**: `protected()`가 `RenameReceipt`를 **요구** → 사전확인 뮤턴트는 컴파일 불가라고 주장. ⚠ **r2에서 거짓 판정** — `RenameReceipt`는 **아무 것에도 바인딩되지 않은 unit 토큰**이라 `pins.rs`가 **무관한 rename에서** 발급받을 수 있었다 → **r2/P-3으로 재개정**(아래) |

### 적대적 사전검증 (2026-07-12) — 개정 1차안 3렌즈 반증, **3건 전부 fatal**

r1을 봉인했다고 **주장한** 개정 1차안을 재게이트 전에 자체 반증했다. 결과: **전멸**.

| 렌즈 | 점수 | 판정 | 무엇이 죽였나 |
|---|---|---|---|
| **crash** | 3/10 | **FATAL** | "**design-4의 fail-OPEN이 옷만 갈아입고 부활했다**" — `arm()`은 `pass_live`일 때만 흔적을 남기고 `PinGuard::drop`은 `armed`를 감소시킨다. **패스와 패스 사이에 무장하고 죽은(=취소된) put은 흔적을 0으로 남긴다** → P3 시드도 못 보고 `touched`에도 없다 → **유실 시퀀스 존재** |
| **flip** | 5.5/10 | **FATAL** | "**두 번째 플립은 소멸하지 않았다 — 좁아졌을 뿐인데 설계는 소멸했다고 주장한다**"(= P-2가 다른 옷을 입었다). `arm()`이 sticky이므로 **ENOSPC 재시도 폭풍에서 연기가 누적**된다 → 설계 자신의 "starvation 불가" 주장이 **자기 코드에서 거짓** |
| **mutant** | 3/10 | **FATAL** | 타입 강제 주장 2건이 **코드로 반증**됨(F1: "사전확인 뮤턴트는 컴파일 불가"는 **거짓** — `pins.rs` 안에서 재배치하면 된다). **살아남는 load-bearing 순서 뮤턴트 3개.** 그리고 "**GC의 유일한 파괴 연산**" 주장이 거짓 — **격리 분기에 같은 버그가 살아 있다**(→ F4 → **F-25**) |

**세 fatal의 공통 뿌리(= 최종안이 봉인한 것)**:

> **흔적과 커밋의 비대칭.** `arm()`(흔적)은 취소 시 `PinGuard::drop`으로 **즉시 죽는데**, 커밋(`rename`)은
> `spawn_blocking`이라 **취소를 뚫고 착지한다**(`tokio/src/fs/mod.rs:312`). 흔적과 커밋이 **다른 스레드에서
> 다른 시각에** 결정된다.

**봉인**: 커밋 rename과 핀의 수명을 **하나의 무취소 blocking 클로저**에 가둔다(tokio:
`task/blocking.rs:107-120` — *시작된 blocking 태스크는 abort 불가*). 그러면 흔적을 "시도"가 아니라
**"착지(rename이 `Ok` 반환)"**라는 **확정 사실**로 정의할 수 있다 → crash 유실 시퀀스 **표현 불가**,
flip의 ENOSPC 누적 **잔량 0**, mutant의 M3/M5 클래스 **소멸**(`armed` 맵·P3 시드가 통째로 사라진다).
남은 F1(영수증)은 **모듈 경계**로 봉인하되 그 경계를 **정직히 명시**한다.
⚠ **이 안이 r2에 제출됐고, r2가 그 중 두 곳을 다시 깼다** — 아래 r2 표.

### Codex Plan Review — r2 (2026-07-12) — `needs-attention`, 2 findings

아티팩트: `docs/reviews/reconcile-gc-dedup-race/plan-r2.json` (reviewedSha `9f435b5`, reviewedTree `11fbd50`).
Codex 요약: *"Do not ship the plan yet. **P-1 and the committed RED witness are sound**, but **P-2 and P-3 remain
unresolved blockers.** No new critical issue or materially simpler safe fix was found; naive rechecks and coarse locks
retain TOCTOU or change blocking behavior."*
**인간 triage 2026-07-12 — 2건 전부 Accept.** (P-1은 **해소 확인** — 무덤 이름공간·복구는 **건드리지 않는다**.)

| # | Finding (요지) | Severity | Decision | Reason | Action (이 개정에서 실제로 한 것) |
|---|---|---|---|---|---|
| **P-2** (잔존) | **겹치는 실패 put이 여전히 GC 행동을 바꾼다.** 계약은 **무덤 rename 시점에 살아있는 핀**이 있으면 X를 복원하는데, **그 시점에는 그 put의 결말을 모른다.** put이 X를 관측하고, GC가 settle·복원하는 내내 살아있다가, 그 뒤 포인터 rename이 **실패**해 포인터를 하나도 만들지 않을 수 있다. 매 패스에 겹치는 실패가 반복되면 **회수가 무한정 연기**된다. **T-C1은 이걸 못 잡는다** — 실패한 put이 **이미 반환되고 live 핀이 죽은 뒤에** reconcile을 돌리기 때문 | **high** (0.99) | **Accept** | 반박 불가. `live`를 **"성공할 결말"의 프록시**로 쓴 것이 잘못이었다. 프록시는 결말이 아니다 | **`live`를 보호 술어에서 제거**하고 **대기 조건으로 강등**했다. 핀에 **단조 id**를 주고, `PassGuard::grave()`가 **무덤 rename 직후** 그 sha의 live id를 스냅샷한다(= **코호트**, 고정·유한). `Graved::settle(self)`는 **코호트가 전부 종료될 때까지 `Notify`로 await**한 뒤 **`landed(sha)` 하나만** 본다 → **결말을 알고 나서 판정**. 무덤 **이후** 핀은 `blob_intact`에서 **ENOENT를 보고 바이트를 재기록**하므로 **자급자족** → 코호트 밖(핵심 사실 **D**). 유실 0 · **연기 0**. 증인: **T-C3**(park → 무덤 → settle 대기 → **rename 강제 실패** → **`gc_deleted == 1`**). 대기 상계·성능·데드락 부재는 §대기의 상계 / §남은 위험 6. ⚠ **r3에서 P-2 봉인 자체는 sound 판정** — **그러나 이 Action이 적은 "대기 상계"가 거짓이었다**(`upload_timeout`은 abort 불가능한 blocking 클로저를 죽이지 못한다) → **r3/P-4로 대기를 유한화**(아래) |
| **P-3** (잔존) | **`RenameReceipt`가 주장한 순서를 강제하지 못한다.** 그것은 **아무 것에도 묶이지 않은 unit 토큰**이고 범용 `pub(crate) rename_durable`이 **임의의 경로에 대해** 그것을 만든다 → `pins.rs` 안의 코드가 **무관한 rename에서 영수증을 얻어** blob→무덤 전이 **이전에** 위험한 확인을 할 수 있고, `atomic.rs`를 건드리지 않고도 **컴파일된다**. **T-B2도 그 창을 열지 않아 초록으로 남는다**(put이 `pre_grave` 이전에 이미 live이므로 어떤 사전확인이든 `live`를 본다) | **high** (0.97) | **Accept** | 반박 불가. **증거가 전이에 바인딩되지 않으면 증거가 아니다.** 그리고 개정 1차안이 mutant 렌즈 F1에서 죽은 것과 **같은 종류의 과장**을 우리가 다시 했다 | **`RenameReceipt` 삭제.** 보호 판정을 **`Graved`의 메서드로만** 노출한다: `Graved`는 **`PassGuard::grave()`의 rename이 성공했을 때만** 태어나고(private 필드 · `pins.rs` 밖 생성자 0 · derive 0), **자기 `sha`와 코호트를 품는다** → 판정이 **전이·sha에 바인딩**된다. API는 **`Graved::settle(self)` 하나뿐**(자기 자신을 **소비**), **`BlobPins::protected()` 제거** → `reconcile.rs`는 사전확인을 **표현조차 못 한다**. **T-B2 개정**: GC를 `pre_grave`에서 park하고 **그 안쪽에서** put이 시작·완주 → 사전확인 시점에 `live`·`landed` **둘 다 비어 있다** → 뮤턴트는 **Reap → 404 → RED**. ⚠ 봉인은 **모듈 경계**다(`pins.rs`에 새 API를 추가하면 풀린다) — **과장하지 않고 명시**, T-B2가 **2차 방어선** |

**r2가 확인해 준 것(그대로 유지)**: P-1 봉인(무덤 이름공간·`recover_graves`)은 **sound** · 커밋된 RED 증인
(`red.sha 6545808`)도 **sound** · **더 단순한 안전한 픽스는 없다**(naive recheck는 TOCTOU 잔존, coarse lock은
blocking 행동 변경 → 기각 근거 §왜 락 계열이 아닌가와 **일치**).

### 인간 결정 — **수동 3라운드 승인** (2026-07-12, 하드룰 4 (b) 경로)

Codex plan gate는 **2라운드 재리뷰 루프**를 상한으로 둔다(r1 → 개정 → r2). r2가 다시 `needs-attention`을 냈으므로
하드룰 4의 갈림길이다: **(a)** 설계를 접고 재라우팅, 또는 **(b)** **인간이 명시적으로 추가 라운드를 승인**.

| 항목 | 내용 |
|---|---|
| **결정** | **(b) — 수동 3라운드 승인.** 개정 범위를 **P-2·P-3 두 결함으로만 엄격 제한**한다 |
| **근거** | r2의 두 finding은 **국소적이고 정확하다**. P-1(치명)은 **해소됐고**, RED 증인·근본 원인·증분 분해·Preserved Contract는 r2가 **문제 삼지 않았다**. 재라우팅은 이미 **검증된 뼈대를 버리는** 비용이 크다. 그리고 r2 스스로 *"no materially simpler safe fix was found"*라고 적었다 — **대안이 더 낫다는 증거가 없다** |
| **제약 (엄수)** | 이 3라운드에서 **P-2·P-3 외에는 아무것도 바꾸지 않는다.** 설계가 커지면 새 결함이 들어온다. frontmatter · Root cause · Regression test · D-1~D-4 · F-25~F-28 · Preserved Contract의 **"부분 해결" 선언** · **B-3의 격리 분기 봉인 제외**(D-4, 하드룰 10) — **전부 불변** |
| **승인자** | 인간 (2026-07-12) |
| **다음** | r3 plan review. **머신 소유 GREEN 검증은 재실행하지 않는다**(r2 `next_steps` 지시 그대로) |

### Codex Plan Review — r3 (2026-07-12) — `needs-attention`, 1 finding (**신규**)

아티팩트: `docs/reviews/reconcile-gc-dedup-race/plan-r3.json` (reviewedSha `8776ac9`, reviewedTree `c19a3eb`).
Codex 요약: *"Do not ship. **P-2, P-3, and the committed RED witness now look sound**, but **the new cohort wait can
freeze reconciliation indefinitely.** No materially simpler safe root fix was found."*
**인간 triage 2026-07-12 — 1건 Accept.**

> **r3가 sound로 확인해 준 것(그대로 유지 — 이 개정에서 건드리지 않는다)**: **P-1**(무덤 이름공간·`recover_graves`)
> · **P-2**(코호트 모델 · `live` 강등 · `landed` 단일 보호 술어) · **P-3**(`RenameReceipt` 삭제 ·
> `Graved::settle(self)` 단일 API) · **커밋된 RED 증인**(`red.sha 6545808`) · **더 단순한 안전한 픽스는 없다**.
> ⇒ **P-4 하나만** 봉인한다. **세 라운드에 걸쳐 검증된 뼈대를 건드리는 것이 가장 큰 리스크다.**

| # | Finding (요지) | Severity | Decision | Reason | Action (이 개정에서 실제로 한 것) |
|---|---|---|---|---|---|
| **P-4** (신규) | **무한정 코호트 대기가 커밋된 객체를 좌초시키고 GC를 정지시킬 수 있다.** 주장된 **`< gc_grace` 상계는 거짓**이다: `upload_timeout`은 **호출자 퓨처를 드롭할 뿐**인데, 계획은 **의도적으로** `PinGuard`를 **abort 불가능한 `spawn_blocking` 클로저**로 옮겨 write·rename·fsync 내내 보유한다. 큐에 걸리거나 **멈춘 파일시스템 연산**이 코호트 멤버를 **무한정 살아있게** 만들 수 있다. GC는 이미 X를 무덤으로 옮겼으므로 **포인터 rename 이후에 stall이 나면 실재하는 포인터가 무한정 404**를 내고, **`pass_lock`이 이후의 모든 복구·GC 패스를 막는다**. **T-C3와 T-B2는 훅을 항상 해제하므로 두 증인 모두 이 실패를 커버하지 못한다** | **high** (0.99) | **Accept** | **반박 불가.** 그리고 **우리가 이 사실을 이미 알고 있었다** — §"코드로 확인한 근거" 표에 *"시작된 blocking 태스크는 **abort 불가**"*라고 **직접 인용해 놓고**, 바로 그 성질을 **유실 창을 닫는 도구**로 쓰면서, **같은 성질이 대기의 상계를 파괴한다는 것은 보지 못했다.** 무취소 커밋의 **대가**를 계상하지 않은 것이다. r2안의 상계 표(`< gc_grace`)는 **우리 자신의 코드가 반증하는 주장**이었다 | **`Graved::settle()`을 유한·fail-CLOSED로.** ① **`landed` 확정 → 대기 0**(즉시 복원). `Notify`를 **`landed` 삽입에서도** 울려 대기 **도중** 착지도 **즉시** 깨운다 → *"실재하는 포인터가 무한정 404"* 창을 **닫는다**. ② 그 외에는 **명시적 `settle_timeout`**(= `upload_timeout + 60s`, `main.rs`가 cfg에서 파생·주입)까지만 대기 — ⚠ **`upload_timeout`이 상계가 **아님**을 문서에 정직하게 명시**(§settle_timeout). ③ 타임아웃 → **fail-CLOSED**: **무덤을 정본으로 복원**(보존 최우선) · tombstone **유지**(D-2) · **`gc_deleted` 무증가**. ④ **패스는 반드시 해제**(`PassGuard` drop → `pass_lock`) → *"이후의 모든 복구·GC 패스를 막는다"*가 **불가능**해진다. ⑤ **`tracing::error!(sha, cohort_size, waited_ms)`** — ⚠ **`ReconcileStats` 필드는 추가하지 않는다**(전수 `assert_eq!` 3곳 = 두 번째 플립). 전파는 **로그 + 루프 계속**(중단하면 멈춘 핀 하나가 **다른 blob의 GC를 막는다** = P-4를 `?`로 갈아입힌 것 — §에러 표면 논증). ⑥ **degraded-path 연기를 특성화**(§degraded-path 연기 · §남은 위험 12). **증인: T-P4a**(rename **이전** 영구 스톨 — 뮤턴트 = 무한 대기 → 후속 패스가 영영 안 끝남 → **RED**) · **T-P4b**(rename **이후** 스톨, `landed`=true — 뮤턴트 = 즉시복원 제거 → 타임아웃 창 내내 **404** → **RED**). 훅을 **절대 해제하지 않는** park(§park 함정)로 r3가 지적한 *"T-C3/T-B2는 훅을 항상 해제한다"*를 정면으로 메운다 |

### 인간 결정 — **round 4 승인** (2026-07-12, 하드룰 4 (b) 경로)

| 항목 | 내용 |
|---|---|
| **결정** | **(b) — round 4 승인.** 개정 범위를 **P-4 한 결함으로만 엄격 제한**한다 |
| **근거** | r3는 **P-1·P-2·P-3과 RED 증인을 전부 sound로 확인**했다 — 뼈대가 세 라운드에 걸쳐 검증됐다. P-4는 **국소적이고 정확하며**, **P-2 봉인이 도입한 코호트 대기 자체의 결함**이지 설계 방향의 오류가 아니다(대기는 여전히 옳다 — **유한하게** 해야 할 뿐이다). 재라우팅은 **검증된 뼈대를 버리는** 비용이 크고, r3도 *"no materially simpler safe root fix was found"*라고 적었다 |
| **제약 (엄수)** | 이 라운드에서 **P-4 외에는 아무것도 바꾸지 않는다.** 코호트 모델 · `landed` 단일 보호 술어 · `Graved::settle(self)` 단일 API · `RenameReceipt` 제거 · 무덤 이름공간 · 복구 경로 · frontmatter · Root cause · Regression test · D-1~D-4 · F-25~F-28 · **"부분 해결" 선언** · **B-3의 격리 분기 봉인 제외**(D-4) — **전부 불변**. **`ReconcileStats`에 필드를 추가하지 않는다**(두 번째 플립 금지). **`bugfix-lock.json`의 `scope[]`를 넓히지 않는다**(→ `src/config.rs`에 새 노브를 만들지 않는다 = F-29) |
| **승인자** | 인간 (2026-07-12) |
| **다음** | r4 plan review. **머신 소유 GREEN 검증은 재실행하지 않는다** |

### Codex Plan Review — r4 (2026-07-13) — `needs-attention`, 1 finding (**테스트 안무**)

아티팩트: `docs/reviews/reconcile-gc-dedup-race/plan-r4.json` (reviewedSha `cdbac3b`, reviewedTree `3a544d2`).
Codex 요약: *"Do not ship. The committed RED record matches `red.sha`, its tree, and the exact DATA LOSS assertion,
but **P-4's T-P4b witness is red for the wrong reason and does not exercise settlement**. **No other new critical
P-4 issue found. Simpler alternative: none; repair the test choreography without changing the fix model.**"*
**인간 triage 2026-07-13 — 1건 Accept.**

> **r4가 sound로 확인해 준 것(그대로 유지 — 이 개정에서 건드리지 않는다)**: **P-4의 fix model 전체** —
> `settle()`의 **유한 대기** · `landed` **즉시복원** · **타임아웃 fail-CLOSED 복원** · **패스 해제** ·
> `Graved::settle(self)` **단일 API** · **코호트 모델** · **무덤 이름공간** · **복구 경로** · 커밋된 **RED 증인**.
> **새 P-4 결함 0건 · 더 단순한 대안 없음.** ⇒ **`src/` 설계는 한 글자도 바꾸지 않는다.**
> **고칠 것은 T-P4b 하나의 단계 순서와 단언뿐이다.** 네 라운드에 걸쳐 검증된 뼈대를 건드리는 것이 가장 큰 리스크다.

| # | Finding (요지) | Severity | Decision | Reason | Action (이 개정에서 실제로 한 것) |
|---|---|---|---|---|---|
| **P-5** (신규) | **T-P4b가 참조 스냅샷 *이전에* 포인터를 착지시킨다.** 그 테스트의 2~4단계는 메타 rename과 `landed` 삽입이 `run_once_at` **시작 전에** 끝나도록 짜여 있다. 그런데 실행 순서상 **`collect_referenced`가 블롭 처리보다 먼저** 돌므로 **그 스냅샷이 이미 보이는 포인터를 포함한다** → reconcile은 **참조됨 분기**(`refs.contains(&name)` → `pending.remove`)를 타고 **`grave()`도 `settle()`도 호출하지 않는다**. 기대한 복원 로그가 없으니 T-P4b는 **엉뚱한 이유로 RED**이고, **그 단언이 없으면 landed-우선 복원을 제거해도 초록으로 남을 수 있다.** **즉시 복원도, no-404 P-4 봉인도 아무것도 증명하지 못한다** | **critical** (0.99) | **Accept** | **반박 불가.** 그리고 **우리가 이 순서를 문서에 직접 적어 놓고도 보지 못했다** — §6의 GC 루프는 `PassGuard::begin`(= `recover_graves` → **`collect_referenced`**)을 **블롭 루프보다 먼저** 돌리고(`reconcile.rs:55`도 오늘 그렇다), T-B2는 *"`collect_referenced`는 `pre_grave`보다 **구조적으로 먼저** 끝난다"*고 **정확히 그 사실에 기대어** 결정성을 논증했다. **같은 사실을 T-P4b에서는 거꾸로 밟았다.** 게다가 증상이 **조용하다**: 복원 테스트의 기대값은 `gc_deleted == 0`인데 **참조됨 분기로 새도 `gc_deleted == 0`**이다 — **두 세계가 단언으로 구별되지 않았다** | **T-P4b를 두 증인으로 분리**하고 **둘 다 reconcile을 먼저 시작**시킨다(= `collect_referenced`가 포인터를 **놓친 뒤** put을 진행). **T-P4b-1** = 무덤 시점에 **`landed`가 이미 true** ∧ **핀 live** → **대기 0 · 즉시 복원**(`pre_grave` park 안에서 put을 커밋 rename까지 진행시켜 `in_commit_post_landed`에 park). 뮤턴트(즉시복원 제거 / `landed` 삽입 제거) → **30s 예산을 전부 태운다** → `timeout(2s)` `Err` ∧ 로그 문자열 역전 → **RED ×2**. **T-P4b-2** = **settlement가 이미 대기 중일 때** 착지 → **`landed` 알림이 대기를 깨운다**(put을 `in_commit_pre_rename`에 park → GC가 무덤 rename 후 **대기 진입**(`timeout(200ms,&mut gc)` pending **단언**) → **그제서야** 해제 → 착지 후 **`in_commit_post_landed`에 계속 park**해 **핀 drop이라는 대체 기상 수단을 제거**) → **`notify_waiters()` 제거 뮤턴트가 결정적으로 RED**(r3에서 *equivalent*로 분류했던 것 — **철회**. 단 **지연 결함이지 유실이 아님**을 명시). **전수 점검**: T-B1/T-B2/T-B4/T-C1/T-C2/T-C3/T-B5에 **같은 함정이 있는지 전부 확인 → 순서 결함 없음**(전부 포인터가 collect **이후에** 착지하거나 **아예 착지하지 않는다**). 그러나 **그 사실을 테스트가 스스로 증명하지 않았다** → **모든** 배리어 테스트에 **삭제 분기 자기검증**(`stats.referenced`의 **정확한 값** + `post_grave` 훅의 `graved` 관측)을 **의무화**(§삭제 분기 자기검증 · `Graved` 봉인 체크리스트 ⑦). **`src/`는 무변경 · 훅 0개 추가**(T-P4b-1/-2는 기존 훅만 쓴다) |

### 인간 결정 — **round 5 승인** (2026-07-13, 하드룰 4 (b) 경로)

| 항목 | 내용 |
|---|---|
| **결정** | **(b) — round 5 승인.** 개정 범위를 **T-P4b의 테스트 안무 하나로만** 엄격 제한한다 |
| **근거** | r4는 **P-4의 fix model 자체를 sound로 판정**했고(*"No other new critical P-4 issue found"*), **더 단순한 대안이 없음**을 확인했으며, 처방까지 **명시**했다(*"repair the test choreography **without changing the fix model**"*). 결함은 **설계가 아니라 증인**에 있다 — 그리고 **증인이 아무것도 증명하지 못하는 것은 치명적이다**(봉인을 제거해도 초록일 수 있다). 재설계는 **네 라운드에 걸쳐 검증된 뼈대를 버리는** 순손실이다 |
| **제약 (엄수)** | **`src/`·`tests/` 코드는 한 줄도 바꾸지 않는다**(이 라운드는 **문서 전용**). `settle()`의 유한 대기 · `landed` 즉시복원 · 타임아웃 fail-closed 복원 · 패스 해제 · `Graved::settle(self)` 단일 API · 코호트 모델 · 무덤 이름공간 · 복구 경로 · **`Hooks` 필드 7개**(**추가 금지** — T-P4b-1/-2는 기존 훅으로 짠다) · frontmatter · Root cause · Regression test · **T-P4a**(r4에서 문제 없었다 — **그대로 유지**) · D-1~D-4 · F-25~F-29 · **"부분 해결" 선언** · **B-3의 격리 분기 봉인 제외**(D-4) — **전부 불변**. `ReconcileStats` **필드 추가 금지**. `bugfix-lock.json`의 `scope[]` **불변** |
| **승인자** | 인간 (2026-07-13) |
| **다음** | r5 plan review. **머신 소유 RED/GREEN 검증은 재실행하지 않는다**(r4 `next_steps` 지시 그대로: *"leave machine-owned regression and GREEN execution untouched"*) |

### Codex Plan Review — r5 (2026-07-13) — `needs-attention`, 1 finding (**랑데부 신호**)

아티팩트: `docs/reviews/reconcile-gc-dedup-race/plan-r5.json` (reviewedSha `9ad2f68`, reviewedTree `8550423`).
Codex 요약: *"Do not ship: **T-P4b-1 is sound and the RED record matches `red.sha`**, but **T-P4b-2 can fail correct
code before exercising the landed notification**. **Simpler alternative: add one explicit pre-rename arrival
handshake while retaining the fix model.** Open question: none."*
**인간 triage 2026-07-13 — 1건 Accept.**

> **r5가 sound로 확인해 준 것(그대로 유지 — 이 개정에서 건드리지 않는다)**: **T-P4b-1**(역할·단계·단언·뮤턴트
> **전부**) · **커밋된 RED 증인**(`red.sha 6545808` — 트리·단언까지 일치) · **P-4의 fix model**(r4에서 이미 sound)
> · **더 단순한 대안 없음**. Codex 자신이 처방에 못박았다: *"**This needs no new production hook or fix-model
> change.**"* ⇒ **`Hooks` 필드 7개 · `settle()` · 코호트 · `landed` · 무덤 이름공간 · 복구 경로 — 전부 불변.**
> **고칠 것은 테스트가 「다음 단계로 언제 넘어가는가」 하나뿐이다.**

| # | Finding (요지) | Severity | Decision | Reason | Action (이 개정에서 실제로 한 것) |
|---|---|---|---|---|---|
| **P-6** (신규) | **T-P4b-2가 put이 `park_A`에 도달했음을 증명하지 못한다.** `park_A`는 *"풀 수 있는 대기"*로만 서술돼 있고 **도착 신호가 없다.** 3단계가 put을 spawn하고 4단계가 **곧바로** reconcile을 spawn하는데, **`tokio::spawn`은 spawn된 태스크를 동기적으로 폴링하지 않는다** → GC가 **put이 X를 핀하기도 전에** 무덤으로 옮겨 **빈 코호트**를 캡처하고 **즉시 reap**할 수 있다. 그러면 pending 단언(또는 후속 단언)이 **`notify_waiters()` 제거가 아니라 셋업 스케줄링 때문에** 실패한다. 이는 *"두 증인 모두 reconcile을 먼저 시작한다"*는 **계획 자신의 약속과도 모순**된다. 따라서 이 증인은 **엉뚱한 이유로 RED**가 될 수 있고 **P-5를 확실히 봉하지 못한다** | **critical** (0.99) | **Accept** | **반박 불가.** 그리고 **우리는 r4에서 정확히 반대편 함정(참조됨 분기 누수)을 봉인하면서 이 함정을 새로 만들었다** — T-P4b-1은 *"reconcile 먼저 → `gc_arrived` await"*로 **올바르게** 짰으면서, T-P4b-2는 **put을 먼저 spawn하고 기다리지 않았다.** 두 증인이 **같은 약속**(*"둘 다 reconcile을 먼저 시작한다"* — §T-P4b 분리 블록)을 내걸었는데 **한쪽만 지켰다.** 근본 병은 r4의 것과 **다르다**: 그건 **순서**(포인터가 collect 전에 착지), 이건 **스케줄링**(spawn ≠ polled) — **`referenced`/`graved` 자기검증으로는 잡히지 않는다**(빈 코호트 reap도 `referenced == 0` ∧ `graved == [X]`를 **만족한다**) | **T-P4b-2를 승인된 순서로 재작성**: reconcile spawn → **`pre_grave` 도달(`gc_arrived`) await** → put spawn → **`park_A`가 내보내는 `pre_rename_reached` await** → `pre_grave` 해제 → **`post_grave`(`graved_reached`) await + pending 프로브** → `park_A` 해제 → **`post_landed_reached` await**(핀은 `park_B`가 살려 둔다) → `timeout(2s, gc)`. **spawn만 하고 넘어가는 지점 0개.** 그리고 **일반화**한다 — **§랑데부 규율**(모든 park에 **「도착 신호 + 해제 신호」 쌍** 의무화 · 신호는 **전부 테스트 쪽 채널** · **체크리스트 표**로 다음 개정이 조용히 약화시키지 못하게 못박음). **전수 재점검(이 렌즈로 다시)**: **T-B2 · T-B4 · T-C2 · T-C3 · T-P4a · T-B5① 에서 같은 함정 발견 → 전부 도착 신호 추가**. ⚠ **T-C3가 최악이었다** — 빈 코호트 reap의 `gc_deleted == 1`이 **기대값과 우연히 일치**해 **조용히 GREEN**으로 남고 *"live를 보호 술어로 되돌리는"* 뮤턴트가 **살아남는다**(r2/P-2가 명시 요구한 증인이 **아무것도 지키지 않는다**). **T-P4b-1은 이미 두 도착 신호를 갖고 있어 무변경**(r5 sound 판정과 일치) · T-B1/T-C1/T-B5②③④/T-Q2/T-Q3는 **park·spawn 지점 없음 → 함정 구조적 부재**(체크리스트에 *"확인했고 없음"*으로 명시). **`src/`·`tests/` 무변경 · 프로덕션 훅 0개 추가**(Codex 처방 그대로) |

### 인간 결정 — **round 6 승인** (2026-07-13, 하드룰 4 (b) 경로)

| 항목 | 내용 |
|---|---|
| **결정** | **(b) — round 6 승인.** 개정 범위를 **배리어 테스트의 랑데부 안무 하나로만** 엄격 제한한다 |
| **근거** | r5는 **T-P4b-1과 RED 증인을 sound로 확인**했고, **처방까지 명시**했다(*"add one explicit pre-rename arrival handshake **while retaining the fix model**"* · *"**This needs no new production hook or fix-model change.**"*). 결함은 **설계가 아니라 증인의 스케줄링 전제**에 있다 — 그리고 **증인이 엉뚱한 이유로 RED가 되거나(T-P4b-2) 조용히 GREEN으로 남는 것(T-C3)은 치명적이다**(봉인을 제거해도 통과할 수 있다). **다섯 라운드에 걸쳐 검증된 뼈대를 건드리는 것이 가장 큰 리스크다** |
| **제약 (엄수)** | **`src/`·`tests/` 코드는 한 줄도 바꾸지 않는다**(이 라운드도 **문서 전용**). **새 프로덕션 훅 금지** — `Hooks` 필드 **7개 불변**(도착 신호는 **테스트가 그 7개에 꽂는 클로저 안**에서만 산다). `settle()`의 유한 대기 · `landed` 즉시복원 · 타임아웃 fail-closed 복원 · 패스 해제 · `Graved::settle(self)` 단일 API · 코호트 모델 · 무덤 이름공간 · 복구 경로 · frontmatter · Root cause · Regression test · Single-Flip Contract · Preserved Contract · **"부분 해결" 선언** · D-1~D-4 · F-25~F-29 · **B-3의 격리 분기 제외**(D-4) · `bugfix-lock.json`의 `scope[]` — **전부 불변**. **T-P4b-1**(r5 sound) · **T-P4a의 역할·단언·뮤턴트**는 **그대로 유지**한다 |
| **⚠ 범위 판정 (명시)** | 사전 지시의 *"유지할 것"* 목록에 **T-P4a**가 들어 있으나, **전수 재점검 지시**가 T-P4a를 **명시적으로 스윕 대상에 포함**시켰고 **실제로 함정이 있었다**(3단계 spawn → 4단계 `run_once_at` — 그 사이 await 없음). **후자를 따른다**: T-P4a에 **도착 await(3′) 한 줄만 추가**하고 **역할·단언 ①~⑤·뮤턴트 분석은 한 글자도 바꾸지 않았다.** *"유지"* 는 **설계 동결**이지 **결함 보존**이 아니다 |
| **승인자** | 인간 (2026-07-13) |
| **다음** | r6 plan review. **머신 소유 RED/GREEN 검증은 재실행하지 않는다**(r5 `next_steps` 지시 그대로: *"Repeat the scoped round-5 plan review without running machine-owned RED or GREEN tests."*) |

### Codex Plan Review — r6 (2026-07-13) — `needs-attention`, 1 finding (**취소 완료**)

아티팩트: `docs/reviews/reconcile-gc-dedup-race/plan-r6.json` (reviewedSha `59aad86`, reviewedTree `bb52e60`).
Codex 요약: *"Do not ship: **P-6 itself is resolved and the committed RED witness remains sound**, but **T-C2 still
assumes `abort()` completes cancellation synchronously.** **Simpler alternative: await the aborted task before
starting GC; no production hook or fix-model change.** Open question: none."*
**인간 triage 2026-07-13 — 1건 Accept.**

> **r6이 sound로 확인해 준 것(그대로 유지 — 이 개정에서 건드리지 않는다)**: **P-6 해소**(랑데부 신호 · T-P4b-2의
> 재작성 · 도착/해제 쌍 의무화) · **커밋된 RED 증인**(`red.sha 6545808`) · **P-4의 fix model**(r4에서 이미 sound) ·
> **T-P4b-1**(r5에서 이미 sound) · **더 단순한 대안 없음**. Codex가 **세 라운드 연속으로** 못박았다:
> *"**no production hook or fix-model change**"*. ⇒ **`Hooks` 필드 7개 · `settle()` · 코호트 · `landed` ·
> 무덤 이름공간 · 복구 경로 — 전부 불변.** **고칠 것은 테스트가 「무엇을 관측하고 나서 다음 단계로 가는가」뿐이다.**

| # | Finding (요지) | Severity | Decision | Reason | Action (이 개정에서 실제로 한 것) |
|---|---|---|---|---|---|
| **P-7** (신규) | **T-C2가 호출자 취소가 *완료*됐음을 증명하지 못한다.** `pre_rename_reached`는 blocking 클로저가 **시작**됐음만 증명하는데, 계획은 곧바로 `abort()`를 부르고 **즉시 GC를 spawn**한다. **tokio 1.52.3은 `JoinHandle::abort()`가 취소를 *스케줄만* 하며 task local이 드롭되기 전에 반환할 수 있다고 명시**한다. ⇒ T-C2가 죽이겠다고 선언한 바로 그 **caller-owned `PinGuard` 뮤턴트**에서 GC가 **아직 살아있는 가드를 코호트로 포착**하고 settlement를 park했다가, 풀린 클로저가 포인터를 착지시킨 뒤 **복원**한다 → **테스트가 GREEN으로 남는다** → **취소로 인한 데이터 손실 경로가 그대로 출하된다.** `graved_reached`와 pending 프로브는 **GC의 상태**를 증명할 뿐 **취소 완료**를 증명하지 않는다 | **critical** (0.99) | **Accept** | **반박 불가.** 그리고 **이것은 r5/P-6과 같은 병의 세 번째 변종이다** — 우리는 *"`spawn` ≠ 폴링됨"*을 봉인하면서 **바로 옆 칸의 *"`abort()` ≠ 취소 완료"*는 보지 못했다.** 병의 이름은 **「비동기 연산의 *개시*를 그것의 *완료*로 착각한다」**이며, r4(순서) → r5(스케줄링) → r6(취소)로 **매번 옷만 갈아입고 돌아왔다.** ⇒ **한 테스트를 고치는 것으로는 부족하다** | **① T-C2 안무 확정**: `abort()` → **`timeout(2s, &mut put)` = `Ok(Err(e))` ∧ `assert!(e.is_cancelled())`** → *그 다음에* GC spawn. `park_A`는 **계속 막아 둔다** → 올바른 코드에서는 시작된 클로저가 **핀을 든 채 detach**되고(`blocking.rs:107-120`), caller-owned 뮤턴트에서는 **가드가 드롭된다** → 빈 코호트 → **Reap → 뒤늦은 착지 → 404 → RED**(독립 신호 3개). **② 클래스를 쓸었다**(인간 요구) — **8개 함정 항목 × 전 배리어 테스트** 매트릭스(§「개시 ≠ 완료」 클래스 전수 점검). **T-C2 외에 5건을 더 찾았다**: **T-B5①**(같은 P-7 — abort 후 `pass_lock`을 쥔 채 새 패스 시작 → **hang**) · **T-B5④**(`drop(pass)` 누락 → **hang**) · **T-P4a**(뮤턴트 RED 논증이 **거짓** — `timeout`의 `Err`는 안쪽 퓨처를 **드롭**해 `pass_lock`을 **푼다**) · **T-P4b-1**(`oneshot` park은 `Fn` 훅에 **컴파일되지 않는다** → `Notify`) · **T-B1/T-B2/T-B4/T-C3**(버려진 핸들이 **패닉을 삼킨다** → `JoinError` 언랩 + 결과 단언 의무화; T-C3는 **put 완주 await**까지 추가). **③ 규율을 갱신**: §랑데부 규율의 **첫 줄을 「규칙 0 — 개시 ≠ 완료」로 올리고**, 체크리스트 표에 **「취소 완료 await」 열**을 추가했다. **`Graved` 봉인 체크리스트에 ⑨**를 신설(*"새 배리어 테스트는 8개 함정을 1:1 대조하고 매트릭스에 행을 추가한다"*). **④ 보조정리 L**을 명문화 — *"put 완주 await ⇒ 핀 사망"*은 참이고 *"취소 ⇒ 핀 사망"*은 **거짓**이다(올바른 코드). **그 비대칭이 T-C2의 명제 그 자체다.** **`src/`·`tests/` 무변경 · 프로덕션 훅 0개 추가**(Codex 처방 그대로) |

### 인간 결정 — **round 7 승인** (2026-07-13, 하드룰 4 (b) 경로)

| 항목 | 내용 |
|---|---|
| **결정** | **(b) — round 7 승인.** 다만 개정 범위를 **"T-C2 한 테스트"가 아니라 「개시 ≠ 완료」 함정 클래스 *전체*의 전수 점검**으로 **명시적으로 확장**한다 |
| **근거** | **같은 병이 세 번째로 돌아왔다**(r4 순서 → r5 스케줄링 → r6 취소). 매번 **한 증인만** 고쳤고, 매번 **다음 라운드에 다른 옷을 입고** 나타났다. **Codex가 지적한 곳만 고치는 것은 이 병에 대해 증명된 실패 전략이다.** ⇒ 이번에는 **함정을 8개 항목으로 열거하고 전 배리어 테스트와 1:1 대조**한다. 그 판단이 **옳았음이 즉시 입증됐다** — Codex가 지목한 **T-C2 외에 5건**을 더 찾았고, 그 중 **T-B5①은 P-7과 완전히 같은 함정**(abort 후 취소 완료 미대기)이며 **T-B5④·T-P4b-1은 테스트를 hang 또는 컴파일 실패**시켰을 결함이다. **한 테스트만 고쳤다면 r7에서 다시 물렸을 것이다** |
| **제약 (엄수)** | **`src/`·`tests/` 코드는 한 줄도 바꾸지 않는다**(이 라운드도 **문서 전용**). **새 프로덕션 훅 금지** — `Hooks` 필드 **7개 불변**(도착·취소완료 신호는 **테스트가 그 7개에 꽂는 클로저 안**과 **테스트 함수 본문**에서만 산다). `settle()`의 유한 대기 · `landed` 즉시복원 · 타임아웃 fail-closed 복원 · 패스 해제 · `Graved::settle(self)` 단일 API · 코호트 모델 · 무덤 이름공간 · 복구 경로 · frontmatter · Root cause · Regression test · Single-Flip Contract · Preserved Contract · **"부분 해결" 선언** · D-1~D-4 · F-25~F-29 · **B-3의 격리 분기 제외**(D-4) · `bugfix-lock.json`의 `scope[]` — **전부 불변**. **T-P4b-1·T-P4b-2의 역할·단언·뮤턴트 분석**과 **T-P4a의 역할·단언 ①~⑤·뮤턴트 표적**은 **유지**한다 |
| **⚠ 범위 판정 (명시)** | 전수 점검이 **T-P4a의 뮤턴트 *논증* 1건이 거짓임**을 드러냈다(`timeout` Err → 퓨처 드롭 → `pass_lock` 해제 → *"후속 패스가 막힌다"*는 **성립하지 않는다**). **유지 대상은 "역할·단언·뮤턴트 표적"이지 "틀린 설명"이 아니다** — r6의 범위 판정과 **동일한 원칙**을 적용해 **설명만 정정**하고 **단언은 한 글자도 바꾸지 않았다**. *"유지"는 **설계 동결**이지 **결함·거짓 보존**이 아니다.* 마찬가지로 **T-P4b-1의 `oneshot`은 r5에서 sound 판정을 받았으나 컴파일되지 않는다** — 판정은 **안무**에 대한 것이었지 **채널 타입**에 대한 것이 아니었다. 고친다 |
| **승인자** | 인간 (2026-07-13) |
| **다음** | r7 plan review. **머신 소유 RED/GREEN 검증은 재실행하지 않는다**(r6 `next_steps` 지시 그대로: *"Revise only T-C2's choreography as recommended, then repeat the scoped review without changing the production hook set or fix model."* — ⚠ **인간이 그 범위를 클래스 전수로 확장했다**. 확장분은 **전부 테스트 안무이며 프로덕션 훅·fix model을 건드리지 않는다** → Codex의 제약 조건과 **충돌하지 않는다**) |

### Codex Plan Review — r7 (2026-07-13) — `needs-attention`, 2 findings (**P-7 해소 + 테스트 안무 2건**)

아티팩트: `docs/reviews/reconcile-gc-dedup-race/plan-r7.json` (reviewedSha `7fd55ed`, reviewedTree `8f6792e`).
Codex 요약: *"Do not ship: **P-7 itself is resolved and the committed RED witness remains sound**, but the class
sweep still contains **a no-op recovery test** and **unobserved post-park failures**. **Simpler alternative: none
for P-7 beyond the implemented bounded await.** Open question: none."*
`next_steps`: *"**Revise only these test choreographies; keep the seven-hook production model unchanged.**"*
**인간 triage 2026-07-13 — 2건 Accept.**

> **r7이 sound로 확인해 준 것(그대로 유지 — 이 개정에서 건드리지 않는다)**: **P-7 해소**(T-C2·T-B5①의 취소 완료
> await) · **커밋된 RED 증인**(`red.sha 6545808`) · **fix model 전체**(r4에서 sound) · **`Hooks` 7필드** ·
> `settle()` · 코호트 · `landed` · 무덤 이름공간 · 복구 경로 · **"P-7에 대해 더 단순한 대안은 없다"**.
> Codex가 **네 라운드 연속으로** 못박았다: *"no production hook or fix-model change."*
> ⇒ **이번에도 고칠 것은 테스트가 「무엇을 관측하는가」뿐이다.**

| # | Finding (요지) | Severity | Decision | Reason | Action (이 개정에서 실제로 한 것) |
|---|---|---|---|---|---|
| **P-8** (신규) | **T-B5④가 async grave 퓨처를 폴링도 않고 버린다.** `PassGuard::grave`는 **async**인데 `let _ = pass.grave(..)`는 **폴링되지 않은 퓨처를 드롭**한다 ⇒ **blob→무덤 rename이 아예 일어나지 않는다.** `drop(pass)` 후 다음 패스는 **원래의 멀쩡한 blob**을 발견하고, **`recover_graves`가 깨져 있어도** 테스트가 **통과한다.** 의도한 fail-closed 증인이 **아무것도 증명하지 못한다** | **critical** (0.99) | **Accept** | **반박 불가.** 그리고 이것은 **r6 전수 점검의 사각지대**였다 — r6은 **같은 줄**을 보고도 *"`drop(pass)` 누락"*(함정 4)만 잡았다. *"`Graved`를 흘린다"*를 **의도**로 읽는 순간 **`Graved`가 애초에 만들어지지도 않았다는 사실**이 시야에서 사라졌다. **「개시 ≠ 완료」의 새 변종**: **호출 ≠ 폴링** | **① T-B5④ 안무 확정**: `pass.grave(&x_sha).await.expect(…)` → **복구 이전 디스크 단언**(무덤 **정확히 1개** ∧ 정본 blob **부재** ∧ `graved == vec![X_sha]`) → **`drop(graved)`**(= `settle()` **미호출** = 누수) → **무덤·blob 재확인**(파괴적 Drop 부재) → **`drop(pass)`**(명시 · 타입이 순서를 강제) → **복구 패스**(`now`를 만료 이전으로 → **복원 그 자체를 직접 관측**) → blob 존재 ∧ 무덤 0. **뮤턴트 `recover_graves` 삭제가 이제 죽는다**(P-8 이전에는 **GREEN**이었다) + **파괴적 Drop 뮤턴트** · **rename 없는 `Graved` 뮤턴트**도 함께 죽는다. **② 함정 클래스 10번 신설**(*async 퓨처 폴링*) → **전수 재점검**: `docs/` 스니펫 · `src/` · `tests/` 전부 훑었고 — **`let _ = <async>`로 퓨처를 흘리는 곳은 T-B5④ 하나뿐이었다**(`src/`·`adversarial.rs`의 `let _ = …`는 **전부 `.await`가 붙어 있다** = 폴링됨 · **결과만** 버린다 = **함정 5**의 기존 계약) |
| **P-9** (신규) | **의도적으로 park된 put 핸들이 teardown 실패를 삼킨다.** 전수 점검이 T-P4a·T-P4b-1·T-P4b-2를 *"park 이후 실행되는 코드가 없다"*며 **면제**했으나, 계획 자신이 *"sender를 드롭하면 `recv()`가 풀려 커밋 클로저가 완주한다"*고 적어 놓았다 ⇒ **teardown 중에 rename·fsync·guard-drop·태스크 실패가 실제로 일어나는데** 보유한 put 핸들을 **await하지 않으므로** 패닉·에러가 있어도 **테스트는 초록**이다 — **이 전수 점검이 없애겠다던 바로 그 함정** | **high** (0.97) | **Accept** | **반박 불가 — 그리고 가장 뼈아프다.** 전수 점검이 **면제 사유를 발명해 스스로를 통과시켰다.** *"코드가 없다"*는 **논증**이고, **규칙 0이 금지하는 바로 그것**이다. §park 함정이 이미 *"tx drop → 훅 반환 → **클로저 완주**"*라고 적어 두었으므로 **계획은 자기 문서와 모순돼 있었다.** `commit_pointer`가 `spawn_blocking(…).await.expect("join")`으로 끝난다는 사실이 이것을 **critical에 가깝게** 만든다 — teardown 패닉이 **put 태스크의 패닉**이 되고, **버려진 핸들이 삼킨다** | **① 세 테스트 모두 teardown 시퀀스 확정**: 핸들 **보유**(`let put = tokio::spawn(…)`) → **영구 stall 증인 단언 전부 완료** → **명시적 `drop(tx)`**(T-P4a) / **`drop(tx_put)`**(T-P4b-1) / **`drop(tx_B)`**(T-P4b-2 — `tx_A`는 6단계에서 이미 해제) → **`timeout(5s, put)`** → **`JoinError` 언랩 + 안쪽 `put()` 결과 언랩**(= **`Ok`**). **순서 엄수**를 명시(먼저 해제하면 핀이 drop돼 **시나리오 자체가 사라진다**). **② 함정 클래스 9번 신설**(*teardown*) + **면제 사유 무효화**(§규율 7·8 개정, 봉인 체크리스트 ⑨ 개정) → **전수 재점검**: teardown에 재개될 것이 남는 테스트는 **이 셋 + T-C2**뿐이고, **T-C2는 핸들이 구조적으로 없다**(abort = detach = **명제 그 자체**) → **대리 관측**(Restore + 포인터 실재)으로 rename·`landed`까지 봉인하고 **fsync 이후 패닉만 미관측 잔여로 기록**했다(**⚠ 새 훅을 만들지 않는다** — 설계 동결). 나머지 전 테스트는 **park을 본문에서 해제하고 핸들을 본문에서 완주 await**하거나 **park·spawn이 아예 없다** ⇒ **teardown 잔여 0**. **`src/`·`tests/` 무변경 · 프로덕션 훅 0개 추가**(Codex 처방 그대로) |

### 인간 결정 — **상시 승인(standing approval)** (2026-07-13)

| 항목 | 내용 |
|---|---|
| **결정** | ***"findings가 테스트 안무에 머무는 동안 멈추지 말고 권장안대로 진행하라 — 설계 모델을 건드리거나 실질 트레이드오프가 생기면 에스컬레이션."*** ⇒ r7의 P-8·P-9는 **둘 다 테스트 안무**이므로 **라운드별 인간 승인을 기다리지 않고 즉시 적용**했다 |
| **적용 범위 (경계 — 엄수)** | **자동 진행 가능**: 배리어 테스트의 **안무·단언·랑데부·teardown**, 함정 표·매트릭스·체크리스트, Review Decision Log. **⚠ 즉시 에스컬레이션**: **`Hooks` 필드 추가/변경**(7개 고정) · `settle()`·코호트·`landed`·무덤 이름공간·복구 경로의 **의미 변경** · **관측 행동 플립 수 변화** · `bugfix-lock.json` `scope[]` 변경 · **실질 트레이드오프**(성능·가용성·복잡도 교환) · *"부분 해결"* 선언의 약화 |
| **이 라운드의 판정** | P-8·P-9 **전부 테스트 안무** — **프로덕션 훅 0개 추가 · fix model 무변경 · `src/`·`tests/` 코드 0줄** ⇒ **경계 안**. Codex의 `next_steps`(*"Revise only these test choreographies; keep the seven-hook production model unchanged"*)와 **정확히 일치**한다 |
| **⚠ 범위 판정 (명시)** | 인간 지시가 **두 함정 클래스의 전수 재점검**을 요구했다(*"Codex가 지적한 곳만 고치는 것은 이 병에 대해 증명된 실패 전략이다"* — r6의 판단을 **유지**). 그 결과 **T-C2의 teardown 잔여 1건**을 새로 찾았고 — **고칠 수 없음(구조적)** 을 확인하고 **기록**했다. *"확인 안 함"이 아니라 **"확인했고, 알고 남긴다"***. 그리고 **r6이 "무해"로 면제한 3건이 실제로는 결함**이었음이 드러났다 ⇒ **면제 사유는 근거가 아니다. 신호가 근거다** |
| **승인자** | 인간 (2026-07-13) |
| **다음** | r8 plan review(범위: **T-B5④ · T-P4a · T-P4b-1 · T-P4b-2의 안무 + 두 클래스 전수 재점검**). **머신 소유 RED/GREEN 검증은 재실행하지 않는다**(r7 `next_steps` 지시 그대로: *"Repeat the scoped plan review without re-running RED or GREEN tests."*) |

### 인간 결정 (2026-07-12) — D-1 ~ D-3 유지

| # | 결정 | 근거 |
|---|---|---|
| D-1 | **경로 기반 `run_once(&Path,…)` 삭제** (shim 유지 아님) | shim은 자기만의 빈 핀 등록부를 든 **두 번째 GC 소유자** = 지금 고치는 버그를 공개 API로 재생산 |
| D-2 | **복원 분기에서 tombstone 유지**(`pending.remove()` 안 함) | 그 put이 결국 실패해 진짜 가비지였다면, tombstone을 지운 탓에 가비지가 **새 grace를 선물받는다**. 복원은 "판단 보류"지 "참조됨 확정"이 아니다 |
| D-3 | **`Store::new`는 `pub` 유지** — 전제를 **doc + 테스트**로 못박음 | `pub(crate)` 축소는 crate 외부인 `tests/*.rs` 다수를 고쳐야 해 anti-cheat 게이트와 충돌할 위험. ⚠ **테스트 배선 함정**은 §호출부 전수 참조 |

### 컨덕터 판정 (2026-07-12)

| # | 결정 | 근거 |
|---|---|---|
| **D-4** | **최종안의 "B-3 = 격리 분기(F4) 봉인"을 제외한다.** B-3은 **위생·관측성·문서만**. 격리 분기는 **현행 그대로 보존**(`rename(blob → .corrupt)` 직접). F4는 **F-25로 분리**(청사진·T-Q1 포함) | **두 번째 관측 행동 플립**이다("치유된 blob이 격리되어 404가 된다" → "안 된다"). gated-bugfix **하드룰 10**: *"두 번째 관측 행동 플립은 근본 원인을 공유하거나 first-increment diff 안에 들어오더라도 **항상 별도 파이프라인**."* 최종안 스스로도 "게이트가 one-flip 엄격 해석을 요구하면 별도 bugfix로 파일링 가능"이라고 인정했다. **대가**: 이 픽스는 증상 클래스에 대해 **부분 해결**이며, 최종안 §2의 "GC의 유일한 파괴 연산" 주장은 **거짓으로 남는다** → §Preserved Contract와 §남은 위험 1에 **미해결 유실 경로로 굵게 명시**했다. 릴리스 게이트가 이 사실이 숨겨졌다고 판단하면 **Blocker**다 |

### Codex Plan Review — r8: clean — verdict approve, 0 findings, reviewedSha `a22771b` (docs/reviews/reconcile-gc-dedup-race/plan-r8.json). *"Ship the Round 8 plan. P-8 now proves the grave and restoration; P-9 observes all three parked put tasks through teardown after the stall assertions. The committed RED record matches red.sha and pins the stated data-loss symptom; no tests were re-run. No new critical issue found. Simpler alternative: none. Open question: none."*

---

## Structure Gate (B-1 — walking skeleton)

### Codex Structure Review — r1 (2026-07-13) — `needs-attention`, 1 finding

아티팩트: `docs/reviews/reconcile-gc-dedup-race/structure-r1.json` (reviewedSha `15f23c6`).
Codex 요약: *"Do not ship B-1: **caller cancellation lets the new uncancellable commit escape same-key
serialization**, so an expired upload can overwrite or resurrect state after a later successful operation."*
**인간 triage 2026-07-13 — Accept.**

| # | Finding (요지) | Severity | Decision | Reason | Action (B-1에서 실제로 한 것) |
|---|---|---|---|---|---|
| **S-1** | **무취소 커밋이 키별 락보다 오래 산다.** `KeyGuard`가 **호출자 퓨처**에 남아 있으므로 `upload_timeout`·disconnect가 그것을 **풀어버린다** → 같은 `bucket/key`의 재시도·delete가 락을 얻어 **먼저 끝나고**, 뒤늦게 깨어난 `spawn_blocking` rename이 **더 새로운 포인터를 덮어쓰거나 삭제된 키를 되살린다**(조용한 무결성 손상) | **high** (0.99) | **Accept** | 반박 불가. 픽스가 **핀**을 무취소 클로저로 옮기면서 **키 락은 옮기지 않았다** — 같은 논증(*"시작된 blocking 태스크는 abort 불가"*)이 **두 가드 모두에** 적용되는데 하나만 봤다 | **키 락을 커밋 클로저로 이전**(P3′). `PinGuard::commit_pointer(self, key: KeyGuard, …)`가 **두 가드를 함께 소유**하고, 클로저 안에서 **획득 역순(LIFO)으로 드롭**한다: ① 핀 ② 키 락 → 키 락이 풀리는 순간 그 put은 **이미 terminal**이다. 증인: **T-S1**(`commit_holds_key_lock_until_rename_lands`) |

### 인간 결정 — S-2 (2026-07-13): **재시작-필요 복구 계약을 교환으로 수용하고 명문화**

S-1의 봉인이 **새 degraded-path 행동**을 낳는다: 파일시스템 연산이 반환하지 않으면 그 `bucket/key`는
**syscall이 반환하거나 프로세스가 재시작될 때까지 쓰기 불가**가 된다(무취소 클로저가 키 락을 쥔 채이므로).

| 항목 | 내용 |
|---|---|
| **결정** | **교환을 수용한다 — 가드를 타임아웃으로 놓는 것은 금지**(S-1이 되살아난다) |
| **근거** | **잠김(가용성) < 되살아나기(무결성)**. 멈춘 fs는 병리적 상황이고 그때 이 스토어는 이미 사실상 죽어 있다(`reconcile`도 같은 fs를 읽는다). 홈랩 **단일 replica + RWO PVC**라 blast radius가 **그 키 하나**다 |
| **대가의 지불** | **침묵하지 않는다** — `KeyLocks::lock`이 `LOCK_WARN_AFTER`를 넘기면 `tracing::error!(bucket, key, …)`. **행동은 불변**(계속 기다린다 · 에러 반환 0 · 상계 0) |
| **문서화** | §Preserved Contract의 **「⚠ 재시작-필요 복구 계약 (S-2)」** — **릴리스 게이트 제출물**. 잠김 **없이** 되살아나기를 막는 설계(키-바인드 펜싱 / 버전화된 포인터 발행)는 **F-30**으로 분리 |
| **증인** | **T-S2**(`wedged_commit_keeps_key_unwritable_and_says_so_loudly`) — 쓰기 불가 · **경고 발화** · **행동 불변** · **delete가 이긴다**(부활 0)를 **한 테스트가 전부** 못박는다 |

### Codex Structure Review — r2 (2026-07-13) — `needs-attention`, 1 finding (**신규 — 테스트 seam**)

아티팩트: `docs/reviews/reconcile-gc-dedup-race/structure-r2.json` (reviewedSha `97b0ff1`, reviewedTree `9717d4c`).
Codex 요약: *"Do not ship B-1 yet. **S-1 and S-2 are structurally resolved**, but **the committed seam cannot host
B-2's required deterministic witnesses** without an unplanned visibility change or weaker tests."*
`next_steps`: *"Add the test-only cross-module bridge to B-1. … **Keep `run_once_at`, protection state, and all
seven hook fields private in production.**"*
**인간 triage 2026-07-13 — Accept.**

> **r2가 sound로 확인해 준 것(그대로 유지 — 건드리지 않는다)**: **S-1 해소**(키 락의 커밋 클로저 이전 · LIFO 드롭) ·
> **S-2 해소**(재시작-필요 복구 계약의 명문화 + T-S2) · **fix model 전체** · **`Hooks` 7필드** · `settle()` ·
> 코호트 · `landed` · 무덤 이름공간 · 복구 경로. ⇒ **`src/`의 프로덕션 행동은 한 글자도 바꾸지 않는다.**
> **고칠 것은 테스트 seam 하나뿐이다.**

| # | Finding (요지) | Severity | Decision | Reason | Action (이 개정에서 실제로 한 것) |
|---|---|---|---|---|---|
| **S-3** (신규) | **B-2의 결정적 테스트 seam이 형제 private 모듈로 쪼개져 있다.** `run_once_at`은 **`reconcile.rs` private**이고, B-2가 규정한 배리어 안무를 구성하는 데 필요한 **훅 7필드는 형제 모듈 `pins.rs` private**이다. `pins.rs`의 테스트는 `Hooks`를 지을 수 있지만 **주입형 시각의 reconciler를 부를 수 없고**, `reconcile.rs`의 테스트는 **그 반대**다. **B-2는 같은 결정적 증인 안에서 두 기능을 모두 요구한다**(§6: `run_once_at` + `Hooks{pre_grave, post_grave, …}`) → **프로덕션 가시성을 넓히거나 · 계획에 없던 다리를 놓거나 · 기록된 주입형-시각 안무를 약화시키지 않고는** B-1 위에 얹을 수 없다. **seam이 아직 싸게 바뀔 수 있는 지금 고쳐야 한다** | **high** (0.99) | **Accept** | **반박 불가.** 그리고 **계획서 자신이 그 다리를 이미 전제하고 있었다** — B-2 §6의 T-C1은 *"`run_once_at` → `gc_deleted == 1`"*이라 적고, 라이브니스 sanity는 *"**모든** `run_once_at` 호출을 `timeout`으로 감싼다"*고 규정한다. **호출부는 `pins.rs`인데 그 함수는 `reconcile.rs` private이다** — 계획은 **존재하지 않는 seam 위에 안무를 그려 놓았다.** B-1에서 잡지 않았다면 B-2 구현자가 **가시성을 넓히거나(봉인 파괴) 안무를 약화**시키는 것으로 풀었을 것이다 | **Codex 권고 그대로 — 다른 것은 아무것도 바꾸지 않았다.** ① `reconcile.rs`에 **`#[cfg(test)] pub(super) async fn run_once_at_for_test(store, now, gc_grace, settle_timeout)`** 추가 — `run_once_at`에 **위임만** 한다. `pub(super)` = **`store` 모듈 스코프** → `store::pins::tests`에서 보인다. ② **프로덕션 표면 무변화 증명**: `run_once_at`은 여전히 **모듈 private `async fn`**(`pub` 아님) · **`Hooks` 7필드 · `landed`/`live` 보호 상태는 `pins.rs` private 그대로** · 다리는 `#[cfg(test)]`라 **릴리스 빌드에 부재**(`cargo build --release` 통과 · 경고 0) · `reconcile.rs` diff = **+22 −0**(GC 삭제/격리 분기 **바이트 동일**). ③ **T-C1을 다리로 전환** — `run_once`(벽시계) + `gc_grace = 0` 우회를 **`run_once_at_for_test(&s, T0, GRACE=3600s, …)`**로 바꿔 tombstone 기준 시각과 reconcile의 `now`를 **같은 `T0`**로 묶었다(**결정성만 강화 · 단언 무변경**). ④ **다리 스모크 증인 신설** — `barrier_hooks_and_injected_clock_compose_in_one_witness`: **한 테스트가** `Hooks{during_collect}`를 짓고 **주입형 시각으로 두 패스**를 돌려(`T0` → 최초 관측 / `T0+GRACE+1s` → 회수, **sleep 0**) B-2의 안무가 **실제로 구성 가능함**을 기계로 증명한다. ⑤ **B-2 증분 문서에 §4.1 신설** — 각 증인이 **어느 모듈에 살고 어떤 다리를 쓰는지** 표로 못박았다(전 배리어 증인 → `pins.rs` + 다리 · `recover_graves` 가드/훅 불필요 테스트 → `reconcile.rs` + 직접 호출). **회귀 테스트는 여전히 RED · characterization 118 green · `allow(dead_code)` 5개 유지 · `ReconcileStats` 필드 추가 0 · 프로덕션 훅 0개 추가.** |

**⚠ 이 개정이 하지 *않은* 것(경계)**: 프로덕션 가시성 확대 **0** · `Hooks` 필드 변경 **0** · `settle()`·코호트·
`landed`·무덤 이름공간·복구 경로의 의미 변경 **0** · 관측 행동 플립 수 변화 **0** · `bugfix-lock.json` `scope[]`
변경 **0**. **상시 승인(standing approval)의 경계 안**이다 — S-3은 **테스트 seam**이지 fix model이 아니다.
