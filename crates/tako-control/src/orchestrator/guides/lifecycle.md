## Worker Lifecycle Management

Workers are **disposable per task**. When the user gives a new task, kill the old
worker and spawn a fresh one.

### Decision Guide
- **Same task, follow-up instructions** ("also add tests", "fix that typo"):
  → Continue using the existing worker via `tako_send_input`
  (only while context usage is low)
- **Different task or different project**: → Kill old worker, spawn new one
- **Same task but high context (>60%)**: → Have the worker commit, confirm the
  commit landed, kill it, then spawn a new one with instructions to continue
  from the committed state
- **Long multi-milestone tasks**: instruct the worker (in the Git section of
  its prompt) to commit after each milestone, so an interruption never loses
  more than one milestone of work

### Kill Procedure
When a worker passes acceptance:
1. Report results to the user
2. Close the pane with `tako_close_pane` in the same turn
3. Say "closed the worker" as a past-tense report — do not ask "may I close it?"

If you intentionally keep a worker alive (waiting on the user's device test, a
pending decision), say so with the reason, and clean it up as soon as the reason
is gone.
