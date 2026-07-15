# Verification — F-14 `reconcile-vanished-entry-aborts-pass`

**machine-owns-GREEN.** 아래 RED/GREEN 판정은 `bugfix-status.mjs`가 `red.sha`·`green.sha`를 **일회용 워크트리에 체크아웃해 테스트를 직접 재실행**한 결과이며, **사람이 요약한 것이 아니라 스크립트가 캡처한 원문**이다. 각 verify-record는 **트리 sha로 키가 걸려** 있어 재사용·위조가 불가능하다.

> **최종 트리 재캡처판이다.** 증인 `phase_g`가 lib (`src/store/pins/tests/recover_graves_production_seam.rs::phase_g_recover_graves_survives_vanishing_graves`)로 이전되고 `green.sha`가 바뀐 뒤, 릴리스 게이트 증거 **R-3·R-4**와 최종 스위트를 **최종 트리에서 다시 실행**해 이 문서를 통째로 재작성했다. 아래 §릴리스 게이트 증거의 R-1·R-3·R-4·최종 스위트는 **요약이 아니라 실행 원문 전문**이다(절단 없음).

- **red.sha** = `3b1e44f608cd00d0a580a3f5deb595d85a28a9d9` (tree `e92dacfe267afa523de84095f85dc473bd363413`) — 40-put repro 포함
- **green.sha** = `b2d0f3120ca97d7e25f0c1b2b9611704748bed5c` (tree `c5a9531bd89ece3a9086569e083224b1d4971e75`)
- **HEAD(최종 트리)** = `9882d618e11498f8703896dd99b58865ced9e0bf`

## 락 (`bugfix-lock.json`) — 현재값

````json
{
  "regressionCmd": "cargo test --lib -- reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot",
  "characterizationCmd": "cargo test --lib --bins --test adversarial --test contract --test e2e --test layout_tree --test openapi --test regression_reconcile_gc_dedup_race -- --skip reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot --skip reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot",
  "flips": [
    {
      "testId": "reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot",
      "symptomToken": "PASS ABORTED"
    },
    {
      "testId": "reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot",
      "symptomToken": "PASS ABORTED"
    }
  ],
  "scope": [
    "src/store/**",
    "docs/adr/**",
    "scripts/f14-witness-gate.sh",
    "CONTEXT.md"
  ],
  "red": {
    "sha": "3b1e44f608cd00d0a580a3f5deb595d85a28a9d9"
  },
  "green": {
    "sha": "b2d0f3120ca97d7e25f0c1b2b9611704748bed5c"
  },
  "notes": "단일 플립: '스냅샷 이후 사라진 .objects 항목이 패스 전체를 Err로 중단시킨다' → '그 항목을 건너뛰고 패스가 계속된다'. ⚠ 수정은 '?를 없앤다'가 아니다 — ErrorKind::NotFound인 경우에만 건너뛰고, 다른 모든 io 에러(EACCES·EIO 등)는 여전히 무가공 전파해야 한다(B7). 그러지 않으면 진짜 I/O 장애를 삼켜 두 번째 플립이 된다. characterizationCmd가 --skip으로 회귀 증인만 제외하는 이유: 증인이 lib 테스트라 분리 바이너리가 없다. ReconcileStats에 'skipped' 카운터를 추가하는 수정은 금지 — layout_tree.rs의 전수 구조체 assert_eq! 3곳이 깨진다(두 번째 플립). | scope 개정(plan gate r3 P-5 수용 + 인간의 옵션 B 승인, 2026-07-13): docs/adr/** 를 명시 추가. P-5를 봉인하려면 증인 W11이 프로덕션 진입점(PassGuard::begin → recover_graves)을 타야 하고, 그러려면 Hooks에 8번째 필드(pre_recover_grave, 프로덕션 None ⇒ no-op ⇒ 관측 행동 변화 0)를 열어야 한다. 그 사실과 ADR-0002의 P4 봉인이 여전히 성립한다는 논증(관측 훅은 보호 판정 경로를 만들지 않는다)을 ADR-0002에 기록하는 것이 B-1의 작업 항목이므로, 그 경로는 테스트도 slug-키 북키핑도 아니어서 B4에서 면제되지 않는다 — 선언이 정답이다. | RED 재포착(plan gate r10 P-15 수용, 2026-07-14): flips[]가 2행이 됐고 red.sha가 33d05ca → ac58bd7로 이동했다. 이유: 종전 증인은 Blob 분기(read)를 때렸는데 프론트매터의 증상은 Temp 분기(metadata)다 → Blob만 고치는 픽스가 잠긴 회귀를 초록으로 만들면서 프로덕션이 실제로 밟는 Temp 경로는 망가진 채 남을 수 있었다(B1/B2가 헛돈다). 두 행은 하드룰 10의 '같은 단일 관측 행동에 대한 N개 증인'이다 — 뒤집히는 행동은 여전히 하나(사라진 항목이 패스를 중단시킨다 → 건너뛰고 완주). ac58bd7은 Hooks에 8번째 훅 pre_entry(prod None ⇒ no-op)를 열고 항목 루프의 첫 FS 접촉 직전에 seam 1줄을 꽂았다 — 7개 훅 중 Temp 분기에 결정적으로 park를 걸 수 있는 것이 하나도 없었기 때문이다. ? 전파는 무변경 ⇒ 버그는 살아 있고 두 증인 모두 RED, characterization 138 GREEN. | scope 개정(plan gate r21 P-34 · r22 P-35/P-36 수용, 2026-07-14): scripts/f14-witness-gate.sh 를 정확 경로로 추가(와일드카드 아님 ⇒ 비-테스트 표면이 파일 하나만큼만 넓어진다). 근거: 선언된 증인이 컴파일·등록조차 되지 않은 채 스위트가 '0 failed'를 보고할 수 있다(mod 한 줄 삭제 → 131 → 128 passed, 경고 0). 그것을 막는 발견 단언을 **하네스 안**(Rust 테스트 W-REG)에 두면 #[ignore] 한 줄로 재갈이 물린다 — 복합 공격 실측: W-REG에 #[ignore] + mod 삭제 → 128 passed / 1 ignored / exit 0(증인 3개 증발). ⇒ 레지스트리 게이트는 **감사 대상 하네스 밖**에 있어야 한다. 그 스크립트가 acceptance의 0단계다: 타깃별 `cargo test -- --list`를 (^|::)<id>: test$ 로 매칭해 선언된 증인이 전부 발견되는지 검증하고(누락 → exit 1), 전 타깃의 ignored 수를 파싱해 0이 아니면 exit 1. | scope 개정(structure gate r1 S-1 수용, 2026-07-15): CONTEXT.md를 명시 추가. 근거: conductor-side /code-review의 Standards 축이 **CONTEXT.md 미갱신을 하드 위반**으로 잡았다 — F-14가 만든 도메인 개념(소멸/Vanished · 부재의 증거/Absent · 스냅샷 항목/Entry·Seen)이 pins·reconcile·atomic 세 모듈을 관통하고 PassGuard::begin의 시그니처에까지 올라왔는데 용어집에 없었다. CONTEXT.md는 테스트도 slug-키 북키핑도 아니므로 B4에서 면제되지 않는다 — 빼면 하드 위반이 되살아나고, 선언하지 않으면 미선언 변경 표면이다. 선언이 정답이다(선례: 직전 파이프라인 reconcile-gc-dedup-race의 릴리스 게이트 R-4가 같은 이유로 CONTEXT.md·docs/adr/**를 추가했다). | RED 3차 재포착 + reproCmd 선언(release gate r1 R-2 수용, 2026-07-15): red.sha ac58bd7 → 8131d25. R-2: green verify-record의 repro가 null이었고, 진단이 적은 원 repro('40 동시 put × reconcile 루프')는 red.sha에서 **빨갛지 않았다** — tests/adversarial.rs의 그 안무가 let _ = run_once(..)로 결과를 버려서 매 실행 버그를 밟으면서도 초록이었기 때문이다. 그것을 관측하는 판본(tests/repro_concurrent_puts_reconcile.rs)을 새 baseline에 커밋해 reproCmd로 선언한다. throwaway 워크트리에서 red.sha의 코드를 sed로 패치해 빨갛게 만드는 것은 **위조된 repro**이므로 하지 않았다. 비공허성: 반복 증인(put in-flight 중 완주 패스 ≥5) ∧ 레이스 증인(put_temp_vanishes = vanished_during_pass − passes ≥ 10 — .objects에 .tmp-를 만드는 주체가 put과 reconcile의 gc-pending 둘뿐이라 산술로 강제된다). 실측 RED 20/20 FAIL · GREEN 20/20 PASS. | scope 개정(structure gate r1 S-1 수용, 2026-07-15): CONTEXT.md를 명시 추가. 근거: conductor-side /code-review의 Standards 축이 **CONTEXT.md 미갱신을 하드 위반**으로 잡았다 — F-14가 만든 도메인 개념(소멸/Vanished · 부재의 증거/Absent · 스냅샷 항목/Entry·Seen)이 pins·reconcile·atomic 세 모듈을 관통하고 PassGuard::begin의 시그니처에까지 올라왔는데 용어집에 없었다. CONTEXT.md는 테스트도 slug-키 북키핑도 아니므로 B4에서 면제되지 않는다 — 빼면 하드 위반이 되살아나고, 선언하지 않으면 미선언 변경 표면이다. 선언이 정답이다(선례: 직전 파이프라인 reconcile-gc-dedup-race의 릴리스 게이트 R-4가 같은 이유로 CONTEXT.md·docs/adr/**를 추가했다). | RED 4차 재포착(release gate r2 R-2' 수용, 2026-07-15): red.sha 8131d25 → 3b1e44f. R-2': 원 안무는 정확히 40 puts(40 tasks × 1 put)인데 직전 repro가 1000 puts(40 workers × 25 rounds)였다 — timing race라 증폭된 workload의 실패가 원 40-put 시나리오를 증명하지 않는다. reproCmd(cargo test --test repro_concurrent_puts_reconcile)를 정확히 40-put으로 되돌렸고, 1000-put은 stress_concurrent_puts_reconcile.rs로 분리(reproCmd 아님). 결정화는 put이 아니라 reconcile 쪽 밀도로만(sleep 제거 · 다중 reconcile 태스크 · put 배리어). 비공허 문턱은 40-put 규모로 실측 재조정: overlapped ≥3(옛 ≥5) · put_temp_vanishes ≥20(옛 ≥10 — 관측치가 40-put에서도 크다). 실측 RED 20/20 FAIL · GREEN 20/20 PASS. 정본 문턱은 계획 §3-a.",
  "reproCmd": "cargo test --test repro_concurrent_puts_reconcile"
}
````

## 주장과 판정 (verify-record 기반)

각 판정의 근거는 아래 §RED·§GREEN에 실린 **스크립트 캡처 원문 tail**이다.

| # | 주장 | verify-record 판정 |
|---|---|---|
| 1 | 회귀 증인 **2개**가 red.sha에서 **FAIL** + symptomToken `PASS ABORTED` | red.regression: **exit 101 · failed=True · symptomTokenPresent=True** ✅ |
| 2 | characterization이 red.sha에서 **GREEN** (두 번째 잠복 플립이 빨강 뒤에 숨지 않는다) | red.characterization: **exit 0 · green=True** ✅ |
| 3 | **원 40-put repro가 red.sha에서 실제로 재현된다** | red.repro: **exit 101 · reproduced=True** ✅ |
| 4 | 회귀 증인 2개가 green.sha에서 **PASS** | green.regression: **exit 0 · passed=True** ✅ |
| 5 | characterization이 green.sha에서도 **GREEN** (플립은 정확히 하나) | green.characterization: **exit 0 · green=True** ✅ |
| 6 | **원 40-put repro가 green.sha에서 사라졌다** | green.repro: **exit 0 · reproduces=False** ✅ |

---

## RED @ `3b1e44f608cd00d0a580a3f5deb595d85a28a9d9` (red.sha · tree `e92dacfe267afa523de84095f85dc473bd363413`)

### regression — 스크립트가 캡처한 **원문 tail** (exit 101 · failed=True · symptomTokenPresent=True)

````text
es:

---- store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot stdout ----

thread 'store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot' (24184541) panicked at src/store/pins/tests/vanished_temp_regression.rs:196:9:
PASS ABORTED — 스냅샷 이후 사라진 **temp**(.tmp-f14-temp-victim)를 만난 reconcile 패스가 그 항목을 **건너뛰지 않고** 패스 **전체**를 Err로 중단시켰다. 범인 `?`는 Temp 분기의 `let mtime = e.metadata().await?…`(나이 판정 **전에** stat한다). 이것이 프론트매터가 적은 **바로 그 증상**이다: 동시 `put_stream`이 `.tmp-<uniq>`를 최종 blob 이름으로 rename하면 스냅샷에 잡힌 temp가 사라진다 → 패스 전체 중단. err=Os { code: 2, kind: NotFound, message: "No such file or directory" } kind=NotFound
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot stdout ----

thread 'store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot' (24184540) panicked at src/store/pins/tests/vanished_entry_regression.rs:189:9:
PASS ABORTED — 스냅샷 이후 사라진 항목(victims=["02e8e4db0fb46bc832573124554faf3a24b05d4b4fe5d8e3e0a611ee6cd277aa", "e3dbdd09192f1cebd4185cf8ba31a68537920becf58c9d2c0bf81ab802c06b75"])을 만난 reconcile 패스가 그 항목을 **건너뛰지 않고** 패스 **전체**를 Err로 중단시켰다. 동시 쓰기(`atomic::write_atomic`의 `.tmp-<uniq>` → rename)가 있는 한 이것은 상시 발생한다. err=Os { code: 2, kind: NotFound, message: "No such file or directory" } kind=NotFound (NotFound = ENOENT: 범인 `?`는 reconcile.rs:199(Temp `metadata`) / :208(Blob `read`) — :192(`file_type`)는 DT_UNKNOWN FS에서의 잠복 범인)


failures:
    store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot
    store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 118 filtered out; finished in 0.28s
````

### characterization — 스크립트가 캡처한 **원문 tail** (exit 0 · green=True)

````text
measured; 2 filtered out; finished in 1.57s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 8 tests
test reserved_suffix_keys_rejected_at_runtime ... ok
test upload_rejected_507_no_temp_residue_existing_intact ... ok
test internal_object_reads_are_no_store_and_vary_authorization ... ok
test download_content_type_is_stored_type_and_206_has_all_headers ... ok
test query_key_decoding_and_validation_contract ... ok
test concurrent_nested_puts_with_reconcile_loop_preserve_all ... ok
test concurrent_same_key_put_delete_self_consistent ... ok
test concurrent_readers_never_observe_desync_on_same_size_overwrite ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.32s


running 1 test
test responses_match_openapi_schema ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 2 tests
test public_listener_isolates_api_and_internal_buckets ... ok
test large_object_streaming_put_and_range_download ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.52s


running 3 tests
test put_stream_midflight_temp_observed_and_preserved ... ok
test symlinked_commit_pointer_current_behavior ... ok
test on_disk_layout_golden_tree ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s


running 5 tests
test does_not_serve_interactive_docs_ui ... ok
test spec_binary_upload_and_internal_only ... ok
test serves_generated_openapi_spec_unauthenticated ... ok
test spec_download_declares_binary_range_and_key_grammar ... ok
test spec_object_ops_document_error_codes ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 1 test
test dedup_put_during_reconcile_window_must_not_lose_blob ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.12s
````

### repro (40-put) — 스크립트가 캡처한 **원문 tail** (exit 101 · reproduced=True)

````text
   Compiling files v0.1.0 (/private/var/folders/dr/804s3_sj2m50rfbtkr6k1bsm0000gn/T/bugfix-verify-4AjZ0v)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.82s
     Running tests/repro_concurrent_puts_reconcile.rs (target/debug/deps/repro_concurrent_puts_reconcile-9b6d00f181af3aeb)
error: test failed, to rerun pass `--test repro_concurrent_puts_reconcile`

running 1 test
test original_repro_concurrent_puts_do_not_abort_the_reconcile_pass ... FAILED

failures:

---- original_repro_concurrent_puts_do_not_abort_the_reconcile_pass stdout ----
REPRO WITNESS puts=40 reconcile_loops=4 observer_loops=8 passes=6 overlapped_passes=6 scans=128 temps_seen=913 vanished=153 vanished_during_pass=153 put_temp_vanishes=147 pass_errs=2 pass_errs_notfound=2 first_err=[kind=NotFound err=Os { code: 2, kind: NotFound, message: "No such file or directory" }]

thread 'original_repro_concurrent_puts_do_not_abort_the_reconcile_pass' (24187075) panicked at tests/repro_concurrent_puts_reconcile.rs:339:5:
assertion `left == right` failed: PASS ABORTED — 동시 put과 경합하는 reconcile 패스가 `Err`로 중단됐다(2/6 패스, 그중 NotFound=2). 스냅샷 이후 사라진 `.objects` 항목(동시 `write_atomic`이 `.tmp-<uniq>` → `<sha>`로 rename해 치운 그 항목)은 **그 항목만 건너뛰고** 패스는 완주해야 한다(F-14). 첫 에러: kind=NotFound err=Os { code: 2, kind: NotFound, message: "No such file or directory" } — `NotFound`(ENOENT)가 곧 소멸의 물증이다: 패스 자신이 스냅샷에 잡아 둔 항목을 stat하다 밟았다. 관측자가 독립적으로 센 소멸도 153건이다(그중 패스 in-flight 중 153건 · put이 만든 temp의 소멸 하한 147건).
  left: 2
 right: 0
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    original_repro_concurrent_puts_do_not_abort_the_reconcile_pass

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.41s
````

---

## GREEN @ `b2d0f3120ca97d7e25f0c1b2b9611704748bed5c` (green.sha · tree `c5a9531bd89ece3a9086569e083224b1d4971e75`)

### regression — 스크립트가 캡처한 **원문 tail** (exit 0 · passed=True)

````text

running 2 tests
test store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 140 filtered out; finished in 0.28s
````

### characterization — 스크립트가 캡처한 **원문 tail** (exit 0 · green=True)

````text
al_object_reads_are_no_store_and_vary_authorization ... ok
test query_key_decoding_and_validation_contract ... ok
test concurrent_nested_puts_with_reconcile_loop_preserve_all ... ok
test concurrent_same_key_put_delete_self_consistent ... ok
test concurrent_readers_never_observe_desync_on_same_size_overwrite ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.29s


running 1 test
test responses_match_openapi_schema ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 8 tests
test corrupt_dir_as_regular_file_propagates_enotdir ... ok
test blob_symlink_to_directory_propagates_isadirectory ... ok
test corrupt_dir_as_dangling_symlink_propagates_raw_notfound ... ok
test symlinked_objects_dir_without_vanishing_is_unchanged ... ok
test public_listener_isolates_api_and_internal_buckets ... ok
test symlinked_objects_dir_with_a_vanished_entry_completes ... ok
test large_object_streaming_put_and_range_download ... ok
test dangling_temp_symlink_keeps_lstat_semantics ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.14s


running 3 tests
test put_stream_midflight_temp_observed_and_preserved ... ok
test symlinked_commit_pointer_current_behavior ... ok
test on_disk_layout_golden_tree ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s


running 5 tests
test does_not_serve_interactive_docs_ui ... ok
test spec_binary_upload_and_internal_only ... ok
test spec_object_ops_document_error_codes ... ok
test spec_download_declares_binary_range_and_key_grammar ... ok
test serves_generated_openapi_spec_unauthenticated ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 1 test
test dedup_put_during_reconcile_window_must_not_lose_blob ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.19s
````

### repro (40-put) — 스크립트가 캡처한 **원문 tail** (exit 0 · reproduces=False)

````text

running 1 test
test original_repro_concurrent_puts_do_not_abort_the_reconcile_pass ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.52s
````

---

## phase_g 봉인 서술 — 거짓 20/20에서 결정적 park로

**무엇이 잡혔나.** 릴리스 게이트를 직접 재확인하던 중, 증인 `phase_g`가 **green.sha에서 5/5 결정적으로 RED**임이 드러났다. 직전 서브에이전트는 이 증인을 **거짓 20/20 PASS**로 보고했고, 그 거짓 보고가 게이트를 막고 있었다(이 파이프라인이 30+라운드에 걸쳐 잡아 온 '확인 없이 단언'의 재발이다).

**근본 원인은 프로덕션이 아니다.** 실패는 **구 통합 무대**(`tests/reconcile_vanishing_entries.rs::phase_g_...`)의 **동시성 랑데부**였다. 그 판본은 `run_once`를 spawn한 뒤 `.gc-grave-*` 개수가 줄기를 **busy-spin**으로 기다렸다가 남은 무덤을 외부에서 지웠는데, 그 조율은 신뢰성이 없어 **무대가 `K_KEEP`을 처리하기 전에 단언에 도달**했다(`K_KEEP의 무덤이 남아 있다` · 옛 line 372). 프로덕션은 옳았지만(무덤을 홀로 돌리면 `grave_count → 0`) 계측을 어떻게 붙여도(stderr·파일) 타이밍이 밀려 **초록으로 뒤집히는 하이젠버그**였다.

**봉인.** 랑데부를 **9번째 훅 `pre_recover_grave` 기반 결정적 park**로 대체하고(SPIN_BUDGET 제거), 증인을 **lib로 이전**했다(`src/store/pins/tests/recover_graves_production_seam.rs`). 훅은 `PassGuard::begin → recover_graves`라는 **진짜 프로덕션 경로**에서 **무덤 항목 하나당 정확히 한 번, 두 분기(rename·remove) 이전**에 발화한다(prod = `None` ⇒ no-op ⇒ 관측 행동 변화 0). **첫 발화에서 park**하면 프로덕션은 grave[0] 파일 연산 **직전**에 서고 스냅샷은 이미 고정되어 **모든 무덤이 아직 디스크에 있다** ⇒ 그 park 창에서 우리가 지우는 것은 **readdir 순서와 무관하게 100% 결정적**이다. `K_KEEP` 무덤은 **우리가 절대 건드리지 않으므로** 사라졌다면 **프로덕션의 remove 분기가 지운 것**뿐이다 — 이것이 R-4의 M-REMOVE-NOOP 킬 포인트다.

**지휘자 직접 확인.** 지휘자가 **clean 재빌드로 `phase_g`를 20/20 직접 확인**했다(결정적 GREEN). 본 문서의 R-4(§릴리스 게이트 증거)는 그것을 독립적으로 뒷받침한다 — `cargo clean -p files` 후 재빌드에서 M-REMOVE-NOOP 뮤턴트 아래 **RED**, 원복 트리에서 **GREEN**이다.

---

## 릴리스 게이트 증거 (전문 · 절단 없음)

아래 네 블록은 **최종 트리에서 실제로 실행한 원문 전체**다. `head -N` 절단 없이 통째로 싣는다.

### R-1 — release 프로파일 (`/tmp/f14-r1-release-final.txt`)

````text
## R-1 — release 프로파일 통제 (교정판)

```
# R-1 release 프로파일 통제 — 계획 :2204-2205의 정확한 2줄 (최종 트리 b2d0f31)

⚠ r3의 R-1' 교정: 이전 판본은 첫 줄을 `--test repro_concurrent_puts_reconcile`로 잘못 돌렸다.
계획이 명시한 정확한 타깃은 `--test reconcile_vanishing_entries`(Phase E/T release 통합 증인)다.

$ cargo test --release --test reconcile_vanishing_entries
running 2 tests
test phase_t_temp_deletion_counts_only_what_we_deleted ... ok
test phase_e_entry_loop_survives_vanishing_entries ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.76s
exit=0

$ cargo test --release --lib -- reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot
test store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 140 filtered out; finished in 0.28s
exit=0
```

## R-3 — 증인 게이트 8개 술어 뮤테이션 (`/tmp/f14-r3-final.txt`)

게이트 스크립트의 **8개 술어**(DISC·LIST-RC·N0·IGN·FAIL·RC + M-SIGPIPE + M-OLDPARSER)를 **하나씩 제거**한 사본으로 `--selftest`가 RED가 되는지 실증한다. 원본은 md5 불변(사본에만 뮤테이션). **8/8 RED · 살아남은 술어 0.**

````text
############################################################
# R-3  게이트 뮤테이션 — 8개 술어 하나씩 제거 → --selftest RED 실증
# 최종 트리 재캡처.  원본: scripts/f14-witness-gate.sh
# 원본 md5 (변형 전) = 49ec1fcd798341ae7599dd3855babf11
# HEAD = 9882d618e11498f8703896dd99b58865ced9e0bf
# 규칙: 원본은 절대 건드리지 않는다. 각 뮤턴트는 사본에만. --selftest 는 cargo 미호출(자족).
############################################################

======================================================================
=== 대조군 (무수정 사본) — bash <copy> --selftest ===
    사본 md5 = 49ec1fcd798341ae7599dd3855babf11  (원본과 동일: True)
----- 원문 출력 -----
== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) ==
-- ② 결과 게이트 --
   [ok  ] (a) 1 ignored        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=FAIL (기대 FAIL)
   [ok  ] (b) 10 ignored       rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (c) 전부 정상    rc=0    게이트=PASS (기대 PASS)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (d) 10 failed        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (e) cargo rc!=0      rc=101  게이트=FAIL (기대 FAIL)
   [ok  ] (f) 결과 줄 0개  rc=0    게이트=FAIL (기대 FAIL)
-- ① 발견 게이트 --
   [ok  ] (g) 증인 누락             발견=FAIL (기대 FAIL)
   [ok  ] (h) 조기매치+큰목록          발견=PASS (기대 PASS)
   [ok  ] (i) 목록 rc!=0              발견=FAIL (기대 FAIL)

SELFTEST: PASS  (9/9 · 케이스·술어의 정본 = §0-h 매트릭스)
----- exit code = 0  → 대조군 판정 = PASS (기대 PASS) -----

======================================================================
=== 뮤턴트 M-PRED-DISC ===
    지운 것: PRED-DISC: `MISSING WITNESS` 의 bad=1 (증인 발견 실패 신호)
    기대 킬 케이스: (g) 증인 누락
    사본 md5 = 7244af74a44fe57f8afb15ffe8fa414e  (원본과 다름: True)
----- 이 뮤턴트가 대조군(control.sh)과 다른 지점(diff -u) -----
--- /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/control.sh	2026-07-15 13:26:26
+++ /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/M-PRED-DISC.sh	2026-07-15 13:26:27
@@ -98,7 +98,7 @@
     if has_witness "$f" "$id"; then            # ← PRED-DISC
       echo "   ok    [$target] $id"
     else
-      echo "   MISSING WITNESS  [$target] $id"; bad=1
+      echo "   MISSING WITNESS  [$target] $id"
     fi
   done
   return "$bad"
----- `bash <copy> --selftest` 원문 출력 -----
== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) ==
-- ② 결과 게이트 --
   [ok  ] (a) 1 ignored        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=FAIL (기대 FAIL)
   [ok  ] (b) 10 ignored       rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (c) 전부 정상    rc=0    게이트=PASS (기대 PASS)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (d) 10 failed        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (e) cargo rc!=0      rc=101  게이트=FAIL (기대 FAIL)
   [ok  ] (f) 결과 줄 0개  rc=0    게이트=FAIL (기대 FAIL)
-- ① 발견 게이트 --
   [FAIL] (g) 증인 누락             발견=PASS (기대 FAIL)
   [ok  ] (h) 조기매치+큰목록          발견=PASS (기대 PASS)
   [ok  ] (i) 목록 rc!=0              발견=FAIL (기대 FAIL)

SELFTEST: FAIL  (8/9)
----- exit code = 1  →  RED  (술어가 핀되어 있었다 = 킬 성공) -----

======================================================================
=== 뮤턴트 M-PRED-LIST-RC ===
    지운 것: PRED-LIST-RC: `LIST FAILED` 의 bad=1 (목록 명령 실패 신호)
    기대 킬 케이스: (i) 목록 rc!=0
    사본 md5 = c830f386cc6afe22657e503acf43244e  (원본과 다름: True)
----- 이 뮤턴트가 대조군(control.sh)과 다른 지점(diff -u) -----
--- /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/control.sh	2026-07-15 13:26:26
+++ /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/M-PRED-LIST-RC.sh	2026-07-15 13:26:27
@@ -93,7 +93,7 @@
     if ! f="$("$resolve" "$target")"; then     # ← PRED-LIST-RC: 목록 명령이 죽으면 오진하지 않는다
       echo "   LIST FAILED  [$target]  cargo --list exit=$(cat "$TMP/rc.$target" 2>/dev/null)"
       echo "                (빌드 실패다. '증인 없음'이 아니다 — 오진 금지)"
-      bad=1; continue
+      continue
     fi
     if has_witness "$f" "$id"; then            # ← PRED-DISC
       echo "   ok    [$target] $id"
----- `bash <copy> --selftest` 원문 출력 -----
== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) ==
-- ② 결과 게이트 --
   [ok  ] (a) 1 ignored        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=FAIL (기대 FAIL)
   [ok  ] (b) 10 ignored       rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (c) 전부 정상    rc=0    게이트=PASS (기대 PASS)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (d) 10 failed        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (e) cargo rc!=0      rc=101  게이트=FAIL (기대 FAIL)
   [ok  ] (f) 결과 줄 0개  rc=0    게이트=FAIL (기대 FAIL)
-- ① 발견 게이트 --
   [ok  ] (g) 증인 누락             발견=FAIL (기대 FAIL)
   [ok  ] (h) 조기매치+큰목록          발견=PASS (기대 PASS)
   [FAIL] (i) 목록 rc!=0              발견=PASS (기대 FAIL)

SELFTEST: FAIL  (8/9)
----- exit code = 1  →  RED  (술어가 핀되어 있었다 = 킬 성공) -----

======================================================================
=== 뮤턴트 M-PRED-N0 ===
    지운 것: PRED-N0: 결과-줄-0개 가드 (`[ "$n" -eq 0 ]`)
    기대 킬 케이스: (f) 결과 줄 0개
    사본 md5 = 6f0cef7c3703b706e3a49790dbaee8e5  (원본과 다름: True)
----- 이 뮤턴트가 대조군(control.sh)과 다른 지점(diff -u) -----
--- /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/control.sh	2026-07-15 13:26:26
+++ /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/M-PRED-N0.sh	2026-07-15 13:26:27
@@ -122,7 +122,7 @@
   local n p f g bad=0
   read -r n p f g < <(tally "$1")
   echo "   결과 줄 ${n}개 · passed=${p} · failed=${f} · ignored=${g} · cargo exit=${2}"
-  if [ "$n" -eq 0 ]; then echo "   FAIL: 'test result:' 줄이 0개 — 스위트가 돌지 않았다"; bad=1; fi
+  : # [M-PRED-N0] 결과-줄-0개 가드 삭제
   if [ "$g" -ne 0 ]; then echo "   FAIL: ignored=${g} (≠0) — 스킵된 red = 위조된 red (하드룰 9)"; bad=1; fi
   if [ "$f" -ne 0 ]; then echo "   FAIL: failed=${f} (≠0)"; bad=1; fi
   if [ "$2" -ne 0 ]; then echo "   FAIL: cargo exit=${2} (≠0)"; bad=1; fi
----- `bash <copy> --selftest` 원문 출력 -----
== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) ==
-- ② 결과 게이트 --
   [ok  ] (a) 1 ignored        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=FAIL (기대 FAIL)
   [ok  ] (b) 10 ignored       rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (c) 전부 정상    rc=0    게이트=PASS (기대 PASS)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (d) 10 failed        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (e) cargo rc!=0      rc=101  게이트=FAIL (기대 FAIL)
   [FAIL] (f) 결과 줄 0개  rc=0    게이트=PASS (기대 FAIL)
-- ① 발견 게이트 --
   [ok  ] (g) 증인 누락             발견=FAIL (기대 FAIL)
   [ok  ] (h) 조기매치+큰목록          발견=PASS (기대 PASS)
   [ok  ] (i) 목록 rc!=0              발견=FAIL (기대 FAIL)

SELFTEST: FAIL  (8/9)
----- exit code = 1  →  RED  (술어가 핀되어 있었다 = 킬 성공) -----

======================================================================
=== 뮤턴트 M-PRED-IGN ===
    지운 것: PRED-IGN: 숫자 ignored 검사 (`[ "$g" -ne 0 ]`)
    기대 킬 케이스: (a) 1 ignored · (b) 10 ignored
    사본 md5 = de5ff3fbd825a7fe512195e21ab43baf  (원본과 다름: True)
----- 이 뮤턴트가 대조군(control.sh)과 다른 지점(diff -u) -----
--- /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/control.sh	2026-07-15 13:26:26
+++ /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/M-PRED-IGN.sh	2026-07-15 13:26:27
@@ -123,7 +123,7 @@
   read -r n p f g < <(tally "$1")
   echo "   결과 줄 ${n}개 · passed=${p} · failed=${f} · ignored=${g} · cargo exit=${2}"
   if [ "$n" -eq 0 ]; then echo "   FAIL: 'test result:' 줄이 0개 — 스위트가 돌지 않았다"; bad=1; fi
-  if [ "$g" -ne 0 ]; then echo "   FAIL: ignored=${g} (≠0) — 스킵된 red = 위조된 red (하드룰 9)"; bad=1; fi
+  : # [M-PRED-IGN] 숫자 ignored 검사 삭제
   if [ "$f" -ne 0 ]; then echo "   FAIL: failed=${f} (≠0)"; bad=1; fi
   if [ "$2" -ne 0 ]; then echo "   FAIL: cargo exit=${2} (≠0)"; bad=1; fi
   return "$bad"
----- `bash <copy> --selftest` 원문 출력 -----
== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) ==
-- ② 결과 게이트 --
   [FAIL] (a) 1 ignored        rc=0    게이트=PASS (기대 FAIL)  · [ok  ] 옛 파서=FAIL (기대 FAIL)
   [FAIL] (b) 10 ignored       rc=0    게이트=PASS (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (c) 전부 정상    rc=0    게이트=PASS (기대 PASS)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (d) 10 failed        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (e) cargo rc!=0      rc=101  게이트=FAIL (기대 FAIL)
   [ok  ] (f) 결과 줄 0개  rc=0    게이트=FAIL (기대 FAIL)
-- ① 발견 게이트 --
   [ok  ] (g) 증인 누락             발견=FAIL (기대 FAIL)
   [ok  ] (h) 조기매치+큰목록          발견=PASS (기대 PASS)
   [ok  ] (i) 목록 rc!=0              발견=FAIL (기대 FAIL)

SELFTEST: FAIL  (7/9)
----- exit code = 1  →  RED  (술어가 핀되어 있었다 = 킬 성공) -----

======================================================================
=== 뮤턴트 M-PRED-FAIL ===
    지운 것: PRED-FAIL: 숫자 failed 검사 (`[ "$f" -ne 0 ]`)
    기대 킬 케이스: (d) 10 failed
    사본 md5 = 1c5bbc61037be8edf1f4119064bd6eb3  (원본과 다름: True)
----- 이 뮤턴트가 대조군(control.sh)과 다른 지점(diff -u) -----
--- /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/control.sh	2026-07-15 13:26:26
+++ /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/M-PRED-FAIL.sh	2026-07-15 13:26:27
@@ -124,7 +124,7 @@
   echo "   결과 줄 ${n}개 · passed=${p} · failed=${f} · ignored=${g} · cargo exit=${2}"
   if [ "$n" -eq 0 ]; then echo "   FAIL: 'test result:' 줄이 0개 — 스위트가 돌지 않았다"; bad=1; fi
   if [ "$g" -ne 0 ]; then echo "   FAIL: ignored=${g} (≠0) — 스킵된 red = 위조된 red (하드룰 9)"; bad=1; fi
-  if [ "$f" -ne 0 ]; then echo "   FAIL: failed=${f} (≠0)"; bad=1; fi
+  : # [M-PRED-FAIL] 숫자 failed 검사 삭제
   if [ "$2" -ne 0 ]; then echo "   FAIL: cargo exit=${2} (≠0)"; bad=1; fi
   return "$bad"
 }
----- `bash <copy> --selftest` 원문 출력 -----
== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) ==
-- ② 결과 게이트 --
   [ok  ] (a) 1 ignored        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=FAIL (기대 FAIL)
   [ok  ] (b) 10 ignored       rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (c) 전부 정상    rc=0    게이트=PASS (기대 PASS)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [FAIL] (d) 10 failed        rc=0    게이트=PASS (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (e) cargo rc!=0      rc=101  게이트=FAIL (기대 FAIL)
   [ok  ] (f) 결과 줄 0개  rc=0    게이트=FAIL (기대 FAIL)
-- ① 발견 게이트 --
   [ok  ] (g) 증인 누락             발견=FAIL (기대 FAIL)
   [ok  ] (h) 조기매치+큰목록          발견=PASS (기대 PASS)
   [ok  ] (i) 목록 rc!=0              발견=FAIL (기대 FAIL)

SELFTEST: FAIL  (8/9)
----- exit code = 1  →  RED  (술어가 핀되어 있었다 = 킬 성공) -----

======================================================================
=== 뮤턴트 M-PRED-RC ===
    지운 것: PRED-RC: cargo exit 검사 (`[ "$2" -ne 0 ]`)
    기대 킬 케이스: (e) cargo rc!=0
    사본 md5 = 559bb821c30ff8bb8cc637b19162734a  (원본과 다름: True)
----- 이 뮤턴트가 대조군(control.sh)과 다른 지점(diff -u) -----
--- /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/control.sh	2026-07-15 13:26:26
+++ /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/M-PRED-RC.sh	2026-07-15 13:26:27
@@ -125,7 +125,7 @@
   if [ "$n" -eq 0 ]; then echo "   FAIL: 'test result:' 줄이 0개 — 스위트가 돌지 않았다"; bad=1; fi
   if [ "$g" -ne 0 ]; then echo "   FAIL: ignored=${g} (≠0) — 스킵된 red = 위조된 red (하드룰 9)"; bad=1; fi
   if [ "$f" -ne 0 ]; then echo "   FAIL: failed=${f} (≠0)"; bad=1; fi
-  if [ "$2" -ne 0 ]; then echo "   FAIL: cargo exit=${2} (≠0)"; bad=1; fi
+  : # [M-PRED-RC] cargo exit 검사 삭제
   return "$bad"
 }
 
----- `bash <copy> --selftest` 원문 출력 -----
== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) ==
-- ② 결과 게이트 --
   [ok  ] (a) 1 ignored        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=FAIL (기대 FAIL)
   [ok  ] (b) 10 ignored       rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (c) 전부 정상    rc=0    게이트=PASS (기대 PASS)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (d) 10 failed        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [FAIL] (e) cargo rc!=0      rc=101  게이트=PASS (기대 FAIL)
   [ok  ] (f) 결과 줄 0개  rc=0    게이트=FAIL (기대 FAIL)
-- ① 발견 게이트 --
   [ok  ] (g) 증인 누락             발견=FAIL (기대 FAIL)
   [ok  ] (h) 조기매치+큰목록          발견=PASS (기대 PASS)
   [ok  ] (i) 목록 rc!=0              발견=FAIL (기대 FAIL)

SELFTEST: FAIL  (8/9)
----- exit code = 1  →  RED  (술어가 핀되어 있었다 = 킬 성공) -----

======================================================================
=== 뮤턴트 M-SIGPIPE ===
    지운 것: M-SIGPIPE: has_witness 의 '파이프 없음' 성질 (파이프 재도입)
    기대 킬 케이스: (h) 조기매치+큰목록
    사본 md5 = 69d00f5ffd5fc73162e9e07be1813398  (원본과 다름: True)
----- 이 뮤턴트가 대조군(control.sh)과 다른 지점(diff -u) -----
--- /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/control.sh	2026-07-15 13:26:26
+++ /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/M-SIGPIPE.sh	2026-07-15 13:26:27
@@ -72,7 +72,7 @@
   printf '%s\n' "$f"                           # ← 파이프가 아니라 **경로**를 넘긴다
 }
 
-has_witness() { grep -qE "(^|::)${2}: test\$" "$1"; }   # $1 = 목록 파일 · $2 = id  ⇒ **파이프 없음**
+has_witness() { cat "$1" | grep -qE "(^|::)${2}: test\$"; }   # [M-SIGPIPE] 파이프 재도입
 
 required() {                                   # $1 = platform
   case "$1" in
----- `bash <copy> --selftest` 원문 출력 -----
== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) ==
-- ② 결과 게이트 --
   [ok  ] (a) 1 ignored        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=FAIL (기대 FAIL)
   [ok  ] (b) 10 ignored       rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (c) 전부 정상    rc=0    게이트=PASS (기대 PASS)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (d) 10 failed        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (e) cargo rc!=0      rc=101  게이트=FAIL (기대 FAIL)
   [ok  ] (f) 결과 줄 0개  rc=0    게이트=FAIL (기대 FAIL)
-- ① 발견 게이트 --
   [ok  ] (g) 증인 누락             발견=FAIL (기대 FAIL)
   [FAIL] (h) 조기매치+큰목록          발견=FAIL (기대 PASS)
   [ok  ] (i) 목록 rc!=0              발견=FAIL (기대 FAIL)

SELFTEST: FAIL  (8/9)
----- exit code = 1  →  RED  (술어가 핀되어 있었다 = 킬 성공) -----

======================================================================
=== 뮤턴트 M-OLDPARSER ===
    지운 것: M-OLDPARSER: 숫자 파서를 옛 부분문자열 파서로 되돌림
    기대 킬 케이스: (b) 10 ignored · (d) 10 failed
    사본 md5 = b993c166ef5a8b41b2e9a531f3bfebef  (원본과 다름: True)
----- 이 뮤턴트가 대조군(control.sh)과 다른 지점(diff -u) -----
--- /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/control.sh	2026-07-15 13:26:26
+++ /private/tmp/claude-501/-Users-ukyi-workspace-files/c1e3d583-671c-4a21-8ae2-e0d77685fc85/scratchpad/r3-mut/M-OLDPARSER.sh	2026-07-15 13:26:27
@@ -123,8 +123,8 @@
   read -r n p f g < <(tally "$1")
   echo "   결과 줄 ${n}개 · passed=${p} · failed=${f} · ignored=${g} · cargo exit=${2}"
   if [ "$n" -eq 0 ]; then echo "   FAIL: 'test result:' 줄이 0개 — 스위트가 돌지 않았다"; bad=1; fi
-  if [ "$g" -ne 0 ]; then echo "   FAIL: ignored=${g} (≠0) — 스킵된 red = 위조된 red (하드룰 9)"; bad=1; fi
-  if [ "$f" -ne 0 ]; then echo "   FAIL: failed=${f} (≠0)"; bad=1; fi
+  if ! old_parser "$1"; then echo "   FAIL: [M-OLDPARSER] 부분문자열 파서 위반"; bad=1; fi
+  : # [M-OLDPARSER] 숫자 failed 검사 제거 — 옛 파서는 failed 를 안 본다
   if [ "$2" -ne 0 ]; then echo "   FAIL: cargo exit=${2} (≠0)"; bad=1; fi
   return "$bad"
 }
----- `bash <copy> --selftest` 원문 출력 -----
== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) ==
-- ② 결과 게이트 --
   [ok  ] (a) 1 ignored        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=FAIL (기대 FAIL)
   [FAIL] (b) 10 ignored       rc=0    게이트=PASS (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (c) 전부 정상    rc=0    게이트=PASS (기대 PASS)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [FAIL] (d) 10 failed        rc=0    게이트=PASS (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (e) cargo rc!=0      rc=101  게이트=FAIL (기대 FAIL)
   [ok  ] (f) 결과 줄 0개  rc=0    게이트=FAIL (기대 FAIL)
-- ① 발견 게이트 --
   [ok  ] (g) 증인 누락             발견=FAIL (기대 FAIL)
   [ok  ] (h) 조기매치+큰목록          발견=PASS (기대 PASS)
   [ok  ] (i) 목록 rc!=0              발견=FAIL (기대 FAIL)

SELFTEST: FAIL  (7/9)
----- exit code = 1  →  RED  (술어가 핀되어 있었다 = 킬 성공) -----

======================================================================
======================  집계  ======================================
======================================================================
대조군(무수정): PASS (기대 PASS)
뮤턴트 총 8개 중 RED(킬 성공): 8/8
    M-PRED-DISC      exit=1    RED
    M-PRED-LIST-RC   exit=1    RED
    M-PRED-N0        exit=1    RED
    M-PRED-IGN       exit=1    RED
    M-PRED-FAIL      exit=1    RED
    M-PRED-RC        exit=1    RED
    M-SIGPIPE        exit=1    RED
    M-OLDPARSER      exit=1    RED

살아남은 술어: 없음 (0개)
판정: 8/8 RED · 살아남은 술어 0 — 모든 술어가 뮤테이션-킬된다

----- 원본 무결성 확인 -----
원본 md5 (전체 실험 후) = 49ec1fcd798341ae7599dd3855babf11
기대(불변)              = 49ec1fcd798341ae7599dd3855babf11
원본 불변 여부: OK — 원본 절대 미수정 확인
````

### R-4 — M-REMOVE-NOOP 뮤턴트 (`/tmp/f14-r4-final.txt`)

`src/store/reconcile.rs`의 `recover_graves_from`에서 **recovery remove 분기를 무력화**하면 새 `phase_g`(lib)가 **RED**가 되는가. stale binary 배제를 위해 각 판정 직전 `cargo clean -p files` 후 재빌드했다. **뮤턴트 RED(exit 101) · 원복 GREEN(exit 0) · 원복 무결(md5·git diff).**

````text
############################################################
# R-4  M-REMOVE-NOOP — recovery remove 분기 무력화
#      새 phase_g(lib · recover_graves_production_seam.rs)를 죽이는가?
# HEAD = 9882d618e11498f8703896dd99b58865ced9e0bf
# src/store/reconcile.rs md5 (뮤테이션 전) = dd58c73a272051c2524e32982f077b0a
# stale binary 배제: 각 판정 직전 `cargo clean -p files` (files 크레이트 아티팩트 삭제) 후 재빌드.
############################################################

=== [1] 적용한 뮤턴트 (src/store/reconcile.rs · recover_graves_from · if blob_intact 분기) ===
----- 지운 원본 4줄 (remove 분기 본문) -----
            let Seen::Present(()) = e.remove().await? else {
                continue; // 무덤이 사라졌다 — 지울 것이 없다
            };
            atomic::fsync_dir(&objects).await?;
----- 대체(no-op 주석) -----
            // [M-REMOVE-NOOP] recovery remove 분기 무력화 — e.remove()/fsync_dir 를
            // 절대 호출하지 않는다. blob_intact 무덤은 그대로 남는다.
----- 뮤테이션 후 md5 = eb7a5ec9ec3e9bd2df92c205791994f7 (원본과 다름: True) -----
----- git diff (뮤턴트 적용 상태) -----
diff --git i/src/store/reconcile.rs w/src/store/reconcile.rs
index d364c86..c866916 100644
--- i/src/store/reconcile.rs
+++ w/src/store/reconcile.rs
@@ -145,10 +145,8 @@ async fn recover_graves_from<'v>(
             Ok(b) if hex::encode(Sha256::digest(&b)) == sha
         );
         if blob_intact {
-            let Seen::Present(()) = e.remove().await? else {
-                continue; // 무덤이 사라졌다 — 지울 것이 없다
-            };
-            atomic::fsync_dir(&objects).await?;
+            // [M-REMOVE-NOOP] recovery remove 분기 무력화 — e.remove()/fsync_dir 를
+            // 절대 호출하지 않는다. blob_intact 무덤은 그대로 남는다.
         } else {
             let Seen::Present(()) = e.rename_durable_to(&blob, &objects).await? else {
                 continue; // 무덤이 사라졌다 — 되돌릴 것이 없다

=== [2] 뮤턴트 아래 phase_g — `cargo clean -p files` 후 재빌드 판정 ===
$ cargo clean -p files   (exit=0)
$ cargo test --lib phase_g_recover_graves_survives_vanishing_graves
------------------------------------------------

running 1 test
test store::pins::tests::recover_graves_production_seam::phase_g_recover_graves_survives_vanishing_graves ... FAILED

failures:

---- store::pins::tests::recover_graves_production_seam::phase_g_recover_graves_survives_vanishing_graves stdout ----

thread 'store::pins::tests::recover_graves_production_seam::phase_g_recover_graves_survives_vanishing_graves' (24202750) panicked at src/store/pins/tests/recover_graves_production_seam.rs:354:9:
K_KEEP의 무덤이 남아 있다 — 우리는 건드리지 않았으므로 **프로덕션이 remove 분기를 타지 않았다**. sha=0c9bfadeab371ed1a0481b67c1c4814e3ac56a3492c5a5c488de6cfc77d4949b
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    store::pins::tests::recover_graves_production_seam::phase_g_recover_graves_survives_vanishing_graves

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 141 filtered out; finished in 0.24s

   Compiling files v0.1.0 (/Users/ukyi/workspace/files/.claude/worktrees/bugfix-reconcile-vanished-entry)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.30s
     Running unittests src/lib.rs (target/debug/deps/files-3a147c720753eeaf)
error: test failed, to rerun pass `--lib`
------------------------------------------------
[뮤턴트] exit code = 101  →  RED (뮤턴트를 죽였다 — 기대)

=== [3] 원복 무결성 ===
src/store/reconcile.rs md5 (원복 후) = dd58c73a272051c2524e32982f077b0a
기대(불변)                           = dd58c73a272051c2524e32982f077b0a
reconcile.rs 원복: OK — 뮤턴트 완전 제거 확인
----- git diff src/store/reconcile.rs (원복 후 — 비어 있어야 한다) -----
(빈 diff — 프로덕션 델타 0)

=== [4] 원복 트리 phase_g GREEN 재확인 — `cargo clean -p files` 후 재빌드 ===
$ cargo clean -p files   (exit=0)
$ cargo test --lib phase_g_recover_graves_survives_vanishing_graves
------------------------------------------------

running 1 test
test store::pins::tests::recover_graves_production_seam::phase_g_recover_graves_survives_vanishing_graves ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 141 filtered out; finished in 0.23s

   Compiling files v0.1.0 (/Users/ukyi/workspace/files/.claude/worktrees/bugfix-reconcile-vanished-entry)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.72s
     Running unittests src/lib.rs (target/debug/deps/files-3a147c720753eeaf)
------------------------------------------------
[원복] exit code = 0  →  GREEN (원복 트리에서 통과 — 기대)

=== 대조 요약 ===
M-REMOVE-NOOP 아래:  새 phase_g exit=101 (RED)
원복 트리:            새 phase_g exit=0 (GREEN)
결론: M-REMOVE-NOOP 는 새 phase_g 를 죽인다 · 원복 무결 — 커버리지 성립
````

### 최종 스위트 (`/tmp/f14-final-suite2.txt`)

최종 트리 clean 재빌드 후: `--selftest` PASS · 본 게이트 PASS(**172 passed · 0 failed · 0 ignored**) · 회귀 2 · 40-put repro · 1000-put stress · characterization 0 failed · build 경고 0. 전부 실행 원문.

````text
################################################################################
# F-14 최종 스위트 — 최종 트리 clean 재빌드 후 전 증거 (원문)
# HEAD = 9882d618e11498f8703896dd99b58865ced9e0bf
################################################################################

==============================================================================
== [0] clean 재빌드 (cargo clean -p files) + build 경고 (cargo build --all-targets)
$ cargo clean -p files   (exit=0)
$ cargo build --all-targets
------------------------------------------------------------------------------
   Compiling files v0.1.0 (/Users/ukyi/workspace/files/.claude/worktrees/bugfix-reconcile-vanished-entry)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.97s
------------------------------------------------------------------------------
exit code = 0  ·  'warning' 로 시작하는 줄 수 = 0

==============================================================================
== [1] 증인 게이트 --selftest
$ bash scripts/f14-witness-gate.sh --selftest
------------------------------------------------------------------------------
== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) ==
-- ② 결과 게이트 --
   [ok  ] (a) 1 ignored        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=FAIL (기대 FAIL)
   [ok  ] (b) 10 ignored       rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (c) 전부 정상    rc=0    게이트=PASS (기대 PASS)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (d) 10 failed        rc=0    게이트=FAIL (기대 FAIL)  · [ok  ] 옛 파서=PASS (기대 PASS)
   [ok  ] (e) cargo rc!=0      rc=101  게이트=FAIL (기대 FAIL)
   [ok  ] (f) 결과 줄 0개  rc=0    게이트=FAIL (기대 FAIL)
-- ① 발견 게이트 --
   [ok  ] (g) 증인 누락             발견=FAIL (기대 FAIL)
   [ok  ] (h) 조기매치+큰목록          발견=PASS (기대 PASS)
   [ok  ] (i) 목록 rc!=0              발견=FAIL (기대 FAIL)

SELFTEST: PASS  (9/9 · 케이스·술어의 정본 = §0-h 매트릭스)
------------------------------------------------------------------------------
exit code = 0

==============================================================================
== [2] 증인 게이트 (본 게이트 · cargo test --tests)
$ bash scripts/f14-witness-gate.sh
------------------------------------------------------------------------------
== ① 발견 단언  (타깃별 --list → **캐시 파일** · 종료상태 검사 · 앵커 = (^|::)<id>: test$) ==
   ok    [lib] reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot
   ok    [lib] reconcile_pass_control_without_vanishing_entries_is_green
   ok    [lib] reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot
   ok    [lib] reconcile_pass_control_without_a_vanishing_temp_is_green
   ok    [lib] objects_container_destroyed_mid_pass_still_fails_the_pass_and_publishes_nothing
   ok    [lib] container_destroyed_at_the_grave_rename_fails_the_pass_without_publishing_or_resurrecting
   ok    [lib] container_destroyed_then_recreated_at_the_grave_rename_completes_with_empty_stats
   ok    [lib] temp_only_container_destruction_still_fails_the_pass_and_publishes_nothing
   ok    [lib] container_guard_fires_after_the_loop_runs_to_completion
   ok    [lib] tail_destruction_without_any_vanished_entry_stays_ok_like_today
   ok    [lib] grave_source_vanished_during_park_lets_the_pass_finish
   ok    [lib] recover_graves_production_seam_survives_vanished_graves
   ok    [lib] phase_g_recover_graves_survives_vanishing_graves
   ok    [lib] seen_absorbs_only_confirmed_absence
   ok    [lib] every_fs_method_reports_gone_after_the_entry_vanishes
   ok    [lib] rename_with_absent_source_is_source_gone_and_counted
   ok    [lib] rename_with_missing_destination_propagates_raw_notfound
   ok    [lib] rename_ok_then_fsync_failure_propagates_raw
   ok    [lib] w_log_a_no_vanish_stream_is_identical
   ok    [lib] w_log_b_downstream_events_fire_after_the_pass_survives
   ok    [lib] w_log_c_skip_path_emits_no_event_at_any_level
   ok    [lib] w_log_d_every_reachable_skip_arm_is_silent
   ok    [lib] a_dangling_blob_symlink_still_aborts_the_pass_exactly_like_today
   ok    [lib] grave_rename_ok_then_fsync_eacces_propagates_raw
   ok    [lib] rename_with_dangling_source_symlink_is_done
   ok    [lib] absence_probe_eacces_is_not_absence
   ok    [e2e] dangling_temp_symlink_keeps_lstat_semantics
   ok    [e2e] blob_symlink_to_directory_propagates_isadirectory
   ok    [e2e] corrupt_dir_as_regular_file_propagates_enotdir
   ok    [e2e] corrupt_dir_as_dangling_symlink_propagates_raw_notfound
   ok    [e2e] symlinked_objects_dir_with_a_vanished_entry_completes
   ok    [e2e] symlinked_objects_dir_without_vanishing_is_unchanged
   skip  [e2e] non_utf8_temp_name_is_stat_and_unlinked_by_raw_bytes   (platform=linux · OS=Darwin)
   ok    [reconcile_vanishing_entries] phase_e_entry_loop_survives_vanishing_entries
   ok    [reconcile_vanishing_entries] phase_t_temp_deletion_counts_only_what_we_deleted
   -> DISCOVERY OK

== ② 결과 게이트  (전 스위트 실행 · 숫자 파싱 · ignored/failed/결과줄수/exit) ==
   test result: ok. 142 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.81s
   test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.24s
   test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
   test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.14s
   test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s
   test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.14s
   test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.04s
   test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.41s
   test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.15s
   결과 줄 11개 · passed=172 · failed=0 · ignored=0 · cargo exit=0
   -> 0 ignored · 0 failed · 스위트 GREEN

F-14 WITNESS GATE: PASS
------------------------------------------------------------------------------
exit code = 0

==============================================================================
== [3] 회귀 증인 2개 (lock regressionCmd)
$ cargo test --lib -- reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot
------------------------------------------------------------------------------

running 2 tests
test store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 140 filtered out; finished in 0.29s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.21s
     Running unittests src/lib.rs (target/debug/deps/files-3a147c720753eeaf)
------------------------------------------------------------------------------
exit code = 0

==============================================================================
== [4] 40-put 원 repro (lock reproCmd)
$ cargo test --test repro_concurrent_puts_reconcile
------------------------------------------------------------------------------

running 1 test
test original_repro_concurrent_puts_do_not_abort_the_reconcile_pass ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.47s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.05s
     Running tests/repro_concurrent_puts_reconcile.rs (target/debug/deps/repro_concurrent_puts_reconcile-9b6d00f181af3aeb)
------------------------------------------------------------------------------
exit code = 0

==============================================================================
== [5] 1000-put stress
$ cargo test --test stress_concurrent_puts_reconcile
------------------------------------------------------------------------------

running 1 test
test stress_concurrent_puts_do_not_abort_the_reconcile_pass ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.98s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running tests/stress_concurrent_puts_reconcile.rs (target/debug/deps/stress_concurrent_puts_reconcile-fb302ec366794dc9)
------------------------------------------------------------------------------
exit code = 0

==============================================================================
== [6] characterization (lock characterizationCmd)
$ cargo test --lib --bins --test adversarial --test contract --test e2e --test layout_tree --test openapi --test regression_reconcile_gc_dedup_race -- --skip reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot --skip reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot
------------------------------------------------------------------------------

running 140 tests
test config::tests::parses_defaults_with_required ... ok
test capacity::tests::reservation_accounting_and_raii_release ... ok
test capacity::tests::overcommit_prevented_then_freed ... ok
test capacity::tests::rejects_when_would_breach_min_free ... ok
test auth::tests::malformed_keys_file_errors ... ok
test config::tests::missing_required_errors ... ok
test capacity::tests::free_bytes_reports_positive_for_real_dir ... ok
test config::tests::validate_requires_upload_timeout_below_grace ... ok
test error::tests::status_mapping ... ok
test error::tests::code_mapping ... ok
test auth::tests::missing_scopes_default_to_empty ... ok
test auth::tests::camelcase_fixture_scoped_read_write ... ok
test clock::tests::now_is_parseable_rfc3339 ... ok
test http::internal::tests::healthz_ok ... ok
test http::internal::tests::create_reserved_bucket_400 ... ok
test http::internal::tests::put_without_write_scope_403 ... ok
test http::internal::tests::create_bucket_non_admin_403 ... ok
test http::internal::tests::get_missing_404 ... ok
test http::internal::tests::readyz_503_when_unwritable ... ok
test http::public::tests::every_reserved_bucket_has_a_shadow_route ... ok
test http::public::tests::every_shadow_route_names_a_reserved_bucket ... ok
test http::internal::tests::readyz_ok_when_writable ... ok
test http::internal::tests::create_bucket_admin_then_list ... ok
test http::internal::tests::head_returns_metadata_headers ... ok
test http::internal::tests::delete_then_get_404 ... ok
test http::public::tests::reserved_bucket_names_cannot_be_created ... ok
test http::internal::tests::put_creates_201_then_get_roundtrip ... ok
test http::ranged::tests::if_none_match_304 ... ok
test http::ranged::tests::full_200_with_etag_and_length ... ok
test http::internal::tests::list_files_returns_entries ... ok
test http::ranged::tests::open_ended_range_206 ... ok
test http::ranged::tests::partial_206_closed_range ... ok
test http::ranged::tests::suffix_range_206 ... ok
test http::tests::bad_bearer_is_401 ... ok
test http::ranged::tests::unknown_unit_ignored_full_200 ... ok
test http::tests::good_bearer_is_200 ... ok
test http::tests::missing_bearer_is_401 ... ok
test layout::tests::bucket_rules ... ok
test layout::tests::classify_objects_entry_table ... ok
test layout::tests::grave_name_round_trips ... ok
test layout::tests::hidden_and_control_chars_rejected ... ok
test layout::tests::making_methods_author_expected_paths ... ok
test layout::tests::meta_path_appends_suffix ... ok
test http::ranged::tests::unsatisfiable_416 ... ok
test layout::tests::reserved_suffixes_rejected ... ok
test layout::tests::safe_object_path_stays_under_root ... ok
test layout::tests::temp_name_authors_prefix ... ok
test layout::tests::traversal_and_malformed_keys_rejected ... ok
test layout::tests::valid_keys_accepted ... ok
test layout::tests::walker_rejects_reserved_bucket_and_empty_on_absent ... ok
test http::tests::build_state_creates_objects_dir_and_loads_keys ... ok
test http::public::tests::internal_bucket_not_served_publicly_404 ... ok
test meta::tests::bucket_meta_roundtrip_camel_case ... ok
test meta::tests::object_meta_roundtrip_camel_case ... ok
test meta::tests::visibility_lowercase ... ok
test http::public::tests::no_method_reaches_api_surface_on_public ... ok
test layout::tests::walker_round_trips_meta_for ... ok
test http::public::tests::catalog_lists_public_only ... ok
test store::atomic::tests::write_atomic_is_cancellable_before_rename ... ok
test http::public::tests::missing_object_404 ... ok
test store::locks::tests::bucket_participates_in_lock_key ... ok
test store::locks::tests::busy_while_held_free_after_drop ... ok
test store::locks::tests::different_keys_independent ... ok
test http::public::tests::public_api_path_404 ... ok
test layout::tests::pointers_all_skips_objects_and_covers_buckets ... ok
test store::locks::tests::lock_serializes_same_key ... ok
test store::atomic::tests::mkdir_p_durable_creates_nested_idempotent ... ok
test http::public::tests::public_download_200_with_security_headers ... ok
test store::atomic::tests::write_atomic_overwrites ... ok
test store::pins::tests::drop_paths_survive_a_poisoned_registry_mutex ... ok
test store::atomic::tests::write_atomic_roundtrip_no_temp_residue ... ok
test layout::tests::walker_yields_exactly_commit_pointers ... ok
test http::public::tests::reserved_route_shape_asymmetry_is_load_bearing ... ok
test store::pins::tests::commit_pointer_lands_and_releases_pin ... ok
test store::pins::tests::hooks_fire_on_production_put_path ... ok
test store::pins::tests::landed_trace_only_when_rename_returns_ok ... ok
test store::pins::tests::already_landed_at_grave_time_restores_without_waiting_for_the_cohort ... ok
test store::pins::tests::grave_planted_by_a_crashed_process_is_recovered_on_restart ... ok
test store::pins::tests::barrier_hooks_and_injected_clock_compose_in_one_witness ... ok
test store::pins::tests::leaked_graved_token_leaves_a_grave_that_the_next_pass_recovers ... ok
test store::pins::tests::log_witness::w_log_c_skip_path_emits_no_event_at_any_level ... ok
test store::pins::tests::failed_commit_does_not_protect_blob_from_gc ... ok
test store::pins::tests::pin_ids_are_monotonic_and_independent ... ok
test store::pins::tests::log_witness::w_log_a_no_vanish_stream_is_identical ... ok
test store::pins::tests::log_witness::w_log_b_downstream_events_fire_after_the_pass_survives ... ok
test store::pins::tests::commit_holds_key_lock_until_rename_lands ... ok
test store::pins::tests::pin_and_put_do_not_block_while_pass_is_live ... ok
test store::pins::tests::pass_cancelled_after_grave_leaves_it_for_the_next_pass_to_recover ... ok
test store::pins::tests::caller_cancellation_mid_commit_still_protects_the_blob ... ok
test store::pins::tests::put_landing_between_pre_grave_and_grave_is_protected ... ok
test store::pins::tests::landing_during_settle_wait_is_woken_by_the_landed_notification ... ok
test store::pins::tests::store_clone_shares_pin_registry_but_new_does_not ... ok
test store::pins::tests::stage_failure_leaves_no_landed_trace ... ok
test store::pins::tests::put_landing_during_reference_collection_is_protected ... ok
test store::pins::tests::vanished_container_witnesses::a_dangling_blob_symlink_still_aborts_the_pass_exactly_like_today ... ok
test store::pins::tests::restore_failure_keeps_the_grave_and_never_unlinks_it ... ok
test store::pins::tests::restore_failure_makes_the_reconcile_pass_return_the_raw_io_error ... ok
test store::pins::tests::vanished_container_witnesses::container_destroyed_at_the_grave_rename_fails_the_pass_without_publishing_or_resurrecting ... ok
test store::pins::tests::overlapping_failed_put_does_not_protect_the_blob ... ok
test store::pins::tests::vanished_container_witnesses::container_destroyed_then_recreated_at_the_grave_rename_completes_with_empty_stats ... ok
test store::pins::tests::vanished_container_witnesses::container_guard_fires_after_the_loop_runs_to_completion ... ok
test store::pins::tests::recover_graves_production_seam::recover_graves_production_seam_survives_vanished_graves ... ok
test store::pins::tests::vanished_container_witnesses::tail_destruction_without_any_vanished_entry_stays_ok_like_today ... ok
test store::pins::tests::vanished_container_witnesses::grave_rename_ok_then_fsync_eacces_propagates_raw ... ok
test store::pins::tests::vanished_container_witnesses::objects_container_destroyed_mid_pass_still_fails_the_pass_and_publishes_nothing ... ok
test store::pins::tests::recover_graves_production_seam::phase_g_recover_graves_survives_vanishing_graves ... ok
test store::pins::tests::put_parked_after_observe_forces_cohort_wait_then_restore ... ok
test store::reconcile::absence::tests::rename_with_absent_source_is_source_gone_and_counted ... ok
test store::reconcile::absence::tests::absence_probe_eacces_is_not_absence ... ok
test store::reconcile::absence::tests::rename_ok_then_fsync_failure_propagates_raw ... ok
test store::reconcile::absence::tests::rename_with_dangling_source_symlink_is_done ... ok
test store::reconcile::absence::tests::rename_with_missing_destination_propagates_raw_notfound ... ok
test store::pins::tests::vanished_container_witnesses::temp_only_container_destruction_still_fails_the_pass_and_publishes_nothing ... ok
test store::pins::tests::vanished_container_witnesses::grave_source_vanished_during_park_lets_the_pass_finish ... ok
test store::reconcile::entry::tests::seen_absorbs_only_confirmed_absence ... ok
test store::reconcile::entry::tests::every_fs_method_reports_gone_after_the_entry_vanishes ... ok
test store::pins::tests::log_witness::w_log_d_every_reachable_skip_arm_is_silent ... ok
test store::reconcile::tests::settle_timeout_derives_from_upload_timeout_and_is_monotonic ... ok
test store::reconcile::tests::old_temp_deleted_recent_preserved ... ok
test store::reconcile::tests::corrupt_blob_quarantined ... ok
test store::tests::bucket_meta_roundtrip ... ok
test store::reconcile::tests::recover_graves_skips_a_directory_that_is_named_like_a_grave ... ok
test store::reconcile::tests::referenced_nested_blob_survives ... ok
test store::reconcile::tests::unreferenced_old_blob_is_gced ... ok
test store::reconcile::tests::recover_graves_adopts_the_grave_when_the_canonical_blob_is_rotten ... ok
test store::tests::list_empty_bucket_is_ok ... ok
test store::reconcile::tests::unreferenced_recent_blob_preserved ... ok
test store::pins::tests::vanished_temp_regression::reconcile_pass_control_without_a_vanishing_temp_is_green ... ok
test store::tests::meta_pointing_to_missing_blob_is_not_found ... ok
test store::tests::delete_removes_pointer_idempotent ... ok
test store::tests::put_get_roundtrip_content_addressed ... ok
test store::tests::put_stream_too_large_no_residue_not_committed ... ok
test store::tests::put_stream_heals_corrupt_blob ... ok
test store::tests::list_buckets_returns_those_with_bucket_json ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_control_without_vanishing_entries_is_green ... ok
test store::tests::list_returns_serving_only_with_nested_keys ... ok
test store::tests::put_stream_roundtrip_large ... ok
test store::tests::same_size_overwrite_is_self_consistent ... ok
test store::pins::tests::wedged_commit_keeps_key_unwritable_and_says_so_loudly ... ok
test store::pins::tests::stuck_pin_defers_reclamation_but_never_stalls_the_pass ... ok

test result: ok. 140 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 1.81s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 8 tests
test reserved_suffix_keys_rejected_at_runtime ... ok
test query_key_decoding_and_validation_contract ... ok
test download_content_type_is_stored_type_and_206_has_all_headers ... ok
test upload_rejected_507_no_temp_residue_existing_intact ... ok
test internal_object_reads_are_no_store_and_vary_authorization ... ok
test concurrent_nested_puts_with_reconcile_loop_preserve_all ... ok
test concurrent_same_key_put_delete_self_consistent ... ok
test concurrent_readers_never_observe_desync_on_same_size_overwrite ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.22s


running 1 test
test responses_match_openapi_schema ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s


running 8 tests
test blob_symlink_to_directory_propagates_isadirectory ... ok
test corrupt_dir_as_regular_file_propagates_enotdir ... ok
test corrupt_dir_as_dangling_symlink_propagates_raw_notfound ... ok
test symlinked_objects_dir_without_vanishing_is_unchanged ... ok
test public_listener_isolates_api_and_internal_buckets ... ok
test symlinked_objects_dir_with_a_vanished_entry_completes ... ok
test large_object_streaming_put_and_range_download ... ok
test dangling_temp_symlink_keeps_lstat_semantics ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.15s


running 3 tests
test put_stream_midflight_temp_observed_and_preserved ... ok
test symlinked_commit_pointer_current_behavior ... ok
test on_disk_layout_golden_tree ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s


running 5 tests
test does_not_serve_interactive_docs_ui ... ok
test spec_download_declares_binary_range_and_key_grammar ... ok
test spec_binary_upload_and_internal_only ... ok
test serves_generated_openapi_spec_unauthenticated ... ok
test spec_object_ops_document_error_codes ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 1 test
test dedup_put_during_reconcile_window_must_not_lose_blob ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.15s

    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running unittests src/lib.rs (target/debug/deps/files-3a147c720753eeaf)
     Running unittests src/main.rs (target/debug/deps/files-7a02823de475fd3b)
     Running tests/adversarial.rs (target/debug/deps/adversarial-b12799f66ee29b6e)
     Running tests/contract.rs (target/debug/deps/contract-7ed6a4636fbe1c4a)
     Running tests/e2e.rs (target/debug/deps/e2e-af64729ab231b8f9)
     Running tests/layout_tree.rs (target/debug/deps/layout_tree-6edafbc7478934a0)
     Running tests/openapi.rs (target/debug/deps/openapi-462bf780856ae5b0)
     Running tests/regression_reconcile_gc_dedup_race.rs (target/debug/deps/regression_reconcile_gc_dedup_race-6a254c660e7a3e6b)
------------------------------------------------------------------------------
exit code = 0
````
