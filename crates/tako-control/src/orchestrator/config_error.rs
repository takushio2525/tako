//! orchestrator の設定ファイル（projects.yaml / accounts.yaml / profiles）の読み書きと
//! エージェント名の検証の断り（#2365）。同じ置き場の委任台帳（ledger.yaml）と引き継ぎファイル
//! （`handoff/`）の読み書きの断りもここに置く（文の形が同じ = 読めない・解析できない・書けない）。
//!
//! 日英の対は [`ConfigError::text_in`] の (バリアント, 言語) の網羅 match の 1 か所に置く
//! （形は [`crate::config_io::WriteError`] と同じ）。日本語は #2365 より前の固定文とバイト一致
//! （`tests/issue1762_dispatch_error_lang.rs` が突き合わせる）。
//!
//! # 口は 2 つ
//!
//! 読み書きの口（`ProjectsConfig::load` ほか）は従来どおり `String` を返し、文は**日本語のまま**
//! （呼び手の多くは persist.log・`tako master` の起動の流れ・日本語の枠の文へ載せる）。
//! 断りを表示言語で返す dispatch（CLI / MCP の `orchestrator projects` / `profiles` / `accounts`・
//! spawn・引き継ぎ・adopt・`ledger`・`handoffs write`・`supervisor set_mode`）だけが
//! 型の口（`*_typed`）を通し、枠（`invalid parameter:` ほか）と中身を同じ言語にそろえる
//! （1 つの断りの中で日英を混ぜない）。
//! 設定ファイルの中身になる文はここに無い（断りだけ）
//!
//! # 読めないプロファイルの理由（#2256 の系統）
//!
//! 起動・継承元・表示・判定の経路がプロファイルを読めないときの理由は
//! [`UnreadableProfile`]（`<path> を読めない（<原因>）`）を芯にし、止めずに組み込み既定で進む経路の
//! 知らせは [`ProfileFallback`]、#1825 の「作らなかった」は [`ConfigError::ProjectProfileNotInherited`]
//! で持つ。利用者へ返す口（応答の `warnings`・断り・CLI の警告の行・check-health）は表示言語、
//! persist.log（日本語の診断ログ）は `text_in(Lang::Ja)` で同じ型から組む。以前は persist.log と
//! 共有する日本語の `String` だったので、英語表示でも理由だけ日本語のまま混ざった

use tako_core::i18n::Lang;
use tako_core::load_issue::YamlReason;

use super::agent::WorkerAgent;
use super::ProfileKind;
use crate::config_io::{LockError, WriteError};
use crate::DispatchError;

/// 読み取り・パースの断りが指す設定ファイル
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFile {
    /// `projects.yaml`
    Projects,
    /// `accounts.yaml`
    Accounts,
    /// `profiles/<name>.yaml` / `solo-profiles/<name>.yaml`
    Profile,
    /// `ledger.yaml`（委任台帳）
    Ledger,
}

/// orchestrator の設定ファイルの読み書きとエージェント名の検証の断り。
/// `error` は OS / serde のエラー文（言語に依らずそのまま載せる）、パス・名前は表記のまま埋める
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// 設定の置き場（ホームディレクトリ / データディレクトリ）を決められない
    HomeUnresolved,
    /// 設定ファイルを読めない
    Read { file: ConfigFile, error: String },
    /// 設定ファイルを YAML として解釈できない
    Parse {
        file: ConfigFile,
        reason: YamlReason,
    },
    /// YAML に書き出せない
    Serialize(String),
    /// 排他ロックを取れない
    Lock(LockError),
    /// 書き出せない
    Write(WriteError),
    /// 設定の置き場のディレクトリを作れない
    CreateDir(String),
    /// `profiles/` を作れない
    CreateProfilesDir(String),
    /// 既定の `default.yaml` を書けない
    WriteDefaultProfile(String),
    /// プロファイルの置き場を一覧できない
    ListProfiles(String),
    /// プロファイルを消せない
    DeleteProfile(String),
    /// プロファイルが無い（`kind` は `None` = 種別を言わない旧来の `Profile::load` の文）
    ProfileNotFound {
        kind: Option<ProfileKind>,
        name: String,
        path: String,
    },
    /// 作ろうとしたプロファイルが既にある
    ProfileExists { kind: ProfileKind, name: String },
    /// 既定プロファイル `default` は消せない
    DefaultProfileUndeletable(ProfileKind),
    /// プロファイルの種別（master / solo）が不正
    InvalidKind(String),
    /// プロファイル名が空
    ProfileNameEmpty,
    /// プロファイル名が `.` で始まる
    ProfileNameLeadingDot(String),
    /// プロファイル名が `-` で始まる
    ProfileNameLeadingDash(String),
    /// プロファイル名に使えない文字がある
    ProfileNameBadChar(char),
    /// accounts.yaml にそのアカウントが無い
    AccountNotFound(String),
    /// アカウントが `inherit: true` と `config_dir` を両方持つ
    AccountInheritWithDir(String),
    /// アカウントが `config_dir` も `inherit: true` も持たない
    AccountNoConfigDir(String),
    /// projects.yaml にそのプロジェクトが無い
    ProjectNotFound(String),
    /// プロジェクトの cwd が無い（展開後のパス）
    ProjectCwdMissing(String),
    /// エージェント種別名が不正
    UnknownAgent(String),
    /// master にできない種別（agy）
    MasterAgentUnsupported(WorkerAgent),
    /// プロファイルの `master_agent` が不正（中身は [`Self::UnknownAgent`] か [`Self::MasterAgentUnsupported`]）
    ProfileMasterAgent(Box<ConfigError>),
    /// プロファイルの `worker_agent` が不正（中身は [`Self::UnknownAgent`]）
    ProfileWorkerAgent(Box<ConfigError>),
    /// プロファイルの cwd が無い（`expanded` は `~` 展開後・`raw` は元の指定）
    ProfileCwdMissing { expanded: String, raw: String },
    /// プロファイルの projects に projects.yaml に無いキーがある
    ProfileProjectsUnregistered(Vec<String>),
    /// プロファイルの env に tako の内部変数がある
    ProfileEnvReserved(Vec<String>),
    /// ctx_threshold が範囲の外
    CtxThresholdOutOfRange(u32),
    /// 委任台帳にその ID のエントリが無い
    LedgerEntryNotFound(String),
    /// 検収結果（outcome）が pass / rework / fail のどれでもない
    LedgerOutcomeInvalid(String),
    /// プロジェクトキーが引き継ぎファイルの名前に使えない
    HandoffKeyUnusable(String),
    /// 引き継ぎの置き場のディレクトリを作れない（`dir` は作ろうとしたパス）
    HandoffDirCreate { dir: String, error: String },
    /// プロファイルを読めない（#2256 の系統の理由の芯。継承元・`tako migrate status` の予告が使う）
    ProfileUnreadable(UnreadableProfile),
    /// 継承元の default.yaml を読めないので専用プロファイル `key` を作らなかった（#1825。
    /// `why` は読めない理由 = [`Self::ProfileUnreadable`] か置き場を決められない断り）
    ProjectProfileNotInherited { key: String, why: Box<ConfigError> },
    /// `tako master -<key>` の専用プロファイルが無く、その理由が #1825（作っていない）。
    /// `not_found` は「見つからない」、`why` は [`Self::ProjectProfileNotInherited`]（#2227）
    ProjectProfileMissing {
        not_found: Box<ConfigError>,
        why: Box<ConfigError>,
    },
}

impl ConfigError {
    /// 表示言語を明示しての文言。(バリアント, 言語) の網羅 match なので、バリアントを足すと
    /// 日英の両方を書くまでコンパイルが通らない。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::HomeUnresolved, Ja) => "ホームディレクトリが取得できない".to_string(),
            (Self::HomeUnresolved, En) => "cannot determine the home directory".to_string(),
            (Self::Read { file, error }, Ja) => match file {
                ConfigFile::Projects => format!("projects.yaml の読み取りに失敗: {error}"),
                ConfigFile::Accounts => format!("accounts.yaml の読み取りに失敗: {error}"),
                ConfigFile::Profile => format!("プロファイルの読み取りに失敗: {error}"),
                ConfigFile::Ledger => format!("ledger.yaml の読み取りに失敗: {error}"),
            },
            (Self::Read { file, error }, En) => match file {
                ConfigFile::Projects => format!("failed to read projects.yaml: {error}"),
                ConfigFile::Accounts => format!("failed to read accounts.yaml: {error}"),
                ConfigFile::Profile => format!("failed to read the profile: {error}"),
                ConfigFile::Ledger => format!("failed to read ledger.yaml: {error}"),
            },
            (Self::Parse { file, reason }, Ja) => {
                let reason = reason.text_in(lang);
                match file {
                    ConfigFile::Projects => format!("projects.yaml のパースに失敗: {reason}"),
                    ConfigFile::Accounts => format!("accounts.yaml のパースに失敗: {reason}"),
                    ConfigFile::Profile => format!("プロファイルのパースに失敗: {reason}"),
                    ConfigFile::Ledger => format!("ledger.yaml のパースに失敗: {reason}"),
                }
            }
            (Self::Parse { file, reason }, En) => {
                let reason = reason.text_in(lang);
                match file {
                    ConfigFile::Projects => format!("cannot parse projects.yaml: {reason}"),
                    ConfigFile::Accounts => format!("cannot parse accounts.yaml: {reason}"),
                    ConfigFile::Profile => format!("cannot parse the profile: {reason}"),
                    ConfigFile::Ledger => format!("cannot parse ledger.yaml: {reason}"),
                }
            }
            (Self::Serialize(e), Ja) => format!("YAML のシリアライズに失敗: {e}"),
            (Self::Serialize(e), En) => format!("failed to serialize the YAML: {e}"),
            (Self::Lock(e), _) => e.text_in(lang),
            (Self::Write(e), _) => e.text_in(lang),
            (Self::CreateDir(e), Ja) => format!("ディレクトリの作成に失敗: {e}"),
            (Self::CreateDir(e), En) => format!("failed to create the directory: {e}"),
            (Self::CreateProfilesDir(e), Ja) => format!("profiles ディレクトリの作成に失敗: {e}"),
            (Self::CreateProfilesDir(e), En) => {
                format!("failed to create the profiles directory: {e}")
            }
            (Self::WriteDefaultProfile(e), Ja) => format!("default.yaml の書き込みに失敗: {e}"),
            (Self::WriteDefaultProfile(e), En) => format!("failed to write default.yaml: {e}"),
            (Self::ListProfiles(e), Ja) => {
                format!("プロファイルディレクトリの読み取りに失敗: {e}")
            }
            (Self::ListProfiles(e), En) => {
                format!("failed to read the profiles directory: {e}")
            }
            (Self::DeleteProfile(e), Ja) => format!("プロファイルの削除に失敗: {e}"),
            (Self::DeleteProfile(e), En) => format!("failed to delete the profile: {e}"),
            (Self::ProfileNotFound { kind, name, path }, Ja) => match kind {
                Some(kind) => format!(
                    "{} プロファイル '{name}' が見つからない: {path}",
                    kind.as_str()
                ),
                None => format!("プロファイル '{name}' が見つからない: {path}"),
            },
            // ` not found` を含める（`DispatchError::says_not_found` の当たり方を日英でそろえる）
            (Self::ProfileNotFound { kind, name, path }, En) => match kind {
                Some(kind) => format!("{} profile '{name}' not found: {path}", kind.as_str()),
                None => format!("profile '{name}' not found: {path}"),
            },
            (Self::ProfileExists { kind, name }, Ja) => format!(
                "{} プロファイル '{name}' は既に存在する（上書きするなら set を使う）",
                kind.as_str()
            ),
            (Self::ProfileExists { kind, name }, En) => format!(
                "{} profile '{name}' already exists (use set to overwrite it)",
                kind.as_str()
            ),
            (Self::DefaultProfileUndeletable(kind), Ja) => format!(
                "既定プロファイル 'default' は削除できない（{} が参照する）",
                kind.launch_bin()
            ),
            (Self::DefaultProfileUndeletable(kind), En) => format!(
                "the default profile 'default' cannot be deleted ({} uses it)",
                kind.launch_bin()
            ),
            (Self::InvalidKind(got), Ja) => format!(
                "kind が不正: '{got}'（{}）",
                ProfileKind::VALUES.join(" / ")
            ),
            (Self::InvalidKind(got), En) => format!(
                "invalid kind: '{got}' ({})",
                ProfileKind::VALUES.join(" / ")
            ),
            (Self::ProfileNameEmpty, Ja) => "プロファイル名が空".to_string(),
            (Self::ProfileNameEmpty, En) => "the profile name is empty".to_string(),
            (Self::ProfileNameLeadingDot(name), Ja) => {
                format!("プロファイル名は '.' で始められない: '{name}'")
            }
            (Self::ProfileNameLeadingDot(name), En) => {
                format!("a profile name cannot start with '.': '{name}'")
            }
            (Self::ProfileNameLeadingDash(name), Ja) => format!(
                "プロファイル名は '-' で始められない（起動コマンドのオプションと紛れる）: '{name}'"
            ),
            (Self::ProfileNameLeadingDash(name), En) => format!(
                "a profile name cannot start with '-' (it would be read as an option \
                 of the launch command): '{name}'"
            ),
            (Self::ProfileNameBadChar(bad), Ja) => {
                format!("プロファイル名に使えない文字が含まれる: '{bad}'（英数字と - _ . のみ）")
            }
            (Self::ProfileNameBadChar(bad), En) => format!(
                "the profile name contains a character that cannot be used: '{bad}' \
                 (only letters, digits, - _ and .)"
            ),
            (Self::AccountNotFound(name), Ja) => {
                format!("アカウント '{name}' が accounts.yaml に見つからない")
            }
            (Self::AccountNotFound(name), En) => {
                format!("account '{name}' not found in accounts.yaml")
            }
            (Self::AccountInheritWithDir(name), Ja) => format!(
                "アカウント '{name}' は inherit: true と config_dir を同時に指定している\
                 （既定の資格情報を使うなら config_dir を消す）"
            ),
            (Self::AccountInheritWithDir(name), En) => format!(
                "account '{name}' sets both inherit: true and config_dir \
                 (remove config_dir to use the default credentials)"
            ),
            (Self::AccountNoConfigDir(name), Ja) => format!(
                "アカウント '{name}' に config_dir が無い\
                 （別 config dir なら config_dir を、既定の資格情報なら inherit: true を指定する）"
            ),
            (Self::AccountNoConfigDir(name), En) => format!(
                "account '{name}' has no config_dir (set config_dir for a separate config dir, \
                 or inherit: true for the default credentials)"
            ),
            (Self::ProjectNotFound(key), Ja) => {
                format!("プロジェクト '{key}' が projects.yaml に見つからない")
            }
            (Self::ProjectNotFound(key), En) => {
                format!("project '{key}' not found in projects.yaml")
            }
            (Self::ProjectCwdMissing(cwd), Ja) => format!("cwd が存在しない: {cwd}"),
            (Self::ProjectCwdMissing(cwd), En) => format!("the cwd does not exist: {cwd}"),
            (Self::UnknownAgent(got), Ja) => {
                format!("未対応のエージェント種別 '{got}'（対応: claude / codex / agy）")
            }
            (Self::UnknownAgent(got), En) => {
                format!("unsupported agent kind '{got}' (supported: claude / codex / agy)")
            }
            (Self::MasterAgentUnsupported(agent), Ja) => format!(
                "{} は master 非対応（master は claude / codex のみ。worker としては利用可能）",
                agent.as_str()
            ),
            (Self::MasterAgentUnsupported(agent), En) => format!(
                "{} cannot be the master (the master is claude / codex only; \
                 it can still be a worker)",
                agent.as_str()
            ),
            (Self::ProfileMasterAgent(inner), Ja) => {
                format!(
                    "プロファイルの master_agent が不正: {}",
                    inner.text_in(lang)
                )
            }
            (Self::ProfileMasterAgent(inner), En) => {
                format!(
                    "invalid master_agent in the profile: {}",
                    inner.text_in(lang)
                )
            }
            (Self::ProfileWorkerAgent(inner), Ja) => {
                format!(
                    "プロファイルの worker_agent が不正: {}",
                    inner.text_in(lang)
                )
            }
            (Self::ProfileWorkerAgent(inner), En) => {
                format!(
                    "invalid worker_agent in the profile: {}",
                    inner.text_in(lang)
                )
            }
            (Self::ProfileCwdMissing { expanded, raw }, Ja) => {
                format!("プロファイルの cwd が存在しない: {expanded}（元の指定: {raw}）")
            }
            (Self::ProfileCwdMissing { expanded, raw }, En) => {
                format!("the profile's cwd does not exist: {expanded} (as written: {raw})")
            }
            (Self::ProfileProjectsUnregistered(keys), Ja) => format!(
                "プロファイルの projects に projects.yaml に未登録のキーがある: {}",
                keys.join(", ")
            ),
            (Self::ProfileProjectsUnregistered(keys), En) => format!(
                "the profile's projects has keys that are not registered in projects.yaml: {}",
                keys.join(", ")
            ),
            (Self::ProfileEnvReserved(keys), Ja) => format!(
                "プロファイルの env に tako 内部変数が含まれている（上書き禁止）: {}",
                keys.join(", ")
            ),
            (Self::ProfileEnvReserved(keys), En) => format!(
                "the profile's env contains tako's internal variables \
                 (they cannot be overridden): {}",
                keys.join(", ")
            ),
            (Self::CtxThresholdOutOfRange(v), Ja) => format!(
                "ctx_threshold は {}〜{} の範囲で指定する: {v}",
                tako_core::handoff::CTX_THRESHOLD_MIN,
                tako_core::handoff::CTX_THRESHOLD_MAX
            ),
            (Self::CtxThresholdOutOfRange(v), En) => format!(
                "ctx_threshold must be between {} and {}: {v}",
                tako_core::handoff::CTX_THRESHOLD_MIN,
                tako_core::handoff::CTX_THRESHOLD_MAX
            ),
            (Self::LedgerEntryNotFound(id), Ja) => format!("エントリ '{id}' が見つからない"),
            // ` not found` を含める（`DispatchError::says_not_found` の当たり方を日英でそろえる）
            (Self::LedgerEntryNotFound(id), En) => format!("ledger entry '{id}' not found"),
            (Self::LedgerOutcomeInvalid(got), Ja) => {
                format!("不正な outcome '{got}'。使用可能: pass, rework, fail")
            }
            (Self::LedgerOutcomeInvalid(got), En) => {
                format!("invalid outcome '{got}'. Allowed: pass, rework, fail")
            }
            (Self::HandoffKeyUnusable(key), Ja) => {
                format!("プロジェクトキーがファイル名として使えない: {key}")
            }
            (Self::HandoffKeyUnusable(key), En) => {
                format!("the project key cannot be used as a file name: {key}")
            }
            (Self::HandoffDirCreate { dir, error }, Ja) => {
                format!("引き継ぎの置き場を作れない: {dir} ({error})")
            }
            (Self::HandoffDirCreate { dir, error }, En) => {
                format!("cannot create the handoff folder: {dir} ({error})")
            }
            (Self::ProfileUnreadable(why), _) => why.text_in(lang),
            (Self::ProjectProfileNotInherited { key, why }, Ja) => format!(
                "{}。default を継承できないので専用プロファイル '{key}' は作らなかった\
                 （組み込み既定で作ると個人ルール・モデルが抜けたまま残る）。\
                 default.yaml を直すと tako migrate run（GUI の次の起動でも自動）で作られる",
                why.text_in(lang)
            ),
            (Self::ProjectProfileNotInherited { key, why }, En) => format!(
                "{}. The dedicated profile '{key}' was not created because default cannot be \
                 inherited (creating it from the built-in defaults would leave it without the \
                 personal rules and model). Fix default.yaml and tako migrate run (or the next \
                 GUI launch, automatically) creates it",
                why.text_in(lang)
            ),
            (Self::ProjectProfileMissing { not_found, why }, Ja) => {
                format!("{}。{}", not_found.text_in(lang), why.text_in(lang))
            }
            (Self::ProjectProfileMissing { not_found, why }, En) => {
                format!("{}; {}", not_found.text_in(lang), why.text_in(lang))
            }
        }
    }

    /// 日本語の文（`String` を返す従来の口が使う。#2365 より前とバイト一致）
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

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

impl std::error::Error for ConfigError {}

impl From<LockError> for ConfigError {
    fn from(e: LockError) -> Self {
        Self::Lock(e)
    }
}

impl From<WriteError> for ConfigError {
    fn from(e: WriteError) -> Self {
        Self::Write(e)
    }
}

/// 読めないプロファイルの原因（#2256 の系統。`orchestrator::read_profile_base_at` が返す）
#[derive(Debug, Clone, PartialEq)]
pub enum UnreadableCause {
    /// 読み取りに失敗（OS のエラー文。言語に依らずそのまま載せる）
    Read(String),
    /// YAML として解釈できない（種類と位置だけ = #2230）
    Parse(YamlReason),
}

impl UnreadableCause {
    /// (形, 言語) の網羅 match。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::Read(e), Ja) => format!("読み取りに失敗: {e}"),
            (Self::Read(e), En) => format!("read failed: {e}"),
            (Self::Parse(reason), _) => reason.text_in(lang),
        }
    }
}

/// 読めない中身を退避したか（起動の経路 = `orchestrator::profile_base_at` だけが退避する）
#[derive(Debug, Clone, PartialEq)]
pub enum Quarantine {
    /// `<name>.unreadable[.<n>].bak` へ移した（退避先のパス）
    Moved(String),
    /// 退避できなかった（元のファイルには触っていない）
    Failed,
}

/// 読めないプロファイル（`<path> を読めない（<原因>）`）。#2256 の系統の理由の芯で、起動・継承元・
/// 表示・判定の経路が共有する。利用者へ見せる口（CLI / MCP / GUI）は表示言語で、persist.log は
/// 日本語で [`Self::text_in`] を引く
#[derive(Debug, Clone, PartialEq)]
pub struct UnreadableProfile {
    /// 読もうとしたファイル
    pub path: String,
    pub cause: UnreadableCause,
    /// 退避の結果（`None` = 読むだけの経路 = 退避の文を足さない）
    pub quarantine: Option<Quarantine>,
}

impl UnreadableProfile {
    /// (形, 言語) の網羅 match。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        let (path, cause) = (&self.path, self.cause.text_in(lang));
        match (&self.quarantine, lang) {
            (None, Ja) => format!("{path} を読めない（{cause}）"),
            (None, En) => format!("cannot read {path} ({cause})"),
            (Some(Quarantine::Moved(to)), Ja) => {
                format!("{path} を読めない（{cause}）。中身は {to} に退避した")
            }
            (Some(Quarantine::Moved(to)), En) => {
                format!("cannot read {path} ({cause}); its contents were moved to {to}")
            }
            (Some(Quarantine::Failed), Ja) => format!(
                "{path} を読めない（{cause}）。退避はできなかったが default.yaml そのものには触っていない"
            ),
            (Some(Quarantine::Failed), En) => format!(
                "cannot read {path} ({cause}); its contents could not be set aside, \
                 but the file itself was not touched"
            ),
        }
    }
}

/// 呼び出し元 master のプロファイルで立てるもの（[`ProfileFallback::CallerLaunch`]）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerLaunch {
    /// worker（`orchestrator spawn`）
    Worker,
    /// コンフリクト解消エージェント（`git resolve`）
    ConflictResolver,
}

impl CallerLaunch {
    pub fn text_in(self, lang: Lang) -> &'static str {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::Worker, Ja) => "worker",
            (Self::Worker, En) => "the worker",
            (Self::ConflictResolver, Ja) => "コンフリクト解消エージェント",
            (Self::ConflictResolver, En) => "the conflict-resolution agent",
        }
    }
}

/// 読めないプロファイルの代わりに組み込み既定で立てた / 答えた / 判断したことの知らせ
/// （#2227 / #2233 / #2256）。理由（[`UnreadableProfile`]）に、何を既定で代えたかと直すと
/// いつ効くかを足す。止めずに進む経路の `warnings`・CLI の警告の行・persist.log が同じ文を引く
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileFallback {
    /// 読めない default.yaml の代わりに組み込み既定で master / solo を立てる（#2227 / #2233）
    DefaultLaunch {
        why: UnreadableProfile,
        kind: ProfileKind,
    },
    /// 呼び出し元 master のプロファイル `name` の代わりに組み込み既定で `launching` を立てる（#2256）
    CallerLaunch {
        why: UnreadableProfile,
        name: String,
        launching: CallerLaunch,
    },
    /// 起動時のプロファイル `launched_as` の代わりに組み込み既定と比べて採用を判断した（#2256）
    Adopt {
        why: UnreadableProfile,
        launched_as: String,
    },
    /// 手順書を組み込み既定で組み立て、個人ルールの on-demand 部は読めていない（#2256）
    Guide(UnreadableProfile),
    /// 表示している値は組み込み既定（`orchestrator self` / `supervisor status` ほか。#2233）
    Display(UnreadableProfile),
    /// 引き継ぎの後任 master のプロファイル `name` を読めない（`why` = `tako master` と同じ読み込みの
    /// 断り）ので、止めずに組み込み既定で後任を立てる（#2233。表示言語は #2365）
    Successor { name: String, why: Box<ConfigError> },
}

impl ProfileFallback {
    /// (バリアント, 言語) の網羅 match。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::DefaultLaunch { why, kind }, Ja) => format!(
                "{}。default.yaml の個人ルール（prompt_blocks）・モデルなどの設定を使わず、\
                 組み込み既定で {} を立てる。default.yaml を直すと次の起動から効く",
                why.text_in(lang),
                kind.as_str()
            ),
            (Self::DefaultLaunch { why, kind }, En) => format!(
                "{}. Starting the {} on tako's built-in defaults, without the personal rules \
                 (prompt_blocks), model and other settings in default.yaml. Fix default.yaml \
                 and it takes effect from the next launch",
                why.text_in(lang),
                kind.as_str()
            ),
            (Self::CallerLaunch { why, name, launching }, Ja) => format!(
                "{}。呼び出し元 master のプロファイル '{name}' の設定（worker のモデル・effort・\
                 agent・アカウント・env・projects の制限）を使わず、組み込み既定で {} を\
                 立てる。直すと次から効く",
                why.text_in(lang),
                launching.text_in(lang)
            ),
            (Self::CallerLaunch { why, name, launching }, En) => format!(
                "{}. Starting {} on tako's built-in defaults, without the settings of the \
                 calling master's profile '{name}' (the worker's model, effort, agent, account, \
                 env and projects limit). Fix it and it takes effect from the next time",
                why.text_in(lang),
                launching.text_in(lang)
            ),
            (Self::Adopt { why, launched_as }, Ja) => format!(
                "{}。起動時のプロファイル '{launched_as}' の代わりに組み込み既定と比べて採用を判断した\
                 （system prompt・master の系統の食い違いの判断が実際と違いうる）。直すと次の採用から効く",
                why.text_in(lang)
            ),
            (Self::Adopt { why, launched_as }, En) => format!(
                "{}. The adoption was judged against tako's built-in defaults instead of the \
                 launch profile '{launched_as}' (the judgment of a system prompt or master agent \
                 mismatch may differ from reality). Fix it and it takes effect from the next adoption",
                why.text_in(lang)
            ),
            (Self::Guide(why), Ja) => format!(
                "{}。手順書の値（引き継ぎの閾値など）はこのファイルではなく組み込み既定で組み立てた。\
                 個人ルールの on-demand 部（local-rules）は読めていない。直すと次の呼び出しから効く",
                why.text_in(lang)
            ),
            (Self::Guide(why), En) => format!(
                "{}. The guide's values (such as the handoff threshold) were built from tako's \
                 built-in defaults, not from this file, and the on-demand part of the personal \
                 rules (local-rules) could not be read. Fix it and it takes effect from the next call",
                why.text_in(lang)
            ),
            (Self::Display(why), Ja) => format!(
                "{}。表示している値はこのファイルではなく組み込み既定。直すと次の呼び出しから効く",
                why.text_in(lang)
            ),
            (Self::Display(why), En) => format!(
                "{}. The values shown are tako's built-in defaults, not this file's. Fix it and \
                 it takes effect from the next call",
                why.text_in(lang)
            ),
            (Self::Successor { name, why }, Ja) => format!(
                "後任 master のプロファイル '{name}' を読めない（{}）。引き継ぎは止めずに、\
                 個人ルール（prompt_blocks）・モデル・アカウントなどの設定を使わない組み込み既定で\
                 後任を立てる。プロファイルを直すと次の起動から効く",
                why.text_in(lang)
            ),
            (Self::Successor { name, why }, En) => format!(
                "cannot read the successor master's profile '{name}' ({}). The handoff goes on: \
                 the successor starts on tako's built-in defaults, without the personal rules \
                 (prompt_blocks), model, account and other settings of that profile. Fix the \
                 profile and it takes effect from the next launch",
                why.text_in(lang)
            ),
        }
    }
}

impl std::fmt::Display for ProfileFallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}
