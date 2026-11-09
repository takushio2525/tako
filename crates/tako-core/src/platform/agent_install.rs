//! エージェント CLI の公式インストール手順（抽象境界 B17。#868 / #525 / #989）
//!
//! 「各エージェント CLI をどう入れるか」のプラットフォーム差をここへ閉じ込める。
//!
//! ## なぜ純粋関数にするか
//!
//! 手順は [`Platform`] を**引数で受ける純粋関数**として書く。こうすると
//! **macOS 上から Windows 向けの手順を検証できる**（#515 と同じ作法）。
//! 実行してよいかどうかは [`crate::platform::support`] のマトリクスが決めるので、
//! 「手順を知っていること」と「その環境で自動実行してよいこと」を混ぜない。
//!
//! ## 調査の根拠（2026-08-21 実測）
//!
//! 経路は推測せず、公式ドキュメントとインストーラの実物を確認して決めた。
//!
//! - 公式 docs（`https://code.claude.com/docs/en/setup.md`）のインストールタブは
//!   **「Native Install (Recommended)」** が第一候補。Homebrew タブには
//!   「Homebrew installations do not auto-update」と明記があり、native は
//!   「automatically update in the background」。ゼロスタートの利用者を
//!   置いていかないので native を採る（brew は選ばない）
//! - `https://claude.ai/install.sh` は 302 で
//!   `https://downloads.claude.ai/claude-code-releases/bootstrap.sh` へ飛ぶ 217 行の bash。
//!   中身は「sudo 拒否 → OS/arch 判定（Rosetta 込み）→ `latest` 取得 →
//!   `manifest.json` の SHA256 で検証 → `$HOME/.claude/downloads/` へ落として
//!   `chmod +x` → `<binary> install` → 一時ファイル削除」。**署名検証は
//!   インストーラ自身が行う**ので tako は二重にやらない
//! - 設置先は `~/.local/bin/claude`（symlink）→ `~/.local/share/claude/versions/<version>`
//! - このスクリプトは Windows を**明示的に非対応**にしている
//!   （`MINGW*|MSYS*|CYGWIN*` で exit 1）。Windows の公式手順は PowerShell の
//!   `irm https://claude.ai/install.ps1 | iex`（docs の Native Install タブ）
//!
//! ## Windows 側の調査の根拠（2026-09-01 実測。#1057）
//!
//! `https://claude.ai/install.ps1` は 3,189 バイトの PowerShell スクリプトで、
//! `install.sh` と同じ形をしている（32bit 拒否 → arch 判定（ARM64 込み）→ `latest` 取得 →
//! `manifest.json` の SHA256 で検証 → `$env:USERPROFILE\.claude\downloads\` へ落として
//! `claude.exe install <target>` → 一時ファイル削除）。**署名検証はインストーラ自身が行う**。
//!
//! - 先頭は `param(...)` で **shebang を持たない**ので、取得物の見分け方は
//!   プラットフォームごとに変える（[`ScriptSignature`]）
//! - `param()` を持つので `-File` で走らせても既定 `$Target = "latest"` が効く
//! - 実機の ExecutionPolicy は `CurrentUser = RemoteSigned`（実測）。落としたファイルを
//!   `-File` で走らせると弾かれる。公式の `irm | iex` は文字列を食わせるので ExecutionPolicy の対象外。
//!   #1057 は `-ExecutionPolicy Bypass`（Process スコープ）で避けたが、グループポリシーで
//!   AllSigned / Restricted を強制した PC では負けるので、#2790 から**本文をテキストで読んで
//!   `-EncodedCommand` の起動片から走らせる**（[`ScriptPassing::PowerShellText`]。`irm | iex` と同じく
//!   実行ポリシーの対象外）。失敗したときに案内する手で打つ 1 行は公式の `irm … | iex` のまま
//!   （同じ理由で実行ポリシーの対象外で、最も簡単な形 = #322）
//! - 設置先は `~\.local\bin\claude.exe`。ここは
//!   [`crate::platform::exe`] の走査対象なので、PATH が再ログインまで
//!   伝播しない Windows でも導入直後に検出できる
//!
//! ## codex の調査の根拠（2026-09-09 実測。#989）
//!
//! `https://chatgpt.com/codex/install.sh` は 302 で
//! `https://releases.openai.com/codex/install.sh` へ飛ぶ 1,209 行の `#!/bin/sh`。
//!
//! - **`sudo` の参照が 0 件**。`BIN_DIR="${CODEX_INSTALL_DIR:-$HOME/.local/bin}"` /
//!   `CODEX_HOME_DIR="${CODEX_HOME:-$HOME/.codex}"` /
//!   `STANDALONE_ROOT="$CODEX_HOME_DIR/packages/standalone"` で、ホームの中だけで完結する
//! - `codex-package_SHA256SUMS` を落として突き合わせる = **署名検証はインストーラ自身が行う**
//! - PATH も**インストーラ自身**がマーカーブロックで profile へ書く（`pick_profile` が
//!   darwin + zsh なら `~/.zprofile`）。tako の置き場所も同じ `~/.local/bin` なので
//!   [`InstallRecipe::launcher_dir_rel`] が返す 1 個のブロックで 3 系統ぶんを兼ねる
//! - **最後に `maybe_launch_codex_now` が `Start Codex now? [y/N]` を `/dev/tty` から聞く**。
//!   代行するときは [`InstallRecipe::installer_env`] の `CODEX_NON_INTERACTIVE=1` が必須
//!   （無いと codex の TUI が立ってインストーラが返らない）
//! - `uname -s` が Darwin / Linux 以外なら
//!   `install.sh supports macOS and Linux. Use install.ps1 on Windows.` で exit 1
//! - **背景の自動更新は無い**。`codex doctor` が
//!   `↑ updates 0.153.4 available (current 0.153.0, dismissed 0.144.5)` と知らせるだけで、
//!   更新は `codex update`（実測）
//! - Windows の設置先は `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`
//!   （`install.ps1` の `$defaultVisibleBinDir`。claude と違って `~\.local\bin` ではない）
//!
//! ## agy の調査の根拠（2026-09-09 実測。#989）
//!
//! 公式 docs（`https://antigravity.google/docs/cli/install.md`）の
//! 「macOS and Linux」が `curl -fsSL https://antigravity.google/cli/install.sh | bash`。
//! 取得物は 239 行の `#!/bin/bash`。
//!
//! - **`sudo` の参照が 0 件**。`TARGET_DIR="$HOME/.local/bin"` /
//!   `BINARY_PATH="$TARGET_DIR/agy"` で、マニフェストの `sha512` と突き合わせてから置く
//!   （**署名検証はインストーラ自身が行う**）
//! - **単一バイナリ**なのでバージョンごとの実体ディレクトリが無い
//!   （= [`InstallRecipe::payload_rel`] は `None`）。展開前の置き場は
//!   `~/.cache/antigravity/staging` で、`trap cleanup EXIT` が消す
//! - 既に在れば `Notice: 'agy' is already installed at …` で **exit 0**（= 冪等）
//! - 同じ出力に `The Antigravity CLI automatically self-updates in the background during
//!   regular runs.` と明記があるので `auto_updates: true`
//! - 対話プロンプトを持たない（`read` / `/dev/tty` の参照なし）ので env の追加は要らない
//! - 最後に `"$BINARY_PATH" install` を呼んで PATH を設定する
//!   （`agy install` = 「Configure environment paths and shell settings」。実測の `agy help install`）
//! - Windows の設置先は `%LOCALAPPDATA%\agy\bin`（公式 docs の Windows 節）
//!
//! ## Windows の codex / agy の調査の根拠（2026-11-08 実測。#2791）
//!
//! #989 では `install.ps1` が実在するのを確かめただけで、実機で通したのは claude だけ（#1057）だったので
//! codex / agy の Windows は案内止まり（`tako_can_run: false`）にしていた。#2791 で中身を読み、
//! どちらも**ユーザー権限だけで入る**ことを確かめて代行へ倒した（UAC の要る手段は無い）。
//!
//! - codex: 公式 README（`openai/codex`）の Windows の手順は
//!   `powershell -ExecutionPolicy ByPass -c "irm https://chatgpt.com/codex/install.ps1 | iex"`
//!   （案内の 1 行は PowerShell のプロンプトで打つ形の `irm … | iex` のまま = #2790 / #322）。代わりの手段は
//!   npm（`npm install -g @openai/codex`。Node が要る）だけで winget は無い。`install.ps1` は 302 で
//!   `https://releases.openai.com/codex/install.ps1` へ飛ぶ 71,052 バイト（Authenticode の署名つき）。
//!   管理者権限の参照は 0 件で、実体は `$env:CODEX_HOME`（空なら `%USERPROFILE%\.codex`）の
//!   `packages\standalone` へ置き、`%LOCALAPPDATA%\Programs\OpenAI\Codex\bin` をそこへの junction にする。
//!   ユーザー環境変数 Path も**インストーラ自身**が書く。`SHA256SUMS` で検証する（署名検証はインストーラ自身）。
//!   問いは `Start Codex now?` と npm 版の削除の 2 つで、どちらも `CODEX_NON_INTERACTIVE=1`
//!   （[`InstallRecipe::installer_env`]）か入出力のリダイレクトで聞かずに「いいえ」になる
//! - agy: 公式 Quickstart（`https://antigravity.google/docs/getting-started`）の
//!   「Windows (PowerShell)」が `irm https://antigravity.google/cli/install.ps1 | iex`（7,165 バイト）。
//!   `%LOCALAPPDATA%\agy\bin\agy.exe` へマニフェストの sha512 と突き合わせてから置き、
//!   最後に `agy.exe install` が PATH を整える。既に在れば `Notice: 'agy.exe' is already installed` で 0 を返す
//!   （= 冪等）。**取得物の片付けが `$script:` のスコープに依る**（関数の中で `$script:stagingPayload` へ書き、
//!   `finally` で素の `$stagingPayload` を読む）ので、起動片は `-File` / `irm | iex` と同じスコープで走らせる
//!   （`platform::shell::powershell_script_text_command` のドットソース。子スコープだと片付けが飛び、
//!   `%LOCALAPPDATA%\antigravity\staging\agy.exe` が残る）。agy は `Set-StrictMode -Version Latest` の下で最上位の
//!   `$MyInvocation.MyCommand` を読むので、起動片は `$MyInvocation` を `$null` にしない（#2822 は本文を関数
//!   `install.ps1` として定義してその名前をドットソースする = 失敗の文にインストーラの全文が出ない）
//! - 起こし方は claude と同じ 1 実装（[`WINDOWS_POWERSHELL_RUNNER`] = 本文をテキストで読む `-EncodedCommand`。
//!   #2790）。実行ポリシーを強制した PC でも止まらない
//! - 実走は Windows の CI のランナーで行う（`scripts/windows/test-agent-install-2791.ps1`）。
//!   Windows の実機（クライアント版）では未実測なので、宣言（[`crate::platform::support`] の
//!   `tako_setup_bootstrap` 行）はその旨を書いたうえで縮退のままにしてある
//!
//! A/B は `TAKO_2791_LEGACY=1`（Windows の codex / agy は状態照会と案内までに戻る。[`current_recipe`]）
//!
//! ## Windows の claude は Git Bash を要らない（2026-11-09 実測。#2433）
//!
//! #2433 の調査時点で、Claude Code の Windows ネイティブ版が Git Bash（Git for Windows）を要るかを
//! setup が確かめていなかった（要るのに無い PC では「導入済み」と言った後で claude が止まるおそれ）。
//! Windows の CI のランナーで本物の claude（2.1.319）を、Git を隠した砂場で起こして測った
//! （`scripts/windows/test-claude-git-bash-2433.ps1`。認証は使わず、偽の Messages API へ向けて 1 ターン走らせる）:
//!
//! - **Git を隠しても入り、起動し、1 ターン走る**。シェルのツールは PowerShell だけが出て、
//!   コマンドは PowerShell で走る（`--debug` の行は `Git Bash not found; BashTool will be unavailable`）。
//!   認証なしの起動は Git の有無に依らず `Not logged in · Please run /login`（終了コード 1）
//! - Git が既定の置き場（`C:\Program Files\Git`）に在れば **PATH に無くても** Bash を使う。標準でない
//!   置き場の Git は PATH の `git` か `CLAUDE_CODE_GIT_BASH_PATH` で見つける
//! - `CLAUDE_CODE_GIT_BASH_PATH` が存在しないファイルを指しても止まらず、自動検出へ戻る
//!   （`[WARN] … not found; falling back to auto-detection`）
//!
//! だから setup は Git Bash の有無を段の判定に入れない（無くても `ready` へ進める）。違いは
//! claude がコマンドを走らせるシェルだけなので、それを導入計画の 1 行（[`InstallRecipe::shell_note`]）で知らせる。
//! 上流が Git Bash を必須へ戻したら、CI の同じ検査（導入代行の実走の段と同じ条件で走る）が組み合わせを名指しで落ちる

use super::support::Platform;
use std::path::PathBuf;

/// 公式インストーラの手順を持っているエージェント CLI（#868 → #989 で 3 系統へ）。
///
/// **ローカル LLM（`agent_support::Agent::Local`）はここに無い**。Ollama は
/// 「エージェント CLI」ではなく runtime で、導入したあとにモデルを pull する段が
/// 増えるので手順の形が違う（#990 の担当）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
    Agy,
}

impl AgentKind {
    /// 列挙の正本。並びは「基準系 → master を務められる系統 → worker 専用」
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Agy];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == value)
    }

    /// 利用者へ見せる製品名（コマンド名ではなく「何を入れるのか」）
    pub fn product(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
            Self::Agy => "Antigravity CLI",
        }
    }
}

/// インストーラの取得元 URL とその素性
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallerSource {
    /// 取得 URL（公式）
    pub url: &'static str,
    /// 取得したものを何で実行するか（`bash` / `powershell` 等）
    pub interpreter: &'static str,
    /// 人間へ提示する 1 行コマンド（公式 docs に載っているそのままの形）
    pub official_command: &'static str,
}

/// 取得したものが本物のインストーラかの見分け方。
///
/// **プロキシが返す HTML エラーページを弾くため**に見る（公式のトラブルシュートに
/// `syntax error near unexpected token '<'` として載っている実在の失敗モード）。
/// 判定そのものは [`looks_like_installer`] の純粋関数なので、
/// **macOS から Windows 側の判定も検証できる**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptSignature {
    /// 先頭が shebang（`#!`）。`install.sh` はこの形
    Shebang,
    /// PowerShell スクリプト。`.ps1` は shebang を持たないので既知のマーカーで見る
    PowerShell,
}

/// PowerShell スクリプトだと分かる語（2026-09-01 に `install.ps1` の実物で確認）。
///
/// 実物の先頭は `param(` → `Set-StrictMode -Version Latest` →
/// `$ErrorActionPreference = "Stop"` の順で、どれか 1 つでも当たれば十分。
/// **どれか 1 つに絞らない**のは、上流が冒頭の書き方を変えても
/// 「HTML ではない」判定が生き残るようにするため
const POWERSHELL_MARKERS: &[&str] = &[
    "param(",
    "Set-StrictMode",
    "$ErrorActionPreference",
    "Invoke-RestMethod",
    "Invoke-WebRequest",
    "Write-Output",
    "#Requires",
];

/// 取得した先頭バイト列が本物のインストーラに見えるか（**純粋関数**）
pub fn looks_like_installer(signature: ScriptSignature, head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let trimmed = text.trim_start();
    match signature {
        ScriptSignature::Shebang => trimmed.starts_with("#!"),
        // HTML / XML / JSON のエラーページを弾いたうえで PowerShell の語を要求する
        ScriptSignature::PowerShell => {
            !trimmed.starts_with('<')
                && !trimmed.starts_with('{')
                && POWERSHELL_MARKERS.iter().any(|m| trimmed.contains(m))
        }
    }
}

/// 取得したスクリプトをインタプリタへどう渡すか（#2790）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptPassing {
    /// パスを argv の最後に置く（`bash <path>` / `sh <path>`）
    Path,
    /// 本文をテキストで読み、`-EncodedCommand` の起動片から scriptblock として走らせる（PowerShell）。
    /// **実行ポリシーの対象外**なので、グループポリシーで AllSigned / Restricted を強制した PC でも
    /// 止まらない（組み立ての正本は `platform::shell::powershell_script_text_args`）
    PowerShellText,
}

/// #2790 より前の Windows の起こし方（`TAKO_2790_LEGACY=1` の A/B と、CI で「これは弾かれる」を
/// 示す比較のためだけに残す）。`-ExecutionPolicy Bypass` は Process スコープなので、グループポリシーの
/// MachinePolicy / UserPolicy に負ける
pub const LEGACY_POWERSHELL_FILE_ARGS: &[&str] = &[
    "-NoLogo",
    "-NoProfile",
    // tako:ps-file-ok A/B の旧アームと CI の「これは弾かれる」の比較だけ（#2790）
    "-ExecutionPolicy",
    "Bypass",
    // tako:ps-file-ok 同上
    "-File",
];

/// `TAKO_2790_LEGACY=1` で #2790 より前（`-ExecutionPolicy Bypass -File`）の起こし方へ戻す
pub fn issue2790_legacy() -> bool {
    std::env::var_os("TAKO_2790_LEGACY").is_some()
}

/// 取得したインストーラを走らせるインタプリタ（**データだけ**。解決は呼び出し側）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterSpec {
    /// 実行ファイルの候補。前から順に探し、最初に見つかったものを使う
    pub candidates: &'static [&'static str],
    /// どれも見つからなかったときに名前で起こす最後の手段
    /// （Windows の `CreateProcess` と unix の絶対パスはどちらもこれで起動できる）
    pub fallback: &'static str,
    /// スクリプトの渡し方（#2790。それまでは「パスの直前に置く引数」の並びだった）
    pub passing: ScriptPassing,
    /// 取得先ファイルの拡張子（`.ps1` は #2790 より前の `-File` が要求した名残だが、
    /// 取得物が何かを一時ファイルの名前で見分けられるので残す）
    pub script_ext: &'static str,
    /// 取得したものの見分け方
    pub signature: ScriptSignature,
}

impl InterpreterSpec {
    /// スクリプトのパスを与えて argv を組む（**起こし方の正本**。A/B は `TAKO_2790_LEGACY=1`）
    pub fn args_for(&self, script: &std::path::Path) -> Vec<std::ffi::OsString> {
        self.args_for_in(script, issue2790_legacy())
    }

    /// [`Self::args_for`] の純粋関数版（`legacy` = #2790 より前の `-ExecutionPolicy Bypass -File`）
    pub fn args_for_in(&self, script: &std::path::Path, legacy: bool) -> Vec<std::ffi::OsString> {
        match self.passing {
            ScriptPassing::Path => vec![script.as_os_str().to_os_string()],
            ScriptPassing::PowerShellText if legacy => LEGACY_POWERSHELL_FILE_ARGS
                .iter()
                .map(std::ffi::OsString::from)
                .chain([script.as_os_str().to_os_string()])
                .collect(),
            ScriptPassing::PowerShellText => super::shell::powershell_script_text_args(script)
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect(),
        }
    }
}

/// 実体の置き場（[`InstallRecipe::payload_rel`]）の根（#2810）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadRoot {
    /// ホーム（`$HOME` / `%USERPROFILE%`）
    Home,
    /// codex の置き場（`CODEX_HOME`。空・未設定なら `~/.codex`）。インストーラ自身が
    /// `${CODEX_HOME:-$HOME/.codex}/packages/standalone`（Windows も `$env:CODEX_HOME` → `%USERPROFILE%\.codex`）へ置く
    CodexHome,
}

/// 1 エージェントぶんのインストール手順
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallRecipe {
    pub agent: AgentKind,
    pub platform: Platform,
    pub source: InstallerSource,
    /// インストール後にランチャーが置かれるパス（`$HOME` からの相対）
    pub launcher_rel: &'static str,
    /// 実体（バージョンごとのディレクトリ）が置かれるパス（[`Self::payload_root`] からの相対）。
    ///
    /// **単一バイナリの系統は `None`**（agy はランチャー自身が本体なので、
    /// 「本体の置き場所」を別に見せると同じパスを 2 回言うことになる）
    pub payload_rel: Option<&'static str>,
    /// [`Self::payload_rel`] の根（#2810）。codex のインストーラは実体を `CODEX_HOME` の下へ置くので、
    /// ホームからの相対で書くと空でない `CODEX_HOME` のときに計画の置き場がずれる
    pub payload_root: PayloadRoot,
    /// バックグラウンド自動更新が効くか（機械が読む側）
    pub auto_updates: bool,
    /// 以後どう更新されるかの 1 行（人が読む側）。**`auto_updates` の真偽から
    /// 文面を組まない**のは、codex が「自動ではないが自分で知らせる」の第 3 の形だから
    /// （`codex doctor` の `↑ updates … available`。実測）
    pub update_note: &'static str,
    /// この系統がコマンドを走らせるシェルについて、導入の前に知らせる 1 行（人が読む側）。
    ///
    /// **段の判定には使わない**（要る物ではなく、有無で振る舞いが変わる物の説明）。いまは Windows の claude
    /// だけが持つ: Git for Windows が無くても動き、そのときコマンドは PowerShell で走る（#2433 の実測）。
    /// 確かめていない系統には書かない（codex / agy の Windows のシェルは推測で埋めない）
    pub shell_note: Option<&'static str>,
    /// tako が実行を代行してよいか。false = 手順を案内するだけ
    pub tako_can_run: bool,
    /// インストールが失敗したときに案内する公式ページ（2026-09-09 に 200 を実測）。
    /// **系統ごとに違う**ので、claude のトラブルシュートを codex の失敗に出さない
    pub troubleshoot_url: &'static str,
    /// インストーラへ渡す環境変数。**対話プロンプトを黙らせるため**に使う
    /// （codex は `CODEX_NON_INTERACTIVE=1` が無いと最後に `Start Codex now?` を
    /// `/dev/tty` から聞いて返ってこない。実測）
    pub installer_env: &'static [(&'static str, &'static str)],
    /// 取得したインストーラの走らせ方（`source.interpreter` は人へ見せる名前、
    /// こちらは実際に組む argv）
    pub runner: InterpreterSpec,
}

impl InstallRecipe {
    /// PATH へ通すべきディレクトリ（`$HOME` からの相対）。
    ///
    /// **ランチャーのパスから導出する**（プラットフォームで `match` しない）。
    /// #989 で codex / agy を足した時点で「どの OS でも `.local/bin`」が
    /// 成り立たなくなった（Windows の codex は
    /// `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`、agy は `%LOCALAPPDATA%\agy\bin`）。
    /// 導出にすれば置き場所の正本が `launcher_rel` の 1 箇所になり、両者がずれない
    pub fn launcher_dir_rel(&self) -> &'static str {
        self.launcher_rel
            .rsplit_once('/')
            .map(|(dir, _)| dir)
            .unwrap_or(".")
    }

    /// 実際のランチャーパス（home を与えて解決する。**環境変数を読まない**ので
    /// 隔離 HOME のテストがそのまま書ける）
    pub fn launcher_path_in(&self, home: &std::path::Path) -> PathBuf {
        rel_join(home, self.launcher_rel)
    }

    /// PATH へ通すべきディレクトリの絶対パス
    pub fn launcher_dir_in(&self, home: &std::path::Path) -> PathBuf {
        rel_join(home, self.launcher_dir_rel())
    }

    /// 実体ディレクトリの絶対パス（単一バイナリの系統は `None`）。
    ///
    /// `codex_home` は**解決済みの** codex の置き場（`CODEX_HOME`。空・未設定なら `<home>/.codex`）。
    /// 解決の正本は tako-control の `codex_session::codex_home` で、tako-core はそれに依存できないので
    /// 値で受ける（ここで `.codex` を組むと正本とずれる。#2798 / #2810）
    pub fn payload_dir_in(
        &self,
        home: &std::path::Path,
        codex_home: &std::path::Path,
    ) -> Option<PathBuf> {
        let root = match self.payload_root {
            PayloadRoot::Home => home,
            PayloadRoot::CodexHome => codex_home,
        };
        self.payload_rel.map(|rel| rel_join(root, rel))
    }

    /// 取得したインストーラを走らせる `Command`（**起こし方の 1 実装**。#2790）。
    ///
    /// インタプリタの解決（候補を前から探し、無ければ [`InterpreterSpec::fallback`]）・引数
    /// （[`InterpreterSpec::args_for`]）・インストーラへ渡す env（[`Self::installer_env`]）・
    /// コンソールを出さない設定（#586）までを組む。stdio と待ち方は呼び手が決める
    /// （`setup_bootstrap::run_installer`）。Windows の CI のポリシー検査
    /// （`tests/issue2790_install_execution_policy_windows.rs`）も同じここを通して偽のインストーラを起こす。
    /// 起こしたインタプリタの名前は `Command::get_program` で読める
    pub fn installer_command(&self, script: &std::path::Path) -> std::process::Command {
        let program = self
            .runner
            .candidates
            .iter()
            .find_map(|name| super::exe::find(name))
            .unwrap_or_else(|| self.runner.fallback.to_string());
        let mut command = std::process::Command::new(program);
        super::process::no_console_window(&mut command);
        command.args(self.runner.args_for(script));
        // インストーラの対話プロンプトを黙らせる env（codex の `Start Codex now?` 対策）。
        // **手順の側が持つデータ**なので、ここに系統名を書かない
        for (key, value) in self.installer_env {
            command.env(key, value);
        }
        command
    }
}

/// `/` 区切りの相対パスを結合する（Windows でも `\` に解決される）
fn rel_join(home: &std::path::Path, rel: &str) -> PathBuf {
    let mut path = home.to_path_buf();
    for part in rel.split('/') {
        path.push(part);
    }
    path
}

/// 指定プラットフォーム向けのインストール手順。
///
/// **`Platform` を引数で受ける純粋関数**なので、macOS の `cargo test` から
/// Windows 向けの内容も検証できる
pub fn recipe(platform: Platform, agent: AgentKind) -> InstallRecipe {
    match (platform, agent) {
        (Platform::MacOs, AgentKind::Claude) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://claude.ai/install.sh",
                interpreter: "bash",
                official_command: "curl -fsSL https://claude.ai/install.sh | bash",
            },
            launcher_rel: ".local/bin/claude",
            payload_rel: Some(".local/share/claude/versions"),
            payload_root: PayloadRoot::Home,
            auto_updates: true,
            update_note: "Claude Code が自分でバックグラウンド更新します",
            shell_note: None,
            troubleshoot_url: "https://code.claude.com/docs/en/troubleshoot-install",
            tako_can_run: true,
            installer_env: &[],
            runner: InterpreterSpec {
                candidates: &["bash"],
                fallback: "/bin/bash",
                passing: ScriptPassing::Path,
                script_ext: "sh",
                signature: ScriptSignature::Shebang,
            },
        },
        (Platform::Windows, AgentKind::Claude) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://claude.ai/install.ps1",
                interpreter: "powershell",
                official_command: "irm https://claude.ai/install.ps1 | iex",
            },
            launcher_rel: ".local/bin/claude.exe",
            payload_rel: Some(".local/share/claude/versions"),
            payload_root: PayloadRoot::Home,
            auto_updates: true,
            update_note: "Claude Code が自分でバックグラウンド更新します",
            // #2433: Git を隠したランナーで claude 2.1.319 が起動して 1 ターン走った（PowerShell ツール）
            shell_note: Some(
                "Git for Windows は要りません。無ければ claude は PowerShell でコマンドを実行し、\
                 入っていれば Git Bash を使います",
            ),
            troubleshoot_url: "https://code.claude.com/docs/en/troubleshoot-install",
            // #1057 で Windows 11 実機の通し実測を経て true へ倒した
            // （記録は `.agent/plans/2026-08-windows-main-merge-wip.md` の #1057 節）
            tako_can_run: true,
            installer_env: &[],
            runner: WINDOWS_POWERSHELL_RUNNER,
        },
        // codex（2026-09-09 実測。モジュールドキュメントの「codex の調査の根拠」）
        (Platform::MacOs, AgentKind::Codex) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://chatgpt.com/codex/install.sh",
                // 取得物の shebang は `#!/bin/sh`（公式の 1 行も `| sh`）
                interpreter: "sh",
                official_command: "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
            },
            launcher_rel: ".local/bin/codex",
            // インストーラが `CODEX_HOME` の下へ置く（ホームの `.codex` の決め打ちにしない。#2810）
            payload_rel: Some("packages/standalone"),
            payload_root: PayloadRoot::CodexHome,
            // 背景更新は無い。`codex doctor` が知らせるだけ（実測）
            auto_updates: false,
            update_note: "codex が新しい版を知らせます（`codex update` で更新します）",
            shell_note: None,
            troubleshoot_url: "https://learn.chatgpt.com/docs/codex/cli",
            tako_can_run: true,
            // これが無いと最後の `Start Codex now? [y/N]` で返ってこない（実測）
            installer_env: &[("CODEX_NON_INTERACTIVE", "1")],
            runner: InterpreterSpec {
                candidates: &["sh"],
                fallback: "/bin/sh",
                passing: ScriptPassing::Path,
                script_ext: "sh",
                signature: ScriptSignature::Shebang,
            },
        },
        (Platform::Windows, AgentKind::Codex) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://chatgpt.com/codex/install.ps1",
                interpreter: "powershell",
                official_command: "irm https://chatgpt.com/codex/install.ps1 | iex",
            },
            // `install.ps1` の `$defaultVisibleBinDir`（claude と違い `~\.local\bin` ではない）
            launcher_rel: "AppData/Local/Programs/OpenAI/Codex/bin/codex.exe",
            // インストーラが `CODEX_HOME` の下へ置く（ホームの `.codex` の決め打ちにしない。#2810）
            payload_rel: Some("packages/standalone"),
            payload_root: PayloadRoot::CodexHome,
            auto_updates: false,
            update_note: "codex が新しい版を知らせます（`codex update` で更新します）",
            shell_note: None,
            troubleshoot_url: "https://learn.chatgpt.com/docs/codex/cli",
            // ユーザー権限だけで入るので代行する（#2791。モジュールドキュメントの「Windows の codex / agy」）
            tako_can_run: true,
            // これが無いと端末つきの実行で最後の `Start Codex now? [y/N]` を聞く（install.ps1 の Prompt-YesNo）
            installer_env: &[("CODEX_NON_INTERACTIVE", "1")],
            runner: WINDOWS_POWERSHELL_RUNNER,
        },
        // agy（2026-09-09 実測。モジュールドキュメントの「agy の調査の根拠」）
        (Platform::MacOs, AgentKind::Agy) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://antigravity.google/cli/install.sh",
                // 取得物の shebang は `#!/bin/bash`（`set -euo pipefail` を使う）
                interpreter: "bash",
                official_command: "curl -fsSL https://antigravity.google/cli/install.sh | bash",
            },
            launcher_rel: ".local/bin/agy",
            // 単一バイナリなので実体ディレクトリが無い
            payload_rel: None,
            payload_root: PayloadRoot::Home,
            auto_updates: true,
            update_note: "Antigravity CLI が実行のたびに自分で背景更新します",
            shell_note: None,
            troubleshoot_url: "https://antigravity.google/docs/cli/troubleshooting",
            tako_can_run: true,
            installer_env: &[],
            runner: InterpreterSpec {
                candidates: &["bash"],
                fallback: "/bin/bash",
                passing: ScriptPassing::Path,
                script_ext: "sh",
                signature: ScriptSignature::Shebang,
            },
        },
        (Platform::Windows, AgentKind::Agy) => InstallRecipe {
            agent,
            platform,
            source: InstallerSource {
                url: "https://antigravity.google/cli/install.ps1",
                interpreter: "powershell",
                official_command: "irm https://antigravity.google/cli/install.ps1 | iex",
            },
            // 公式 docs の Windows 節（`C:\Users\<username>\AppData\Local\agy\bin`）
            launcher_rel: "AppData/Local/agy/bin/agy.exe",
            payload_rel: None,
            payload_root: PayloadRoot::Home,
            auto_updates: true,
            update_note: "Antigravity CLI が実行のたびに自分で背景更新します",
            shell_note: None,
            troubleshoot_url: "https://antigravity.google/docs/cli/troubleshooting",
            // ユーザー権限だけで入るので代行する（#2791。モジュールドキュメントの「Windows の codex / agy」）
            tako_can_run: true,
            installer_env: &[],
            runner: WINDOWS_POWERSHELL_RUNNER,
        },
    }
}

/// [`recipe`] に #2791 の A/B を重ねる（**純粋関数**。`legacy_2791` = #2791 より前）。
///
/// #2791 より前は Windows の codex / agy を代行せず、状態照会と公式手順の案内までだった
/// （`tako_can_run: false`）。それ以外の欄は前後で同じ
pub fn recipe_in(platform: Platform, agent: AgentKind, legacy_2791: bool) -> InstallRecipe {
    let mut r = recipe(platform, agent);
    if legacy_2791 && platform == Platform::Windows && agent != AgentKind::Claude {
        r.tako_can_run = false;
    }
    r
}

/// `TAKO_2791_LEGACY=1` で #2791 より前（Windows の codex / agy は状態照会と案内まで）へ戻す。
/// **同一バイナリで A/B が取れる**（Windows の CI の `scripts/windows/test-agent-install-2791.ps1` が使う）
pub fn issue2791_legacy() -> bool {
    std::env::var_os("TAKO_2791_LEGACY").is_some()
}

/// Windows の `install.ps1` を走らせる形（3 系統で同じなので 1 箇所に置く）。
///
/// - **5.1（`powershell.exe`）を先に置く**。Windows へ必ず同梱されている側を既定に
///   するとマシンごとの差が出ない（pwsh 7 は任意導入）。3 系統の `install.ps1` は
///   どれも 5.1 で動く形で書かれている（`Set-StrictMode` / `Invoke-RestMethod` / `Invoke-WebRequest`、
///   codex は junction の付け替えに `Add-Type` の C#。Windows の CI で 5.1 から実走して確かめる = #2791）
/// - **本文をテキストで読んで走らせる**（[`ScriptPassing::PowerShellText`]。#2790）。落としたファイルを
///   `-File` で走らせると既定の `RemoteSigned` に弾かれる（claude で実機実測。#1057）。#1057 は
///   `-ExecutionPolicy Bypass` で避けたが、これは Process スコープの指定なので、グループポリシーで
///   AllSigned / Restricted を強制した管理端末（学校・会社の PC）では負ける。実行ポリシーが止めるのは
///   スクリプトファイルだけで、公式の `irm | iex` が通るのも文字列を食わせるから。同じ理由で通る形へ寄せた
/// - `-NoProfile` はユーザーの profile を挟まないため
const WINDOWS_POWERSHELL_RUNNER: InterpreterSpec = InterpreterSpec {
    candidates: &["powershell", "pwsh"],
    passing: ScriptPassing::PowerShellText,
    fallback: "powershell.exe",
    script_ext: "ps1",
    signature: ScriptSignature::PowerShell,
};

/// この実行環境のプラットフォーム
pub fn current_platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::MacOs
    }
}

/// この実行環境向けの手順（A/B は `TAKO_2791_LEGACY=1` = [`issue2791_legacy`]）
pub fn current_recipe(agent: AgentKind) -> InstallRecipe {
    recipe_in(current_platform(), agent, issue2791_legacy())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 公式 docs（2026-08-21 実測）と食い違ったらここが落ちる。
    /// **経路を推測で変えないための固定**
    #[test]
    fn macosは公式のnativeインストーラを使う() {
        let r = recipe(Platform::MacOs, AgentKind::Claude);
        assert_eq!(r.source.url, "https://claude.ai/install.sh");
        assert_eq!(r.source.interpreter, "bash");
        assert_eq!(
            r.source.official_command,
            "curl -fsSL https://claude.ai/install.sh | bash"
        );
        assert!(r.auto_updates, "native は自動更新が効くのが採用理由");
        assert!(r.tako_can_run, "macOS は tako が実行を代行する");
    }

    /// install.sh は Windows を明示的に弾く（実物で確認）。
    /// **Windows で誤って bash 経路を走らせない**ための固定
    #[test]
    fn windowsはpowershell経路で実行する() {
        let r = recipe(Platform::Windows, AgentKind::Claude);
        assert_eq!(r.source.url, "https://claude.ai/install.ps1");
        assert_eq!(r.source.interpreter, "powershell");
        assert!(r.tako_can_run, "#1057 の実機実測を経て代行する");
        assert!(r.launcher_rel.ends_with(".exe"));
        // bash 経路が混ざっていない（`install.sh` は Windows を明示的に弾くので、
        // ここが bash に化けると必ず失敗する）
        assert!(!r.runner.candidates.contains(&"bash"));
        assert_eq!(r.runner.script_ext, "ps1");
    }

    /// 走らせ方はプラットフォームごとに排他（**macOS から両方を検証する**）
    #[test]
    fn 走らせ方はプラットフォームごとに決まる() {
        let mac = recipe(Platform::MacOs, AgentKind::Claude).runner;
        assert_eq!(mac.candidates, &["bash"]);
        assert_eq!(mac.passing, ScriptPassing::Path, "bash はパスを直接受ける");
        assert_eq!(mac.script_ext, "sh");
        assert_eq!(mac.signature, ScriptSignature::Shebang);

        let win = recipe(Platform::Windows, AgentKind::Claude).runner;
        assert_eq!(win.candidates, &["powershell", "pwsh"]);
        // 本文をテキストで読んで走らせる（実行ポリシーの対象外。#2790）
        assert_eq!(win.passing, ScriptPassing::PowerShellText);
        assert_eq!(win.signature, ScriptSignature::PowerShell);
    }

    #[test]
    fn unixのargvはスクリプトのパス1個() {
        let script = std::path::Path::new("/tmp/tako-install.sh");
        for agent in AgentKind::ALL {
            let mac = recipe(Platform::MacOs, agent).runner;
            assert_eq!(
                mac.args_for(script),
                vec![std::ffi::OsString::from("/tmp/tako-install.sh")],
                "{} は引数 1 個",
                agent.as_str()
            );
        }
    }

    // --- #2790: Windows の起こし方（実行ポリシーに頼らない） ---

    /// 一時 dir の置き場の例（日本語のユーザー名・空白・単引用符を含む = 層ごとに崩れやすい形）
    const WIN_SCRIPT_2790: &str =
        r"C:\Users\山田 太郎\AppData\Local\Temp\tako-claude-install-'1'.ps1";

    /// `-EncodedCommand` の値を復号して、`container_safe_script`（#906）が足した末尾の空白を落とす
    fn decoded_2790(encoded: &std::ffi::OsString) -> String {
        let text = crate::platform::shell::decode_powershell_command(&encoded.to_string_lossy());
        text.trim_end_matches(' ').to_string()
    }

    /// **製品の既定の口（`args_for` = env を読む側）で**組み立てを縛る。`TAKO_2790_LEGACY=1` の注入や
    /// `-ExecutionPolicy Bypass -File` へ戻す変更は、このテストが名指しで落とす（受け入れ条件 3）
    #[test]
    fn issue2790_windowsの導入代行は実行ポリシーに頼らずencodedcommandで起こす() {
        let script = std::path::Path::new(WIN_SCRIPT_2790);
        for agent in AgentKind::ALL {
            let who = agent.as_str();
            let args = recipe(Platform::Windows, agent).runner.args_for(script);
            let words: Vec<String> = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            // `-OutputFormat Text` = 捕捉したエラーを CLIXML にさせない（`-File` のときと同じ平文）
            assert_eq!(
                words[..5],
                [
                    "-NoLogo",
                    "-NoProfile",
                    "-OutputFormat",
                    "Text",
                    "-EncodedCommand"
                ],
                "{who}: {words:?}"
            );
            assert_eq!(words.len(), 6, "{who}: 符号化した 1 個で終わる: {words:?}");
            // 実行ポリシーに頼る形・スクリプトファイルを実行する形が混ざっていない
            for flag in ["-ExecutionPolicy", "Bypass", "-File", "-Command"] {
                assert!(
                    !words.iter().any(|w| w.eq_ignore_ascii_case(flag)),
                    "{who}: {flag} が残っている: {words:?}"
                );
            }
            assert!(
                !words.iter().any(|w| w.contains("tako-claude-install")),
                "{who}: スクリプトのパスを素の argv で渡している: {words:?}"
            );
            // 復号すると本文の正本に戻る（UTF-16LE の base64）
            let decoded = decoded_2790(&args[5]);
            assert_eq!(
                decoded,
                crate::platform::shell::powershell_script_text_command(script),
                "{who}"
            );
            assert!(
                decoded.contains("[scriptblock]::Create([System.IO.File]::ReadAllText("),
                "{who}: 本文をテキストで読んでいない: {decoded}"
            );
            assert!(decoded.is_ascii(), "{who}: 起動片は ASCII だけ: {decoded}");
        }
    }

    /// パスの日本語・空白・単引用符は ASCII だけの式で埋まり、復号すると元のパスの式へ戻る
    /// （シェル統合 #2432 と同じ `powershell_ascii_literal` を共有する）
    #[test]
    fn issue2790_起動片はパスの非asciiと空白と引用符を崩さない() {
        let script = std::path::Path::new(WIN_SCRIPT_2790);
        let args = recipe(Platform::Windows, AgentKind::Claude)
            .runner
            .args_for_in(script, false);
        let decoded = decoded_2790(&args[5]);
        let literal = crate::shell_integration::powershell_ascii_literal(WIN_SCRIPT_2790);
        assert_eq!(
            literal,
            r"'C:\Users\' + [char]0x5C71 + [char]0x7530 + ' ' + [char]0x592A + [char]0x90CE + '\AppData\Local\Temp\tako-claude-install-''1''.ps1'"
        );
        // #2822 から本文は関数 `install.ps1` として定義し、名前でドットソースする（1 行目は色の止め）
        assert_eq!(
            decoded,
            format!(
                "{}\n\
                 try {{\n\
                 ${{function:install.ps1}} = [scriptblock]::Create([System.IO.File]::ReadAllText({literal}))\n\
                 . install.ps1\n\
                 }} catch {{ [Console]::Error.WriteLine(($_ | Out-String).TrimEnd()); exit 1 }}\n\
                 exit 0\n",
                crate::platform::shell::POWERSHELL_PLAIN_WHEN_CAPTURED
            )
        );
    }

    /// 起動片は本文を**ドットソース**で走らせる（#2791）。子スコープ（`&`）だと、最上位の変数と
    /// `$script:` の変数が別になり、agy の `install.ps1` の取得物の片付け（`$script:stagingPayload` を書いて
    /// `finally` で `$stagingPayload` を読む）が飛ぶ。#2822 からは本文を関数 `install.ps1` として定義し、
    /// その名前をドットソースする（スコープは同じ。5.1 の最上位の `Write-Error` にインストーラの全文が
    /// 出ない）。製品の既定の口（`args_for` = 3 系統の Windows のレシピ）で縛るので、`&` へ戻す変更は
    /// ここが名指しで落とす
    #[test]
    fn issue2791_起動片はscriptblockをドットソースで走らせる() {
        let script = std::path::Path::new(WIN_SCRIPT_2790);
        for agent in AgentKind::ALL {
            let who = agent.as_str();
            let args = recipe(Platform::Windows, agent).runner.args_for(script);
            let decoded = decoded_2790(&args[5]);
            assert!(
                decoded.contains("\n${function:install.ps1} = [scriptblock]::Create(")
                    && decoded.contains("\n. install.ps1\n"),
                "{who}: 起動片が本文の関数をドットソースしていない（子スコープでは $script: の変数が最上位と別になる。#2791 / #2822）: {decoded}"
            );
            assert!(
                !decoded.contains("& ([scriptblock]") && !decoded.contains("& install.ps1"),
                "{who}: 子スコープの呼び出しが残っている: {decoded}"
            );
        }
        // 旧形（比較のためだけに組める）は子スコープの `&`。違いは呼び出しの 1 文字だけ
        let new = crate::platform::shell::powershell_script_text_command_in(script, false);
        let old = crate::platform::shell::powershell_script_text_command_in(script, true);
        assert_eq!(
            old,
            new.replacen("try { . (", "try { & (", 1),
            "旧形との差は呼び出しの 1 文字だけ"
        );
        assert_ne!(old, new);
    }

    /// 符号化そのものの往復（非 ASCII・改行・引用符・`$`・バッククォートを含む本文）。
    /// 起動片は ASCII だけだが、同じ符号化を #2791 ほかが本文に使っても化けないことを縛る
    #[test]
    fn issue2790_符号化はutf16leのbase64で元の本文に戻る() {
        let body = "param([string]$Target = \"latest\")\n\
                    Write-Output '日本語の印 ''単'' \"二重\" `$dollar ✅'\r\n\
                    exit 7\n";
        let encoded = crate::platform::shell::encode_powershell_command(body);
        assert!(
            encoded
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)),
            "base64 の文字だけ: {encoded}"
        );
        let decoded = crate::platform::shell::decode_powershell_command(&encoded);
        assert_eq!(decoded.trim_end_matches(' '), body);
    }

    /// argv に載るのは起動片だけなので、長さはインストーラの大きさに依らない。
    /// 置き場が長い非 ASCII のパスでもコマンドラインの上限（32,767 文字）に十分収まる
    /// （本文を載せる案は codex の `install.ps1` = 71,052 バイトで約 19 万字になり載らない）
    #[test]
    fn issue2790_起こし方の長さはインストーラの大きさに依らず上限に収まる() {
        const WINDOWS_COMMAND_LINE_MAX: usize = 32_767;
        let program = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
        let line_len = |script: &str| {
            let args = recipe(Platform::Windows, AgentKind::Claude)
                .runner
                .args_for_in(std::path::Path::new(script), false);
            program.len() + args.iter().map(|a| a.len() + 1).sum::<usize>()
        };
        let typical = line_len(r"C:\Users\winuser\AppData\Local\Temp\tako-claude-install-4242.ps1");
        assert!(typical < 2_000, "ふだんの置き場: {typical} 文字");
        // 260 文字ぶんの非 ASCII（1 文字が `[char]0xNNNN` の 12 文字 + 区切りへ膨らむ最悪の形）
        let long = format!(r"C:\{}\tako-claude-install-4242.ps1", "山".repeat(260));
        let worst = line_len(&long);
        assert!(
            worst < WINDOWS_COMMAND_LINE_MAX / 2,
            "長い非 ASCII の置き場: {worst} 文字"
        );
    }

    /// 修正前の形は A/B（`TAKO_2790_LEGACY=1`）と CI の「これは弾かれる」の比較でだけ組む
    #[test]
    fn issue2790_修正前の起こし方はabの旧アームで再現できる() {
        let script = std::path::Path::new(WIN_SCRIPT_2790);
        let args = recipe(Platform::Windows, AgentKind::Claude)
            .runner
            .args_for_in(script, true);
        let mut expected: Vec<std::ffi::OsString> = LEGACY_POWERSHELL_FILE_ARGS
            .iter()
            .map(std::ffi::OsString::from)
            .collect();
        expected.push(script.as_os_str().to_os_string());
        assert_eq!(args, expected);
        // unix の渡し方は A/B で変わらない
        let mac = recipe(Platform::MacOs, AgentKind::Claude).runner;
        assert_eq!(
            mac.args_for_in(script, true),
            mac.args_for_in(script, false)
        );
    }

    /// 失敗したときに案内する手で打つ 1 行も実行ポリシーの対象外の形（#2790）。
    /// 公式の `irm <url> | iex` は文字列を `iex` で走らせる = スクリプトファイルを実行しないので
    /// 管理端末でも通り、最も簡単な形（#322）。`-File` や `-ExecutionPolicy` を足した形にしない
    #[test]
    fn issue2790_windowsで手で打つ案内も実行ポリシーの対象外の公式の1行() {
        for agent in AgentKind::ALL {
            let r = recipe(Platform::Windows, agent);
            assert_eq!(
                r.source.official_command,
                format!("irm {} | iex", r.source.url),
                "{}",
                agent.as_str()
            );
            for flag in ["-File", "-ExecutionPolicy", "Bypass", "powershell"] {
                assert!(
                    !r.source.official_command.contains(flag),
                    "{}: {flag}",
                    agent.as_str()
                );
            }
        }
    }

    /// 取得したものの見分け方（**実物の先頭で確認した形**を固定する）。
    /// プロキシの HTML / JSON エラーページを弾けることが要点
    #[test]
    fn 取得したものがインストーラかを署名で見分ける() {
        // `install.sh`（2026-08-21 実物）
        assert!(looks_like_installer(
            ScriptSignature::Shebang,
            b"#!/bin/bash\nset -e\n"
        ));
        assert!(looks_like_installer(
            ScriptSignature::Shebang,
            b"\n#!/usr/bin/env bash\n"
        ));
        // `install.ps1`（2026-09-01 実物の先頭。shebang を持たない）
        let ps1 = b"param(\n    [Parameter(Position=0)]\n    [string]$Target = \"latest\"\n)\n\nSet-StrictMode -Version Latest\n$ErrorActionPreference = \"Stop\"\n";
        assert!(looks_like_installer(ScriptSignature::PowerShell, ps1));
        // 署名が逆だと通らない（プラットフォームの取り違えを検出する）
        assert!(!looks_like_installer(ScriptSignature::Shebang, ps1));
        assert!(!looks_like_installer(
            ScriptSignature::PowerShell,
            b"#!/bin/bash\nset -e\n"
        ));
        // HTML / JSON のエラーページ・空はどちらの署名でも弾く
        for signature in [ScriptSignature::Shebang, ScriptSignature::PowerShell] {
            assert!(!looks_like_installer(signature, b"<!DOCTYPE html>"));
            assert!(!looks_like_installer(signature, b"<html><body>403"));
            assert!(!looks_like_installer(
                signature,
                b"{\"error\":\"forbidden\"}"
            ));
            assert!(!looks_like_installer(signature, b""));
        }
    }

    #[test]
    fn 設置先はhomeを与えて解決する() {
        let home = std::path::Path::new("/tmp/h");
        let r = recipe(Platform::MacOs, AgentKind::Claude);
        assert_eq!(r.launcher_path_in(home), home.join(".local/bin/claude"));
        assert_eq!(r.launcher_dir_in(home), home.join(".local/bin"));
        assert_eq!(
            r.payload_dir_in(home, &home.join("codex-home")),
            Some(home.join(".local/share/claude/versions"))
        );
    }

    /// codex の実体は**渡された codex の置き場**の下（ホームの `.codex` を決め打ちしない。#2810）。
    /// 両 OS の手順とも同じ根を持ち、claude / agy は codex の置き場に引きずられない
    #[test]
    fn issue2810_codexの本体はcodexの置き場の下() {
        let home = std::path::Path::new("/tmp/h");
        for platform in [Platform::MacOs, Platform::Windows] {
            for codex_home in [
                std::path::PathBuf::from("/tmp/custom-codex"),
                std::path::PathBuf::from(r"D:\codex"),
            ] {
                let codex = recipe(platform, AgentKind::Codex);
                assert_eq!(codex.payload_root, PayloadRoot::CodexHome, "{platform:?}");
                assert_eq!(
                    codex.payload_dir_in(home, &codex_home),
                    Some(codex_home.join("packages").join("standalone")),
                    "{platform:?} {codex_home:?}"
                );
                let claude = recipe(platform, AgentKind::Claude);
                assert_eq!(claude.payload_root, PayloadRoot::Home, "{platform:?}");
                assert!(claude
                    .payload_dir_in(home, &codex_home)
                    .is_some_and(|p| p.starts_with(home)));
                assert_eq!(
                    recipe(platform, AgentKind::Agy).payload_dir_in(home, &codex_home),
                    None
                );
            }
        }
    }

    /// macOS は 3 系統とも `~/.local/bin`（= tako の PATH ブロック 1 個で足りる。#989 の
    /// やること 4）。**Windows は違う**ので、そこを取り違えないように両方を固定する
    #[test]
    fn macosの置き場は3系統とも同じでwindowsは系統ごとに違う() {
        for agent in AgentKind::ALL {
            assert_eq!(
                recipe(Platform::MacOs, agent).launcher_dir_rel(),
                ".local/bin",
                "{} の macOS の置き場",
                agent.as_str()
            );
        }
        assert_eq!(
            recipe(Platform::Windows, AgentKind::Claude).launcher_dir_rel(),
            ".local/bin"
        );
        assert_eq!(
            recipe(Platform::Windows, AgentKind::Codex).launcher_dir_rel(),
            "AppData/Local/Programs/OpenAI/Codex/bin"
        );
        assert_eq!(
            recipe(Platform::Windows, AgentKind::Agy).launcher_dir_rel(),
            "AppData/Local/agy/bin"
        );
    }

    /// 3 系統 × 2 プラットフォームの手順が**全部揃っていて素性が矛盾しない**
    /// （#989。1 マスでも埋め忘れると `recipe` が組めないので、埋まっている中身を見る）
    #[test]
    fn 全系統全プラットフォームの手順が揃っている() {
        let home = std::path::Path::new("/tmp/h");
        for platform in [Platform::MacOs, Platform::Windows] {
            for agent in AgentKind::ALL {
                let r = recipe(platform, agent);
                let who = format!("{}/{}", platform.as_str(), agent.as_str());
                assert_eq!(r.agent, agent, "{who}");
                assert_eq!(r.platform, platform, "{who}");
                assert!(r.source.url.starts_with("https://"), "{who} は https 必須");
                // 公式の 1 行に取得元 URL がそのまま出ている（案内と実行がずれない）
                assert!(
                    r.source.official_command.contains(r.source.url),
                    "{who} の official_command に URL が無い"
                );
                assert!(!r.update_note.is_empty(), "{who} の更新の説明が空");
                assert!(
                    r.troubleshoot_url.starts_with("https://"),
                    "{who} のトラブルシュート URL"
                );
                // ランチャーはコマンド名で終わる（`.exe` は Windows だけ）
                assert!(
                    r.launcher_rel.ends_with(agent.as_str())
                        || r.launcher_rel.ends_with(&format!("{}.exe", agent.as_str())),
                    "{who} の launcher_rel={}",
                    r.launcher_rel
                );
                assert_eq!(
                    r.launcher_rel.ends_with(".exe"),
                    platform == Platform::Windows,
                    "{who} の拡張子"
                );
                // 置き場所は必ずホームの中（管理者権限が要らないことの裏付け）
                assert!(
                    !r.launcher_rel.starts_with('/') && !r.launcher_rel.contains(':'),
                    "{who} の置き場所がホームの外"
                );
                assert!(r.launcher_dir_in(home).starts_with(home), "{who}");
                // codex の置き場が既定（ホームの中）のとき、本体もホームの中
                if let Some(payload) = r.payload_dir_in(home, &home.join(".codex")) {
                    assert!(payload.starts_with(home), "{who} の本体がホームの外");
                }
                // 根からの相対も外へ出ない（`CODEX_HOME` を外へ向けたときに外へ出るのは根だけ）
                if let Some(rel) = r.payload_rel {
                    assert!(
                        !rel.starts_with('/') && !rel.contains(':') && !rel.contains(".."),
                        "{who} の payload_rel={rel}"
                    );
                }
                assert!(!agent.product().is_empty(), "{who} の製品名が空");
            }
        }
    }

    /// 失敗の案内は系統ごとに違う（claude のページを codex の失敗に出さない。#989）
    #[test]
    fn トラブルシュートの案内は系統ごとに違う() {
        for platform in [Platform::MacOs, Platform::Windows] {
            let urls: Vec<&str> = AgentKind::ALL
                .into_iter()
                .map(|a| recipe(platform, a).troubleshoot_url)
                .collect();
            for (i, a) in urls.iter().enumerate() {
                for b in urls.iter().skip(i + 1) {
                    assert_ne!(a, b, "{platform:?}: 案内先が同じ");
                }
            }
            // 他系統の失敗に claude のページを出さない（実際に踏んだ退行。#989）
            assert!(!recipe(platform, AgentKind::Codex)
                .troubleshoot_url
                .contains("claude"));
            assert!(!recipe(platform, AgentKind::Agy)
                .troubleshoot_url
                .contains("claude"));
        }
    }

    /// 単一バイナリの系統は「本体の置き場所」を持たない（同じパスを 2 回言わない）
    #[test]
    fn 単一バイナリのagyは本体の置き場所を持たない() {
        for platform in [Platform::MacOs, Platform::Windows] {
            assert!(recipe(platform, AgentKind::Agy).payload_rel.is_none());
            assert!(recipe(platform, AgentKind::Claude).payload_rel.is_some());
            assert!(recipe(platform, AgentKind::Codex).payload_rel.is_some());
        }
    }

    /// 3 系統 × 2 プラットフォームのすべてで導入を代行する（#2791。Windows の codex / agy の
    /// `install.ps1` はユーザー権限だけで入る = モジュールドキュメントの「Windows の codex / agy」）。
    /// Windows の codex / agy を案内止まりへ戻す変更はここが名指しで落とす
    #[test]
    fn issue2791_windowsも3系統とも導入を代行する() {
        for platform in [Platform::MacOs, Platform::Windows] {
            for agent in AgentKind::ALL {
                let r = recipe(platform, agent);
                assert!(
                    r.tako_can_run,
                    "{}/{} は代行する（#2791）",
                    platform.as_str(),
                    agent.as_str()
                );
                // A/B の旧アームが無いときは recipe と同じ
                assert_eq!(recipe_in(platform, agent, false), r);
            }
        }
        // Windows で代行する 3 系統は同じ 1 実装（#2790 の本文をテキストで読む -EncodedCommand）で起こす
        for agent in AgentKind::ALL {
            assert_eq!(
                recipe(Platform::Windows, agent).runner,
                WINDOWS_POWERSHELL_RUNNER,
                "{}",
                agent.as_str()
            );
        }
    }

    /// `TAKO_2791_LEGACY=1` の旧アーム: Windows の codex / agy だけが案内止まり（`tako_can_run: false`）へ戻り、
    /// macOS と Windows の claude と、代行の可否以外の欄は変わらない
    #[test]
    fn issue2791_旧アームはwindowsのcodexとagyだけ案内止まりへ戻す() {
        for platform in [Platform::MacOs, Platform::Windows] {
            for agent in AgentKind::ALL {
                let new = recipe(platform, agent);
                let old = recipe_in(platform, agent, true);
                let delegated_since_2791 =
                    platform == Platform::Windows && agent != AgentKind::Claude;
                assert_eq!(
                    old.tako_can_run,
                    !delegated_since_2791,
                    "{}/{}",
                    platform.as_str(),
                    agent.as_str()
                );
                assert_eq!(
                    InstallRecipe {
                        tako_can_run: new.tako_can_run,
                        ..old
                    },
                    new,
                    "{}/{}: 代行の可否以外が変わった",
                    platform.as_str(),
                    agent.as_str()
                );
            }
        }
    }

    /// codex のインストーラは最後に `Start Codex now? [y/N]` を聞く（実測）。
    /// **この env が落ちると代行が返ってこない**ので、両プラットフォームで固定する
    #[test]
    fn codexのインストーラは非対話envを必ず渡す() {
        for platform in [Platform::MacOs, Platform::Windows] {
            assert_eq!(
                recipe(platform, AgentKind::Codex).installer_env,
                &[("CODEX_NON_INTERACTIVE", "1")],
                "{}",
                platform.as_str()
            );
            // 他の系統は対話プロンプトを持たないので env を足さない（推測で増やさない）
            assert!(recipe(platform, AgentKind::Claude).installer_env.is_empty());
            assert!(recipe(platform, AgentKind::Agy).installer_env.is_empty());
        }
    }

    /// codex は「自動更新ではないが自分で知らせる」第 3 の形（実測）。
    /// `auto_updates` の真偽から文面を組むと嘘になるので、両方を別に固定する
    #[test]
    fn 更新のされ方は系統ごとに違う() {
        let mac = |a| recipe(Platform::MacOs, a);
        assert!(mac(AgentKind::Claude).auto_updates);
        assert!(mac(AgentKind::Agy).auto_updates);
        assert!(!mac(AgentKind::Codex).auto_updates);
        assert!(mac(AgentKind::Codex).update_note.contains("codex update"));
    }

    #[test]
    fn 系統名は文字列と往復する() {
        for agent in AgentKind::ALL {
            assert_eq!(AgentKind::parse(agent.as_str()), Some(agent));
        }
        assert_eq!(AgentKind::parse("ollama"), None);
        assert_eq!(AgentKind::parse(""), None);
    }

    /// Windows の claude は Git for Windows が無くても動く（#2433 の実測）ので、手順は「要らない」と
    /// PowerShell で走ることを知らせる。確かめていない系統・macOS には書かない（推測で埋めない）
    #[test]
    fn issue2433_windowsのclaudeだけがシェルの1行を持ちgitを要ると言わない() {
        let mut with_note = Vec::new();
        for platform in [Platform::MacOs, Platform::Windows] {
            for agent in AgentKind::ALL {
                if let Some(note) = recipe(platform, agent).shell_note {
                    with_note.push((platform, agent, note));
                }
            }
        }
        let [(platform, agent, note)] = with_note[..] else {
            panic!("シェルの 1 行を持つ手順が Windows の claude の 1 つでない: {with_note:?}");
        };
        assert_eq!((platform, agent), (Platform::Windows, AgentKind::Claude));
        for needle in ["Git for Windows は要りません", "PowerShell", "Git Bash"] {
            assert!(note.contains(needle), "「{needle}」が無い: {note}");
        }
    }
}
