# Verification — F-14 `reconcile-vanished-entry-aborts-pass`

**machine-owns-GREEN.** 아래 판정은 `bugfix-status.mjs --verify-flip`이 `red.sha`와 `green.sha`를
**일회용 워크트리에 체크아웃해 테스트를 직접 재실행**한 결과다. **사람이 요약한 것이 아니라 스크립트가
캡처한 원문**이며, 각 레코드는 **트리 sha로 키가 걸려** 있다(재사용·위조 불가).

> **릴리스 게이트 r1의 R-1·R-2·R-3를 반영한 개정판이다.** R-2를 봉인하려고 **RED를 3차 재포착**했다 —
> 원 repro(`40 동시 put × reconcile 루프`)는 종전 baseline에서 **빨갛지 않았다**(`tests/adversarial.rs`의
> 그 안무가 `let _ = run_once(..)`로 결과를 버려 매 실행 버그를 밟으면서도 초록이었다). 그것을 **관측하는**
> 판본을 새 baseline에 커밋해 `reproCmd`로 선언했다.

## 락 (`bugfix-lock.json`)
```json
{
  "regressionCmd": "cargo test --lib -- reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot",
  "characterizationCmd": "cargo test --lib --bins --test adversarial --test contract --test e2e --test layout_tree --test openapi --test regression_reconcile_gc_dedup_race -- --skip reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot --skip reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot",
  "reproCmd": "cargo test --test repro_concurrent_puts_reconcile",
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
    "sha": "8131d252e7c21a25b5ddb6697290577444f0a96f"
  },
  "green": {
    "sha": "d866b368ff5af0c732e7f71bb521644959f2bf3e"
  }
}
```
## 주장과 판정

| # | 주장 | 판정 (스크립트의 재실행) |
|---|---|---|
| 1 | 회귀 증인 **2개**가 red.sha에서 **FAIL** + symptomToken `PASS ABORTED` | **exit 101 · failed=True · symptomTokenPresent=True** ✅ |
| 2 | characterization이 red.sha에서 **GREEN** (두 번째 잠복 플립이 빨강 뒤에 숨지 않는다) | **exit 0 · green=True** ✅ |
| 3 | **원 repro가 red.sha에서 실제로 재현된다** (R-2) | **exit 101 · reproduced=True** ✅ |
| 4 | 회귀 증인 2개가 green.sha에서 **PASS** | **exit 0 · passed=True** ✅ |
| 5 | characterization이 green.sha에서도 **GREEN** (플립은 정확히 하나) | **exit 0 · green=True** ✅ |
| 6 | **원 repro가 green.sha에서 사라졌다** (R-2) | **exit 0 · reproduces=False** ✅ |

---

## RED @ `8131d252e7c21a25b5ddb6697290577444f0a96f` (red.sha = red-baseline, 3차)

트리 sha: `e280d17be68f59d495f4d291d78b346a1c67a15f`

### regression — 스크립트가 캡처한 **원문 tail**

```
es:

---- store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot stdout ----

thread 'store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot' (22345230) panicked at src/store/pins/tests/vanished_temp_regression.rs:196:9:
PASS ABORTED — 스냅샷 이후 사라진 **temp**(.tmp-f14-temp-victim)를 만난 reconcile 패스가 그 항목을 **건너뛰지 않고** 패스 **전체**를 Err로 중단시켰다. 범인 `?`는 Temp 분기의 `let mtime = e.metadata().await?…`(나이 판정 **전에** stat한다). 이것이 프론트매터가 적은 **바로 그 증상**이다: 동시 `put_stream`이 `.tmp-<uniq>`를 최종 blob 이름으로 rename하면 스냅샷에 잡힌 temp가 사라진다 → 패스 전체 중단. err=Os { code: 2, kind: NotFound, message: "No such file or directory" } kind=NotFound
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot stdout ----

thread 'store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot' (22345229) panicked at src/store/pins/tests/vanished_entry_regression.rs:189:9:
PASS ABORTED — 스냅샷 이후 사라진 항목(victims=["02e8e4db0fb46bc832573124554faf3a24b05d4b4fe5d8e3e0a611ee6cd277aa", "e3dbdd09192f1cebd4185cf8ba31a68537920becf58c9d2c0bf81ab802c06b75"])을 만난 reconcile 패스가 그 항목을 **건너뛰지 않고** 패스 **전체**를 Err로 중단시켰다. 동시 쓰기(`atomic::write_atomic`의 `.tmp-<uniq>` → rename)가 있는 한 이것은 상시 발생한다. err=Os { code: 2, kind: NotFound, message: "No such file or directory" } kind=NotFound (NotFound = ENOENT: 범인 `?`는 reconcile.rs:199(Temp `metadata`) / :208(Blob `read`) — :192(`file_type`)는 DT_UNKNOWN FS에서의 잠복 범인)


failures:
    store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot
    store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 118 filtered out; finished in 0.28s
```

### characterization — 스크립트가 캡처한 **원문 tail**

```
measured; 2 filtered out; finished in 1.61s


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

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.29s


running 1 test
test responses_match_openapi_schema ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 2 tests
test public_listener_isolates_api_and_internal_buckets ... ok
test large_object_streaming_put_and_range_download ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.47s


running 3 tests
test put_stream_midflight_temp_observed_and_preserved ... ok
test symlinked_commit_pointer_current_behavior ... ok
test on_disk_layout_golden_tree ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s


running 5 tests
test does_not_serve_interactive_docs_ui ... ok
test spec_download_declares_binary_range_and_key_grammar ... ok
test spec_binary_upload_and_internal_only ... ok
test spec_object_ops_document_error_codes ... ok
test serves_generated_openapi_spec_unauthenticated ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 1 test
test dedup_put_during_reconcile_window_must_not_lose_blob ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.51s
```

### repro — 스크립트가 캡처한 **원문 tail**

```
   Compiling files v0.1.0 (/private/var/folders/dr/804s3_sj2m50rfbtkr6k1bsm0000gn/T/bugfix-verify-oxalbv)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.85s
     Running tests/repro_concurrent_puts_reconcile.rs (target/debug/deps/repro_concurrent_puts_reconcile-9b6d00f181af3aeb)
error: test failed, to rerun pass `--test repro_concurrent_puts_reconcile`

running 1 test
test original_repro_concurrent_puts_do_not_abort_the_reconcile_pass ... FAILED

failures:

---- original_repro_concurrent_puts_do_not_abort_the_reconcile_pass stdout ----
REPRO WITNESS puts=1000 passes=41 overlapped_passes=41 scans=125 temps_seen=2076 vanished=332 vanished_during_pass=332 put_temp_vanishes=291 pass_errs=34 pass_errs_notfound=34 first_err=[kind=NotFound err=Os { code: 2, kind: NotFound, message: "No such file or directory" }]

thread 'original_repro_concurrent_puts_do_not_abort_the_reconcile_pass' (22347529) panicked at tests/repro_concurrent_puts_reconcile.rs:287:5:
assertion `left == right` failed: PASS ABORTED — 동시 put과 경합하는 reconcile 패스가 `Err`로 중단됐다(34/41 패스, 그중 NotFound=34). 스냅샷 이후 사라진 `.objects` 항목(동시 `write_atomic`이 `.tmp-<uniq>` → `<sha>`로 rename해 치운 그 항목)은 **그 항목만 건너뛰고** 패스는 완주해야 한다(F-14). 첫 에러: kind=NotFound err=Os { code: 2, kind: NotFound, message: "No such file or directory" } — `NotFound`(ENOENT)가 곧 소멸의 물증이다: 패스 자신이 스냅샷에 잡아 둔 항목을 stat하다 밟았다. 관측자가 독립적으로 센 소멸도 332건이다(그중 패스 in-flight 중 332건 · put이 만든 temp의 소멸 하한 291건).
  left: 34
 right: 0
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    original_repro_concurrent_puts_do_not_abort_the_reconcile_pass

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.19s
```

## GREEN @ `d866b368ff5af0c732e7f71bb521644959f2bf3e` (green.sha)

트리 sha: `de8ee3f74f5b5a28cac6df265d131cc68124ab8e`

### regression — 스크립트가 캡처한 **원문 tail**

```

running 2 tests
test store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 139 filtered out; finished in 0.29s
```

### characterization — 스크립트가 캡처한 **원문 tail**

```
_stored_type_and_206_has_all_headers ... ok
test internal_object_reads_are_no_store_and_vary_authorization ... ok
test concurrent_nested_puts_with_reconcile_loop_preserve_all ... ok
test concurrent_same_key_put_delete_self_consistent ... ok
test concurrent_readers_never_observe_desync_on_same_size_overwrite ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.29s


running 1 test
test responses_match_openapi_schema ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


running 8 tests
test blob_symlink_to_directory_propagates_isadirectory ... ok
test corrupt_dir_as_dangling_symlink_propagates_raw_notfound ... ok
test corrupt_dir_as_regular_file_propagates_enotdir ... ok
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

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s


running 5 tests
test does_not_serve_interactive_docs_ui ... ok
test spec_binary_upload_and_internal_only ... ok
test spec_object_ops_document_error_codes ... ok
test spec_download_declares_binary_range_and_key_grammar ... ok
test serves_generated_openapi_spec_unauthenticated ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 1 test
test dedup_put_during_reconcile_window_must_not_lose_blob ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.50s
```

### repro — 스크립트가 캡처한 **원문 tail**

```

running 1 test
test original_repro_concurrent_puts_do_not_abort_the_reconcile_pass ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.15s
```


---

# 릴리스 게이트 r1 결함별 증거

## R-1 — release 프로파일 통제 (계획이 필수로 못박은 2줄)

```
════════════════════════════════════════════════════════════════════════════
 F-14 릴리스게이트 r1 · R-1 — acceptance의 **--release 2줄** (B-1 프로파일 편향 보상 통제)
 계획 docs/bugfixes/reconcile-vanished-entry-aborts-pass.md :2197-2199 의 명령 **그대로**
 tree: 7b67f33 + R-4 테스트 수정(프로덕션 델타 0 · md5 검증)
 host: Darwin 25.3.0 arm64   rustc: rustc 1.93.1 (01f6ddf75 2026-02-11)   date: 2026-07-15 02:57:07
════════════════════════════════════════════════════════════════════════════

── 계획이 요구하는 정확한 2줄 (계획 원문 인용 :2197-2199) ──────────────────
cargo test --release --test reconcile_vanishing_entries                      # B-1 보상 통제(프로파일 편향)
cargo test --release --lib -- reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot \
                             reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot

════════ [1/2] cargo test --release --test reconcile_vanishing_entries ════════
     Running tests/reconcile_vanishing_entries.rs (target/release/deps/reconcile_vanishing_entries-cdb69f2d3843238f)

running 3 tests
test phase_g_recover_graves_survives_vanishing_graves ... ok
test phase_t_temp_deletion_counts_only_what_we_deleted ... ok
test phase_e_entry_loop_survives_vanishing_entries ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.64s


>>> 종료코드 = 0

════════ [2/2] cargo test --release --lib -- \
                 reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot \
                 reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot ════════
     Running unittests src/lib.rs (target/release/deps/files-9325554f54819e92)

running 2 tests
test store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 139 filtered out; finished in 0.28s


>>> 종료코드 = 0

════════════════════════════════════════════════════════════════════════════
 결론: --release 2줄 **모두 실행 · 모두 exit 0 · 0 failed · 0 ignored**
       ⇒ B-1(프로파일 편향: cfg!(debug_assertions))의 보상 통제가 **실제로 관측되었다**.
════════════════════════════════════════════════════════════════════════════
```

## R-3 — 증인 게이트의 술어 뮤테이션 감사 (8/8 RED · 살아남은 술어 0)

게이트가 **자기 술어 하나가 약화된 뒤 다시 거짓 초록이 되지 않음**을 증명한다.

```
════════════════════════════════════════════════════════════════════════════════════
 F-14 릴리스게이트 r1 · R-3 — **게이트 술어 뮤테이션 킬 감사** (원문)
 대상: scripts/f14-witness-gate.sh --selftest
 술어: 계획 §0-h의 6개(DISC·LIST-RC·N0·IGN·FAIL·RC) + M-SIGPIPE + M-OLDPARSER = **8개**
 tree: 7b67f33 (+ R-4 테스트 수정)
 md5(원본 스크립트) = 5898670238661b81864cbbf5fe24eddc    date: 2026-07-15 03:04:37
════════════════════════════════════════════════════════════════════════════════════

  기대: **뮤턴트를 넣으면 selftest가 RED(exit≠0)** — 살아남으면(PASS) 그 술어는 **핀되지 않은 것**이다.
  ⚠ 뮤턴트는 **사본**에 가한다. 원본은 건드리지 않는다(끝에서 md5로 확인).

───────────────────────────────────────────────────────────────────────────────────
■ control
   무엇을 지웠나 : (없음 — 무뮤턴트 사본. 사본이 원본처럼 도는지 확인하는 대조군)
   뮤턴트 적용   : 예
   selftest 출력 :
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
   exit code     : 0
   판정          : PASS ✅ (대조군: 통과해야 한다)
───────────────────────────────────────────────────────────────────────────────────
■ M-PRED-IGN
   무엇을 지웠나 : verdict(): 'if [ "$g" -ne 0 ] … FAIL: ignored=…' **줄 삭제**
   뮤턴트 적용   : 예
   selftest 출력 :
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
   exit code     : 1
   판정          : RED  ✅ (죽었다)
───────────────────────────────────────────────────────────────────────────────────
■ M-PRED-FAIL
   무엇을 지웠나 : verdict(): 'if [ "$f" -ne 0 ] … FAIL: failed=…' **줄 삭제**
   뮤턴트 적용   : 예
   selftest 출력 :
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
   exit code     : 1
   판정          : RED  ✅ (죽었다)
───────────────────────────────────────────────────────────────────────────────────
■ M-PRED-N0
   무엇을 지웠나 : verdict(): 'if [ "$n" -eq 0 ] … test result: 줄이 0개' **줄 삭제**
   뮤턴트 적용   : 예
   selftest 출력 :
      == --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) ==
```

## R-4 — Phase G의 공허한 단언 봉인 (결정적 증거)

`M-REMOVE-NOOP`(remove 분기가 **아예 한 번도 안 돈다**)에서 **옛 증인은 exit 0으로 초록**이었다.

```
════════════════════════════════════════════════════════════════════════════════════
 F-14 릴리스게이트 r1 · R-4 — **고친 증인이 실제로 무언가를 증명하는가** (뮤턴트 실증 원문)
 대상 증인: tests/reconcile_vanishing_entries.rs :: phase_g_recover_graves_survives_vanishing_graves
 tree: 7b67f33 (+ R-4 테스트 수정 · **프로덕션 델타 0**)
 md5(src/store/reconcile.rs) 원본 = dd58c73a272051c2524e32982f077b0a    date: 2026-07-15 03:06:43
════════════════════════════════════════════════════════════════════════════════════

R-4가 지적한 공허함(세 겹):
  ① stepped_K = {k : 정본 존재 ∧ 무덤 부재} 로 remove-분기 커버리지를 주장했다.
     그런데 K의 정본은 아무도 안 지우고(항상 존재), 남은 무덤은 **테스트가 스스로 전부 지웠다**(항상 부재)
     ⇒ stepped_K ≡ K = 12 가 **프로덕션이 무엇을 했든 참** ⇒ 공허.
  ② grave_count == 0 도 같은 이유로 **항상 참**(우리가 전부 지웠다).
  ③ K의 무덤 내용을 **정본과 똑같이** 심었다 ⇒ rename으로 잘못 가는 뮤턴트가 **바이트-동일한** 정본을
     만들어 디스크에도 stats에도 흔적이 없다 ⇒ 분기 *선택*을 아무것도 핀하지 못했다.

수정: K를 **K_KILL(8: 우리가 무덤을 지운다 → killed_K)** + **K_KEEP(4: 절대 안 건드린다 → 프로덕션만이
      그 무덤을 없앨 수 있다)** 로 쪼개고, K 무덤 내용을 **쓰레기**로 심는다(rename되면 정본이 오염된다).

───────────────────────────────────────────────────────────────────────────────────
■ [M1] 뮤턴트 **M-REMOVE-RAW** (remove 분기를 픽스 이전의 raw `?`로 되돌린다)
      - let Seen::Present(()) = e.remove().await? else { continue; };
      + tokio::fs::remove_file(objects.join(e.name())).await?;
   무대: 정상(랑데부 후 K_KILL 무덤을 우리가 지운다)   기대: **RED**   증인: **고친 것**
      test phase_g_recover_graves_survives_vanishing_graves ... FAILED
      test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.01s
      thread 'phase_g_recover_graves_survives_vanishing_graves' (22322215) panicked at tests/reconcile_vanishing_entries.rs:358:14:
      PASS ABORTED — 스냅샷 이후 사라진 **무덤**이 패스를 중단시켰다(Phase G): Os { code: 2, kind: NotFound, message: "No such file or directory" }
      >>> cargo exit = 101
   판정: RED ✅ — 고친 증인이 뮤턴트를 **죽인다**.

   ⚠ **정직한 유보**: 이 무대에서는 **옛 증인도** 뮤턴트를 죽인다(창을 밟으면 .expect가 발화한다).
     ⇒ M1만으로는 'R-4 수정이 무언가를 벌었다'를 증명하지 못한다. **M2·M4가 그것을 증명한다.**

───────────────────────────────────────────────────────────────────────────────────
■ [M2] **공허함의 격리** — 뮤턴트 M-REMOVE-RAW는 그대로 두고, **K 무덤을 외부에서 지우지 않는다**
   (= 프로덕션이 K 경주를 전부 이긴 세계 = remove-분기 소멸 창을 **한 번도 밟지 않는다**).
   옛 기준(stepped_K)과 새 기준(killed_K)을 **같은 실행에서** 나란히 잰다.
   기대: 패스는 Ok로 완주(뮤턴트 **생존**) · 옛 기준은 ≥1 로 **초록**(= 공허) · 새 기준은 0 ⇒ **RED**
      round=0 패스=Ok(완주) escaped_R=11 옛_stepped_K=12 새_killed_K=0
      round=1 패스=Ok(완주) escaped_R=12 옛_stepped_K=12 새_killed_K=0
      round=2 패스=Ok(완주) escaped_R=11 옛_stepped_K=12 새_killed_K=0
      ────────────────────────────────────────────────────────────────
      패스 중단 라운드      : 0/3   (0 = **뮤턴트가 살아남았다**)
      escaped_R (rename창)  : 34
      옛 기준 stepped_K     : 36  → `>= 1` **통과** ⇒ "remove 분기 커버됨"이라 보고한다
      새 기준 killed_K      : 0  → `>= 1` **실패** ⇒ RED로 소리친다
      ────────────────────────────────────────────────────────────────
      ⇒ **뮤턴트는 살아남았는데 옛 기준은 초록이다** = R-4가 말한 **공허한 통과**. 실증됨.
      test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.19s
      >>> cargo exit = 0  (프로브 자체는 '뮤턴트 생존 + 옛기준 초록 + 새기준 0'을 단언하므로 통과 = 실증 성공)

───────────────────────────────────────────────────────────────────────────────────
■ [M3] 뮤턴트 **M-BRANCH-RENAME** (blob_intact 검사 무력화 → K도 **항상 rename 분기**)
      - if blob_intact {          + if false {
   K 무덤 내용이 **쓰레기**이므로 rename되면 정본이 오염된다 ⇒ 엔트리 루프가 **격리**한다.
   기대: **RED** (옛 증인은 K 무덤을 정본과 **똑같이** 심었으므로 이 뮤턴트를 **볼 수 없었다** — R-4 ③)
      test phase_g_recover_graves_survives_vanishing_graves ... FAILED
      test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.09s
      thread 'phase_g_recover_graves_survives_vanishing_graves' (22323220) panicked at tests/reconcile_vanishing_entries.rs:382:37:
      K의 정본은 살아남는다. round=0 sha=7bf240d68ba01d7cfc7972563fd777e2b67d6a2a11e05b2d638f0b44bfcd7ebb: No such file or directory (os error 2)
```

## 최종 트리 — 증인 게이트 · 스위트

```
════════════════════════════════════════════════════════════════════════════════
 F-14 릴리스게이트 r1 — 최종 검증 (필수 통과 1~4) · 2026-07-15 03:08:06
 tree: 7b67f33 + R-4 테스트 수정 (프로덕션 델타 0)
════════════════════════════════════════════════════════════════════════════════

── ① 증인 게이트 ───────────────────────────────────────────────────────────────
  $ bash scripts/f14-witness-gate.sh --selftest   → exit=0
     SELFTEST: PASS  (9/9 · 케이스·술어의 정본 = §0-h 매트릭스)
  $ bash scripts/f14-witness-gate.sh             → exit=0
        -> DISCOVERY OK
        결과 줄 10개 · passed=171 · failed=0 · ignored=0 · cargo exit=0
        -> 0 ignored · 0 failed · 스위트 GREEN
     F-14 WITNESS GATE: PASS

── ② 회귀 증인 2개 + 원 repro ──────────────────────────────────────────────────
  $ regressionCmd (lock 동결)                     → exit=0
     test store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot ... ok
     test store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot ... ok
     test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 139 filtered out; finished in 0.31s
  $ reproCmd: cargo test --test repro_concurrent_puts_reconcile → exit=0
     test original_repro_concurrent_puts_do_not_abort_the_reconcile_pass ... ok
     test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.14s

── ③ characterization + 전체 스위트 ────────────────────────────────────────────
  $ characterizationCmd (lock 동결)               → exit=0
     test result: ok. 139 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 1.81s
     test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.33s
     test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
     test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.14s
     test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s
     test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.49s
     ⇒ characterization: failed=0 · ignored=0
  $ cargo test (전체)                             → exit=0
     test result: ok. 141 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.84s
     test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.27s
     test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
     test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.15s
     test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s
     test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
     test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.10s
     test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.57s
     test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.31s
     test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     ⇒ 전체: passed=171 · **failed=0** · **ignored=0**

── ④ 빌드 경고 · clippy · fmt ──────────────────────────────────────────────────
  $ cargo build → 경고 0 개
  $ cargo clippy --all-targets → exit=0 · warning/error 10 개
  $ cargo fmt --check → exit=1 (⚠ 델타 있음)

════════════════════════════════════════════════════════════════════════════════
 종합
   ① selftest exit=0 · gate exit=0
   ② 회귀 exit=0 · repro exit=0
   ③ characterization exit=0 (failed=0) · 전체 exit=0 (failed=0 · ignored=0)
   ④ build 경고=0 · clippy exit=0 (10) · fmt exit=1
   ⇒ 합계 지표 = 11  (⚠ 확인 필요)
```
