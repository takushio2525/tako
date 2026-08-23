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
    /// #868 / #525。状態照会と手順の提示はできるが、実行の代行は macOS だけ
    pub const WIN_SETUP_BOOTSTRAP: Note = Note::new(
        "状態の確認と公式手順の案内はできるが、インストールの実行代行は macOS だけ（Windows は PowerShell 版インストーラを案内する）",
        "Status and official instructions work, but tako only runs the installer for you on macOS (on Windows it points to the PowerShell installer)",
    );
    /// #525。配置はできるが器が OSC を落とす
    pub const WIN_SHELL_INTEGRATION_PSMUX: Note = Note::new(
        "配置はできるが psmux が OSC を外へ通さないため、cwd 追従とコマンド状態は TAKO_BACKEND=none のときだけ働く",
        "Installs, but psmux does not pass OSC through, so cwd tracking and command state only work with TAKO_BACKEND=none",
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

    /// 「リモートからフォルダを開く」（#919 / #65）。
    /// バックエンドは Windows 10 以降が同梱する OpenSSH クライアント（`ssh` / `sftp`）
    /// なので**移植は要らない設計**だが、実機で 1 度も測っていないので Pending。
    /// 実測すべき点: ControlMaster が Windows の named pipe / ソケットで張れるか、
    /// `ControlPath` の引用が通るか、`ssh_pane_script` の PowerShell 版が動くか
    pub const WIN_REMOTE_FOLDER: Note = Note::new(
        "同梱の OpenSSH クライアントで動く設計だが Windows 実機で未実測",
        "Designed to work with the bundled OpenSSH client, but not yet measured on Windows",
    );

    /// GUI ライク表示モードのチャットビュー（#691 の G2 以降）。
    /// 表示レイヤだけの機能なので #517 で足りそうに見えるが、**会話の解決が
    /// 永続バックエンドのセッション名を鍵にしている**（`.agent/plans/2026-07-gui-mode.md`
    /// §4 G2 の帰結: チャット化されるのはバックエンドを持つペインだけ）ため、
    /// GUI 起動と永続化戦略の両方が要る。スターター側（#694 / #739）は GUI 起動だけで
    /// 動くので `tako_ui_mode` は `WIN_TERMINAL` のまま
    pub const WIN_GUI_CHAT: Note = Note::new(
        "GUI 起動に加えて、チャットビューが会話をひも付けるための永続バックエンド（tmux 相当）の決定が前提",
        "Requires GUI startup plus deciding the persistent backend (the tmux equivalent) that the chat view resolves conversations through",
    );

    /// #513 の担当範囲。実装はプラットフォーム共通（ファイル操作 + git のみ）で、
    /// パス可搬化の Windows 表記も macOS 上の単体テストで検証済み。
    /// 残っているのは**実機での配線確認**だけなので、その一点だけを理由として書く
    pub const WIN_CONFIG_SHARE: Note = Note::new(
        "実装はプラットフォーム共通だが、Windows 実機での配線確認が未了",
        "The implementation is platform-neutral, but wiring has not yet been verified on real Windows hardware",
    );

    /// OS が同等機能を標準で持っていて、tako 側の実装が不要なもの（#600）
    pub const WIN_NO_PSREADLINE_NEEDED: Note = Note::new(
        "Windows の PowerShell は PSReadLine の予測入力を標準搭載しているため、tako 側の注入は要らない",
        "Windows PowerShell ships PSReadLine predictive input, so tako does not need to inject anything",
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
    gate_in(platform, key, crate::i18n::lang())
}

/// `gate` の言語を明示する版。**言語グローバルに触らず解決できる**ようにするため、
/// 実体はこちらの純粋関数に置く（`Note::text_in` と同じ方針）。
///
/// 表示言語の解決を 1 箇所に集約する意味もある。定型文と理由文で別々に
/// `i18n::lang()` を読むと、その間に言語が切り替わったとき
/// 「日本語の定型文 + 英語の理由文」のような混在が出る（#608）
pub fn gate_in(platform: Platform, key: &str, lang: crate::i18n::Lang) -> Result<(), String> {
    match support_for(platform, key) {
        // 未登録は素通しする。登録漏れで機能が止まるより、T1 の失敗で気付く方がよい
        None => Ok(()),
        Some(s) if s.is_usable() => Ok(()),
        Some(s) => {
            let note = s.note().map(|n| n.text_in(lang)).unwrap_or_default();
            let target = platform.as_str();
            Err(match (lang, s.issue()) {
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
        // #600: tako 内 zsh の入力予測（zsh-autosuggestions をシェル統合経路で注入）
        key: "tako_autosuggest",
        macos: Support::Supported,
        windows: Support::Unsupported {
            note: notes::WIN_NO_PSREADLINE_NEEDED,
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
        // #725: GUI モードのチャットビュー本文コピー。表示レイヤの機能だが、
        // 会話の解決に永続バックエンドが要る（#739 で理由を精緻化）
        key: "tako_chat_copy",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_GUI_CHAT,
            issue: 519,
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
        // #513: AI 系設定の git ベース共有。GUI にも tmux にも依存しない
        key: "tako_config_share",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_CONFIG_SHARE,
            issue: 513,
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
        // #813: 上限後の自動復帰。ダイアログへの応答が tmux バックエンド（detached access）
        // 経由なので、Windows は永続バックエンドの移植（#526 のオーケストレーション層）待ち
        key: "tako_limit_resume",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_ORCHESTRATOR,
            issue: 526,
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
        // #657: メニューの操作。両プラットフォームで「メニューは存在し、一覧 list と
        // 項目の実行 invoke が使える」ので Supported。
        //
        // macOS だけ `open` / `close` が使えない（メニューを OS のメニューバーが
        // 所有しており、tako にポップアップさせる手段が無い）が、これを `Degraded`
        // にはしない。理由は 2 つ:
        //
        // - macOS ではメニューバー自体が**ネイティブで完全に動く**。ユーザーから見て
        //   欠けている機能は無く、`Degraded` は実態より重い表現になる
        // - `Degraded` の note は `PlatformFacts` 経由で system prompt へ入り、
        //   `known_limitations_markdown` 経由で**リリースノートの「既知の制限」節**にも
        //   出る（#594）。macOS 側に縮退はこれまで 1 件も無く、Windows 移植で macOS の
        //   リリースノートが増えるのは筋が通らない
        //
        // 使えない組み合わせは呼んだ瞬間に dispatch が理由と代替（invoke）を名指しで
        // 返す（`require_in_window_menu`）ので、必要な情報は使用時点で届く
        key: "tako_menu",
        macos: Support::Supported,
        windows: Support::Supported,
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
        // #552: 自動命名された名前の固定（GUI のピン印と 1:1）
        key: "tako_pin_tab_title",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
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
        key: "tako_preview_copy_code",
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
        key: "tako_remote_folder",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE_FOLDER,
            issue: 919,
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
    // #868。macOS は公式 native インストーラ（install.sh）で実行まで代行する。
    // Windows は install.sh 自身が非対応（MINGW*/MSYS*/CYGWIN* で exit 1）で、
    // 公式手順は PowerShell の install.ps1。状態照会と手順の提示までは動くが
    // 実行の代行は実機で確かめてから倒す（#525）
    Feature {
        key: "tako_setup_bootstrap",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_SETUP_BOOTSTRAP,
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
    // #525。Windows 実機で実測して倒した（作法どおり、動くことを確認してから）:
    // 配置・冪等・解除の完全復帰と OSC 7 / 133 の到達（pwsh 7 と 5.1 の両方）を確認。
    // ただし**器が psmux だと OSC が外へ出ない**ので Supported ではなく Degraded
    Feature {
        key: "tako_shell_integration",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_SHELL_INTEGRATION_PSMUX,
        },
    },
    Feature {
        key: "tako_show_command",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TERMINAL,
            issue: 517,
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
        // #694 / #739: 表示レイヤだけの切替で、モードトグルとスターター
        // （プロファイル選択 ▾ を含む）は GUI 本体（#517）が動けばそのまま動く。
        // チャットビューだけは永続バックエンドにも依存するが、それは
        // `tako_chat_copy`（WIN_GUI_CHAT）側で追跡する
        key: "tako_ui_mode",
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

    /// 検証に使う「Windows で Pending の機能」を表から拾う。
    /// キーを直書きすると、その機能が実装されて Supported になった瞬間にテストが
    /// 「理由が無い」で落ちる（実際 #591 の棚卸しで `tako_git_log` がそうなった）
    fn any_pending_on_windows() -> (&'static str, u32) {
        MATRIX
            .iter()
            .find_map(|f| f.windows.issue().map(|issue| (f.key, issue)))
            .expect("Windows に Pending が 1 件も無い（テストの前提が崩れている）")
    }

    /// 表示言語を切り替えると理由文も切り替わること（`&'static str` 直書きへの退行防止）。
    /// **言語グローバルへの追従そのものが検査対象**なので、ここは
    /// `lang_guard` で直列化する（#608。他のテストは `text_in` / `gate_in` を使うこと）
    #[test]
    fn 理由文は表示言語に追従する() {
        let _guard = i18n::testing::lang_guard();
        let (key, _) = any_pending_on_windows();
        let note = support_for(Platform::Windows, key)
            .unwrap()
            .note()
            .expect("Pending には理由があるはず");
        i18n::set_lang(Lang::Ja);
        let ja = note.text();
        i18n::set_lang(Lang::En);
        let en = note.text();
        assert_eq!(ja, note.ja());
        assert_eq!(en, note.en());
        assert_ne!(ja, en);
    }

    /// 診断メッセージも表示言語に追従すること（同上。グローバル追従の検査なので直列化する）
    #[test]
    fn gateの診断も表示言語に追従する() {
        let _guard = i18n::testing::lang_guard();
        let (key, issue) = any_pending_on_windows();
        i18n::set_lang(Lang::En);
        let en = gate(Platform::Windows, key).unwrap_err();
        i18n::set_lang(Lang::Ja);
        let ja = gate(Platform::Windows, key).unwrap_err();
        assert!(
            !en.chars()
                .any(|c| matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF)),
            "英語の診断に日本語が残っている: {en}"
        );
        let tag = format!("#{issue}");
        assert!(en.contains(&tag) && ja.contains(&tag));
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

    /// 縮退時の診断メッセージはマトリクス由来（二重管理を作らない）。
    ///
    /// 言語は `gate_in` / `text_in` に明示して渡す。**言語グローバルを読むと
    /// 診断と理由文を別々のタイミングで解決することになり、その間に
    /// 言語切替テストが走ると不一致で落ちる**（#608 の再現経路）
    #[test]
    fn gateの診断はマトリクスの理由と追跡先を含む() {
        let (key, issue) = any_pending_on_windows();
        let note = support_for(Platform::Windows, key).unwrap().note().unwrap();
        for lang in [Lang::Ja, Lang::En] {
            let err = gate_in(Platform::Windows, key, lang).expect_err("Windows では未対応のはず");
            assert!(
                err.contains(note.text_in(lang)),
                "{lang:?} の診断に note が含まれない: {err}"
            );
            assert!(
                err.contains(&format!("#{issue}")),
                "{lang:?} の診断に追跡 Issue が含まれない: {err}"
            );
            assert!(gate_in(Platform::MacOs, key, lang).is_ok());
        }
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
