# Optional: AI diagnosis (`diag`, contract `diag/1`)

`cleanmanager diag` explains **one** scanned object (what it is, what deleting it would affect, what is still unknown) by sending a redacted evidence package to a provider **the person configured**. It is an optional extension: everything else in this skill works without it.

## When to use it

- Only when the user asks why something exists or what deleting one specific object would do, and `cleanmanager diag config --json` shows a configured provider.
- Not configured (`exit 7 unsupported`, returned immediately): say so and continue with the core results. Never configure a provider, store an API key or pick a CLI adapter for the user.

## Disclosure is the person's choice

1. `cleanmanager diag preview --scan <SCAN_ID> --entry <ENTRY_ID> --json --non-interactive` - builds the exact payload and destination without sending anything. Show the user the destination (provider, endpoint, model) and the `payload` fields; full paths and child summaries are off unless they opt in.
2. Send only after the user agreed to **that** preview: `cleanmanager diag create --scan <SCAN_ID> --entry <ENTRY_ID> --payload-digest <DIGEST> --json` with exactly the same `--provider`/`--disclose`/`--question` options as the preview (any difference changes the digest and nothing is sent). A wider scope, a user answer or another destination makes a new preview; confirm again. Never reuse a digest to send something the user has not seen.
3. Being an external agent yourself does not make you the destination: do not paste the evidence package into your own context as a substitute for the person's provider choice unless they ask for that.

## Reading the result

- `diag show <DIAGNOSIS_ID> --json` / `diag list --json` return the same ID and evidence the desktop shows.
- Report the conclusion with its cited evidence IDs and the `unknowns`. Claims marked unsupported or model-inferred stay labelled that way.
- A diagnosis never approves, never adds candidates and never contains shell commands. The only way forward is the normal flow: preview -> plan (exactly the diagnosed candidate) -> `plan review` by the person -> `plan apply`.
- `expired` or stale diagnoses cannot hand off to a plan; rescan or re-preview instead.
