---
bugfix: reconcile-vanished-entry-aborts-pass
invariant-class: bugfix       # Rule 0: 관측 행동이 정확히 하나 뒤집힌다(사라진 항목이 패스를 중단시킴 → 건너뜀)
entry-track: bug
review-track: standard        # 단일 증분 국소 수정(새 seam 불필요). 배리어 1~4는 트랙 무관하게 적용된다
pipeline-stage: design
issue-tracker: local
worktree:
branch:
consent-scope:
symptom: "reconcile가 .objects 스냅샷을 뜬 뒤 항목별 stat/read를 하는 사이, 동시 put_stream이 .tmp-<uniq>를 최종 blob 이름으로 rename하면, 사라진 경로에 대한 stat/read가 ENOENT를 하드 io::Error로 전파해 **패스 전체가 Err로 중단**된다(그 항목만 건너뛰는 게 아니라). 쓰기 트래픽이 있는 동안 reconcile이 사실상 완주하지 못해 GC·temp 정리·격리가 안 돌고 디스크가 찬다."
red-baseline: ac58bd7982d06e46f37cd4aa6a9c274d93bd8195
bugfix-lock: red
spike-1:
---

## Track note

**출처**: gated-refactor `arch-deepening-2026-07`이 R-4(reconcile 이관) 중 코드를 읽다
발견해 Follow-up backlog **F-14**로 파일링한 항목. 리팩터의 규율상 보존된 채 남았다.

**의심 경로**(현행 `src/store/reconcile.rs`에서 확인함 — 방금 머지된 gated-bugfix
`reconcile-gc-dedup-race` 이후에도 **그대로 살아 있다**):
- `.objects` 직속 항목을 `Vec<DirEntry>`로 **스냅샷**한 뒤 루프를 돈다(순회 중 변경 회피가 목적).
- **Temp 분기**: `let mtime = e.metadata().await?.modified()...` — 사라진 경로 → ENOENT → `?`.
- **Blob 분기**: `let content = tokio::fs::read(&p).await?;`(무결성 검증) — 사라진 경로 → ENOENT → `?`.
- 동시 `put_stream`(그리고 `write_atomic`의 치유 경로)은 `.objects/.tmp-<uniq>`를 만든 뒤
  `.objects/<sha>`로 **rename**한다 → 스냅샷에 잡힌 Temp 항목이 루프 도중 **사라진다**.

**왜 기존 테스트가 못 잡나**(확인함): `tests/adversarial.rs`의
`concurrent_nested_puts_with_reconcile_loop_preserve_all`이 reconcile 루프에서
**`let _ = reconcile::run_once(...).await;`**로 **결과를 버린다**. 패스가 Err로 중단돼도
테스트는 아무것도 관측하지 못한다.

**Rule 0 판정**: 뒤집히는 관측 행동은 **하나**다 — "스냅샷 이후 사라진 항목이 패스 전체를
`Err`로 중단시킨다" → "그 항목을 건너뛰고 패스가 계속된다".
보존해야 할 것: **B7의 나머지**(reconcile은 `io::Result`를 **무가공** 전파한다 — ENOENT
**이외의** io 에러는 여전히 그대로 올라가야 한다) · 2단계 tombstone GC 의미론 · 무덤/정산
(직전 픽스) · temp grace 보존 · 비트로트 격리 · 목록/서빙 계약 · `ReconcileStats` 필드 정의.
순-신규 행동 없음, 플립 다수 아님 → `bugfix`.

⚠ **미묘한 지점**: 수정은 "`?`를 없앤다"가 아니다. **`ErrorKind::NotFound`인 경우에만**
그 항목을 건너뛰고, 다른 모든 io 에러는 **여전히 무가공 전파**해야 한다. 그러지 않으면
진짜 I/O 장애(EACCES·EIO)를 조용히 삼켜 **두 번째 플립**이 된다.

**주의 — 재현이 아직 실증되지 않았다.** 코드 독해의 산물이므로 `diagnose` 단계가 먼저
red-capable repro를 만들어야 하고, 재현이 안 되면 여기서 멈춘다.

**기반**: main `d941a22` (reconcile-gc-dedup-race 머지 직후, origin에 push됨). 스위트 136 passed.
