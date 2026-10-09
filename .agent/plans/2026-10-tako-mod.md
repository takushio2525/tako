# tako mod: Claude Code の mod 連携の設計（エピック #1877 / S0・S7）

> スライス（S1〜S6・S7-0〜S7-7）の worker がこれを読んで実装に入れる粒度で書く。方針（やること 1〜5）の
> 正本は #1877 の本文と追加依頼（2026-10-09 の 1 / 2 = §9）。ここに書くのは「実物で確かめたこと」「決めたこと」「切り方」。
> **§9（S7）は §0〜§7 の一部を見直した**。食い違う箇所は §9 が正。
> 試作は Claude Code **2.1.294**（2026-10-08）の実物で行った。mod の API は early access で
> 版ごとに動くので、**型定義（`.claude-plugin/types/claude-code/index.d.ts`）と実物を正**とし、
> この文書の API 記述が実物と食い違ったら文書を直してから実装する。

## 0. 結論（決めたこと）

| 論点 | 決定 | 根拠の節 |
|---|---|---|
| mod → tako の通信路 | **`$.process.run` で tako CLI（`tako mod report`）を叩く** | §2 |
| tako → mod の通信路 | 当面は **report の応答に相乗り**（+ 1 秒 flush / 15 秒 heartbeat）。常時 push は S6 で `$.process.spawn` + 長ポーリング CLI | §2.3 |
| Claude Code の中からの tako 操作 | ボタン・スラッシュコマンドから**既存の CLI をそのまま呼ぶ**（2 つ目の実装を作らない） | §2 / §7 S4 |
| mod の置き場と導入 | tako が `<data_dir>/claude-mod/tako/` へ展開し、**ペインの env `CLAUDE_CODE_PLUGIN_DIRS` に足す**。Claude Code の設定ファイルは 1 バイトも書かない。**S7 で見直し**: 加えて `tako setup` が使っている設定 dir ごとに `skills/tako/` へ管理印つきの写しを置く（settings 系のファイルは書かない） | §3 / **§9.4** |
| 描画の主（S7） | tako の GUI 側で描いていたコマンドカード・使用制限 / ctx のバー・チャット風の表示を **mod が Claude Code の中で描く**。mod が効いていないペインは今の tako 側の表示のまま | **§9.3 / §9.6** |
| 定型の UI 設定（S7） | ボタン・表示項目・並び・色は `<data_dir>/claude-mod/ui.json`（スキーマ・検証・#916 の移行）。setup / CLI / MCP の同じ口で選んで変える。mod は報告の応答で受け取る | **§9.7** |
| 対応する Claude Code の版 | **下限 2.1.294**（実測した版）。下限未満・不明ならその claude には注入しない | §1.6 / §3.4 |
| 落ち方 | mod の報告が**新鮮な間だけ**一次ソース。無い・古い・止まったら今の画面 / transcript 読み取りへ落ちる | §6 |
| MATRIX | 能力ごとに新しいマスを、**claude 側の実装が入るスライスの PR で**足す（claude 列は常に Supported = 番犬 `claudeは基準系なので全て対応済み`） | §4.4 |

## 1. 試作で確かめた実物（Claude Code 2.1.294）

### 1.1 試作の構成

- mod `tako-probe`（使い捨て。製品コードには入れていない）を scratchpad に置き、
  **隔離 GUI**（`TAKO_ISOLATED=1`・仮想ディスプレイ `tako-vd`・インストール済み v0.8.27）の
  ペインの中で `claude --plugin-dir <scratch>/mods/tako-probe --model haiku` を起動した。
  本番の tako・本番の claude 設定には繋いでいない（§1.9 で後片付けまで確認）
- mod がやったこと: `session.start` / `turn.start|step|complete` / `classic.PermissionRequest` /
  `classic.Notification` / `tool.call` / `session.append` の値を JSON Lines へ記録 → AbovePrompt の
  帯に ctx% と 5h / 7d → 帯のボタンと `/tako-probe` から CLI・MCP・HTTP の 3 経路で tako を呼んで
  遅延を測る → サイドバーのペイン（`$.ui.open`）に同じ値を出す
- 試作のソースの要点は §1.8

### 1.2 `$.session.usage()` の実値

起動直後（API 応答がまだ 1 回も無い）:

```json
{"startedAt":1791452829561,"context":{"window":1000000},"rateLimits":[],"cost":{"usd":0}}
```

1 ターン後（haiku で「ok」と答えさせただけ）:

```json
{"startedAt":1791452829561,
 "context":{"tokens":59426,"window":1000000,"percent":6},
 "rateLimits":[{"kind":"five_hour","percentUsed":3,"resetsAt":"2026-10-08T14:30:00.000Z"},
               {"kind":"seven_day","percentUsed":2,"resetsAt":"2026-10-10T13:00:00.000Z"}],
 "cost":{"usd":0.00698652}}
```

- **最初の API 応答まで `tokens` / `percent` は欠け、`rateLimits` は空配列**（0 埋めではない。
  型の doc の「One the engine does not have is left out, never zeroed」どおり）。
  tako 側は「欠け = 未観測」として扱い、0% と表示しない
- `percent` は tako の `ctx_usage::used_percent` と同じ式（`59426 / 1000000` の四捨五入 = 6）。
  分子 `tokens` は直前の `turn.complete.usage` の `input + cache_read + cache_creation`
  （`2 + 25792 + 33632 = 59426`）と一致
- **`window` を claude 自身が答える**。#1021 で「`context_window_size` はどこにもファイルとして
  残っていない」ので `declared_context_window` が実測できた族しか返せなかった問題が、
  mod 経由なら原理的に消える（haiku 5.5 で 1,000,000 を実測）
- `cost.usd` は CLI では常にある（master の要約にあった「cost 欄があるか」は**ある**で確定）

### 1.3 ターン・権限待ち・質問待ち

1 ターンの流れ（実値。`turnId` は省略）:

```json
{"kind":"turn.start","data":{"text":"Reply with exactly one word: ok"}}
{"kind":"turn.step","data":{"index":0,"model":"claude-haiku-5-5","effort":"medium","messageCount":15}}
{"kind":"turn.complete","data":{"answer":"ok","durationMs":1193,"isAborted":false,"reason":"answer",
  "usage":{"input_tokens":2,"output_tokens":4,"cache_read_input_tokens":25792,
           "cache_creation_input_tokens":33632,"model":"claude-haiku-5-5"}}}
```

権限ダイアログ（`--settings '{"permissions":{"ask":["Bash"]}}'` で強制。ユーザー設定の許可ルールが
あると `touch` でもダイアログは出ない）の時系列:

| 経過 | イベント | 中身（抜粋） |
|---|---|---|
| +0 ms | `turn.start` | |
| +1,391 ms | `classic.PermissionRequest` | `tool_name: "Bash"`・`tool_input: {command, description}`・`permission_mode: "default"`・`effort: {level: "medium"}`（ほかに `session_id` / `transcript_path` / `cwd` / `prompt_id`） |
| +7,403 ms | `classic.Notification` | `notification_type: "permission_prompt"`・`message: "Claude needs your permission"` |
| +23,244 ms | `tool.call` の結果 | Esc で拒否 → `isError: true` |
| +23,260 ms | `turn.complete` | `reason: "answer"` |

- **ダイアログの表示と同時に `classic.PermissionRequest` が来る**。今の画面の文言パース
  （`claude_tui::detect_choice_dialog`）より早く・確実で、ツール名と引数まで構造で取れる
- `classic.Notification` は約 6 秒遅れて来るので、状態の一次ソースには使わない（補助）
- **AskUserQuestion も `classic.PermissionRequest` を出す**（`tool_name: "AskUserQuestion"`・
  `tool_input: {questions}`。`tool.call` の開始から +15 ms）。権限待ちと質問待ちは `tool_name` で分ける
- **ダイアログ・質問の表示中は AbovePrompt の帯が隠れる**（画面の抜粋で確認）。
  帯に「承認待ちです」を出す設計は成り立たない（出すならステータス行かトースト）

**追記（S1 #1879 → S2 #1880 の実測）: 組織アカウントでは classic 系が届かない**

- 2.1.294 の**組織アカウント**（`~/dev/tako` の direnv が `CLAUDE_CONFIG_DIR` を組織用の設定 dir へ
  向ける環境）では、`classic.PreToolUse` / `classic.PermissionRequest` / `classic.Notification` が
  mod へ **1 つも届かない**（`on('*')` で観測して 0 件）。関数フック（`tool.call` / `tool.check` /
  `turn.*`）は届く。個人アカウントでは同じ版で 3 つとも届く
- S1 はこれを受けて、PermissionRequest を正としつつ、届かないときは AskUserQuestion = `tool.call` の
  開始で `question`、ask ルールに当たった `tool.check` と ExitPlanMode = `permission` で拾う。
  どちらの環境かは報告の `classic_events` で分かる
- **拾えないもの**: ルールの無い `ask`（default モードのダイアログ）は auto モードの分類器が黙って
  通しうるので `tool.check` の判定だけでは「ダイアログが出た」と言えず、拾わない。S2 は
  `classic_events: false` のとき画面のダイアログ検出と突き合わせ、食い違いを `worker_status` の
  `warnings` に出す（画面を正にする。respond も画面を読む）
- **effort**: classic 系（PermissionRequest / Stop）だけが運ぶと S1 では欠けていたが、`turn.step` の
  イベントが毎ステップ `effort`（文字列。上の 1 ターンの流れの `"effort":"medium"`）を持つので S2 で
  そこから拾う。`turn.step` は**ストリームのフック**で、`async function*` で受けて
  `return yield* next(e)` で素通しする形でしか書けない（普通の関数で登録すると validate が
  `turn.step streams, so it takes async function* ($, e, next)` で落とす）。`.catch` も同じ形

### 1.4 会話の行（チャット表示の材料）: `session.append`

会話に行が足されるたびに 1 回ずつ来る。本文は記録せず形だけを取った（2 ターン分）:

| door | origin | type / role / name | ブロック |
|---|---|---|---|
| `prompt` | `{kind: "composer"}` | user / user | text（利用者の発話。`turn.start` より前） |
| `attachment` | `{kind: "engine"}` | attachment / user / `total_tokens_reminder` | text |
| `response` | `{kind: "model", model}` | assistant / assistant | **1 行 1 ブロック**（`tool_use` / `thinking` / `text` が別々の行） |
| `hook-context` | `{kind: "hook", event: "PreToolUse"}` | attachment / user / `hook_success` | なし |
| `tool-result` | `{kind: "tool", tool: "Bash"}` | user / user | tool_result |
| `notice` | `{kind: "engine"}` | system / - / `stop_hook_summary`・`turn_duration`・`informational` | なし or text |

- `next(e)` の答えに transcript 上の `uuid` が載る。**本文は transcript が既に持っている**ので、
  mod から tako へ本文を流す必要は無い（S5 は「uuid + door + origin の通知」と transcript の
  既存パーサの組み合わせにする。§7 S5）

### 1.5 描画とボタン

画面の抜粋（隔離 GUI のペイン 324 桁。1 ターン後に `/tako-probe` を 1 回実行した直後。空白は詰めた）。
プロンプトの上の帯:

```text
tako pane 1 | claude-haiku-5-5 | ctx 6% (59426/1000000) | five_hour 3% seven_day 2% | idle   [-]
[ tako list (CLI) ][ tako list (MCP) ][ split right ]
CLI 7ms exit=0 panes=1 | MCP(tako) 27ms err=false bytes=1158
─────────────────────────────────────────────
❯
```

右のサイドバーのペイン（`│` より右。画面の上端から）:

```text
│                                    ✕
│pane: 1
│model: claude-haiku-5-5
│ctx: 6%
│five_hour: 3% reset 2026-10-08T14:30:00.000Z
│seven_day: 2% reset 2026-10-10T13:00:00.000Z
│CLI 7ms exit=0 panes=1 | MCP(tako) 27ms err=false bytes=1158
```

- 帯（`ui.render` の `AbovePrompt`）は 21 行のペインでは `↓ 2 more` に畳まれ、68 行で全部出た。
  **帯は 1 行に収める**のが前提（S3 の受け入れ条件）
- `$.ui.open` のペインは、コマンドから開くと右のサイドバーに出た（324 桁。型の doc では
  頼まれずに開くのは 144 桁以上のときだけ）
- **帯のボタンは ctrl+x → Tab でフォーカスが入り、Enter で押せる**。「split right」を押すと
  `$.process.run([tako, "split", "--right"])` が走り、隔離 GUI に pane 2（`origin: cli`）が生えた。
  = **Claude Code の中から tako のペイン操作を呼べる**ことを実機で確認
- ユーザーが自分の statusLine を設定していると、claude の下段に ctx / 5h / 7d が既に出る
  （この開発機がそう）。statusLine を持たない一般ユーザーには出ないので、帯の情報は重複しても
  意味があるが、**既定は控えめ**（§7 S3）

### 1.6 読み込み方と版

| 確かめたこと | 結果 |
|---|---|
| `--plugin-dir <dir>` | 読み込まれる。確認ダイアログなし |
| env `CLAUDE_CODE_PLUGIN_DIRS=<dir>`（フラグなし） | **同じく読み込まれる**。確認ダイアログなし。ペインの `TAKO_PANE_ID` も見える |
| 読み込み中に mod のファイルを書き換える | 両方とも**ホットリロードする**（`session.start` が再発火。モジュール変数は消え、`$.state` は残る） |
| 読み込むたびに書かれるもの | mod フォルダの中の **`.claude-plugin/types/`**（型定義一式）と、`<CLAUDE_CONFIG_DIR>/plugins/data/<name>-inline/`（空 dir） |
| 旧版 2.1.280（mod の正式導入 2.1.287 より前） | `tako-probe: hooks module did not load: … $ is passed to "update", imported from "claude-code": $ is followed only into a function declared in this same file, never across an import` を transcript に 1 行出して**読み飛ばし、セッションは普通に動く** |
| Claude Code の設定ファイル | `settings.json` 類の更新時刻は試作の前後で不変（9/25 以前のまま） |

ここから決まること:

- **mod を署名済みの `.app` の中に置いて直接読ませてはいけない**（読み込みのたびに
  `.claude-plugin/types/` が書かれ、bundle が改変される）。展開先は `<data_dir>` 側（§3.2）
- 旧版では致命的にはならないが、利用者の画面に 1 行出る。**版の下限を決めて、満たさない
  claude には注入しない**（§3.4）。下限は**実測した 2.1.294**。2.1.287〜2.1.293 は未実測なので
  下げるなら実測してから
- 試作の claude は隔離ペインでも `~/.claude` を使っていた（ログインシェルが
  `CLAUDE_CONFIG_DIR` を明示設定していた）。**env 方式は設定 dir がいくつあっても関係ない**（§3.1）

### 1.7 ドキュメント・Issue の要約との食い違い

| 要約・想定 | 実物 |
|---|---|
| `$.mcp.connect()` で tako の MCP に繋がる | `connect` が繋げるのは **mod 自身の manifest（`mcpServers`）に書いた server だけ**。同じコマンドがユーザー設定に既にあると、その名前（`tako`）で答えて**二重には起動しない**（プロセス木で `tako mcp serve` が 1 本だけを確認）。**`--strict-mcp-config` のセッションでは `{isConnected: false, reason: "policy", message: "This session runs with --strict-mcp-config, which connects no plugin servers."}` で断られ**、`$.mcp.call` も `no connected MCP tool "tako_list_panes" on a server named "tako"` で失敗する |
| `$.session.usage()` の値 | §1.2 のとおり。初回応答までは欠ける・cost はある |
| フックは 10 秒 | 型の doc は「フックごとに予算がある。**飛行中の `next` / `$` 呼び出しは予算に数えない**（`$.clock.sleep` は数える）」。10 秒という数値は今回確かめていない |
| （想定外）`$` を渡す補助関数 | **ファイル最上位の関数宣言でなければならない**（`register` の中のクロージャへ `$` を渡すと validate が `$ is passed to "rec", which is not a function declared at the top of this file` で落とす）。2.1.280 は `claude-code` から import した `update($, …)` も拒んだ |
| （想定外）権限の状態 | 帯はダイアログ表示中に隠れる。AskUserQuestion も PermissionRequest を出す（§1.3） |
| （想定外）`$.http.fetch` | `socketPath`（Unix ソケット越しの HTTP）も取れる。tako の IPC は 1 行 JSON で HTTP ではないので直接は話せないが、内蔵 HTTP MCP（`TAKO_MCP_URL`）にはセッション確立なしで `tools/call` が通った |

### 1.8 試作のソースの要点（抜粋）

```tsx
// 補助関数は最上位に置く（validate の規則。§1.7）
async function viaCli($: EngineInterface) {
  const t0 = await $.clock.now()
  const r = await $.process.run([TAKO, 'list'], { timeoutMs: 5000 })
  const ms = (await $.clock.now()) - t0
  // … `tako list` の JSON からペイン数を数えて記録
  return `CLI ${ms}ms exit=${r.exitCode}`
}

export const register: Register = on => {
  on('turn.complete', async ($, e, next) => {
    const done = await next(e)
    await rec($, 'turn.complete', { ...e, answer: e.answer.slice(0, 80) })
    await rec($, 'session.usage@complete', await refresh($, 'idle')) // $.session.usage() → $.state
    return done
  })
  on('classic.PermissionRequest', async ($, e, next) => {
    await rec($, 'classic.PermissionRequest', e)
    await update($, snap, prev => ({ ...prev, turn: 'permission' }))
    return next(e)
  })
  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const s = await read($, snap)
    if (e.props.hasSurvey) return next(e)
    const { Box, Button, Text } = $.ui.resolve(e)
    return (
      <Box flexDirection="column">
        <Text dimColor>tako pane {s.pane ?? '-'} | {s.model ?? '-'} | ctx {s.ctxPercent ?? '-'}% | …</Text>
        <Button key="split" label="split right"
          onPress={async () => { await $.process.run([TAKO, 'split', '--right'], { timeoutMs: 5000 }) }} />
      </Box>
    )
  })
}
```

`claude plugin validate` は `hooks:` / `calls:` / `env reads:` / `state writes:` を列挙し、
`tool.call` などに `.catch` が無いことを `gating hook without .catch` として申告した（§5 の規約で塞ぐ）。

### 1.9 後片付け（実施済み）

隔離 GUI は起動時に控えた pid だけを止めた（名前一致の kill はしていない）。tmux の残骸なし
（隔離 GUI は永続化 OFF）。試作が作った `~/.claude/plugins/data/tako-probe-inline/`（空）は
`rmdir` で消した。mod と記録は scratchpad の中だけ。

## 2. 通信路の比較と決定

### 2.1 比較（遅延は隔離 GUI での実測。n = 6 の中央値）

| 経路 | 遅延 | tako が落ちているとき | 複数セッション | tako 再起動をまたぐ tmux worker | セッションの MCP 構成 | 依存するもの |
|---|---|---|---|---|---|---|
| **A. `$.process.run` → tako CLI** | **8.5 ms**（8〜17） | 即 `exit=1` + 理由 1 行（10 ms 未満） | env の `TAKO_PANE_ID` で自動に分かれる（pane 1 / 2 で実測） | **繋がる**。既存プロセスの `TAKO_TOKEN` が古くても CLI の `control.json` フォールバック（FR-2.2.9）が吸収する | 無関係 | CLI の絶対パス（env で渡す） |
| B. `$.mcp.call` → tako MCP（stdio ブリッジ） | 18.5 ms（15〜43。connect は 2 ms） | 呼び出しが失敗 | 同上（ブリッジが env を読む） | ブリッジが IPC へ繋ぐので A と同等の見込み（未実測） | **依存する**。`--strict-mcp-config` で `policy` 拒否を実測。ユーザーが tako の MCP を外しても不通 | セッションの MCP 構成 |
| C. `$.http.fetch` → 内蔵 HTTP MCP | **1.5 ms**（1〜8） | 閉じたポートで約 10 ms で失敗 | `X-Tako-Pane` ヘッダで分かれる | **壊れる**。tako が再起動すると `TAKO_MCP_URL`（ポート）と `TAKO_TOKEN` が変わるが、tmux の `update-environment` は**既存プロセスには届かない**（`tmux_backend.rs` の注記）。直すには mod 側に `control.json` の再発見を書く = CLI の解決ロジックの 2 実装目 | 無関係 | env の URL / トークン |
| D. ファイル経由（mod が `$.fs.write`、tako が 2 秒 tick で読む） | tick 依存（最大 2 秒。未実測） | 書けるが読まれない（復帰後に読める） | ペインごとのファイル | 繋がる（data dir 固定） | 無関係 | tako 側の監視・掃除・競合処理。双方向にはもう 1 本要る |

### 2.2 決定: A（CLI）

- **dispatch に 1:1 で載る経路そのもの**なので、AI（MCP / CLI）と mod が同じ操作セマンティクスを
  通る（設計原則 5・開発不変条件）。操作を足すたびに mod 用の口を別に作らない
- **tako の再起動をまたいで生き残る worker**（tmux の中の claude）で繋がり続けるのは A だけ。
  worker の監視が tako の主用途なので、ここで壊れる経路は主経路にできない
- セッションの MCP 構成（strict / 無効化 / 組織ポリシー）に左右されない
- 8.5 ms は「1 秒に 1 回以下」の報告には無視できる（§5 の flush で回数を抑える）

**代替案とトレードオフ**: C は 5 倍速いが、再起動で壊れる穴を塞ぐと CLI の解決ロジックを mod に
写すことになる。報告頻度が上がって CLI の起動コストが効いてきたら（目安: 1 セッションあたり
秒間 5 回超）、そのとき「C + 失敗時だけ A へ落ちる」を検討する。B は MCP 構成に依存するので採らない。
D は双方向にできず掃除の責務が増えるので採らない。

### 2.3 tako → mod（逆向き）

- **当面（S1〜S5）**: `tako mod report` の応答に tako 側のスナップショット（このペインの名前・
  タブ・worker の要約・tako 側の設定）を載せて返す。mod は報告のたびに最新を受け取る。
  アイドル中も 15 秒の heartbeat で更新される
- **常時 push が要る機能**（チャット入力を `$.prompt.submit` で送る・権限ダイアログへ
  mod 経由で答える・tako から `/compact` を頼む 等）は S6 で別設計にする。候補は
  `$.process.spawn([tako, "mod", "watch"])` の長命な子 + 長ポーリングの dispatch。
  IPC は今は要求 / 応答だけなので、**サーバー発信の仕組みを足すかどうか**が S6 の判断点

## 3. 置き場・同梱・自動導入

### 3.1 比較

| 方式 | 設定ファイルへの書き込み | `CLAUDE_CONFIG_DIR` が複数（univ 等） | tako の外の claude | 更新 | 利用者が止める手段 |
|---|---|---|---|---|---|
| **A. ペインの env に `CLAUDE_CODE_PLUGIN_DIRS`（推奨）** | **なし** | **関係ない**（env なので設定 dir を問わない。§1.6 で実測） | 読み込まれない（tako のペインだけ） | 展開先を書き換えるとホットリロード（実測） | `tako mod off`（tako の設定） |
| B. marketplace として登録 + `claude plugin install`（user スコープ） | **設定 dir ごとに** `enabledPlugins` / `extraKnownMarketplaces` を書く | 設定 dir の数だけ入れ直す・どれに入れたか追跡が要る | 読み込まれる → mod 側で休眠が要る | フォルダ marketplace は `/reload-plugins` か次の起動で反映 | `/plugin` の UI |
| C. tako が起動する claude にだけ `--plugin-dir` を付ける | なし | 関係ない | 読み込まれない | 同 A | `tako mod off` |

**決定: A**（**S7 で見直し**: 2.1.294 には `<設定 dir>/skills/<名前>/` に置いた plugin を読む口があり、
settings 系のファイルを書かずに「この PC の claude へ入れる」ができると実測した。A は残したまま、setup が
そこへ写しを置く形を足した = §9.4。B も一時の設定 dir で 1 周して比べた）。
ゼロコンフィグ（設計原則）と「本番設定を書き換えない」を両立できるのは A だけ（S0 時点の比較。S7 の C' も両立する）。
C は master / solo / spawn は覆えるが、**利用者がペインで自分で打った `claude`** を覆えない。
B は利用者の Claude Code 設定へ tako が書き込み続けることになり、設定 dir の追跡と後始末が要る。

**代替案とトレードオフ**: A の弱点は「`/plugin` の UI に出ない（＝ Claude Code 側で止める手段が
無い）」こと。tako 側の `tako mod off` と、`tako mod status` で「どのペインで読み込まれているか」を
見せることで補う。B を opt-in で足す余地は残す（tako の外の claude にも帯を出したい人向け）が、
S1〜S6 には入れない。

### 3.2 リポジトリ内の置き場と展開

- ソース: `crates/tako-core/claude-mod/`（`shell-integration/` と同じ並び）
  - `.claude-plugin/plugin.json`（`name: "tako"`・`version` は tako のワークスペース版と一致させる）
  - `hooks/hooks.json`（`{"modules": ["./register.ts"]}`）・`hooks/register.ts`（ビルド工程なし。
    Claude Code 自身が TS を読む）・`types/index.d.ts`（`$.state` の契約）・`tests/*.test.ts`
  - 開発機で `--plugin-dir crates/tako-core/claude-mod` を使うと `.claude-plugin/types/` が
    書かれるので **`.gitignore` に足す**
- 展開: `tako_core::claude_mod::install()`（`shell_integration::install()` と同じ型。
  `include_str!` で埋め込み → `<data_dir>/claude-mod/tako/` へ書く）
  - 発火は **GUI 起動時** と **`tako setup`**（#1500 のゼロタッチの段に 1 行足す）
  - 冪等: 中身が同じなら書かない（書くとホットリロードが無駄に走る）。違えば一時 dir に書いてから
    差し替える（途中の状態を読ませない）
  - `.claude-plugin/types/` は Claude Code が書くものなので消さない・比較から外す
- **`.app` の中には置かない**（§1.6。読み込みで bundle が改変される）

### 3.3 env の注入

- 注入する変数: `CLAUDE_CODE_PLUGIN_DIRS`（展開先）と `TAKO_CLI`（実行中の tako の CLI の
  絶対パス。mod が `$.process.run` に渡す。PATH 先頭の古い `target` を掴む #432 の罠を避ける）
- 既存の値があれば**上書きせず連結**する（区切りは macOS / Linux が `:`、Windows が `;`）
- 直接ペイン: `spawn_session` の env へ足す。tmux ペイン: `backend::session_pinned_pairs` に
  `shell_integration::env()` と同じ形で足す（`new-session -e`）。tako の再起動後に attach した
  セッションで**新しく起動する** claude にも届くよう、tmux の `update-environment`
  （`tmux_backend.rs` の `set -g update-environment 'TAKO_SOCKET TAKO_TOKEN TAKO_MCP_URL'`）にも足す
- tako 自身が app プロセスから起こす `claude -p`（自動リネーム `autorename.rs` 等）には**注入しない**
  （ペインの env だけ。自動リネームは `--strict-mcp-config` で軽く走らせている経路なので、mod を
  載せて重くしない）

### 3.4 版の下限と止め方

- `MIN_CLAUDE_FOR_MOD = 2.1.294`（実測した版。下げるなら実測してから）
- 判定は既存の版抽出（`stale_binary` の `claude --version`。起動は `agent_probe::run` を通す =
  #1261 の門番）を**控えて使う**。ペインを作るたびに `claude --version` を起こさない
  - 控えが無い（未判定）ときは注入しない。背景で 1 回だけ判定し（#1503 の教訓でタイムアウト付き）、
    次に作るペインから効く
- 止め方: tako の設定 `claude_mod`（既定 `true`）。`tako mod off` / `on` と MCP `tako_mod` から変える。
  **settings.json のフィールド追加は serde の default で旧ファイルがそのまま読める**（#916 の
  `migration_registry` テストが落ちたら、PR 本文でそう明示する）
- `claude --safe-mode` / `--bare` / `disableAllHooks` / 組織の `allowManagedModsOnly` では
  mod が読まれない → §6 の落ち方へ（tako 側では「報告が来ない」として見える）

## 4. tako 側のデータと操作（S1 で作るもの）

### 4.1 dispatch

`Request::Mod`（1 本）に action を持たせる:

| action | 中身 | CLI | MCP |
|---|---|---|---|
| `status` | 展開先・版・注入の有無・ペインごとの最終報告（鮮度つき） | `tako mod`（引数なし = status。#322 の最簡形） | `tako_mod` の `action=status` |
| `on` / `off` | 設定 `claude_mod` の切替 | `tako mod on` / `off` | `tako_mod` の `action=on|off` |
| `report` | mod からの状態報告（§4.2）。応答に tako 側のスナップショット（§2.3） | `tako mod report`（stdin に JSON。`--help` には出さない） | **出さない**（理由は下） |

- `report` を MCP カタログに載せないのは、AI が叩くと**自分の状態を偽って注入できるだけ**で、
  AI にとっての等価物（読む側の `tako_orchestrator_self` / `tako list` / `tako mod status`）は
  別にあるから。この例外理由は S1 で `requirements.md` の FR-2.42 に書く
- MCP カタログの増分は `tako_mod` 1 本（目安 +600 B）。**カタログは v0.8.27 の時点で
  197,141 / 204,800 バイト（残り約 7.6 KB）**なので、説明文は 1〜2 行に抑え、S1 の PR で
  `tako context-budget` の前後の値を本文に書く

### 4.2 報告の形（`schema: 1`）

```json
{
  "schema": 1,
  "mod_version": "0.8.28",
  "claude_version": "2.1.294",
  "session_id": "…",
  "at": 1791452867009,
  "model": "claude-haiku-5-5",
  "effort": "medium",
  "context": {"tokens": 59426, "window": 1000000, "percent": 6},
  "rate_limits": [{"kind": "five_hour", "percent_used": 3, "resets_at": "2026-10-08T14:30:00.000Z",
                   "observed_at": 1791452867009}],
  "cost_usd": 0.00698652,
  "turn": "idle | busy | permission | question",
  "pending_tool": "Bash",
  "last_turn": {"duration_ms": 1193, "reason": "answer"},
  "ended": false
}
```

- 呼び出し元のペインは CLI の既定（`TAKO_PANE_ID`）で決まる。orphan 復元で番号が変わった
  ペインは dispatch の `stale_pane_map`（#210）がそのまま解決する
- **会話の本文・プロンプト・ツールの引数は載せない**（AGENTS.md の絶対ルール。`pending_tool` は
  ツール名だけ）。`rate_limits` は**アカウント単位**の値なので、tako 側で `CLAUDE_CONFIG_DIR`
  （mod が env から読んで渡す。ログには出さない）ごとに束ね、最新の `observed_at` を採る
- 保存は **GUI のメモリだけ**（永続化しない = マイグレーション対象の永続ファイルを増やさない）。
  ペインが閉じたら捨てる

### 4.3 鮮度

- mod は変化があれば最大 1 秒に 1 回、変化が無くても 15 秒に 1 回報告する（§5）
- tako は**最終報告から 45 秒**を過ぎた報告を「無い」とみなす（heartbeat 3 回分の取りこぼしを許す）。
  `ended: true`（`session.end`）を受けたら即座に捨てる
- 鮮度の判定は純関数（`tako_core` 側。2 秒 tick で呼ぶので軽く）

### 4.4 MATRIX（`agent_support`）

能力ごとにキーを分け、**claude 側の実装が入るスライスの PR で**足す（claude 列は Supported 必須の番犬があるので、先に Pending で置くことはできない）:

| キー（案） | 足すスライス | claude | codex / agy | local |
|---|---|---|---|---|
| `claude_mod_state` | S1 | Supported | Pending（#1885） | `local_pending_first_class()` |
| `claude_mod_band` | S3 | Supported | 同上 | 同上 |
| `claude_mod_actions` | S4 | Supported | 同上 | 同上 |
| `claude_mod_chat` | S5 | Supported | 同上 | 同上 |

- codex / agy を **Unsupported にしない**。「上流に同等の拡張点が無い」はまだ調べていないので
  Pending（#1885）にする（`AgentSupport::Unsupported` の doc の禁止事項）
- 既存のマス（`master_ctx_percent` / `worker_limit_detect` / `worker_permission_dialog` /
  `worker_status_detect`）は claude 列が既に Supported なので変えない。S2 で根拠欄に
  「mod があれば一次ソース、無ければ画面」と書き足す
- OS 軸（`platform::support`）: Windows は Pending（#1886。#467 との接点。区切り `;` の単体テストは
  S1 で入れるが、実機の Claude Code で mod が動くかは未実測）

## 5. mod の作りの規約（S1 で `crates/tako-core/claude-mod/` に入れるときに守る）

- **観測だけで、判断を奪わない**: `tool.call` / `classic.PermissionRequest` などゲートになる
  イベントのフックは、必ず `next(e)` へ流し、`.catch(($, e, next) => next(e))` を付ける
  （mod が壊れても tool call と権限ダイアログを止めない）。`tool.check` に答えない・権限を
  勝手に許可しない
- **フックの中で CLI を待たない**: フックは `$.state` とモジュール変数を更新して dirty を立てる
  だけにし、`session.start` で張った `$.clock.every(1000)` の flush が CLI を叩く
  （変化が無ければ 15 秒ごとの heartbeat だけ）。報告が詰まってもターンが遅れない
- **tako の外では休眠**: `TAKO_PANE_ID` か `TAKO_CLI` が無ければ何も呼ばない。CLI は接続不能時に
  `control.json` へフォールバックするので、tako の外の claude から叩くと**別の tako インスタンスへ
  繋がりうる**。env の有無で入口を締める
- **失敗で騒がない**: CLI の失敗（tako が落ちている等）は `$.ui.log(…, { to: "debug" })` だけ。
  トーストやトランスクリプトに毎回出さない
- `$` を渡す補助関数はファイル最上位の関数宣言にする（§1.7）
- サブエージェントの `turn.*`（`agentId` あり）は ctx の計算に入れない（#1021 の
  `isSidechain` 除外と同じ理由）
- 検証: `claude plugin validate` が通ること・`claude plugin test` の `*.test.ts` が terminal と
  desktop の両 surface で通ること。`scripts/` にまとめる（`claude` が無い CI では飛ばして
  「未実測」と出す）。**夜間リリースの前段で毎晩回す**（`scripts/check-claude-mod.sh`。#1892。
  運用は `.agent/release.md`「tako mod の検査」）。CI に `claude` を入れるかは [提案]

## 6. 落ち方（mod が無い・古い・止まったとき）

| 値 | 優先順（左が先） | 備考 |
|---|---|---|
| ctx%・窓 | **mod（新鮮）** → 画面 → transcript → none | `claude_ctx::resolve` の先頭に足す。`ctx_source: "mod"`。画面とも突き合わせて差を `warnings` に出す（#1021 の自己検証を続ける） |
| 使用制限（5h / 7d の % と reset） | **mod（新鮮）** → 既存の経路 | 停止の判定は**画面のまま**（#813 の安全条件。`limit_stop::detect_limit_stop_with` の `LimitHint` へ mod の `resets_at` を渡す = #985 の codex と同じ型） |
| ターン状態・権限待ち・質問待ち | **mod（新鮮）** → 画面（`claude_tui` / `orchestrator::wait`） | 応答（`tako orchestrator respond`）は当面キー送出のまま（S6 で再検討） |
| モデル・effort | **mod（新鮮）** → transcript | |
| チャット表示 | **mod の通知 + transcript** → transcript のポーリング + 画面ミラー | S5 |

「新鮮」は §4.3。落ちた理由（`mod_absent` / `mod_stale` / `claude_too_old` / `disabled`）を
`tako mod status` と `ctx_reason` 等に**黙らず出す**。

**S2（#1880）で実装した形**（要件は FR-2.42.9〜12）:

- 理由のキーは `ctx_mod_reason`（ctx%）と `mod_reason`（ターン状態・使用制限）。`ctx_reason` は
  「ctx% が null の理由」のまま変えていない（意味を混ぜると、画面から取れたときに理由が入って
  読み手が null 判定を誤る）。語彙は `tako mod` の行の `reason.code` と同じ（+ `mod_no_usage` /
  `legacy_env`）。引き当ては `tako_core::claude_mod::lookup` の 1 本
- ~~使用制限の束ね方は「最新の `observed_at`」ではない~~（**#1903 で最新の観測へ戻した**。下記）:
  S2 の時点の mod は heartbeat（15 秒）のたびに `$.session.usage()` を読み直して報告時刻を
  `observed_at` に打っていたので、1 時間放置したペインの古い % も「今」の時刻で届いた。そのため
  S2 は「`resets_at` が遅い（= 新しい窓）→ 同じ窓なら `percent_used` が大きい → `observed_at`」で
  選んでいた（A/B `TAKO_1903_LEGACY=1` で今も選べる）
- 停止の手がかり（`LimitHint::from_mod`）は `percent_used >= 100` の窓の解除時刻（複数なら遅い方）。
  上限に当たっていなければ手がかり無し（画面のパースのまま）= codex #985 と同じ型
- `permission` / `question` は承認・回答の後もそのツールが返るまで残る（FR-2.42.7）ので、
  `worker_status` は画面が生成中なら busy を採る（`ModTurn::status_word`）。mod と画面の
  ダイアログ検出の食い違いは `warnings` へ
- 4 経路の取得元は外から読める: `orchestrator self` の `ctx_source` / `auto_handoff_tick`（#749 の
  tick が最後に見た値）・`worker_status` の `ctx_source` / `status_source`・`tako ui-mode` の
  `chat_header`（チャットヘッダの残量バー）
- A/B: `TAKO_1877_S2_LEGACY=1`（報告は受け取るが一次ソースに使わない）。実経路テストは
  `scripts/test-mod-primary-1880.sh`、番犬は `crates/tako-control/tests/issue1880_mod_primary_watchdog.rs`

**S2 の続き（#1903）で変えたこと**（要件は FR-2.42.11 / FR-2.42.18）:

- **`observed_at` は値が変わったときだけ打つ**（`register.ts` の `stampLimits`）: 窓ごとに「% か
  `resets_at` が前と違えば今の時刻、同じなら前の時刻」。窓ごとの時刻は `$.state` の `limitsSeen` に置く
  （モジュール変数だとホットリロード = tako の更新で mod が書き換わるたびに全窓が「今」の観測になる）。
  鮮度は従来どおり tako の受信時刻（`StoredReport::received`）で測るので、heartbeat の意味は変わらない
- **束ね方を本節の表どおり「最新の観測」へ**: 窓の種類ごとに `observed_at` が新しい → 同時刻なら
  `resets_at` が遅い → % が大きい。S2 の順と違いが出るのは「同じ窓の % が下がった」とき（上限の
  引き上げ・早めのリセット）で、S2 の順は放置したペインの古い大きい % を採り続けていた
- **ステータスバーの 5h / 7d も mod を先に見る**: フォーカス順で最初に mod の使用制限が引けたペインの
  アカウントの値（`account_rate_limits` の 1 実装）。5h / 7d の窓が無ければ画面の値。取得元は
  `tako limit-service --refresh` の `claude.source`
- 検査スクリプトは 1 実装（`scripts/check-claude-mod.sh`）。注記の文言一致に肯定形の自己検査を足した
  （`.agent/release.md`「tako mod の検査」）。実経路テストは `scripts/test-mod-limits-1903.sh`、
  番犬は `crates/tako-control/tests/issue1903_mod_limits_watchdog.rs`

## 7. スライス

依存:

```text
S1 基盤（同梱・展開・注入・報告・status）                 #1879
 ├─ S2 一次ソース化（ctx・使用制限・状態）                 #1880
 ├─ S3 Claude Code の画面に出す（帯・ペイン）              #1881
 ├─ S4 Claude Code から tako を操作（コマンド・ボタン）    #1882
 ├─ S5 チャット表示を構造化通知で                          #1883
 └─ Windows 実機確認                                       #1886
S6 tako → mod の push（S2〜S5 の後。[提案] を含む）        #1884
調査: codex / agy の同等の拡張点（いつでも）               #1885
S7（描画の主を mod へ・setup で導入・定型の UI 設定）       #1958〜#1965 = §9.8
```

**S7 で変わったこと**: S4（#1882）はボタンの語彙（ui.json の `tako`）の正本と `/tako <操作>` に絞り、
S5（#1883）は #1964 に置き換えた。詳細は §9.8。

S2〜S5 は S1 の後なら並行できるが、`progress.md` の衝突（#1228）を避けるため着地は 1 本ずつ。

### S1 基盤（#1879）

- やること: §3（展開・env 注入・版の下限・`claude_mod` 設定）+ §4（`Request::Mod`・
  `tako mod` / `tako mod report`・MCP `tako_mod`・報告の保持と鮮度）+ §5 の規約で書いた mod の
  最小版（状態を集めて報告するだけ。描画なし）+ FR-2.42 + MATRIX `claude_mod_state`
- 受け入れ条件:
  1. 隔離 GUI の直接ペインと tmux ペインの両方で、利用者が素の `claude` を打つだけで
     `tako mod` に そのペインの報告（ctx・使用制限・turn・model）が載る（1 ターン以内）
  2. 権限ダイアログ・AskUserQuestion の表示中に `turn` が `permission` / `question` になる
  3. 版の下限未満（2.1.280 の実体で）と `tako mod off` では注入されない。tako の外の claude は
     影響を受けない。`claude --safe-mode` では報告が来ず、`tako mod` が理由を出す
  4. tako を止めた状態で claude が普通に動き、transcript に mod 起因の行が出ない
  5. tako の再起動をまたいで生き残った tmux の claude から報告が届き続ける（CLI フォールバック）
  6. Claude Code の設定ファイルの更新時刻が前後で変わらない
  7. A/B: `TAKO_1877_NO_MOD=1` で注入しない（同一バイナリで旧挙動）
  8. `claude plugin validate` / `claude plugin test` が通る。番犬: mod のゲートになるフックに
     `.catch` が付いていること・報告の payload に本文系のキーが無いこと

### S2 一次ソース化（#1880）

- やること: §6 の表の上 4 行。`claude_ctx::resolve` / `limit_stop` の hint / `orchestrator::wait`・
  `worker_status` / チャットヘッダの残量バー（#702）/ #749 の自動ハンドオフの tick が mod の報告を
  先に見る
- 受け入れ条件: 4 経路（`orchestrator self` / `worker_status` / #749 tick / チャットヘッダ）が
  `ctx_source: "mod"` を返す。statusLine を外した claude（画面に ctx が出ない構成）でも ctx% が
  取れる。報告を止める注入で 45 秒後に画面 / transcript へ落ちて理由が出る。使用制限の reset 時刻が
  mod の値（秒精度）になる。A/B: `TAKO_1877_S2_LEGACY=1`

### S3 Claude Code の画面に出す（#1881）

- やること: 帯（1 行。このペインの名前・タブ・worker 数と要注意の数・ctx / 使用制限は閾値を
  超えたときだけ）・`/tako` でサイドバーのペイン（詳細）・帯を隠すトグル（`$.store` に保存）。
  データは report の応答（§2.3）
- 受け入れ条件: 80 桁・144 桁・300 桁で 1 行に収まる。ダイアログ表示中に隠れても壊れない。
  `claude plugin test` で terminal と desktop の両 surface。statusLine を持つ利用者の画面で
  情報が二重にならない既定（閾値未満は出さない）。MATRIX `claude_mod_band`

**S3（#1881）で実装した形**（要件は FR-2.42.13〜17）:

- **判断は tako、詰めるのは mod**: `tako mod report` の応答の `tako.view`（`tako_core::claude_mod::BandView`）に
  ペイン名・タブ・worker と要注意（`classify_worker`）・閾値を超えた警告（`band_warnings`）・サイドバー用の
  ctx / 使用制限・閾値・トグルの中継を載せる。mod は `bodyColumns` に合わせて優先度の低い区切りから落とし
  （`fitBand`）、`wrap="truncate-end"` の 1 本の `Text` で描く。材料は `$.state`（`tako.view` /
  `tako.bandHidden`）に写し、描画が購読する（書けば描き直される・ホットリロードでも残る）
- 実画面（2.1.294・組織アカウント・隔離 tmux 21 行）の帯:
  `tako | オーケストレーターの ma… | タブ tako-wt-1881 の帯の検証… | worker 2 | 要注意 1 | ctx 6% | 7d 42% | 5h 5%`
  （閾値 0 の検証 env で区切りを全部並べた形）。80 桁ではタブ名と worker 数が落ちる。`bodyColumns` は
  端末の幅 − 5 を実測。**`ui.render` / `command.run` / `$.store` / `$.ui.open` は組織アカウントでも届く**
  （classic 系だけが届かない = FR-2.42.7 の差は帯に影響しない）
- `$.store` の実体は `<設定 dir>/plugins/store/<mod 名>_inline-<hash>.json`。**hash は mod の名前で決まり
  置き場のパスに依らない**（scratchpad の試作で置き場を変えても同じファイル名）= 隔離テストの claude も
  本番の tako mod と同じファイルを読み書きするので、実経路テストは前後で退避・復元する
- worker の数え方は右パネル orch と同じ 1 実装（`Workspace::workers_of`。orch ビューの inline の規則を寄せた）
- A/B: `TAKO_1877_S3_LEGACY=1`（応答に view を載せない）。検証用: `TAKO_1881_BAND_THRESHOLD=<%>`。
  実経路テストは `scripts/test-claude-mod-band-1881.sh`、mod のテストは `claude-mod/tests/band.test.ts`
- 残る遅れ: worker の状態の変化は**master 側の次の報告**（変化が無ければ 15 秒の heartbeat）で帯に届く。
  即時に届けるには tako → mod の push（S6）が要る

### S4 Claude Code から tako を操作（#1882）

- やること: スラッシュコマンド（`/tako split` 等。`immediate` = Claude のターン中でも即実行）と
  ペインのボタン。**中身は既存 CLI を `$.process.run` で呼ぶだけ**。右クリックメニューとの対応表:

  | ペインの右クリック（`main.rs` の `pane_context_menu_items`） | mod から呼ぶもの | 備考 |
  |---|---|---|
  | cwd をコピー | `$.ui.copy`（`$.session.cwd()`） | tako を呼ばず mod の中で完結 |
  | Finder で開く（cwd） | `tako file open <cwd>` | メニューは `open_default(cwd)` を呼んでいる |
  | 右に分割 / 下に分割 | `tako split --right` / `--down` | 試作で実測済み |
  | 会話を保って再起動 / 引き継ぎを書かせて再起動 | `tako session-restart --mode harness` / `handoff` | 実行中の claude 自身が終わる |
  | リミット後の自動復帰 | `tako limit-resume on` / `off` | |
  | バックグラウンドへ | `tako background` | 自分のペインが画面から外れる |
  | 閉じる | `tako close` | 自分のペインを閉じる = claude も終わる |
  | このペインでリモート接続… | 対象外 | 素のシェルのペインにだけ出る項目（`can_ssh`）。エージェントのペインには出ない |
  | パスをコピー / Finder で表示 / デフォルトアプリで開く | 対象外 | プレビューペインの項目 |
  | 言語サーバの項目（#1684） | 対象外 | コードの本文の項目 |
  | （メニューの外）ペインに名前を付ける | `tako title <名前>` | ヘッダ側の操作だが、Claude Code の中から呼べると便利なので足す |

- 受け入れ条件: 表の「呼ぶ」行がすべて実機で通る。**番犬**: 右クリックメニューに項目が増えたら
  この表（mod 側の対応表の正本）に「呼ぶ / 対象外 + 理由」の行が無いと落ちる。MATRIX `claude_mod_actions`

### S5 チャット表示を構造化通知で（#1883）

- やること: mod が `session.append` の `{uuid, door, origin, type, name}`（本文なし）と
  `turn.*`・`prompt.submit` を報告に載せ、tako の GUI ライク表示（#691 系）が transcript を
  ポーリングせず通知で読み直す。キュー中の発話（FR-2.23.15）と作業中インジケータ（FR-2.23.13）を
  画面の推測ではなく mod の値で出す。本文は transcript の既存パーサ（`uuid` で引く）
- 受け入れ条件: 新着の反映が通知から 1 tick 以内。キューの発話が 1 対 1 で重複排除される。
  mod が無いときは今の経路のまま。MATRIX `claude_mod_chat`
- 判断点: 報告の量（1 ターンで 10 行前後 = §1.4）を 1 秒 flush でまとめて送る形で足りるか、
  実測して決める

### S6 tako → mod の push（[提案] を含む。#1884）

- やること: §2.3 の常時 push の設計と、それで初めてできる機能（チャット入力を `$.prompt.submit`
  で送る / 権限ダイアログへ mod 経由で答える / tako から compact を頼む）の要否の判断
- 受け入れ条件: 設計書の追記と、採るなら最初の 1 機能の実装

### 調査: codex / agy の同等の拡張点（#1885）

- codex / agy に「プロセス内で動き、状態を構造で取れ、画面に描ける」拡張点があるかを調べ、
  MATRIX の `claude_mod_*` の codex / agy 列を Pending → Supported / Unsupported（根拠つき）へ確定する

## 8. 未検証・リスク

> S7 の未検証・リスクは §9.9。

- **API の揺れ**: mod の API は early access。Claude Code の更新で mod が読まれなくなると、
  §6 のとおり画面読み取りへ落ちるだけで壊れはしないが、気づけないと「いつの間にか一次ソースが
  消えていた」になる。`tako mod status` / `check-health` に「claude を起動したのに 60 秒報告が
  無い」ペインを出す（S1）。validate / test は夜間リリースの前段で毎晩回し、落ちたら通知する
  （#1892。前回合格した claude の版と比べて「Claude Code の更新で壊れた」かを出し分ける）
- 版 2.1.287〜2.1.293 は未実測（下限を 2.1.294 にした理由）
- Windows の Claude Code で mod（と `CLAUDE_CODE_PLUGIN_DIRS` の `;` 区切り）が動くかは未実測
- desktop / VS Code 拡張 / Remote Control の surface は未実測（tako のペインは terminal だけなので
  S1〜S5 には影響しない）
- `$.mcp.connect` で mod 自身の manifest に tako の MCP を書けば、`tako setup-mcp` 無しでも tako の
  ツールがモデルに載る可能性がある（未実測。コマンドの書き方が利用者の登録と違うと**ツール一式が
  二重に載る**ので、S1〜S5 では manifest に MCP を書かない）
- SSH ペインのリモート側で動く claude には env も展開先も届かない（画面読み取りのまま）

## 9. S7: 描画の主を mod へ・setup での導入・定型の UI 設定（2026-10-09）

> エピック #1877 のユーザーの追加依頼 1（「GUI 表示を mod に落として描く・コマンドカードや使用制限も mod で・
> setup で入れて他の mod と競合しない・mod なしでも今までどおり」）と追加依頼 2（「ステータスバーの 5h / 7d / ctx の
> バーも mod で・カスタムボタン（`/compact` のワンボタン等）の土台・tako setup から定型化された UI で、
> 軽いモデルでも壊さず調整できる」）を受けた見直し。**ここが §0〜§7 と食い違えばここが正**。

### 9.0 結論（S7 で決めたこと）

| 論点 | S0〜S3 | S7 の決定 | 節 |
|---|---|---|---|
| 導入 | ペインの env `CLAUDE_CODE_PLUGIN_DIRS` だけ | **setup が設定 dir ごとに `<設定 dir>/skills/tako/` へ管理印つきの写しを置く**（settings 系のファイルは書かない）。env 注入は残す（同名なら env 側だけが読まれ二重にならない） | §9.4 |
| 描画の主 | 帯 1 行と `/tako` のサイドバーだけ mod | コマンドカード・使用制限 / ctx のバー・カスタムボタン・チャット風の描画も **mod が Claude Code の中で描く** | §9.3 |
| tako 側の表示を止める条件 | （無し） | **新鮮な報告 ∧ 報告の `renders` にその表示がある ∧ ui.json の置き場が mod**。それ以外は今の表示のまま | §9.6 |
| UI の調整 | `/tako band on\|off` だけ | `<data_dir>/claude-mod/ui.json`（スキーマ・検証・#916）。setup / CLI / MCP の同じ口で**選んで**変える。mod は報告の応答で受け取る | §9.7 |
| 共存 | 帯は自分の行だけ返す | 描画は**必ず `next(e)` を包む**。帯は 1 行。名前 `tako` の衝突は入れない | §9.5 |
| ボタンの押し方 | 帯は ctrl+x → Tab | tako は**クリックを TUI へ渡していない**ので当面はキー操作。クリックの転送は #1961（判断点つき） | §9.2 |

### 9.1 試作の構成

- **本物の認証を写さない**ため、Claude Code 2.1.294 の実物を**一時の `CLAUDE_CONFIG_DIR`** で動かし、
  API は **ローカルの偽 Messages API**（`ANTHROPIC_BASE_URL`。Node の数十行・SSE で応答・ダミー鍵は一時の
  `.claude.json` で承認済みにする）へ向けた。最後の発話の合言葉で台本を選ぶ（`tako_show_command` の
  tool_use / 長いコマンドのコードブロック / Bash の権限 / AskUserQuestion / 既定は `ok`）
  - API キー経由なので `$.session.usage().rateLimits` は空（使用制限のバーの見た目は固定値で確かめた）。
    ctx は応答の usage から出る（`ctx 6%`）
- 隔離 GUI（`scripts/lib/isolated-gui.sh`・仮想ディスプレイ・`TAKO_PERSIST=0`・main `d851f0d` 相当の debug ビルド）の
  直接ペインと、素の tmux（`-f /dev/null`）で動かした。tako のペインの env（`TAKO_PANE_ID` / `TAKO_CLI` /
  `CLAUDE_CODE_PLUGIN_DIRS`）はそのまま使い、本物の tako mod（S3）も同時に読ませた
- 試作の mod は 3 本（使い捨て。製品コードには入れていない）: 描画の試作 `tako-s7`・他の mod の代役 `other-mod`
  （帯を独占する / 包む、`$.ui.status`、`/tako` の同名登録、`ui.copy` の拒否）・導入方式を測る印の mod（名前は本物と同じ `tako`）
- plugin-authoring スキルは使っていない（既定の書き先が自セッションへ効くため。メモリ「Claude Code の mod の試作手順」）

### 9.2 実物で確かめた描画とボタン（2.1.294）

| 確かめたこと | 結果 |
|---|---|
| コマンドカード（AI の `tako_show_command`） | MCP の呼び出しは **`ToolGroup`（「Called tako」の 1 行）に畳まれ、`ToolUse` のフックは呼ばれない**。`ToolGroup` を `isExpanded: true` で `next` へ渡すと `ToolUse` として描かれ、枠 + `Code` + ボタンのカードへ描き替えられた（下の採取） |
| 長いコマンド | `Code`（`wrap`）で見た目だけ折り返し、Copy に渡るのは **157 字・改行なしの論理文字列** |
| コードブロック（`AssistantMessage`） | エンジンの描画（`await next(e)`）の下にボタン行を足せる。画面のコードは物理改行されるが、ボタンが渡すのは論理文字列（FR-2.22 の問題を mod で解ける） |
| 吹き出し（`UserMessage`） | `Box` の枠 + `justifyContent: flex-end` で右寄せの吹き出しになった |
| `$.ui.status` | **「⚠ <plugin 名>: 」の警告の体裁**で、利用者の statusLine の**上**に 1 行増える（上書きはしない）。常時表示の情報には向かない |
| `PromptHint.tail` | hint 行（`⏵⏵ auto mode on …`）の末尾に dim で足せる: `· 5h ▁ 4% 7d ▂ 22% ctx ▁ 6%`（tako のステータスバーと同じ見た目） |
| `SessionMode` | 119 桁のペインでは出なかった（理由は未解明。採らない） |
| トースト | 右上に「plugin 名」の見出しつきの小箱で約 4 秒 |
| 帯（`AbovePrompt`）の行数 | `maxRows` は**全画面では下段の残り**（21 行のペインで **1 行**、窓を高くして 4 行）、main screen では端末の高さ。超えると「↓ N more」に畳まれ、フォーカスしてスクロール |
| カスタムボタン | 帯のボタンから `$.command.run({ command: "compact" })` で **`/compact` が流れて会話が圧縮された**（「Compacted」）。`tako split --right` も通った |
| ボタンの押し方 | 帯 / ペインは **ctrl+x → Tab でフォーカス → ホットキー**（main screen でも全画面でも押せた）。**会話の行のボタンはクリックでしか押せない**（フォーカスできる場所は `Pane` と `AbovePrompt` だけ = 型定義の `UiFocusComponent`）。main screen では会話の行も帯もクリックは効かない |
| tako のクリック | **tako は TUI へクリックを渡していない**（`on_pane_mouse_down` = cmd+クリックのリンク / tako 自身の選択。渡すのはホイールだけ = `terminal.rs` の `wheel_report_bytes`）。ペインへ SGR のクリック列を送れば押せたので、Claude Code 側は受け取れる → #1961 |
| 全画面 / main screen | 2.1.294 は**直接ペインでも素の tmux の中でも既定で全画面**（tmux の `alternate_on=1`・`mouse_any_flag=1`）。`CLAUDE_CODE_NO_FLICKER=0` で main screen（マウス報告 off）。型定義の「tmux では既定で main screen」は実物と食い違う |
| 二重表示（今の tako） | カードが登録された瞬間に tako の GPUI のカード帯がペインの下に 7 行を取り、claude の viewport が **21 → 14 行**に縮んだ（mod のカードと同じものが二重に出る） |
| Copy の経路 | `$.ui.copy` は**呼んだ plugin 自身の `ui.copy` フックを通らず、他の plugin のフックだけを通る**（他の mod が拒否・書き換えできる）。書き先は型定義では端末のクリップボードの道具（pbcopy 等）か OSC 52 |

カードの採取（隔離 GUI の直接ペイン・119 桁。1 回目の採取。最後の行は #1958 で tako がカードを断ったときの表示）:

```text
╭──────────────────────────────────────────────────────────────────────────────────╮
│ tako  ビルドと確認                                                               │
│ cargo build -p tako-cli && TAKO_ISOLATED=1 TAKO_DATA_DIR=/tmp/tako-s7-very-long- │
│ data-dir-name ./target/debug/tako mod --json | jq ".panes[] | {pane, reason}"    │
│ [ Copy 1 ] [ Run 1 in a new pane ]                                               │
│ tako mod                                                                         │
│ [ Copy 2 ] [ Run 2 in a new pane ]                                               │
│ tako did not take the card (see ctrl+o)                                          │
╰──────────────────────────────────────────────────────────────────────────────────╯
⏺ カードに出しました。
```

帯（本物の tako mod の行 + カスタムボタン）と PromptHint のバー:

```text
tako | <ペイン名> | タブ <タブ名>                                                    [-]
[ compact ] [ split right ]
─────────────────────────────────────────────────────────────────────────────────────
❯
─────────────────────────────────────────────────────────────────────────────────────
  ⏵⏵ auto mode on (shift+tab to cycle) · ← for agents · 5h ▁ 4% 7d ▂ 22% ctx ▁ 6%
```

「Run 1」を押すと mod が既存の `tako show-command --run --card 2 --index 1` を呼び、tako に新しいペインが立って
カードの実行記録（`runs[0] = {pane: 2, state: exited, exit_code: 101}`）が付いた = PC の帯・スマホと同じ記録を通る。

### 9.3 mod で描くもの / 描かないもの（棚卸し）

| tako 側の表示（いま） | 決定 | mod でどう描くか | 理由・メモ | スライス |
|---|---|---|---|---|
| コマンドカード（FR-2.22。ペインの下の GPUI の帯） | **移す** | 会話の行 = `ToolGroup` を展開して `ToolUse` をカードに（全画面）。**押す口は帯の tako の行**（`カード N` + Copy / Run のホットキー。main screen でも押せる）。正本は tako の store（実行記録はスマホと共通） | 二重表示と viewport の縮み（§9.2）が消える。会話の行のボタンはクリック前提なので #1961 までは帯が主 | #1963 |
| 使用制限・ctx のバー（画面下のステータスバーの claude の区画。FR-2.42.18） | **移す** | `PromptHint.tail` に `5h ▁ 4% 7d ▂ 22% ctx ▁ 6%`（置き場は ui.json で `band` / `off` も） | 追加依頼 2。利用者の statusLine が既に出していれば描かない。codex / agy / シェルにフォーカスしたとき・mod なしではステータスバーのまま | #1962 |
| S3 の帯の ctx / 使用制限（閾値超え） | そのまま | バーと同じ値。バーが出ていれば帯からは外す | 同じ情報を 2 か所で動かさない | #1962 |
| カスタムボタン（新規） | **mod で描く** | 帯の tako の行の中（1 行に詰める）。既定は `compact` 1 個 | 追加依頼 2 | #1960 / #1962 |
| GUI ライク表示のチャット（FR-2.23。`tako ui-mode gui`） | **段階的に移す** | 吹き出し・コードのコピー・ツールの要約を mod で描き、GUI モードで mod が効いているペインは GPUI のチャットの代わりにターミナル表示（mod の描画）を出す | 入力欄のミラー（#737）・生成中やキューの画面推測が要らなくなる。描けないもの: 権限ダイアログ（エンジン専有）・プロポーショナルフォント・画像。初心者がクリックで押せるには #1961 が先 | #1964 |
| チャットヘッダの残量バー（GUI モード） | チャットと一緒に | （mod のチャットでは PromptHint のバー） | | #1964 |
| 承認カード（GUI モード） | **移さない** | （`$.ui.notice` で 1 行足すのが限界） | 型定義: 権限ダイアログはエンジンだけが描く | — |
| スターター（空ペインの 3 カード） | **移さない** | | claude がまだ居ないペイン | — |
| 右パネル（fleet / orch / git / tasks / diagnostics） | **移さない** | （`/tako` のサイドバーに worker 一覧は S3 で済み） | tako 全体の視界で、claude 以外のペインも含む | — |
| ステータスバーの残り（ブランチ・ポート・codex の使用制限等） | **移さない** | | 窓の chrome。claude の外の情報 | — |
| ペインの右クリックの操作（#1882） | ボタンの語彙へ | ui.json の `tako` 語彙（S4 の対応表が正本） | ボタンの中身は既存 CLI | #1882 / #1960 |

### 9.4 導入方式の比較（一時の設定 dir で 1 周ずつ実測）

| | A. ペインの env（S1） | B. ローカルの marketplace + `claude plugin install` | **C'. `<設定 dir>/skills/tako/` に写しを置く（採用）** |
|---|---|---|---|
| 設定 dir に書くもの | なし | `settings.json`（`extraKnownMarketplaces` / `enabledPlugins`。**Claude Code が書き直し、値は保つがキー順が変わる**）・`plugins/known_marketplaces.json`・`plugins/installed_plugins.json`・`plugins/cache/<mkt>/tako/<版>/` の写し | `skills/tako/` の dir 1 つだけ（**settings 系のファイルは不変**。`.claude.json` の記帳は普段の起動でも変わる） |
| 読まれる場所 | env の dir | marketplace の**元の dir**（`plugin list` の「Read from」）。キャッシュの写しではない | `skills/tako`（symlink も辿る）。`tako@skills-dir` |
| tako の外の claude | 読まれない | 読まれる（mod は休眠） | 読まれる（mod は休眠） |
| `/plugin` の見え方 | 出ない | `tako@<mkt>  ✔ enabled` | `tako@skills-dir  ✔ loaded`。利用者の `disable` で `settings.json` に `"tako@skills-dir": false`（利用者の操作） |
| 版の更新 | 展開先を書き換えるとホットリロード | 元の版を上げれば**次の起動**で反映（`plugin update` 不要）。走っているセッションは `/reload-plugins` まで旧（ファイル監視は inline だけ） | 書き換えると**ホットリロード**（`session.start` が再発火） |
| 外したあと | — | `uninstall` + `marketplace remove` で値は元どおり。ただし空の `extraKnownMarketplaces: {}`・空の `plugins/*.json`・`.orphaned_at` つきの写しが残る（バイトは戻らない） | dir を消すだけ |
| tako を消したら | 読まれない | 元の dir が無いと `/plugin` に「✘ failed to load（cache-miss）」。起動は黙って続く | 写しなら残って休眠。symlink なら宙に浮いて黙って読まれない |
| `$.store` | `tako_inline-*.json` | `tako_<mkt>-*.json` | `tako_skills-dir-*.json`（**読み込み元ごとに別ファイル**） |

**同名の扱い（3 方式に共通）**:

- env / `--plugin-dir`（inline）に同名 `tako` があれば **inline だけが読まれる**（B では「Plugin "tako" from
  --plugin-dir overrides installed version」、C' でも hooks module は `tako@inline` の 1 つだけ）= S1 の注入と
  setup の導入を同時に持っても**二重には読まれない**（名前を `tako` に揃えている限り）
- 別の出どころの同名 `tako`（利用者の別の marketplace 等）は、両方 enabled でも**先に来た方だけが読まれ、もう一方は
  黙って読まれない**（「another plugin of that name loads first」）

**決定: C'（写し + 管理印）を setup の導入にし、A は残す**（#1959）。

- 理由: 「この PC の claude に入れる」（`/plugin` に出て、利用者が Claude Code 側で止められる）と、FR-2.42.2 の
  「settings 系のファイルを書かない」を両立できるのは C' だけ。B は `settings.json` を書き直し、外しても空の
  キーと写しが残り、ホットリロードもしない
- A を残す理由: tako のペインでは inline が勝つので、**その tako の版の mod が必ず読まれ**、ホットリロードし、
  setup が知らない設定 dir（direnv で切り替わる dir 等）の claude にも効く
- **写しにする**（symlink にしない）: Windows の symlink は権限が要る（#513 の設定共有と同じ理由）・利用者が
  `~/.claude` を dotfiles で同期していると、ホームパス入りの link が他の機械へ渡る（tako の設定共有は symlink を
  辿らないので tako 経由では渡らない）。写しは tako の版の更新時に差し替える（中身が同じなら書かない）
- **管理印** `skills/tako/.tako-managed`（tako が置いた印・版・内容のハッシュ）。**印の無い `skills/tako` は利用者のもの**
  なので触らない（`name_conflict`）。別の出どころの `tako` があれば（`plugins/installed_plugins.json` と settings の
  `enabledPlugins` を読むだけで分かる）その設定 dir には置かず、**そのペインへの env 注入もしない**（inline は installed
  を上書きするので利用者の `tako` を潰す）
- 置く先: accounts.yaml の設定 dir（`AccountsConfig::list_resolved()`）+ 既定（`claude_default_config_dir()`）+
  実行時に mod の報告の `config_dir` で見えた dir（GUI 起動時の差分検出で足す = #916 と同じ二段構え）
- 利用者が Claude Code 側で止めた（`enabledPlugins["tako@skills-dir"] === false`）ら、env で読まれた mod も
  `$.settings.read()` で見て休眠し、最後の報告で `user_disabled` を返す（env 注入が利用者の選択を上書きしない）
- 外し方: `tako mod uninstall`（印つきの写しだけ消す）と setup の undo
- FR-2.42.2 の文言は #1959 で「settings 系のファイル（`settings*.json`・`plugins/*.json`）は書かない。置くのは
  `skills/tako` の管理印つきの写しだけ」へ改める
- env 注入なし・`skills/tako` の写しだけでも、本物の tako mod（S3）が tako のペインで読まれて報告と帯の描画まで
  動くことを実測した（`TAKO_PANE_ID` / `TAKO_CLI` の env は今どおり要る = `tako mod off` でこの 2 つも止まれば休眠する）

### 9.5 共存（他の mod / plugin / statusLine）の実測と規則

- **読み込み順 = 外側から** `--plugin-dir`（引数の順）→ `CLAUDE_CODE_PLUGIN_DIRS`（並びの順）→ installed / skills-dir。
  すべて tier `user` で、同じイベントのフックはこの順に入れ子になる（デバッグログの `environment N` の順）
- **帯の取り合い**: 外側の mod が `next()` を呼ばずに答えると、内側の帯は描画で走らない（デバッグログ
  「answered ui.render without next(); nothing beneath it ran」）。**いまの S3 の帯は描くときに自分の行だけを返す
  ので、利用者が `skills/` や marketplace で入れた他の mod の帯を tako のペインで消している**（tako が env = 外側の
  ため。実測で確認）。`next(e)` の答えは、下に誰も居なければ `{type: "engine"}`、居ればその描画
- **規則**: tako の mod は `ui.render` の全フックで**必ず `next(e)` の答えを包む**（帯は「他の mod の行 + tako の行」を
  縦に並べる・`AssistantMessage` 等はエンジンの描画の下に足す）。他の mod が tako を隠すのは防げないので、帯の
  フックが呼ばれていない（`band.columns` が付かない）ことを `tako mod` に「帯は他の mod に隠された」と出す
- **帯の行数は共有資源**: 全画面の 21 行のペインで 1 行。tako の行は**ボタン込みで 1 行**に詰める（2 行にすると
  他の mod の行と合わせて「↓ N more」に畳まれる）。複数行の詳細は `/tako` のサイドバー（`$.ui.open`）
- **`$.ui.status`**: plugin ごとに 1 行で並ぶ（他の mod・利用者の statusLine を上書きしない）。警告の体裁なので
  tako は常時表示に使わない
- **スラッシュコマンド**: `$.command.register` の同名は**先に登録した方が取り、後は失敗**（`session.start` の非同期の
  速さで入れ替わりうる）。tako は `/tako` の登録の失敗を報告に出し、別名は作らない（ボタンと `/tako` の中身は
  帯から届く）
- **利用者の statusLine**: tako は書き換えない。使用量のバーは statusLine が既に ctx / 使用制限を出している構成
  （`$.settings.read()` の statusLine + tako の画面読み取りで判定）では描かない
- **コピー**: `$.ui.copy` は他の mod の `ui.copy` フックに拒否されうる → 結果が `isCopied: false` なら tako の
  dispatch のコピー（`tako show-command --copy`）へ落とす
- **同名 `tako`**: §9.4 のとおり検出して入れない・注入しない

### 9.6 フォールバックと、tako 側の表示を止める条件

| 状況 | mod | tako 側 |
|---|---|---|
| setup 前・管理外の設定 dir（env 注入は効く） | env で読まれて描く | mod の `renders` に従う |
| `tako mod off` / 版の下限未満 / `name_conflict` | 置かない・注入しない | **今の表示**（GPUI のカード帯・ステータスバー・GPUI のチャット・画面読み取り） |
| tako の外の claude | 休眠（`TAKO_PANE_ID` / `TAKO_CLI` が無い） | 関与しない |
| 組織アカウント（classic 系が届かない） | 描画は関数フック（`ui.render`）なので**影響なし**（S3 で組織 / 個人の両方の設定 dir で帯を実測）。権限待ちは S1 の代替で拾う | mod の `renders` に従う |
| `--safe-mode` / `--bare` / `disableAllHooks` / `allowManagedModsOnly` | 読まれない | 今の表示（`tako mod` に理由） |
| 利用者が `/plugin` で disable | 休眠して `user_disabled` | 今の表示 |
| 他の mod に帯を隠された | 帯の `renders` が偽 | 帯は mod 専用なので代わりは出さず `tako mod` に理由 |
| 報告が古い（45 秒） | — | **1 tick（2 秒）で今の表示へ戻る** |

**止める条件**は表示ごとに「**新鮮な報告 ∧ 報告の `renders` にその表示がある ∧ ui.json の置き場が mod**」:

- 報告（`schema: 1` に後方互換で追加）の `renders`: `band`（描けたか）・`usage_bar`（置き場 or null）・
  `buttons`（数）・`cards`（**描いたカードの id**）・`chat`（チャット風の描画をしているか）。無い = 何も描いていない
  （古い mod は今の表示のまま）
- コマンドカード: `renders.cards` に載った id だけ GPUI の帯から外す（store には残る = スマホ・CLI は同じ）
- 使用量のバー: **フォーカス中のペイン**の claude の mod が `renders.usage_bar` なら、ステータスバーの claude の
  5h / 7d / ctx の区画を出さない
- チャット: GUI モードで `renders.chat` のペインは GPUI のチャットビューの代わりにターミナル表示

### 9.7 定型の UI 設定（`ui.json`）

- **置き場は tako の data dir 側**: `<data_dir>/claude-mod/ui.json`。Claude Code の設定 dir へは書かないので
  FR-2.42.2 と矛盾しない。mod は**ファイルを読まず**、報告の応答 `tako.view.ui` で検証済みの値を受け取る
  （tako の外では効かない・壊れた値は mod へ届かない・`$.store` の読み込み元ごとの分裂（§9.4）に左右されない）
- **正本 1 実装** `tako_core::claude_mod_ui`（既定・検証・語彙・書き込みは一時ファイル → 差し替え）
- 形（`schema_version: 1`。案）:

```json
{
  "schema_version": 1,
  "band": { "hidden": false, "segments": ["pane", "tab", "workers", "attention", "card", "buttons"] },
  "usage_bar": { "place": "prompt_hint", "items": ["five_hour", "seven_day", "ctx"] },
  "buttons": [{ "id": "compact", "label": "compact", "hotkey": "c", "action": { "kind": "slash", "command": "compact" } }],
  "cards": { "place": "auto" },
  "chat": { "place": "auto", "bubble": "gui_only", "code_copy": true, "tool_summary": "gui_only" },
  "colors": { "accent": "suggestion", "warn": "warning", "dim": "subtle" }
}
```

- **語彙（選ぶだけ。任意の JS は持たない）**: action は 4 種 — `slash`（許可リスト: compact / clear / context / cost /
  model / effort と `/tako`。描くときに `$.command.list()` に無い名前は描かない）・`tako`（#1882 の対応表の操作 id）・
  `shell`（利用者が書いたコマンドを**新しい tako のペイン**で実行 = カードの run と同じ経路・FR-2.22.7 の検証）・
  `prompt`（入力欄へ文を入れるだけ = `$.prompt.fill`、送らない）。色は Claude Code のテーマのキーだけ。並びは id の
  並べ替えだけ。ボタンは 8 個まで・label 16 桁・hotkey は数字か小文字 1 字で重複不可。**既定のボタンは `compact` 1 個**
- **口は 3 つで同じ 1 実装**: CLI `tako mod ui`（今の値と**選べる値**の一覧）/ `set <key> <value>` / `button add|remove|move` /
  `reset`、MCP `tako_mod` の `action=ui`（op と値の enum を inputSchema に載せる）、`tako setup` の段（対話 = 既定 /
  おすすめ / 最小の 3 択 + ボタンを選ぶ・`--answers` のキー `mod_ui`）。#322 の最簡形の規約どおり、引数なしは表示
- **軽いモデルでも壊さない**: すべて「選ぶ」口（enum）+ 検証で、不正な値は**書かずに許される値の一覧を返す**。読めない
  ui.json は既定で動き、元は `.unreadable.bak` へ保全（#916 の機構）。性質テスト（ランダムな操作列で常にスキーマを満たす）
- **軽いモデルでの検証方法**（今回は本物の認証を写さない制約で未実測）: 認証のある環境で Sonnet / Haiku に
  `tako mod ui` の口だけで調整を 20 件（「compact のボタンを足して」「バーを消して」「ボタンを左へ」等）頼む
  スクリプト `scripts/test-mod-ui-llm.sh`（一時の設定 dir・CI 外）で、スキーマ違反 0 件と依頼との一致率を測る（#1960）
- **#916**: ui.json を `tako-control::migrations::SPECS` に登録（`migration_registry` の指紋に載る）。S3 の帯のトグル
  （`$.store` の `band`）は ui.json の `band.hidden` を正本に移し、初回は mod の報告の `band.hidden` / `toggled_at` を取り込む
- tako → mod の届き方は今の「報告の応答」のまま（変更は最大 15 秒の heartbeat で届く）。即時にしたければ #1884 の push

### 9.8 スライス（S7）

```text
#1958 [バグ] MCP の tako_show_command の pane 省略（いつでも）
#1959 S7-1 setup が設定 dir ごとに skills/tako へ入れる                 （依存なし）
#1960 S7-2 定型の UI 設定 ui.json と setup / CLI / MCP の口              （依存なし。#1959 と並行可）
#1961 [判断点] tako がクリックを TUI へ渡す                              （依存なし）
#1962 S7-3 帯を next で包む 1 行に + カスタムボタン + バー               ← #1960
#1963 S7-4 コマンドカードを mod で描く                                   ← #1958 #1960 #1962
#1964 S7-6 チャット風の描画と GUI モードの切り替え（#1883 の置き換え）   ← #1960 #1963 #1961
#1965 S7-7 この PC の claude へ setup で入れて実機で効く（最後）         ← #1959 #1960 #1962 #1963
```

- 既存のスライスとの関係: #1882（S4）は ui.json の `tako` 語彙の正本（右クリックメニューとの対応表）と `/tako <操作>`・
  番犬に絞る（#1960 の後）。#1883（S5）は #1964 に置き換えて閉じる（GPUI のチャットを mod の通知で読み直す改善は
  #1964 のフォールバック側に含める）。#1884（S6）は据え置き（ui.json の変更を即時に届けたくなったら）。#1891 の
  check-health には #1959 の設定 dir ごとの導入状態も載せる。#1886（Windows）では `skills/` の写しも確かめる
- 着地は 1 本ずつ（`progress.md` の衝突 = #1228）

### 9.9 未検証・リスク（S7）

- **使用制限の実値**: 偽 API（API キー）では `rateLimits` が空なので、バーの使用制限は見た目だけを固定値で確かめた。
  実値の取り方は S2 / #1903 の実測に依る
- **軽いモデルでの setup**: 本物の認証を写さない制約で未実測（§9.7 の検証方法を #1960 の受け入れに入れた）
- **skills-dir という置き場**は 2.1.294 の `claude plugin init` が案内する口だが、mod と同じく early access で揺れうる →
  夜間検査（#1892 の `scripts/check-claude-mod.sh`）に「skills-dir から読まれる」を足す（#1959）
- 管理された環境（`strictKnownMarketplaces` / `allowManagedModsOnly`）で skills-dir が読まれるかは未実測
- 版の下限未満の claude が `skills/tako` を見たときの振る舞い（1 台に複数の版が混在する場合）は未実測。setup は PATH の
  claude の版で判定する
- `SessionMode` が 119 桁で出なかった理由は未解明（採らないので影響なし）
- クリックの転送（#1961）は claude ペインのドラッグ選択の手触りを変えるので、既定はユーザー判断
- Windows（`skills/` の写し・`;` 区切り）は #1886
