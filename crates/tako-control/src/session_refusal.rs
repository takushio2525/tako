//! 会話のカタログ（sessions.yaml）・会話の再開（`resume_launch`）・端末ログの読み取りの断り（#2365）。
//!
//! dispatch の `sessions list / show / resume / link` と `logs read` は、以前はこれらの口の
//! `Err(String)`（日本語の固定文）を `Operation` の中身としてそのまま載せていたので、英語表示でも
//! CLI / MCP の応答の理由だけ日本語で返った。日英の対は [`SessionRefusal::text_in`] の
//! (バリアント, 言語) の網羅 match の 1 か所に置く（形は [`crate::orchestrator::ConfigError`] と同じ）。
//! 日本語は #2365 より前の固定文とバイト一致（`tests/issue1762_dispatch_error_lang.rs` が突き合わせる）。
//!
//! # 口は 2 つ
//!
//! 従来の口（`SessionCatalog::load` / `resolve_id` / `resume_launch` / `ResumeLaunch::materialize` ほか）は
//! `String` を返し、文は**日本語のまま**（呼び手の多くは persist.log・復元の内訳・`tako orchestrator resume` の
//! 日本語の表示へ載せる）。断りを表示言語で返す dispatch だけが型の口（`*_typed`）を通し、枠と中身を
//! 同じ言語にそろえる。「見つからない」系は英語にも ` not found` を含め、
//! [`crate::DispatchError::says_not_found`] の当たり方を日英でそろえる（リモートの HTTP 層が 404 へ寄せる判定）

use tako_core::i18n::Lang;
use tako_core::load_issue::YamlReason;

use crate::config_io::WriteError;
use crate::orchestrator::ConfigError;
use crate::{DispatchError, Reason};

/// 会話のカタログ・再開・端末ログの断り。`error` は OS / serde のエラー文（言語に依らずそのまま載せる）、
/// ID・パス・名前は表記のまま埋める
#[derive(Debug, Clone, PartialEq)]
pub enum SessionRefusal {
    // ---- カタログ（sessions.yaml） ----
    /// カタログの置き場を決められない（日本語は #2365 より前の文 = ホームディレクトリと言う）
    CatalogHomeUnresolved,
    /// sessions.yaml を読めない
    CatalogRead(String),
    /// sessions.yaml を YAML として解釈できない
    CatalogParse(YamlReason),
    /// 前方一致する会話がカタログに無い（`prefix` は指定のまま）
    SessionNotFound(String),
    /// 前方一致する会話が複数ある（`candidates` は短縮 ID）
    SessionAmbiguous {
        prefix: String,
        count: usize,
        candidates: Vec<String>,
    },

    // ---- 再開の組み立て（resume_launch / sessions::resume_command） ----
    /// 会話 ID の形が不正（パストラバーサル対策で弾いた）
    InvalidSessionId,
    /// その系統の会話は resume の手段を持たない（`agent` は系統名）
    ResumeUnsupported(String),
    /// claude の会話の記録（transcript）が見つからない（`id` は短縮 ID）
    TranscriptMissing(String),
    /// プロファイル・エージェント名・アカウントの断り（中身の文は `ConfigError` の表）
    Config(ConfigError),
    /// プロファイルの master が claude ではないので、claude の会話をその組み立てで再開できない
    MasterAgentNotClaude { profile: String, agent: String },
    /// system prompt の保存先を作れない（文は `Reason::SystemPromptDirFailed` と同じ）
    PromptDir(String),
    /// system prompt を書き出せない（文は `Reason::SystemPromptWriteFailed` と同じ）
    PromptWrite(String),
    /// worker の MCP の設定ファイルの置き場を作れない（`dir` はそのフォルダ）
    McpSettingsDir { dir: String, error: String },
    /// worker の MCP の設定ファイルを書き出せない
    McpSettingsWrite(WriteError),

    // ---- 端末ログ ----
    /// 端末ログを読めない（`path` はログファイル）
    LogUnreadable { path: String, error: String },
}

impl SessionRefusal {
    /// 表示言語を明示しての文言。(バリアント, 言語) の網羅 match なので、バリアントを足すと
    /// 日英の両方を書くまでコンパイルが通らない。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::CatalogHomeUnresolved, Ja) => "ホームディレクトリが取得できない".to_string(),
            (Self::CatalogHomeUnresolved, En) => "cannot determine the home directory".to_string(),
            (Self::CatalogRead(error), Ja) => format!("sessions.yaml の読み取りに失敗: {error}"),
            (Self::CatalogRead(error), En) => format!("failed to read sessions.yaml: {error}"),
            (Self::CatalogParse(reason), Ja) => {
                format!("sessions.yaml のパースに失敗: {}", reason.text_in(lang))
            }
            (Self::CatalogParse(reason), En) => {
                format!("failed to parse sessions.yaml: {}", reason.text_in(lang))
            }
            (Self::SessionNotFound(prefix), Ja) => format!(
                "セッション '{prefix}' がカタログに見つからない（tako sessions list で確認）"
            ),
            (Self::SessionNotFound(prefix), En) => format!(
                "session '{prefix}' not found in the catalog (check with tako sessions list)"
            ),
            (
                Self::SessionAmbiguous {
                    prefix,
                    count,
                    candidates,
                },
                Ja,
            ) => format!(
                "セッション '{prefix}' の候補が {count} 件ある（もう少し長い ID を指定）: {}",
                candidates.join(", ")
            ),
            (
                Self::SessionAmbiguous {
                    prefix,
                    count,
                    candidates,
                },
                En,
            ) => format!(
                "session '{prefix}' matches {count} entries (give a longer ID): {}",
                candidates.join(", ")
            ),
            (Self::InvalidSessionId, Ja) => "session_id の形式が不正".to_string(),
            (Self::InvalidSessionId, En) => "the session_id is malformed".to_string(),
            (Self::ResumeUnsupported(agent), Ja) => format!(
                "agent '{agent}' のセッションは resume 非対応（対応: claude / codex / agy）"
            ),
            (Self::ResumeUnsupported(agent), En) => format!(
                "sessions of agent '{agent}' cannot be resumed (supported: claude / codex / agy)"
            ),
            (Self::TranscriptMissing(id), Ja) => format!(
                "会話 {id} の記録（claude の transcript）が見つからない。\
                 削除されたか、登録されていない設定 dir にある（`tako orchestrator accounts list`）"
            ),
            (Self::TranscriptMissing(id), En) => format!(
                "the record of conversation {id} (claude's transcript) not found; \
                 it was deleted or is in a config dir that is not registered \
                 (`tako orchestrator accounts list`)"
            ),
            (Self::Config(e), _) => e.text_in(lang),
            (Self::MasterAgentNotClaude { profile, agent }, Ja) => format!(
                "プロファイル '{profile}' のエージェントは {agent} なので claude の会話を\
                 その組み立てで再開できない（`tako sessions resume <id>` で開く）"
            ),
            (Self::MasterAgentNotClaude { profile, agent }, En) => format!(
                "profile '{profile}' launches {agent}, so a claude conversation cannot be resumed \
                 with its launch (open it with `tako sessions resume <id>`)"
            ),
            (Self::PromptDir(error), _) => {
                Reason::SystemPromptDirFailed(error.clone()).text_in(lang)
            }
            (Self::PromptWrite(error), _) => {
                Reason::SystemPromptWriteFailed(error.clone()).text_in(lang)
            }
            (Self::McpSettingsDir { dir, error }, Ja) => format!("{dir} を作れない: {error}"),
            (Self::McpSettingsDir { dir, error }, En) => format!("cannot create {dir}: {error}"),
            (Self::McpSettingsWrite(e), _) => e.text_in(lang),
            (Self::LogUnreadable { path, error }, Ja) => {
                format!("ログを読めない ({path}): {error}")
            }
            (Self::LogUnreadable { path, error }, En) => {
                format!("cannot read the log ({path}): {error}")
            }
        }
    }

    /// 従来の `String` の口が返す文（日本語）
    pub fn into_ja(self) -> String {
        self.text_in(Lang::Ja)
    }

    /// 前置きの無い理由の文として載せる（表示言語で組む）
    pub fn operation(self) -> DispatchError {
        DispatchError::Operation(self.to_string())
    }
}

impl std::fmt::Display for SessionRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

impl std::error::Error for SessionRefusal {}

impl From<ConfigError> for SessionRefusal {
    fn from(e: ConfigError) -> Self {
        Self::Config(e)
    }
}
