# tako remote 全面刷新計画（2026-07-16 改訂）

> 旧計画（Quick Tunnel + Workers KV relay + Pages PWA。実装済み・v0.5.x 現行）は
> この文書の git 履歴を参照。本改訂はその現行構成を **Tailscale Serve 一本へ全面刷新**する計画。
>
> - 根拠レビュー: `reviews/2026-07-10_gpt5.6solレビュー.md`（P0×5 / High×8 / Medium×6）
> - 接続方式の比較検討: `reviews/2026-07-10_tako-remote接続方式・認証設計.md`
> - UI デザインカンプ: `design/takoremote-ui/`（Claude Design handoff。正はこのカンプ）

## 0. ユーザー確定事項（2026-07-16）

1. **安全性最優先**。工数・難易度は考慮しない
2. **接続リンク（URL）の固定は絶対条件**
3. リモート手段は**最も安全な 1 方式に一本化**（複数 transport のテスト・ケアは不可能なため）
4. リモート有効化のセットアップはしやすく保つ（`tako setup` に導線）
5. UI は `design/takoremote-ui/project/Tako Remote Chat UI.dc.html` を**忠実再現**し、
   チャット画面以外（ペイン一覧・ペアリング・設定）も同スタイルで統一
6. ペイン識別の改善: 現行の「ペイン ID しか出ない」を廃し、agent 種別・タイトル・
   マシン名・タブ内位置・モデルを表示（カンプのヘッダ仕様どおり）
7. 実装 worker は Opus 4.6[1m] + max effort。全弾完了後に **codex worker で安全性レビュー**
8. **`tako remote setup` 専用コマンドを新設**する（Tailscale 導入〜接続確認までの対話ウィザード）。
   **未セットアップ状態で `tako remote start` を実行したら「`tako remote setup` を実行して
   ください」と案内して止める**（黙って失敗させない・勝手に進めない）

## 1. transport 選定: Tailscale Serve（確定）

| 条件 | Tailscale Serve | BYO named Tunnel + Access | Quick Tunnel（現行） |
|---|---|---|---|
| 通信の中身を第三者が読めない | ◎ WireGuard E2E。中継 DERP も復号不能 | × Cloudflare edge が TLS 終端＝平文を見られる | × 同左 |
| 入口の非公開性 | ◎ public internet に存在しない | × 公開 URL + Access 門番 | × 公開 URL |
| リンク固定 | ◎ `https://<mac>.<tailnet>.ts.net` 恒久 | ◎（要ドメイン購入） | × 毎回変わる |
| セットアップ | 両端末にアプリ + 同一アカウント（無料） | ドメイン + Tunnel + Access 設計 | ゼロコンフィグ |

- 決定打: E2E 暗号化（中間者が構造的に読めない）+ 入口が世界に存在しない、の 2 点
- 受け入れる制約: **Mac とスマホの両方に Tailscale アプリ + 同一アカウントが必須**（ユーザー承認済み）
- Tailscale アカウント侵害への防御: tako 側ペアリング（第二層）が阻止 + tailnet lock を docs で推奨
- Funnel（public 公開機能）は**使わない**。Serve のみ（tailnet 内限定）

## 2. 刷新後アーキテクチャ

```
iPhone（Tailscale アプリ・同一 tailnet）
  │  WireGuard E2E 暗号化（直接 P2P、不可なら DERP 中継＝中身は読めない）
  ▼
https://<mac 名>.<tailnet>.ts.net       ← 恒久固定 URL・ホーム画面 PWA 化
  │  tailscale serve（tailnet 内のみ・identity ヘッダ付与）
  ▼
127.0.0.1 の tako remote daemon         ← localhost bind・PWA もここから配信（同一 origin）
  │  認証層① tailnet identity 検証（Tailscale が認証した端末か）
  │  認証層② tako 機器ペアリング（Mac 側で承認済み端末か + role）
  ▼
tako core → dispatch → pane 操作        ← 正規経路（監査・最後の pane 保護・送達検証つき）
```

## 3. 廃止するもの（攻撃面の削減）

存在ごと削除する: cloudflared 依存 / Quick Tunnel / relay Worker + Workers KV /
公開 Pages PWA（`tako-remote.pages.dev`）/ URL fragment の bearer token /
localStorage の token 保存 / `--insecure` 平文 LAN モード。

これにより GPT レビュー指摘のうち **P0-5・H-1・H-2・H-4・M-6 は対応不要（消滅）**。

## 4. 認証設計（二層）

### 層① Tailscale device identity
- daemon は 127.0.0.1 のみ bind。tailscale serve 経由以外の到達経路を持たない
- serve が付与する identity（Tailscale-User-Login 等）+ LocalAPI whois で接続元ノードを特定
- **信頼性境界を弾 0 で実測**（ローカル別プロセスによるヘッダ偽装可否を含む）。
  偽装可能な経路が見つかれば whois 照合を必須化

### 層② tako 機器ペアリング
- 初回アクセス時: daemon が接続元 identity を読み、**Mac 画面に承認ダイアログ**
  （端末名・identity・要求 role を表示）→ ユーザーが許可/拒否 → 許可で端末登録
- role: **Observe**（画面閲覧のみ・既定）/ **Interact**（入力・承認応答・添付可）/
  **Manage**（close / resize）/ **Admin**（端末管理）。昇格は Mac 側承認
- 端末別 revoke / interact session の idle timeout / 接続開始・終了の macOS 通知 /
  status bar の接続中インジケータ + kill switch
- 監査 metadata（接続開始終了・端末・route・入力 byte 数。**内容は記録しない**＝既存ログ規約維持）
- **AI フルコントロール不変条件の例外**: ペアリング承認・role 昇格は Mac 側の人間操作のみ。
  MCP / CLI に承認 API を作らない（理由を `.agent/requirements.md` に明記する）。
  start / stop / status は従来どおり MCP / CLI から可（tailnet 内限定露出のため）

## 5. UI 刷新要件（カンプ対応表）

正は `design/takoremote-ui/project/Tako Remote Chat UI.dc.html`（511 行・読み切ること）。
デザイントークン: `--bg:#0B0D10 --panel:#121519 --claude:#D6795C --codex:#ECECEC --agy:#6C9EFF`、
Geist / Geist Mono。**フォントは自己ホスト**（Google Fonts 参照禁止。tailnet 内で外部依存ゼロ）。

| カンプ | 内容 | 実装弾 |
|---|---|---|
| 1a | claude チャット（吹き出し・ツールカード・実行中パルス + 停止） | 弾 5a |
| 1b | codex チャット（**承認待ちカード: 許可(y)/拒否(n)**） | 弾 5b |
| 1c | agy チャット（分析カード・選択肢ボタン） | 弾 5b |
| 1d | term 切替後のターミナル表示 + quick keys + フォントサイズ | 弾 5a（現行リーダービュー資産を再利用） |
| 1e | スラッシュコマンドのインライン候補 | 弾 5b |
| 1f | モデル / エフォート切替シート（/model・/effort として送信） | 弾 5b |
| 1g | ファイル添付シート（作業 dir へアップロード → パス渡し） | 弾 5b |

- iPhone モック装飾（ノッチ・23:06・5G 表示・ホームバー）は**実装対象外**
- テーマはカンプどおり**ダーク固定**（light 追加は自己判断でやらない＝忠実再現）
- ペイン識別改善: 一覧とヘッダに agent 種別アイコン・タイトル（autorename 由来）・role・
  マシン名・タブ内位置（2/4 形式）・モデル名を表示。remote API のペイン情報を拡張して supply
- チャット化の対象は **claude を完全対応**。codex / agy は term ビュー + 状態表示をベースに
  ベストエフォート（transcript 形式が異なるため。無理に同等化しない）

### チャット UI の技術方針
- transcript は既存 `/api/sessions/:id/messages`（正規化済み）を土台に拡張
- 送信・停止（ESC）・承認（y/n）・/model・/effort はすべて**正規 dispatch Send 経路**
  （#95 の送達検証資産に乗せる。remote 専用の送信経路を作らない）
- 承認待ち・busy の検出は `claude_tui` / orchestrator worker_status の既存検出資産を再利用
- ファイル添付: 新規 `POST /api/upload`。**Interact role 必須・サイズ上限（既定 20MB）・
  保存先はペイン cwd 配下の専用 dir（`.tako-remote-uploads/`）固定・パス traversal 検証・
  実行権限を付けない**

## 5.5 セットアップ・利用導線の全体設計

> remote に触れるすべての入口と状態遷移をここで定義する。各弾はこの導線に整合させる。

### 導線 A: 初回セットアップ（Mac 側）
1. 入口は 3 つ、すべて同じウィザードへ収束する:
   ① CLI `tako remote setup` ② AI に「スマホから見られるようにして」→ MCP `tako_remote_setup`
   ③ `tako setup`（全体セットアップ）末尾の案内 1 行「リモート接続は `tako remote setup`」
2. ウィザードの流れ: Tailscale 検出（GUI 版 / CLI 版両対応）→ 未導入なら brew / App Store
   案内 + その場インストール（y/N）→ ログイン確認（未ログインならブラウザ認証へ誘導して待機）→
   MagicDNS + HTTPS 証明書の有効化確認（未有効なら管理画面 URL を提示）→ serve 設定 →
   自己接続確認 → **スマホ側手順の表示**（下記 導線 B）
3. 方針の整合: 全体 `tako setup` は質問ゼロ（#262）だが、**remote setup は明示対話型**とする。
   ネットワーク到達性を作る操作であり、既定で黙って有効化しない（`--yes` / `--answers` で
   非対話も可能にし、dispatch / MCP と 1:1 の開発不変条件は維持する）

### 導線 B: 初回セットアップ（スマホ側）
ウィザード末尾と docs に同一手順を表示:
①スマホに Tailscale アプリを入れる → ②Mac と同じアカウントでログイン →
③Mac に表示された QR（**固定 URL のみ。secret は含まない**）をスキャン →
④ブラウザで開くと Mac 側にペアリング承認ダイアログが出る → ⑤Mac で許可 →
⑥ホーム画面に追加（以後この導線は二度と不要）

### 導線 C: 日常利用
- `tako remote start`（CLI / MCP / GUI）→ URL は固定なのでスマホはホーム画面から開くだけ
- **未セットアップで start したら**: setup 状態判定（tailscale 導入・ログイン・HTTPS・serve の
  各項目）を行い、**不足項目を列挙して「`tako remote setup` を実行してください」と案内して停止**
- 稼働中は Mac の status bar に常時インジケータ（クリックで接続端末一覧 + kill switch）

### 導線 D: 端末管理・2 台目以降
- 2 台目: 導線 B と同一（URL 固定なので QR 再発行不要。`tako remote status` でも URL 表示）
- 一覧・失効: `tako remote devices list` / `tako remote devices revoke <id>` + MCP 1:1 +
  GUI（status bar → パネル）。**承認・role 昇格だけは Mac 側 GUI 限定**（§4）
- 全遮断: kill switch（GUI）= `tako remote stop` 相当を即時実行

### 導線 E: 既存ユーザーの移行
- v0.6.0 起動時: 旧 Quick Tunnel の state / config を検出したら通知 + 移行ガイドへ誘導
  （setup changes.yaml rev 配信で「remote が Tailscale 方式に変わりました → `tako remote setup`」）
- `tako setup` の依存チェックは cloudflared を削除し tailscale（任意）を追加

## 6. 弾構成（実行単位）

> worker は原則 Opus 4.6[1m] + max。各弾 = 1 Issue = 1 worker = 1 PR。
> **弾 1・2 は main へ**（現行構成にも有益な独立改善のため通常リリースに乗せる）。
> **弾 3〜6 は統合ブランチ `renewal/remote-transport` に積む**（Quick Tunnel 削除という
> 破壊的変更を含むため。夜間自動リリース（nightly-release.sh）に中途半端な状態を
> 拾わせない）。弾 7 のレビュー後に main へマージし v0.6.0 として一括リリース。

### 弾 0: Tailscale Serve 実機 PoC（調査のみ・計画全体の関門）
- brew で tailscale 導入（**ログインのブラウザ認証はユーザー協力**）、iPhone 側もユーザー協力
- 検証項目: ①serve で HTTP/**WebSocket** が両方通るか ②identity ヘッダの実在と
  **偽装可能性の境界**（ローカル別プロセスから 127.0.0.1 直叩きした場合を含む）
  ③MagicDNS + HTTPS 証明書の有効化手順と URL 固定性 ④iPhone 実機で PWA ロード・
  ホーム画面追加・service worker 動作 ⑤App Store 版 / CLI 版の検出方法・パス差
  ⑥未導入・未ログイン・serve 未設定それぞれのエラー表現（setup 用）⑦レイテンシ実測（直接 P2P / DERP）
- 成果物: `.agent/investigations/tailscale-serve-poc.md`。**ここの実測が弾 3 以降の設計の正**
- 失敗条件（WS 不通・identity 検証不能等）が出たら実装に入らず計画を見直す

### 弾 1: daemon 封じ込め（transport 非依存・弾 0 と並行可・main 直行）
- P0-1: secure は `127.0.0.1` bind / P0-3: `/tmp` 廃止 → `<data_dir>/remote/` 0700 +
  作成時 0600 + `O_NOFOLLOW` + atomic rename / P0-4: stop の PID 同一性検証 + 終了確認 +
  `--force` 分離 / P0-2 の一部: MCP から `show_token`・`insecure=true` を削除 /
  H-5: capture-pane の pipe deadlock 根治（別スレッド drain）/ H-6: blocking read の
  timeout（reader thread + recv_timeout）/ M-4: 全 API に `Cache-Control: no-store, private`
- 受け入れ: 隔離環境で LAN 別ホストから到達不能・stop 後のプロセス残存ゼロ・
  2000 行 capture でも deadlock しない、の実測

### 弾 2: dispatch 統合 + remote API v2（main 直行）
- H-7: remote の tmux 直叩きを廃止し core + dispatch 正規経路へ（最後の pane 保護・
  #95 送達検証・CloseReason 整合が remote にも効く）。app 不在時は read-only fallback のみ残す
- H-8: Enter の失敗 status 確認（正規経路移行で自然解消することを確認）
- M-5: WS を pane ごとの共有 broadcaster 化（接続数分の tmux subprocess 乱立を解消）
- API v2（弾 5 の UI が必要とする情報を先に用意）:
  ペイン情報拡張（agent 種別・タイトル・role・cwd・タブ内位置・モデル・状態）/
  エージェント状態（busy / idle / **承認待ち**）/ transcript 拡張。
  認証は現行 token のまま（差し替えは弾 4。二重改修を避けるため接続層に手を入れない）
- 受け入れ: remote 経由 input/close/resize が dispatch を通ることの実測 +
  最後の pane close が拒否される実測 + WS 複数接続で subprocess 数が増えない実測

### 弾 3: Tailscale transport 一本化（統合ブランチ開始点）
- `tailscale serve` の起動・状態管理・`ts.net` URL 解決を daemon に統合。
  tailscale 未導入・未ログイン・HTTPS 未有効・未セットアップの `remote start` は
  **「`tako remote setup` を実行してください」と具体的な不足項目つきで案内して停止**
  （setup 状態の判定関数はここで新設し、弾 6 のウィザードと共有する）
- cloudflared / Quick Tunnel / relay 登録 / `--insecure` のコード削除。
  `web/tako-remote-worker/` は削除（本番 Worker の停止は弾 6）
- **`tako setup` の依存チェック（#88）から cloudflared を削除**（検出・用途説明・brew 案内・
  `--check` 表示・docs の依存表）。tailscale の依存チェック追加は弾 6 のウィザードと同時
- MCP / CLI の remote 系引数を新方式へ同期（AI フルコントロール表・セルフテスト期待値更新）
- 受け入れ: 実 tailnet で start → iPhone から固定 URL 到達 → stop → 到達不能の通し実測。
  cloudflared / trycloudflare / relay への参照がコードから 0 件

### 弾 4: 機器ペアリング認証 + PWA daemon 配信化
- PWA を daemon の静的配信へ（同一 origin・バージョン一致）。build 成果物の同梱方式は
  既存 `build-app.sh` の PWA ビルド工程を流用
- §4 の二層認証をフル実装: 層① identity 検証（弾 0 の実測に基づく whois 照合）+
  層② ペアリング（Mac 承認ダイアログ・端末レジストリ・role・revoke・idle timeout・
  macOS 通知・status bar インジケータ + kill switch・監査 metadata）
- 長寿命 bearer token / QR の token 埋め込みを全廃（URL は固定なので QR は URL のみ）
- 端末管理の CLI / MCP: `tako remote devices list / revoke` + MCP 1:1（導線 D。
  承認・role 昇格だけは GUI 限定 = §4 の例外）+ status bar インジケータ / kill switch（導線 C）
- 受け入れ: ①未登録端末は画面データを 1 バイトも受け取れない ②revoke 即時反映
  ③Mac 承認なしに Interact 不能 ④Observe 端末は input が 403、の 4 点を実機実測

### 弾 5a: UI 刷新・基盤（チャット + ペイン一覧 + ペイン識別）
- カンプ 1a / 1d の忠実再現: チャットビュー（吹き出し・ツールカード・実行中 + 停止）、
  chat/term トグル、term ビュー（現行リーダービュー資産に quick keys / A± を統合）
- ペイン一覧（ホーム）とペアリング・設定画面を同デザイントークンで新規作成
- ペイン識別改善（§5）・フォント自己ホスト
- 受け入れ: **実装画面とカンプの横並びスクショ比較を PR に添付**（自己解釈で崩さない）+
  iPhone 実機での操作確認記録

### 弾 5b: UI 刷新・高度機能
- カンプ 1b / 1c / 1e / 1f / 1g: 承認待ちカード（y/n。Interact role 必須）、
  選択肢ボタン、スラッシュコマンド候補、モデル / エフォートシート（/model・/effort 送信）、
  ファイル添付（§5 の upload API 新設を含む）
- 受け入れ: 実 claude ペインで承認カード → 許可 → 実行継続の通し実測 + upload の
  role 制限・サイズ上限・traversal 拒否のテスト

### 弾 6: setup 導線 + threat model + docs + 旧資産片付け
- **`tako remote setup` 専用コマンド新設**（対話ウィザード）: Tailscale 検出（GUI/CLI 両対応）→
  未導入なら brew / App Store 案内（その場インストール y/N）→ ログイン確認（ブラウザ認証誘導）→
  MagicDNS + HTTPS 有効化ガイド → serve 設定 → 接続確認 → 固定 URL の QR（PNG）表示。
  dispatch + MCP `tako_remote_setup` と 1:1（非対話は `--yes` / `--answers`、既存 setup の型を踏襲）
- `tako setup` 本体には remote の案内 1 行（「リモートは `tako remote setup`」）+
  依存チェックへ tailscale を追加（任意扱い）
- threat model を `.agent/` に新設（信頼するもの / しないもの / Tailscale 侵害時の挙動 /
  ts.net ホスト名が CT log に載る事実の明記）。`.agent/concept.md`・`architecture.md` の
  「リモート機能は持たない」矛盾を解消。requirements.md にペアリング例外を明記
- README / docs サイト全面更新・CHANGELOG・setup changes.yaml rev 追加・
  既存ユーザー移行ガイド（Quick Tunnel 廃止の breaking change 告知）
- 本番 Cloudflare 資産の停止・削除（Pages プロジェクト / relay Worker）
- 既存ユーザー移行（導線 E）: 旧 state 検出 → 通知 → 移行ガイド誘導
- 受け入れ: クリーン環境で**導線 A→B→C の通し実測**（setup → スマホ接続 → 再訪）+
  未 setup start の誘導文言確認 + docs build 緑

### 弾 7: codex 安全性レビュー → 修正 → v0.6.0 リリース
- codex worker（`agent: codex`）で統合ブランチ全体をセキュリティ観点レビュー
  （認証境界・upload API・identity 偽装・削除漏れの残存参照・secrets）
- 指摘は master が検収し、修正 worker（Opus）を回して潰す。2 巡上限
- 完了後: 統合ブランチ → main マージ → v0.6.0 リリース（tag + Release + cask +
  `build-app.sh --install`）

## 7. 実行順序

```
弾 0（PoC・関門）∥ 弾 1（封じ込め）      ← 並行（調査 vs 実装で無衝突）
  → 弾 2（dispatch 統合 + API v2）        ← main 直行はここまで
  → 弾 3（Tailscale 一本化）              ← 統合ブランチ renewal/remote-transport
  → 弾 4（ペアリング + daemon 配信）
  → 弾 5a（UI 基盤）→ 弾 5b（UI 高度機能）
  → 弾 6（setup + docs + 片付け）
  → 弾 7（codex レビュー → v0.6.0）
```

弾 2 以降はすべて remote 周辺の同一ファイル群を触るため**直列**。並行させない。

## 8. リスクと対処

| リスク | 対処 |
|---|---|
| tailscale serve が WS / identity で期待を満たさない | 弾 0 を関門にし、実装着手前に判明させる。満たさなければ計画を再検討（実装トークンを溶かさない） |
| identity ヘッダのローカル偽装 | 弾 0 で境界実測 → 必要なら whois 照合必須化。第二層（ペアリング）が常に控える |
| Tailscale アカウント侵害 | ペアリング層が阻止。tailnet lock を docs 推奨 |
| upload API が新攻撃面 | Interact role 必須・上限 20MB・保存先固定・traversal 検証・実行権限なし（弾 5b の受け入れ条件） |
| リモートからの承認（y/n）で破壊的コマンドが通る | 承認応答は Interact role 必須。tailnet + ペアリング済み端末＝本人の操作と整理（threat model に明記） |
| 夜間自動リリースが刷新途中の main を出荷 | 破壊的変更（弾 3〜6）は統合ブランチに隔離。main には完成後のみマージ |
| 既存 Quick Tunnel ユーザーの breaking change | v0.6.0 で CHANGELOG・移行ガイド・setup changes 配信 |
| CI 停止中（Actions 枠） | 各弾ローカル全緑 + 隔離セルフテスト + 実機 e2e を DoD に固定 |
| PWA を弾 4 と弾 5 で二度改修する手戻り | 弾 4 は接続・認証層のみ（画面は最小限）、弾 5 が画面全面刷新、と役割を固定 |
| codex / agy の transcript 形式差でチャット化が破綻 | claude 完全対応 + 他はベストエフォートとスコープを先に固定（§5） |

## 9. 計画の最終評価（2026-07-16 master 実施）

- **抜け漏れ点検**: GPT レビュー P0×5 / H×8 / M×6 の全 19 件を弾へ対応付けた。
  消滅 5 件（P0-5 / H-1 / H-2 / H-4 / M-6）、弾 1 で 6 件（P0-1/2/3/4, H-5, H-6, M-4）、
  弾 2 で 3 件（H-7, H-8, M-5）、弾 4 で 3 件（M-1 TTL, M-2 role/revoke, M-3 監査）。
  H-3（未認証 health）はペアリング + identity 検証で置換（弾 4）。対応漏れなし
- **UI 要件点検**: カンプ 7 画面すべて弾 5a/5b に割当。ペイン識別改善は API（弾 2）と
  表示（弾 5a）に分解済み。添付機能のサーバー側（upload API）を弾 5b に計上済み
- **導線点検**: 導線 A〜E（§5.5）を弾 3（未 setup ガード）/ 弾 4（端末管理・kill switch）/
  弾 6（ウィザード・移行・docs）へ対応付けた。`tako setup`（質問ゼロ #262）と
  `tako remote setup`（明示対話）の方針差は「ネットワーク到達性を作る操作は既定で
  黙って有効化しない」という原則で整合を取った
- **残存リスク**: 弾 0 の結果次第で Tailscale 前提が崩れる可能性（そのための関門設計）。
  Mac 側承認ダイアログは GPUI 実装のため弾 4 は tako-app にも手が入る（工数大だが直列なので許容）
- **判定**: 抜け・矛盾なし。弾 0 + 弾 1 から着手可
