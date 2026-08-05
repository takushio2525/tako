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
//!
//! ## この表を直したら
//!
//! 1. 縮退の理由は `PlatformFacts` 経由で **master / solo / setup の system prompt にも
//!    注入される**（#516）。宣言が実態とずれると、テスターのエージェントに
//!    「この環境では使えない」という誤情報がそのまま渡る
//! 2. doc サイトの「Windows 対応状況」ページはこの表からの生成物なので、
//!    `node scripts/gen-windows-support-docs.mjs` で再生成する（#591）

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

    /// #519 の担当範囲
    pub const WIN_PERSIST: Note = Note::new(
        "tmux バックエンドに依存。Windows の永続化戦略の決定が前提",
        "Depends on the tmux backend; requires deciding the Windows persistence strategy",
    );
    /// #519 M2: Windows の永続化は psmux（外部の tmux 互換 CLI）を器にする。
    /// **導入は任意**なので、有無で復元の深さが変わることをそのまま書く
    pub const WIN_PERSIST_PSMUX: Note = Note::new(
        "psmux（tmux 互換の永続化バックエンド）を導入すると実行中プロセスと画面ごと復元する。\
         未導入ならタブ・ペイン構成と cwd のみ復元し、実行中プロセスは tako の終了時に停止する",
        "With psmux (a tmux-compatible persistence backend) installed, running processes and \
         screen contents are restored. Without it, only tabs, panes and cwd are restored; \
         running processes stop when tako exits",
    );
    /// #519: 器がアウトオブプロセス到達（`DetachedAccess`）を持たない環境。
    /// 「フォールバックが失敗した」ではなく「そもそも到達手段が無い」ことを言う。
    ///
    /// **採取（読み）だけは psmux で動くようになった**（#519 の読み取り専用到達）ので、
    /// この理由文が当てはまるのは送出まで要る機能だけになっている
    pub const WIN_NO_DETACHED_REACH: Note = Note::new(
        "ペイン外からの採取（scrollback）に到達手段が要る。psmux 等の器を導入していない Windows では取得できない",
        "Capturing a pane from outside the app (scrollback) needs out-of-process reach, which is \
         unavailable on Windows without a session host such as psmux",
    );
    /// #519: psmux の `capture-pane` で scrollback を採れるようになった。
    /// **psmux の導入は任意**なので、あるとき / 無いときで何が変わるかをそのまま書く。
    /// 折返し結合（tmux の `-J`）が psmux では効かない点も、報告の見た目が変わるので書く
    pub const WIN_REPORT_PSMUX: Note = Note::new(
        "psmux（tmux 互換の永続化バックエンド）を導入していれば scrollback を採れる。\
         未導入なら claude の transcript からのみ報告を作る（他のエージェントでは報告が取れない）。\
         psmux は折返し行の結合に対応しないため、長い行は折り返されたまま出る",
        "With psmux (a tmux-compatible persistence backend) installed, scrollback is captured. \
         Without it, reports come only from the claude transcript (no report for other agents). \
         psmux does not join wrapped lines, so long lines stay hard-wrapped",
    );
    /// #687: psmux ペインでは器（psmux）がスクロールを持つ。tako はユーザーのホイールと
    /// 同じ経路で器を動かし、位置を読み戻して返す。**全画面 TUI では位置を読めない**ので、
    /// 何ができて何ができないかを具体的に書く（「使えない」と誤解されると回避行動を取られる）
    pub const WIN_SCROLL_BACKEND_OWNED: Note = Note::new(
        "psmux ペインではスクロール位置を器（psmux）が持つ。tako はユーザーのホイールと同じ経路で \
         器を動かし実位置を読み戻すが、器の粒度でしか位置を指定できない。\
         ペイン内のアプリが全画面（claude 等）のときはそのアプリが位置を持つため、\
         スクロールは効くが位置は返せず、to での絶対指定もできない",
        "In psmux panes the session host owns the scroll position. tako drives it through the same \
         path as the user's wheel and reads the real position back, but can only land on positions \
         the host's granularity allows. When a full-screen app (such as claude) runs in the pane, \
         that app owns the position: scrolling works, but the position cannot be read back and \
         absolute positioning with `to` is unavailable",
    );
    /// #519: 任意の tmux サーバーを直接操作する機能面。psmux は tmux 互換だが
    /// 「他人が立てた tmux セッションの発見と片付け」という用途自体が Windows には無い
    pub const WIN_TMUX_SERVER: Note = Note::new(
        "tmux サーバーそのものを操作する機能。Windows に tmux は無い",
        "Operates the tmux server itself, which does not exist on Windows",
    );
    /// #693: PDF のテキスト選択・コピーは Windows では使えない。content stream のパースと
    /// フォントエンコーディングの解決が必要で、lopdf 単体では困難なため
    pub const WIN_PDF_NO_TEXT_LAYER: Note = Note::new(
        "PDF のテキスト選択・コピーは Windows では使えない。PDF 内部のテキスト抽出には \
         フォントエンコーディングの解決が必要で、現在の構成では困難なため",
        "PDF text selection and copy are not available on Windows. Extracting text from PDF \
         internals requires font encoding resolution, which is not feasible with the current stack",
    );
    /// #521: 動画再生は AVFoundation 実装なので macOS 限定
    pub const WIN_VIDEO_MACOS_ONLY: Note = Note::new(
        "動画プレビューが macOS（AVFoundation）実装のため Windows では再生できない",
        "Video preview is implemented with macOS AVFoundation, so it cannot play on Windows",
    );
    /// #657: メニューは macOS では OS のグローバルメニューバーが所有するので、
    /// tako 側から開閉できない（項目の実行と一覧は OS メニューでも成立する）。
    /// **これは Windows の縮退ではなく macOS 側の縮退**という珍しい向きの例
    pub const MAC_MENU_IS_OS_OWNED: Note = Note::new(
        "メニューは OS のメニューバーが所有するため tako から開閉できない（open / close は不可）。\
         構成の取得 list と項目の実行 invoke は使える",
        "The menu is owned by the OS menu bar, so tako cannot open or close it \
         (open / close unavailable). Listing the structure and invoking items both work",
    );
    /// #521: PDF は Windows でも開けるようになった（Windows.Data.Pdf）。
    /// プレビューの中身で残る欠けは動画だけ
    pub const WIN_PREVIEW_NO_VIDEO: Note = Note::new(
        "コード・Markdown・画像・PDF は表示できる。動画は macOS 実装のため表示できない",
        "Code, Markdown, images and PDFs render. Video does not, as it is implemented for macOS only",
    );
    // #617 で B8 の Windows 実装（explorer.exe /select, / ShellExecuteW /
    // SHFileOperationW + FOF_ALLOWUNDO）が入り、`tako_file_op` は全操作が動くようになった。
    // 縮退理由 WIN_FILE_OP_PARTIAL（「ゴミ箱へ移動は完全削除になる」）は**実態と食い違う**
    // ので削除した。宣言が残っていると system prompt へ誤情報が注入される（このファイルの
    // モジュール doc「この表を直したら」を参照）

    // #524 で B5（プロセス検査）と B9（スリープ防止）の Windows 実装が入り、
    // `tako_port_detect` は Supported、`tako_sleep_guard` は下の理由で Degraded に
    // なった。共通の縮退理由 WIN_OS_API（「OS API の Windows 実装が前提」）は
    // 使う機能が無くなったので削除した（残すと system prompt へ誤情報が入る）

    /// #524 + #697: アイドルスリープの抑止（`PowerCreateRequest`）も
    /// 蓋を閉じたままの継続稼働（電源プランの `GUID_LIDCLOSE_ACTION` を倒す）も動く。
    ///
    /// 残る差は 2 つだけ。**本体温度の監視**（macOS の `NSProcessInfo.thermalState` 相当が無い）と、
    /// **蓋の開閉状態の表示**（`RegisterPowerSettingNotification` にウィンドウハンドルが要り、
    /// 状態表示のためだけに持つには重い）。どちらも蓋閉じ継続の動作そのものには影響しない
    /// （#524 時点の「相当する API が無い」は誤りだった。#697 で実測して訂正）
    pub const WIN_SLEEP_GUARD_NO_THERMAL: Note = Note::new(
        "アイドルスリープの防止と蓋を閉じたままの継続稼働はどちらも動く。\
         本体温度の監視と蓋の開閉状態の表示だけが macOS 固有のため Windows には無い\
         （動作そのものには影響しない）",
        "Both idle-sleep prevention and keeping the machine running with the lid closed work. \
         Only thermal monitoring and showing the lid open/closed state are macOS-specific and \
         unavailable on Windows (neither affects how lid-close continuation behaves)",
    );
    /// #525: 実行ペインは PowerShell で起こすようにした（pwsh 7 → Windows PowerShell 5.1 の順に解決）。
    /// 残る差は `&&` / `||` だけ。**何が落ちて何をすれば直るか**まで書く
    /// （「Windows では使えない」と誤解されると回避行動を取られてしまうため）
    pub const WIN_RUN_NO_CHAIN_ON_PS51: Note = Note::new(
        "実行ペインは PowerShell で動く。ただし PowerShell 7 が無く Windows PowerShell 5.1 だけの環境では、\
         `&&` / `||` でつないだコマンド（C / C++ / Rust の拡張子既定を含む）が構文エラーになる。\
         PowerShell 7 を入れると解消する",
        "The run pane works through PowerShell. On machines that only have Windows PowerShell 5.1 \
         (no PowerShell 7), commands chained with `&&` or `||` — including the built-in extension \
         defaults for C / C++ / Rust — fail to parse. Installing PowerShell 7 resolves this",
    );
    /// #525: 環境チェック・設定生成・MCP 登録・winget 案内まで通る。
    /// setup から設定**できない**項目だけが残る（何が残るかを具体的に書く）
    pub const WIN_SETUP_PARTIAL: Note = Note::new(
        "環境チェック・設定の生成・MCP 登録・winget での導入案内は動く。\
         シェル統合（PowerShell）は Windows 未対応のため、状態の表示だけで設定はできない",
        "Environment checks, config generation, MCP registration and winget install guidance all work. \
         Shell integration (PowerShell) is unavailable on Windows, so setup only reports its status \
         instead of configuring it",
    );
    /// #525: シェル統合（OSC 7 / 133）は zsh / bash / fish 用スクリプトしか同梱していない。
    /// PowerShell 版が無いと **cwd 追従とコマンド状態の検知が効かない**ので、
    /// 「何が起きないか」を具体的に書く（設定漏れと区別できるように）
    pub const WIN_NO_SHELL_INTEGRATION: Note = Note::new(
        "シェル統合（OSC 7 / 133）が zsh / bash / fish 用のみで PowerShell 版が無い。\
         ペインの cwd 追従とコマンド実行状態の検知が働かない",
        "Shell integration (OSC 7 / 133) ships only for zsh / bash / fish; there is no PowerShell \
         version. Pane cwd tracking and command state detection do not work",
    );
    /// #525 と同根（claude バイナリの解決が POSIX シェル経由）。
    /// 機能そのものは残り、命名の質だけが落ちるので Pending ではなく Degraded
    pub const WIN_AUTO_RENAME_HEURISTIC: Note = Note::new(
        "AI による命名は claude CLI の解決が Windows で効かないため働かず、ヒューリスティック命名にとどまる",
        "AI naming does not run because the claude CLI cannot be resolved on Windows; naming falls back to heuristics",
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
        windows: Support::Supported,
    },
    Feature {
        key: "tako_auto_rename",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_AUTO_RENAME_HEURISTIC,
        },
    },
    Feature {
        key: "tako_background_kill",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_background_list",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_background_pane",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_check_health",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_close_pane",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_collapse_tab",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_confirm_close",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_create_tab",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_equalize_layout",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_fda",
        macos: Support::Supported,
        windows: Support::Unsupported {
            note: notes::WIN_NO_TCC,
        },
    },
    Feature {
        // #617: reveal / trash / open_default / open_with とも Windows 実装済み
        // （ごみ箱は FOF_ALLOWUNDO で復元可能。完全削除ではない）
        key: "tako_file_op",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_focus_pane",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_foreground_pane",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_branch_create",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_checkout",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_commit",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_conflicts",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_diff",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_log",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_merge",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_merge_abort",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_pull",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_push",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_resolve_agent",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_show",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_stage",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_git_unstage",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_lang",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_limit_service",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_list_panes",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_logs",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_menu",
        // macOS はメニューが OS のメニューバーに載るので tako から開閉できない
        // （構成の取得 list と項目の実行 invoke は動く）。Windows は自前描画の
        // メニューバー行なので全操作が使える（#657）
        macos: Support::Degraded {
            note: notes::MAC_MENU_IS_OS_OWNED,
        },
        windows: Support::Supported,
    },
    Feature {
        key: "tako_move_pane_to_tab",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_open_dir",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_open_file",
        macos: Support::Supported,
        // #521: PDF は OS 標準の Windows.Data.Pdf で開けるようになった。残るのは動画だけ
        windows: Support::Degraded {
            note: notes::WIN_PREVIEW_NO_VIDEO,
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
        windows: Support::Supported,
    },
    // #662: ダイアログの内容取得と応答。tako-app が保持しているペインは
    // in-process（画面採取もキー送出も）で完結するため Windows でも動く。
    // ペイン消失後の detached 経路は #519 の役割 B 待ちだが、
    // 「ダイアログが出ている」= ペインは生きているので実用上の縮退はない
    Feature {
        key: "tako_orchestrator_dialog",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_handoff",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_launch_status",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_layout",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_ledger",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_profiles",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_projects",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_report",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_REPORT_PSMUX,
        },
    },
    Feature {
        key: "tako_orchestrator_respond",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_run",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_run_result",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_run_status",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_self",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_spawn",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_supervisor",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_worker_status",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_orchestrator_workers",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_panel",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_persist",
        macos: Support::Supported,
        // #519 M2 で器（psmux）が入った。**導入は任意**なので Pending ではなく Degraded:
        // psmux があれば完全復元、無ければ構成のみ復元（どちらも動く）
        windows: Support::Degraded {
            note: notes::WIN_PERSIST_PSMUX,
        },
    },
    Feature {
        key: "tako_pin_preview",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_platform",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_port_detect",
        macos: Support::Supported,
        // #524: GetExtendedTcpTable + Toolhelp32。ペイン配下の判定は
        // 制御端末（macOS）ではなく PTY 直下プロセスの子孫で行う
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_apply",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_autosave",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_cache",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_changelog",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_edit",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_follow_link",
        macos: Support::Supported,
        // #693: lopdf で PDF 構造を解析しリンク注釈を取得。テキスト選択はできない
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_link_list",
        macos: Support::Supported,
        // #693: lopdf で PDF 構造を解析しリンク注釈を取得
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_outline",
        macos: Support::Supported,
        // #693: lopdf で PDF のアウトライン（しおり）を取得
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_redo",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_reload",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_replace",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_save",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_search",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_undo",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_preview_view",
        macos: Support::Supported,
        // #521: ズーム / パンの対象は PDF と画像。両方 Windows で動くようになった
        windows: Support::Supported,
    },
    Feature {
        key: "tako_read_pane",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_recent",
        macos: Support::Supported,
        windows: Support::Supported,
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
        windows: Support::Supported,
    },
    Feature {
        key: "tako_reorder_tab",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_resize_pane",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_run",
        macos: Support::Supported,
        // #525 で実行ペインを PowerShell 経由にした（実測 2026-07-29: pwsh 7.6.4 /
        // Windows PowerShell 5.1 の両方で終了コードの回収まで動作）。
        // 残るのは 5.1 のみの環境で `&&` / `||` が構文エラーになる点だけ
        windows: Support::Degraded {
            note: notes::WIN_RUN_NO_CHAIN_ON_PS51,
        },
    },
    Feature {
        key: "tako_run_defaults",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_run_interactive",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_RUN_NO_CHAIN_ON_PS51,
        },
    },
    Feature {
        key: "tako_run_interactive_status",
        macos: Support::Supported,
        // 終了コードは実行ペインが出すマーカー行から拾うので、経路は macOS と同一。
        // cmdlet が `$LASTEXITCODE` を設定しない差は `platform::shell` 側で吸収済み
        windows: Support::Degraded {
            note: notes::WIN_RUN_NO_CHAIN_ON_PS51,
        },
    },
    Feature {
        key: "tako_run_resolve",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_scroll_pane",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_SCROLL_BACKEND_OWNED,
        },
    },
    Feature {
        key: "tako_select_tab",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_send_input",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    // #662: キー符号化は tako-core の純粋関数で OS 差が無い（送出先は同じ PTY）
    Feature {
        key: "tako_send_keys",
        macos: Support::Supported,
        windows: Support::Supported,
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
        windows: Support::Supported,
    },
    Feature {
        key: "tako_settings",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_setup",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_SETUP_PARTIAL,
        },
    },
    Feature {
        key: "tako_setup_changes",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_setup_mcp",
        macos: Support::Supported,
        // #525 で解決。旧実装は claude の探索が `which`（Windows に無い）だったため
        // 一切見つからず自動登録が丸ごと失敗していた。探索を抽象境界 B16
        //（PATH + PATHEXT）へ寄せて解消。実測 2026-07-27: 登録・再登録とも成功
        windows: Support::Supported,
    },
    Feature {
        key: "tako_sleep_guard",
        macos: Support::Supported,
        // #524: PowerCreateRequest / PowerSetRequest でアイドルスリープを抑止。
        // #697: 蓋閉じ継続も電源プランの lid action を倒して動くようになった。
        // 残るのは thermal 監視と蓋の開閉表示だけなので Degraded は維持
        windows: Support::Degraded {
            note: notes::WIN_SLEEP_GUARD_NO_THERMAL,
        },
    },
    Feature {
        key: "tako_split_pane",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_ssh_hosts",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_stale_binary",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_task_checkpoint",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_task_gate",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_task_gate_check",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_task_gate_show",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_task_list",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_task_resume",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_telemetry",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_theme",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_tmux_cleanup",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TMUX_SERVER,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tmux_kill",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TMUX_SERVER,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tmux_list",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TMUX_SERVER,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tmux_open",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TMUX_SERVER,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tmux_resize",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TMUX_SERVER,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tmux_select_window",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TMUX_SERVER,
            issue: 519,
        },
    },
    Feature {
        key: "tako_tree_folder",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_update",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_video_playback",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_VIDEO_MACOS_ONLY,
            issue: 521,
        },
    },
    Feature {
        key: "tako_video_seek",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_VIDEO_MACOS_ONLY,
            issue: 521,
        },
    },
    Feature {
        key: "tako_video_volume",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_VIDEO_MACOS_ONLY,
            issue: 521,
        },
    },
    Feature {
        key: "tako_web",
        macos: Support::Supported,
        windows: Support::Supported,
    },
    Feature {
        key: "tako_window",
        macos: Support::Supported,
        windows: Support::Supported,
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

    /// 表示言語を切り替えると理由文も切り替わること（`&'static str` 直書きへの退行防止）
    #[test]
    fn 理由文は表示言語に追従する() {
        let original = i18n::lang();
        let (key, _) = any_pending_on_windows();
        let note = support_for(Platform::Windows, key)
            .unwrap()
            .note()
            .expect("Pending には理由があるはず");
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
        let (key, issue) = any_pending_on_windows();
        i18n::set_lang(Lang::En);
        let en = gate(Platform::Windows, key).unwrap_err();
        i18n::set_lang(Lang::Ja);
        let ja = gate(Platform::Windows, key).unwrap_err();
        i18n::set_lang(original);
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

    /// 縮退時の診断メッセージはマトリクス由来（二重管理を作らない）
    #[test]
    fn gateの診断はマトリクスの理由と追跡先を含む() {
        let (key, issue) = any_pending_on_windows();
        let err = gate(Platform::Windows, key).expect_err("Windows では未対応のはず");
        let note = support_for(Platform::Windows, key).unwrap().note().unwrap();
        assert!(err.contains(note.text()), "診断に note が含まれない: {err}");
        assert!(
            err.contains(&format!("#{issue}")),
            "診断に追跡 Issue が含まれない: {err}"
        );
        assert!(gate(Platform::MacOs, key).is_ok());
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

    /// prompt 注入用。同じ理由文が何十件も並ばないよう重複は畳む。
    ///
    /// **#657 まで macOS 側の縮退はゼロだった**（macOS 先行開発なので当然）。
    /// in-window メニューバーだけは「Windows は自前描画なので開閉できる / macOS は
    /// メニューを OS が所有するので tako から開閉できない」という**逆向きの差**に
    /// なったため、macOS 側にも縮退が入りうる前提へ改めた。ここを
    /// 「macOS は縮退ゼロ」で固定し直すと、宣言と実態の食い違い（= AI への誤情報）を
    /// 通してしまう（このファイルのモジュール doc「この表を直したら」を参照）
    #[test]
    fn 縮退理由の一覧は重複しない() {
        for platform in [Platform::Windows, Platform::MacOs] {
            let notes = degraded_notes(platform);
            assert!(!notes.is_empty(), "{platform:?} の縮退理由が空");
            let mut dedup = notes.clone();
            dedup.sort_unstable();
            dedup.dedup();
            assert_eq!(
                dedup.len(),
                notes.len(),
                "{platform:?} の degraded_notes に重複がある"
            );
        }
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
