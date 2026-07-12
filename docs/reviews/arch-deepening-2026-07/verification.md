# Verification — arch-deepening-2026-07 (Layout 소유 모듈)

작성: 2026-07-12 · stage `verification` · 브랜치 `refactor-arch-deepening-2026-07`
검증 SHA: `bb7a73c` (A-3 정리 커밋 — 마지막 코드 변경; 이후는 pipeline 북키핑 커밋뿐)

**개정 이력**: release gate r1의 **R-1**(Must Fix, 인간 triage Accept)을 수용해 C1·C2·C4의
증거를 **명령 원문 + 셸이 기계 기록한 exit code**로 교체했다. 초판은 손으로 추린 경고
목록과 합성된 `EXIT: 0` 줄을 담고 있었다 — 그건 요약이지 증거가 아니며, 이 파이프라인의
machine-owns-GREEN 원칙상 증거로 인정될 수 없다. 게이트가 정확히 그 결함을 잡았다.

이 리팩터의 **불변식은 행위 보존**이다. 따라서 증명해야 할 claim은 gated-refactor
계약이 정한 것뿐이며, 계획서가 perf sanity 명령을 핀하지 않았으므로(성능 후보는
전부 F-7·F-16·F-18로 라우팅 — metric 선언 시 gated-perf) perf claim은 없다.

| # | Claim | 증명 명령 | 결과 |
|---|---|---|---|
| C1 | behavior lock의 **핀된 `testCmd`** 초록 | `cargo test` | ✅ exit 0 |
| C2 | **전체 스위트** 초록 | `cargo test` (이 저장소에서 lock testCmd = 전체 스위트) | ✅ 105 passed / 8 suites |
| C3 | **anti-cheat**: characterization 테스트 미약화·미삭제·미스킵 | `git diff c889f75..HEAD -- tests/` 외 | ✅ **tests/ 바이트 동일** |
| C4 | 변경 파일 **신규 clippy 경고 0** | `cargo clippy --all-targets` | ✅ exit 0, 경고 5건 전부 미변경 파일 |

---

## C1 · C2 — lock testCmd + 전체 스위트

lock(`characterization-lock.json`): `testCmd = "cargo test"`, `baselineSha = c889f75`,
`green = true`, `cases = 94`(baseline 시점).

아래는 **Cargo의 원문 출력**(컴파일 진행 라인만 제외)이며, exit code는 **셸이 기계
기록**한 값이다. 개별 테스트 이름이 전부 찍혀 있어 스위트 구성과 통과 여부가 원문에서
직접 읽힌다.

```
$ cargo test 2>&1; echo "MACHINE-CAPTURED EXIT CODE: $?"
     Running unittests src/lib.rs (target/debug/deps/files-3a147c720753eeaf)
running 86 tests
...
test layout::tests::walker_yields_exactly_commit_pointers ... ok
test http::public::tests::reserved_route_shape_asymmetry_is_load_bearing ... ok
test store::tests::list_empty_bucket_is_ok ... ok
test store::tests::bucket_meta_roundtrip ... ok
test store::reconcile::tests::corrupt_blob_quarantined ... ok
test store::reconcile::tests::old_temp_deleted_recent_preserved ... ok
test store::tests::meta_pointing_to_missing_blob_is_not_found ... ok
test store::reconcile::tests::unreferenced_old_blob_is_gced ... ok
test store::reconcile::tests::unreferenced_recent_blob_preserved ... ok
test store::tests::delete_removes_pointer_idempotent ... ok
test store::reconcile::tests::referenced_nested_blob_survives ... ok
test store::tests::put_get_roundtrip_content_addressed ... ok
test store::tests::put_stream_too_large_no_residue_not_committed ... ok
test store::tests::put_stream_heals_corrupt_blob ... ok
test store::tests::list_buckets_returns_those_with_bucket_json ... ok
test store::tests::put_stream_roundtrip_large ... ok
test store::tests::list_returns_serving_only_with_nested_keys ... ok
test store::tests::same_size_overwrite_is_self_consistent ... ok
test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.64s

     Running unittests src/main.rs (target/debug/deps/files-7a02823de475fd3b)
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/adversarial.rs (target/debug/deps/adversarial-b12799f66ee29b6e)
running 8 tests
test reserved_suffix_keys_rejected_at_runtime ... ok
test upload_rejected_507_no_temp_residue_existing_intact ... ok
test download_content_type_is_stored_type_and_206_has_all_headers ... ok
test query_key_decoding_and_validation_contract ... ok
test internal_object_reads_are_no_store_and_vary_authorization ... ok
test concurrent_nested_puts_with_reconcile_loop_preserve_all ... ok
test concurrent_same_key_put_delete_self_consistent ... ok
test concurrent_readers_never_observe_desync_on_same_size_overwrite ... ok
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.36s

     Running tests/contract.rs (target/debug/deps/contract-7ed6a4636fbe1c4a)
running 1 test
test responses_match_openapi_schema ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s

     Running tests/e2e.rs (target/debug/deps/e2e-af64729ab231b8f9)
running 2 tests
test public_listener_isolates_api_and_internal_buckets ... ok
test large_object_streaming_put_and_range_download ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.45s

     Running tests/layout_tree.rs (target/debug/deps/layout_tree-6edafbc7478934a0)
running 3 tests
test put_stream_midflight_temp_observed_and_preserved ... ok
test symlinked_commit_pointer_current_behavior ... ok
test on_disk_layout_golden_tree ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s

     Running tests/openapi.rs (target/debug/deps/openapi-462bf780856ae5b0)
running 5 tests
test does_not_serve_interactive_docs_ui ... ok
test serves_generated_openapi_spec_unauthenticated ... ok
test spec_object_ops_document_error_codes ... ok
test spec_download_declares_binary_range_and_key_grammar ... ok
test spec_binary_upload_and_internal_only ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

   Doc-tests files
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

MACHINE-CAPTURED EXIT CODE: 0
```

*(lib 스위트의 86개 중 위에는 마지막 18줄만 옮겼다 — `running 86 tests` 헤더와
`test result: ok. 86 passed` 꼬리가 원문에 그대로 있어 전수 통과가 확인된다. 나머지
7개 스위트는 개별 테스트 이름까지 전문이다.)*

**86 + 0 + 8 + 1 + 2 + 3 + 5 + 0 = 105 passed / 0 failed / 0 ignored / 8 suites.**

lock testCmd가 곧 전체 스위트이므로 C1과 C2는 같은 실행이 증명한다.

### characterization 앵커 3종 (plan gate P-2·P-3·P-4가 요구한 핀) — 위 원문에서 직접 확인

- `put_stream_midflight_temp_observed_and_preserved ... ok` ← **P-3**: 업로드 중 `.tmp-*` 관측 / grace 보존 / 에러 시 잔재 0
- `symlinked_commit_pointer_current_behavior ... ok` ← **P-2·P-4**: 심링크가 유일 포인터인 블롭(referenced:2, gc_pending:0)
- `on_disk_layout_golden_tree ... ok` ← **B3**: 온디스크 이름 규칙 골든 스냅샷

---

## C3 — anti-cheat: characterization 테스트 미약화 (핵심)

이 파이프라인의 hard rule: *"증분 후 스위트가 red면 증분을 고친다 — 테스트를 고치지
않는다. characterization 테스트 약화/삭제/스킵은 게이트의 anti-cheat Blocker."*

### ① `tests/`는 baseline 이후 **바이트 동일**

```
$ git diff --stat c889f75..HEAD -- tests/
(출력 없음 — 변경 0)
```

파일별 라인 수 대조:

| 파일 | baseline `c889f75` | HEAD |
|---|---|---|
| `tests/adversarial.rs` | 398 | 398 |
| `tests/contract.rs` | 99 | 99 |
| `tests/e2e.rs` | 169 | 169 |
| `tests/layout_tree.rs` | 220 | 220 |
| `tests/openapi.rs` | 193 | 193 |

lock이 핀한 characterization 스위트(골든 레이아웃 트리 · 심링크 커밋 포인터 ·
mid-stream temp 관측 · adversarial 동시성/400 modes · OpenAPI 계약)는 **단 한 줄도
수정되지 않았다.** 골든 값 재기록도 없다.

### ② 스킵된 테스트 0

```
$ grep -rn '#\[ignore\]' src tests --include='*.rs'
(없음)
```

### ③ 테스트 수는 **증가만** 했다 (94 → 105)

| 시점 | 통과 수 | 증감 사유 |
|---|---|---|
| baseline `c889f75` (lock `cases`) | 94 | — |
| R-1 이후 | 101 | layout seam 자체 단위 테스트(분류 테이블·round-trip·워커 tempdir) + path.rs 인라인 테스트 **단언 불변 축자 이주** |
| R-5 이후 | 102 | `bucket_participates_in_lock_key` — 리뷰가 찾은 뮤턴트 구멍(`lock_key`가 bucket을 무시해도 전 스위트 통과) 봉쇄 |
| R-6 이후 (HEAD) | 105 | 정합성 가드 2종(정·역방향) + `reserved_route_shape_asymmetry_is_load_bearing`(405 + `Allow` 핀) |

**감소·약화 0.** 추가된 11건은 전부 seam의 성질을 **더 강하게** 고정하며, 그중 3건은
"뮤턴트가 기존 스위트를 통과함"을 실증한 뒤 그 뮤턴트를 죽이는 것으로 검증됐다.

---

## C4 — 변경 파일 신규 clippy 경고 0

> **release gate r1 / R-1 수용 반영**: 이 블록은 원래 손으로 추린 경고 위치 목록과
> 합성된 `EXIT: 0` 줄을 담고 있었다 — 요약이지 증거가 아니다. 아래는 **Cargo/Clippy의
> 원문 stdout/stderr 전문**이며, exit code는 **셸이 기계 기록**한 값이다.
> (`touch src/lib.rs`로 캐시를 무효화해 전량 재-lint 강제.)

```
$ cargo clippy --all-targets 2>&1; echo "MACHINE-CAPTURED EXIT CODE: $?"
    Checking files v0.1.0 (/Users/ukyi/workspace/files/.claude/worktrees/refactor-arch-deepening-2026-07)
warning: this can be `std::io::Error::other(_)`
 --> src/capacity.rs:7:39
  |
7 |     let s = statvfs(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
  |                                       ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  |
  = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.93.0/index.html#io_other_error
  = note: `#[warn(clippy::io_other_error)]` on by default
help: use `std::io::Error::other`
  |
7 -     let s = statvfs(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
7 +     let s = statvfs(path).map_err(|e| std::io::Error::other(e))?;
  |

warning: this can be `std::io::Error::other(_)`
  --> src/error.rs:68:18
   |
68 |         let io = std::io::Error::new(std::io::ErrorKind::Other, "x");
   |                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.93.0/index.html#io_other_error
help: use `std::io::Error::other`
   |
68 -         let io = std::io::Error::new(std::io::ErrorKind::Other, "x");
68 +         let io = std::io::Error::other("x");
   |

warning: this can be `std::io::Error::other(_)`
  --> src/error.rs:82:18
   |
82 |         let io = std::io::Error::new(std::io::ErrorKind::Other, "x");
   |                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.93.0/index.html#io_other_error
help: use `std::io::Error::other`
   |
82 -         let io = std::io::Error::new(std::io::ErrorKind::Other, "x");
82 +         let io = std::io::Error::other("x");
   |

warning: this can be `std::io::Error::other(_)`
  --> src/http/ranged.rs:95:47
   |
95 |                       return AppError::Internal(std::io::Error::new(
   |  _______________________________________________^
96 | |                         std::io::ErrorKind::Other,
97 | |                         "seek failed",
98 | |                     ))
   | |_____________________^
   |
   = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.93.0/index.html#io_other_error
help: use `std::io::Error::other`
   |
95 ~                     return AppError::Internal(std::io::Error::other(
96 ~                         "seek failed",
   |

warning: the following explicit lifetimes could be elided: 'a
   --> src/http/ranged.rs:167:12
    |
167 |     fn hdr<'a>(resp: &'a Response, name: header::HeaderName) -> &'a str {
    |            ^^         ^^                                         ^^
    |
    = help: for further information visit https://rust-lang.github.io/rust-clippy/rust-1.93.0/index.html#needless_lifetimes
    = note: `#[warn(clippy::needless_lifetimes)]` on by default
help: elide the lifetimes
    |
167 -     fn hdr<'a>(resp: &'a Response, name: header::HeaderName) -> &'a str {
167 +     fn hdr(resp: &Response, name: header::HeaderName) -> &str {
    |

warning: `files` (lib) generated 2 warnings (run `cargo clippy --fix --lib -p files` to apply 2 suggestions)
warning: `files` (lib test) generated 5 warnings (2 duplicates) (run `cargo clippy --fix --lib -p files --tests` to apply 3 suggestions)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.34s
MACHINE-CAPTURED EXIT CODE: 0
```

**0 errors.** 경고 5건(중복 2건 포함해 lib test 기준 보고)의 발생 파일은 원문이 직접
가리킨다: `src/capacity.rs:7` · `src/error.rs:68` · `src/error.rs:82` ·
`src/http/ranged.rs:95` · `src/http/ranged.rs:167`.

이 리팩터가 **변경한 파일 목록**(`git diff --name-only c889f75..HEAD -- src/`):

```
src/http/internal/files.rs   src/http/public.rs      src/http/state.rs
src/layout.rs                src/lib.rs              src/path.rs (삭제)
src/store/atomic.rs          src/store/buckets.rs    src/store/listing.rs
src/store/locks.rs           src/store/mod.rs        src/store/objects.rs
src/store/reconcile.rs       src/store/tests.rs
```

경고 5건이 나온 파일(`capacity.rs` · `error.rs` · `ranged.rs`)은 **이 목록에 하나도
없다** — 전부 기존 코드의 기존 경고이며 이번 리팩터 범위 밖이다(계획서·증분 acceptance가
명시). **변경 파일 신규 경고 0**이 기계로 증명된다.

---

## 행위 보존 — 증분별 등가성 증거 요약

전체 상세는 각 `docs/increments/arch-deepening-2026-07/R-*.md`의 Result에 있다.

| 증분 | 행위 등가성의 핵심 증거 |
|---|---|
| R-1 | seam 기립. path.rs 인라인 테스트를 **단언 불변으로 축자 이주**(이동 ≠ 약화). structure gate r2 **approve, 0 findings** |
| R-2 | 온디스크 바이트 동등: `temp_name(u)` ≡ 기존 `format!(".tmp-{u}")`, `temp_blob_path` ≡ `root/.objects/.tmp-<u>`, `Layout::meta_for` ≡ 기존 합성식. `buckets.rs`의 `valid_bucket` 가드 제거가 등가임을 개별 검증(같은 에러·같은 시점·I/O 이전) |
| R-3 | B5 수용집합 등가성을 **적대적 이름 5,219개 전수 비교**로 증명(불일치 0). 순회 형태 동일(LIFO 스택·반복 중 dir push·이름 필터 전 `file_type()` 조회) → **첫 io 에러의 정체까지 보존** |
| R-4 | syscall 순서 보존: **O1**(Reserved는 `file_type` 조회 **전** continue — 예약 이름 무-stat) · **O2**(dir 스킵이 Temp/Blob 앞) · Temp>Blob 우선 · 대문자 hex의 Blob 분류 후 내용 검증 격리(B6, 정규화 없음) · 무가공 `io::Result` 전파(B7) |
| R-5 | 락 맵 키 문자열이 **바이트 동일**(같은 합성식·인자·순서 — 이동한 건 포맷의 **위치**뿐). guard 획득 위치·스코프·drop 시점 3곳 불변. B8 앵커 `tests/adversarial.rs` 무수정 green |
| R-6 | **11경로 × 7메서드 = 77셀** wire 실측 대조 → 전부 동일. 특히 `PUT/POST/DELETE/OPTIONS /healthz/foo`의 **405 + `Allow: GET,HEAD`**(폐기된 원안이 404로 뒤집었을 셀) 보존 |

**계획 개정 4건**(A-1 `Layout::root()` · A-2/A-3 `safe_object_path`·`meta_path` 가시성 ·
A-4 R-6 원안 폐기)은 전부 계획서 Review Decision Log에 근거와 함께 기록됐다. 특히
**A-4는 계획서의 전제가 거짓임을 wire 레벨 실측으로 밝혀낸 건**이며, 원안대로 진행했다면
10개 셀의 관측 행동이 바뀌었을 것이다.

---

## 판정

**4개 claim 전부 통과.** 행위 보존이 기계 증거로 성립하며, characterization 스위트는
baseline 이후 바이트 단위로 손대지 않았다. 모든 claim의 증거는 명령 원문과 셸이
기계 기록한 exit code로 뒷받침된다(요약·재구성 없음).
