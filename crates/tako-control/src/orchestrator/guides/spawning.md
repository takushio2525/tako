## Running Workers (Recommended)

Use `tako_orchestrator_run` for one-shot tasks. It spawns, waits for completion,
reads output, and closes the pane — all in a single MCP call. No Monitor setup needed.

```
tako_orchestrator_run({
  project: "project-key",
  prompt: "<prompt built from the Worker Prompt Template>",
  label: "short-label"
})
```

Returns `{ status, output, pane_id, duration_seconds, ... }`.
- `status: "completed"` — worker finished successfully
- `status: "timeout"` — hit the timeout (default 30 min); output contains partial results
- `status: "error"` — worker pane disappeared

Optional params: `timeout_seconds` (default 1800), `auto_close` (default true),
`output_lines` (default 200), `pane`, `tab`.

The returned `output` is a worker report like any other: run Acceptance
Inspection on it before telling the user the task is done.

## Spawning Workers (Advanced)

For long-running or interactive workers, use `tako_orchestrator_spawn` + manual monitoring.

```
tako_orchestrator_spawn({
  project: "project-key",
  prompt: "<prompt built from the Worker Prompt Template>",
  label: "short-label"
})
```

This will:
1. Look up the project's working directory from the configuration
2. Split a new pane and start the worker agent CLI in it (`claude` by default)
3. Send your prompt to the worker (with delivery verification)
4. Return the pane ID and tmux_session for monitoring

Always pass a `label` (2-4 words naming the deliverable) — without it the pane
title is just the project name and the user cannot tell workers apart. Check the
returned `agent` / `model` / `effort` fields and correct course if they are not
what you intended.

Workers can also run on other agent CLIs via the `agent` parameter
(`"claude"` / `"codex"` / `"agy"`, both spawn and run). Only pick a non-default
agent when the profile's Available Worker Agents section (below) lists it or the
user asks for it. `model` / `effort` are interpreted in that agent's native
vocabulary. codex / agy workers are monitored by screen heuristics (no
`claude agents` signal), so allow extra time before judging them idle.
