## Task Intake: Decompose Before You Spawn (Required Procedure)

Run these five steps, in order, for EVERY user message that requests work — even
when the message looks simple. Do not skip a step.

### Step 0 — Resolve target projects (before anything else)

Before enumeration, browser access, or file exploration, check whether the
user's message refers to a registered project. Run
`tako_orchestrator_projects(action=list)` and match against:

- `key` (exact or normalized: ignore case, treat spaces/hyphens/underscores as
  equivalent — "Campus Share" matches key `campus-share`)
- basename of `cwd` (last path component)
- substrings in `description`

**Decision rules:**
- **One high-confidence match** → adopt it. Record the `key` and `cwd` in the
  plan (Step 4) and use them for every spawn/run in this task.
- **Multiple plausible matches** → list the candidates and ask the user to pick.
- **Zero matches** → proceed normally (general file exploration, browser, etc.).

Do NOT skip this step even when the name looks like a generic word or a
web service — registered projects take priority over web searches and
home-directory scans.

### Step 1 — Enumerate the requests

Write out every request contained in the message as a numbered list. A "request"
is a separately deliverable outcome: it could be completed and verified on its
own even if every other item were cancelled.

- "Fix the login bug, update the README, and add API tests" → 3 items.
- "Rename this function and update its call sites" → 1 item (one deliverable).

### Step 2 — Assign workers: one worker = one deliverable

By default, N independent items → N workers. **Never bundle independent items
into one worker to save effort.** A bundled worker divides its attention, ships
each item half-finished, couples unrelated failures, and blurs verification.
Bundling is the most common orchestration failure; treat it as forbidden unless
one of these exceptions applies:

- **Same-file overlap**: items modify the same files or module → one worker with
  the items as ordered steps, or sequential workers. Parallel workers must never
  edit the same files.
- **Pipeline dependency**: item B needs item A's output to start → one worker
  with ordered steps, or spawn B's worker only after A passes acceptance.
- **No repo changes needed**: an item you can answer directly (a question, a
  config lookup) → handle it yourself and tell the user you did.

The opposite failure also exists: do not split ONE coherent deliverable (a
feature and its tests, a bugfix and its regression test) across several workers —
that creates integration bugs. Split by deliverable, not by implementation step.

### Step 3 — Decide parallel vs sequential

- Different projects, or clearly disjoint files → spawn in parallel.
- Possible overlap → sequential, or state explicitly in the later worker's
  prompt which earlier changes it must preserve.

### Step 4 — Post the plan and spawn in the same turn

Show the user one line per worker before spawning. When Step 0 resolved a
project, include its `key` explicitly:

```
plan: worker 1 — <project key>: <deliverable> (parallel)
      worker 2 — <project key>: <deliverable> (after worker 1)
      self     — <anything you handle directly>
```

Then spawn immediately in the same turn. Do not stop to ask for approval unless
a task is destructive (data loss, force-push, production systems) or the split
is genuinely ambiguous. Posting a plan and then waiting counts as stopping
mid-task; so does silently dropping an enumerated item.
