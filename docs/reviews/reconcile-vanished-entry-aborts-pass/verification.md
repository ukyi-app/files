# Verification — F-14 `reconcile-vanished-entry-aborts-pass`

**machine-owns-GREEN.** 아래 판정은 `bugfix-status.mjs --verify-flip`이 `red.sha`와 `green.sha`를
**일회용 워크트리에 체크아웃해 테스트를 직접 재실행**한 결과다. 사람이 요약한 것이 아니라 **스크립트가
캡처한 원문**이며, 각 레코드는 **트리 sha로 키가 걸려** 있다(위조·재사용 불가).

## 락 (`bugfix-lock.json`)

```json
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
    "sha": "ac58bd7982d06e46f37cd4aa6a9c274d93bd8195"
  },
  "green": {
    "sha": "582d2b07dea15b6f3a0645e2f488352c27dd1a0c"
  }
}
```

## 주장 (gated-bugfix의 claims source = 스크립트의 재실행)

| # | 주장 | 명령 | 판정 |
|---|---|---|---|
| 1 | 회귀 증인 2개가 **red.sha에서 FAIL**하고 symptomToken `PASS ABORTED`를 낸다 | `regressionCmd` @ `ac58bd7` | **exit 101 · failed=True · symptomTokenPresent=True** ✅ |
| 2 | characterization이 **red.sha에서 GREEN** (두 번째 잠복 플립이 빨강 뒤에 숨지 않는다) | `characterizationCmd` @ `ac58bd7` | **exit 0 · green=True** ✅ |
| 3 | 회귀 증인 2개가 **green.sha에서 PASS** | `regressionCmd` @ `582d2b0` | **exit 0 · passed=True** ✅ |
| 4 | characterization이 **green.sha에서도 GREEN** (플립은 정확히 하나) | `characterizationCmd` @ `582d2b0` | **exit 0 · green=True** ✅ |

---

## RED @ `ac58bd7982d06e46f37cd4aa6a9c274d93bd8195` (red.sha = red-baseline)

트리 sha: `8696e17232ec54d355229dfdc3b65b17d208b0f8`

### regression — 스크립트가 캡처한 **원문 tail**

```
es:

---- store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot stdout ----

thread 'store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot' (21311675) panicked at src/store/pins/tests/vanished_temp_regression.rs:196:9:
PASS ABORTED — 스냅샷 이후 사라진 **temp**(.tmp-f14-temp-victim)를 만난 reconcile 패스가 그 항목을 **건너뛰지 않고** 패스 **전체**를 Err로 중단시켰다. 범인 `?`는 Temp 분기의 `let mtime = e.metadata().await?…`(나이 판정 **전에** stat한다). 이것이 프론트매터가 적은 **바로 그 증상**이다: 동시 `put_stream`이 `.tmp-<uniq>`를 최종 blob 이름으로 rename하면 스냅샷에 잡힌 temp가 사라진다 → 패스 전체 중단. err=Os { code: 2, kind: NotFound, message: "No such file or directory" } kind=NotFound
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot stdout ----

thread 'store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot' (21311674) panicked at src/store/pins/tests/vanished_entry_regression.rs:189:9:
PASS ABORTED — 스냅샷 이후 사라진 항목(victims=["02e8e4db0fb46bc832573124554faf3a24b05d4b4fe5d8e3e0a611ee6cd277aa", "e3dbdd09192f1cebd4185cf8ba31a68537920becf58c9d2c0bf81ab802c06b75"])을 만난 reconcile 패스가 그 항목을 **건너뛰지 않고** 패스 **전체**를 Err로 중단시켰다. 동시 쓰기(`atomic::write_atomic`의 `.tmp-<uniq>` → rename)가 있는 한 이것은 상시 발생한다. err=Os { code: 2, kind: NotFound, message: "No such file or directory" } kind=NotFound (NotFound = ENOENT: 범인 `?`는 reconcile.rs:199(Temp `metadata`) / :208(Blob `read`) — :192(`file_type`)는 DT_UNKNOWN FS에서의 잠복 범인)


failures:
    store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot
    store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 118 filtered out; finished in 0.28s
```

### characterization — 스크립트가 캡처한 **원문 tail**

```
measured; 2 filtered out; finished in 1.58s


running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s


running 8 tests
test reserved_suffix_keys_rejected_at_runtime ... ok
test upload_rejected_507_no_temp_residue_existing_intact ... ok
test download_content_type_is_stored_type_and_206_has_all_headers ... ok
test internal_object_reads_are_no_store_and_vary_authorization ... ok
test query_key_decoding_and_validation_contract ... ok
test concurrent_nested_puts_with_reconcile_loop_preserve_all ... ok
test concurrent_same_key_put_delete_self_consistent ... ok
test concurrent_readers_never_observe_desync_on_same_size_overwrite ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.27s


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

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s


running 5 tests
test does_not_serve_interactive_docs_ui ... ok
test spec_object_ops_document_error_codes ... ok
test serves_generated_openapi_spec_unauthenticated ... ok
test spec_binary_upload_and_internal_only ... ok
test spec_download_declares_binary_range_and_key_grammar ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 1 test
test dedup_put_during_reconcile_window_must_not_lose_blob ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.50s
```

## GREEN @ `582d2b07dea15b6f3a0645e2f488352c27dd1a0c` (green.sha)

트리 sha: `6a7bed4fcc449447d4e827cf21e7bb28aac2eadb`

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

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.30s


running 1 test
test responses_match_openapi_schema ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s


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
test serves_generated_openapi_spec_unauthenticated ... ok
test spec_download_declares_binary_range_and_key_grammar ... ok
test spec_object_ops_document_error_codes ... ok
test spec_binary_upload_and_internal_only ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 1 test
test dedup_put_during_reconcile_window_must_not_lose_blob ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.52s
```


---

## 추가 증거 — 증인 게이트 (acceptance 0단계)

`scripts/f14-witness-gate.sh`는 **선언된 증인이 컴파일·등록됐는지**를 스위트보다 **먼저** 검증한다.
(Rust는 `src/store/pins/tests/` 아래 파일을 자동 발견하지 않는다 — `mod` 한 줄만 빠져도 스위트가
**조용히 `0 failed`로 통과**하면서 증인이 증발한다. 실측: 131 → 128 passed, 경고 0.)

### `--selftest` (게이트가 자기 자신의 회귀 증인이다)

```
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
```

### 본 게이트 (green.sha 트리)

```
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
   ok    [reconcile_vanishing_entries] phase_g_recover_graves_survives_vanishing_graves
   ok    [reconcile_vanishing_entries] phase_t_temp_deletion_counts_only_what_we_deleted
   -> DISCOVERY OK

== ② 결과 게이트  (전 스위트 실행 · 숫자 파싱 · ignored/failed/결과줄수/exit) ==
   test result: ok. 141 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.88s
   test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.28s
   test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
   test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.15s
   test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s
   test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
   test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.78s
   test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.63s
   결과 줄 9개 · passed=170 · failed=0 · ignored=0 · cargo exit=0
   -> 0 ignored · 0 failed · 스위트 GREEN

F-14 WITNESS GATE: PASS
```

### `cargo build` (경고 0)

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.16s
```
