#!/usr/bin/env bash
# Collects a real-Mac verification bundle for CleanManager (see docs/platforms/macos-verification.md).
#
# Read-only for your data: tests write only under $TMPDIR, the CLI smoke uses a throwaway store and
# a fixture tree it creates, and nothing is deleted outside those. Results go to
# target/mac-verify/<timestamp>/ (git-ignored); attach summary.md and the folder's .tgz to a GitHub issue.
#
# Usage: tools/mac_verify.sh [--quick] [--scan-dir DIR] [--desktop]
#   --quick      skip the full workspace test run (CLI smoke only)
#   --scan-dir   additionally run a read-only scan of DIR (e.g. ~/Downloads); results stay local
#   --desktop    also build and test the desktop web UI and the Tauri host
set -u

quick=0; scan_dir=""; desktop=0
while [ $# -gt 0 ]; do
  case "$1" in
    --quick) quick=1 ;;
    --scan-dir) shift; scan_dir="${1:-}" ;;
    --desktop) desktop=1 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

[ "$(uname -s)" = "Darwin" ] || { echo "run this on macOS" >&2; exit 2; }
repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo" || exit 2
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
out="$repo/target/mac-verify/$stamp"
mkdir -p "$out"
summary="$out/summary.md"
store="$out/store"
fixture="${TMPDIR:-/tmp}/cm-mac-verify-$$"
export CM_STORE_DIR="$store"

say() { printf '%s\n' "$*" | tee -a "$summary"; }
# step_expect NAME RC CMD...: runs CMD, keeps full output in NAME.log, records pass/fail and duration; exit code RC
# counts as a pass (0 for most steps).
step_expect() {
  local name="$1" want="$2"; shift 2
  local t0=$SECONDS
  "$@" >"$out/$name.log" 2>&1
  local rc=$?
  local result="FAIL ($rc)"
  if [ $rc -eq "$want" ]; then result="pass"; [ "$want" -ne 0 ] && result="pass (expected exit $want)"; fi
  say "| $name | $result | $((SECONDS - t0)) s |"
  [ $rc -eq "$want" ]
}
step() { local name="$1"; shift; step_expect "$name" 0 "$@"; }

say "# CleanManager macOS verification — $stamp"
say ""
say "- commit: \`$(git rev-parse HEAD)\`$(git diff --quiet || echo ' (working tree modified)')"
say "- macOS: $(sw_vers -productVersion) ($(sw_vers -buildVersion)), $(uname -m), model $(sysctl -n hw.model)"
say "- rust: $(rustc --version 2>/dev/null || echo missing); node: $(node --version 2>/dev/null || echo missing)"
say "- elevated: $([ "$(id -u)" = 0 ] && echo yes || echo no)"
say ""
say "| step | result | time |"
say "| --- | --- | --- |"

if [ $quick -eq 0 ]; then
  step workspace-tests cargo test --locked --workspace --no-fail-fast
  step workspace-clippy cargo clippy --locked --workspace --all-targets -- -D warnings
fi
step build-cli cargo build --locked --release -p cm-cli || { say ""; say "CLI build failed; see build-cli.log"; exit 1; }
cli="$repo/target/release/cleanmanager"

# Fixture: a small tree with a symlink boundary, an unreadable folder and a non-UTF-8-safe name.
mkdir -p "$fixture/docs/nested" "$fixture/locked" "$fixture/proj/node_modules/pkg"
printf 'hello' >"$fixture/docs/a.txt"
head -c 1048576 /dev/zero >"$fixture/docs/nested/one-mib.bin"
printf '{}' >"$fixture/proj/package.json"
printf 'x' >"$fixture/proj/node_modules/pkg/index.js"
printf 'Signature: 8a477f597d28d172789f06886806bc55\n' >"$fixture/docs/CACHEDIR.TAG"
ln -s /etc "$fixture/link-to-etc"
printf 'n' >"$fixture/$(printf 'caf\xc3\xa9')"
printf 's' >"$fixture/locked/secret" && chmod 000 "$fixture/locked"

json() { "$cli" "$@" --json; }
step cli-capabilities json capabilities
step cli-doctor "$cli" doctor --format text
# The fixture's locked folder makes this scan partial (exit 5, docs/cli-protocol.md); anything else is a failure.
step_expect cli-scan-fixture 5 json scan --root "$fixture"
python3 -c '
import json, sys
r = json.load(open(sys.argv[1]))
issues = r.get("issues", [])
ok = r.get("status") == "partial" and len(issues) == 1 and issues[0].get("code") == "permission_required" \
    and issues[0].get("scope_display", "").endswith("/locked")
sys.exit(0 if ok else 1)' "$out/cli-scan-fixture.log" 2>/dev/null \
  || say "| cli-scan-fixture-shape | FAIL (expected partial with one permission_required issue on locked/) | 0 s |"
scan_id="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("scan_id",""))' "$out/cli-scan-fixture.log" 2>/dev/null)"
if [ -n "$scan_id" ]; then
  step cli-entries json entries list --scan "$scan_id" --limit 50
  step cli-analyze json analyze --scan "$scan_id"
fi
step cli-experimental-on json experimental --state on
step cli-capabilities-experimental json capabilities
for group in "clean list" "purge roots list" "installer locations" "insights overview" "apps list" "startup list" "optimize list" "status --count 1"; do
  # shellcheck disable=SC2086
  step "cli-$(echo "$group" | tr ' ' '-')" json $group
done
if [ -n "$scan_dir" ]; then
  step cli-scan-user-dir json scan --root "$scan_dir"
fi

if [ $desktop -eq 1 ]; then
  ( cd apps/desktop && npm ci ) >"$out/desktop-npm-ci.log" 2>&1
  step desktop-build bash -c 'cd apps/desktop && npm run build'
  step desktop-vitest bash -c 'cd apps/desktop && npx vitest run'
  step desktop-host-tests bash -c 'cd apps/desktop/src-tauri && cargo test --locked'
fi

chmod 700 "$fixture/locked" 2>/dev/null; rm -rf "${fixture:?}"
say ""
say "Full logs: target/mac-verify/$stamp/*.log. FAIL rows are the interesting ones; unsupported results in the"
say "cli-* logs are expected where the macOS backend does not exist yet."
tar -czf "$out.tgz" -C "$repo/target/mac-verify" "$stamp" 2>/dev/null && say "Bundle: target/mac-verify/$stamp.tgz"
