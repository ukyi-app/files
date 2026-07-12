---
bugfix: reconcile-gc-dedup-race
invariant-class: bugfix       # Rule 0: 관측 행동이 정확히 하나 뒤집힌다(살아있는 블롭이 GC됨 → 안 됨)
entry-track: bug
review-track: full            # 데이터 손실 + 동시성 + 스토리지 표면 → full(세 게이트 전부)
pipeline-stage: red-capture
issue-tracker: local
worktree:
branch:
consent-scope:
symptom: "reconcile가 참조 스냅샷을 뜬 뒤 동시 put이 dedup 경로로 그 블롭을 커밋하면, GC가 살아있는 블롭을 삭제한다 — 커밋 포인터는 남고 블롭만 사라져 객체가 영구 non-servable이 된다(GET 404 / list 제외). 데이터 손실."
red-baseline: 65458082b6692acd0345763da96ef9a811ae745e
bugfix-lock: red
spike-1:
---

## Track note

**출처**: 방금 완주한 gated-refactor `arch-deepening-2026-07`이 코드를 읽다 발견해
Follow-up backlog **F-1**로 파일링한 항목이다(`docs/refactors/arch-deepening-2026-07.md`).
리팩터의 규율상 "발견한 잠재 버그는 고치지 않고 파일링한다"에 따라 보존된 채 남았다.

**의심 경로**(리팩터가 지목한 것 — 아직 **재현 실증 전**):
- `src/store/reconcile.rs` — `collect_referenced`가 참조 sha 집합의 **스냅샷**을 뜬다.
  이후 `.objects` 항목을 돌며 미참조 블롭에 tombstone GC를 적용한다(2단계: 최초 관측 →
  grace 경과 후 삭제).
- `src/store/objects.rs` — `put`의 **dedup 경로**: 같은 sha의 블롭이 이미 있으면 바이트를
  다시 쓰지 않고 커밋 포인터만 원자적으로 생성한다.
- 레이스 가설: 스냅샷 시점에 미참조였고 `.gc-pending`에 이미 grace를 넘긴 상태로 등록돼
  있던 블롭을, 그 직후 put이 dedup으로 참조하면 → reconcile은 낡은 스냅샷을 근거로 삭제를
  집행한다. 커밋 포인터만 남고 블롭이 사라진다.

**주의 — 재현이 실증되지 않았다.** 백로그 항목은 코드 독해의 산물이지 관측된 사고가
아니다. 따라서 `diagnose` 단계(diagnosing-bugs Phases 1–4)가 **먼저 red-capable repro를
만들어야** 하고, 재현이 안 되면 여기서 멈춘다(가짜 양성이면 파이프라인을 헛돈다).
grace 창(`gc_grace`)과 주입형 `now`(`run_once_at`)가 있으므로 결정적 재현이 가능할
것으로 보인다.

**Rule 0 판정**: 뒤집히는 관측 행동은 **하나**다 — "동시에 dedup 커밋된 블롭이 GC로
삭제된다" → "삭제되지 않는다". 나머지(2단계 tombstone 의미론, temp grace 보존, 격리,
목록·서빙 계약)는 전부 보존되어야 한다. 순-신규 행동 없음, 플립 다수 아님 → `bugfix`.
아키텍처적 근본 원인(올바른 seam 부재)으로 판명되면 Fork B → gated-refactor로 재라우팅한다.

**기반**: main `02f58d7` (arch-deepening-2026-07 머지 + ADR-0001 직후, origin에 push됨).
스위트 105 passed / 8 suites.

## ⛔ BLOCKED — plan gate r2 escalated (2026-07-12)

Codex plan gate가 **2라운드 상한을 소진**했고 여전히 `needs-attention`이다
(`docs/reviews/reconcile-gc-dedup-race/plan-r2.json`, reviewedSha `9f435b5`).

- **P-1 해소됨** — `.gc-grave-<sha>` 평면 이름공간 + 전이 fsync + 복구 경로.
- **P-2 잔존**(high) — `live` 항이 **결과를 모르는** in-flight put까지 보호하므로,
  그 put이 결국 실패하면(포인터 미생성) GC는 헛되이 회수를 연기한 것이 된다.
  매 패스에 겹치는 실패가 반복되면 **회수가 무한정 연기**될 수 있다 → 오늘과 다른
  용량/통계 행동 = **두 번째 관측 행동 플립**. T-C1은 이걸 못 잡는다(실패한 put이
  이미 반환되고 핀이 죽은 **뒤에** reconcile을 돌린다).
- **P-3 잔존**(high) — `RenameReceipt`가 **아무 것에도 묶이지 않은 unit 토큰**이고
  `rename_durable`이 `pub(crate)`라, `pins.rs`가 **무관한 rename으로 영수증을 발급**받아
  무덤 rename **이전에** 사전확인을 할 수 있다 → `atomic.rs`를 건드리지 않고도 뮤턴트가
  **컴파일된다**. T-B2도 그 창을 열지 않는다.

**하드룰 4에 의해 게이트는 BLOCKED다.** 인간이 (a) 잔여 위험 면제, (b) 수동 3라운드 승인,
(c) 흐름 중단 중 하나를 명시적으로 결정하기 전까지 executing 진입·dispatch·landing 금지.
침묵은 면제가 아니다.
