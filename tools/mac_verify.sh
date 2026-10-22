#!/usr/bin/env bash
# Collects a real-Mac verification bundle for CleanManager (see docs/platforms/macos-verification.md).
#
# Read-only for your data: tests write only under $TMPDIR, the CLI smoke uses a throwaway store and a fixture tree
# it creates, and nothing is deleted outside those. With --device-tests some tests move their own fixture files to
# your real Trash and take them out again. Results go to target/mac-verify/<timestamp>/ (git-ignored):
# summary.md and run-record.json are redacted and meant to be pasted publicly; every raw *.log is kept unredacted.
#
# Usage: tools/mac_verify.sh [options]
#   --quick              skip the full workspace test and clippy runs (CLI smoke only)
#   --scan-dir DIR       additionally run a read-only scan of DIR (e.g. ~/Downloads); results stay local
#   --desktop            also build and test the desktop web UI and the Tauri host, build the desktop app
#                        (debug, no bundle) and launch it for a 20 s smoke
#   --device-tests       set CM_ALLOW_MAC_DEVICE_TESTS=1 for the workspace test step only
#   --step-timeout SECS  limit for every CLI and desktop step (default 300)
#   --build-timeout SECS limit for every build and test step: cargo, npm (default 3600)
#   --self-check         static sanity checks of this script (runs on any OS), then exit
#   -h, --help           show this help
set -u

# Read-only command groups, run after experimental features are turned on in the throwaway store. Each must exit 0,
# or 5 (partial: part of the answer is unknown, for example while macOS shows a privacy prompt).
ro_groups=(
  "clean list" "purge roots list" "installer locations" "insights overview" "apps list" "startup list"
  "optimize list" "status --count 1" "process list --limit 20" "apps updates" "apps uninstall list"
  "apps uninstall orphans" "optimize diagnose --window 2" "trash list" "recovery list"
)

tab="$(printf '\t')"
nl="$(printf '\n_')"
nl="${nl%_}"
bounded_rc=0
bounded_secs=0
bounded_timed_out=0
desktop_window="not run (--desktop not given)"

usage() { sed -n '2,/^set -u$/p' "$0" | sed -e '/^set -u$/d' -e 's/^# \{0,1\}//'; }
die() { echo "mac_verify: $*" >&2; exit 2; }
is_uint() {
  case "$1" in ''|*[!0-9]*) return 1 ;; esac
  [ "$1" -gt 0 ]
}

# group_step_name "process list --limit 20" -> cli-process-list (options and their values are left out).
group_step_name() {
  local w name=cli
  for w in $1; do
    case "$w" in -*) break ;; esac
    name="$name-$w"
  done
  printf '%s\n' "$name"
}

# --- self-check: static sanity with no external tools (bash, awk, grep, sed, sort only) ---------------------------
# The pattern lists are here-document bodies, which the checks skip, so they do not flag themselves.
bash4_constructs() {
  cat <<'LIST'
declare -A
local -A
typeset -A
declare -n
local -n
mapfile
readarray
wait -n
coproc
,,}
^^}
&>>
|&
;;&
EPOCHSECONDS
EPOCHREALTIME
LIST
}

# External commands this script may run on macOS; anything else in command position must be a bash builtin, a bash
# keyword or a function defined here. GNU-only tools such as timeout are deliberately absent.
known_commands() {
  cat <<'LIST'
awk basename bash cargo cat chmod cp cut date dirname env git grep head hostname id ln ls mkdir mktemp mv node npm
npx osascript pgrep ps python3 rm rustc scutil sed shasum sleep sort sw_vers sysctl tar tee touch tr uname uniq wc
LIST
}

# awk: "LINE<TAB>text" for every line outside here-documents that is not a whole-line comment. With mode=code the
# quoted strings, escapes, arithmetic and trailing comments are blanked out as well.
code_lines_awk() {
  cat <<'AWK'
skip != "" { if ($0 == skip) skip = ""; next }
{
  line = $0
  if (match(line, /<<'[A-Za-z_]+'/)) skip = substr(line, RSTART + 3, RLENGTH - 4)
  if (line ~ /^[ \t]*#/) next
  if (mode == "code") {
    gsub(/\\./, "_", line)
    gsub(/\$\(\([^)]*\)\)/, " _A_ ", line)
    gsub(/"[^"]*"/, " _Q_ ", line)
    gsub(/'[^']*'/, " _Q_ ", line)
    sub(/(^|[ \t])#.*$/, "", line)
  }
  print NR "\t" line
}
AWK
}

# awk over code lines: "def<TAB>LINE<TAB>NAME" for function definitions, "use<TAB>LINE<TAB>WORD" for every word in
# command position (after ; && || | & $( and backquotes, past keywords, case patterns and VAR=value prefixes).
command_words_awk() {
  cat <<'AWK'
BEGIN { FS = "\t" }
{
  n = $1; line = $2
  if (match(line, /^[ \t]*[A-Za-z_][A-Za-z0-9_]*\(\)/)) {
    name = substr(line, RSTART, RLENGTH); gsub(/[ \t()]/, "", name)
    print "def\t" n "\t" name
    line = substr(line, RSTART + RLENGTH)
  }
  gsub(/\$\(|`|&&|\|\||\||;|&/, "\n", line)
  count = split(line, segs, "\n")
  for (i = 1; i <= count; i++) {
    words = split(segs[i], w, /[ \t]+/)
    for (j = 1; j <= words; j++) {
      x = w[j]
      if (x == "") continue
      if (x ~ /\)$/ && x !~ /^\(/) continue
      if (x ~ /^(if|then|else|elif|do|while|until|!|time|\{|\}|\(|\)|\(\(|\)\))$/) continue
      if (x ~ /^[A-Za-z_][A-Za-z0-9_]*=/) continue
      if (x ~ /^(for|case|select|function|\[\[|in|esac|fi|done|\]\]|;;)$/) break
      if (x ~ /^[A-Za-z_][A-Za-z0-9_.-]*$/ && x != "_Q_" && x != "_A_") print "use\t" n "\t" x
      break
    }
  }
}
AWK
}

self_check() {
  local script="$1" failures=0 raw code defs uses u w n known pat hits names dups opt t lt sq dq
  report() {
    if [ "$1" = ok ]; then printf '  ok   %s\n' "$2"; else printf '  FAIL %s\n' "$2"; failures=$((failures + 1)); fi
  }
  printf 'self-check: %s\n' "$script"
  if bash -n "$script"; then report ok "bash -n"; else report fail "bash -n"; fi
  if [ "$(head -n 1 "$script")" = "#!/usr/bin/env bash" ]; then report ok "shebang"; else report fail "first line must be #!/usr/bin/env bash"; fi
  # Counted with tr and wc: some grep and awk builds (Git Bash on Windows) drop carriage returns while reading.
  if [ "$(tr -d '\r' <"$script" | wc -c | tr -d ' ')" = "$(wc -c <"$script" | tr -d ' ')" ]; then
    report ok "LF line endings"
  else
    report fail "carriage returns in the file (bash on macOS rejects CRLF line endings)"
  fi
  raw="$(awk -v mode=raw "$(code_lines_awk)" "$script")"
  code="$(awk -v mode=code "$(code_lines_awk)" "$script")"

  set -f
  hits=""
  local IFS="$nl"
  for pat in $(bash4_constructs); do
    u="$(printf '%s\n' "$raw" | grep -F -e "$pat" | head -n 1)"
    [ -n "$u" ] && hits="$hits ${u%%"$tab"*}:$pat"
  done
  IFS=" $tab$nl"
  if [ -z "$hits" ]; then report ok "no bash 4+ constructs (macOS ships bash 3.2)"; else report fail "bash 4+ constructs at line:pattern$hits"; fi

  t='/t'; t="${t}mp"
  hits="$(printf '%s\n' "$raw" | grep -F -e "$t" | grep -v -F -e ":-$t" | cut -f 1 | tr '\n' ' ')"
  if [ -z "$hits" ]; then report ok "no hard-coded temporary directory outside the TMPDIR fallback"; else report fail "hard-coded temporary directory at lines $hits"; fi

  sq="$(printf '\047')"; dq="$(printf '\042')"
  hits="$(printf '%s\n' "$code" | grep -e "$sq" -e "$dq" | cut -f 1 | tr '\n' ' ')"
  if [ -z "$hits" ]; then report ok "quoted strings close on their own line"; else report fail "multi-line quoted string at lines $hits (keep strings on one line so these checks stay reliable)"; fi

  lt='<'
  hits="$(printf '%s\n' "$raw" | grep -E -e "$lt$lt[-A-Za-z_\"]" | cut -f 1 | tr '\n' ' ')"
  if [ -z "$hits" ]; then report ok "here-documents use quoted tags"; else report fail "unquoted or dash-form here-document tags at lines $hits (the checks only skip quoted ones)"; fi

  names="$(printf '%s\n' "$code" | grep -oE '(^|[[:space:]])(step|step_expect|ro_step|build_step)[[:space:]]+[a-z][a-z0-9-]*' | awk '{ print $NF }')"
  # A literal row name may appear on several alternative code paths (pass or fail), so each counts once.
  names="$names$nl$(printf '%s\n' "$code" | grep -oE '(^|[[:space:]])row[[:space:]]+[a-z]+[[:space:]]+[a-z][a-z0-9-]*' | awk '{ print $NF }' | sort -u)"
  for u in "${ro_groups[@]}"; do names="$names$nl$(group_step_name "$u")"; done
  dups="$(printf '%s\n' "$names" | grep -v '^$' | sort | uniq -d | tr '\n' ' ')"
  if [ -z "$dups" ]; then report ok "step names unique ($(printf '%s\n' "$names" | grep -c -v '^$') rows)"; else report fail "duplicate step names: $dups"; fi

  defs="$nl$(printf '%s\n' "$code" | awk "$(command_words_awk)" | awk -F '\t' '$1 == "def" { print $3 }')$nl"
  uses="$(printf '%s\n' "$code" | awk "$(command_words_awk)" | awk -F '\t' '$1 == "use" { print $2 ":" $3 }')"
  known=" $(known_commands | tr '\n' ' ') "
  hits=""
  IFS="$nl"
  for u in $uses; do
    n="${u%%:*}"; w="${u#*:}"
    case "$defs" in *"$nl$w$nl"*) continue ;; esac
    case "$known" in *" $w "*) continue ;; esac
    case "$(type -t "$w" 2>/dev/null)" in builtin|keyword) continue ;; esac
    hits="$hits $n:$w"
  done
  IFS=" $tab$nl"
  set +f
  if [ -z "$hits" ]; then report ok "every command is a function defined here, a bash builtin or a listed macOS tool"; else report fail "undefined function or unlisted command at line:word$hits"; fi

  hits=""
  for opt in $(sed -n '2,/^set -u$/p' "$script" | grep -oE '^#[[:space:]]+--[a-z-]+' | awk '{ print $2 }'); do
    grep -qE -e "[[:space:]|]$opt[)|]" "$script" || hits="$hits $opt"
  done
  if [ -z "$hits" ]; then report ok "every documented option is parsed"; else report fail "documented but not parsed:$hits"; fi

  if [ "$failures" -eq 0 ]; then echo "self-check passed"; else echo "self-check: $failures failure(s)"; fi
  [ "$failures" -eq 0 ]
}

orig_args=("$@")
quick=0; scan_dir=""; desktop=0; device_tests=0; step_timeout=300; build_timeout=3600; self_check_only=0
while [ $# -gt 0 ]; do
  case "$1" in
    --quick) quick=1 ;;
    --scan-dir) shift; scan_dir="${1:-}"; [ -n "$scan_dir" ] || die "--scan-dir needs a directory" ;;
    --desktop) desktop=1 ;;
    --device-tests) device_tests=1 ;;
    --step-timeout) shift; step_timeout="${1:-}"; is_uint "$step_timeout" || die "--step-timeout needs a whole number of seconds > 0" ;;
    --build-timeout) shift; build_timeout="${1:-}"; is_uint "$build_timeout" || die "--build-timeout needs a whole number of seconds > 0" ;;
    --self-check) self_check_only=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
  shift
done
if [ "$self_check_only" -eq 1 ]; then
  self_check "$0"
  exit $?
fi

[ "$(uname -s)" = "Darwin" ] || { echo "run this on macOS (only --self-check works elsewhere)" >&2; exit 2; }
repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo" || exit 2
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
run_id="run_mac_verify_$(printf '%s' "$stamp" | tr 'A-Z' 'a-z')"
out="$repo/target/mac-verify/$stamp"
mkdir -p "$out"
summary="$out/summary.md"
tsv="$out/steps.tsv"
privacy_tsv="$out/privacy.tsv"
: >"$tsv"
: >"$privacy_tsv"
store="$out/store"
tmp="${TMPDIR:-/tmp}"
tmp="${tmp%/}"
fixture="$tmp/cm-mac-verify-$$"
export CM_STORE_DIR="$store"

say() { printf '%s\n' "$*" | tee -a "$summary"; }

# descendants PID: PID and all of its descendants, parents first (pgrep -P is part of macOS).
descendants() {
  local c
  echo "$1"
  for c in $(pgrep -P "$1" 2>/dev/null); do descendants "$c"; done
}

# run_bounded LIMIT LOG CMD...: run CMD with stdin from /dev/null and stdout+stderr in LOG. After LIMIT seconds CMD
# and every descendant get TERM, then KILL 5 s later. Sets bounded_rc (124 on timeout), bounded_secs and
# bounded_timed_out. Pure bash: macOS has no GNU timeout.
run_bounded() {
  local limit="$1" log="$2" t0=$SECONDS pid pids p waited
  shift 2
  "$@" >"$log" 2>&1 </dev/null &
  pid=$!
  bounded_timed_out=0
  while kill -0 "$pid" 2>/dev/null; do
    if [ $((SECONDS - t0)) -ge "$limit" ]; then
      bounded_timed_out=1
      pids="$(descendants "$pid" | tr '\n' ' ')"
      for p in $pids; do kill -TERM "$p" 2>/dev/null; done
      waited=0
      while [ "$waited" -lt 5 ] && kill -0 "$pid" 2>/dev/null; do sleep 1; waited=$((waited + 1)); done
      for p in $pids; do kill -KILL "$p" 2>/dev/null; done
      break
    fi
    sleep 0.2
  done
  wait "$pid" 2>/dev/null
  bounded_rc=$?
  if [ "$bounded_timed_out" -eq 1 ]; then
    bounded_rc=124
    printf '\n[mac_verify] TIMEOUT after %s s; stopped processes: %s\n' "$limit" "$pids" >>"$log"
  fi
  bounded_secs=$((SECONDS - t0))
}

want_has() {
  case "|$1|" in *"|$2|"*) return 0 ;; esac
  return 1
}

# row KIND NAME CLASS RESULT RC SECONDS: one summary table row and one line of steps.tsv (read for the run record).
# CLASS is pass, partial, fail, timeout or info (info rows are not graded).
row() {
  say "| $2 | $4 | $6 s |"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5" "$6" >>"$tsv"
}

# run_step KIND NAME WANT CMD...: run CMD bounded (build steps by --build-timeout, the others by --step-timeout), its
# full output in NAME.log. WANT lists the exit codes that pass ("0", "5" or "0|5"); exit 5 where 0 also passes is
# reported as partial.
run_step() {
  local kind="$1" name="$2" want="$3" limit="$step_timeout" cls result
  shift 3
  [ "$kind" = build ] && limit="$build_timeout"
  run_bounded "$limit" "$out/$name.log" "$@"
  if [ "$bounded_timed_out" -eq 1 ]; then
    cls=timeout; result="TIMEOUT after $limit s"
  elif want_has "$want" "$bounded_rc"; then
    if [ "$bounded_rc" -eq 0 ]; then cls=pass; result=pass
    elif [ "$want" = "$bounded_rc" ]; then cls=pass; result="pass (expected exit $bounded_rc)"
    else cls=partial; result="partial (exit $bounded_rc)"
    fi
  else
    cls=fail; result="FAIL ($bounded_rc)"
  fi
  row "$kind" "$name" "$cls" "$result" "$bounded_rc" "$bounded_secs"
  [ "$cls" = pass ] || [ "$cls" = partial ]
}
build_step() { local name="$1"; shift; run_step build "$name" 0 "$@"; }
step() { local name="$1"; shift; run_step cli "$name" 0 "$@"; }
step_expect() { local name="$1" want="$2"; shift 2; run_step cli "$name" "$want" "$@"; }
ro_step() { local name="$1"; shift; run_step cli "$name" "0|5" "$@"; }

json() { "$cli" "$@" --json; }

# json_field FILE KEY: a top-level string field of a JSON log, or nothing.
json_field() {
  python3 -I - "$1" "$2" 2>/dev/null <<'PYEOF'
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        v = json.load(f).get(sys.argv[2], "")
except Exception:
    v = ""
print(v if isinstance(v, str) else "")
PYEOF
}

# The fixture's locked folder makes the scan partial with exactly one permission_required issue on locked/.
scan_shape_ok() {
  python3 -I - "$1" <<'PYEOF'
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        r = json.load(f)
except Exception as e:
    print("not a JSON scan_result:", e)
    sys.exit(1)
issues = r.get("issues", [])
ok = r.get("status") == "partial" and len(issues) == 1 and issues[0].get("code") == "permission_required" \
    and issues[0].get("scope_display", "").endswith("/locked")
print("status=%s issues=%s" % (r.get("status"), [i.get("code") for i in issues]))
sys.exit(0 if ok else 1)
PYEOF
}

cleanup_fixture() {
  [ -d "$fixture" ] || return 0
  chmod 700 "$fixture/locked" 2>/dev/null
  rm -rf "${fixture:?}"
}

# desktop_smoke: launch the debug desktop build with its own store for 20 s, check that it stays alive and (when
# macOS lets System Events answer) how many windows it has, then stop exactly the process it started.
desktop_smoke() {
  local bin="" c pid t0=$SECONDS alive=1 rc waited=0 windows
  for c in "${CARGO_TARGET_DIR:-$repo/apps/desktop/src-tauri/target}/debug/cleanmanager-desktop" \
    "$repo/apps/desktop/src-tauri/target/debug/cleanmanager-desktop"; do
    if [ -x "$c" ]; then bin="$c"; break; fi
  done
  if [ -z "$bin" ]; then
    desktop_window="not checked (no built desktop binary)"
    row desktop desktop-launch-smoke fail "FAIL (no built desktop binary)" 127 0
    return 1
  fi
  mkdir -p "$out/desktop-store"
  CM_STORE_DIR="$out/desktop-store" "$bin" >"$out/desktop-launch-smoke.log" 2>&1 </dev/null &
  pid=$!
  while [ $((SECONDS - t0)) -lt 20 ]; do
    sleep 1
    kill -0 "$pid" 2>/dev/null || { alive=0; break; }
  done
  if [ "$alive" -eq 0 ]; then
    wait "$pid" 2>/dev/null
    rc=$?
    desktop_window="not checked (the app exited)"
    row desktop desktop-launch-smoke fail "FAIL (exited after $((SECONDS - t0)) s, exit $rc)" "$rc" $((SECONDS - t0))
    return 1
  fi
  row desktop desktop-launch-smoke pass "pass (alive for 20 s)" 0 $((SECONDS - t0))
  # Asking System Events may show an Automation prompt the first time; either answer is fine.
  run_bounded 15 "$out/desktop-window.log" osascript -e \
    "tell application \"System Events\" to count windows of (first process whose unix id is $pid)"
  windows="$(tr -d '[:space:]' <"$out/desktop-window.log")"
  case "$windows" in
    ''|*[!0-9]*) desktop_window="not checked (System Events did not answer; see desktop-window.log)" ;;
    *) desktop_window="$windows window(s) reported by System Events" ;;
  esac
  [ "$bounded_timed_out" -eq 1 ] && desktop_window="not checked (no answer within 15 s; an Automation prompt may be open)"
  row desktop desktop-window info "info: $desktop_window" "$bounded_rc" "$bounded_secs"
  kill -TERM "$pid" 2>/dev/null
  while [ "$waited" -lt 5 ] && kill -0 "$pid" 2>/dev/null; do sleep 1; waited=$((waited + 1)); done
  kill -KILL "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null
  return 0
}

totals_line() {
  local p='$3 != "info" { n[$3]++ } END { printf "%d passed, %d partial, %d failed, %d timed out", n["pass"], n["partial"], n["fail"], n["timeout"] }'
  awk -F "$tab" "$p" "$tsv"
}

privacy_section() {
  local kind name cls result rc secs log pc pp any=0
  say ""
  say "## Privacy prompts and slow steps"
  say ""
  say "A first read of a privacy-protected folder waits until its macOS dialog is answered (see"
  say "docs/platforms/macos-verification.md). Steps that took more than 30 s:"
  say ""
  while IFS="$tab" read -r kind name cls result rc secs; do
    [ "${secs:-0}" -gt 30 ] 2>/dev/null || continue
    any=1
    if [ "$kind" = build ]; then
      say "- $name ($kind): $secs s, $result"
    else
      say "- $name ($kind): $secs s, $result -- look for an unanswered privacy dialog"
    fi
  done <"$tsv"
  [ "$any" -eq 1 ] || say "- none"
  say ""
  say "permission_cause / privacy_prompt_pending seen in CLI and desktop outputs:"
  say ""
  any=0
  while IFS="$tab" read -r kind name cls result rc secs; do
    [ "$kind" = build ] && continue
    log="$out/$name.log"
    [ -f "$log" ] || continue
    pc="$(grep -o -F permission_cause "$log" | wc -l | tr -d ' ')"
    pp="$(grep -o -F privacy_prompt_pending "$log" | wc -l | tr -d ' ')"
    [ "$pc" -gt 0 ] || [ "$pp" -gt 0 ] || continue
    any=1
    say "- $name: permission_cause x$pc, privacy_prompt_pending x$pp"
    printf '%s\tpermission_cause\t%s\n%s\tprivacy_prompt_pending\t%s\n' "$name" "$pc" "$name" "$pp" >>"$privacy_tsv"
  done <"$tsv"
  [ "$any" -eq 1 ] || say "- none"
  say ""
  say "If a step above waited on a dialog, answer it, rerun the same command and say whether you allowed or denied it."
}

submit_note() {
  say ""
  say "## How to submit"
  say ""
  say "1. Open an issue at https://github.com/patrickleehua/CleanManager/issues and paste this summary.md."
  say "2. Attach run-record.json from the same folder. Both are redacted: the home folder is shown as ~, the user name"
  say "   as <user> and the computer name as <host>."
  say "3. The *.log files and the .tgz are the raw outputs and are NOT redacted: they hold local paths, the names of"
  say "   items in your Trash and your app and process lists. Do not attach them publicly; share them only when a"
  say "   maintainer asks, after looking through them."
  say "4. Maintainers: review the raw logs, copy run-record.json to docs/evidence/runs/$run_id.json, keep status and"
  say "   actual as recorded, add review findings to limitations and run python -I tools/validate_docs.py. A run record"
  say "   alone never turns a device-matrix claim into verified."
}

# write_run_record: redacts summary.md in place and writes run-record.json (schemas/v1/run-record.schema.json).
write_run_record() {
  if ! python3 -I -c 'pass' 2>/dev/null; then
    echo "run-record.json not written: python3 is not available" | tee -a "$summary"
    return 1
  fi
  CMV_STAMP="$stamp" CMV_COMMIT="$commit" CMV_DIRTY="$dirty" CMV_PRODUCT="$product" CMV_BUILD="$build" \
  CMV_MODEL="$model" CMV_ARCH="$arch" CMV_TRANSLATED="$translated" CMV_CLI_SHA256="$cli_sha256" \
  CMV_ELEVATED="$elevated" CMV_QUICK="$quick" CMV_DESKTOP="$desktop" CMV_DEVICE_TESTS="$device_tests_set" \
  CMV_SCAN_DIR_GIVEN="$([ -n "$scan_dir" ] && echo 1 || echo 0)" CMV_STEP_TIMEOUT="$step_timeout" \
  CMV_BUILD_TIMEOUT="$build_timeout" CMV_DESKTOP_WINDOW="$desktop_window" CMV_USERS="$users" CMV_HOSTS="$hosts" \
  CMV_TMP="$tmp" CMV_HOSTED="${GITHUB_ACTIONS:-}" \
  python3 -I - "$tsv" "$privacy_tsv" "$summary" "$out/run-record.json" ${orig_args[@]+"${orig_args[@]}"} <<'PYEOF'
import json, os, re, subprocess, sys
from datetime import datetime, timezone

tsv, privacy_tsv, summary_path, record_path = sys.argv[1:5]
argv = sys.argv[5:]
env = os.environ.get


def flag(name):
    return env(name) == "1"


# Redaction as in crates/cm-cli/src/doctor_cmd.rs (Redactor): home -> ~, user -> <user>, host -> <host>,
# case-insensitive, longest first.
pairs = []
home = (env("HOME") or "").rstrip("/")
if len(home) > 3:
    pairs.append((home, "~"))
for var, mask in (("CMV_USERS", "<user>"), ("CMV_HOSTS", "<host>")):
    for token in (env(var) or "").splitlines():
        token = token.strip()
        if len(token) >= 2:
            pairs.append((token, mask))
pairs.sort(key=lambda p: len(p[0]), reverse=True)


def text(s):
    for needle, mask in pairs:
        s = re.sub(re.escape(needle), lambda _m, m=mask: m, s, flags=re.IGNORECASE)
    return s


def redact(v):
    if isinstance(v, str):
        return text(v)
    if isinstance(v, list):
        return [redact(x) for x in v]
    if isinstance(v, dict):
        return {k: redact(x) for k, x in v.items()}
    return v


def rows(path, width):
    found = []
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                parts = line.rstrip("\n").split("\t")
                if len(parts) == width:
                    found.append(parts)
    except OSError:
        pass
    return found


def fs_type(path):
    try:
        real = os.path.realpath(path)
        lines = subprocess.run(["/sbin/mount"], capture_output=True, text=True, timeout=10).stdout.splitlines()
    except Exception:
        return None
    best = None
    for line in lines:
        m = re.match(r"^.+? on (.+) \(([^,)]+)", line)
        if not m:
            continue
        point = m.group(1)
        if real == point or real.startswith(point.rstrip("/") + "/"):
            if best is None or len(point) > len(best[0]):
                best = (point, m.group(2))
    return best[1].upper() if best else None


def scan_backend():
    try:
        with open(os.path.join(os.path.dirname(record_path), "cli-scan-fixture.log"), encoding="utf-8") as f:
            b = json.load(f).get("backend")
        return b if isinstance(b, str) else None
    except Exception:
        return None


steps = []
for kind, name, cls, result, rc, secs in rows(tsv, 6):
    steps.append({"name": name, "kind": kind, "class": cls, "result": result,
                  "rc": int(rc) if re.fullmatch(r"-?[0-9]+", rc) else None,
                  "seconds": int(secs) if secs.isdigit() else None})
graded = [s for s in steps if s["class"] != "info"]


def named(cls):
    return [s["name"] for s in graded if s["class"] == cls]


passed, partial, failed, timed_out = named("pass"), named("partial"), named("fail"), named("timeout")
status = "failed" if failed else "incomplete" if (timed_out or not graded) else "passed"
actual = "%d steps: %d passed, %d partial (exit 5), %d failed, %d timed out" % (
    len(graded), len(passed), len(partial), len(failed), len(timed_out))
for label, group in (("failed", failed), ("timed out", timed_out), ("partial", partial)):
    if group:
        actual += "; %s: %s" % (label, ", ".join(group))

privacy = {}
for name, pattern, count in rows(privacy_tsv, 3):
    privacy.setdefault(name, {})[pattern] = int(count) if count.isdigit() else None

lim = []
if env("CMV_HOSTED") == "true":
    lim.append("Ran on a hosted GitHub Actions runner (GITHUB_ACTIONS=true): not a reference device, not real-device acceptance.")
else:
    lim.append("Self-reported bundle from a contributor's Mac, not a hosted CI runner; a maintainer reviews the raw logs "
               "before registering it, and it does not by itself verify any device-matrix claim.")
lim.append("Working tree had uncommitted changes; code_commit is the base commit." if flag("CMV_DIRTY")
           else "Working tree matched code_commit (git status clean).")
if flag("CMV_QUICK"):
    lim.append("--quick: the workspace tests and clippy did not run.")
elif flag("CMV_DEVICE_TESTS"):
    lim.append("--device-tests: CM_ALLOW_MAC_DEVICE_TESTS=1 was set for workspace-tests only; those tests moved their own "
               "fixture files to the real Trash and took them out again, and reset the Quick Look cache.")
else:
    lim.append("CM_ALLOW_MAC_DEVICE_TESTS was not set (removed from the workspace-tests environment): test legs that use "
               "the real Trash or reset Quick Look printed skipped.")
if flag("CMV_DESKTOP"):
    lim.append("--desktop: desktop window " + (env("CMV_DESKTOP_WINDOW") or "unknown")
               + ". A 20 s launch smoke is not a UI acceptance of any page.")
else:
    lim.append("--desktop not given: the desktop app was neither built nor launched.")
lim.append("Full Disk Access and other privacy (TCC) grants were not observed by the script, so permission_context.grants is empty.")
if flag("CMV_TRANSLATED"):
    lim.append("The shell ran under Rosetta translation on arm64 hardware; build and test steps may have produced x86_64 binaries.")
if flag("CMV_SCAN_DIR_GIVEN"):
    lim.append("--scan-dir scanned a folder chosen by the person running it; its entries are only in the raw log.")
lim.append("Redacted: home folder as ~, user name as <user>, computer name as <host>; the raw logs in the bundle are not redacted.")

commit = env("CMV_COMMIT") or ""
sha = env("CMV_CLI_SHA256") or ""
arch = env("CMV_ARCH")
product = env("CMV_PRODUCT") or ""
record = {
    "schema_version": "1.0",
    "test_id": "SCN-07",
    "run_id": "run_mac_verify_" + (env("CMV_STAMP") or "unknown").lower(),
    "recorded_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "code_commit": commit if re.fullmatch(r"[0-9a-f]{40}", commit) else None,
    "binary_sha256": sha if re.fullmatch(r"[0-9a-f]{64}", sha) else None,
    "device_id": "dev_mac_apple_silicon_primary" if arch == "arm64" else "dev_mac_intel",
    "os": {"platform": "macos", "name": ("macOS " + product) if product else None, "build": env("CMV_BUILD") or None,
           "architecture": arch if arch in ("arm64", "x64") else None},
    "filesystem": {"type": fs_type(env("CMV_TMP") or "/"), "volume_id": None},
    "permission_context": {"elevated": flag("CMV_ELEVATED"), "signing_identity": None, "grants": []},
    "scope_digest": None,
    "fixture_manifest_sha256": None,
    "backend": scan_backend(),
    "config": {"quick": flag("CMV_QUICK"), "desktop": flag("CMV_DESKTOP"), "device_tests": flag("CMV_DEVICE_TESTS"),
               "scan_dir_given": flag("CMV_SCAN_DIR_GIVEN"),
               "step_timeout_seconds": int(env("CMV_STEP_TIMEOUT") or 0),
               "build_timeout_seconds": int(env("CMV_BUILD_TIMEOUT") or 0)},
    "command": ["tools/mac_verify.sh", *argv],
    "seed": None,
    "expected": "tools/mac_verify.sh on a real Mac: build and test steps exit 0; the fixture scan is partial (exit 5) with "
                "exactly one permission_required issue on locked/; core CLI steps exit 0; read-only command groups exit 0 "
                "or 5 (partial) within the step timeout; with --desktop the built app stays alive for 20 s.",
    "actual": actual,
    "terminal_state": "completed",
    "exit_code": 0 if status == "passed" else 1,
    "measurements": {
        "steps": [{k: s[k] for k in ("name", "kind", "result", "rc", "seconds")} for s in steps],
        "slow_steps": [{"name": s["name"], "seconds": s["seconds"]} for s in steps if (s["seconds"] or 0) > 30],
        "privacy_markers": privacy,
        "desktop_window": env("CMV_DESKTOP_WINDOW") if flag("CMV_DESKTOP") else None,
        "macos_product_version": product or None,
        "hw_model": env("CMV_MODEL") or None,
        "rosetta_translated": flag("CMV_TRANSLATED"),
    },
    "observer": "tools/mac_verify.sh (self-reported; maintainer review pending)",
    "limitations": lim,
    "status": status,
}
required = ["schema_version", "test_id", "run_id", "recorded_at", "code_commit", "binary_sha256", "device_id", "os",
            "filesystem", "permission_context", "scope_digest", "fixture_manifest_sha256", "backend", "config", "command",
            "seed", "expected", "actual", "terminal_state", "exit_code", "measurements", "observer", "limitations", "status"]
assert sorted(record) == sorted(required), "run record keys differ from the schema"
assert re.fullmatch(r"run_[a-z0-9_-]{3,64}", record["run_id"]), "run_id pattern"
record = redact(record)
with open(record_path, "w", encoding="utf-8", newline="\n") as f:
    f.write(json.dumps(record, ensure_ascii=False, indent=2) + "\n")
with open(summary_path, encoding="utf-8") as f:
    body = f.read()
with open(summary_path, "w", encoding="utf-8", newline="\n") as f:
    f.write(text(body))
print("run record: %s (%s)" % (record["run_id"], status))
PYEOF
}

# --- facts about this Mac ------------------------------------------------------------------------------------------
commit="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
dirty=0
[ -z "$(git status --porcelain 2>/dev/null)" ] || dirty=1
product="$(sw_vers -productVersion 2>/dev/null)"
build="$(sw_vers -buildVersion 2>/dev/null)"
model="$(sysctl -n hw.model 2>/dev/null)"
translated=0
[ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ] && translated=1
if [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ]; then arch=arm64
elif [ "$(uname -m)" = x86_64 ]; then arch=x64
else arch=""
fi
elevated=0
[ "$(id -u)" = 0 ] && elevated=1
users="$(printf '%s\n' "${USER:-}" "$(id -un 2>/dev/null)" "$(id -F 2>/dev/null)")"
hosts="$(hostname 2>/dev/null)$nl$(hostname -s 2>/dev/null)$nl$(scutil --get ComputerName 2>/dev/null)"
hosts="$hosts$nl$(scutil --get LocalHostName 2>/dev/null)$nl$(scutil --get HostName 2>/dev/null)"
device_tests_set=0
if [ "$quick" -eq 1 ]; then
  device_tests_line="not applicable (--quick: no workspace tests)"
elif [ "$device_tests" -eq 1 ]; then
  device_tests_set=1
  device_tests_line="SET: CM_ALLOW_MAC_DEVICE_TESTS=1 for workspace-tests only (tests move their own fixture files to your Trash and take them out again)"
else
  device_tests_line="not set (CM_ALLOW_MAC_DEVICE_TESTS removed from the workspace-tests environment; Trash and Quick Look legs print skipped)"
fi
[ "${GITHUB_ACTIONS:-}" = true ] && device_tests_line="$device_tests_line; GITHUB_ACTIONS=true enables those legs anyway"

say "# CleanManager macOS verification -- $stamp"
say ""
say "- commit: \`$commit\`$([ "$dirty" -eq 1 ] && echo ' (working tree modified)')"
say "- macOS: $product (build $build), $(uname -m)$([ "$translated" -eq 1 ] && echo ' under Rosetta'), model $model"
say "- rust: $(rustc --version 2>/dev/null || echo missing); node: $(node --version 2>/dev/null || echo missing)"
say "- elevated: $([ "$elevated" -eq 1 ] && echo yes || echo no)"
say "- step timeout: $step_timeout s for CLI and desktop steps, $build_timeout s for build and test steps"
say "- device tests: $device_tests_line"
say "- desktop: $([ "$desktop" -eq 1 ] && echo 'build, tests and a 20 s launch smoke requested' || echo 'not requested')"
say "- run record: run-record.json (run_id $run_id)"
say ""
say "| step | result | time |"
say "| --- | --- | --- |"

# --- build and workspace tests -------------------------------------------------------------------------------------
if [ "$quick" -eq 0 ]; then
  # Only this step sees the device-test switch; without --device-tests it is removed even if the caller exported it.
  test_env="-u CM_ALLOW_MAC_DEVICE_TESTS"
  [ "$device_tests_set" -eq 1 ] && test_env="CM_ALLOW_MAC_DEVICE_TESTS=1"
  # test_env is split into words on purpose.
  build_step workspace-tests env $test_env cargo test --locked --workspace --no-fail-fast
  build_step workspace-clippy cargo clippy --locked --workspace --all-targets -- -D warnings
fi
cli="${CARGO_TARGET_DIR:-$repo/target}/release/cleanmanager"
cli_ok=0
build_step build-cli cargo build --locked --release -p cm-cli && [ -x "$cli" ] && cli_ok=1
cli_sha256=""
[ "$cli_ok" -eq 1 ] && cli_sha256="$(shasum -a 256 "$cli" | awk '{ print $1 }')"

# --- CLI smoke on a fixture and read-only commands -----------------------------------------------------------------
if [ "$cli_ok" -eq 1 ]; then
  # Fixture: a small tree with a symlink boundary, an unreadable folder and a non-ASCII name.
  trap cleanup_fixture EXIT
  trap 'cleanup_fixture; exit 130' INT
  trap 'cleanup_fixture; exit 143' TERM
  mkdir -p "$fixture/docs/nested" "$fixture/locked" "$fixture/proj/node_modules/pkg"
  printf 'hello' >"$fixture/docs/a.txt"
  head -c 1048576 /dev/zero >"$fixture/docs/nested/one-mib.bin"
  printf '{}' >"$fixture/proj/package.json"
  printf 'x' >"$fixture/proj/node_modules/pkg/index.js"
  printf 'Signature: 8a477f597d28d172789f06886806bc55\n' >"$fixture/docs/CACHEDIR.TAG"
  ln -s /etc "$fixture/link-to-etc"
  printf 'n' >"$fixture/$(printf 'caf\303\251')"
  printf 's' >"$fixture/locked/secret" && chmod 000 "$fixture/locked"

  step cli-capabilities json capabilities
  step cli-doctor "$cli" doctor --format text
  # The locked folder makes this scan partial (exit 5, docs/cli-protocol.md); anything else is a failure.
  step_expect cli-scan-fixture 5 json scan --root "$fixture"
  step cli-scan-fixture-shape scan_shape_ok "$out/cli-scan-fixture.log"
  scan_id="$(json_field "$out/cli-scan-fixture.log" scan_id)"
  if [ -n "$scan_id" ]; then
    step cli-entries json entries list --scan "$scan_id" --limit 50
    step cli-analyze json analyze --scan "$scan_id"
  fi
  step cli-experimental-on json experimental --state on
  step cli-capabilities-experimental json capabilities
  for group in "${ro_groups[@]}"; do
    # The group is split into words on purpose.
    ro_step "$(group_step_name "$group")" json $group
  done
  if [ -n "$scan_dir" ]; then
    ro_step cli-scan-user-dir json scan --root "$scan_dir"
  fi
  cleanup_fixture
else
  say ""
  say "CLI build failed; the CLI steps were skipped (see build-cli.log)."
  say ""
fi

# --- desktop -------------------------------------------------------------------------------------------------------
if [ "$desktop" -eq 1 ]; then
  desktop_window="not checked (desktop build failed)"
  build_step desktop-npm-ci bash -c 'cd apps/desktop && npm ci'
  build_step desktop-build bash -c 'cd apps/desktop && npm run build'
  build_step desktop-vitest bash -c 'cd apps/desktop && npx vitest run'
  build_step desktop-host-tests bash -c 'cd apps/desktop/src-tauri && cargo test --locked'
  if build_step desktop-app-build bash -c 'cd apps/desktop && npm run tauri -- build --debug --no-bundle'; then
    desktop_smoke
  fi
fi

# --- summary, run record, bundle -----------------------------------------------------------------------------------
say ""
say "Totals: $(totals_line)."
say "Full logs: target/mac-verify/$stamp/*.log (raw, not redacted). FAIL and TIMEOUT rows are the interesting ones."
privacy_section
submit_note
write_run_record
# Nothing is added to summary.md after this point: write_run_record has just redacted it.
record_status="$(json_field "$out/run-record.json" status)"
if tar -czf "$out.tgz" -C "$repo/target/mac-verify" "$stamp" 2>/dev/null; then
  echo "Bundle (raw logs, not redacted): target/mac-verify/$stamp.tgz"
else
  echo "Could not create target/mac-verify/$stamp.tgz; the folder target/mac-verify/$stamp is complete."
fi
echo "Paste target/mac-verify/$stamp/summary.md and attach run-record.json (both redacted). Status: ${record_status:-unknown}"
[ "$record_status" = passed ]
