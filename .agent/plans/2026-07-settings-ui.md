# 設定画面（Cmd+,）詳細設計 — Issue #459

> 作成: 2026-07-22。対象コミット: `f244a6f`（main）。
> macOS 標準の Cmd+, で開く独立設定ウィンドウを新設する。設定の実体は settings.json
> （テキストベース）のまま、GUI はその読み書きフロントエンドに徹する。
> 個別設定の変更は**既存 dispatch を直接呼ぶ**ことで CLI / MCP / GUI の 1:1 を構造的に保証する。

## 0. 概要・設計原則

- **設定の正はテキスト**: settings.json（+ config.yaml / profiles/*.yaml）が唯一の永続層。
  設定画面は読み書きの UI にすぎず、独自の保存形式を持たない
- **dispatch フロントエンド原則**: 設定画面の各操作は `tako_control::dispatch::dispatch()`
  （dispatch.rs:62。`&mut dyn ControlHost` を取る同期関数）を GUI 内から直接呼ぶ。
  IPC / MCP と完全に同一のコードパスを通るため、挙動の 1:1 が構造的に成立する
- **新設 dispatch は最小**: 新規は「設定画面を開く」`Settings` のみ。色設定・フォントは
  既存 `Theme` dispatch の action 拡張で賄う（MCP ツール数は `tako_settings` の +1 だけ）
- **開発不変条件**（AGENTS.md）: 全機能は MCP / CLI から操作可能。UI 文字列は
  `ui_text/` の `tr!` で日英必須。絵文字禁止（SVG アイコン）

非ゴール:
- キーバインドのカスタマイズ（別 Issue。今回はタブとして作らない）
- config.yaml / profiles YAML の全項目 GUI 化（主要項目のみ。全編集は高度タブ・YAML 直編集で逃がす）
- macOS NSColorPanel 統合のカラーピッカー（hex 入力 + スウォッチで足りる。FFI 追加は過剰）

---

## 1. 設定画面のタブ構成と各タブの設定項目一覧

左ナビ 7 タブ。各行 = 設定項目 / 永続先 / 呼ぶ dispatch（既存 or 新設）。

### 1.1 一般（General）

| 項目 | UI | 永続先 | dispatch |
|---|---|---|---|
| 表示言語 | ドロップダウン system / ja / en | settings.language | `Lang { action:"set", value }`（protocol.rs:850） |
| タブ・ペインの AI 自動リネーム | トグル | settings.auto_rename | `AutoRename`（protocol.rs:292） |
| listen ポート検知 + 提案チップ | トグル | settings.port_detect | `PortDetect`（protocol.rs:295） |
| セッション永続化（tmux バックエンド） | トグル + 状態注記（tmux 不在 / secondary） | settings.tmux_persist | `Persist`（protocol.rs:299。応答に available / secondary あり dispatch.rs:1227-1251） |
| ペイン close 時の確認ダイアログ | トグル | config.yaml confirm_close（setup.rs:264） | `ConfirmClose`（protocol.rs:302）**※永続化の追加が必要（§5.3）** |
| エラーレポート自動送信 | トグル（既定 OFF・opt-in 注記） | settings.telemetry | `Telemetry { action:"on"/"off" }`（protocol.rs:859） |
| 利用制限表示サービス | ドロップダウン claude / codex / agy | settings.limit_service | `LimitService`（protocol.rs:866） |
| プレビューのライブリロード | トグル | settings.preview_live_reload | `PreviewReload`（protocol.rs:380) |
| プレビュー画像キャッシュ上限 | 数値入力（256〜8192 MiB） | settings.preview_cache_max_mb | `PreviewCache`（protocol.rs:383） |
| ペインの平文ログ | トグル + 上限 2 種の数値入力 | settings.pane_logs / pane_log_max_mb / pane_log_total_max_mb | `Logs { action:"set" }`（protocol.rs:917、dispatch.rs:2861-2887） |

- sidebar_width（settings.sidebar_width）は GUI ドラッグ（#307）で完結しているため設定画面には出さない
- 「起動時挙動」に相当する独立設定は現状 tmux_persist のみ（復元 ON/OFF がそれ）。項目名を
  「起動時にセッションを復元（tmux 永続化）」とし、ユーザー語彙に寄せる

### 1.2 外観（Appearance）

| 項目 | UI | 永続先 | dispatch |
|---|---|---|---|
| テーマ | セグメント（ダーク / ライト）+ プリセットドロップダウン | settings.theme | `Theme { action:"set", mode }`（拡張後は preset 名も可。§2.4） |
| 色設定（58 キー） | カテゴリ別アコーディオン。各行 = 名前 + 説明 + スウォッチ + hex 入力 + リセット | settings.theme_colors.<dark\|light> / theme_presets | `Theme { action:"set-color" / "reset-color" / "reset-colors" }`（新 action。§2.4） |
| テーマプリセット | 「現在の色を保存」/ 選択 / 削除 | settings.theme_presets | `Theme { action:"save-preset" / "delete-preset" }` |
| フォント | family テキスト入力 + size 数値入力 | settings.font_family / font_size（新設。§2.6） | `Theme { action:"set-font" }` |

色カテゴリは Theme 構造体の区分（theme.rs:70-133）どおり 6 分割:
ターミナル（21）/ 背景階層（12）/ ボーダー（5）/ テキスト（5）/ アクセント（10）/ UI クローム（5）。詳細は §2。

### 1.3 Code Runner

| 項目 | UI | 永続先 | dispatch |
|---|---|---|---|
| 拡張子→コマンドのテーブル | 全行表示（merged_defaults）。各行 = 拡張子 / コマンド / ソースバッジ（builtin / user）/ 編集 / リセット（user 行のみ削除） | settings.runner_defaults | `RunnerDefaults { ext, command, remove }`（protocol.rs:1072、dispatch.rs:3432-3490） |
| 新規追加 | 拡張子 + コマンドの 2 入力 + 追加ボタン | 同上 | 同上 |
| 変数リファレンス | ヘルプ表示（読み取り専用） | - | - |

- テーブルの元データは `tako_core::merged_defaults()`（runner.rs:342。builtin 21 個
  runner.rs:24-48 に user を重ねたもの）。dispatch `RunnerDefaults`（ext 省略）の全一覧応答が
  source: user / builtin を返すのでそれをそのまま描画する
- builtin 行の「編集」は user 上書きの作成（`RunnerDefaults { ext, command }`）、
  user 行の「リセット」は `remove: true`（効果 = builtin へ戻る。builtin が無い拡張子は行が消える）
- 「フォールバック挙動」の解釈: 解決順はファイル内宣言 → 拡張子既定（user → builtin）→
  エラー（runner.rs `resolve`）で固定であり、設定可能なフォールバックは「builtin を user で
  上書き / user を消して builtin へ戻す」こと。この因果をタブ内ヘルプに 2 行で明記する
- 変数リファレンス（expand_variables、runner.rs:287-321）: `${file}`（絶対パス）/
  `${fileDir}` / `${fileBase}` / `${fileNoExt}` / `${ext}`。全てシェルクォート済み

### 1.4 セットアップ（Setup）

tako setup 相当の確認・変更。読み取りは既存 dispatch の status 系、変更は対応 dispatch。

| 項目 | UI | dispatch |
|---|---|---|
| エージェント CLI 検出状態 | claude / codex / agy の導入・認証・プラン表示 + 再検出 | `SetupRun` の検出ロジック（setup.rs）を read-only で呼ぶ status API（§5.3 の拡張参照） |
| プロファイル（profiles/default.yaml 等） | 一覧 + 選択プロファイルの主要フィールド編集（master_agent / model / effort / worker_agent / skip_permissions / tab_naming_convention） | `OrchestratorProfiles { list / show / set }`（protocol.rs:489） |
| MCP 登録状態 | 登録済み判定表示 + 「登録する」ボタン | `SetupMcp`（protocol.rs:464） |
| フルディスクアクセス（FDA） | 状態表示 + 「システム設定を開く」 | `Fda { action:"status" / "open" }`（protocol.rs:795、fda.rs） |
| エージェント共通ルール同期 | 状態表示 + 「同期する」 | `AgentsSyncRules { action:"status" / "sync" }`（protocol.rs:810） |
| setup 追従（未適用の変更） | SetupChanges の未適用一覧 + 「tako setup を実行」（新ペインで `tako setup` を起動 = `RunInteractive` 利用） | `SetupChanges`（protocol.rs:802）+ `RunInteractive`（protocol.rs:1020） |
| orchestrator 挙動 | auto_close / auto_push トグル（config.yaml） | `SetupRun { answers: { orchestrator: {...} } }`（部分 answers。setup.rs:140-163） |

- 対話が必要な複雑ケース（プラン再選択・instruction 書き換え）は GUI で作り込まず
  「tako setup を実行」ボタン（ターミナルペインで対話）へ誘導する。設定画面は確認 + 単純変更まで

### 1.5 スリープ防止（Sleep Guard）

| 項目 | UI | 永続先 | dispatch |
|---|---|---|---|
| モード | ラジオ off / on / while-agents-running | settings.sleep_guard_mode | `SleepGuard { action:"set", mode }`（protocol.rs:824、dispatch.rs:2526-2622） |
| 電源条件 | ラジオ ac-only / always | settings.sleep_guard_power | 同上（power_condition） |
| 蓋閉じ継続 | ラジオ off / while-agents-running + sudoers 登録・解除ボタン | settings.lid_sleep_mode | 同上（lid_sleep_mode / install-lid-sleep / remove-lid-sleep） |
| 現在状態 | busy_agents・thermal_state の read-only 表示（#440 ポップオーバーと同素材） | - | `SleepGuard { action:"status" }` |

enum 値は sleep_guard.rs:40-113（SleepGuardMode / LidSleepMode / PowerCondition）。

### 1.6 リモート（Remote）

設定というより状態 + 操作が主体。

| 項目 | UI | dispatch |
|---|---|---|
| デーモン状態 | running / URL（トークンマスク済み）/ transport 表示 | `RemoteStatus`（既存。マスクは #104 の既定） |
| 開始 / 停止 | ボタン | `RemoteStart` / `RemoteStop`（protocol.rs:705-717） |
| セットアップ状態 | tailscale 検出・serve 可否 + 不足の案内 | `RemoteSetup`（protocol.rs:739） |
| デバイス一覧 | ペアリング済みデバイス表示 + 失効 | `RemoteDevices`（protocol.rs:730） |

### 1.7 高度（Advanced）

| 項目 | UI | 挙動 |
|---|---|---|
| settings.json 直接編集 | パス表示 + 複数行テキストエディタ + 保存 / 再読み込み | 保存時に serde パース検証（`serde_json::from_str::<Settings>`）→ 失敗は行番号つきエラー表示・書き込まない。成功時は settings::save + 全設定の再適用（§4.4 apply_settings） |
| 外部で開く | 「Finder で表示」「エディタで開く」ボタン | FileOp Reveal 相当 / `open -t` |
| 関連ファイルへの案内 | config.yaml / profiles/ / projects.yaml のパス一覧（read-only） | 表示のみ + Finder で表示 |

---

## 2. 色設定のスキーマ（最重要）

### 2.1 現在のテーマシステム（調査結果）

`tako_core::theme::Theme`（theme.rs:66-139）が全描画色の一元管理（FR-4。直書き禁止）。
色値は計 **58**（単色フィールド 42 + ansi 16 色）。全フィールドとも `Rgb`（sRGB u8×3、theme.rs:9)。
ビルトインは `default_dark()`（Catppuccin Mocha、theme.rs:157-235）と
`default_light()`（Catppuccin Latte、theme.rs:240-318）の 2 つ。
モードは `ThemeMode { Dark, Light }`（theme.rs:41）、settings.theme（"dark"/"light"）から
`Settings::theme_mode()`（settings.rs:135-137。不明値は Dark フォールバック）で解決され、
`Theme::for_mode()`（theme.rs:149）で実体化される。GUI 側は `TakoApp.theme` に保持
（main.rs:1779）、切替は `set_theme_mode`（main.rs:11876。`Theme::for_mode` で全置換）。

**全 58 色キーの一覧とユーザーカスタマイズ可否**。全キーがユーザーカスタマイズ**可能**とする
（Theme 構造体のフィールドはすべて描画に使われており、隠す理由がない）。
settings.json のキー名は Theme フィールド名と 1:1（snake_case）、ansi のみ named key に展開する:

| カテゴリ | キー（settings.json / dispatch 共通） | 対応 UI 要素（theme.rs コメント由来） |
|---|---|---|
| ターミナル (21) | background | ターミナル背景（Base） |
| | foreground | ターミナル前景文字 |
| | cursor / cursor_text | カーソル / カーソル上の文字 |
| | selection_background | 選択範囲の背景 |
| | ansi_black, ansi_red, ansi_green, ansi_yellow, ansi_blue, ansi_magenta, ansi_cyan, ansi_white | ANSI 0-7（theme.rs:73 `ansi[16]` の 0-7） |
| | ansi_bright_black … ansi_bright_white（8 個） | ANSI 8-15 |
| 背景階層 (12) | crust | 最暗の外殻（theme.rs:79） |
| | mantle | タブバー背景と同系の面 |
| | surface_0 / surface_1 / surface_2 | パネル・カードの面（暗い順） |
| | surface_hover | ホバー面 |
| | surface_highlight | 強調面（選択行等） |
| | chip_surface | cwd チップ・カード類（theme.rs:86-87） |
| | shelved_surface | 退避（shelved）行（theme.rs:88-89） |
| | surface_hover_strong | ドロップダウン行等の強ホバー（theme.rs:90-91） |
| | danger_header / danger_surface | 失敗ペインのヘッダ / 失敗タブカード（theme.rs:92-95） |
| ボーダー (5) | border_inner | サイドバー等の内側罫線（theme.rs:98-99） |
| | border_subtle / border_default / border_strong / border_heavy | 罫線階層（薄い順） |
| テキスト (5) | text_secondary / text_tertiary / text_muted / text_faint / text_overlay | 文字階層（明るい順。text_muted が UI 最頻出 theme.rs:108-109） |
| アクセント (10) | accent | アクセント（フォーカス枠・リンク・アクティブ要素） |
| | accent_muted | 非フォーカスのアクセント減光（ペイン番号バッジ等 theme.rs:115-116） |
| | accent_border_muted | アクセント系ボーダー減光（ミニマップ非フォーカス枠 theme.rs:117-118） |
| | idle_border | 待機要素の枠（theme.rs:119-120） |
| | green / red / yellow | 状態色（実行中 / 失敗 / 注意） |
| | teal / mauve / peach | 補助アクセント |
| UI クローム (5) | pane_border | ペイン境界線 |
| | tab_bar_background | タブバー背景 |
| | tab_active_background / tab_active_foreground | アクティブタブの面 / 文字 |
| | tab_inactive_foreground | 非アクティブタブの文字 |

フォント 3 フィールド（font_family / font_size / line_height、theme.rs:136-138）は §2.6。

### 2.2 settings.json のスキーマ

`Settings`（settings.rs:12-66）に 2 フィールドを追加する（すべて `#[serde(default)]` で後方互換）:

```jsonc
{
  // 既存: "dark" / "light"。拡張後はプリセット名も可（例 "my-ocean"）
  "theme": "dark",

  // 新設: ビルトイン dark / light への部分上書き。キーは §2.1 の 58 色名、値は "#RRGGBB"
  "theme_colors": {
    "dark":  { "accent": "#89b4fa", "background": "#101018" },
    "light": { "accent": "#1e66f5" }
  },

  // 新設: 名前付きカスタムプリセット。base のビルトインに colors を重ねた完成形
  "theme_presets": {
    "my-ocean": {
      "base": "dark",                      // "dark" / "light"（ThemeMode と ANSI 既定の起点）
      "colors": { "accent": "#00ced1", "ansi_blue": "#00a0c0" }
    }
  }
}
```

Rust 型（tako-control::settings に追加）:

```rust
#[serde(default)]
pub theme_colors: BTreeMap<String, BTreeMap<String, String>>,   // "dark"/"light" → 色名 → hex
#[serde(default)]
pub theme_presets: BTreeMap<String, ThemePreset>,

#[derive(Serialize, Deserialize, ...)]
pub struct ThemePreset {
    pub base: String,                          // "dark" / "light"
    #[serde(default)]
    pub colors: BTreeMap<String, String>,      // 色名 → hex
}
```

構造体フィールド 58 本ではなく `BTreeMap<String, String>` を採る理由: 部分上書きの表現が自然
（未指定 = ビルトイン値）、未知キーを警告に落とせる、UI のテーブル描画・dispatch の
key/value 受け渡しが一列で書ける。既知キー集合はコード生成でなく
`Theme::COLOR_KEYS: &[&str]`（58 要素の const）を tako-core に置き、getter/setter を
`Theme::color(&self, key) -> Option<Rgb>` / `Theme::set_color(&mut self, key, Rgb) -> bool`
の match で実装する（テストで COLOR_KEYS と match の網羅一致を機械検査する）。

### 2.3 テーマ解決ロジック

tako-core::theme に追加する純関数（GPUI 非依存・単体テスト可能）:

```rust
/// "#RRGGBB"（6 桁のみ。3 桁 #RGB は非対応と明記）を Rgb へ
pub fn parse_hex_color(s: &str) -> Option<Rgb>;

impl Theme {
    /// overrides を適用。未知キー・不正 hex は無視して警告文字列で返す（fail-soft）
    pub fn apply_overrides(&mut self, overrides: &BTreeMap<String, String>) -> Vec<String>;
}
```

tako-control::settings に解決の入口を追加:

```rust
impl Settings {
    /// theme 値 → Theme 実体。優先順:
    /// 1. theme_presets[theme] があれば base のビルトイン + preset.colors
    /// 2. theme が "dark"/"light" ならビルトイン + theme_colors[theme]
    /// 3. どちらでもなければ default_dark()（既存フォールバックと同じ）
    pub fn resolve_theme(&self) -> (Theme, Vec<String>);   // 警告つき
}
```

呼び替え箇所: `TakoApp::new` の初期化（main.rs:1779 `Theme::for_mode(...)` →
`settings.resolve_theme()`）、`set_theme_mode`（main.rs:11880）、`toggle_theme`
（main.rs:5163）。`theme_mode()`（settings.rs:135）はプリセット名なら base のモードを返すよう拡張。

**toggle の意味論**: プリセット選択中に `Theme { action:"toggle" }` / タブバーのテーマボタンを
押した場合は「base の反対側のビルトイン」へ切り替える（プリセットから離脱する）。
dark ↔ light の単純往復という既存の意味（dispatch.rs:2649-2652）を保つため。

### 2.4 dispatch の拡張（`Request::Theme`）

既存 `Theme { action, mode }`（protocol.rs:840-846、実装 dispatch.rs:2624-2669）に
フィールドと action を追加する。既存 3 action（status / set / toggle）の互換は完全維持:

```rust
Theme {
    action: Option<String>,   // 既存: status / set / toggle
                              // 追加: colors / set-color / reset-color / reset-colors
                              //       save-preset / delete-preset / set-font
    mode: Option<String>,     // set: "dark" / "light" / プリセット名（拡張）
    // --- 追加フィールド（全部 Option、serde default） ---
    target: Option<String>,   // 色操作の対象: "dark" / "light" / プリセット名。省略 = 現在の theme
    key: Option<String>,      // set-color / reset-color の色名（§2.1 の 58 キー）
    value: Option<String>,    // set-color の "#RRGGBB"
    name: Option<String>,     // save-preset / delete-preset のプリセット名
    font_family: Option<String>, // set-font
    font_size: Option<f32>,      // set-font（8.0〜32.0 にクランプ）
}
```

各 action の仕様:

- `colors`: 解決済み全 58 色（hex）+ 各キーの source（builtin / override）+ 現在の
  theme / 利用可能プリセット一覧を返す（GUI とCLI 一覧の共通データ源）
- `set-color`: target（省略時は現在 theme。ビルトイン名なら theme_colors[target] へ、
  プリセット名なら theme_presets[target].colors へ）に key=value を書き settings::save。
  value は parse_hex_color 検証、不正は InvalidParams。現在表示中のテーマが対象なら
  `host.set_theme_mode` 相当の再解決（新設 `host.reload_theme()`。§4.3）で即時反映
- `reset-color` / `reset-colors`: 上書きの削除（1 キー / 全キー）
- `save-preset`: name 検証（`[a-z0-9-]{1,32}`、"dark"/"light" は予約で拒否）。現在の解決済み
  テーマとビルトイン base との**差分だけ**を colors に保存（フル 58 キー保存はしない。
  ビルトイン更新へ追従できる）
- `delete-preset`: 削除。theme が該当プリセットを指していたら theme を base へ戻す
- `set-font`: settings.font_family / font_size を更新（§2.6）

永続化は既存 Theme dispatch と同じ `TAKO_SELF_TEST` ガード方針（dispatch.rs:2655）。

CLI（tako-cli。既存 `tako theme [dark|light|toggle]` の拡張）:

```
tako theme                                  # 現在値（プリセット名含む）
tako theme <dark|light|プリセット名>          # 切替
tako theme toggle
tako theme colors [--json]                  # 58 色一覧（source つき）
tako theme color <key> <#RRGGBB> [--target <t>]
tako theme color <key> --reset [--target <t>]
tako theme reset-colors [--target <t>]
tako theme preset save <name>
tako theme preset delete <name>
tako theme font [<family>] [--size <pt>]
```

MCP: `tako_theme` の inputSchema に追加フィールドを足すだけ（新ツールなし）。

### 2.5 ライブプレビュー

- 色変更 dispatch の末尾で `host.reload_theme()`（新設 ControlHost メソッド。TakoApp 実装は
  `self.theme = settings.resolve_theme().0` + `cx` 不要）→ dispatch 完了後の `cx.notify()` で
  全 GPUI ウィンドウが invalidate される（#339 の viewport 方式: entity を描画中の
  全ウィンドウは notify で自動再描画。main.rs:14400-14402 のコメントが根拠）
- 設定ウィンドウ自身（別 entity）は `cx.observe(&tako_app)` で追随（§4.2）
- 適用タイミングは hex 入力の確定時（Enter / フォーカスアウト）。1 文字ごとの適用はしない
  （不正中間値と settings::save 連打を避ける）

### 2.6 フォント設定

settings.json に `font_family: Option<String>` / `font_size: Option<f32>` を追加
（None = ビルトイン既定 Menlo / 13.0。theme.rs:231-233）。resolve_theme で Theme の
font_family / font_size / line_height に反映し、line_height は `font_size * (17.0/13.0)` の
比率固定で自動算出する（個別指定は作らない。既存比率 theme.rs:232-233 の維持）。
ターミナルグリッドの再レイアウトが走ることを実装時に確認する（既存 ZoomIn/ZoomOut =
cmd+= / cmd+- がランタイムのフォントサイズ変更を既にやっているため、同経路に乗せる）。

---

## 3. ウィンドウの実装方式

### 3.1 独立 GPUI ウィンドウ + 専用 root view

- `SettingsWindow` entity（新規モジュール `crates/tako-app/src/settings_window.rs`）を
  root view にした独立ウィンドウを `cx.open_window()` で開く。GPUI はウィンドウごとに
  任意の root view 型を取れる（open_primary_window main.rs:14373-14398 と同形で、
  返す view を `cx.new(SettingsWindow::new)` にするだけ）
- メインウィンドウ群（TakoApp entity 共有の viewport 方式 #339）とは**別 entity**。
  モーダルではなく、開いたままメイン操作可能（macOS System Settings と同じ非モーダル独立ウィンドウ）

```rust
struct SettingsWindow {
    tako_app: WeakEntity<TakoApp>,      // dispatch 呼び出しと observe の対象
    tab: SettingsTab,                    // 選択中タブ（General / Appearance / Runner / Setup / Sleep / Remote / Advanced）
    settings: Settings,                  // 表示用スナップショット（observe で reload）
    // タブ別の編集状態（hex 入力中テキスト、advanced エディタバッファ、非同期取得キャッシュ等）
    ...
}
```

### 3.2 レイアウト

- macOS System Settings 風: 左に縦ナビ（幅 180px。アイコン 16px + ラベル、選択行は
  surface_highlight + accent）、右にコンテンツ（スクロール可、`overflow_y_scroll`）
- タイトルバーは**標準**（`appears_transparent` を使わない。tako_titlebar_options
  main.rs:14313-14321 はメイン専用）。タイトルは `tr!("tako 設定", "tako Settings")`
- サイズ: 初期 760x560、最小 620x420。ウィンドウ位置・サイズは**永続化しない**（毎回センター。
  layout.json の windows[] に含めない = 復元対象外。§8.4）
- UI 部品は既存実装を流用: テキスト入力 = sidebar.rs のインライン入力（EntityInputHandler、
  IME 対応）を共通コンポーネント化、ドロップダウン = limit_service メニュー（status_bar.rs）、
  アコーディオン = right_panel.rs、トグル/ボタン = 既存スタイル踏襲

### 3.3 開く経路と単一インスタンス

1. **Cmd+,**: keybindings.rs の `actions!` に `OpenSettings` を追加し
   `KeyBinding::new("cmd-,", OpenSettings, None)`（cmd-, は現状未使用。§8.5）。
   ハンドラは #103 の教訓どおり `cx.on_action` の**グローバル登録**（フォーカスパス非依存。
   main.rs の Quit と同所）
2. **コマンドパレット**: palette_items / palette_execute（main.rs:5241 / 5388）に
   `"open-settings"` を追加。ラベルは ui_text/palette.rs に `tr!` で追加
3. **CLI / MCP**: `tako settings [--tab <名>]` / `tako_settings`（§5.1）

単一インスタンス: `TakoApp.settings_window: Option<WindowHandle<SettingsWindow>>` を保持。
既に開いていれば `activate`（前面化）+ タブ指定があれば切替のみ。dispatch は GPUI の
Window/Context を取れないため、`pending_viewport_opens`（main.rs:11820-11821）と同じ
**pending キューパターン**を使う: dispatch ハンドラは
`host.open_settings_window(tab)` → TakoApp が `pending_settings_open = Some(tab)` を立て、
render の sync 段（process_pending_new_windows main.rs:9930 と同所）で実ウィンドウを開く。

### 3.4 閉じる・ライフサイクル

- 赤ボタン / Cmd+W で閉じる。root div に `on_action(ClosePane => close window)` を付けて
  Cmd+W を設定ウィンドウ内では「ウィンドウを閉じる」に読み替える（メインの ClosePane =
  ペイン close はメインウィンドウの root にのみ効く）
- `on_window_should_close` で `TakoApp.settings_window = None` に戻すだけ。タブ合流等の
  メイン用 close 処理（handle_window_close）は通さない
- 設定ウィンドウだけが残った状態は作らない前提にしない（メイン全 close 後も設定ウィンドウは
  独立に生存し得る）。#381 の Dock 復帰（reopen_or_restore main.rs:14345）はメイン
  viewport のみ数えるため干渉しないが、受け入れ条件で明示検証する（§7 M1 / §8.4）

---

## 4. settings.json との双方向バインディング

### 4.1 読み込み

- 設定ウィンドウを開くたび `settings::load()`（settings.rs:156-160。不在・破損は既定値）で
  スナップショットを取得
- render 毎のファイル IO はしない（60fps での都度 load は作法として避ける）

### 4.2 変更 → 永続化 → ライブ反映（一方向ループ）

```
設定 UI 操作
  → tako_app.update(cx, |app, cx| dispatch::dispatch(app, Request::…, PaneOrigin::User))
      （dispatch 内で settings::save + host setter。既存コードパスそのまま）
  → cx.notify()（TakoApp）
  → 全メインウィンドウ再描画（entity 共有の自動 invalidate）
  → SettingsWindow の observe(TakoApp) 発火 → settings::load() で再読込 → 自身も再描画
```

- 書き込みは**個別設定の変更のたびに即時**（既存 dispatch がそうなっている。
  settings::save は tmp 書き + rename のアトミック書き settings.rs:167-189）
- 「保存」ボタンは高度タブのテキスト編集のみ（§1.7）。他タブは即時反映で Apply/OK ボタンなし
  （macOS System Settings の作法）
- CLI / MCP / master 会話（「言語変えて」）からの変更も同じ dispatch → notify を通るため、
  **開いている設定画面へ自動反映**される（observe が拾う）。追加実装ゼロで要件 6 を満たす

### 4.3 ControlHost の追加メソッド

| メソッド | 用途 |
|---|---|
| `open_settings_window(&mut self, tab: Option<String>)` | Settings dispatch の pending キュー投入（既定実装は no-op。host.rs の他 setter と同様） |
| `reload_theme(&mut self)` | settings から resolve_theme し直して self.theme を差し替え（色変更・preset 切替・set-font 用） |

既存 setter（set_theme_mode host.rs:199 / set_ui_lang host.rs:206 / set_auto_rename
host.rs:147 等）はそのまま使う。設定系 setter が `Context` 不要（`&mut self` のみ）で
あることは確認済み（main.rs:11828-11919）。

### 4.4 settings.json の外部直接編集

- エディタで直接書き換えた場合に GUI が自動追従する仕組みは**現状なく、今回も作らない**
  （ファイル watcher の新設 + 全設定の適用経路整備が必要になり過剰。
  preview_watch.rs の OS ネイティブ監視を流用すれば将来可能、とだけ記す）
- 代わりに高度タブへ「再読み込み」ボタンと、保存時の一括適用関数
  `TakoApp::apply_settings(&mut self, s: &Settings)` を新設する。中身は既存 setter の列挙
  （set_auto_rename は永続化込みなので、適用専用に「永続化なし setter」へ分離するか、
  適用順を settings::save 最後の 1 回にまとめる。実装時は後者: バッファ全体を save してから
  各ランタイム状態を settings 値で上書きする）

---

## 5. MCP / CLI / dispatch 1:1

### 5.1 新設 dispatch: `Settings`

```rust
/// 設定ウィンドウを開く（Issue #459）。個別設定の変更は各既存 dispatch を使う
Settings {
    action: Option<String>,   // "open"（既定）/ "status"（開いているか + 現在タブ）
    tab: Option<String>,      // general / appearance / runner / setup / sleep / remote / advanced
}
```

- CLI: `tako settings [--tab <名>]`（tako-cli に追加）
- MCP: `tako_settings`（**107 個目**。説明文に「個別設定は tako_lang / tako_theme /
  tako_run_defaults 等を使う。これは画面を開く操作」と明記し、AI の誤用を防ぐ）
- スナップショット更新: `crates/tako-app/testdata/mcp_tools_snapshot.txt`（現在 106 行）に
  1 行追加（#358 の tools/list スナップショット検証。main.rs:16265-16313）

### 5.2 既存 dispatch の GUI フロントエンド化（新設不要の確認）

§1 の表のとおり、以下は**既存のまま**設定画面から呼ぶ:
`Lang` / `AutoRename` / `PortDetect` / `Persist` / `Telemetry` / `LimitService` /
`PreviewReload` / `PreviewCache` / `Logs(set)` / `RunnerDefaults` / `SleepGuard` /
`Theme(status/set/toggle)` / `Fda` / `SetupMcp` / `SetupChanges` / `SetupRun` /
`AgentsSyncRules` / `OrchestratorProfiles` / `RemoteStart/Stop/Setup/Devices` / `RunInteractive`。

### 5.3 不足していて新設・拡張が必要な dispatch 一覧

| # | 対象 | 内容 | 規模 |
|---|---|---|---|
| 1 | `Settings`（新設） | 設定画面を開く / 状態照会（§5.1） | 新バリアント + CLI + MCP 1 ツール |
| 2 | `Theme`（拡張） | colors / set-color / reset-color / reset-colors / save-preset / delete-preset / set-font の 7 action + 5 フィールド（§2.4） | 既存バリアントの後方互換拡張 |
| 3 | `ConfirmClose`（拡張） | 現在ランタイムのみ（set_confirm_close main.rs:11868-11870 は永続化なし。初期値は config.yaml confirm_close setup.rs:315-316）。dispatch に config.yaml への保存を追加 | ハンドラ数行 |
| 4 | エージェント CLI 検出の read-only status（拡張） | setup の検出ロジック（導入・認証・プラン）を照会専用で返す。`SetupChanges` と対にする `SetupStatus`（新設）か `Fda` 同様の action 追加かは実装時判断。GUI セットアップタブの表示データ源 | 検出関数は setup.rs に実装済み・公開形を整えるだけ |

---

## 6. 既存設定システムの調査結果（この設計の根拠）

### 6.1 settings.json の現行スキーマ（17 フィールド）

`Settings`（crates/tako-control/src/settings.rs:12-66）。パスは `<data_dir>/settings.json`
（settings_path settings.rs:151-153、data_dir は tako_core::paths）:

| フィールド | 型 | 既定 |
|---|---|---|
| auto_rename | bool | true |
| port_detect | bool | true |
| preview_live_reload | bool | true |
| preview_cache_max_mb | u64 | 512 |
| tmux_persist | bool | true |
| sleep_guard_mode | enum off/on/while-agents-running | while-agents-running |
| sleep_guard_power | enum ac-only/always | ac-only |
| lid_sleep_mode | enum off/while-agents-running | off |
| pane_logs | bool | true |
| pane_log_max_mb | u64 | 5 |
| pane_log_total_max_mb | u64 | 200 |
| theme | String | "dark" |
| sidebar_width | u32 | 244 |
| telemetry | bool | false |
| limit_service | String | "claude" |
| language | String | "system" |
| runner_defaults | BTreeMap<String,String> | {} |

- load: 不在・破損は既定値へ fail-soft（settings.rs:156-165）
- save: tmp 書き + rename のアトミック書き（settings.rs:179-189）。
  **注意**: config_io の flock + 世代バックアップ（#169）は settings.json には未適用
  （config.yaml / profiles / projects.yaml のみ）。load→save の RMW 窓が存在する（§8.3）
- 後方互換の作法: 追加フィールドは `#[serde(default)]`（settings.rs:3 のモジュールコメントが規約）

### 6.2 設定を書く経路の全列挙

1. **dispatch 経由**（正道。§1 の表の各バリアント）: `load() → 変更 → save()` +
   ControlHost setter で GUI 反映。`TAKO_SELF_TEST` / `cfg!(test)` 中は save しない方針が
   横断適用されている（dispatch.rs:2655, 2695, 2735, 2774 / main.rs:5165, 5186, 11831, 11855）
2. **GUI 直接**（dispatch と同じ状態遷移を手元実装するパターン）: toggle_theme
   （main.rs:5157-5173）、toggle_language（main.rs:5177-5194）、save_sidebar_width
   （main.rs:5196-5205）、ControlHost 実装内の set_auto_rename / set_port_detect
   （main.rs:11828-11862）
   → **設定画面はこのパターンを増やさず、§4.2 のとおり dispatch 直呼びに統一する**
3. **起動時読み込み**: TakoApp::new が theme / sidebar_width / limit_service / language /
   pane_log_config / sleep_guard を個別 load（main.rs:1779-1975）

### 6.3 テーマの現行実装

§2.1 に記載。加えて: ThemeMode パース失敗は Dark フォールバック
（settings.rs:272-283 のテストで固定済み）= theme フィールドへプリセット名を入れても
**旧バイナリで安全に dark へ落ちる**（§8.1 の後方互換根拠）。

### 6.4 dispatch 基盤

- `dispatch(host: &mut dyn ControlHost, request, origin)`（dispatch.rs:62-71）は UI スレッドで
  同期実行。perf_span 計測つき。GUI 内から直接呼べる（origin は `PaneOrigin::User`。
  pane.rs:45-54）
- ControlHost trait（host.rs）は設定系 getter/setter を既定 no-op で持つ（host.rs:147-220 等）
- MCP ツールは現在 **106**（crates/tako-app/testdata/mcp_tools_snapshot.txt = 1 行 1 ツール、
  106 行で実測確認）

### 6.5 関連ファイル（settings.json 以外の設定永続層）

| ファイル | 内容 | 設定画面での扱い |
|---|---|---|
| config.yaml（orchestrator） | confirm_close / agents_sync / spawn_layout / auto_close / auto_push / ctx_threshold 等（setup.rs:249-360） | セットアップタブで主要のみ。残りはパス案内 |
| profiles/*.yaml | Profile: master_agent / model / effort / worker_model_policy / worker_agent(s) / prompt_blocks / tab_naming_convention（orchestrator/mod.rs:292-332） | セットアップタブで主要のみ（OrchestratorProfiles set 経由） |
| projects.yaml | orchestrator プロジェクト | 対象外（`tako orchestrator projects` へ案内のみ） |
| layout.json | タブ・ペイン・ウィンドウ復元 | 対象外（設定ではない） |

---

## 7. 実装マイルストーン分割

各 M は独立 PR・全品質ゲート（fmt / clippy -D warnings / test / セルフテスト）通過を前提。
UI 文字列は各 M で `ui_text/settings.rs`（新設）に `tr!` 日英併記（M7 でまとめない）。

### M1: ウィンドウ枠 + タブナビ + 開く 3 経路

- SettingsWindow entity + 独立ウィンドウ + 左ナビ 7 タブ（中身はプレースホルダ）
- Cmd+,（グローバル on_action）/ パレット「設定を開く」/ dispatch `Settings` + CLI
  `tako settings` + MCP `tako_settings` + pending キュー
- 単一インスタンス（再実行で前面化 + タブ切替）
- **受け入れ（機械検証）**:
  - セルフテスト新項目: dispatch `Settings{open}` → 応答 ok、`cx.windows().len()` が +1、
    再実行で増えない（単一インスタンス）、`Settings{status}` が現在タブを返す
  - tools/list スナップショット 107 行で一致
  - 隔離実機（TAKO_ISOLATED=1）: Cmd+, で開く・Cmd+W で閉じる・メインウィンドウ全 close 後も
    プロセス生存 + Dock 復帰正常（#381 回帰なし）のスクリーンショット記録

### M2: 一般タブ

- §1.1 の 10 項目（トグル / ドロップダウン / 数値入力）を dispatch 直呼びで実装
- ConfirmClose の config.yaml 永続化（§5.3-3）
- observe(TakoApp) による外部変更の追随
- **受け入れ**: セルフテストで①GUI トグル相当の dispatch → settings.json 値変化（隔離
  データディレクトリで実ファイル検証）②`tako lang en` 実行 → 設定画面スナップショットの
  language 表示が en に追随（observe 経路)③不正値（preview_cache_max_mb 範囲外）が
  InvalidParams で拒否され UI にエラー表示

### M3: 外観タブ（色設定 + プリセット + ライブプレビュー）

- tako-core: parse_hex_color / COLOR_KEYS / apply_overrides + 単体テスト
  （58 キー網羅・不正 hex・未知キー警告）
- settings: theme_colors / theme_presets / font_family / font_size + resolve_theme
- dispatch Theme 拡張 7 action + CLI `tako theme color/colors/preset/font` + MCP スキーマ拡張
- 外観タブ UI: カテゴリ別アコーディオン 58 行 + スウォッチ + hex 入力 + リセット +
  プリセット保存/切替/削除 + フォント
- **受け入れ**: ①`tako theme color accent "#ff0000"` → colors 応答の accent が
  source=override で #ff0000、GUI 実ピクセル変化（隔離実機 screencapture でタブバー
  アクセント色の RGB 実測）②reset-colors で既定復帰③`preset save` → `tako theme <name>` →
  再起動後も維持（隔離実機）④不正 hex / 未知キー / 予約名 preset が InvalidParams
  ⑤旧 settings.json（新フィールドなし）読み込みで挙動不変（後方互換テスト）

### M4: Code Runner タブ

- merged_defaults テーブル（builtin/user バッジ・編集・リセット・新規追加）+ 変数ヘルプ
- **受け入れ**: ①`tako run-default py "python3.12 ${fileBase}"` → 設定画面テーブルに
  source=user で反映②画面から追加した拡張子が `tako run-default` 一覧と settings.json
  runner_defaults に出る③user 行リセットで builtin 表示へ戻る（セルフテスト +
  隔離データディレクトリの実ファイル検証）

### M5: セットアップタブ

- エージェント CLI 検出表示（§5.3-4 の status 公開形整備を含む）/ プロファイル主要編集 /
  MCP 登録 / FDA / ルール同期 / setup 追従 + 「tako setup を実行」ボタン
- **受け入れ**: ①表示値が `Fda{status}` / `SetupChanges` / `OrchestratorProfiles{show}` の
  応答と一致（セルフテストで同一 dispatch を叩き比較）②「tako setup を実行」で
  RunInteractive ペインが開き `tako setup` が走る（隔離実機）

### M6: スリープ防止 + リモート + 高度タブ

- SleepGuard 3 設定 + 状態表示 / Remote 状態 + start/stop + devices / settings.json
  テキストエディタ（パース検証 + apply_settings 一括適用 + 再読み込み）
- **受け入れ**: ①sleep-guard set が settings.json と `tako sleep-guard status` に一致
  ②高度タブで不正 JSON 保存 → 拒否 + エラー表示、正常 JSON 保存 → 全タブ表示と GUI
  （テーマ等）が新値に一致③remote start/stop の状態遷移表示（隔離 + TAKO_REMOTE_TEST_MODE）

### M7: 仕上げ（1:1 監査 + ドキュメント）

- §1 の全項目について「GUI 操作 ↔ CLI/MCP」の 1:1 を表で監査し、取りこぼしを回収
- AGENTS.md コマンド表 / docs（CLI リファレンス・MCP ツール一覧）/ manual-checks.md
  （IME 入力・スクロール・ライト/ダーク見た目等の人手確認項目）更新
- **受け入れ**: セルフテスト全緑（FAILED 0）+ ツール数 107 + docs build 緑

---

## 8. エッジケース・リスク

### 8.1 settings.json の後方互換

- 追加フィールドは全部 `#[serde(default)]`。旧ファイル読み込みの既定値テストを
  settings.rs の既存テスト（「不在や破損は既定値になる」settings.rs:231-269）に追記
- theme へのプリセット名格納は、旧バイナリでは ThemeMode::parse 失敗 → Dark フォールバック
  （settings.rs:136）で安全。ダウングレード時も起動不能にならない
- serde は未知キーを無視する（settings.rs:3）ため、新バイナリで書いた設定を旧バイナリが
  読み書きすると **theme_colors 等が消える**（旧 save が新フィールドを持たない）。
  既知の制約として明記し、対策はしない（ダウングレード運用は非サポート）

### 8.2 色設定の妥当性

- hex は `#RRGGBB` 6 桁のみ受理。dispatch 経由の不正値は InvalidParams で拒否
- settings.json 直編集の不正値・未知キーは resolve_theme が**無視 + 警告**（fail-soft。
  起動不能・パニックにしない）。警告は stderr と高度タブに表示
- コントラスト崩壊（background = foreground 等）は検証しない（ユーザーの自由）。
  ただし reset-colors でワンタッチ復帰できることを保証する。プリセット削除で参照中 theme は
  base へ自動復帰（ダングリング参照を作らない）

### 8.3 同時操作・競合

- 設定ウィンドウと CLI/MCP の並行変更: 同一プロセス内は dispatch が UI スレッド直列
  （dispatch.rs:58-59）なので競合しない。**別プロセス**（CLI の別コマンド同時実行）は
  settings::save の load→save RMW 窓で後勝ち上書きがあり得る（既存の制約。config_io の
  flock 未適用 §6.1）。頻度・実害が小さいため今回は受容し、config_io 移行を別 Issue として
  起票することを M7 に含める
- 設定ウィンドウ内の表示 stale: observe で解消（§4.2）。observe が取りこぼす
  非 dispatch 経路の変更（外部エディタでの直編集）は §4.4 のとおり手動再読み込み

### 8.4 GPUI 独立ウィンドウの制約

- ウィンドウ枚数と終了・復帰判定: #381 の reopen_or_restore（main.rs:14345-14367）と
  handle_window_close はメイン viewport（TakoApp の windows）だけを対象にしており、
  設定ウィンドウ（別 root view）は数に入らない設計とする。ただし GPUI の
  `cx.windows()` を枚数判定に使う箇所が将来含めて誤カウントしないよう、M1 受け入れで
  「メイン全 close + 設定ウィンドウ残存 → Dock 復帰正常」を必ず検証する
- layout.json の windows[]（#339 persist）に設定ウィンドウを**含めない**（復元対象外）。
  復元コードが未知ウィンドウを作らないことは既存構造のまま（SettingsWindow は
  workspace.windows() に登録しない）
- dispatch から Window を直接作れない制約は pending キュー（§3.3）で回避。
  同パターンの実績: pending_viewport_opens（main.rs:11820）
- キーバインドはプロセス共通（cx 単位）のため、設定ウィンドウで効かせたくないメイン用
  アクション（SplitRight 等）は「root div にハンドラを付けない」ことで自然に無効になる
  （on_action はビュー階層で解決される）。グローバル登録済みの Quit / OpenSettings のみ
  設定ウィンドウでも発火する — Quit は仕様どおり、OpenSettings は前面化で冪等

### 8.5 キーバインド衝突

- cmd-, は keybindings.rs:52-93 の全 38 バインドに存在しないことを確認済み。追加は安全
- 設定ウィンドウ内 Cmd+W（ClosePane アクション）: §3.4 のとおり root div でウィンドウ close に
  読み替え。メインの意味（ペイン close）と混線しない

### 8.6 テスト・検証系

- `TAKO_SELF_TEST` 中は settings::save しない既存方針（§6.2-1）に新 action も従う。
  したがってセルフテストは「応答 JSON + GUI 内状態（TakoApp.theme の Rgb 値等）」を検証し、
  実ファイル永続化はデータディレクトリ隔離（TAKO_ISOLATED=1、#177 / TAKO_DATA_DIR #112）の
  e2e で検証する（M3 受け入れに記載）
- mcp_tools_snapshot.txt の +1 更新を忘れるとセルフテスト 32 が落ちる（意図した設計。
  M1 の受け入れに含む）

### 8.7 実装規模の見積もり注意

- 58 色行の UI は 1 行コンポーネント化（名前 / スウォッチ / 入力 / リセット）で機械的に並べる。
  アコーディオン初期状態は「アクセント」のみ展開（最も触られる）
- セットアップタブはデータ取得が子プロセス実行（claude CLI 等）を含むため、
  タブ表示時に background executor で非同期取得 → 取得中スピナー表示（#168 の
  「UI スレッドで子プロセスを叩かない」教訓を厳守。OffloadJob dispatch.rs:77 参照）
