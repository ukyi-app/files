#!/usr/bin/env bash
# F-14 증인 게이트 — 증인 레지스트리의 **단일 권위**.
#   ① 발견 — 타깃별 `--list`를 **파일로 캐시** · **cargo 종료 상태 검사** · **캐시 파일에 직접** grep.
#   ② 결과 — 전 스위트를 돌려 `test result:` 줄에서 **숫자를 뽑아 정수 0과 비교**한다.
#   --selftest — **술어 하나당 케이스 하나**(직교) · **모든 술어에 "지우면 RED"를 실증**했다 (§0-h).
#
# ⚠⚠ **발견에 파이프라인 금지 (r24/P-38)**: `list_for | grep -qE` 는 `pipefail` 아래에서 **성공한 매치를
#     실패로 뒤집는다** — `grep -q` 가 첫 매치에 종료하면 상류 `cat` 이 **SIGPIPE**를 맞아 파이프라인이
#     **141**을 낸다 ⇒ **거짓 `MISSING WITNESS`**(실측: 첫 줄 매치 · 1.1 MB → **20/20** · 57 KB → **4/30**).
# ⚠  `cargo … --list` 의 **종료 상태를 본다** — 빌드가 깨지면 목록이 비고, 그것을 grep 하면 증인 전부가
#     `MISSING WITNESS` 로 나온다 = **오진**(진짜 원인은 빌드 실패다 — 실측).
# ⚠⚠ **부분문자열 매칭 금지 (r23/P-37)**: `grep -vc '0 ignored'` 는 `10 ignored` 를 통과시킨다.
# ⚠  exit code 하나만 믿지 마라 — cargo 는 ignored 가 있어도 **0으로 끝난다**(실측). 둘 다 본다.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

OS="$(uname -s)"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

# ── 정본 레지스트리 —  "<target>|<id>|<platform>" ────────────────────────────
#   target   : lib | <통합 테스트 바이너리 이름>      platform : all | unix | linux
WITNESSES=$(cat <<'REG'
lib|reconcile_pass_survives_an_entry_that_vanishes_after_the_snapshot|all
lib|reconcile_pass_control_without_vanishing_entries_is_green|all
lib|reconcile_pass_survives_a_temp_that_vanishes_after_the_snapshot|all
lib|reconcile_pass_control_without_a_vanishing_temp_is_green|all
lib|objects_container_destroyed_mid_pass_still_fails_the_pass_and_publishes_nothing|all
lib|container_destroyed_at_the_grave_rename_fails_the_pass_without_publishing_or_resurrecting|all
lib|container_destroyed_then_recreated_at_the_grave_rename_completes_with_empty_stats|all
lib|temp_only_container_destruction_still_fails_the_pass_and_publishes_nothing|all
lib|container_guard_fires_after_the_loop_runs_to_completion|all
lib|tail_destruction_without_any_vanished_entry_stays_ok_like_today|all
lib|grave_source_vanished_during_park_lets_the_pass_finish|all
lib|recover_graves_production_seam_survives_vanished_graves|all
lib|phase_g_recover_graves_survives_vanishing_graves|all
lib|seen_absorbs_only_confirmed_absence|all
lib|every_fs_method_reports_gone_after_the_entry_vanishes|all
lib|rename_with_absent_source_is_source_gone_and_counted|all
lib|rename_with_missing_destination_propagates_raw_notfound|all
lib|rename_ok_then_fsync_failure_propagates_raw|all
lib|w_log_a_no_vanish_stream_is_identical|all
lib|w_log_b_downstream_events_fire_after_the_pass_survives|all
lib|w_log_c_skip_path_emits_no_event_at_any_level|all
lib|w_log_d_every_reachable_skip_arm_is_silent|all
lib|a_dangling_blob_symlink_still_aborts_the_pass_exactly_like_today|unix
lib|grave_rename_ok_then_fsync_eacces_propagates_raw|unix
lib|rename_with_dangling_source_symlink_is_done|unix
lib|absence_probe_eacces_is_not_absence|unix
e2e|dangling_temp_symlink_keeps_lstat_semantics|unix
e2e|blob_symlink_to_directory_propagates_isadirectory|unix
e2e|corrupt_dir_as_regular_file_propagates_enotdir|unix
e2e|corrupt_dir_as_dangling_symlink_propagates_raw_notfound|unix
e2e|symlinked_objects_dir_with_a_vanished_entry_completes|unix
e2e|symlinked_objects_dir_without_vanishing_is_unchanged|unix
e2e|non_utf8_temp_name_is_stat_and_unlinked_by_raw_bytes|linux
reconcile_vanishing_entries|phase_e_entry_loop_survives_vanishing_entries|all
reconcile_vanishing_entries|phase_t_temp_deletion_counts_only_what_we_deleted|all
REG
)

# ── ① 발견 — **캐시 파일 · 종료 상태 검사 · 파이프 없음** (P-38) ─────────────
list_file() {                                  # $1 = target → 목록 **파일 경로**를 stdout에. 실패 = rc 2
  local t="$1" f="$TMP/list.$1" rc=0
  if [ ! -f "$TMP/ok.$1" ]; then
    if [ "$t" = "lib" ]; then cargo test --lib        -- --list > "$f" 2> "$TMP/err.$1"
    else                      cargo test --test "$t"  -- --list > "$f" 2> "$TMP/err.$1"
    fi
    rc=$?                                      # ⚠ **리스트 자체의 종료 상태**(빌드 실패 ≠ 증인 부재)
    [ "$rc" -ne 0 ] && { printf '%s\n' "$rc" > "$TMP/rc.$1"; return 2; }
    : > "$TMP/ok.$1"
  fi
  printf '%s\n' "$f"                           # ← 파이프가 아니라 **경로**를 넘긴다
}

has_witness() { grep -qE "(^|::)${2}: test\$" "$1"; }   # $1 = 목록 파일 · $2 = id  ⇒ **파이프 없음**

required() {                                   # $1 = platform
  case "$1" in
    all)   return 0 ;;
    unix)  case "$OS" in Darwin|Linux|FreeBSD) return 0 ;; esac ;;
    linux) [ "$OS" = "Linux" ] && return 0 ;;
  esac
  return 1
}

discover() {                                   # $1 = 목록-해결자 함수명 · stdin = 레지스트리 ⇒ 0/1
  local resolve="$1" bad=0 target id platform f
  while IFS='|' read -r target id platform; do
    [ -z "${target:-}" ] && continue
    if ! required "$platform"; then
      echo "   skip  [$target] $id   (platform=$platform · OS=$OS)"; continue
    fi
    if ! f="$("$resolve" "$target")"; then     # ← PRED-LIST-RC: 목록 명령이 죽으면 오진하지 않는다
      echo "   LIST FAILED  [$target]  cargo --list exit=$(cat "$TMP/rc.$target" 2>/dev/null)"
      echo "                (빌드 실패다. '증인 없음'이 아니다 — 오진 금지)"
      bad=1; continue
    fi
    if has_witness "$f" "$id"; then            # ← PRED-DISC
      echo "   ok    [$target] $id"
    else
      echo "   MISSING WITNESS  [$target] $id"; bad=1
    fi
  done
  return "$bad"
}

# ── ② 파서 — **숫자를 뽑아 정수 비교한다** ───────────────────────────────────
#   원문:  test result: ok. 132 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.83s
tally() {                                      # $1 = 스위트 출력 → "<결과줄수> <passed> <failed> <ignored>"
  awk '/^test result:/ {
         n++
         for (i = 1; i < NF; i++) {
           if ($(i+1) ~ /^passed;?$/)  p += $i + 0
           if ($(i+1) ~ /^failed;?$/)  f += $i + 0
           if ($(i+1) ~ /^ignored;?$/) g += $i + 0
         }
       }
       END { printf "%d %d %d %d\n", n+0, p+0, f+0, g+0 }' "$1"
}

verdict() {                                    # $1 = 출력 파일 · $2 = cargo exit  →  0 PASS / 1 FAIL
  local n p f g bad=0
  read -r n p f g < <(tally "$1")
  echo "   결과 줄 ${n}개 · passed=${p} · failed=${f} · ignored=${g} · cargo exit=${2}"
  if [ "$n" -eq 0 ]; then echo "   FAIL: 'test result:' 줄이 0개 — 스위트가 돌지 않았다"; bad=1; fi
  if [ "$g" -ne 0 ]; then echo "   FAIL: ignored=${g} (≠0) — 스킵된 red = 위조된 red (하드룰 9)"; bad=1; fi
  if [ "$f" -ne 0 ]; then echo "   FAIL: failed=${f} (≠0)"; bad=1; fi
  if [ "$2" -ne 0 ]; then echo "   FAIL: cargo exit=${2} (≠0)"; bad=1; fi
  return "$bad"
}

old_parser() {                                 # r22/r23 판본 재현 — **회귀 핀**(P-37). 0 PASS / 1 FAIL
  local bad; bad=$(grep 'test result:' "$1" | grep -vc '0 ignored')
  [ "$bad" -eq 0 ]
}

# ── --selftest — **술어 하나당 케이스 하나**(직교 · r24/P-39) ────────────────
#   ⚠⚠ 픽스처 = **(출력, rc) 쌍**이다. cargo 종료코드를 **합성 파라미터**로 분리하지 않으면
#      (d)·(f)가 **rc≠0으로도** 실패해 `failed`·결과줄수 술어가 **핀되지 않는다**(P-39 · 실측).
selftest() {
  local rc=0 n_cases=0 n_ok=0        # ⚠ **케이스 수는 세지 않고 *센다*** — 아래 res/dis 호출이 정본이다(P-40)
  local FX_0="test result: ok. 132 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.83s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s"
  local FX_1="test result: ok. 131 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 1.78s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s"
  local FX_10="test result: ok. 122 passed; 0 failed; 10 ignored; 0 measured; 0 filtered out; finished in 1.75s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s"
  local FX_F10="test result: FAILED. 122 passed; 10 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.90s"
  local FX_NONE="error: could not compile \`files\` (lib test) due to 1 previous error"

  res() {              # 결과 케이스. $1 이름 · $2 출력 · $3 **합성 cargo rc** · $4 기대 · $5 옛-파서 기대
    n_cases=$((n_cases + 1)); local ok=1
    printf '%s\n' "$2" > "$TMP/fx"
    verdict "$TMP/fx" "$3" > "$TMP/out" 2>&1; local st=$?
    local got=PASS; [ "$st" -ne 0 ] && got=FAIL
    local mark="ok  "; [ "$got" != "$4" ] && { mark="FAIL"; rc=1; ok=0; }
    printf '   [%s] %-20s rc=%-3s  게이트=%-4s (기대 %s)' "$mark" "$1" "$3" "$got" "$4"
    if [ -n "${5:-}" ]; then
      local ogot=PASS; old_parser "$TMP/fx" || ogot=FAIL
      local omark="ok  "; [ "$ogot" != "$5" ] && { omark="FAIL"; rc=1; ok=0; }
      printf '  · [%s] 옛 파서=%-4s (기대 %s)' "$omark" "$ogot" "$5"
    fi
    printf '\n'
    [ "$ok" -eq 1 ] && n_ok=$((n_ok + 1))
    return 0
  }

  FX_LIST_RC=0
  fx_list() {          # 목록-해결자의 **테스트 대역**(cargo 미호출). $1 = target
    [ "$FX_LIST_RC" -ne 0 ] && { printf '%s\n' "$FX_LIST_RC" > "$TMP/rc.$1"; return 2; }
    printf '%s\n' "$TMP/fxlist.$1"
  }
  dis() {              # 발견 케이스. $1 이름 · $2 레지스트리 1행 · $3 기대
    n_cases=$((n_cases + 1))
    discover fx_list <<< "$2" > "$TMP/dout" 2>&1; local st=$?
    local got=PASS; [ "$st" -ne 0 ] && got=FAIL
    local mark="ok  "; if [ "$got" != "$3" ]; then mark="FAIL"; rc=1; else n_ok=$((n_ok + 1)); fi
    printf '   [%s] %-20s          발견=%-4s (기대 %s)\n' "$mark" "$1" "$got" "$3"
  }

  echo "== --selftest — 술어 × 케이스 (직교: 케이스 하나가 술어 하나만 죽인다) =="
  echo "-- ② 결과 게이트 --"
  res "(a) 1 ignored"      "$FX_1"    0   FAIL FAIL   # → PRED-IGN
  res "(b) 10 ignored"     "$FX_10"   0   FAIL PASS   # → PRED-IGN  + 옛-파서 회귀 핀(P-37)
  res "(c) 전부 정상"      "$FX_0"    0   PASS PASS   # → 대조군: 어떤 술어도 발화하지 않는다
  res "(d) 10 failed"      "$FX_F10"  0   FAIL PASS   # → PRED-FAIL ⚠ **rc=0**(r23은 101 — P-39)
  res "(e) cargo rc!=0"    "$FX_0"    101 FAIL        # → PRED-RC   nonzero-exit **전용** 증인
  res "(f) 결과 줄 0개"    "$FX_NONE" 0   FAIL        # → PRED-N0   ⚠ **rc=0**(r23은 101 — P-39)

  echo "-- ① 발견 게이트 --"
  printf 'store::pins::tests::log_witness::w_log_a_no_vanish_stream_is_identical: test\n' > "$TMP/fxlist.lib"
  dis "(g) 증인 누락"      "lib|w_log_d_every_reachable_skip_arm_is_silent|all"    FAIL   # → PRED-DISC
  { printf 'store::pins::tests::log_witness::w_log_a_no_vanish_stream_is_identical: test\n'
    awk 'BEGIN{for(i=0;i<20000;i++) printf "store::pins::tests::filler::padding_%06d: test\n", i}'
  } > "$TMP/fxlist.big"                              # 1.1 MB · **첫 줄이 매치** ⇒ SIGPIPE 무대
  dis "(h) 조기매치+큰목록" "big|w_log_a_no_vanish_stream_is_identical|all"        PASS   # → M-SIGPIPE 킬러
  FX_LIST_RC=101
  dis "(i) 목록 rc!=0"     "lib|w_log_a_no_vanish_stream_is_identical|all"         FAIL   # → PRED-LIST-RC
  FX_LIST_RC=0

  echo
  # ⚠ **숫자를 박지 않는다** — 위 res/dis 호출에서 **센다**(케이스를 더하면 분모가 저절로 는다 · P-40).
  if [ "$rc" -eq 0 ]; then echo "SELFTEST: PASS  (${n_ok}/${n_cases} · 케이스·술어의 정본 = §0-h 매트릭스)"
  else                     echo "SELFTEST: FAIL  (${n_ok}/${n_cases})"; fi
  return "$rc"
}

[ "${1:-}" = "--selftest" ] && { selftest; exit $?; }

echo "== ① 발견 단언  (타깃별 --list → **캐시 파일** · 종료상태 검사 · 앵커 = (^|::)<id>: test\$) =="
if ! discover list_file <<< "$WITNESSES"; then
  echo; echo "DISCOVERY FAILED — 선언된 증인이 그 타깃의 바이너리에 없다(또는 목록 명령이 죽었다)."
  echo "  원인: mod 등록 누락(M-NOMOD) · 파일 미작성(M-NOMOD') · 개명 · cfg 축출 · **빌드 실패**."
  exit 1
fi
echo "   -> DISCOVERY OK"

echo
echo "== ② 결과 게이트  (전 스위트 실행 · 숫자 파싱 · ignored/failed/결과줄수/exit) =="
cargo test --tests > "$TMP/suite.txt" 2>&1     # ⚠ --tests 는 lib·bins·통합을 **전부** 포함한다
suite_rc=$?
grep '^test result:' "$TMP/suite.txt" | sed 's/^/   /'
if ! verdict "$TMP/suite.txt" "$suite_rc"; then
  echo; echo "RESULT GATE FAILED — 실행 결과의 **숫자**를 판다(소스 grep이 아니다)"
  echo "  ⇒ #[ignore] · #[cfg_attr(…, ignore)] · 매크로 판본을 표기와 무관하게 전부 잡는다."
  exit 1
fi
echo "   -> 0 ignored · 0 failed · 스위트 GREEN"
echo; echo "F-14 WITNESS GATE: PASS"
