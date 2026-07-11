---
id: R-4
title: reconcile의 레이아웃 재유도 소멸 — 워커 + 분류기 + Layout 경로 소비
status: done
blocked-by: [R-1]
plan: docs/refactors/arch-deepening-2026-07.md
created: 2026-07-10
closed: 2026-07-12
---

## What moves

- `collect_referenced`의 수동 2단 순회 → `layout.pointers_all()` 소비
  (`io::Result` `?` 무가공 전파 그대로 — B7; 내용 read/파싱의 조용한 스킵 잔존).
- `.objects` 스캔의 이름 판정(`.gc-pending.json`/`.corrupt`/`.tmp-`/64-hex) →
  `classify_objects_entry` match. **O1 순서 준수**: Reserved는 file_type 조회 전
  continue(예약 이름 무-stat 보존), O2: dir 스킵은 Temp/Blob 처리 앞.
- `.objects`·`.gc-pending.json`·`.corrupt` 경로 저작 → `Layout` 메서드 경유.
- `run_once(root, gc_grace)` pub 시그니처 불변 — 내부에서 `Layout::new`.
- grace/mtime 정책·격리 mechanics·tombstone 로직은 reconcile 소유 그대로.

## Acceptance

- [x] characterization suite green (`cargo test`) — 특히 심링크-유일-포인터
      referenced:2, mid-stream temp 보존, 골든 트리
- [x] `cargo clippy` green
- [x] no weakening of the characterization tests (anti-cheat)
- [x] reconcile.rs의 **비-테스트 코드**에서 레이아웃 리터럴(`.meta.json`·`.tmp-`·
      `.objects`·`.gc-pending.json`·`.corrupt`·64-hex 판정) 소멸.
      **인라인 `#[cfg(test)] mod tests`의 온디스크 리터럴은 raw 유지**(상수 경유 금지 —
      동어반복이 되어 회귀 감지력을 잃는다. plan gate P-4가 잡았던 실패 유형)

## Result

**커밋** `5e94efa` (증분 시작 fixed point `d2c6be5`). 74줄 삭제 / 61줄 추가.

**행위 보존 증거**: `cargo test` = **101 passed / 8 suites** — baseline 동일(컨덕터 직접
실행). clippy 0 errors, reconcile.rs 신규 경고 0. 인라인 `#[cfg(test)] mod tests`는
**바이트 동일**(diff hunk가 테스트 mod 시작 전에 끝남) — 온디스크 리터럴 14개 raw 유지.

**컨덕터측 2축 리뷰**(fixed point `d2c6be5`):
- **Spec 축 clean** — Blocker/Major/Minor 0. 이 증분의 위험은 전부 **syscall 순서**에
  있었고, 구파일(`git show d2c6be5:src/store/reconcile.rs`)과 대조해 전수 확인:
  **O1** — `classify_objects_entry`는 순수 `&str → enum`(await·I/O 없음)이므로
  Reserved continue가 여전히 첫 `file_type().await` **앞**에 있다 → 예약 이름 무-stat 보존.
  **O2** — dir 스킵이 `match class` 앞. **Temp > Blob 우선** 및 대문자 hex의 Blob 분류 →
  내용 검증 격리(B6, 정규화 없음) 보존. `.corrupt`가 파일이든 디렉터리든 구·신 모두
  `file_type` 전에 continue(구별 불가 — 동일). 격리 후 `continue`가 `match`가 아니라
  `for`에 바인딩되어 GC 블록을 건너뛰고 다음 항목으로 진행하는 것도 확인.
  워커 경로: 루트 직속 파일 비후보 · `.objects` 미진입 · 중첩 키 재귀(P2-1) ·
  `.tmp-` 접두 `*.meta.json` 배제 · **무가공 `io::Result` `?` 전파(B7 — AppError 매핑 없음)** ·
  read/파싱 실패 조용한 skip · 심링크 lstat 통과 전부 보존.
- **Standards 축**: hard violation 0, 스멜 0(`matches!` + `match` 이중 스위치는 O1이
  강제한 형태라 Repeated Switch가 아님. `Reserved | Other => {}`를 `_` 대신 명시한 것은
  C1 총체성 보존으로 평가). judgement call 1건 → **A-3**(아래).

**A-3 (Accept — R-6 이후 정리 커밋으로 유예)**: A-2가 `safe_object_path`·`meta_path`를
private이 아닌 `pub(crate)`로 둔 이유는 **"R-4의 crate 내부 사용 여지"**였는데, 정작
R-4가 **둘 다 쓰지 않았다**(유일 호출자는 여전히 `Layout::meta_for`). 유예 사유가
해소됐으므로 module-private으로 축소한다. 다만 R-5·R-6이 소비할 가능성을 완전히
배제한 뒤 닫기 위해 **전 증분 완료 후 별도 정리 커밋**으로 실행한다(verification 전).

**Latent bugs 발견(고치지 않음)**: F-14~F-16으로 파일링.

