These are baseline heuristics for model/effort assignment, distilled from
real delegation outcomes. Local overrides (judgment-local.md) take precedence.

**By task type:**

- `bugfix-rooted` (root cause already identified, machine-verifiable fix):
  Any current-generation model at effort high is usually sufficient.
  Reserve top-tier models for fixes touching >5 files or subtle concurrency.
- `bugfix-unrooted` (symptoms only, needs investigation):
  Start with a scout worker (investigation) to find the root cause, then
  spawn the fix as bugfix-rooted. Skipping the scout is the #1 cause of
  rework in this category.
- `investigation` (read-only analysis, no code changes):
  Lower-tier models with effort high work well. Keep output_lines high
  to capture the full analysis.
- `feature-verifiable` (new feature with machine-checkable acceptance):
  Match model tier to complexity. Simple additions: mid-tier. Cross-cutting
  features or API design: top-tier.
- `feature-ui` (visual/interaction work, needs human verification):
  Always require operation log or screenshot in the report. Mark as
  "implemented but unverified" if evidence is missing — never as done.
- `docs` (documentation only):
  Any model. Effort medium is usually enough. Watch for spec drift.
- `review` (code review, audit):
  Mid-to-high tier. Multiple perspectives (correctness, security) benefit
  from higher reasoning effort.

**Cross-cutting rules:**

- A task that commits immediately after stopping (no thinking time between
  last edit and commit) often skips self-review. Add "re-read your entire
  diff" to verification steps for such workers.
- Workers hitting >60% context are making worse decisions. Checkpoint and
  hand off rather than pushing through.
- First-attempt pass rate is the best signal for model adequacy. If a
  task_type x model combination shows <70% first-pass rate in ledger stats,
  consider upgrading the model for that type.
