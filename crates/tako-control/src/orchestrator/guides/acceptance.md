## Acceptance Inspection (Before Reporting to the User)

Never relay a worker's "done" as fact — verify it. When a worker reports
completion (or `tako_orchestrator_run` returns output):

1. **Read the report** (`tako_orchestrator_report`; falls back to scrollback
   if the transcript is unavailable). Use `tako_read_pane` only for layout
   checks and liveness — it truncates on narrow panes.
2. **Check evidence against the acceptance criteria you set.** Every criterion
   needs evidence: an actual command with its output, or a concrete
   observation. If any is missing, send ONE message naming exactly which
   evidence is missing. Do not accept claims without evidence; do not re-ask
   vaguely.
3. **Spot-check independently.** Look at the diff stat and the key hunks
   (`git diff` / `git show` in the project directory, or have the worker print
   them). For "change A to B" tasks, confirm A actually became B in the code.
   Keep it targeted — this is verification, not a re-review of the repo.
4. **Non-machine-verifiable work** (visual UI, real devices, IME, rendering):
   require an operation log or screenshot in the report. Without one, report
   the task to the user as "implemented but unverified on <X>" — never as done.
5. **Verdict**:
   - PASS → report to the user: what changed, the evidence in one or two
     lines, remaining risks. Then close the worker per the lifecycle rules.
   - FAIL → send the worker a concrete defect list (expected vs actual, one
     line per defect) and re-inspect the fix. After 2 failed rounds, stop
     retrying: re-examine the root cause or the task split, and tell the user
     where things stand. A third blind retry wastes tokens and usually hides a
     mis-scoped task.
6. **Issue closing**: close an Issue (or let the worker close it) only when the
   original symptom is confirmed gone in the environment where it was reported,
   or an equivalent. A worker's claim alone never closes an Issue.

### Acceptance Gate (machine-verifiable criteria)

Use `tako_task_gate` to define machine-checkable acceptance criteria when
spawning a task. Then run `tako_task_gate_check` when the worker reports
completion — it executes Command predicates and checks PR merge status
automatically. This replaces manual `cargo test` / `gh pr view` round-trips.

- **Define** (at spawn time or any time before check):
  `tako_task_gate({ task_id, criteria: [{id: "tests", kind: {type: "command", cmd: "cargo test --workspace"}}, {id: "pr", kind: {type: "pr_merged", pr_number: 247}}] })`
- **Check** (after worker reports done):
  `tako_task_gate_check({ task_id, sync_checkpoint: true })`
  Command criteria run in the gate's cwd. `sync_checkpoint: true` (default)
  transitions the checkpoint phase to `done` when all criteria pass.
- **Show** (inspect current state):
  `tako_task_gate_show({ task_id })`
- Custom criteria (`type: "custom"`) are skipped by gate check — set their
  status manually via `tako_task_gate` with a `record_results` action.
