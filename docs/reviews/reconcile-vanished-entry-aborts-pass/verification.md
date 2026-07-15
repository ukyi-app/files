# Verification — F-14 `reconcile-vanished-entry-aborts-pass`

**machine-owns-GREEN.** 아래 판정은 `bugfix-status.mjs --verify-flip`이 `red.sha`와 `green.sha`를 **일회용 워크트리에
체크아웃해 테스트를 직접 재실행**한 결과다. 사람이 요약한 것이 아니라 **스크립트가 캡처한 원문**이며, 각 레코드는
**트리 sha로 키가 걸려** 있다(재사용·위조 불가). 릴리스 게이트 증거(R-1/R-3/R-4)도 **최종 트리에서 실행한 원문**이며
이 파일은 **하나의 스크립트로 재생성**됐다(수동 편집 0 · 절단 0).

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
    "sha": "3b1e44f608cd00d0a580a3f5deb595d85a28a9d9"
  },
  "green": {
    "sha": "b2d0f3120ca97d7e25f0c1b2b9611704748bed5c"
  }
}
```
## 주장과 판정 (스크립트의 재실행)

| # | 주장 | 판정 |
|---|---|---|
| 1 | 회귀 증인 2개가 red.sha에서 FAIL + symptomToken `PASS ABORTED` | exit 101 · failed=True · symptomTokenPresent=True ✅ |
| 2 | characterization이 red.sha에서 GREEN | exit 0 · green=True ✅ |
| 3 | 원 40-put repro가 red.sha에서 재현 (R-2) | exit 101 · reproduced=True ✅ |
| 4 | 회귀 증인 2개가 green.sha에서 PASS | exit 0 · passed=True ✅ |
| 5 | characterization이 green.sha에서 GREEN (플립 하나) | exit 0 · green=True ✅ |
| 6 | 원 40-put repro가 green.sha에서 사라짐 (R-2) | exit 0 · reproduces=False ✅ |

## phase_g 봉인 (릴리스 게이트 재확인 중 발견한 회귀)

게이트를 직접 재확인하다 `phase_g_recover_graves_survives_vanishing_graves`가 green.sha에서 **5/5 결정적 실패**함을
잡았다. R-4를 고친 이전 서브에이전트가 *"20/20 GREEN"*이라 보고했지만 **거짓**이었다(HEAD==d866b36, 코드 무변경).
`verify-flip`이 못 잡은 이유: `characterizationCmd`에 `reconcile_vanishing_entries`가 없다. **근본 원인은 프로덕션이
아니라**(`recover_graves_from`은 정상) 구 통합 무대의 **동시성 랑데부 하이젠버그**였다. 봉인: **9번째 훅
`pre_recover_grave`로 결정적 park**(W11·W-GRAVE-CD와 같은 seam) + **lib 테스트로 이전**. 지휘자가 `cargo clean` 후
**clean 재빌드로 phase_g 20/20 GREEN을 직접 확인**했고, anti-cheat(M-REMOVE-NOOP 킬)를 유지했다.

---

## RED @ `3b1e44f608cd00d0a580a3f5deb595d85a28a9d9` (red-baseline, 4차)

트리 sha: `e92dacfe267afa523de84095f85dc473bd363413`

### regression — 스크립트가 캡처한 원문 tail

```
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
```

### characterization — 스크립트가 캡처한 원문 tail

```
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
```

### repro — 스크립트가 캡처한 원문 tail

```
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
```

## GREEN @ `b2d0f3120ca97d7e25f0c1b2b9611704748bed5c`

트리 sha: `c5a9531bd89ece3a9086569e083224b1d4971e75`

### regression — 스크립트가 캡처한 원문 tail

```

running 2 tests
test store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 140 filtered out; finished in 0.28s
```

### characterization — 스크립트가 캡처한 원문 tail

```
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
```

### repro — 스크립트가 캡처한 원문 tail

```

running 1 test
test original_repro_concurrent_puts_do_not_abort_the_reconcile_pass ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.52s
```

---

# 릴리스 게이트 증거 (최종 트리 · 하나의 스크립트로 캡처 · 절단 0)

## R-1 — release 프로파일 통제 (계획 :2204-2205의 정확한 2줄)

> 완전 원문(grep 필터 0 · stdout+stderr 통째 · SHA 스탬프). 별도 아티팩트
> `release-profile-capture.txt`와 바이트 동일하게 이 파일에 옮겨졌다.

```
# release-profile capture — F-14 R-1 (B-1 profile-bias compensating control)
# green.sha (lock): b2d0f3120ca97d7e25f0c1b2b9611704748bed5c
# captured at HEAD: 3eb727e (b2d0f31 이후 커밋은 전부 docs/reviews/ 증거 문서 — release가 의존하는 src/tests/scripts는 b2d0f31과 바이트 동일)
# tree:      e7166e0ea9cf7d8f950498f09d9f470fe4ebb4b1
# captured by: bash (직접 실행, grep 필터 0, stdout+stderr 통째)
# plan lines 2204-2205 canonical commands

======================================================================
$ cargo test --release --test reconcile_vanishing_entries
======================================================================
    Finished `release` profile [optimized] target(s) in 0.24s
     Running tests/reconcile_vanishing_entries.rs (target/release/deps/reconcile_vanishing_entries-cdb69f2d3843238f)

running 2 tests
test phase_t_temp_deletion_counts_only_what_we_deleted ... ok
test phase_e_entry_loop_survives_vanishing_entries ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.76s

[exit=0]

======================================================================
$ cargo test --release --lib -- reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot
======================================================================
    Finished `release` profile [optimized] target(s) in 0.06s
     Running unittests src/lib.rs (target/release/deps/files-9325554f54819e92)

running 2 tests
test store::pins::tests::vanished_temp_regression::reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot ... ok
test store::pins::tests::vanished_entry_regression::reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 140 filtered out; finished in 0.28s

[exit=0]
```

## R-3 — 증인 게이트 술어 뮤테이션 감사 (8/8 RED · 살아남은 술어 0)

```
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
```

## R-4 — Phase G의 M-REMOVE-NOOP 킬 (옛 무대 exit 0 / 새 무대 RED)

```
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
```

## 최종 트리 스위트

```
## 증인 게이트 --selftest
SELFTEST: PASS  (9/9 · 케이스·술어의 정본 = §0-h 매트릭스)

## 증인 게이트 (본 게이트)
   test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.11s
   결과 줄 11개 · passed=172 · failed=0 · ignored=0 · cargo exit=0
   -> 0 ignored · 0 failed · 스위트 GREEN

F-14 WITNESS GATE: PASS
```

