# リモート大刷新の先行調査 — Claude 公式モバイル連携への委譲は成立する（#1059）

- 調査日: 2026-09-01
- 対象 Issue: #1059（エピック）。Refs #66 / #65 / #791 / #919 / #966 / #1006 / #283 / #104
- 実施範囲: 読み取り専用調査。コード変更なし。本番 remote デーモン・serve 設定には触っていない
- 実測環境: macOS（darwin-arm64）/ Claude Code **2.1.232**（native build）/ tako main `1f89caf`（v0.8.2）

## 0. 結論（先に要点）

1. **柱 1 は成立する**。公式機能の名前は **Remote Control**（`claude remote-control` /
   `claude --remote-control` / セッション内 `/remote-control`）。ローカル CLI セッションを
   claude.ai/code と Claude モバイルアプリから操作できる。
2. **特定セッションを開くディープリンクは存在する**。形は `https://claude.ai/code/<session-id>`
   （`session_…` または `cse_…`）。**実測で URL 文字列そのものを採取済み**。
   QR も同じ URL で、公式 docs は「QR を読むとアプリで直接開く」と明記している。
   → Issue が想定した「無ければアプリを開くだけに縮退」は**不要**。
3. **tako はその URL をローカルから読める**。tako が既に読んでいる **transcript jsonl** に
   `type:"system", subtype:"bridge_status"` の行があり、**`url` フィールドに完成形の URL が入る**。
   新しいファイル形式への依存は増えない（transcript は #702 / #716 / #112 で既に解析済み）。
4. **ただし条件が 1 つある**: tako が spawn する master / worker は**現状 Remote Control に
   繋がっていない**（実測: tako 管轄の 5 セッションすべて未接続）。tako 側で
   `--remote-control` を渡す（プロファイル opt-in）か、ユーザー設定の
   `remoteControlAtStartup` を true にする必要がある。
5. **セキュリティの委譲範囲は明確**: Remote Control に委譲した会話は
   **Anthropic のサーバーに transcript が保存され**、認証は claude.ai アカウント（+ Team /
   Enterprise の Trusted Devices）に移る。tako の機器ペアリング二層認証（#283）と
   role（observe / interact / manage / admin）は**その会話には効かない**。
   柱 2（ターミナル）・柱 3（ファイル）は tako 自前認証のままなので、
   **「会話だけ公式・画面とファイルは tako」という境界線**になる。
6. **柱 3 は API 層がまるごと不足**（ダウンロード・ブラウズ・プレビュー・編集の
   HTTP ルートが 1 本も無い）。一方 **PC 側の部品は全部ある**（dispatch の
   `OpenFile` / `Preview*` / `FileOp` / `RemoteFolder`）ので、daemon に proxy ルートを
   足すのが主作業。アップロードだけは既存（`POST /api/upload`）。

## 1. 混同しやすい 3 機能の切り分け（一次情報）

Issue の「cloud sessions / teleport / claude.ai/code」は**別々の機能**で、tako が要るのは
1 つだけ。公式 docs の記述で切り分ける。

| 機能 | 起動 | セッションが動く場所 | tako への適合 |
|---|---|---|---|
| **Remote Control** | `claude remote-control` / `claude --remote-control` / `/remote-control` | **ローカルマシン**（claude.ai / アプリは「窓」） | ◎ **これが柱 1 の答え** |
| Claude Code on the web（cloud sessions） | `claude --cloud "<task>"` | Anthropic のクラウド VM（GitHub から clone） | × ローカルのペイン・ファイルを触れない |
| Teleport | `claude --teleport [<id>]` / `/teleport` | ローカル（**クラウド → ローカルへ引き込む**） | × 向きが逆 |

出典（公式 docs の明示的な区別）:

- 「Unlike [Claude Code on the web], which runs on cloud infrastructure, Remote Control sessions
  run directly on your machine and interact with your local filesystem. The web and mobile
  interfaces are a window into that local session.」
  — <https://code.claude.com/docs/en/remote-control>
- 「`--cloud` creates cloud sessions. `--remote-control` is unrelated: it exposes a local CLI
  session for monitoring from the web.」
  — <https://code.claude.com/docs/en/claude-code-on-the-web>
- 「From the CLI, session handoff is one-way: you can pull cloud sessions into your terminal with
  `--teleport`, but you can't push an existing terminal session to the web.」
  — 同上

実 CLI（`claude --help`。2.1.232）の該当行も同じ 3 つを別フラグとして持つ:

```
  --cloud [description|session_id|url]  Create a cloud session with the given description, or
                                        attach to an existing one by session ID or claude.ai/code URL
  --remote-control [name]               Start an interactive session with Remote Control enabled
                                        (optionally named)
  --teleport [session]                  Resume a teleport session, optionally specify session ID
```

## 2. Remote Control の成立条件（公式 docs + この機での実測）

### 2.1 要件（docs より）

| 項目 | 条件 |
|---|---|
| プラン | Pro / Max / Team / Enterprise。**API キー認証は不可** |
| 組織設定 | Team / Enterprise は**既定 OFF**。Owner が admin settings の Remote Control トグルを入れる必要がある |
| 認証 | claude.ai ログイン（`claude auth login` / `/login`）。`claude setup-token` の長寿命トークン・`CLAUDE_CODE_OAUTH_TOKEN` は**不可**（model 要求専用スコープ） |
| API エンドポイント | `api.anthropic.com` 直。Bedrock / Vertex（Agent Platform）/ Foundry / `ANTHROPIC_BASE_URL` の差し替え・enterprise gateway は**不可** |
| フィーチャーフラグ評価 | `DISABLE_TELEMETRY` / `DO_NOT_TRACK` / `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC` / `DISABLE_GROWTHBOOK` のいずれかが立っていると**不可** |
| ワークスペース信頼 | 対象ディレクトリで一度 `claude` を通し信頼ダイアログを承諾しておく（ホーム直下は信頼が保存されない） |
| ZDR | Zero Data Retention の組織は**有効化できない** |

出典: <https://code.claude.com/docs/en/remote-control>（Requirements / Connection and security /
Troubleshooting 節）

### 2.2 この機での実測

- `claude remote-control --help` が **exit 0 でフラグ一覧を印字**。docs は
  「Claude Code checks Remote Control eligibility before printing help, so
  `claude remote-control --help` returns an error instead of this flag list when you aren't
  signed in with an eligible account」と書いているので、**このアカウントは適格**。
- `claude doctor` の出力に **Remote Control 節が問題なしで出る**（`No installation issues found.`）。
- tako が spawn に使う env（`DISABLE_TELEMETRY` 等・`ANTHROPIC_BASE_URL`）は
  **tako のソースに 1 箇所も無い**（`grep -rn` で 0 件）。つまり tako 自身が要件を壊していない。

### 2.3 有効化の 3 経路 + 自動接続

| 経路 | 形 | tako にとって |
|---|---|---|
| サーバーモード | `claude remote-control`（`--spawn same-dir\|worktree\|session` / `--capacity N`） | tako のペイン内で 1 プロセスが複数の claude セッションを持つ形になり、**tako の「1 ペイン = 1 エージェント」モデルと噛み合わない**。採らない |
| 対話セッション | `claude --remote-control ["<name>"]`（別名 `--rc`） | ◎ **tako の spawn 経路にそのまま乗る**（`build_master_cmd` / `build_worker_cmd` に 1 フラグ） |
| 既存セッションから | `/remote-control`（別名 `/rc`） | 既に走っている master / worker を後から繋ぐ用。#640 の送達確認つき経路で打てる |
| 全セッション自動 | `/config` の **Enable Remote Control for all sessions**、設定ファイルは `remoteControlAtStartup: true`（user settings / managed settings） | ユーザー設定なので tako が勝手に書くべきではない（#513 の共有分類にも関わる）。**案内するだけ**が妥当 |

`remoteControlAtStartup` の三値（docs）: `true` = 常に自動接続 / `false` = OFF（project・local の
`false` は managed の `true` にも勝つ）/ `default` = 組織既定 → Claude Code の現行既定。

### 2.4 tako 管轄セッションの現状（実測 = ここが柱 1 の唯一の実装点）

セッション台帳 `<config dir>/sessions/<pid>.json` の `bridgeSessionId` の有無で
Remote Control 接続を判定した（#1011 で tako が既に読んでいるファイル）。

| config dir（アカウント） | 生存セッション | `bridgeSessionId` あり |
|---|---|---|
| 既定（`~/.claude`。個人アカウント） | 9 | **7** |
| tako の別アカウント（`CLAUDE_CONFIG_DIR` 指定） | 5 | **0** |

- 既定アカウント側は `.claude.json` に `seenNotifications: {"remote-control-auto-on": 3}` があり、
  **自動接続が既定で入った旨の通知が表示されている**（`remoteControlAtStartup` は
  どの settings にも書かれていない = サーバー側既定で ON になったと読める）。
- 別アカウント側には同じ通知が無く、`bridgeSessionId` も 0 件。**適格なのに未接続**。
- → **tako が明示的に有効化しない限り、tako の master / worker はスマホから見えない**。
  アカウント既定に依存すると「アカウントによって見えたり見えなかったりする」ので、
  **tako 側の明示 opt-in（プロファイル）が正しい設計**。

## 3. ディープリンク: 存在する（実測つき）

### 3.1 URL の形

CLI バイナリ内の URL 組み立て（`strings` で採取。可読部の引用）:

```js
function RIr(e,t){ … return "https://claude.ai" }
function WS(e,t,r){ let {toCompatSessionId:n}=…, o=n(e), s=`${RIr(o,t)}/code/${o}`;
                    return r?`${s}?${new URLSearchParams(r)}`:s }
```

呼び出し側（同じ採取）:

```js
return { url: WS(t.bridgeSessionId, t.sessionIngressUrl), sessionId: t.bridgeSessionId }
```

id の正規化（同じ採取）:

```js
function V3(e){ if(!e.startsWith("cse_")) return e; … return "session_"+e.slice(4) }   // toCompatSessionId
function vU(e){ if(!e.startsWith("session_")) return e; return "cse_"+e.slice(8) }     // toInfraSessionId
function Lm(e){ return e.replace(/^(?:session|cse)_/,"") }                             // sessionIdBody
```

公式 docs も同じ形を明記している:

- 「The ID is the part of the session's URL at claude.ai/code between `/code/` and any `?`.」
  — <https://code.claude.com/docs/en/remote-control>
- 「For `<session-id>`, pass the bare ID, such as `session_...` or `cse_...`, or the session's
  `claude.ai/code/<id>` URL, with or without the scheme or query string.」
  — <https://code.claude.com/docs/en/claude-code-on-the-web>
- 同 docs の実行例: `View: https://claude.ai/code/session_01DiUkqY2kzbUbDmW1w96rfi?from=cli&m=0`

### 3.2 実測（Remote Control セッションを 1 本立てて採取 → 明示 pid で撤収）

`claude --remote-control "<name>"` を pty 越しに 1 本起動した。画面（ANSI 除去後）:

```
/remote-control is active · Continue here, on your phone, or at
https://claude.ai/code/session_01XXXXXXXXXXXXXXXXXXXXXX
```

（実際に採取した id は 24 文字の `session_…`。本レポートでは伏せる。以下同様）

### 3.3 モバイルアプリで開けるか

- docs: 「**Scan the QR code** shown alongside the session URL to open it directly in the
  Claude app.」「**Open the session URL** in any browser to go directly to the session on
  claude.ai/code.」— <https://code.claude.com/docs/en/remote-control>
- **未確認（実機なし）**: iOS / Android の実アプリで universal link として開くところは
  この調査では踏んでいない。URL 自体を `WebFetch` で叩くと **403**（claude.ai ログインが要る =
  リンクは秘密ではなくアカウント認証が前提、という裏取りにはなる）。

## 4. tako が session URL を取る経路（実測 3 系統）

| 経路 | 中身 | 評価 |
|---|---|---|
| **① transcript jsonl の `bridge_status` 行** | `{"type":"system","subtype":"bridge_status","content":"/remote-control is active · … at https://claude.ai/code/session_01XX…","url":"https://claude.ai/code/session_01XX…", …}` | ◎ **推奨**。**完成形の URL がそのまま入っている**。tako は `transcript.rs` で同じファイルを既に読む（`claude_config_dirs()` が**アカウントごとの config dir を横断**する = #571 対策済み）。現行の正規化は `type:"user"` / `"assistant"` だけを拾うので、**抽出を足すだけで既存のチャット整形に影響しない** |
| ② transcript の `bridge-session` 行 | `{"type":"bridge-session","sessionId":"<uuid>","bridgeSessionId":"cse_01XX…","lastSequenceNum":0,"ownerAccountUuid":"…","ownerOrganizationUuid":"…"}` | △ id は `cse_` 接頭辞なので `cse_→session_` 変換が要る（§3.1 の `V3`）。**アカウント UUID を含むのでログ・診断に出さない**（AGENTS.md の絶対ルール） |
| ③ セッション台帳 `<config dir>/sessions/<pid>.json` の `bridgeSessionId` | `session_01XX…`（こちらは `session_` 接頭辞で観測） | △ 非公開レイアウト。**pid → セッションの対応が要るときだけ**（#1011 と同じ扱い） |
| ④ `claude agents --json` | **不可**。フィールドは `cwd / kind / name / pid / sessionId / startedAt / status` のみ（Remote Control 接続中のセッションでも増えない = 実測） | × 公式 CLI 出力に URL は載らない |

補足（実測での注意点）:

- 台帳の `name` は**リモート側のセッション表題とは別物**（`--remote-control "<name>"` を渡しても
  台帳側は `nameSource: "derived"` の自動名だった）。UI に出すなら **リモート表題ではなく
  tako 側のタブ名 / role を使う**のが安全。
- プロセス終了で**台帳ファイルは消える**（撤収時に実測）。transcript は残るので、
  「過去のセッションのリンク」を出したいなら ① が唯一の経路。

## 5. アカウントの一致問題（設計上いちばん効く制約）

docs: 「Auto-connect signs in with your own claude.ai account, so a session it starts appears
only in your own account's Claude apps and grants no one else access.」
— <https://code.claude.com/docs/en/remote-control>

tako は **worker / master ごとに `CLAUDE_CONFIG_DIR` を切り替える**（#504 / #511 / #512 / #547）。
つまり Remote Control セッションは**その worker のアカウント配下**に登録される。

- スマホが個人アカウントでログインしていて、master が別アカウント（組織など）で走っていると
  **そのセッションはスマホの一覧に出ない**。
- したがって PWA の一覧には **「どのアカウントのセッションか」を必ず出す**必要がある。
  出さないと「押しても出てこない」という切り分け不能な不具合になる。
- 実測でもこの機は 2 アカウント併存で、接続済み / 未接続がアカウント単位で分かれていた（§2.4）。

## 6. セキュリティ: 委譲で tako 自前認証から外れる範囲

`.agent/threat-model-remote.md` の現行モデルは
**① Tailscale identity（`X-Forwarded-For` / `X-Forwarded-Host` 検証）+ ② 機器ペアリング**の
二層で、画面データは tailnet 内 WireGuard E2E に閉じている。Remote Control に会話を委譲すると
**その会話だけ**が別のモデルへ移る。

| 観点 | tako 自前（現行） | Remote Control 委譲後（会話部分） |
|---|---|---|
| 経路 | tailnet 内 HTTPS（`tailscale serve` → 127.0.0.1）。公開インターネットに出ない | **Anthropic API 経由**（ローカルからの outbound HTTPS のみ。inbound ポートは開かない） |
| 認証 | Tailscale identity + Mac 画面で承認した機器 | **claude.ai アカウント**（+ Team / Enterprise の Trusted Devices ベータで端末登録 + 18 時間以内のサインイン + 生体確認） |
| 権限粒度 | role 4 段（observe / interact / manage / admin）。observe 端末は入力すると 403 | **無い**。そのアカウントでログインした端末は会話を steer できる（`/model` `/effort` 変更・permission 応答・subagent 停止まで） |
| 会話の保存先 | ローカル transcript のみ（tako は読んで返すだけ） | **Anthropic サーバーに transcript が保存される**（同期と再接続のため。Data usage ポリシー準拠） |
| 承認の代行 | tako の `respond`（画面のダイアログ実在を再検証してから番号キー） | 公式側が permission プロンプトを転送。**未応答のまま 5 分でタイムアウトするダイアログ種別がある**（`dialogExpiry`） |
| 無効化 | `tako remote stop` | `disableRemoteControl` 設定 / 組織ポリシー |

出典: <https://code.claude.com/docs/en/remote-control>（Connection and security / Trusted Devices /
Limitations 節）

**設計上の帰結**:

1. **既定 OFF・プロファイル単位の opt-in にする**。tako が黙って会話を Anthropic 側へ
   同期させ始めてはいけない（現行の「ローカルに閉じている」性質を静かに変えることになる）。
2. **柱 2 / 柱 3 は委譲しない**。ターミナル画面とファイルは tako の二層認証 + role の中に残す。
   委譲するのは「AI との対話」だけ、という線を明文化する。
3. ZDR 組織・`DISABLE_TELEMETRY` を設定した環境では**成立しない**ので、
   PWA は「公式リンクが取れない理由」を出せる必要がある（§9 の Issue B に含める）。

## 7. 現行 remote 実装の棚卸し

### 7.1 daemon の HTTP ルート（`crates/tako-control/src/remote.rs`。6322 行）

必要 role は `required_role()` が正（未知の POST は安全側の Manage）。

| ルート | 役割 | 柱 |
|---|---|---|
| `GET /api/health` | 生存確認 | 共通 |
| `GET /api/me` | この端末の登録状態・role・version | 共通 |
| `POST /api/pair` | ペアリング / role 昇格要求（Mac 画面に承認ダイアログ） | 共通 |
| `GET /api/devices` / `POST /api/devices/revoke` | 端末管理（Admin） | 共通 |
| `GET /api/admin/state` / `pair/approve` / `pair/deny` / `devices/revoke` | Mac 側 GUI 用（XFF 付きは常に拒否） | 共通 |
| `GET /api/v2/panes` | ペイン一覧 + カード用スニペット・活動状態（`remote_preview.rs`。438 行） | 柱 2（一覧は柱 1 でも使う） |
| `GET /api/panes/:id/screen` | 画面（ANSI 可） | 柱 2 |
| `GET /api/panes/:id/scrollback` | 履歴 | 柱 2 |
| `WS /ws?pane=:id` | 画面差分プッシュ | 柱 2 |
| `POST /api/panes/:id/input` | 入力（text / keys。Interact） | 柱 2（+ 柱 1 のチャット送信） |
| `POST /api/panes/:id/close` / `resize` | close / resize（Manage） | 柱 2 |
| `GET /api/agents` | claude セッション解決（pane → `session_id`） | **柱 1** |
| `GET /api/sessions/:id/messages?tail=N` | transcript 正規化（`transcript.rs`。1678 行） | **柱 1** |
| `POST /api/panes/:id/respond` | permission ダイアログ応答（Interact） | **柱 1** |
| `POST /api/upload` | ファイルアップロード（Interact。20MB / ペイン cwd 配下の `.tako-remote-uploads/` 固定 / 0o600 / traversal 拒否 / symlink 拒否） | 柱 3（既存） |
| （静的） | 埋め込み PWA 配信。**任意 Content-Type でバイト列を返す実装が既にある**（`Response::from_data` + `content_type_for`） | 柱 3 の download で再利用 |

daemon → GUI は **IPC で `protocol::Request` を dispatch する正規経路**を持つ（#281 H-7）。
`Send` / `OrchestratorRespond` / `Close` / `TmuxResize` を既にこれで通している。
app 不在時は read-only fallback。

### 7.2 PWA（`web/tako-remote/`。src 計 3507 行）

| ファイル | 行 | 役割 | 柱 |
|---|---|---|---|
| `app.jsx` | 138 | ルーティング（`#/` → 一覧、`#/panes/:id` → 端末）+ ペアリング前段 + PWA バージョン不一致バナー | 共通 |
| `pages/pairing.jsx` | 147 | 機器ペアリング | 共通 |
| `pages/panes.jsx` | 453 | ペイン一覧（カード・状態ピル・タブグループ。#621） | 柱 1 / 2 |
| `pages/terminal.jsx` | 543 | `view` = `chat`（既定）/ 端末リーダー。ペイン間移動・キー送信・フォント | 柱 1 / 2 |
| `components/chat-view.jsx` | 718 | **自前チャットビュー**（`client.messages()` で transcript 表示 + 送信 + 承認カード + `client.upload()`） | **柱 1（委譲対象）** |
| `components/agent-icon.jsx` | 59 | エージェント種別アイコン | 柱 1 |
| `ansi.js` | 154 | 自前 ANSI SGR パーサ（#63 のリーダービュー） | 柱 2 |
| `api.js` | 134 | API クライアント（同一 origin・bearer 無し） | 共通 |

**委譲で縮む範囲**: `chat-view.jsx`（718 行）の transcript 表示・送信・承認カード。
**残る範囲**: 一覧（`panes.jsx`）・端末（`terminal.jsx` + `ansi.js`）・ペアリング・アップロード。

### 7.3 柱 3 に足りない層（現状の穴）

- **ダウンロード**: ルートが**無い**。バイト列を返す仕組み自体はある（§7.1 最終行）。
- **ブラウズ / プレビュー / 編集**: ルートが**無い**。ただし PC 側 dispatch は揃っている:
  `OpenFile` / `PreviewView` / `PreviewEdit` / `PreviewApply` / `PreviewSave` / `PreviewSearch` /
  `PreviewReplace` / `PreviewUndo` / `PreviewRedo` / `FileOp` / `TreeFolder`。
- **SSH 先**: `RemoteFolder`（#919 / #966 の SFTP。`open` / `close` / `list` / `ls` /
  `open-file` / `pending` / `push` / `auto`）と `SshHosts` / `OpenRemote`（#1006）が dispatch に在る。
  **スマホ → Mac → SSH 先の 2 ホップ**は「Mac が SFTP を張り、スマホは Mac の API を見る」形で、
  新しい信頼境界は増えない（スマホは SSH 鍵に触らない）。
- **注意**: dispatch は JSON-RPC なので、ファイル本体は base64 か
  **daemon が直接ファイルを読む**かの選択になる。後者だと**パス認可が daemon 側に生える**ので、
  脅威モデルの更新点はここに集中する（§8）。

## 8. 脅威モデル（`.agent/threat-model-remote.md`）の更新点

柱ごとに、追記が必要な項目を列挙する。**#104 / #287 の枠（identity 検証 + Origin 完全一致 +
role）は崩さない**前提。

### 柱 1（会話の委譲）

- 新節「Claude 公式 Remote Control への委譲」: §6 の表をそのまま入れる。
  とくに **transcript が Anthropic サーバーに保存される**ことと
  **role が効かない**ことは受容リスクとして明記が要る。
- tako が出す**ディープリンクは秘密ではない**（開くには claude.ai ログインが必要 = 403 実測）。
  ただし**セッション id は共有すると follow-up 送信の宛先になる**（`claude -p --cloud <id>` が
  id / URL を受ける）ので、**PWA のログ・監査ログに URL を書かない**（ペイン内容と同基準）。

### 柱 3（ダウンロード）

新しく大きい攻撃面。最低限これを決めてから実装する。

- **読み出し可能なパスの範囲**: 「ファイルツリーに現に出ているルート配下だけ」に閉じる
  （タブの cwd + `tree add` されたフォルダ + `remote-folder open` 済みの SSH 先）。
  `/` からの任意読み出しは**作らない**。判定は**純粋関数**にして daemon と GUI が同じ 1 実装を通す。
- **traversal / symlink**: upload 側と同じ基準（`..` / 絶対パス拒否・symlink の follow 拒否）を
  **読み出しにも**適用する。
- **role**: 読み出しは Observe では**許さない**（画面閲覧と「ソースコード全部持ち出せる」は別物）。
  **新しい role 段を足すか、Interact 以上にする**かは設計判断が要る（推奨: Interact 以上）。
- **サイズ / 種別**: 上限とレンジ配信の扱い。実行可能ファイル・鍵ファイル（`.env` / `*.pem` /
  `id_rsa`）を**既定で弾く**か出すかを決める。
- **監査**: ダウンロードは**パスを出さずに**「バイト数 + 端末名」で記録（#287 P2-2 と同基準）。

### 柱 3（SSH 先のアップロード）

- 2 ホップの後段は #966 の SFTP（アトミック `put` → `rename`・**内容そのもの**での競合検知・
  mode 復元・押し出せなかった保存の退避）。**この保証をスマホ経路でも壊さない**
  （`--force` に相当する操作をスマホから既定で出さない）。
- 書き込み先は「`remote-folder open` 済みのフォルダ配下」に閉じる。

## 9. 実装 Issue 分割案

依存順に並べた。Issue の「実装順の目安: 柱 1 → 柱 3 → 柱 2」に沿う。
各 Issue の受け入れ条件は**実測できる形**で書いた。

### 柱 1

**A. Remote Control のプロファイル opt-in と有効化**（依存なし・最初にこれ）

- tako の spawn 経路（`build_master_cmd` / `build_worker_cmd` / 引き継ぎの後任 master /
  `tako solo`）に `--remote-control` を渡す。プロファイル `remote_control`（既定 false）で gate。
  CLI `tako orchestrator profiles set <名前> --remote-control true` / MCP / 設定画面の 3 経路 1:1。
- 適格性の事前判定（§2.1 の要件を読み取りだけで確認）と、非適格時の**理由 + 次の一手**。
  `agent_support::MATRIX`（#982）に「Remote Control で会話を委譲できるか」を 1 マス足す
  （claude = Supported、codex / agy = Unsupported + 上流に同等機能が無い旨の根拠）。
- 受け入れ: ①opt-in したプロファイルで spawn すると、その worker の transcript に
  `bridge_status` 行が出る ②opt-in していないと出ない ③非適格環境
  （`DISABLE_TELEMETRY=1` を注入）で理由つきに落ちる ④`TAKO_<n>_LEGACY=1` で旧挙動

**B. session URL の解決と 1:1 公開**（A に依存）

- `tako-control` に `claude_remote_link`（純粋関数 + transcript からの抽出）を新設。
  正は **`bridge_status` 行の `url`**、無ければ `bridge-session` 行の `bridgeSessionId` を
  `cse_→session_` 変換して組む（§3.1 / §4）。**アカウント UUID は保持しない**。
- `GET /api/agents` と `GET /api/v2/panes` の応答に
  `remote_link { url, session_id, account_label, state }` を足す。
  `state` は `connected` / `not_connected` / `ineligible: <理由>` / `unknown`。
- CLI `tako sessions link [--pane N]` / MCP `tako_sessions` の action で同じ値を返す
  （開発不変条件 = UI でできることは AI からもできる）。
- 受け入れ: ①実 Remote Control セッションで URL が取れ、CLI / MCP / API の 3 経路が一致
  ②接続していないペインは `not_connected`（URL を捏造しない）③**診断ログに URL を出さない**
  番犬テスト ④`no_personal_data` 番犬を通る

**C. PWA を「一覧 + 公式へ送り出す」形へ**（B に依存）

- `panes.jsx` のカードに「Claude で開く」を出す（`remote_link.url` を新規タブ / アプリへ）。
  `not_connected` は理由 + 「PC 側で有効化する方法」を出す（押せるボタンは作らない =
  ユーザー設定を勝手に書かない）。
- `terminal.jsx` の既定 `view` を再検討。**自前チャットは残す**
  （非適格環境・codex / agy worker・オフライン tailnet では公式が使えないため
  = フォールバックとして必要。Issue の「縮小 / 廃止 / フォールバック維持」への回答は**維持**）。
- アカウント表示（§5）を一覧に出す。
- 受け入れ: ①実機スマホで一覧 → タップ → Claude アプリで当該セッションが開く
  ②`not_connected` のペインで理由が出る ③自前チャットへの回帰が無い（既存 e2e 緑）

**D. スマホから「新しいタブ + master 起動」**（B に依存。柱 1-2）

- `POST /api/tabs`（`TabNew { cwd }` を dispatch）+ `POST /api/tabs/:id/master`
  （プロファイル選択つき。`OrchestratorSpawn` ではなく `tako master` 相当）。role は **Manage**
  （タブとプロセスを作る = close / resize より強い操作）。
- 起動後、`bridge_status` が出るまで PWA 側でポーリングし、出たら**そのまま公式リンクへ送る**。
  出ないまま上限に達したら理由（§2.1 のどれに当たるか）を出す。
- 受け入れ: ①スマホから 1 操作でタブ + master が立ち、**指示は Claude アプリ側から通る**
  ②opt-in していないプロファイルでは公式リンクが出ず理由が出る
  ③observe / interact role では 403 ④作られたタブが Mac 画面にも出る

### 柱 3

**E. ファイル API 基盤（ブラウズ + プレビュー + ダウンロード。ローカル）**

- 認可の正を純粋関数で新設（「ツリーに現に出ているルート配下だけ」。§8）。
  `GET /api/files?root=&path=` / `GET /api/files/content` / `GET /api/files/download`。
  読み出しは **Interact 以上**。ダウンロードは監査にパスを書かない。
- 受け入れ: ①ツリー外のパスを 403 で拒否（traversal / symlink / 絶対パスの各形）
  ②Observe で 403 ③実機でスマホに保存できる ④監査ログにパスが出ない番犬

**F. スマホからの軽い編集**（E に依存）

- `PUT /api/files/content` を `PreviewEdit` / `PreviewApply` / `PreviewSave` 経由で。
  保存は #966 と同じ「書けるまで成功と言わない」規約。
- 受け入れ: ①編集 → 保存 → Mac 側のプレビューに反映 ②競合時に上書きしない ③Observe で 403

**G. SSH 先のファイル（プレビュー / 編集 / アップロード）**（E / F に依存）

- 一覧に SSH 先ルートを出す（`RemoteFolder` の `list`）。`ls` / `open-file` / `push` を proxy。
  **#966 の競合検知・mode 復元・pending 退避を素通しさせる**（`force` はスマホから出さない）。
- 受け入れ: ①実 SSH 先（Linux / Windows の 2 台）でプレビュー・編集・アップロードが通る
  ②切断中の保存が pending へ退避され、復帰後に push される ③スマホは SSH 鍵に触らない

### 柱 2

**H. SSH ターミナルの切り替え / 新規接続**

- `GET /api/ssh-hosts`（`SshHosts`）+ `POST /api/panes/:id/ssh` / `POST /api/ssh`
  （`OpenRemote` の `target` = `split` / `tab` / `pane`。#1006）。role は Manage。
  #1010 の `ssh_connect`（`phase` / `reason` / `attempt`）を PWA に出す。
- 受け入れ: ①スマホからホストを選ぶと Mac 側にペインができて接続まで進む
  ②到達不能ホストで理由が出て**ペインが消えない**（#919 / #1040 の契約）
  ③`can_ssh_pane` が false のペインはメニューに出ない（#1006 の判定を共有）

## 10. 未確認・推測の明示

| 項目 | 状態 |
|---|---|
| iOS / Android の実アプリで `claude.ai/code/<id>` が universal link として開くこと | **未確認**（実機なし）。docs の「QR を読むとアプリで直接開く」に依拠 |
| `--remote-control` を tako の spawn 経路に足したときの通し（master / worker が実際に繋がる） | **未確認**（本調査は claude 単体で 1 本立てて確認しただけ。コード変更していない） |
| 既定アカウントで自動接続が ON になっている根拠 | **推測**。`seenNotifications: {"remote-control-auto-on": 3}` の存在と `remoteControlAtStartup` 未設定から読んだもので、サーバー側フラグは観測できない |
| Team / Enterprise の Trusted Devices の実挙動 | **未確認**（beta / 組織設定。docs 記載のみ） |
| `bridge_status` 行の安定性（上流のフォーマット変更耐性） | **推測**。公開仕様ではないので、#1011 と同じく**取れなかったときに静かに縮退する**（`state: unknown`）設計が要る |
| ダウンロードで鍵ファイルを弾くかどうか | **未決定**（設計判断。Issue E で決める） |
| 現行 PWA の e2e が委譲後も緑かどうか | **未確認**（コード変更していないため） |

## 11. この調査での副作用と後始末

- Remote Control の実測のため `claude --remote-control` を **1 本**起動し、
  **明示 pid で `kill`** した（wrapper の `script` も同様）。終了後にプロセス 0 件・
  セッション台帳ファイルの消失を確認済み。tako 管轄の 5 セッションは
  **`bridgeSessionId` が付かないまま**（= 触っていない）。
- **残留**: claude.ai のセッション一覧に、その 1 本が **offline 状態で残る**
  （CLI からは削除できない仕様。claude.ai / アプリの一覧から archive / delete できる）。
- 本番 remote デーモン・`tailscale serve` 設定・tako の設定ファイルには**書き込んでいない**。
