# Verification — reconcile-gc-dedup-race

작성: 2026-07-13 · stage `verification` · 브랜치 `bugfix-reconcile-gc-dedup-race`

이 픽스의 불변식은 **정확히 하나의 관측 행동이 뒤집힌다**는 것이다. 따라서 증명해야 할
claim은 gated-bugfix 계약이 정한 것 — **RED→GREEN 플립**과 **주변 행동의 보존** — 뿐이며,
그 증거는 **상태 스크립트가 직접 재실행한 verify-record**다(컨덕터가 손으로 쓴 산문이 아니다).

| # | Claim | 증명 | 결과 |
|---|---|---|---|
| C1 | **플립**: 회귀 테스트가 `red.sha`에서 FAIL, `green.sha`에서 PASS | `bugfix-status.mjs --verify-flip` | ✅ `flipOk: true` |
| C2 | **red-for-the-right-reason**: RED 출력에 선언된 `symptomToken`이 실제로 나타난다 | 같은 스크립트 | ✅ `symptomTokenPresent: true` |
| C3 | **보존**: characterization이 **양쪽 sha에서** green | 같은 스크립트 | ✅ 양쪽 `exit 0` |
| C4 | **단일 플립 표면 바운드(B4)**: 변경된 모든 비-테스트 경로가 `scope[]` 안 | `bugfix-status.mjs` 배리어 | ✅ blockers 0 |
| C5 | **anti-cheat**: characterization 테스트 미약화·미삭제·미스킵 | `git diff red.sha..green.sha -- tests/` | ✅ 아래 |

---

## C1 · C2 · C3 — 스크립트가 재실행해 증명한 RED→GREEN 플립

> **release gate R-1·R-3 수용(2026-07-13)**: 초판은 명령 출력을 4줄로 요약했고,
> 스크립트 hint를 그대로 옮겨 *"repro gone"*이라고 적었다. **둘 다 정정한다** —
> 아래는 **명령 원문**이고, **`reproCmd`는 선언되지 않았으므로**(락의 optional 필드)
> 두 verify-record의 `repro`는 **`null`**이다. **"원본 repro가 사라졌다"고 주장하지 않는다.**
> 최소화된 회귀 증인이 곧 이 픽스의 유일한 플립 증인이다.

**락**(`bugfix-lock.json`):
- `regressionCmd`: `cargo test --test regression_reconcile_gc_dedup_race`
- `characterizationCmd`: `cargo test --lib --bins --test adversarial --test contract --test e2e --test layout_tree --test openapi`
- `flips[]`: `dedup_put_during_reconcile_window_must_not_lose_blob` / symptomToken **`DATA LOSS`**
- `scope[]`: `src/store/**`, `src/main.rs`, `src/layout.rs`, `CONTEXT.md`, `docs/adr/**`
- `reproCmd`: **미선언** → 두 레코드의 `repro`는 `null`
- `red.sha` = `6545808` · `green.sha` = `de8c40f`

### verify-record (스크립트가 직접 재실행해 쓴 것)

**RED** (`bugfix-verify-red-*.json`, treeSha = red.sha의 트리):
```
regression       : exit 101 · failed: true · symptomTokenPresent: true
characterization : exit 0   · green:  true
repro            : null   (reproCmd 미선언)
```
그 `outputTail`은 이제 **자기 판정을 뒷받침한다**(R-1 수정 — tail 창이 symptomToken에
앵커링된다). 발췌:
```
---- dedup_put_during_reconcile_window_must_not_lose_blob stdout ----
assertion `left == right` failed: DATA LOSS: put()이 OK를 반환했는데 reconcile GC가
그 블롭을 삭제 — 커밋 포인터는 남고 블롭 부재 → GET 404 / list 제외 (영구 non-servable).
유실 9/12 (라운드별=[3, 3, 3]), stats=Some(ReconcileStats { referenced: 0, gc_deleted: 4, ... })
  left: 9   right: 0
test result: FAILED. 0 passed; 1 failed
```

**GREEN** (`bugfix-verify-green-*.json`, treeSha = green.sha의 트리):
```
regression       : exit 0 · passed: true
characterization : exit 0 · green:  true
repro            : null
```

**`--verify-flip` 판정**: `ok: true`, **`flipOk: true`**.

스크립트는 두 sha를 각각 **throwaway 워크트리에 체크아웃해 명령을 직접 실행**하고 자신이
관측한 exit code로 레코드를 쓴다. 레코드는 커밋의 **트리 sha로 키잉**되어 재사용·위조가
불가능하다(배리어 B2의 ancestry + HEAD-reachability 검사도 통과).

### green.sha에서의 명령 원문 (컨덕터 재실행)

```
$ cargo test --test regression_reconcile_gc_dedup_race
running 1 test
test dedup_put_during_reconcile_window_must_not_lose_blob ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.47s
EXIT: 0
```

```
$ cargo test --lib --bins --test adversarial --test contract --test e2e --test layout_tree --test openapi
test result: ok. 116 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.63s   (lib)
test result: ok.   0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   (bins)
test result: ok.   8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.37s   (adversarial)
test result: ok.   1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s   (contract)
test result: ok.   2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.45s   (e2e)
test result: ok.   3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s   (layout_tree)
test result: ok.   5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s   (openapi)
EXIT: 0
```
합계 **116 + 0 + 8 + 1 + 2 + 3 + 5 = 135 passed / 0 failed**.

### 확률적 창 — 20회 반복 (무삭제 원문, 별도 아티팩트)

> **release gate r2의 R-3 재지적 수용**: 초판의 20회 블록은 `run N:` 접두사를 손으로 붙이고
> `...`로 줄인 **재구성물**이었다 — 원문이 아니었다. 아래는 **파이프라인이 캡처한 무삭제 원문**을
> 별도 아티팩트로 커밋한 것이다.

이 증인은 동시성 레이스를 재현하므로 1회 GREEN으로는 부족하다. 락의 `regressionCmd`를
green.sha에서 **20회 반복**하고, 매 실행의 **완전한 stdout+stderr와 exit status**를 그대로
커밋했다:

**`docs/reviews/reconcile-gc-dedup-race/evidence-regression-20x.txt`** (250줄)

그 아티팩트의 자체 집계:
```
exit 0: 20
non-zero exits: 0
verdict: 20/20 GREEN
```
각 RUN 블록은 명령 · 완전한 출력 · `EXIT: <code>`를 담는다. 발췌(RUN 1):
```
===== RUN 1/20 =====
$ cargo test --test regression_reconcile_gc_dedup_race
running 1 test
test dedup_put_during_reconcile_window_must_not_lose_blob ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.4xs
EXIT: 0
```
수정 전(red.sha)에는 같은 반복이 **20/20 RED**였고 라운드당 **12개 중 9개**를 잃었다
(RED verify-record의 `outputTail`에 `유실 9/12 (라운드별=[3, 3, 3])`로 남아 있다).

---

## C4 — 단일 플립 표면 바운드 (B4)

`bugfix-status.mjs`의 배리어가 `git diff red.sha..HEAD`의 모든 **비-테스트** 경로가
선언된 `scope[]` 글롭 안에 있음을 확인한다 → **blockers 0**.

변경된 프로덕션 파일: `src/store/{pins,atomic,locks,mod,objects,reconcile}.rs` ·
`src/layout.rs` · `src/main.rs` — 전부 scope 내.
**`src/http/**`·`src/config.rs`·`src/capacity.rs`는 무변경**(scope 밖 표면을 건드리면
두 번째 특성화되지 않은 플립이 된다).

**`ReconcileStats` 필드 0개 추가** — 필드를 늘리면 `tests/layout_tree.rs`의 전수 구조체
`assert_eq!` 3곳이 깨진다(= 두 번째 플립). 정의는 바이트 동일.

**격리(bit-rot) 분기 프로덕션 코드 0줄 변경** — D-4/하드룰 10에 따라 F-25로 분리.

---

## C5 — anti-cheat: characterization 미약화

```
$ git diff 6545808..de8c40f -- tests/
```
회귀 테스트 파일(`tests/regression_reconcile_gc_dedup_race.rs`)의 **기계적 시그니처
치환 3곳뿐**(`run_once(&root, g)` → `run_once(&s2, g, settle)` + `Store::clone()` 캡처(D-3)
+ 상수 1개). **단언은 한 줄도 바뀌지 않았다** — `DATA LOSS` 토큰을 담은 4중 판정
(커밋 포인터 존재 ∧ 블롭 부재 ∧ `get_bytes` 404 ∧ list 제외)이 그대로다.

`tests/layout_tree.rs`·`tests/adversarial.rs`도 같은 기계적 치환뿐이며, **골든 트리의
정렬 스냅샷**·**전수 `ReconcileStats` `assert_eq!` 3곳**·**mid-flight `.tmp-*` 정확히 1개**
단언은 전부 **원문 그대로** 통과한다.

**스킵된 테스트 0**(`#[ignore]` 0건). 테스트 수는 **증가만** 했다:
105(baseline) → **135**. 추가된 것은 전부 이 픽스의 성질을 **더 강하게** 고정하는
증인이며, **전부 뮤턴트 킬로 검증**됐다(주장이 아니라 실측 RED).

---

## 이 픽스가 실제로 무엇을 고쳤는가

`reconcile`이 패스 시작에 뜬 참조 스냅샷으로 GC를 판정하는데, `Store::put`의 dedup 분기가
**바이트를 재기록하지 않고** 커밋 포인터만 기록하므로, 스냅샷 이후 참조를 얻은 블롭이 같은
패스에서 삭제되어 **커밋 포인터만 남고 유일한 사본이 사라졌다**(영구 non-servable).

수정: GC의 삭제를 **가역**으로 바꾼다 — 블롭 이름을 무덤(`.gc-grave-<sha>`)으로 먼저
치우고, 그제서야 **착지(landed)** 흔적을 보고, 착지한 커밋이 있으면 복원한다. 커밋 rename과
핀의 수명은 **하나의 무취소 `spawn_blocking` 클로저**에 갇혀 있어 호출자 취소가
in-flight rename에서 핀을 떼어낼 수 없다.

**게이트가 이 과정에서 잡아낸 것들**(전부 실증됨):
- **plan gate 8라운드** — grave를 `.tmp-`로 두면 **다음 패스가 Temp로 분류해 즉시 삭제**해
  원래의 영구 데이터 손실을 그대로 재현(P-1) · 실패·취소된 put까지 보호하면 **두 번째
  관측 플립**(P-2) · 사전확인 뮤턴트가 **컴파일된다**(P-3) · 코호트 대기가 **무한정**이면
  커밋된 객체가 좌초되고 GC가 영구 정지(P-4) · **"증인이라고 주장했지만 아무것도 증명하지
  못하는 테스트" 5건**(P-5~P-9).
- **structure gate 3라운드** — 무취소 커밋이 **키 락을 벗어나 삭제된 키를 되살린다**(S-1,
  실증됨) · 그 수리가 낳은 가용성 교환(S-2 → 인간이 **재시작-필요 계약**을 명시 수용) ·
  B-2의 배리어 증인이 애초에 **구성 불가능**했다(S-3).
- **컨덕터 2축 리뷰** — B7 계약(**`io::Error`를 하나도 삼키지 않는다**)이 reconcile
  레벨에서 **무방비**였다: `.settle().await?`를 `Err(_) => continue`로 바꾸는 뮤턴트가
  **전 스위트를 통과**했다(`113 passed; 1 failed`로 실증).

## 정직한 경계 — 이 픽스는 **부분 해결**이다

**격리(quarantine) 분기의 동일 유실 경로는 미해결로 남는다**(F-25). 손상 blob을 동시 put이
치유한 **직후** 패스가 그 **치유된 inode**를 `.corrupt`로 옮기면 **같은 증상**(포인터만 남고
blob 부재 → 영구 404)이 재현된다. 이를 고치면 **두 번째 관측 행동 플립**(격리됐어야 할
blob이 격리되지 않는다)이므로 **하드룰 10**에 따라 별도 파이프라인으로 분리했다.
**"증상 클래스 해결"이라고 주장하지 않는다.**

**새 degraded-path 행동**(S-2, 인간이 명시 수용): 파일시스템 연산이 반환하지 않으면 그
`bucket/key`는 syscall이 반환하거나 프로세스가 재시작될 때까지 **쓰기 불가**가 된다. 가드를
타임아웃으로 놓으면 detach된 낡은 커밋이 **성공적으로 삭제된 키를 되살린다**(실증됨) —
**잠김(가용성) < 되살아나기(무결성)**. 관측성(`tracing::error!`)과 증인(T-S2)으로 못박았고,
잠김 없이 되살아나기를 막는 설계는 **F-30**으로 파일링했다.

---

## 판정

**5개 claim 전부 통과.** 플립은 **스크립트의 재실행**으로 증명됐고, 주변 행동은 보존됐으며,
characterization은 baseline 이후 **단언 한 줄도 약화되지 않았다**. release gate로 진행 가능.
