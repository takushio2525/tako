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
