//! setup 系（claude への MCP 登録・`tako setup` の非対話実行と answers の検証・setup の設定 config.yaml・
//! 共通ルール同期・依存の導入・エージェント CLI の導入の系統名とホーム）の断り（#2365）。
//!
//! dispatch の `tako_setup_mcp` / `tako_setup` / `tako_setup_changes` / `tako_agents_sync_rules` /
//! `tako_setup_deps` / `tako_setup_bootstrap` は、以前は dispatch 自身の日本語の固定文と、中身の口
//! （`setup` / `agents_sync` / `setup_deps` / `setup_bootstrap`）の `Err(String)`（日本語の固定文）を
//! `Operation` / `InvalidParams` の中身としてそのまま載せていたので、英語表示でも理由だけ日本語で返った。
//! 日英の対は [`SetupRefusal::text_in`] の (バリアント, 言語) の網羅 match の 1 か所に置く
//! （形は [`crate::OsRefusal`] と同じ）。日本語は #2365 より前の固定文とバイト一致
//! （`tests/issue1762_dispatch_error_lang.rs` が突き合わせる）。
//!
//! # 口は 2 つ
//!
//! 従来の口（`setup::SetupAnswers::validate` / `setup::load_config` / `agents_sync::run_sync` /
//! `setup_deps::install` / `setup_bootstrap::parse_agent` ほか）は `String` を返し、文は**日本語のまま**
//! （CLI の `tako setup` は枠が日本語の手元の処理 = FR-2.42.21 の「setup は日本語」）。断りを表示言語で
//! 返す dispatch だけが型の口（`*_typed`）を通す。CLI の `tako setup-mcp` は dispatch の
//! `setup_mcp_agents` を手元で呼ぶので、呼ぶ前に表示言語を決める（`init_local_lang`）。
//!
//! codex / agy への MCP 登録の断りは [`crate::agent_mcp::AgentMcpError`]（同じ網羅 match の形）が持つ。
//! エージェント名の検証は orchestrator の [`crate::orchestrator::ConfigError`] を使う。
//!
//! エージェント CLI の導入の実行（`tako_setup_bootstrap` の install = インストーラの取得と実行）と PATH 通し
//! （path / undo-path）の断りもここに置く。中身の tako-core の断り（profile の PATH ブロック
//! [`PathBlockError`]・tako CLI の symlink [`LinkError`]・Windows のユーザー PATH [`UserPathError`]）は
//! それぞれの型の `text_in` を包む（Windows だけが組む文も `cfg` の外の表 = macOS の番犬で日英を見る）

use tako_core::claude_mod_ui::UiError;
use tako_core::i18n::Lang;
use tako_core::load_issue::{JsonReason, YamlReason};
use tako_core::platform::user_path::UserPathError;
use tako_core::shell_profile::PathBlockError;
use tako_core::tako_cli_path::LinkError;

use crate::orchestrator::ConfigError;
use crate::DispatchError;

/// setup 系の断り。`error` / `detail` / `stderr` / `output` / `log` は OS・外部コマンド・serde のエラー文と
/// 子の出力（言語に依らずそのまま載せる）。パス・名前・値は表記のまま埋める
#[derive(Debug, Clone, PartialEq)]
pub enum SetupRefusal {
    // ---- claude への MCP 登録（`dispatch::setup_mcp`） ----
    /// `claude mcp add` を起動できない
    ClaudeMcpAddSpawn(String),
    /// `claude mcp add` が非ゼロ終了（`status` は終了状態の表示 = `exit status: 1`）
    ClaudeMcpAddFailed { status: String, stderr: String },
    /// MCP の設定ファイル（`~/.claude.json` / `.mcp.json`）を読めない
    McpConfigRead { path: String, error: String },
    /// MCP の設定ファイルを JSON として解釈できない（書き換えを中止した）
    McpConfigNotJson { path: String, reason: JsonReason },
    /// MCP の設定ファイルの `mcpServers` がオブジェクトでない
    McpServersNotObject,
    /// MCP の設定ファイルの置き場を作れない
    McpConfigDirCreate { path: String, error: String },
    /// MCP の設定を JSON にできない
    McpConfigSerialize(String),
    /// MCP の設定ファイルへ書けない
    McpConfigWrite { path: String, error: String },

    // ---- `tako setup` の非対話実行（MCP の `tako_setup`） ----
    /// answers が JSON の形として読めない（`error` は serde の文）
    AnswersMalformed(String),
    /// answers を JSON にできない
    AnswersEncode(String),
    /// 子の `tako setup` を起動できない
    SetupSpawn(String),
    /// 子の `tako setup` の標準入力を開けない
    SetupStdinUnavailable,
    /// 子の `tako setup` へ answers を送れない
    AnswersSend(String),
    /// 子の `tako setup` の完了待ちに失敗
    SetupWaitFailed(String),
    /// 子の `tako setup` が非ゼロ終了（`output` は子の出力 = setup の枠なので日本語のまま）
    SetupFailed { code: i32, output: String },

    // ---- answers の検証（`setup::SetupAnswers::validate`） ----
    /// `selected_agent` が claude / codex / agy のどれでもない
    SelectedAgent(String),
    /// `provider_plans` のキーが claude / gpt / google のどれでもない
    ProviderPlanKey(String),
    /// `instruction_content` が空
    InstructionEmpty,
    /// `projects` のキーが空
    ProjectKeyEmpty,
    /// `projects.<key>.cwd` が空
    ProjectCwdEmpty(String),
    /// `launch_agent` が claude / codex / agy / none のどれでもない
    LaunchAgent(String),
    /// `mod_ui` が `tako mod ui` に断られる値
    ModUi(UiError),
    /// `sleep_guard.mode` が off / on / while-agents-running のどれでもない
    SleepMode(String),
    /// `sleep_guard.power` が ac-only / always のどれでもない
    SleepPower(String),
    /// `profile` のエージェント名が不正（orchestrator の断り）
    Profile(ConfigError),

    // ---- setup の設定（config.yaml）と埋め込みの変更履歴（changes.yaml） ----
    /// config.yaml の置き場（ホームディレクトリ）を決められない
    ConfigHomeUnresolved,
    /// config.yaml を読めない
    ConfigRead(String),
    /// config.yaml を解釈できない（種類と位置だけ = 中身を引用しない #2230）
    ConfigParse(YamlReason),
    /// 埋め込みの changes.yaml を解釈できない（ビルドの取り違えでしか起きない）
    ChangelogParse(YamlReason),
    /// 埋め込みの changes.yaml の platforms に未知の値
    ChangelogPlatform { revision: u32, value: String },

    // ---- 共通ルール同期（`agents_sync`） ----
    /// 正本のパスが設定されていない
    SyncSourceUnset,
    /// 正本のファイルが無い
    SyncSourceMissing(String),
    /// 正本のファイルを読めない
    SyncSourceRead(String),
    /// 正本のファイルが空
    SyncSourceEmpty,

    // ---- 依存の導入（`setup_deps::install`） ----
    /// 不明な依存の名前（`allowed` はこの環境の対象を ` / ` で並べたもの）
    UnknownDep { got: String, allowed: String },
    /// 導入器（brew / winget）を起動できない
    DepInstallerSpawn { program: String, error: String },
    /// 導入器が非ゼロ終了（`log` は導入器の出力。空でなければ先頭に改行を付けて載せる）
    DepInstallerFailed {
        command: String,
        code: i32,
        log: String,
    },
    /// 導入器は正常終了したのにコマンドを引けない
    DepMissingAfterInstall { command: String, bin: String },

    // ---- エージェント CLI の導入（`setup_bootstrap`。系統名とホームの解決） ----
    /// 不明な系統名（`allowed` は系統を ` / ` で並べたもの）
    UnknownBootstrapAgent { got: String, allowed: String },
    /// ホームディレクトリを決められない（何も書かない）
    BootstrapHomeUnresolved,

    // ---- エージェント CLI の導入の実行（`setup_bootstrap::install_for_typed`） ----
    /// セルフテスト中の実インストール（dry_run でのみ呼べる）
    InstallDuringSelfTest,
    /// この環境では自動インストールを代行できない（`os` は OS の名前、`command` は公式の 1 行）
    InstallNotDelegable { os: String, command: String },
    /// インストーラの取得手段（curl / wget）が無い
    NoDownloader,
    /// 取得手段（curl / wget / PowerShell）・インストーラのインタプリタを起動できない
    InstallerToolSpawn { program: String, error: String },
    /// インストーラを取得できない
    InstallerFetchFailed { url: String, detail: FetchDetail },
    /// 取得したインストーラを読めない
    InstallerUnreadable(String),
    /// 取得した内容がインストーラでない（プロキシの HTML エラーページ等）
    NotAnInstaller { url: String },
    /// インストーラがメモリ不足で中断した（exit 137。`log` はインストーラの出力）
    InstallerOutOfMemory { log: String },
    /// インストーラが途中で強制終了した（exit 128 以上）
    InstallerKilled { code: i32, log: String },
    /// インストーラが失敗した（`troubleshoot_url` はその系統の公式ページ）
    InstallerFailed {
        product: String,
        code: i32,
        troubleshoot_url: String,
        log: String,
    },
    /// インストーラは正常終了したのにランチャーを引けない（`launcher` は `~` 短縮の表示）
    InstalledButMissing { launcher: String, command: String },

    // ---- PATH 通し（`setup_bootstrap::ensure_path_for_typed` / `undo_path_for_typed`） ----
    /// 使っているシェルの設定ファイルが分からない（`shell` は `$SHELL` の値 = 空なら不明、
    /// `dirs` は通したいディレクトリの `~` 短縮の表示）
    ShellProfileUnknown { shell: String, dirs: Vec<String> },
    /// 使っているシェルの設定ファイルが分からない（PATH ブロックの取り除き）
    ShellProfileUnknownForUndo,
    /// Windows のユーザー PATH へ書いたのに読み直すと入っていない
    UserPathNotReflected { dir: String },
    /// profile の PATH ブロックの書き換えの断り（tako-core）
    PathBlock(PathBlockError),
    /// tako CLI の symlink の断り（tako-core）
    CliLink(LinkError),
    /// Windows のユーザー PATH の読み書きの断り（tako-core）
    UserPath(UserPathError),
}

/// インストーラを取得できなかったときの添え書き（取得手段の stderr か、黙って落ちたときの終了コード）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchDetail {
    /// 取得手段が stderr に何も出さずに非ゼロ終了した
    Exit { program: String, code: i32 },
    /// 取得手段の stderr（前後の空白を落としたもの。言語に依らずそのまま載せる）
    Stderr(String),
}

impl FetchDetail {
    fn text_in(&self, lang: Lang) -> String {
        match (self, lang) {
            (Self::Exit { program, code }, Lang::Ja) => format!("（{program} が exit {code}）"),
            (Self::Exit { program, code }, Lang::En) => format!("({program} exited with {code})"),
            (Self::Stderr(detail), Lang::Ja) => format!("（{detail}）"),
            (Self::Stderr(detail), Lang::En) => format!("({detail})"),
        }
    }
}

impl SetupRefusal {
    /// 表示言語を明示しての文言。(バリアント, 言語) の網羅 match なので、バリアントを足すと
    /// 日英の両方を書くまでコンパイルが通らない。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::ClaudeMcpAddSpawn(e), Ja) => format!("claude mcp add の実行に失敗: {e}"),
            (Self::ClaudeMcpAddSpawn(e), En) => format!("failed to run claude mcp add: {e}"),
            (Self::ClaudeMcpAddFailed { status, stderr }, Ja) => {
                format!("claude mcp add が失敗 (exit {status}): {stderr}")
            }
            (Self::ClaudeMcpAddFailed { status, stderr }, En) => {
                format!("claude mcp add failed ({status}): {stderr}")
            }
            (Self::McpConfigRead { path, error }, Ja) => format!("{path} の読み取りに失敗: {error}"),
            (Self::McpConfigRead { path, error }, En) => format!("failed to read {path}: {error}"),
            (Self::McpConfigNotJson { path, reason }, Ja) => format!(
                "{path} を JSON として解釈できないので書き換えを中止した（{}）。\
                 内容を直すか退避してからやり直してください",
                reason.text_in(lang)
            ),
            (Self::McpConfigNotJson { path, reason }, En) => format!(
                "stopped before rewriting {path} because it cannot be read as JSON ({}). \
                 Fix its contents or move it aside, then try again",
                reason.text_in(lang)
            ),
            (Self::McpServersNotObject, Ja) => "mcpServers がオブジェクトでない".to_string(),
            (Self::McpServersNotObject, En) => "mcpServers is not an object".to_string(),
            (Self::McpConfigDirCreate { path, error }, Ja) => {
                format!("{path} の作成に失敗: {error}")
            }
            (Self::McpConfigDirCreate { path, error }, En) => {
                format!("failed to create {path}: {error}")
            }
            (Self::McpConfigSerialize(e), Ja) => format!("JSON のシリアライズに失敗: {e}"),
            (Self::McpConfigSerialize(e), En) => format!("failed to serialize the JSON: {e}"),
            (Self::McpConfigWrite { path, error }, Ja) => {
                format!("{path} への書き込みに失敗: {error}")
            }
            (Self::McpConfigWrite { path, error }, En) => {
                format!("failed to write {path}: {error}")
            }

            (Self::AnswersMalformed(e), Ja) => format!("setup answers が不正: {e}"),
            (Self::AnswersMalformed(e), En) => format!("the setup answers are malformed: {e}"),
            (Self::AnswersEncode(e), Ja) => format!("setup answers の JSON 化に失敗: {e}"),
            (Self::AnswersEncode(e), En) => {
                format!("failed to encode the setup answers as JSON: {e}")
            }
            (Self::SetupSpawn(e), Ja) => format!("tako setup の起動に失敗: {e}"),
            (Self::SetupSpawn(e), En) => format!("failed to start tako setup: {e}"),
            (Self::SetupStdinUnavailable, Ja) => "tako setup の標準入力を開けない".to_string(),
            (Self::SetupStdinUnavailable, En) => {
                "cannot open the standard input of tako setup".to_string()
            }
            (Self::AnswersSend(e), Ja) => format!("setup answers の送信に失敗: {e}"),
            (Self::AnswersSend(e), En) => format!("failed to send the setup answers: {e}"),
            (Self::SetupWaitFailed(reason), Ja) => format!("tako setup の完了待ちに失敗: {reason}"),
            (Self::SetupWaitFailed(reason), En) => {
                format!("failed to wait for tako setup to finish: {reason}")
            }
            (Self::SetupFailed { code, output }, Ja) => {
                format!("tako setup が失敗しました (exit={code}): {output}")
            }
            (Self::SetupFailed { code, output }, En) => {
                format!("tako setup failed (exit={code}): {output}")
            }

            (Self::SelectedAgent(got), Ja) => {
                format!("selected_agent は claude / codex / agy のいずれかです: {got}")
            }
            (Self::SelectedAgent(got), En) => {
                format!("selected_agent must be one of claude / codex / agy: {got}")
            }
            (Self::ProviderPlanKey(got), Ja) => {
                format!("provider_plans のキーは claude / gpt / google のいずれかです: {got}")
            }
            (Self::ProviderPlanKey(got), En) => {
                format!("each provider_plans key must be one of claude / gpt / google: {got}")
            }
            (Self::InstructionEmpty, Ja) => "instruction_content は空にできません".to_string(),
            (Self::InstructionEmpty, En) => "instruction_content cannot be empty".to_string(),
            (Self::ProjectKeyEmpty, Ja) => "projects のキーは空にできません".to_string(),
            (Self::ProjectKeyEmpty, En) => "a projects key cannot be empty".to_string(),
            (Self::ProjectCwdEmpty(key), Ja) => format!("projects.{key}.cwd は空にできません"),
            (Self::ProjectCwdEmpty(key), En) => format!("projects.{key}.cwd cannot be empty"),
            (Self::LaunchAgent(got), Ja) => {
                format!("launch_agent は claude / codex / agy / none のいずれかです: {got}")
            }
            (Self::LaunchAgent(got), En) => {
                format!("launch_agent must be one of claude / codex / agy / none: {got}")
            }
            (Self::ModUi(e), _) => format!("mod_ui: {}", e.text_in(lang)),
            (Self::SleepMode(got), Ja) => format!(
                "sleep_guard.mode は off / on / while-agents-running のいずれかです: {got}"
            ),
            (Self::SleepMode(got), En) => format!(
                "sleep_guard.mode must be one of off / on / while-agents-running: {got}"
            ),
            (Self::SleepPower(got), Ja) => {
                format!("sleep_guard.power は ac-only / always のいずれかです: {got}")
            }
            (Self::SleepPower(got), En) => {
                format!("sleep_guard.power must be one of ac-only / always: {got}")
            }
            (Self::Profile(e), _) => e.text_in(lang),

            (Self::ConfigHomeUnresolved, Ja) => {
                "ホームディレクトリが取得できない（$HOME 未設定）".to_string()
            }
            (Self::ConfigHomeUnresolved, En) => {
                "cannot determine the home directory ($HOME is not set)".to_string()
            }
            (Self::ConfigRead(e), Ja) => format!("config.yaml の読み取りに失敗: {e}"),
            (Self::ConfigRead(e), En) => format!("failed to read config.yaml: {e}"),
            (Self::ConfigParse(reason), Ja) => {
                format!("config.yaml のパースに失敗: {}", reason.text_in(lang))
            }
            (Self::ConfigParse(reason), En) => {
                format!("failed to parse config.yaml: {}", reason.text_in(lang))
            }
            (Self::ChangelogParse(reason), Ja) => {
                format!("changes.yaml のパースに失敗: {}", reason.text_in(lang))
            }
            (Self::ChangelogParse(reason), En) => {
                format!("failed to parse changes.yaml: {}", reason.text_in(lang))
            }
            (Self::ChangelogPlatform { revision, value }, Ja) => format!(
                "changes.yaml revision {revision} の platforms に未知の値: {value}（macos / windows）"
            ),
            (Self::ChangelogPlatform { revision, value }, En) => format!(
                "unknown value in the platforms of changes.yaml revision {revision}: {value} \
                 (macos / windows)"
            ),

            (Self::SyncSourceUnset, Ja) => "共通ルールの正本パスが設定されていません。\n\
                 tako setup で設定するか、--source オプションで指定してください"
                .to_string(),
            (Self::SyncSourceUnset, En) => "the path of the shared rules file is not set.\n\
                 Set it with tako setup, or pass it with the --source option"
                .to_string(),
            (Self::SyncSourceMissing(path), Ja) => format!(
                "正本ファイルが見つかりません: {path}\n\
                 ファイルを作成するか、tako setup で正本のパスを設定してください"
            ),
            (Self::SyncSourceMissing(path), En) => format!(
                "the shared rules file does not exist: {path}\n\
                 Create the file, or set its path with tako setup"
            ),
            (Self::SyncSourceRead(e), Ja) => format!("正本ファイルの読み取りに失敗: {e}"),
            (Self::SyncSourceRead(e), En) => {
                format!("failed to read the shared rules file: {e}")
            }
            (Self::SyncSourceEmpty, Ja) => "正本ファイルが空です".to_string(),
            (Self::SyncSourceEmpty, En) => "the shared rules file is empty".to_string(),

            (Self::UnknownDep { got, allowed }, Ja) => {
                format!("不明な依存: {got:?}（この環境の対象は {allowed}）")
            }
            (Self::UnknownDep { got, allowed }, En) => {
                format!("unknown dependency: {got:?} (the ones for this environment: {allowed})")
            }
            (Self::DepInstallerSpawn { program, error }, Ja) => {
                format!("{program} を起動できません: {error}")
            }
            (Self::DepInstallerSpawn { program, error }, En) => {
                format!("cannot start {program}: {error}")
            }
            (Self::DepInstallerFailed { command, code, log }, Ja) => format!(
                "{command} が失敗しました（exit {code}）。上に出ているエラーを確認してください{}",
                log_detail(log)
            ),
            (Self::DepInstallerFailed { command, code, log }, En) => format!(
                "{command} failed (exit {code}). Check the error shown above{}",
                log_detail(log)
            ),
            (Self::DepMissingAfterInstall { command, bin }, Ja) => format!(
                "{command} は正常終了しましたが {bin} が見つかりません。\n\
                 ターミナルを開き直してから `tako setup deps` で確認してください"
            ),
            (Self::DepMissingAfterInstall { command, bin }, En) => format!(
                "{command} finished without errors, but {bin} cannot be located.\n\
                 Reopen the terminal, then check with `tako setup deps`"
            ),

            (Self::UnknownBootstrapAgent { got, allowed }, Ja) => {
                format!("不明なエージェント: {got:?}（{allowed} のいずれか）")
            }
            (Self::UnknownBootstrapAgent { got, allowed }, En) => {
                format!("unknown agent: {got:?} (one of {allowed})")
            }
            (Self::BootstrapHomeUnresolved, Ja) => {
                "ホームディレクトリを特定できません（HOME が未設定）".to_string()
            }
            (Self::BootstrapHomeUnresolved, En) => {
                "cannot determine the home directory (HOME is not set)".to_string()
            }

            (Self::InstallDuringSelfTest, Ja) => {
                "セルフテスト中は実インストールを行いません（dry_run でのみ呼べます）".to_string()
            }
            (Self::InstallDuringSelfTest, En) => {
                "no real installation runs during the self-test (call it only with dry_run)"
                    .to_string()
            }
            (Self::InstallNotDelegable { os, command }, Ja) => format!(
                "この環境では tako が自動インストールを代行できません（{os}）。\n\
                 次のコマンドを自分で実行してから `tako setup` をやり直してください:\n  {command}"
            ),
            (Self::InstallNotDelegable { os, command }, En) => format!(
                "tako cannot run the installation for you in this environment ({os}).\n\
                 Run the following command yourself, then run `tako setup` again:\n  {command}"
            ),
            (Self::NoDownloader, Ja) => {
                "curl も wget も見つかりません。どちらかを導入してから再実行してください\n\
                 （macOS なら通常 curl が標準で入っています）"
                    .to_string()
            }
            (Self::NoDownloader, En) => {
                "neither curl nor wget can be located. Install one of them, then try again\n\
                 (on macOS, curl is normally installed by default)"
                    .to_string()
            }
            (Self::InstallerToolSpawn { program, error }, Ja) => {
                format!("{program} を起動できません: {error}")
            }
            (Self::InstallerToolSpawn { program, error }, En) => {
                format!("cannot start {program}: {error}")
            }
            (Self::InstallerFetchFailed { url, detail }, Ja) => format!(
                "インストーラを取得できませんでした: {url}\n\
                 {}\n\
                 ネットワーク接続とプロキシ設定を確認してから再実行してください。\n\
                 社内プロキシ環境では {url} へのアクセス許可が要ることがあります",
                detail.text_in(lang)
            ),
            (Self::InstallerFetchFailed { url, detail }, En) => format!(
                "could not download the installer: {url}\n\
                 {}\n\
                 Check the network connection and the proxy settings, then try again.\n\
                 Behind a corporate proxy, access to {url} may need to be allowed",
                detail.text_in(lang)
            ),
            (Self::InstallerUnreadable(e), Ja) => format!("取得したインストーラを読めません: {e}"),
            (Self::InstallerUnreadable(e), En) => {
                format!("cannot read the downloaded installer: {e}")
            }
            (Self::NotAnInstaller { url }, Ja) => format!(
                "取得した内容がインストーラではありません（{url} が HTML かエラーページを返しています）。\n\
                 社内プロキシやネットワーク制限が疑われます。ブラウザで {url} を開いて中身を確認するか、\n\
                 ネットワークを変えて再実行してください"
            ),
            (Self::NotAnInstaller { url }, En) => format!(
                "the downloaded content is not an installer ({url} returned HTML or an error page).\n\
                 A corporate proxy or a network restriction is likely. Open {url} in a browser to \
                 check what it returns,\n\
                 or try again on a different network"
            ),
            (Self::InstallerOutOfMemory { log }, Ja) => format!(
                "インストールがメモリ不足で中断されました（exit 137）。\
                 空きメモリを増やしてから再実行してください{}",
                log_detail(log)
            ),
            (Self::InstallerOutOfMemory { log }, En) => format!(
                "the installation stopped because memory ran out (exit 137). \
                 Free up memory, then try again{}",
                log_detail(log)
            ),
            (Self::InstallerKilled { code, log }, Ja) => format!(
                "インストールが途中で強制終了しました（exit {code}）。\
                 もう一度実行するか、上に出ているメッセージを確認してください{}",
                log_detail(log)
            ),
            (Self::InstallerKilled { code, log }, En) => format!(
                "the installation was killed partway through (exit {code}). \
                 Run it again, or check the messages shown above{}",
                log_detail(log)
            ),
            (
                Self::InstallerFailed {
                    product,
                    code,
                    troubleshoot_url,
                    log,
                },
                Ja,
            ) => format!(
                "{product} のインストーラが失敗しました（exit {code}）。上に出ているエラーを確認してください。\
                 解決しない場合は {troubleshoot_url} を参照してください{}",
                log_detail(log)
            ),
            (
                Self::InstallerFailed {
                    product,
                    code,
                    troubleshoot_url,
                    log,
                },
                En,
            ) => format!(
                "the {product} installer failed (exit {code}). Check the error shown above. \
                 If that does not solve it, see {troubleshoot_url}{}",
                log_detail(log)
            ),
            (Self::InstalledButMissing { launcher, command }, Ja) => format!(
                "インストーラは正常終了しましたが {launcher} が見つかりません。\n\
                 `{command}` を手で実行して、出力に出るエラーを確認してください"
            ),
            (Self::InstalledButMissing { launcher, command }, En) => format!(
                "the installer finished without errors, but {launcher} cannot be located.\n\
                 Run `{command}` by hand and check the errors in its output"
            ),

            (Self::ShellProfileUnknown { shell, dirs }, Ja) => format!(
                "使っているシェル（{}）の設定ファイルが分かりません。\n\
                 次の {} 行をご自身のシェルの設定ファイルへ追加してください:\n{}",
                if shell.is_empty() { "不明" } else { shell },
                dirs.len(),
                export_lines(dirs),
            ),
            (Self::ShellProfileUnknown { shell, dirs }, En) => format!(
                "cannot tell which settings file your shell ({}) uses.\n\
                 Add the following {} to your shell's settings file:\n{}",
                if shell.is_empty() { "unknown" } else { shell },
                if dirs.len() == 1 {
                    "line".to_string()
                } else {
                    format!("{} lines", dirs.len())
                },
                export_lines(dirs),
            ),
            (Self::ShellProfileUnknownForUndo, Ja) => {
                "使っているシェルの設定ファイルが分かりません".to_string()
            }
            (Self::ShellProfileUnknownForUndo, En) => {
                "cannot tell which settings file your shell uses".to_string()
            }
            (Self::UserPathNotReflected { dir }, Ja) => format!(
                "ユーザー PATH へ {dir} を追加できませんでした（書き込み後も反映されていません）。\n\
                 設定 → システム → バージョン情報 → 環境変数 から手で追加してください"
            ),
            (Self::UserPathNotReflected { dir }, En) => format!(
                "could not add {dir} to the user PATH (it is still missing after writing).\n\
                 Add it by hand from Settings → System → About → Environment Variables"
            ),
            (Self::PathBlock(e), _) => e.text_in(lang),
            (Self::CliLink(e), _) => e.text_in(lang),
            (Self::UserPath(e), _) => e.text_in(lang),
        }
    }

    /// 従来の `String` の口が返す文（日本語）
    pub fn into_ja(self) -> String {
        self.text_in(Lang::Ja)
    }

    /// 「無効なパラメータ: 」の断りへ載せる（枠と同じ表示言語で組む）
    pub fn invalid(self) -> DispatchError {
        DispatchError::InvalidParams(self.to_string())
    }

    /// 前置きの無い理由の文として載せる（表示言語で組む）
    pub fn operation(self) -> DispatchError {
        DispatchError::Operation(self.to_string())
    }
}

/// シェルの設定ファイルへ足してもらう行（`export PATH="<dir>:$PATH"` を 1 行ずつ。#2365 より前の組み立てのまま）
fn export_lines(dirs: &[String]) -> String {
    dirs.iter()
        .map(|d| format!("  export PATH=\"{d}:$PATH\""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 導入器・インストーラの出力の添え方（空なら添えない・あれば改行を挟む。#2365 より前の組み立てのまま）
fn log_detail(log: &str) -> String {
    if log.trim().is_empty() {
        String::new()
    } else {
        format!("\n{}", log.trim())
    }
}

impl std::fmt::Display for SetupRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

impl std::error::Error for SetupRefusal {}

impl From<ConfigError> for SetupRefusal {
    fn from(e: ConfigError) -> Self {
        Self::Profile(e)
    }
}

impl From<PathBlockError> for SetupRefusal {
    fn from(e: PathBlockError) -> Self {
        Self::PathBlock(e)
    }
}

impl From<LinkError> for SetupRefusal {
    fn from(e: LinkError) -> Self {
        Self::CliLink(e)
    }
}

impl From<UserPathError> for SetupRefusal {
    fn from(e: UserPathError) -> Self {
        Self::UserPath(e)
    }
}
