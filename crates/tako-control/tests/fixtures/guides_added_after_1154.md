<!-- #1154 の移送後に手順書へ新しく足した本文の宣言（Issue 番号つき）。 -->
<!-- 移送の番犬（prompt_guides.rs の `手順書は原文以外の行を含まない`）は -->
<!-- 「要約・言い換えで意味が変わる」ことを止めるためのもので、新機能の手順を足す道は -->
<!-- 別に要る。新しい種別やイベントを足したときは、その本文をここへ 1 文字も変えずに -->
<!-- 貼る。宣言していない行は今までどおり「創作」として落ちる。 -->
<!-- 置き場をテンプレート（default_system_prompt.md）側にしないのは #1154 の方向と -->
<!-- 逆になるから: 起動時ロードの固定費が増え、同じ表（種別ごとの対処）が 2 か所へ割れる。 -->
<!-- 行頭が <!-- の行は宣言に数えない（この注記そのものを許可行にしないため）。 -->

<!-- #757: ログイン失効（login_expired / relogin）の対処。monitoring の対処表へ追加 -->
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

<!-- #1004: リモートフォルダ / SSH の手順（新 topic `remote`）。prompt 側は topic 表の -->
<!-- 1 行と solo の 3 行だけで、本文はここへ 1 文字も変えずに貼る。 -->

## Remote Folders and SSH Setup

"Open the folder on <host>" and "register a new server" are one procedure with two
halves: the half you do yourself (list, probe, open, read, edit, save) and the half
that needs a human at a prompt (first login, key installation, VPN). Do your half
without asking, and delegate the other half explicitly instead of stalling on it.

### 1. The host list comes from one file

`tako_ssh_hosts` returns the `Host` entries of `~/.ssh/config` — that file only.

- Patterns containing `*` or `?` are excluded (a `Host *` defaults block is not a host).
- **`Include`d files are read** (#1400): relative paths resolve under `~/.ssh/`, `*` and
  `?` globs expand, nesting stops at 16 levels and cycles are detected. A host defined
  only in an included file appears in the list like any other — measured: `Include
  config.d/*.conf` put the hosts of both included files into `tako_ssh_hosts`.
  Files that cannot be read are skipped with the reason in the diagnostic log.
- **Settings under a `Match` block do not belong to the `Host` above it** (#1400), just
  as `ssh` itself treats them. `Match host bastion` / `User root` does not make the
  preceding `Host prod` connect as root.
- Registering a host means appending a `Host` block to `~/.ssh/config`. It shows up on
  the next call; nothing needs restarting. **Never invent the connection details** —
  ask the user for host name, login user, port and which key to use, then write the
  block. Do not copy private keys anywhere, and do not put a passphrase in a file.

### 2. Do these yourself (no user input needed)

| want | call |
|---|---|
| what hosts exist | `tako_ssh_hosts` |
| look at a remote tree without touching the UI | `tako_remote_folder` action `ls` |
| open the folder as a workspace | `tako_remote_folder` action `open` |
| read or change a remote file | action `open-file`, then `tako_preview_save` |
| a shell on the host | `tako_open_remote` |
| saves that could not be pushed | action `pending`, then action `push` |
| why a host did not appear by itself | action `auto` |

`open` also brings up a pane that is already logged in and `cd`-ed into that folder,
and `open-file` plus a save writes back over SFTP (measured: 18 B file edited through
the preview, save reported `state: "saved"` with 34 B, and the remote file changed).

### 3. Delegate anything that needs a human

Everything tako runs over SFTP uses `BatchMode=yes`, so a host that can only
authenticate interactively is **skipped with a reason rather than left hanging**. That
is the signal to hand the interactive step to the user:

- First login on a password / passphrase / 2FA host: open an SSH pane with
  `tako_open_remote` and tell the user to log in there. tako shares one ControlMaster
  socket between the pane and the file tree, so after that single login the tree opens
  with no further authentication.
- Installing a key on a new server (`ssh-copy-id`), unlocking a key, accepting a host
  key you were not expecting, signing into a VPN or Tailscale: run it through
  `tako_run_interactive` (it splits a visible pane, titles it and runs the command;
  poll `tako_run_interactive_status` for the exit code), or hand the user a
  `tako_show_command` card when they should run it themselves.

Do not retry a failure that needs a human. Say which host, which step, and what you
need back.

### 4. The order that works

0. `tako_ssh_hosts` — is the host already known?
1. If not, ask for the details and append the `Host` block to `~/.ssh/config`.
2. Probe with action `ls` before opening anything. It is cheap, it changes no UI, and
   it returns the failure classified, which tells you whether the next move is yours
   or the user's.
3. Open with action `open` (add `terminal: false` if you only want the tree).
4. Edit in the preview and save; check `pending` if the link was down.

### 5. Failures arrive classified — read the class, do not guess

Every remote failure comes back as reason, next step and the raw client output. The
classes and the move each one implies (all measured through `ls`):

- **host name could not be resolved** — the `Host` / `HostName` is wrong, or there is no
  entry for it at all. Fix the entry; do not start guessing host names.
- **connection refused** — reached the machine, nothing listening. Port or no sshd.
- **connection timed out** — network, VPN or the machine being off. User's move.
- **authentication failed** (`Permission denied (publickey)`) — the key is not
  installed or not offered. Open an SSH pane for a one-time login, or delegate
  `ssh-copy-id`. Never loop on it.
- **host key does not match known_hosts** — a user decision. Show it and say the
  remote may have been rebuilt; do not edit `known_hosts` for them.
- **SFTP subsystem unavailable** — the shell works but the file tree cannot.
- **conflict** (#966) — the remote changed since you opened the file. Do not force the
  overwrite; show the user the conflict and let them decide.

Connection failures never close a pane: the pane falls back to the local shell with the
reason still on screen, and `tako_list_panes` shows `ssh_connect` with
`phase: "failed"`, the host, the elapsed seconds and the reason.

### 6. Authentication is inherited, not configured

tako drives the system `ssh` and `sftp` with a shared ControlMaster, so `~/.ssh/config`,
keys, ssh-agent, `known_hosts` and 2FA all apply as they already do in a terminal.
There is nothing to configure inside tako, and there is no separate credential store to
populate. On Windows the OpenSSH client has no connection multiplexing, so each
operation authenticates on its own: a password-only host asks every time, and liveness
is reported as unknown.

### 7. Auto-detection covers the plain case only

Entering `ssh` in a pane adds that host's home to the tree by itself (on by default;
`action: "auto"` reads and toggles it). It deliberately skips forms where the real
destination is not the name on the command line, and action `auto` returns each skip
with its reason: a port the config does not account for, `ProxyJump` /
`ProxyCommand` / `HostName` overrides, a one-shot remote command, `-N`, or a
subsystem call.

- A non-default `Port` is fine when the config is the one declaring it (an `Include`d
  file counts): the check is not "is the port 22" but "does the name alone reach that
  port", so a host with `Port 2222` in the config appears by itself, including in the
  pane tako opened for it (#1411). A `-p` that the config does not account for is still
  skipped — someone typing `ssh -p 2222 <host>` may be reaching a different machine
  than that name resolves to.
- Windows cannot read process command lines, so auto-detection does not run there. Use
  the explicit path.

### 8. Where a connection opens (#1006)

`tako_open_remote` takes `target`:

- `split` (default) — a new pane in the current tab.
- `tab` — a new tab.
- `pane` — **turns an existing pane into the SSH session**; no new pane or tab, and the
  pane ID does not change. Panes that are not a plain shell (a full-screen TUI, a
  running command, an AI agent, a preview) are refused with a reason and a next step,
  so pick `split` for those.

Add `remote_dir` to land in a directory instead of the login `cwd`.

<!-- #1450: ユーザー向けタスク（新 topic `user-tasks`）。prompt 側は topic 表の 1 行だけで、 -->
<!-- 本文はここへ 1 文字も変えずに貼る。 -->

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
<!-- #1453: 走っている master を専用プロファイルへその場で寄せる（adopt）。 -->
<!-- task-intake の Step 0.5 と handoff の後任の節へ追加。prompt 側は 1 行の案内だけ。 -->

### Step 0.5 - Adopt the project's own profile

Once Step 0 resolved a registered project, call
`tako_orchestrator_adopt(name=<project key>)`. This is one call and it does not
restart anything: your session, conversation and context stay exactly as they
are. What changes is tako's own state for your pane - the role label, the
profile behind `tako_orchestrator_self`, the handoff destination, and the
defaults every worker you spawn from now on will start with.

- **Run it before the first spawn of that project.** Workers inherit the profile
  that is current at spawn time; adopting afterwards does not re-parent them.
- **The profile is created for you** if the project has none yet, inherited from
  `default` with the project's own `projects` and `cwd`.
- **From then on, `tako_orchestrator_self` is the source of truth** for which
  profile you are. Its `profile` field, not the name you were launched with.
- **Idempotent.** Calling it again with the same name answers `changed: false`.
  Passing `default` puts you back on the generic profile.
- **It can refuse**, and the refusal is information, not an obstacle: a master
  launched with its own dedicated profile (`tako master -<name>`) is already
  specialised, and a profile whose `master_agent` differs from yours would make
  your successor come back as a different agent. The error names the one command
  that resolves it. Do not work around a refusal by editing the profile files
  directly.

Your system prompt was fixed when your session started and cannot be swapped
mid-conversation. Adopting changes tako's state, not your prompt - so if the
adopted profile carries prompt text you need, the way to get it is a handoff,
which starts the successor with that profile's prompt.

   - **The successor is launched with the profile you currently hold**, which is
     the one `tako_orchestrator_self` reports - including a profile you took on
     mid-session with `tako_orchestrator_adopt`. The `successor_command` field
     of the adopt response names the exact command that will be used
     (`tako master -<key>`), and the project files the successor receives are
     the ones under that profile's `projects`. If you resolved a project this
     session but never adopted it, the successor comes back generic and the
     project's handoff file does not travel with it - adopt first, then hand off.

<!-- #1450 B4: 人待ちの項目は引き継ぎファイルではなく tako_todo へ起票する（handoff の -->
<!-- 「引き継ぎファイルに何を書くか」の節へ追加）。prompt 側は 1 行も足していない。 -->

     **Neither file is where you park things waiting on the user.** An approval,
     a review of something you produced, a permission you cannot grant yourself,
     a post they have to publish — file those with `tako_todo` (the `user-tasks`
     guide), not as a checklist in the handoff. A handoff file is read by your
     successor, never by the user: a list of user-waiting items parked there is
     invisible to the only person who can act on it, and the successor inherits
     the waiting instead of the answer.

<!-- #1466: worker の起票の戻り先（spawn 元の master）と、解けないときの扱い。 -->
<!-- user-tasks の「起票元は自動で入る」段落へ追記した本文をそのまま貼る。 -->
A worker's task comes back to **the master that spawned it**, not to whichever master
happens to be running, so workers can file freely.

The one case that has no answer is a caller tako cannot place: no orchestrator role, no
spawning master left. Those tasks are still filed, but the reply has nowhere to go and
the delivery is recorded as `failed` with a "宛先不明" reason instead of being handed to
an unrelated master. If you need to file from such a place — a bare shell, a script —
call it with `TAKO_ORCHESTRATOR_ROLE=master:<profile>` so the reply has an address.

<!-- #1477: no-investigate の偵察 3 手順を guide `spawning` へ移した。本文は原文そのままで、 -->
<!-- 節を分けるための見出し 1 行だけが新規なのでここへ宣言する。 -->

## Scout Workers: Reconnaissance Before a Real Task
