# Scenario walk-throughs

Each scenario lists the calls, the records to look at and what to tell the user. Exit codes follow `schemas/v1/exit-codes.json`.

## 1. Normal: "my Downloads/caches take too much space"

1. `cleanmanager capabilities --json` - confirm `scan.*` is not `unsupported`.
2. `cleanmanager scan --root "<PATH>" --json --non-interactive` -> `scan_result`, exit 0, `status: completed`, `completeness: verified_scope`.
3. `cleanmanager entries list --scan <SCAN_ID> --limit 20 --json` -> show the top items with sizes.
4. `cleanmanager clean --scan <SCAN_ID> --dry-run --non-interactive --json` -> `selection_preview`. Summarize `summary.selected_count`, `selected_known_bytes`, `review_only_count`, `blocked_count`; explain notable rows with `impact`.
5. The user agrees -> `cleanmanager clean --scan <SCAN_ID> --mode quarantine --non-interactive --json` -> `plan` with `authorization_status: required`.
6. `cleanmanager plan apply <PLAN_ID> --json` -> exit 3 (scenario 2), or exit 0 `operation_report` when an enabled policy already covers it.
7. Report `succeeded`, `processed_logical_bytes`, and that quarantine frees space only after a later purge (`recovery` restores it).

## 2. needs_approval

`plan apply` returns `kind: error`, `code: needs_approval`, exit 3, `details.plan_id`. Nothing was touched.

Tell the user: "Review and approve it in your terminal: `cleanmanager plan review <PLAN_ID>` (type the 8-character code it shows). Tell me when done." Then run `plan apply` once. Do not loop, do not run `plan review` yourself, do not look for a flag that approves. If they decline, the plan simply expires (15 minutes).

## 3. partial

- Scan: exit 5, `status: partial` (`skipped_ranges > 0`, issues such as `permission_required`) or `interrupted` (`host_exited`). The confirmed results are usable; say which scope was unreadable and that sizes are "at least".
- Apply: exit 5, `operation_report.status: partial`. List `skipped`/`failed` items with their `code` (for example `in_use`, `precondition_changed`, `protected`). Do not retry the failed ones by other means.

## 4. stale

- `plan_stale` (exit 6) from `plan apply`: the plan expired or observations changed. Re-run the preview and create a new plan; do not edit the old one.
- A new plan with `stale_items`: those previewed rows changed since the scan (`missing`, `changed`) and were left out. Mention them; a fresh scan shows their current state.
- `cursor_stale` (exit 6) while paging: start the listing again without `--cursor`.

## 5. unknown / reconciliation_required

`operation_report.status: reconciliation_required` or `unknown > 0`, exit 8: the process stopped between an OS call and recording its result. Run `cleanmanager operations reconcile <OPERATION_ID> --json` and report what it confirms. Never re-apply the plan or delete the targets "to make sure".

## 6. policy_denied

`code: policy_denied`, exit 3: a protection layer, hard deny or rule tier refused the object. Explain the stated reason and that the person can review protection with `cleanmanager protect check --path "<PATH>" --json`. Do not remove protection, change rules, or delete the object with shell commands.

## 7. "Free up space" (CLI-13)

The user only asked for space. Run scenario 1 (and, if relevant, `purge scan` / `installer scan` previews). Do **not** start uninstalls, maintenance/optimization, startup changes or admin actions; you may mention them as options ("`startup list` shows 12 startup items; tell me if you want to change any"). Only an explicit request such as "disable the Updater startup item" starts that flow, and it still ends at the person's review.

## 8. Long or interrupted scans

- `scan --detach` returns at once with `scan_status`; poll with `scan wait <SCAN_ID> --timeout 60`. A wait timeout is exit 8 with `scan_status` - the scan keeps running; nothing is cancelled or rolled back.
- `scan cancel` returns a receipt (`accepted`, `state_at_receipt`); the scan ends `cancelled` (exit 130) at the next safe boundary with confirmed rows kept.
- If the hosting process died, `scan show` reports `status: interrupted` with a `host_exited` issue (exit 5); scan again if the user still needs a complete picture.

## 9. "Uninstall <app>" (explicit request)

1. `cleanmanager apps uninstall list --json` -> find the instance by name and publisher; if several match, ask which one. `uninstall.state`: `available` (with `adapter.display`), `unverified` or `unsupported` (protected classes such as security agents). For the last two, give the `reason` and the `settings_uri` / vendor route and stop.
2. `cleanmanager apps uninstall plan create --instance <APP_ID> --json` -> `plan` (`uninstall`: `app_quit` + `tool_adapter`) and `leftover_estimate` (an estimate only). Ask the person to review: `cleanmanager plan review <PLAN_ID>`.
3. After approval, `cleanmanager plan apply <PLAN_ID> --json`. `still_running` / `app_still_running`: the app did not close, nothing ran - ask the user to close it. `running_state_unknown`: skipped, explain. `partial` + `leftovers_remain`: the uninstaller ran (exit code and duration in the message) but left things behind.
4. `cleanmanager apps uninstall leftovers --operation <OPERATION_ID> --json` -> report per group what is preselected (caches and logs only), what needs a choice (settings, personal data, models, sessions; rows with `warnings`), what is `kept`/`blocked`/`manual` and why, and the `external_references` hints.
5. Only the rows the user picks: `cleanmanager apps uninstall leftovers plan create --operation <OPERATION_ID> --items <LEFTOVER_ID> --json` -> a second review. Report each step's result; one failed step (for example `in_use`) makes the whole operation `partial`.
6. If they regret a registry or startup removal: `cleanmanager apps uninstall restore --operation <OPERATION_ID> --json` (another review; recreated keys are never overwritten).

Never uninstall something to free space, never pass `--end-running`, and never delete an install folder or shortcut yourself when CleanManager refuses.

## 10. "My computer is slow"

Read-only first: `cleanmanager status --json`, `cleanmanager optimize diagnose --json`, `cleanmanager process list --by cpu --limit 20 --json`, `cleanmanager startup list --json`. Report what the records say (heavy processes with their `purpose` and `protected` flag, startup items, findings and their suggested next step). Do **not** end processes, disable startup items or run maintenance tasks on your own: offer them, and only the specific change the user then asks for (`process end <PID>`, `startup disable <STARTUP_ITEM_ID>`, `optimize plan create --tasks ...`) creates a plan that still ends at the person's review.
