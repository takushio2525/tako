## Asking the User: File It, Don't Wait For It

Anything that needs the user's hands — an approval, a look at something you produced,
a permission you cannot grant yourself, a post they have to publish — goes into the
user task list. It does not go into the conversation and it does not go into the
handoff file. A line buried in a transcript is not a queue: the user cannot see it
from their phone, the next master does not inherit it, and nobody can tell whether
it was ever answered.

`tako_todo` is the whole interface. It is **not** `tako_task_checkpoint` /
`tako_task_gate` — those track *your* work. This one tracks *theirs*.

### 1. File it the moment you need an answer, then keep working

```
tako_todo({ action: "add", title: "…", kind: "review", body: "…" })
```

Filing is cheap and non-blocking. The user sees a line in tako's notice area
immediately, the item shows up on the desktop panel and on their phone, and the
reply comes back to you. So file it and move on to the next piece of work —
**do not idle a worker or yourself waiting for a human.**

Pick `kind` by what you are asking for:

| kind | use it for |
|---|---|
| `review` | you produced something and want eyes on it (a diff, a video, a page, a screenshot) |
| `confirm` | a decision you are not entitled to make alone ("ship this as 0.9.0?") |
| `permission` | an action that needs the user's authority or their machine (a login, a paid API, a force push) |
| `post` | something the user must publish themselves (X, YouTube, a release announcement) |
| `other` | none of the above |

The origin is filled in for you — the profile, the conversation and the pane are
recorded from the call. You never have to say "reply to me": the reply finds you.

### 2. Write the body so it can be answered without you

The user reads this on a phone, possibly hours later, with none of your context.
Say what you did, what you want decided, and what happens after each answer. One
screen of markdown, no transcript dumps.

Attach the actual artifacts rather than describing them:

- `attachments` — absolute paths. The video, the screenshot, the exported file. The
  phone downloads them straight from here. A path that does not exist yet is allowed
  (file the task first, fill the file in later) and the list marks it as missing.
- `copy_texts` — one entry per thing the user has to paste, as `Label=text`. For a
  post that means the post body, the title, the description and the hashtags as
  **separate** entries, because they go into different fields. One blob of text they
  have to edit down is a task you handed back to them.
- `links` — PRs, issues, deploy previews.

### 3. The reply arrives in your input box

When the user answers, the reply is delivered to the master that filed the task:

```
【ユーザー返答】todo u-12「…」
decision=needs_change via=pwa
コメント: …
```

If that master is gone, tako starts one on the same profile in a new tab and hands
it the task and the reply as its first message. Either way **the answer reaches a
master, and that master is expected to act on it.** Treat the arrival of one of
these lines the way you treat a worker report: read it, then continue the work it
unblocks. `tako_todo({ action: "show", id: "u-12" })` gives you the body, the
attachments and the whole reply thread.

The four decisions differ in what they leave behind:

- `approve` / `reject` — the user is done with it. The task closes. Carry out the
  decision.
- `needs_change` — the task stays open on purpose. Fix what the comment asks for,
  then tell the user it is ready again by replying in the same task (update the body)
  rather than filing a second one.
- `answered` — a question you asked got an answer. The task stays open until the work
  that depended on it is done; close it yourself with `action: "done"` when it is.

### 4. Check the queue at the start of a session

`tako_todo({ action: "list" })` returns the open items and `open_count`. Run it when
you take over a session (a handoff, a restart, the first message of the day). Items
filed by a previous master are still yours to finish — the reply came back to a
master, not to a conversation.

Do not close a task because it looks stale. `dismiss` means "we decided not to do
this", and it is the user's decision, not yours. If something has become irrelevant,
say so in the body and let the user dismiss it.

### 5. What does not belong here

- Your own checkpoints, gates and PR bookkeeping — `tako_task_checkpoint` /
  `tako_task_gate`.
- Anything you can settle yourself by reading the repository, running a command or
  asking a worker. Filing those trains the user to ignore the list.
- Secrets. The body and the comments are stored on disk and shown on a phone.
