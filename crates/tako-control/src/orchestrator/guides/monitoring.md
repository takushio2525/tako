## Monitoring Workers (for spawn, not needed for run)

**After spawning a worker, always set up monitoring. No exceptions.**

Use the Monitor tool to watch for completion:

```
Monitor({
  command: "tako orchestrator watch --pane <N>",
  description: "watching worker idle",
  timeout_ms: 1800000,
  persistent: false,
})
```

`--session-id` is no longer needed — the watch command automatically resolves the
pane to its claude session via pid ancestry. Only pass `--session-id` if you already
have it (e.g. from a previous status check).

The watch command will output when the worker stops:
- `WORKER_IDLE: tako:<pane> (ctx NN%)` — worker completed or awaiting input
- `WORKER_ERROR: tako:<pane> (<kind>)` — worker stalled on a known error
  (API error, usage limit, etc.). Extra `detail:` / `action:` lines follow.
- `WORKER_STALLED: tako:<pane>` — worker appears stuck: no running child
  processes and no busy screen pattern. Extra `detail:` / `action:` lines follow.
- `WORKER_PERMISSION: tako:<pane>` — worker is blocked on a permission dialog
  (tool execution approval). `command:` and numbered options follow.
- `WORKER_DIALOG: tako:<pane> (<kind>)` — worker is blocked on some **other**
  choice dialog (Issue #748): `usage_limit` (limit hit → what to do), `plan_confirm`
  (plan mode execution approval), `select` (model picker, `/mcp` list,
  AskUserQuestion). `title:`, the numbered options (with `← 現在の選択`),
  `action:` and a ready-to-run `respond:` line follow.
- `WORKER_GONE: tako:<pane>` — pane was closed

After WORKER_IDLE, WORKER_ERROR, or WORKER_PERMISSION, `event:` lines may
follow with additional context (Issue #243). These do NOT change the primary
signal — they augment it:
- `event: question` — the worker is asking a question (idle + question pattern
  on screen). Answer via `tako_send_input` or relay to the user.
- `event: permission_dialog` — the worker is blocked on a permission dialog.
  Use `tako_orchestrator_respond` to answer (see WORKER_PERMISSION below).
- `event: choice_dialog dialog_kind=<kind>` — the worker is blocked on a
  non-permission choice dialog (Issue #748). Same tool answers it; see
  "When you receive WORKER_DIALOG" below. `question` is never emitted at the
  same time: a dialog cannot be answered by replying in prose.
- `event: model_switched from=<model> to=<model>` — the worker's model was
  automatically downgraded (e.g. sol limit → sonnet). The worker continues but
  at lower capability. Consider `tako_task_checkpoint` + handoff to a better model.
- `event: context_high percent=<N>` — context usage exceeds 60%. The worker
  risks hitting the context limit. Consider asking the worker to commit progress
  and checkpoint, or prepare a handoff.

### When you receive WORKER_IDLE

1. **Check the `events` first** — if `question` is present, the worker is NOT
   done: it is waiting for your answer. Answer via `tako_send_input`, or relay
   the question to the user if it is genuinely the user's call. Re-arm the watch.
2. **Confirm before acting** — idle notifications can misfire. Read the pane
   with `tako_read_pane`. If it shows an active thinking/working indicator, the
   worker is NOT done: wait and re-arm the watch. Long thinking is normal at
   high effort — allow at least 10 minutes before suspecting a stall.
3. If `model_switched` is present, the worker completed on a downgraded model.
   Note the model change in your inspection — the worker may have made
   lower-quality decisions. Consider re-running critical sections on the
   original model after limits reset.
4. If `context_high` is present (percent > 60%), the worker is nearing its
   context limit. After inspection, consider whether the next task for this
   worker should be a fresh spawn instead of a continuation.
5. Worker reports completion → run Acceptance Inspection, then follow the
   lifecycle rules.

`tako_orchestrator_worker_status` also returns `has_running_children` (true if
the worker's tmux session has active child processes), `collapsed` (true if
the TUI is in a folded "N new messages" state), and `events` (array of detected
events — see above). When `collapsed` is true, the pane text may be incomplete
— use `has_running_children` and the `status` field as the primary signals, not
the screen text.

**`N shells still running` does not mean the worker is busy** (Issue #1273). A
background shell or monitor keeps claude's own `agents --json` reporting `busy`
long after the turn ended, so tako overrides it to `idle` when — and only when —
the screen proves the turn is over: no busy indicator anywhere, an input box that
exists and is empty, not folded, and the status line declaring what is still
running (`background_work`, e.g. `1 shell, 1 monitor`). When that override fires,
`idle_despite_primary_busy` is true and `status_source` still says `agents`.
Priority: a visible busy indicator always wins over the empty input box (claude
draws the input box while generating too), and `Waiting for … to finish` — claude
blocking *on* the background work — stays `busy`. On very narrow panes the status
line is truncated, the evidence is lost, and the worker stays `busy` as before.

### When you receive WORKER_ERROR

The worker stalled — it did NOT complete. Do not run Acceptance Inspection.
Recover by `kind` (also in `tako_orchestrator_worker_status` as
`error.kind` / `error.recommended_action`):

- `api_error` (action: resume) — transient API failure (connection closed,
  timeout). Send a continue nudge via `tako_send_input` (e.g. "続きを実行して")
  and re-arm the watch. The worker keeps its context.
- `usage_limit` (action: wait_reset) — usage limit reached. Read the pane for
  the reset time, wait until then (or tell the user), then send a continue
  nudge. Immediate resends will bounce.
- `limit_dialog` (action: respond_dialog) — a rate-limit dialog (e.g. codex
  model-switch prompt) is blocking. Answer it with `tako_orchestrator_respond`
  (look first with no `choice`), **not** with `tako_send_input`: a bare Enter
  confirms whatever is highlighted, which on codex is "switch to a cheaper
  model". Prefer the option that keeps the current model / waits for the reset.
- `entitlement_blocked` (action: needs_human) — the account itself is blocked:
  seat type without usage credits, usage allocation disabled by an admin, a
  group limit set to $0, a model that requires credits, extra usage exhausted,
  or the service disabled for the org. **Time does not fix this**, so do NOT
  wait for a reset, do NOT send a continue nudge, and do NOT close → respawn:
  every retry hits the same wall. Report the `detail` line to the user verbatim
  and tell them what has to change (admin action, plan/seat change, buying
  credits, or `/model` to a model their plan covers). Then park the worker —
  re-arm the watch only after the user says the account side is fixed.
- `login_expired` (action: relogin) — the login for that account has expired
  mid-session (its OAuth refresh token was invalidated, typically because the
  same account was used from another machine). **`relogin` is never solved by
  `resume`**: it surfaces as `API Error: Unable to connect to API (ENOTFOUND /
  ECONNRESET)`, so a continue nudge looks like the right move and tako used to
  classify it as `api_error` — but not one request gets through until a human
  re-authenticates (measured three times in 2026-08; every time the master spun
  in circles). So do NOT nudge, do NOT wait for a reset, and do NOT close →
  respawn. **Ask the user to run `/login` in that worker's pane** — tako never
  runs it for them, it needs a browser — and **name the account**:
  `error.account` (the name in accounts.yaml) and `error.config_dir` (the
  `CLAUDE_CONFIG_DIR` that worker runs under) come back in the status response,
  and watch prints them on the `account=… config_dir=…` line. With several
  accounts in play the user cannot tell which one to fix without that. The
  worker keeps its context, so once the user is back in, a single continue nudge
  resumes the same conversation. It can also hide *behind* a usage-limit dialog:
  after you answer the dialog, re-arm the watch and expect this kind next.
- `launch_failed` (action: fix_launch) — the agent CLI never started (missing
  CLI, not logged in, immediate exit, local runtime down). The `detail` already
  carries the reason and the next step; do that first (install / log in / start
  the runtime). Nudges and waiting cannot help, and respawning repeats it.
- `execution_refused` (action: retry_spawn) — the CLI **did** start and the
  instruction **did** arrive, but the agent refused to run for its own reason
  (measured: agy waiting on an account-eligibility check). tako only reports
  this when its primary signal shows the agent took **zero** steps, so the
  worker did no work at all and holds no state worth keeping. Unlike
  `launch_failed` nothing is broken, and unlike `entitlement_blocked` **time
  does fix it**: wait a short while, then close the pane and spawn the same
  instruction again. A continue nudge does nothing here (no turn ever started).
  If repeated retries keep landing here, treat it as an account problem and
  show the user the `detail` line.

### When you receive WORKER_STALLED

The worker appears stuck — no running child processes and the screen shows
neither a busy indicator nor an idle prompt. Read the pane to diagnose:
- If it shows a prompt, send a continue nudge via `tako_send_input`.
- If it shows an error, treat as WORKER_ERROR.
- If the output is unclear (TUI may be folded), try `tako_send_input` with
  a brief nudge and re-arm the watch.

### When you receive WORKER_PERMISSION

The worker is blocked on a permission dialog — it is asking for approval to
execute a tool (Bash command, file write, etc.). Read the `command:` and
options to decide:

1. **Safe commands** (build, test, lint, read-only operations, project-scoped
   writes): approve with `tako_orchestrator_respond` (choice "yes" or "1").
   Re-arm the watch afterwards.
2. **Dangerous commands** (rm -rf, database mutations, production deploys,
   credential access, commands outside the project scope): **escalate to the
   user**. Show them the exact command and let them decide. Do NOT auto-approve.
3. **If uncertain**: read the pane with `tako_read_pane` for more context, or
   escalate to the user. When in doubt, escalate.

The `tako_orchestrator_respond` tool verifies the dialog is still present before
sending the response — if the user already dismissed it manually, you will get
an error (not an accidental keypress).

### When you receive WORKER_DIALOG (Issue #748)

Any choice dialog other than a permission prompt. **A dialog owns the input
box**, so `tako_send_input` is refused with an error while one is open (text
would be eaten as key presses and a digit would confirm a choice). Always answer
with `tako_orchestrator_respond`:

- **Look before you answer**: call `tako_orchestrator_respond` with `pane_id`
  and **no `choice`** — it sends nothing and returns the structure
  (`kind`, `title`, `options[{number,label,highlighted}]`, `numbered`).
  `tako_read_pane` / `tako_orchestrator_worker_status` return the same object as
  `choice_dialog`.
- **Then answer** with `choice` = the number **or a distinctive part of the
  label** (case-insensitive; ambiguous matches error out instead of guessing).
  Prefer the label when the option order may shift.

Per kind:

- `usage_limit` (action: respond_wait) — the limit was hit and the worker asks
  what to do. Pick the option that **waits** ("Stop and wait for limit to
  reset" / "Keep current model"). Options that upgrade a plan, buy credits, or
  switch models cost money or capability: **escalate to the user** instead of
  choosing them. Then wait for the reset as with `usage_limit` above.
- `plan_confirm` (action: respond) — the worker finished planning and asks to
  execute. Approve only if the plan matches the task you assigned; otherwise
  pick the "tell Claude what to change" option and send corrections.
- `select` (action: respond) — a picker (`/model`, `/mcp`, AskUserQuestion).
  If it came from AskUserQuestion, this is the worker asking **you**: answer it
  from the task context, or relay to the user when it is genuinely their call.
  Do not silently change a worker's model or configuration.
- `trust` / `bypass` (action: auto_accept, `auto_accepted: true`) — tako accepts
  these itself. Do nothing; they disappear on their own.

Dialogs whose options are **not numbered** (`numbered: false`, e.g. the `/mcp`
list) cannot be answered with number keys — tako navigates with arrow keys and
verifies the cursor landed on the label you asked for before pressing Enter. If
it cannot land there, you get an error and **nothing is confirmed**.

Do NOT close → respawn on WORKER_ERROR, WORKER_STALLED, WORKER_PERMISSION, or
WORKER_DIALOG:
the worker's context is intact and a resume is almost always cheaper than a
respawn.

Restart a worker (close → respawn) ONLY on: explicit error output in the pane
that a resume nudge did not clear, ~10+ minutes with no output and no thinking
indicator, or the worker itself declaring it cannot proceed. Respawning a
worker that was merely thinking throws away its entire context and doubles
token cost.

### Task Checkpoints and Resume (Issue #242)

Use `tako_task_checkpoint` to record a worker's progress (Issue number, branch,
phase, last commit) before or during long tasks. If a worker hits usage_limit
or crashes, the watch loop automatically marks its checkpoint as `suspended`.

To list checkpoints: `tako_task_list` (optionally filter by `phase`).

To resume a suspended task: `tako_task_resume` with the `task_id`. This spawns
a new worker on the same branch/cwd/Issue context with a resume prompt that
includes the last commit and suspension reason. You can override the model
(e.g. switch from sol to fable after a usage_limit):

1. `tako_task_list` with `phase: "suspended"` to find interrupted tasks.
2. `tako_task_resume` with `task_id` (and optionally `model` to switch).
3. The new worker picks up from the last commit. Re-arm the watch as usual.

Best practice: call `tako_task_checkpoint` when spawning a worker, and again
when the worker reports a phase change (e.g. "tests passing" → verifying).
The watch loop handles suspension automatically on errors.
