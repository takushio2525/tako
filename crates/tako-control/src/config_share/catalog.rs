//! 共有分類カタログ（Issue #513 要件 1・2 の「正本」）
//!
//! **何のためにあるか**: AI 系の記憶ファイルをデバイス間で git 共有するとき、
//! 秘匿情報とマシンローカル状態が**構造的に**混ざらないようにする。
//! フィルタ（除外リスト）ではなく**ホワイトリスト**で持ち、
//! カタログに載っていないものは共有されない（fail-closed）。
//!
//! ## 不変条件
//!
//! - **未分類は共有しない**。[`classify`] が `None` を返すファイルは push 対象に入らない。
//!   除外の書き忘れが漏えいになる構造を作らない（#513 要件 1）
//! - **分類し忘れはテストで落ちる**。tako がコード中で組み立てるデータディレクトリ配下の
//!   パスを走査し、カタログ未登録のものがあれば `catalog_coverage` テストが失敗する
//!   （`platform::support::MATRIX` と同じ考え方。#515）
//! - **理由は日英で 1 箇所に持つ**。UI・CLI・`tako config list` がすべてここから引く（#435）
//!
//! ## クラス分け
//!
//! | クラス | 意味 | 例 |
//! |---|---|---|
//! | [`Class::Shared`] | 宣言的な設定。git で共有する | projects.yaml / CLAUDE.md |
//! | [`Class::Local`] | このデバイス固有の状態。共有しない | layout.json / sessions.yaml |
//! | [`Class::Secret`] | 秘匿情報。**絶対に**リポジトリへ入れない | token / .claude.json |

use tako_core::platform::support::Note;

/// 共有ルート = 相対パスの基準ディレクトリ。
/// リポジトリ内のサブディレクトリ名も兼ねる（`tako/` と `claude/`）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Root {
    /// tako のデータディレクトリ（`<data_dir>`。orchestrator 配下も含む）
    TakoData,
    /// claude のホーム（`~/.claude`）
    Claude,
}

impl Root {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TakoData => "tako",
            Self::Claude => "claude",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tako" => Some(Self::TakoData),
            "claude" => Some(Self::Claude),
            _ => None,
        }
    }

    pub fn all() -> &'static [Root] {
        &[Self::TakoData, Self::Claude]
    }

    /// このルートの実体パス。解決できない（$HOME 無し等）なら None
    pub fn live_dir(self) -> Option<std::path::PathBuf> {
        match self {
            Self::TakoData => tako_core::paths::data_dir(),
            Self::Claude => claude_home(),
        }
    }
}

/// claude のホームディレクトリ。`CLAUDE_CONFIG_DIR` があればそれを優先する
/// （アカウント切替中のシェルから叩いても、そのアカウントの設定が対象になる。#512）
pub fn claude_home() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return Some(std::path::PathBuf::from(dir));
        }
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| std::path::PathBuf::from(h).join(".claude"))
}

/// 共有可否の分類
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// 宣言的な設定。git リポジトリへ書き出す
    Shared,
    /// このデバイス固有の状態。共有しない
    Local,
    /// 秘匿情報。共有しない（`Local` と分けるのは、取り違えを目で見て検出できるようにするため）
    Secret,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Local => "local",
            Self::Secret => "secret",
        }
    }

    pub fn is_shared(self) -> bool {
        matches!(self, Self::Shared)
    }
}

/// カタログの 1 エントリ
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub root: Root,
    /// ルートからの相対パス。
    /// - 末尾が `/` ならそのディレクトリ配下すべてが対象
    /// - 末尾が `*` ならファイル名の**前方一致**（`_system_prompt_*` のように
    ///   名前が実行時に決まるファイル。#792）
    pub path: &'static str,
    pub class: Class,
    /// 分類の理由（日英）。`tako config list` と診断がここから引く
    pub note: Note,
    /// 共有コピーから**取り除く**フィールド（#513 要件 1・2）。
    /// 秘匿（profile の env）か、デバイス固有（accounts の config_dir）のもの。
    /// pull ではローカルの値を残す = 共有側が上書きしない。
    /// 構文は `a.b` / `a.*.b`（`*` は map の全キー）
    pub local_fields: &'static [&'static str],
    /// 「取り込み後にこのデバイスで埋めてください」の報告を**免除**する兄弟キー。
    /// 真なら、同じ階層の `local_fields` は無くて当然とみなす
    /// （例: `inherit: true` のアカウントは `config_dir` を持たないのが正しい姿。#512）
    pub needs_local_unless: &'static [&'static str],
}

impl Entry {
    /// ディレクトリ配下すべてを指すエントリか
    pub fn is_dir(&self) -> bool {
        self.path.ends_with('/')
    }

    /// ファイル名の前方一致で効くエントリか（`_system_prompt_*`。#792）
    pub fn is_prefix(&self) -> bool {
        self.path.ends_with('*')
    }

    /// 前方一致に使う部分（末尾の `*` を落としたもの）
    fn match_prefix(&self) -> &'static str {
        self.path.trim_end_matches('*')
    }

    /// リポジトリ内のパス（`tako/orchestrator/projects.yaml` 等）
    pub fn repo_path(&self, rel: &str) -> String {
        format!("{}/{}", self.root.as_str(), rel)
    }
}

/// 分類の理由文。同じ理由を複数エントリで共有するので定数に集約する
pub mod notes {
    use tako_core::platform::support::Note;

    pub const DECLARATIVE: Note = Note::new(
        "エージェント運用の宣言的な設定。デバイス間で同じにしたいもの",
        "Declarative agent configuration that should be identical across devices",
    );
    pub const RULES: Note = Note::new(
        "AI へ渡す指示・ルールの本文。デバイス間で同じにしたいもの",
        "Instruction and rule text handed to the AI; should be identical across devices",
    );
    pub const RUNTIME: Note = Note::new(
        "実行時の状態。デバイスごとに違って当然なので共有しない",
        "Runtime state that legitimately differs per device, so it is not shared",
    );
    /// 器のオーナー記録（#519 M2）。**共有すると #177 の復元強奪ガードが誤作動する**:
    /// 別マシンの pid を「この器の持ち主」と読んでしまい、生きている器を
    /// 奪う / 逆に自分の器を諦める、のどちらにも倒れうる
    pub const BACKEND_OWNER: Note = Note::new(
        "器の所有インスタンス（pid）の記録。マシンをまたぐと持ち主の判定が壊れるので共有しない",
        "Records which local instance (pid) owns each session container; sharing it across \
         machines would break ownership detection, so it is not shared",
    );
    pub const DIAGNOSTIC: Note = Note::new(
        "診断ログ。デバイス固有かつ肥大化するので共有しない",
        "Diagnostic logs: device-specific and ever-growing, so they are not shared",
    );
    pub const GENERATED: Note = Note::new(
        "tako が起動時に生成し直すファイル。共有する意味がない",
        "Regenerated by tako at startup, so there is nothing to share",
    );
    pub const MACHINE: Note = Note::new(
        "このマシン固有の識別子・配置。共有すると別デバイスを壊す",
        "Machine-specific identifiers or locations; sharing them would break the other device",
    );
    pub const SECRET: Note = Note::new(
        "秘匿情報。共有リポジトリへ入れてはならない",
        "Secret material that must never enter the shared repository",
    );
    pub const CREDENTIAL_LOCATION: Note = Note::new(
        "資格情報の保管場所。中身も場所もデバイス固有なので共有しない",
        "Credential storage: both contents and location are device-specific, so they are not shared",
    );
    pub const SESSION: Note = Note::new(
        "会話・セッションの記録。分量が大きく秘匿情報を含みうるので共有しない",
        "Conversation and session records: large and potentially sensitive, so they are not shared",
    );
    pub const BACKUP: Note = Note::new(
        "バックアップ・ロック・一時ファイル。共有対象の元ファイルだけを扱う",
        "Backup, lock, and temporary files; only the original file is handled",
    );
}

/// 常にローカル扱いにするファイル名サフィックス（バックアップ・ロック・一時ファイル）。
/// カタログの前段でここを見るので、`projects.yaml.bak.1` が
/// `projects.yaml`（Shared）に引きずられて共有されることはない
const ALWAYS_LOCAL_SUFFIXES: &[&str] = &[".lock", ".bak", ".sock", ".old", ".corrupt"];

/// 常にローカル扱いにする「含む」パターン（世代バックアップ・一時ファイル）
const ALWAYS_LOCAL_CONTAINS: &[&str] = &[".bak.", ".bak-", ".tmp.", ".wiped-", ".recovery-"];

/// 共有分類カタログ（正本）。
///
/// 並び順は「ルート → 分類の見やすさ」。エントリを増やしたら
/// `tako config list` にそのまま出るので、追加時は note も必ず書くこと
pub const CATALOG: &[Entry] = &[
    // ---------------- tako データディレクトリ: 共有する ----------------
    Entry {
        root: Root::TakoData,
        path: "settings.json",
        class: Class::Shared,
        note: notes::DECLARATIVE,
        // 「バナーを閉じた」「この版の更新通知を閉じた」はそのデバイスの操作履歴。
        // 共有すると別デバイスで初回案内が出なくなる（#513 要件 2）
        local_fields: &["welcome_dismissed", "update_card_dismissed"],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/config.yaml",
        class: Class::Shared,
        note: notes::DECLARATIVE,
        // setup をいつ・どの版で走らせたかはデバイスごとの記録。
        // 共有すると push のたびに無意味な差分が出る
        local_fields: &["setup.completed_at", "setup.applied_version"],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/projects.yaml",
        class: Class::Shared,
        note: notes::DECLARATIVE,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/accounts.yaml",
        class: Class::Shared,
        note: notes::DECLARATIVE,
        // アカウントの**宣言部**（名前・説明・既定モデル）は共有し、
        // 資格情報の在り処（config_dir）は共有しない（#513 要件 1）
        local_fields: &["accounts.*.config_dir"],
        needs_local_unless: &["inherit"],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/profiles/",
        class: Class::Shared,
        note: notes::DECLARATIVE,
        // env は API キー等が入りうる。値は共有せずローカルのものを残す（#500）
        local_fields: &["env"],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/solo-profiles/",
        class: Class::Shared,
        note: notes::DECLARATIVE,
        local_fields: &["env"],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/local-rules.md",
        class: Class::Shared,
        note: notes::RULES,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/master-system.md",
        class: Class::Shared,
        note: notes::RULES,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/conflict-resolver.md",
        class: Class::Shared,
        note: notes::RULES,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/judgment-local.md",
        class: Class::Shared,
        note: notes::RULES,
        local_fields: &[],
        needs_local_unless: &[],
    },
    // ---------------- tako データディレクトリ: 共有しない ----------------
    Entry {
        root: Root::TakoData,
        path: "token",
        class: Class::Secret,
        note: notes::SECRET,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "control.json",
        class: Class::Secret,
        note: notes::SECRET,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "relay_secret",
        class: Class::Secret,
        note: notes::SECRET,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "machine_id",
        class: Class::Secret,
        note: notes::MACHINE,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "remote/",
        class: Class::Secret,
        note: notes::SECRET,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "instances/",
        class: Class::Local,
        note: notes::RUNTIME,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "tako.sock",
        class: Class::Local,
        note: notes::RUNTIME,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "layout.json",
        class: Class::Local,
        note: notes::RUNTIME,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "layout.json.good",
        class: Class::Local,
        note: notes::RUNTIME,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "sessions.yaml",
        class: Class::Local,
        note: notes::SESSION,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "backend-owners",
        class: Class::Local,
        note: notes::BACKEND_OWNER,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "workers.yaml",
        class: Class::Local,
        note: notes::RUNTIME,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "recent.json",
        class: Class::Local,
        note: notes::RUNTIME,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "acceptance_gates.yaml",
        class: Class::Local,
        note: notes::RUNTIME,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "task_checkpoints.yaml",
        class: Class::Local,
        note: notes::RUNTIME,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "config-share.json",
        class: Class::Local,
        note: notes::MACHINE,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "tmux-backend.conf",
        class: Class::Local,
        note: notes::GENERATED,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "shell-integration/",
        class: Class::Local,
        note: notes::GENERATED,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "setup/",
        class: Class::Local,
        note: notes::GENERATED,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "pane-logs/",
        class: Class::Local,
        note: notes::SESSION,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "perf.log",
        class: Class::Local,
        note: notes::DIAGNOSTIC,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "persist.log",
        class: Class::Local,
        note: notes::DIAGNOSTIC,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "panic.log",
        class: Class::Local,
        note: notes::DIAGNOSTIC,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "telemetry.log",
        class: Class::Local,
        note: notes::DIAGNOSTIC,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "telemetry_queue.jsonl",
        class: Class::Local,
        note: notes::DIAGNOSTIC,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "supervisor.log",
        class: Class::Local,
        note: notes::DIAGNOSTIC,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/ledger.yaml",
        class: Class::Local,
        note: notes::RUNTIME,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/review-ledger.yaml",
        class: Class::Local,
        note: notes::RUNTIME,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::TakoData,
        path: "orchestrator/handoff/",
        class: Class::Local,
        note: notes::RUNTIME,
        local_fields: &[],
        needs_local_unless: &[],
    },
    // master / solo の起動ごとに書き出す system prompt の実体（`_system_prompt_<profile>.md`）。
    // プロファイル名で名前が決まるので**前方一致**で分類する（#792）。
    // 正本はプロファイル + 埋め込みテンプレートなので、共有する意味がない
    Entry {
        root: Root::TakoData,
        path: "orchestrator/_system_prompt_*",
        class: Class::Local,
        note: notes::GENERATED,
        local_fields: &[],
        needs_local_unless: &[],
    },
    // ---------------- claude: 共有する ----------------
    Entry {
        root: Root::Claude,
        path: "CLAUDE.md",
        class: Class::Shared,
        note: notes::RULES,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "snippets/",
        class: Class::Shared,
        note: notes::RULES,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "commands/",
        class: Class::Shared,
        note: notes::RULES,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "templates/",
        class: Class::Shared,
        note: notes::RULES,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "agents/",
        class: Class::Shared,
        note: notes::RULES,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "output-styles/",
        class: Class::Shared,
        note: notes::RULES,
        local_fields: &[],
        needs_local_unless: &[],
    },
    // ---------------- claude: 共有しない ----------------
    Entry {
        root: Root::Claude,
        path: ".claude.json",
        class: Class::Secret,
        note: notes::SECRET,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: ".credentials.json",
        class: Class::Secret,
        note: notes::SECRET,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "credentials.json",
        class: Class::Secret,
        note: notes::SECRET,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "settings.json",
        class: Class::Local,
        note: notes::MACHINE,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "settings.local.json",
        class: Class::Local,
        note: notes::MACHINE,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "hooks/",
        class: Class::Local,
        note: notes::MACHINE,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "scripts/",
        class: Class::Local,
        note: notes::MACHINE,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "statusline.sh",
        class: Class::Local,
        note: notes::MACHINE,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "projects/",
        class: Class::Local,
        note: notes::SESSION,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "sessions/",
        class: Class::Local,
        note: notes::SESSION,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "history.jsonl",
        class: Class::Local,
        note: notes::SESSION,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "todos/",
        class: Class::Local,
        note: notes::SESSION,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "tasks/",
        class: Class::Local,
        note: notes::SESSION,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "file-history/",
        class: Class::Local,
        note: notes::SESSION,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "paste-cache/",
        class: Class::Local,
        note: notes::SESSION,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "session-env/",
        class: Class::Local,
        note: notes::SESSION,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "shell-snapshots/",
        class: Class::Local,
        note: notes::GENERATED,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "cache/",
        class: Class::Local,
        note: notes::GENERATED,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "backups/",
        class: Class::Local,
        note: notes::BACKUP,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "archive/",
        class: Class::Local,
        note: notes::BACKUP,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "plugins/",
        class: Class::Local,
        note: notes::MACHINE,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "ide/",
        class: Class::Local,
        note: notes::MACHINE,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "chrome/",
        class: Class::Local,
        note: notes::MACHINE,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "statsig/",
        class: Class::Local,
        note: notes::GENERATED,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "telemetry/",
        class: Class::Local,
        note: notes::DIAGNOSTIC,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "self-improve-insights/",
        class: Class::Local,
        note: notes::SESSION,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "stats-cache.json",
        class: Class::Local,
        note: notes::GENERATED,
        local_fields: &[],
        needs_local_unless: &[],
    },
    Entry {
        root: Root::Claude,
        path: "mcp-needs-auth-cache.json",
        class: Class::Local,
        note: notes::CREDENTIAL_LOCATION,
        local_fields: &[],
        needs_local_unless: &[],
    },
];

/// パスを分類する。**未登録は `None`（= 共有しない）**。
///
/// `rel` はルートからの相対パスで、区切りは `/`。
/// 判定順は「常時ローカル扱いのサフィックス → 完全一致 → ディレクトリ前方一致（最長優先）」。
pub fn classify(root: Root, rel: &str) -> Option<&'static Entry> {
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() {
        return None;
    }
    // `.lock` / `.bak.1` / `.tmp.1234` のような派生ファイルは、
    // 元ファイルが Shared でも決して共有しない
    if is_always_local(rel) {
        return Some(&ALWAYS_LOCAL_ENTRY);
    }
    let mut best: Option<&'static Entry> = None;
    for entry in CATALOG {
        if entry.root != root {
            continue;
        }
        if entry.is_dir() {
            if rel.starts_with(entry.path)
                && best.is_none_or(|b| b.path.len() < entry.path.len())
                && rel.len() > entry.path.len()
            {
                best = Some(entry);
            }
        } else if entry.is_prefix() {
            // 名前が実行時に決まるファイル。**同じディレクトリの中だけ**に効かせる
            // （`_system_prompt_` がディレクトリ名だった場合に配下を巻き込まない）
            let prefix = entry.match_prefix();
            let matches = rel
                .strip_prefix(prefix)
                .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'));
            if matches && best.is_none_or(|b| b.path.len() < entry.path.len()) {
                best = Some(entry);
            }
        } else if rel == entry.path {
            // 完全一致は常に最優先
            return Some(entry);
        }
    }
    best
}

/// バックアップ・ロック・一時ファイルか
pub fn is_always_local(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if name.starts_with('.') && (name == ".DS_Store" || name == ".gitignore") {
        return true;
    }
    if ALWAYS_LOCAL_SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return true;
    }
    ALWAYS_LOCAL_CONTAINS.iter().any(|s| name.contains(s))
}

/// 派生ファイル（バックアップ・ロック）用の合成エントリ。
/// カタログに 1 行ずつ書く代わりに、判定側で共通のローカル分類を返す
static ALWAYS_LOCAL_ENTRY: Entry = Entry {
    root: Root::TakoData,
    path: "*.bak / *.lock / *.tmp",
    class: Class::Local,
    note: notes::BACKUP,
    local_fields: &[],
    needs_local_unless: &[],
};

/// 共有対象のエントリだけを列挙する
pub fn shared_entries(root: Root) -> impl Iterator<Item = &'static Entry> {
    CATALOG
        .iter()
        .filter(move |e| e.root == root && e.class.is_shared())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 未登録のパスは共有されない() {
        // fail-closed: 知らないファイルが勝手に共有対象へ入らない
        assert!(classify(Root::TakoData, "totally-unknown.yaml").is_none());
        assert!(classify(Root::Claude, "some-new-thing/x.md").is_none());
    }

    #[test]
    fn 完全一致がディレクトリ前方一致より優先される() {
        // orchestrator/ 配下の Shared ディレクトリと、その中の個別ファイル分類の共存
        let e = classify(Root::TakoData, "orchestrator/projects.yaml").unwrap();
        assert_eq!(e.class, Class::Shared);
        let e = classify(Root::TakoData, "orchestrator/handoff/master.md").unwrap();
        assert_eq!(e.class, Class::Local);
    }

    #[test]
    fn バックアップとロックは元がsharedでも共有されない() {
        for name in [
            "orchestrator/projects.yaml.bak.1",
            "orchestrator/projects.yaml.lock",
            "orchestrator/projects.yaml.tmp.1234",
            "orchestrator/accounts.yaml.bak-511-512",
            "settings.json.bak",
            "orchestrator/projects.yaml.wiped-141102.bak",
        ] {
            let e = classify(Root::TakoData, name)
                .unwrap_or_else(|| panic!("{name} が分類されていない"));
            assert_eq!(e.class, Class::Local, "{name} が共有対象になっている");
        }
    }

    #[test]
    fn 秘匿ファイルは必ずsecret分類() {
        for (root, path) in [
            (Root::TakoData, "token"),
            (Root::TakoData, "control.json"),
            (Root::TakoData, "relay_secret"),
            (Root::TakoData, "remote/state.json"),
            (Root::Claude, ".claude.json"),
            (Root::Claude, ".credentials.json"),
        ] {
            let e = classify(root, path).unwrap_or_else(|| panic!("{path} が分類されていない"));
            assert!(
                !e.class.is_shared(),
                "{path} が共有対象になっている（クラス {}）",
                e.class.as_str()
            );
        }
    }

    #[test]
    fn ディレクトリ前方一致は最長が勝つ() {
        // profiles/ (Shared) と handoff/ (Local) が兄弟でも取り違えない
        assert_eq!(
            classify(Root::TakoData, "orchestrator/profiles/default.yaml")
                .unwrap()
                .class,
            Class::Shared
        );
        assert_eq!(
            classify(Root::TakoData, "orchestrator/handoff/x.md")
                .unwrap()
                .class,
            Class::Local
        );
    }

    /// #792: 名前が実行時に決まるファイル（`_system_prompt_<profile>.md`）が
    /// 前方一致エントリで分類される
    #[test]
    fn 前方一致エントリが動的名のファイルを分類する() {
        for name in [
            "orchestrator/_system_prompt_default.md",
            "orchestrator/_system_prompt_takodev.md",
            // 走査が畳んだ形（`{profile}` → `*`）もそのまま引ける
            "orchestrator/_system_prompt_*.md",
        ] {
            let e = classify(Root::TakoData, name)
                .unwrap_or_else(|| panic!("{name} が分類されていない"));
            assert_eq!(e.class, Class::Local, "{name}");
            assert_eq!(e.path, "orchestrator/_system_prompt_*");
        }
        // 前方一致は兄弟ファイルを巻き込まない
        assert_eq!(
            classify(Root::TakoData, "orchestrator/projects.yaml")
                .unwrap()
                .class,
            Class::Shared
        );
        // 接頭辞そのものだけ（残りが空）は一致しない
        assert!(classify(Root::TakoData, "orchestrator/_system_prompt_").is_none());
        // 別ディレクトリ配下へは効かない
        assert!(classify(Root::TakoData, "orchestrator/_system_prompt_x/inner.md").is_none());
    }

    #[test]
    fn ディレクトリ自身はエントリに一致しない() {
        // `orchestrator/profiles/` そのもの（末尾スラッシュだけ）は対象にしない
        assert!(classify(Root::TakoData, "orchestrator/profiles/").is_none());
    }

    #[test]
    fn 全エントリに日英の理由がある() {
        for e in CATALOG {
            assert!(!e.note.ja().is_empty(), "{} の日本語理由が空", e.path);
            assert!(!e.note.en().is_empty(), "{} の英語理由が空", e.path);
            assert!(
                !e.note.ja().is_ascii(),
                "{} の日本語理由が英語のまま: {}",
                e.path,
                e.note.ja()
            );
        }
    }

    #[test]
    fn カタログにパスの重複がない() {
        let mut seen = std::collections::BTreeSet::new();
        for e in CATALOG {
            assert!(
                seen.insert((e.root, e.path)),
                "カタログに重複エントリ: {:?} {}",
                e.root,
                e.path
            );
        }
    }

    #[test]
    fn local_fieldsはshared分類にだけ付く() {
        for e in CATALOG {
            if !e.local_fields.is_empty() {
                assert!(
                    e.class.is_shared(),
                    "{} は共有しないのに local_fields がある（無意味）",
                    e.path
                );
            }
        }
    }
}
