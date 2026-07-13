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

**락**(`bugfix-lock.json`):
- `regressionCmd`: `cargo test --test regression_reconcile_gc_dedup_race` (플립 테스트만)
- `characterizationCmd`: `cargo test --lib --bins --test adversarial --test contract --test e2e --test layout_tree --test openapi` (주변 스위트 — 회귀 제외)
- `flips[]`: `dedup_put_during_reconcile_window_must_not_lose_blob` / symptomToken **`DATA LOSS`**
- `scope[]`: `src/store/**`, `src/main.rs`, `src/layout.rs`
- `red.sha` = `6545808` · `green.sha` = `2dd7104`

**RED verify-record** (`bugfix-verify-red-cc8704f….json`, treeSha = red.sha의 트리):
```
regression       : exit 101 · failed: true · symptomTokenPresent: true   ("DATA LOSS")
characterization : exit 0   · green:  true
```

**GREEN verify-record** (`bugfix-verify-green-8a83430….json`, treeSha = green.sha의 트리):
```
regression       : exit 0 · passed: true
characterization : exit 0 · green:  true
```

**`--verify-flip` 판정**: `ok: true`, `flipOk: true` —
*"FLIP proven (FAIL@red → PASS@green, characterization green, repro gone)"*.

스크립트는 두 sha를 각각 **throwaway 워크트리에 체크아웃해 명령을 직접 실행**하고 자신이
관측한 exit code로 레코드를 쓴다. 레코드는 커밋의 **트리 sha로 키잉**되어 재사용·위조가
불가능하다(배리어 B2의 ancestry + HEAD-reachability 검사도 통과).

**회귀 테스트의 확률적 창에 대한 주의**: 이 증인은 동시성 레이스를 재현하므로 컨덕터가
**20회 반복 실행**해 **20/20 GREEN**을 별도 확인했다(수정 전에는 20/20 RED, 유실 9/12).

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
$ git diff 6545808..2dd7104 -- tests/
```
회귀 테스트 파일(`tests/regression_reconcile_gc_dedup_race.rs`)의 **기계적 시그니처
치환 3곳뿐**(`run_once(&root, g)` → `run_once(&s2, g, settle)` + `Store::clone()` 캡처(D-3)
+ 상수 1개). **단언은 한 줄도 바뀌지 않았다** — `DATA LOSS` 토큰을 담은 4중 판정
(커밋 포인터 존재 ∧ 블롭 부재 ∧ `get_bytes` 404 ∧ list 제외)이 그대로다.

`tests/layout_tree.rs`·`tests/adversarial.rs`도 같은 기계적 치환뿐이며, **골든 트리의
정렬 스냅샷**·**전수 `ReconcileStats` `assert_eq!` 3곳**·**mid-flight `.tmp-*` 정확히 1개**
단언은 전부 **원문 그대로** 통과한다.

**스킵된 테스트 0**(`#[ignore]` 0건). 테스트 수는 **증가만** 했다:
105(baseline) → 133 → **134**. 추가된 것은 전부 이 픽스의 성질을 **더 강하게** 고정하는
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
