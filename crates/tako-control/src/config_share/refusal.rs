//! AI 系設定の共有（`tako config` / MCP `tako_config_share`）の断り（#2365）。
//!
//! dispatch の `dispatch_config_share` は、以前は `config_share`（`mod.rs` / `portable.rs` / `guard.rs`）の
//! `Err(String)`（日本語の固定文）をそのまま載せていたので、英語表示でも理由だけ日本語で返った。
//! 日英の対は [`ShareRefusal::text_in`] / [`GitError::text_in`] の (バリアント, 言語) の網羅 match の
//! 1 か所に置く。日本語は #2365 より前の固定文とバイト一致
//! （`tests/issue1762_dispatch_error_lang.rs` が突き合わせる）。
//!
//! # 口
//!
//! `config_share` の口（`init` / `link` / `unlink` / `status` / `push` / `pull` ほか）を呼ぶのは dispatch だけ
//! （CLI `tako config` と `tako setup` の配線も dispatch を直呼びする）なので、日本語の `String` の口は残さず、
//! どの口もこの型を返す。応答の値に載る誤り文（`push` の `push_error`・`status` の `files[].error`）も
//! 同じ型から表示言語で組む。
//!
//! OS / serde / git のエラー文（git 自身の stderr）は言語に依らずそのまま載せ、パスは表記のまま埋める。
//! 日本語が「見つからない」と言わない断り（「ディレクトリが見つかりません」）は、英語も ` not found` を
//! 含めない（[`crate::DispatchError::says_not_found`] の当たり方を日英でそろえる）。

use tako_core::i18n::Lang;
use tako_core::load_issue::{JsonReason, YamlReason};

use super::guard::Finding;
use crate::config_io::WriteError;

/// git を起こした失敗（`args` は空白で繋いだ引数の表記）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// git を起動できない（`error` は OS のエラー文）
    Spawn { args: String, error: String },
    /// git が非ゼロで終わった（`stderr` は git 自身の出力。前後の空白は落とす）
    Failed { args: String, stderr: String },
}

impl GitError {
    /// 表示言語を明示しての文言（(バリアント, 言語) の網羅 match）
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::Spawn { args, error }, Ja) => format!("git の実行に失敗 ({args}): {error}"),
            (Self::Spawn { args, error }, En) => format!("failed to run git ({args}): {error}"),
            (Self::Failed { args, stderr }, Ja) => format!("git {args} が失敗しました:\n{stderr}"),
            (Self::Failed { args, stderr }, En) => format!("git {args} failed:\n{stderr}"),
        }
    }

    /// 引数か git の出力に `needle` を含むか（#2365 より前は日本語の文全体を `contains` で見ていた。
    /// 枠の日本語は英語の `needle` に当たらないので、見る範囲は同じ）
    pub fn mentions(&self, needle: &str) -> bool {
        match self {
            Self::Spawn { args, error } => args.contains(needle) || error.contains(needle),
            Self::Failed { args, stderr } => args.contains(needle) || stderr.contains(needle),
        }
    }
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

/// 設定共有の断り。`error` は OS / serde のエラー文（言語に依らずそのまま載せる）、パスは表記のまま埋める
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareRefusal {
    // ---- 置き場 ----
    /// データディレクトリを解決できない
    DataDirUnresolved,
    /// ホームディレクトリを解決できない（$HOME 未設定）
    HomeUnresolved,
    /// カレントディレクトリを解決できない（相対パスを絶対にできない）
    CwdUnresolved(String),

    // ---- 配線状態（`<data_dir>/config-share.json`） ----
    /// 配線状態を読めない（`path` はそのファイル）
    StateRead { path: String, error: String },
    /// 配線状態を JSON として解釈できない
    StateParse { path: String, reason: JsonReason },
    /// 配線状態を JSON にできない
    StateSerialize(String),
    /// 配線状態を消せない
    StateRemove(String),
    /// 書き出せない（配線状態・pull の取り込み先。`config_io` の文をそのまま使う）
    Write(WriteError),
    /// まだ配線していない
    NotLinked,
    /// 配線先が git リポジトリでない（`.git` が無い）
    LinkedNotGit(String),

    // ---- git ----
    /// git の失敗
    Git(GitError),
    /// commit が「名前が未設定」で断られた（git の文に設定の案内を足す）
    CommitNoIdent(GitError),
    /// `pull --ff-only` が失敗した（分岐の解き方を足す。`repo` は共有リポジトリの表記）
    PullDiverged { git: GitError, repo: String },

    // ---- 変換（`portable`） ----
    /// 読めない（`path` は実体のパスかリポジトリ相対のパス）
    ReadFailed { path: String, error: String },
    /// YAML を解釈できない
    YamlParse(YamlReason),
    /// YAML に書き出せない
    YamlEmit(String),
    /// JSON を解釈できない
    JsonParse(JsonReason),
    /// JSON に書き出せない
    JsonEmit(String),
    /// 秘匿情報らしき内容が共有対象にある（値そのものは持たない = 場所と種類だけ）
    SecretsFound(Vec<Finding>),

    // ---- init / link ----
    /// init 先が空でなく git リポジトリでもない
    InitNotEmpty(String),
    /// ディレクトリを作れない
    CreateDir(String),
    /// link の対象が空
    LinkTargetEmpty,
    /// clone 先が空でない
    CloneDestNotEmpty(String),
    /// clone 先の親ディレクトリを決められない
    CloneParentUnresolved,
    /// link のディレクトリが無い
    DirNotFound(String),
    /// link のディレクトリが git リポジトリでない
    LinkNotGit(String),

    // ---- push（共有リポジトリへの書き出し） ----
    /// ディレクトリを作れない（`path` は作ろうとしたディレクトリ）
    CreateDirAt { path: String, error: String },
    /// 書き込めない
    WriteFailed { path: String, error: String },
    /// 消せない
    RemoveFailed { path: String, error: String },
    /// manifest を YAML にできない
    ManifestSerialize(String),
    /// manifest を書けない
    ManifestWrite(String),
    /// README を書けない
    ReadmeWrite(String),
    /// .gitignore を書けない
    GitignoreWrite(String),
}

impl ShareRefusal {
    /// 表示言語を明示しての文言（(バリアント, 言語) の網羅 match）
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::DataDirUnresolved, Ja) => "データディレクトリを解決できません".to_string(),
            (Self::DataDirUnresolved, En) => "cannot resolve the data directory".to_string(),
            (Self::HomeUnresolved, Ja) => {
                "ホームディレクトリを解決できません（$HOME 未設定）".to_string()
            }
            (Self::HomeUnresolved, En) => {
                "cannot resolve the home directory ($HOME is not set)".to_string()
            }
            (Self::CwdUnresolved(error), Ja) => {
                format!("カレントディレクトリを解決できません: {error}")
            }
            (Self::CwdUnresolved(error), En) => {
                format!("cannot resolve the current directory: {error}")
            }
            (Self::StateRead { path, error }, Ja) => {
                format!("配線状態の読み取りに失敗 ({path}): {error}")
            }
            (Self::StateRead { path, error }, En) => {
                format!("failed to read the link state ({path}): {error}")
            }
            (Self::StateParse { path, reason }, Ja) => {
                format!("配線状態のパースに失敗 ({path}): {}", reason.text_in(lang))
            }
            (Self::StateParse { path, reason }, En) => {
                format!("failed to parse the link state ({path}): {}", reason.text_in(lang))
            }
            (Self::StateSerialize(error), Ja) => format!("配線状態のシリアライズに失敗: {error}"),
            (Self::StateSerialize(error), En) => {
                format!("failed to serialize the link state: {error}")
            }
            (Self::StateRemove(error), Ja) => format!("配線状態の削除に失敗: {error}"),
            (Self::StateRemove(error), En) => format!("failed to remove the link state: {error}"),
            (Self::Write(error), Ja | En) => error.text_in(lang),
            (Self::NotLinked, Ja) => "設定共有が未配線です。`tako config init` で新規リポジトリを作るか、\n\
                 `tako config link <パス|URL>` で既存リポジトリに繋いでください"
                .to_string(),
            (Self::NotLinked, En) => "config share is not linked. Create a new repository with \
                 `tako config init`,\nor connect to an existing one with `tako config link <path|URL>`"
                .to_string(),
            (Self::LinkedNotGit(repo), Ja) => format!(
                "配線先が git リポジトリではありません: {repo}\n\
                 `tako config link <パス|URL>` で繋ぎ直してください"
            ),
            (Self::LinkedNotGit(repo), En) => format!(
                "the linked location is not a git repository: {repo}\n\
                 reconnect with `tako config link <path|URL>`"
            ),
            (Self::Git(error), Ja | En) => error.text_in(lang),
            (Self::CommitNoIdent(error), Ja) => format!(
                "{}\n\nこのデバイスに git の名前が未設定です。次を実行してから再試行してください:\n  \
                 git config --global user.name \"あなたの名前\"\n  \
                 git config --global user.email \"you@example.com\"",
                error.text_in(lang)
            ),
            (Self::CommitNoIdent(error), En) => format!(
                "{}\n\ngit has no user name on this device. Run the following, then retry:\n  \
                 git config --global user.name \"Your Name\"\n  \
                 git config --global user.email \"you@example.com\"",
                error.text_in(lang)
            ),
            (Self::PullDiverged { git, repo }, Ja) => format!(
                "{}\n\n\
                 両方のデバイスで同じ設定を変更した可能性があります。次のどちらかで解決してください:\n\
                 1. 共有リポジトリで解決する:\n   \
                    cd {repo} && git pull --rebase   # 競合したら中身を直して git rebase --continue\n\
                 2. このデバイスの内容を正とする:\n   \
                    cd {repo} && git rebase --abort（実行中なら）→ tako config push\n\
                 解決後にもう一度 `tako config pull` を実行してください",
                git.text_in(lang)
            ),
            (Self::PullDiverged { git, repo }, En) => format!(
                "{}\n\n\
                 Both devices may have changed the same settings. Resolve it in one of these ways:\n\
                 1. Resolve it in the shared repository:\n   \
                    cd {repo} && git pull --rebase   # on conflicts, fix the files and run git rebase --continue\n\
                 2. Keep this device's contents:\n   \
                    cd {repo} && git rebase --abort (if one is in progress) -> tako config push\n\
                 Then run `tako config pull` again",
                git.text_in(lang)
            ),
            (Self::ReadFailed { path, error }, Ja) => format!("読み取りに失敗 ({path}): {error}"),
            (Self::ReadFailed { path, error }, En) => format!("failed to read ({path}): {error}"),
            (Self::YamlParse(reason), Ja) => {
                format!("YAML のパースに失敗: {}", reason.text_in(lang))
            }
            (Self::YamlParse(reason), En) => {
                format!("failed to parse YAML: {}", reason.text_in(lang))
            }
            (Self::YamlEmit(error), Ja) => format!("YAML の出力に失敗: {error}"),
            (Self::YamlEmit(error), En) => format!("failed to write YAML: {error}"),
            (Self::JsonParse(reason), Ja) => {
                format!("JSON のパースに失敗: {}", reason.text_in(lang))
            }
            (Self::JsonParse(reason), En) => {
                format!("failed to parse JSON: {}", reason.text_in(lang))
            }
            (Self::JsonEmit(error), Ja) => format!("JSON の出力に失敗: {error}"),
            (Self::JsonEmit(error), En) => format!("failed to write JSON: {error}"),
            (Self::SecretsFound(findings), _) => {
                let head = match lang {
                    Ja => "秘匿情報らしき内容が共有対象に含まれています。取り除いてから再実行してください:",
                    En => "the shared files contain what looks like a secret. Remove it and run again:",
                };
                let mut lines = vec![head.to_string()];
                for f in findings {
                    lines.push(format!("  - {}:{} ({})", f.path, f.line, f.kind));
                }
                lines.join("\n")
            }
            (Self::InitNotEmpty(repo), Ja) => format!(
                "{repo} は空でないディレクトリで、git リポジトリでもありません。\n\
                 別の場所を --path で指定するか、そこで git init してから `tako config link` してください"
            ),
            (Self::InitNotEmpty(repo), En) => format!(
                "{repo} is a non-empty directory and not a git repository.\n\
                 Pass another location with --path, or run git init there and then `tako config link`"
            ),
            (Self::CreateDir(error), Ja) => format!("ディレクトリの作成に失敗: {error}"),
            (Self::CreateDir(error), En) => format!("failed to create the directory: {error}"),
            (Self::LinkTargetEmpty, Ja) => {
                "リポジトリのパスまたは URL を指定してください".to_string()
            }
            (Self::LinkTargetEmpty, En) => "give the repository path or URL".to_string(),
            (Self::CloneDestNotEmpty(dest), Ja) => format!(
                "clone 先 {dest} が空ではありません。--path で別の場所を指定してください"
            ),
            (Self::CloneDestNotEmpty(dest), En) => format!(
                "the clone destination {dest} is not empty. Pass another location with --path"
            ),
            (Self::CloneParentUnresolved, Ja) => {
                "clone 先の親ディレクトリを解決できません".to_string()
            }
            (Self::CloneParentUnresolved, En) => {
                "cannot resolve the parent directory of the clone destination".to_string()
            }
            // 日本語が「見つからない」と言わないので、英語も ` not found` を含めない
            (Self::DirNotFound(dir), Ja) => format!("ディレクトリが見つかりません: {dir}"),
            (Self::DirNotFound(dir), En) => format!("no such directory: {dir}"),
            (Self::LinkNotGit(repo), Ja) => format!(
                "{repo} は git リポジトリではありません。\n\
                 新規に作るなら `tako config init --path {repo}` を使ってください"
            ),
            (Self::LinkNotGit(repo), En) => format!(
                "{repo} is not a git repository.\n\
                 To create a new one, use `tako config init --path {repo}`"
            ),
            (Self::CreateDirAt { path, error }, Ja) => {
                format!("ディレクトリの作成に失敗 ({path}): {error}")
            }
            (Self::CreateDirAt { path, error }, En) => {
                format!("failed to create the directory ({path}): {error}")
            }
            (Self::WriteFailed { path, error }, Ja) => format!("書き込みに失敗 ({path}): {error}"),
            (Self::WriteFailed { path, error }, En) => {
                format!("failed to write ({path}): {error}")
            }
            (Self::RemoveFailed { path, error }, Ja) => format!("削除に失敗 ({path}): {error}"),
            (Self::RemoveFailed { path, error }, En) => {
                format!("failed to remove ({path}): {error}")
            }
            (Self::ManifestSerialize(error), Ja) => format!("manifest の生成に失敗: {error}"),
            (Self::ManifestSerialize(error), En) => {
                format!("failed to build the manifest: {error}")
            }
            (Self::ManifestWrite(error), Ja) => format!("manifest の書き込みに失敗: {error}"),
            (Self::ManifestWrite(error), En) => {
                format!("failed to write the manifest: {error}")
            }
            (Self::ReadmeWrite(error), Ja) => format!("README の書き込みに失敗: {error}"),
            (Self::ReadmeWrite(error), En) => format!("failed to write the README: {error}"),
            (Self::GitignoreWrite(error), Ja) => format!("gitignore の書き込みに失敗: {error}"),
            (Self::GitignoreWrite(error), En) => {
                format!("failed to write the gitignore: {error}")
            }
        }
    }

    /// 前置きの無い理由の文として載せる（表示言語で組む）
    pub fn operation(self) -> crate::DispatchError {
        crate::DispatchError::Operation(self.to_string())
    }
}

impl std::fmt::Display for ShareRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

impl From<GitError> for ShareRefusal {
    fn from(error: GitError) -> Self {
        Self::Git(error)
    }
}

impl From<WriteError> for ShareRefusal {
    fn from(error: WriteError) -> Self {
        Self::Write(error)
    }
}
