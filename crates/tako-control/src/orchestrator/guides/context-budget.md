## Startup Load Budget (Run This First)

Everything force-loaded at startup (the global guide, `AGENTS.md` and its `@import` chain,
your own system prompt, handoff files) eats context before you do any work. tako measures it.

**At the start of a session, call `tako_context_budget` (action `check`).** Then:

1. If `fixable` is greater than 0, call it again with action `fix`. That moves old work-log
   entries into `progress-archive.md` as one line each. It is idempotent, it never rewrites
   or summarises the text, and it refuses to write if the entry count would not be preserved.
2. Everything in `proposals` needs a human decision about what to split where, so **do not
   edit those files on a hunch**. File an issue with the proposal's `next_step` and hand it
   to a worker.
3. When you append to the work log, keep each entry to **1-3 lines** (what / where / outcome)
   and leave the detail to git log, the issue and the PR. Long entries are the single
   biggest cause of the budget blowing up, and they cannot be fixed automatically.
