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
with its reason: a non-default port, `ProxyJump` / `ProxyCommand` / `HostName`
overrides, a one-shot remote command, `-N`, or a subsystem call.

- **A host whose config sets a non-default `Port` will not appear by itself**, even in
  the pane tako opened for it — tako passes `-p <port>` on that command line, and the
  detector treats any non-22 port as possibly another machine (measured: both the pane
  from `open` and the pane from `tako_open_remote` came back under `skipped` with that
  reason). For those hosts, open the folder explicitly with action `open`.
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
