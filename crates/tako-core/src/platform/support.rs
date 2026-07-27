//! プラットフォーム対応マトリクス（設計 §3）
//!
//! **何のためにあるか**: tako は macOS で先行開発し、安定した差分を Windows へ一括反映する。
//! そのとき「どれがまだ Windows に反映されていないか」を人間の記憶ではなく
//! **テストとコマンドで**押さえるための表がこれ。
//!
//! 設計の正: `.agent/plans/2026-07-windows-port-architecture.md`
//!
//! ## 不変条件
//!
//! - キーは **MCP ツール名と 1:1**。tako の開発不変条件「新機能は必ず MCP / CLI から
//!   操作できる」により、新機能は必ず MCP ツールを増やす。したがってツール表と
//!   突き合わせれば**分類し忘れは必ず検出できる**（パリティテスト T1）
//! - 判定は**純粋関数**。`Platform` を引数で受けるので、**macOS 上でも Windows 側の
//!   縮退表を検証できる**。これが無いと「Windows でどう見えるか」をテストできない
//! - 使えない機能を一覧から消してはいけない。消すと AI は「そんな機能は無い」と誤認し、
//!   回避行動も取れなくなる。**存在させたうえで理由と追跡先を返す**

/// 対応マトリクスが対象とするプラットフォーム
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    MacOs,
    Windows,
}

impl Platform {
    /// 実行中のプラットフォーム。マトリクス未対応の OS は macOS 側の表を使う
    /// （tako が対象とするのは macOS と Windows のみ）
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::MacOs
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::Windows => "windows",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "macos" | "mac" | "darwin" => Some(Self::MacOs),
            "windows" | "win" => Some(Self::Windows),
            _ => None,
        }
    }
}

/// 表示言語に追従する文言。
///
/// マトリクスの理由文を `&'static str` の直書きにすると、英語 UI に日本語が出てしまう。
/// 日英を対で持ち `i18n::lang()` で解決する（`tako-app` の `tr!` と同じ機構。
/// あちらのマクロは `tako-app` 内スコープなので `tako-core` からは使えない）。
///
/// **縮退の理由はここ 1 箇所で定義し、UI・エラーメッセージ・system prompt が
/// すべてここから引く**（設計 §3・§4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
    ja: &'static str,
    en: &'static str,
}

impl Note {
    pub const fn new(ja: &'static str, en: &'static str) -> Self {
        Self { ja, en }
    }

    /// 現在の表示言語での文言
    pub fn text(self) -> &'static str {
        self.text_in(crate::i18n::lang())
    }

    /// 言語を明示しての文言。**言語グローバルに触らず解決できる**ようにするため、
    /// 実体はこちらの純粋関数に置く（文言を早期に解決して凍結させないこと）
    pub fn text_in(self, lang: crate::i18n::Lang) -> &'static str {
        match lang {
            crate::i18n::Lang::Ja => self.ja,
            crate::i18n::Lang::En => self.en,
        }
    }

    pub fn ja(self) -> &'static str {
        self.ja
    }

    pub fn en(self) -> &'static str {
        self.en
    }
}

/// 縮退の理由。**同じ理由を複数の機能で共有するので定数に集約する**
/// （文言を直したいときに 1 箇所で済む）
pub mod notes {
    use super::Note;

    /// #517 の担当範囲
    pub const WIN_TERMINAL: Note = Note::new(
        "GUI 起動とペイン / タブ管理の Windows 実装が前提",
        "Requires the Windows implementation of GUI startup and pane / tab management",
    );
    /// #519 の担当範囲
    pub const WIN_PERSIST: Note = Note::new(
        "tmux バックエンドに依存。Windows の永続化戦略の決定が前提",
        "Depends on the tmux backend; requires deciding the Windows persistence strategy",
    );
    /// #520 の担当範囲
    pub const WIN_GIT: Note = Note::new(
        "git タブの Windows 対応（パス表記と改行コードの可搬性）が前提",
        "Requires Windows support for the git tab (path notation and line-ending portability)",
    );
    /// #520 + #526 の担当範囲（#496 のコンフリクト解消エージェント）。
    /// git タブの状態検出だけでなく**エージェントペインの spawn** にも依存するので、
    /// WIN_GIT とは別に「両方が要る」ことを明示する
    pub const WIN_GIT_RESOLVE_AGENT: Note = Note::new(
        "git タブの Windows 対応に加えて、エージェントペインの spawn（orchestrator の Windows 縮退モード）が前提",
        "Requires Windows support for the git tab plus agent pane spawning (the degraded orchestrator mode on Windows)",
    );
    /// #521 の担当範囲
    pub const WIN_PREVIEW: Note = Note::new(
        "プレビュー / Web ビューの Windows 実装（WebView2・PDF・動画）が前提",
        "Requires the Windows implementation of preview / web view (WebView2, PDF, video)",
    );
    /// #522 の担当範囲
    pub const WIN_OS_INTEGRATION: Note = Note::new(
        "OS 連携（既定アプリ・ゴミ箱・ファイルマネージャ）の Windows 実装が前提",
        "Requires the Windows implementation of OS integration (default app, trash, file manager)",
    );
    /// #524 の担当範囲
    pub const WIN_OS_API: Note = Note::new(
        "OS API（プロセス検査・スリープ防止）の Windows 実装が前提",
        "Requires the Windows implementation of OS APIs (process inspection, sleep prevention)",
    );
    /// #525 の担当範囲
    pub const WIN_SETUP: Note = Note::new(
        "PowerShell シェル統合と setup の Windows 対応が前提",
        "Requires PowerShell shell integration and Windows support in setup",
    );
    /// #526 の担当範囲
    pub const WIN_ORCHESTRATOR: Note = Note::new(
        "orchestrator の Windows 縮退モードが前提",
        "Requires the degraded orchestrator mode on Windows",
    );
    /// #528 の担当範囲
    pub const WIN_REMOTE: Note = Note::new(
        "remote トランスポートと Windows 配布系統が前提",
        "Requires the remote transport and the Windows distribution channel",
    );

    /// 概念自体が存在しないもの
    pub const WIN_NO_TCC: Note = Note::new(
        "Windows に macOS の TCC（フルディスクアクセス）に相当する仕組みが無い",
        "Windows has no equivalent of the macOS TCC (Full Disk Access) mechanism",
    );
}

/// ある機能があるプラットフォームでどこまで使えるか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// 完全に動く
    Supported,
    /// 動くが機能が落ちる。`note` は UI とエラーメッセージにそのまま出る
    Degraded { note: Note },
    /// 未実装。追跡 Issue を必ず持つ
    Pending { note: Note, issue: u32 },
    /// そのプラットフォームには概念自体が存在しない（例: Windows の FDA）
    Unsupported { note: Note },
}

impl Support {
    pub fn status(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Degraded { .. } => "degraded",
            Self::Pending { .. } => "pending",
            Self::Unsupported { .. } => "unsupported",
        }
    }

    /// 縮退の理由（表示言語に追従する）
    pub fn note(self) -> Option<Note> {
        match self {
            Self::Supported => None,
            Self::Degraded { note } | Self::Pending { note, .. } | Self::Unsupported { note } => {
                Some(note)
            }
        }
    }

    pub fn issue(self) -> Option<u32> {
        match self {
            Self::Pending { issue, .. } => Some(issue),
            _ => None,
        }
    }

    /// 呼び出して意味があるか（縮退していても動くなら true）
    pub fn is_usable(self) -> bool {
        matches!(self, Self::Supported | Self::Degraded { .. })
    }
}

/// 1 機能ぶんの対応状況。`key` は MCP ツール名
pub struct Feature {
    pub key: &'static str,
    pub macos: Support,
    pub windows: Support,
}

impl Feature {
    pub fn on(&self, platform: Platform) -> Support {
        match platform {
            Platform::MacOs => self.macos,
            Platform::Windows => self.windows,
        }
    }
}

/// 指定プラットフォームでの対応状況。未登録キーは `None`
/// （`None` = マトリクスへの登録漏れ。T1 が検出する）
pub fn support_for(platform: Platform, key: &str) -> Option<Support> {
    MATRIX.iter().find(|f| f.key == key).map(|f| f.on(platform))
}

/// 指定プラットフォームの機能一覧。`status` を渡すとその状態だけに絞る
pub fn features(platform: Platform, status: Option<&str>) -> Vec<(&'static Feature, Support)> {
    MATRIX
        .iter()
        .map(|f| (f, f.on(platform)))
        .filter(|(_, s)| status.is_none_or(|want| s.status() == want))
        .collect()
}

/// 縮退している機能の説明文。system prompt へ注入して
/// 「この環境で何ができないか」を AI に知らせるのに使う（設計 §4）
pub fn degraded_notes(platform: Platform) -> Vec<&'static str> {
    degraded_note_items(platform)
        .into_iter()
        .map(Note::text)
        .collect()
}

/// 縮退している機能の理由を `Note` のまま返す（重複は畳む）。
///
/// **文言を早期に `&'static str` へ解決すると、その時点の言語で凍結して
/// 言語切替に追従しなくなる**。prompt へ注入するなど後で描画するものは必ずこちらを使う
pub fn degraded_note_items(platform: Platform) -> Vec<Note> {
    let mut seen: Vec<Note> = Vec::new();
    for f in MATRIX {
        if let Some(note) = f.on(platform).note() {
            if !seen.contains(&note) {
                seen.push(note);
            }
        }
    }
    seen
}

/// 実行してよいかの判定。`Err` の中身はそのまま利用者への診断メッセージになる。
/// **メッセージをマトリクス以外の場所に書かない**ための唯一の入口
pub fn gate(platform: Platform, key: &str) -> Result<(), String> {
    match support_for(platform, key) {
        // 未登録は素通しする。登録漏れで機能が止まるより、T1 の失敗で気付く方がよい
        None => Ok(()),
        Some(s) if s.is_usable() => Ok(()),
        Some(s) => {
            let note = s.note().map(Note::text).unwrap_or_default();
            let target = platform.as_str();
            Err(match (crate::i18n::lang(), s.issue()) {
                (crate::i18n::Lang::Ja, Some(issue)) => format!(
                    "{key} は {target} では未対応です（{note}）。追跡: #{issue}。\
                     実装したら crates/tako-core/src/platform/support.rs の対応状況を更新してください"
                ),
                (crate::i18n::Lang::Ja, None) => {
                    format!("{key} は {target} では未対応です（{note}）")
                }
                (crate::i18n::Lang::En, Some(issue)) => format!(
                    "{key} is not available on {target} ({note}). Tracking: #{issue}. \
                     Update crates/tako-core/src/platform/support.rs once implemented."
                ),
                (crate::i18n::Lang::En, None) => {
                    format!("{key} is not available on {target} ({note})")
                }
            })
        }
    }
}

/// 全機能の対応状況。**キーは昇順**（T4 が検証する）
pub const MATRIX: &[Feature] = &[
    Feature {
        key: "tako_agents_sync_rules",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_SETUP,
            issue: 525,
        },
    },
    Feature {
        key: "tako_auto_rename",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_background_kill",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_background_list",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_background_pane",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_check_health",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_close_pane",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_collapse_tab",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_confirm_close",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_create_tab",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_equalize_layout",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_fda",
        macos: Support::Supported,
        windows: Support::Unsupported {
            note: notes::WIN_NO_TCC,
        },
    },
    Feature {
        key: "tako_file_op",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_OS_INTEGRATION,
            issue: 522,
        },
    },
    Feature {
        key: "tako_focus_pane",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_foreground_pane",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_git_branch_create",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_checkout",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_commit",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_conflicts",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_diff",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_log",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_merge",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_merge_abort",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_pull",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_push",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_resolve_agent",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT_RESOLVE_AGENT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_show",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_stage",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_git_unstage",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GIT,
            issue: 520,
        },
    },
    Feature {
        key: "tako_lang",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_limit_service",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_list_panes",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_logs",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PERSIST,
            issue: 519,
        },
    },
    Feature {
        key: "tako_move_pane_to_tab",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_open_dir",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_open_file",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_open_remote",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE,
            issue: 528,
        },
    },
    Feature {
        key: "tako_orchestrator_accounts",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_handoff",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_layout",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_ledger",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_profiles",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_projects",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_report",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_respond",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_run",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_run_result",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_run_status",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_self",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_spawn",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_supervisor",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_worker_status",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_orchestrator_workers",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_panel",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_persist",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PERSIST,
            issue: 519,
        },
    },
    Feature {
        key: "tako_pin_preview",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_platform",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_port_detect",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_OS_API,
            issue: 524,
        },
    },
    Feature {
        key: "tako_preview_apply",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_autosave",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_cache",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_changelog",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_edit",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_follow_link",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_link_list",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_outline",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_redo",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_reload",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_replace",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_save",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_search",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_undo",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_preview_view",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_read_pane",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_recent",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_remote_agents",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE,
            issue: 528,
        },
    },
    Feature {
        key: "tako_remote_devices",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE,
            issue: 528,
        },
    },
    Feature {
        key: "tako_remote_messages",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE,
            issue: 528,
        },
    },
    Feature {
        key: "tako_remote_scrollback",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE,
            issue: 528,
        },
    },
    Feature {
        key: "tako_remote_setup",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE,
            issue: 528,
        },
    },
    Feature {
        key: "tako_remote_start",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE,
            issue: 528,
        },
    },
    Feature {
        key: "tako_remote_status",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE,
            issue: 528,
        },
    },
    Feature {
        key: "tako_remote_stop",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE,
            issue: 528,
        },
    },
    Feature {
        key: "tako_rename_tab",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_reorder_tab",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_resize_pane",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_run",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_run_defaults",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_run_interactive",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_run_interactive_status",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_run_resolve",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_scroll_pane",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_select_tab",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_send_input",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_sessions",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PERSIST,
            issue: 519,
        },
    },
    Feature {
        key: "tako_set_title",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_settings",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_setup",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_SETUP,
            issue: 525,
        },
    },
    Feature {
        key: "tako_setup_changes",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_SETUP,
            issue: 525,
        },
    },
    Feature {
        key: "tako_setup_mcp",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_SETUP,
            issue: 525,
        },
    },
    Feature {
        key: "tako_sleep_guard",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_OS_API,
            issue: 524,
        },
    },
    Feature {
        key: "tako_split_pane",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_ssh_hosts",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_SETUP,
            issue: 525,
        },
    },
    Feature {
        key: "tako_stale_binary",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_SETUP,
            issue: 525,
        },
    },
    Feature {
        key: "tako_task_checkpoint",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_task_gate",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_task_gate_check",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_task_gate_show",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_task_list",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_task_resume",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
        },
    },
    Feature {
        key: "tako_telemetry",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_theme",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_tmux_cleanup",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PERSIST,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tmux_kill",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PERSIST,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tmux_list",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PERSIST,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tmux_open",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PERSIST,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tmux_resize",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PERSIST,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tmux_select_window",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PERSIST,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tree_folder",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_update",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE,
            issue: 528,
        },
    },
    Feature {
        key: "tako_video_playback",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_video_seek",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_video_volume",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_web",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_PREVIEW,
            issue: 521,
        },
    },
    Feature {
        key: "tako_welcome",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
    Feature {
        key: "tako_window",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
        },
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{self, Lang};

    /// T4: 縮退には必ず理由が要る。`Pending` には追跡先も要る。
    /// 「理由も追跡先も無い縮退」を構造的に禁止する
    #[test]
    fn t4_縮退には理由と追跡先が必須() {
        for f in MATRIX {
            for (platform, support) in [(Platform::MacOs, f.macos), (Platform::Windows, f.windows)]
            {
                if let Support::Pending { issue, .. } = support {
                    assert!(
                        issue != 0,
                        "{} / {} が Pending なのに追跡 Issue が無い",
                        f.key,
                        platform.as_str()
                    );
                }
                if !matches!(support, Support::Supported) {
                    assert!(
                        support.note().is_some(),
                        "{} / {} は Supported ではないのに理由が無い",
                        f.key,
                        platform.as_str()
                    );
                }
            }
        }
    }

    /// 理由文の日英カタログ検査（#435 の `ui_text` と同じ基準）。
    /// **英語 UI に日本語が出ないこと**を機械的に担保する
    #[test]
    fn t4_理由文は日英そろっていて英語に日本語が残っていない() {
        for f in MATRIX {
            for support in [f.macos, f.windows] {
                let Some(note) = support.note() else { continue };
                assert!(!note.ja().trim().is_empty(), "{} の日本語が空", f.key);
                assert!(!note.en().trim().is_empty(), "{} の英語が空", f.key);
                assert!(
                    !note
                        .en()
                        .chars()
                        .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF)),
                    "{} の英語に日本語が残っている: {:?}",
                    f.key,
                    note.en()
                );
                for text in [note.ja(), note.en()] {
                    assert!(
                        !text.chars().any(|c| {
                            let cp = c as u32;
                            (0x1F000..=0x1FAFF).contains(&cp)
                                || (0x2600..=0x27BF).contains(&cp)
                                || cp == 0xFE0F
                        }),
                        "{} の理由文に絵文字が含まれている: {text:?}",
                        f.key
                    );
                }
            }
        }
    }

    /// 表示言語を切り替えると理由文も切り替わること（`&'static str` 直書きへの退行防止）
    #[test]
    fn 理由文は表示言語に追従する() {
        let original = i18n::lang();
        let note = support_for(Platform::Windows, "tako_git_log")
            .unwrap()
            .note()
            .expect("Windows の tako_git_log には理由があるはず");
        i18n::set_lang(Lang::Ja);
        let ja = note.text();
        i18n::set_lang(Lang::En);
        let en = note.text();
        i18n::set_lang(original);
        assert_eq!(ja, note.ja());
        assert_eq!(en, note.en());
        assert_ne!(ja, en);
    }

    /// 診断メッセージも表示言語に追従すること
    #[test]
    fn gateの診断も表示言語に追従する() {
        let original = i18n::lang();
        i18n::set_lang(Lang::En);
        let en = gate(Platform::Windows, "tako_git_log").unwrap_err();
        i18n::set_lang(Lang::Ja);
        let ja = gate(Platform::Windows, "tako_git_log").unwrap_err();
        i18n::set_lang(original);
        assert!(
            !en.chars()
                .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF)),
            "英語の診断に日本語が残っている: {en}"
        );
        assert!(en.contains("#520") && ja.contains("#520"));
    }

    /// キーの重複と並び順。順序を固定しておくと差分レビューが読める
    #[test]
    fn t4_キーは一意で昇順() {
        let keys: Vec<&str> = MATRIX.iter().map(|f| f.key).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "MATRIX のキーは昇順で並べること");
        let mut dedup = sorted.clone();
        dedup.dedup();
        assert_eq!(dedup.len(), keys.len(), "MATRIX のキーが重複している");
    }

    /// キーは MCP ツール名なので命名規約に従う
    #[test]
    fn t4_キーはmcpツール名の形をしている() {
        for f in MATRIX {
            assert!(
                f.key.starts_with("tako_")
                    && f.key
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "キーの形式が MCP ツール名と違う: {}",
                f.key
            );
        }
    }

    /// **macOS 上で Windows 側の縮退表を検証できる**ことの担保。
    /// これができないと「Windows でどう見えるか」を mac の開発中に確認できない
    #[test]
    fn 判定は純粋関数なので他プラットフォームの表も引ける() {
        let win = support_for(Platform::Windows, "tako_fda").expect("tako_fda が MATRIX に無い");
        assert_eq!(win.status(), "unsupported");
        assert!(!win.is_usable());
        let mac = support_for(Platform::MacOs, "tako_fda").unwrap();
        assert_eq!(mac.status(), "supported");
    }

    /// 縮退時の診断メッセージはマトリクス由来（二重管理を作らない）
    #[test]
    fn gateの診断はマトリクスの理由と追跡先を含む() {
        let err = gate(Platform::Windows, "tako_git_log").expect_err("Windows では未対応のはず");
        let note = support_for(Platform::Windows, "tako_git_log")
            .unwrap()
            .note()
            .unwrap();
        assert!(err.contains(note.text()), "診断に note が含まれない: {err}");
        assert!(err.contains("#520"), "診断に追跡 Issue が含まれない: {err}");
        assert!(gate(Platform::MacOs, "tako_git_log").is_ok());
    }

    /// マトリクス自身はどのプラットフォームでも引けないと意味がない
    #[test]
    fn マトリクス参照機能は全プラットフォームで使える() {
        for p in [Platform::MacOs, Platform::Windows] {
            assert!(
                support_for(p, "tako_platform").unwrap().is_usable(),
                "{} で tako_platform が使えない",
                p.as_str()
            );
            assert!(gate(p, "tako_platform").is_ok());
        }
    }

    #[test]
    fn 状態で絞り込める() {
        let pending = features(Platform::Windows, Some("pending"));
        assert!(!pending.is_empty(), "Windows の pending が空になっている");
        assert!(pending.iter().all(|(_, s)| s.status() == "pending"));
        assert!(features(Platform::MacOs, Some("pending")).is_empty());
    }

    /// prompt 注入用。同じ理由文が何十件も並ばないよう重複は畳む
    #[test]
    fn 縮退理由の一覧は重複しない() {
        let notes = degraded_notes(Platform::Windows);
        assert!(!notes.is_empty());
        let mut dedup = notes.clone();
        dedup.sort_unstable();
        dedup.dedup();
        assert_eq!(dedup.len(), notes.len(), "degraded_notes に重複がある");
        assert!(degraded_notes(Platform::MacOs).is_empty());
    }

    #[test]
    fn 未登録キーは判定不能を返しgateは素通しする() {
        assert!(support_for(Platform::Windows, "tako_not_a_real_tool").is_none());
        assert!(gate(Platform::Windows, "tako_not_a_real_tool").is_ok());
    }

    #[test]
    fn プラットフォーム名の相互変換() {
        for p in [Platform::MacOs, Platform::Windows] {
            assert_eq!(Platform::parse(p.as_str()), Some(p));
        }
        assert_eq!(Platform::parse("Windows"), Some(Platform::Windows));
        assert_eq!(Platform::parse("linux"), None);
    }
}
