# CleanManager command examples

Every example is checked against the CLI argument table by the `cm-cli` unit tests; lines marked `# draft` belong to command groups that a build may not implement yet (`help` and `capabilities` tell). Placeholders: `<PATH>`, `<SCAN_ID>`, `<SET_ID>`, `<PLAN_ID>`, `<OPERATION_ID>`, `<ENTRY_ID>`, `<CANDIDATE_ID>`, `<QUARANTINE_ID>`, `<APP_ID>`, `<LEFTOVER_ID>`, `<STARTUP_ITEM_ID>`, `<TRASH_ITEM_ID>`, `<TASK_ID>`, `<PID>`. All IDs come from earlier outputs - never build them yourself.

## Contents

- Discover and scan
- Browse a scan
- Catalog cleanup (clean)
- Rule analysis and plans
- Build artifacts (purge) and installers
- Approval, apply, history, recovery
- Protection
- Read-only system views
- Changes on explicit request only (trash, startup, process, maintenance, updates, uninstall)
- Product tools

## Discover and scan

```text
cleanmanager capabilities --json                                             # capabilities
cleanmanager scan --root "<PATH>" --json --non-interactive                   # scan_result; exit 0 complete, 5 partial
cleanmanager scan --root "<PATH>" --format jsonl --non-interactive           # progress lines, then exactly one scan_result
cleanmanager scan --root "<PATH>" --detach --json --non-interactive          # scan_status: a background host owns the scan
cleanmanager scan wait <SCAN_ID> --timeout 60 --json                         # scan_result, or scan_status + exit 8 if still running
cleanmanager scan show <SCAN_ID> --json                                      # scan_status while running; scan_result after
cleanmanager scan cancel <SCAN_ID> --json                                    # scan_cancel_receipt (not a terminal state)
```

## Browse a scan

```text
cleanmanager entries list --scan <SCAN_ID> --limit 20 --json                         # biggest entries first (logical_desc)
cleanmanager entries list --scan <SCAN_ID> --kind file --limit 20 --json             # largest files anywhere in the scope
cleanmanager entries list --scan <SCAN_ID> --parent <ENTRY_ID> --limit 20 --json     # one folder's children
cleanmanager entries list --scan <SCAN_ID> --limit 20 --cursor "<CURSOR>" --json     # next page: same sort/filter/limit
```

`logical_bytes: null` with `size_basis: unknown` means unknown, not zero. A `cursor_stale` error (exit 6) means start the listing again.

## Catalog cleanup (clean)

```text
cleanmanager clean list --json                                                        # cleanup_catalog
cleanmanager clean --scan <SCAN_ID> --dry-run --non-interactive --json                # selection_preview; changes nothing
cleanmanager clean --scan <SCAN_ID> --mode quarantine --non-interactive --json        # plan of default_selected rows only
cleanmanager clean --scan <SCAN_ID> --mode trash --candidates <CANDIDATE_ID> --non-interactive --json   # rows the user picked
```

The plan's `stale_items` lists previewed rows that changed since the scan (deleted, resized); they are not in the plan. All rows stale gives `plan_stale` (exit 6): scan again.

## Rule analysis and plans

```text
cleanmanager analyze --scan <SCAN_ID> --json                                          # analysis_result with candidate_set_id
cleanmanager candidates list --set <SET_ID> --limit 50 --json                         # candidates_page
cleanmanager plan create --set <SET_ID> --mode quarantine --all-default-selected --json
cleanmanager plan create --set <SET_ID> --mode quarantine --candidates <CANDIDATE_ID> --json
cleanmanager plan show <PLAN_ID> --json                                               # plan + authorization_status
```

## Build artifacts (purge) and installers

```text
cleanmanager purge roots --json                                                       # purge_roots
cleanmanager purge scan --non-interactive --json                                      # selection_preview (same as purge --dry-run)
cleanmanager purge plan create --all-default-selected --non-interactive --json        # only non-recent, locally rebuildable artifacts
cleanmanager installer locations --json                                               # installer_locations
cleanmanager installer scan --non-interactive --json                                  # selection_preview; installers are review_only
cleanmanager installer plan create --candidates <CANDIDATE_ID> --non-interactive --json
```

Dependency folders (`node_modules`, `.venv`, `vendor`) and recent projects are never preselected; `--source` (a whole download source) works only for a person in a terminal.

## Approval, apply, history, recovery

```text
cleanmanager plan apply <PLAN_ID> --json                     # exit 3 needs_approval until the person approves
cleanmanager plan review <PLAN_ID>                           # FOR THE PERSON, in their own terminal - never run it for them
cleanmanager operations show <OPERATION_ID> --json           # operation_status / operation_report
cleanmanager operations reconcile <OPERATION_ID> --json      # resolve unknown outcomes; never re-apply instead
cleanmanager operations list --limit 20 --json               # operation_list with the real recovery path per operation
cleanmanager recovery list --json                            # quarantine_list
cleanmanager recovery plan create --restore <QUARANTINE_ID> --json   # its own plan and approval
```

## Protection

```text
cleanmanager protect list --json                                        # protection_list (hard deny, defaults, user entries)
cleanmanager protect check --path "<PATH>" --json                       # protection_check: which layer decides
cleanmanager protect add --path "<PATH>" --note "user asked" --json     # only when the user asks to protect something
```

Removing protection is terminal-only (`protect remove`), for the person.

## Read-only system views

```text
cleanmanager status --json                                                     # status_snapshot; unknown metrics are null with a reason
cleanmanager status --watch --interval 5 --count 3 --format jsonl              # one status_snapshot per line; always bound it with --count
cleanmanager insights --json                                                   # insight_list: guidance for hibernation, pagefile, component store...
cleanmanager insights overview --json                                          # insight_overview: cards with known size or unknown, and when observed
cleanmanager insights downloads --older-than-days 90 --limit 20 --json         # old_downloads_review: largest first, nothing selected
cleanmanager startup list --json                                               # startup_list (read-only inventory)
cleanmanager process list --by memory --limit 20 --json                        # process_list: location, signer, purpose, protection
cleanmanager process show <PID> --json                                         # one process with its local facts
cleanmanager optimize list --json                                              # maintenance_catalog: tasks, applicability, what is not offered
cleanmanager optimize diagnose --window 5 --json                               # maintenance_diagnosis: read-only, never ends processes
cleanmanager optimize history --limit 20 --json                                # maintenance_history (maintenance has no undo)
cleanmanager apps list --limit 50 --json                                       # app_list; uninstall_capability = the route below
cleanmanager apps uninstall list --json                                        # app_uninstall_list: route per instance, or why there is none
cleanmanager apps updates show --json                                          # app_update_list from the last check (never checks)
cleanmanager trash list --json                                                 # trash_list: what is in the user's Recycle Bin
```

`apps updates` (no subcommand) checks winget, the Store and Scoop now and may contact the network; run it only when the user asks to check for updates. Nothing is sent while checks are off (`apps updates config --checks off`).

## Changes on explicit request only

Each line creates a plan (or changes this tool's own settings) that the person reviews with `cleanmanager plan review <PLAN_ID>` in their own terminal; no policy approves these. Run one only when the user asked for that exact change - never as a way to free space or speed the machine up.

```text
cleanmanager trash inspect --scan <SCAN_ID> --entry <ENTRY_ID> --json          # read-only: coverage, protection, holding processes
cleanmanager trash --scan <SCAN_ID> --entry <ENTRY_ID> --json                  # manual_selection plan, Recycle Bin only
cleanmanager trash purge --item <TRASH_ITEM_ID> --json                         # permanent: only items already in the Recycle Bin
cleanmanager startup disable <STARTUP_ITEM_ID> --json                          # startup plan; protected items are refused
cleanmanager startup enable <STARTUP_ITEM_ID> --json
cleanmanager startup remove-stale <STARTUP_ITEM_ID> --json                     # only a Run entry whose program is gone, after a backup
cleanmanager startup restore --operation <OPERATION_ID> --json                 # plan restoring what that operation changed
cleanmanager process end <PID> --json                                          # process plan for the one process the user named; critical ones refused
cleanmanager optimize plan create --tasks <TASK_ID>,<TASK_ID> --json           # maintenance plan; admin tasks share one UAC prompt at apply
cleanmanager apps updates --json                                               # check for updates now (network)
cleanmanager apps updates config --checks off --json                           # turn update checks on or off
cleanmanager apps update --app <APP_ID> --json                                 # update plan; ids from apps updates show (package manager only)
cleanmanager status alerts --state on --cpu-threshold 90 --json                # sustained high-CPU alerts while streaming (off by default)
```

### Uninstall: two separately reviewed plans

```text
cleanmanager apps uninstall list --json                                        # pick the instance; state available / unverified / unsupported
cleanmanager apps uninstall plan create --instance <APP_ID> --json             # stage 1: app_quit + tool_adapter, with leftover_estimate (not authorized)
cleanmanager plan apply <PLAN_ID> --json                                       # after the person's review; report exit code, duration, rescan
cleanmanager apps uninstall leftovers --operation <OPERATION_ID> --json        # uninstall_leftovers: groups, evidence, markers, external_references
cleanmanager apps uninstall leftovers plan create --operation <OPERATION_ID> --items <LEFTOVER_ID>,<LEFTOVER_ID> --json   # stage 2: caches/logs + named ids
cleanmanager apps uninstall orphans --json                                     # leftovers of apps removed earlier: review_only, read-only
cleanmanager apps uninstall restore --operation <OPERATION_ID> --json          # recovery plan for keys and startup values stage 2 removed
```

- `unverified` / `unsupported` in the list: say why (`reason`) and offer the `settings_uri` (Settings > Apps > Installed apps) or the vendor's uninstaller. Never delete the install folder as a substitute.
- Stage 1 may end `failed` with `still_running`: the app did not close and nothing was uninstalled. Ask the user to close it and plan again. A plan that may end the app (`--end-running`) is only created by a person in their own terminal and confirmed there at apply time - never by you.
- Stage 1 `partial` with `leftovers_remain` is normal: the vendor uninstaller ran but left files or keys. Continue with `apps uninstall leftovers`.
- In the leftovers preview only `caches_logs` rows are `selected`. Settings, personal data, models, sessions and anything with `warnings` are added with `--items` only when the user picked those exact ids. `kept` (shared_owner, owner_unknown, app_reappeared), `blocked` and `action: manual` rows can never be planned; relay their `guidance`. `external_references` are read-only hints - tell the user, never edit those files.
- Every teardown step (shortcut, startup entry, registration, key) is its own action in the report; any skipped or failed step makes the operation `partial`, never `completed`.

## Product tools

```text
cleanmanager doctor --json                                    # doctor_report, redacted (no user names, no credentials)
cleanmanager scan --root "<PATH>" --json --non-interactive --debug   # same scan_result on stdout; a human-readable trace (phases, backend, skip reasons, exit codes) on stderr
cleanmanager doctor --export "<PATH>" --json                  # preview only; the file is written after the person confirms in a terminal
cleanmanager completion print powershell --format text        # completion script for eval/source
cleanmanager completion install bash --json                   # preview of the profile change; nothing written
```

`--debug` is accepted by every command and only adds a trace on stderr for troubleshooting (when the user reports a problem): stdout records, exit codes and every decision stay identical. Never parse the trace or branch on it, and never paste it anywhere without the user's consent; it carries no keys, but it does name paths (home shown as `~`).
