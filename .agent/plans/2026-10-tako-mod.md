# tako mod: Claude Code の mod 連携の設計（エピック #1877 / S0）

> スライス（S1〜S6）の worker がこれを読んで実装に入れる粒度で書く。方針（やること 1〜5）の
> 正本は #1877 の本文。ここに書くのは「実物で確かめたこと」「決めたこと」「切り方」。
> 試作は Claude Code **2.1.294**（2026-10-08）の実物で行った。mod の API は early access で
> 版ごとに動くので、**型定義（`.claude-plugin/types/claude-code/index.d.ts`）と実物を正**とし、
> この文書の API 記述が実物と食い違ったら文書を直してから実装する。

## 0. 結論（決めたこと）

| 論点 | 決定 | 根拠の節 |
|---|---|---|
| mod → tako の通信路 | **`$.process.run` で tako CLI（`tako mod report`）を叩く** | §2 |
| tako → mod の通信路 | 当面は **report の応答に相乗り**（+ 1 秒 flush / 15 秒 heartbeat）。常時 push は S6 で `$.process.spawn` + 長ポーリング CLI | §2.3 |
| Claude Code の中からの tako 操作 | ボタン・スラッシュコマンドから**既存の CLI をそのまま呼ぶ**（2 つ目の実装を作らない） | §2 / §7 S4 |
| mod の置き場と導入 | tako が `<data_dir>/claude-mod/tako/` へ展開し、**ペインの env `CLAUDE_CODE_PLUGIN_DIRS` に足す**。Claude Code の設定ファイルは 1 バイトも書かない | §3 |
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

**決定: A**。ゼロコンフィグ（設計原則）と「本番設定を書き換えない」を両立できるのは A だけ。
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
| `claude_mod_state` | S1 | Supported | Pending（#下記の調査 Issue） | `local_pending_first_class()` |
| `claude_mod_band` | S3 | Supported | 同上 | 同上 |
| `claude_mod_actions` | S4 | Supported | 同上 | 同上 |
| `claude_mod_chat` | S5 | Supported | 同上 | 同上 |

- codex / agy を **Unsupported にしない**。「上流に同等の拡張点が無い」はまだ調べていないので
  Pending（調査 Issue）にする（`AgentSupport::Unsupported` の doc の禁止事項）
- 既存のマス（`master_ctx_percent` / `worker_limit_detect` / `worker_permission_dialog` /
  `worker_status_detect`）は claude 列が既に Supported なので変えない。S2 で根拠欄に
  「mod があれば一次ソース、無ければ画面」と書き足す
- OS 軸（`platform::support`）: Windows は Pending（#467 配下に子 Issue。区切り `;` の単体テストは
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
  「未実測」と出す。CI に `claude` を入れるかは [提案]）

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

## 7. スライス

依存:

```text
S1 基盤（同梱・展開・注入・報告・status）
 ├─ S2 一次ソース化（ctx・使用制限・状態）
 ├─ S3 Claude Code の画面に出す（帯・ペイン）
 ├─ S4 Claude Code から tako を操作（コマンド・ボタン）
 └─ S5 チャット表示を構造化通知で
S6 tako → mod の push（S2〜S5 の後。[提案] を含む）
調査: codex / agy の同等の拡張点（MATRIX の codex / agy 列を確定する。いつでも）
```

S2〜S5 は S1 の後なら並行できるが、`progress.md` の衝突（#1228）を避けるため着地は 1 本ずつ。

### S1 基盤

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

### S2 一次ソース化

- やること: §6 の表の上 4 行。`claude_ctx::resolve` / `limit_stop` の hint / `orchestrator::wait`・
  `worker_status` / チャットヘッダの残量バー（#702）/ #749 の自動ハンドオフの tick が mod の報告を
  先に見る
- 受け入れ条件: 4 経路（`orchestrator self` / `worker_status` / #749 tick / チャットヘッダ）が
  `ctx_source: "mod"` を返す。statusLine を外した claude（画面に ctx が出ない構成）でも ctx% が
  取れる。報告を止める注入で 45 秒後に画面 / transcript へ落ちて理由が出る。使用制限の reset 時刻が
  mod の値（秒精度）になる。A/B: `TAKO_1877_S2_LEGACY=1`

### S3 Claude Code の画面に出す

- やること: 帯（1 行。このペインの名前・タブ・worker 数と要注意の数・ctx / 使用制限は閾値を
  超えたときだけ）・`/tako` でサイドバーのペイン（詳細）・帯を隠すトグル（`$.store` に保存）。
  データは report の応答（§2.3）
- 受け入れ条件: 80 桁・144 桁・300 桁で 1 行に収まる。ダイアログ表示中に隠れても壊れない。
  `claude plugin test` で terminal と desktop の両 surface。statusLine を持つ利用者の画面で
  情報が二重にならない既定（閾値未満は出さない）。MATRIX `claude_mod_band`

### S4 Claude Code から tako を操作

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

### S5 チャット表示を構造化通知で

- やること: mod が `session.append` の `{uuid, door, origin, type, name}`（本文なし）と
  `turn.*`・`prompt.submit` を報告に載せ、tako の GUI ライク表示（#691 系）が transcript を
  ポーリングせず通知で読み直す。キュー中の発話（FR-2.23.15）と作業中インジケータ（FR-2.23.13）を
  画面の推測ではなく mod の値で出す。本文は transcript の既存パーサ（`uuid` で引く）
- 受け入れ条件: 新着の反映が通知から 1 tick 以内。キューの発話が 1 対 1 で重複排除される。
  mod が無いときは今の経路のまま。MATRIX `claude_mod_chat`
- 判断点: 報告の量（1 ターンで 10 行前後 = §1.4）を 1 秒 flush でまとめて送る形で足りるか、
  実測して決める

### S6 tako → mod の push（[提案] を含む）

- やること: §2.3 の常時 push の設計と、それで初めてできる機能（チャット入力を `$.prompt.submit`
  で送る / 権限ダイアログへ mod 経由で答える / tako から compact を頼む）の要否の判断
- 受け入れ条件: 設計書の追記と、採るなら最初の 1 機能の実装

### 調査: codex / agy の同等の拡張点

- codex / agy に「プロセス内で動き、状態を構造で取れ、画面に描ける」拡張点があるかを調べ、
  MATRIX の `claude_mod_*` の codex / agy 列を Pending → Supported / Unsupported（根拠つき）へ確定する

## 8. 未検証・リスク

- **API の揺れ**: mod の API は early access。Claude Code の更新で mod が読まれなくなると、
  §6 のとおり画面読み取りへ落ちるだけで壊れはしないが、気づけないと「いつの間にか一次ソースが
  消えていた」になる。`tako mod status` / `check-health` に「claude を起動したのに 60 秒報告が
  無い」ペインを出す（S1）。`claude` を CI に入れて validate / test を夜間に回すのは [提案]
- 版 2.1.287〜2.1.293 は未実測（下限を 2.1.294 にした理由）
- Windows の Claude Code で mod（と `CLAUDE_CODE_PLUGIN_DIRS` の `;` 区切り）が動くかは未実測
- desktop / VS Code 拡張 / Remote Control の surface は未実測（tako のペインは terminal だけなので
  S1〜S5 には影響しない）
- `$.mcp.connect` で mod 自身の manifest に tako の MCP を書けば、`tako setup-mcp` 無しでも tako の
  ツールがモデルに載る可能性がある（未実測。コマンドの書き方が利用者の登録と違うと**ツール一式が
  二重に載る**ので、S1〜S5 では manifest に MCP を書かない）
- SSH ペインのリモート側で動く claude には env も展開先も届かない（画面読み取りのまま）
