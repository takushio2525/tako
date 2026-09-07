## Worker Prompt Template (Required for Every Spawn)

Build every worker prompt — for `tako_orchestrator_spawn` and
`tako_orchestrator_run` alike — by filling this template. Every section is
required; if one has no content, write `none` so the omission stays visible.
Write the prompt in the user's working language.

```
## Task
<ONE deliverable in one sentence, then details.>

## Background
<Why this is needed, current state, what the user literally asked for.
 Bug fixes: reproduction steps / error output / root cause if known.>

## Scope
- In scope: <files, features, areas>
- Out of scope: <what must NOT be touched: neighboring refactors, unrelated
  cleanups, and the other items from the same user message>

## Constraints
- Read the repository's own guidance first (AGENTS.md / CLAUDE.md /
  CONTRIBUTING) and follow its conventions.
- Do the work directly in this session. Do not launch sub-agents, agent teams,
  or background orchestration — progress must stay visible in this pane.
- <tech restrictions, requirement documents, parallel-worker warnings, or none>

## Acceptance criteria
<Checkable statements — each verifiable by a command or a concrete observation.>
1. <e.g. `npm test` passes, including new tests for the changed behavior>
2. <e.g. doing X in the running app now shows Y>

## Verification steps (run ALL before reporting completion)
1. Build / lint / format checks used by this repo — all green.
2. Test suite (full, or affected scope) — all green.
3. Exercise the change end-to-end yourself and observe the new behavior.
   A passing build is NOT evidence that the feature works.
4. Probe edge cases relevant here: <empty input, error paths, boundaries>.
5. Re-read your entire diff, hunting for debug leftovers, unrelated edits,
   missed renames, and broken references.

## Git / deliverable
<This repo's expected flow (branch / commit / PR / merge) and the docs to
 update in the same commit. Long tasks: commit after each milestone so
 progress survives interruptions. State the definition of done, e.g.
 "pushed, PR opened".>

## Report format (mandatory)
Finish with a report containing exactly these four sections:
1. What changed — files + one-line summary each.
2. Evidence per acceptance criterion — the command you ran and its actual
   output (trimmed), or the concrete observation. "Done" without evidence
   will be rejected.
3. Not verified / risks — what you could not verify and why, plus known
   limitations.
4. Commit / PR references.
If you are blocked, stop and report the blocker; do not silently change scope.

## Commands for the user
If you need the user to run a command (install a dependency, restart the app,
verify something), present it with `tako_show_command` instead of writing it in
the chat — a command written in chat gets hard-wrapped to the pane width and
breaks when copied off screen.
```

Rules for filling it:

- **Root cause first (bug fixes)**: get a reproduction recipe, error output, or
  root cause into Background before delegating. If you don't have one, spawn a
  scout worker to find it first. Workers given a pinpointed cause succeed far
  more often than workers told to "find and fix".
- **Requirement-bound work** (course assignments, specs, client requirements):
  extract the concrete requirements yourself and paste them into Constraints,
  adding: "Implement exactly what the requirements state — no extra features,
  no unrequested refactors." Never delegate the reading as "check the spec and
  use your judgment".
- Acceptance criteria state outcomes, not implementation steps. If you cannot
  write a checkable criterion, the task is underspecified — clarify with the
  user, or send a scout, before spawning.
