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

    // ─── 未実測（実装はあるが Windows 実機で確かめていない） ───────────────

    /// 実装がプラットフォーム共通で、macOS と同じ経路を通るもの。
    /// **動く見込みはあるが実機で 1 度も実行していない**ので Supported にはしない
    /// （過大申告は system prompt 経由でエージェントを誤らせる。#516）。
    /// 消し込みの手順は追跡 Issue に書いてある
    pub const WIN_UNVERIFIED: Note = Note::new(
        "実装はプラットフォーム共通で macOS と同じ経路を通るが、Windows 実機での実測がまだ無い（動く見込み。失敗したらまずここを疑う）",
        "The implementation is platform-neutral and takes the same path as macOS, but it has not been exercised on real Windows hardware yet (expected to work; suspect this first if it fails)",
    );

    // ─── 実測で分かっている縮退 ───────────────────────────────────

    /// #693。Windows.Data.Pdf はページ画像は返すが文字位置を返さない
    pub const WIN_PDF_NO_TEXT_LAYER: Note = Note::new(
        "PDF はページ画像として表示できるが、Windows のレンダラが文字位置を返さないため文字選択・目次・PDF 内リンクは使えない（#693）",
        "PDFs render as page images, but the Windows renderer does not expose glyph positions, so text selection, outlines and in-PDF links are unavailable (#693)",
    );

    /// #521。動画は macOS の AVFoundation 実装しかなく、非 macOS はスタブ
    pub const WIN_VIDEO: Note = Note::new(
        "動画デコーダの Windows 実装が無く、動画ファイルを開くとエラーになる",
        "There is no Windows video decoder implementation, so opening a video file returns an error",
    );

    /// #724 症状②。`wry` の WebView2 が COM コールバック内で unwrap して abort する
    pub const WIN_WEBVIEW2_PANIC: Note = Note::new(
        "Web ビューは WebView2 側の巻き戻せない panic でアプリごと落ちるため開けない",
        "The web view cannot be opened: WebView2 raises a non-unwinding panic that aborts the whole app",
    );

    /// #528。daemon の起動・停止が `setsid` / シグナルマスク前提のまま
    pub const WIN_REMOTE_UNVERIFIED: Note = Note::new(
        "remote デーモンの起動・停止に unix 前提の処理が残っており、Windows 実機での通し確認も未了",
        "Starting and stopping the remote daemon still relies on unix-only handling, and no end-to-end run has been measured on Windows",
    );

    /// #1090。SSH クライアントの能力そのものの縮退（多重化が無い）。
    /// **文言の正本は境界側**（[`crate::platform::ssh_client::NO_MULTIPLEXING`]）で、
    /// マトリクスはそれを参照するだけにする（説明が 2 つに分かれない）
    pub const WIN_SSH_NO_MULTIPLEXING: Note = crate::platform::ssh_client::NO_MULTIPLEXING;

    /// #1090 / #976 / #930。リモートフォルダは Windows でも開けるが、
    /// 多重化が無いぶんの縮退と、自動検知が働かないぶんの縮退が重なる
    pub const WIN_REMOTE_FOLDER: Note = Note::new(
        "同梱の OpenSSH クライアントで開けるが、接続多重化（ControlMaster）が無いので操作ごとに認証が起きる（パスワード認証しか無い相手は展開のたびに聞かれる。接続が生きているかも判定できない。#1090）。ペインの ssh を検知した自動追加（#976）は、プロセスのコマンド行を採れないので働かない（明示的に開く経路だけが使える）",
        "Folders open with the bundled OpenSSH client, but without connection multiplexing (ControlMaster) every operation authenticates on its own (a password-only host asks on every expansion, and liveness cannot be determined, #1090). Auto-adding folders by detecting ssh in a pane (#976) does not work because process command lines are unavailable; only the explicit open path is usable",
    );

    /// #519。器は psmux。一覧と kill は通るが attach 系は通らない
    pub const WIN_TMUX_ATTACH: Note = Note::new(
        "永続化の器は psmux で、attach と send-keys を前提にする tmux 操作は動かない",
        "The persistence container is psmux, so tmux operations that assume attach and send-keys do not work",
    );

    /// #519。psmux は `-x` / `-y` を受け取るが反映しない
    pub const WIN_TMUX_RESIZE: Note = Note::new(
        "psmux がセッションの寸法指定（-x / -y）を反映しないため寸法を変えられない",
        "psmux ignores the session size options (-x / -y), so the size cannot be changed",
    );

    /// #760。#722 で AI 経路は走るようになったが、命名の**質**の縮退は残る
    pub const WIN_AUTORENAME_ONCE: Note = Note::new(
        "AI 命名は動くが、シェル統合が無い（#525）ためタブの素材（cwd / タイトル / 実行状態）が起動後に変化せず、命名はタブごとに 1 回だけになる。claude を導入していない環境ではタブ名が PowerShell の実行ファイルパスになる（#760）",
        "AI naming works, but without shell integration (#525) a tab's inputs (cwd, title, command state) never change after startup, so each tab is named only once. Without claude installed the tab name becomes the PowerShell executable path (#760)",
    );

    /// #935。登録と表示は動き、実行だけが落ちる
    pub const WIN_GATE_CHECK_SH: Note = Note::new(
        "ゲートの登録と表示は動くが、コマンド型ゲートの実行が sh -c 決め打ちのため Windows では判定できない（#935）",
        "Registering and showing gates works, but command gates run through a hardcoded sh -c, so they cannot be evaluated on Windows (#935)",
    );

    /// #936。PATH 上の探索（#898 で境界へ寄せた）は動くが、実行中プロセスを特定できない
    pub const WIN_STALE_BINARY_PID: Note = Note::new(
        "PATH 上の claude の実在確認は動くが、実行中の claude のパスを解決できないため古いバイナリの警告が出ない（#936 / #726）",
        "Checking that claude exists on PATH works, but the running claude's path cannot be resolved, so the stale binary warning never appears (#936 / #726)",
    );

    /// #766 / #525。側路（`TAKO_OSC_SINK`）で届くが、能力申告は素通し不可のまま
    /// #1067。ハーネス更新は旧プロセスへの終了要求（境界 B5 の制御側）が要る。
    /// Windows 実装は未着手（`platform::process::terminate` が明示的に Err を返す）
    pub const WIN_SESSION_RESTART_TERMINATE: Note = Note::new(
        "引き継ぎ再起動は使えるが、ハーネス更新（会話を保ったまま CLI を建て直す）はプロセスの終了要求が Windows 未対応のため使えない（#1067 / 境界 B5）",
        "Restarting with a handoff works, but the harness update (rebuilding the CLI while keeping the conversation) is unavailable because process termination is not implemented on Windows (#1067 / boundary B5)",
    );
    pub const WIN_SHELL_INTEGRATION_PSMUX: Note = Note::new(
        "cwd 追従とコマンド状態は器（psmux）越しでも側路で届くが、psmux が OSC を素通ししないため status の effective は false のままになる（#766）",
        "cwd tracking and command state are delivered through a side channel even inside the psmux container, but because psmux does not pass OSC through, the status field effective stays false (#766)",
    );

    /// #937。確認とノート表示は動くが、適用（インストーラー実行）は未実測
    pub const WIN_UPDATE_APPLY_UNVERIFIED: Note = Note::new(
        "更新の確認とリリースノートの表示は動くが、更新の適用（インストーラーの実行と再起動）は Windows 実機で未実測（#937）",
        "Checking for updates and rendering the release notes work, but applying an update (running the installer and restarting) has not been measured on Windows (#937)",
    );

    /// #899。**画面は出るがボタンが効かない**（PR #931 が実機検証待ちで open）
    pub const WIN_WELCOME_INJECTION: Note = Note::new(
        "バナーの表示と案内コマンドの取得は動くが、ボタンからのコマンド投入が LF + POSIX クォート決め打ちなので Windows では実行されない（#899。PR #931 が実機検証待ち）",
        "The banner renders and the suggested commands can be read, but the command injection behind its buttons hardcodes LF and POSIX quoting, so nothing runs on Windows (#899; PR #931 is awaiting verification on real hardware)",
    );

    /// #899。表示レイヤは動くがスターターのボタンだけが効かない
    pub const WIN_STARTER_INJECTION: Note = Note::new(
        "表示モードの切替とチャット表示は動くが、スターターカードのボタンからのコマンド投入が LF + POSIX クォート決め打ちなので Windows では実行されない（#899。PR #931 が実機検証待ち）",
        "Switching the display mode and the chat view work, but the command injection behind the starter cards hardcodes LF and POSIX quoting, so nothing runs on Windows (#899; PR #931 is awaiting verification on real hardware)",
    );

    /// #1057。検出は両 OS で動くが、パッケージ導入の代行は brew（macOS）だけ
    pub const WIN_SETUP_DEPS: Note = Note::new(
        "依存の検出はできるが、導入の実行代行は macOS（Homebrew）だけ。Windows は winget のコマンドを案内する",
        "Dependency detection works, but tako only runs the installer for you on macOS (Homebrew); on Windows it prints the winget command instead",
    );

    /// #970。`canonicalize` の verbatim prefix が OSC 7 経路で `///?/…` へ壊れる
    pub const WIN_OPEN_DIR_VERBATIM: Note = Note::new(
        "新タブは開けるが、ペインの cwd が `///?/C:/…` になり、そのタブでは git 操作が「git リポジトリではない」で止まる（`tab new --cwd` / `tree add` も同じ。#970）",
        "The new tab opens, but the pane cwd becomes `///?/C:/...`, so git operations in that tab stop with \"not a git repository\" (`tab new --cwd` and `tree add` share this; #970)",
    );

    /// #971 / #1038。unix ソケット target の決め打ちは #1038 で解消したが、Windows 実機は未実測
    pub const WIN_REMOTE_SERVE_UNIX: Note = Note::new(
        "#1038 で serve の中継先をループバック TCP へ変えたので、`unix socket serve target is not supported on Windows` で止まる原因は無くなった。ただし Windows 実機での通し（setup の 4 段目 → デーモン起動 → スマホからの接続）は未実測（#971）",
        "The hard-coded unix socket serve target was removed in #1038 (the daemon now listens on loopback TCP), so `unix socket serve target is not supported on Windows` no longer applies. The end-to-end run on real Windows hardware (setup step 4 -> daemon start -> phone access) is still unmeasured (#971)",
    );

    /// #972。器の境界（#519）を通らず `tako_core::tmux` を直に叩いている
    pub const WIN_REMOTE_SCROLLBACK_BACKEND: Note = Note::new(
        "スクロールバックの取得が器の境界を通らず psmux で解決できない（セッション名でもペイン ID でも `no server running` になる。#972）",
        "Scrollback capture bypasses the container boundary and psmux cannot resolve it (both a session name and a pane id return `no server running`; #972)",
    );

    // ─── そもそも要らない / 概念が無い ─────────────────────────────

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

/// Windows 判定の根拠。**何をもってそう言えるのか**を表そのものに持たせる。
///
/// マトリクスは `PlatformFacts` 経由で system prompt へ流れる（#516）ので、
/// 宣言が実態より甘いと「使える」と信じたエージェントが失敗し続ける。
/// 逆に辛いと使える機能を回避する。どちらも実害があるため、
/// **`Supported` / `Degraded` / `Unsupported` は根拠を持つことをテストで強制する**（T7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// 実機の GUI セルフテスト（`TAKO_SELF_TEST=1`）が通した項目。
    /// 2026-08-24 の通しで **FAILED 0 / skip 19** まで到達している（#920）
    SelfTest(&'static str),
    /// 実機の `cargo test` で緑のテスト。
    /// 既知の失敗（#583 系のベースライン）に含まれないことが条件
    UnitTest(&'static str),
    /// 実機で実際に実行した記録（`.agent/plans/` の記録節 / Issue コメント）
    Measured(&'static str),
    /// OS の仕様・設計判断。**実測する対象がそもそも無い**もの
    /// （Windows に TCC が無い / PSReadLine が予測入力を標準搭載している 等）
    ByDesign(&'static str),
    /// 未実測。**`Supported` にはできない**（T7 が落とす）
    Unverified,
}

impl Evidence {
    /// 判定の裏づけになる文言（未実測なら `None`）
    pub fn citation(self) -> Option<&'static str> {
        match self {
            Self::SelfTest(s) | Self::UnitTest(s) | Self::Measured(s) | Self::ByDesign(s) => {
                Some(s)
            }
            Self::Unverified => None,
        }
    }

    /// 根拠の種別（docs の表と `tako platform --json` に出す）
    pub fn kind(self) -> &'static str {
        match self {
            Self::SelfTest(_) => "self-test",
            Self::UnitTest(_) => "unit-test",
            Self::Measured(_) => "measured",
            Self::ByDesign(_) => "by-design",
            Self::Unverified => "unverified",
        }
    }
}

/// 1 機能ぶんの対応状況。`key` は MCP ツール名
pub struct Feature {
    pub key: &'static str,
    pub macos: Support,
    pub windows: Support,
    /// Windows 判定の根拠。macOS 側は開発機なので根拠欄を持たない
    pub windows_evidence: Evidence,
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
        key: "tako_agent_support",
        macos: Support::Supported,
        windows: Support::Supported,
        // 静的な表を引くだけの純粋処理で、OS 依存の口を 1 つも通らない
        // （`tako_platform` と同じ性質）
        windows_evidence: Evidence::UnitTest(
            "agent_parity 5 本と agent_support の単体 15 本が緑（判定は純粋関数で OS を見ない）",
        ),
    },
    Feature {
        key: "tako_agents_sync_rules",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako agents sync-rules --source <正本>` が claude のグローバル指示へマーカーブロックを書き（action=updated + .bak 生成）、未導入の codex / agy は理由つきで skip される",
        ),
    },
    Feature {
        key: "tako_auto_rename",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_AUTORENAME_ONCE,
        },
        windows_evidence: Evidence::Measured(
            "#722 の Windows 11 実測: 隔離 GUI で AI 経路が走りタブ名が AI 由来（同素材のヒューリスティックは PowerShell のパス由来で別物）。セルフテスト項目 51 / 52（適用・手動優先・ON/OFF）も緑。残る縮退は #760 の実測（素材が不変なので 2 回目以降が発火しない）",
        ),
    },
    Feature {
        // #600: tako 内 zsh の入力予測（zsh-autosuggestions をシェル統合経路で注入）
        key: "tako_autosuggest",
        macos: Support::Supported,
        windows: Support::Unsupported {
            note: notes::WIN_NO_PSREADLINE_NEEDED,
        },
        windows_evidence: Evidence::ByDesign(
            "PowerShell が PSReadLine の予測入力を標準搭載しているので注入する対象が無い（セルフテスト項目 41c / 41c-2 は zsh 不在で自動スキップ）",
        ),
    },
    Feature {
        key: "tako_background_kill",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: MCP `tako_background_kill` が killed=2 を返し、`tako backgrounded` が 1 件から空になる",
        ),
    },
    Feature {
        key: "tako_background_list",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 47c（ドロワーに実画面プレビューが並ぶ）",
        ),
    },
    Feature {
        key: "tako_background_pane",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 47b（ー ボタンでバックグラウンドへ退避）",
        ),
    },
    Feature {
        // #725: GUI モードのチャットビュー本文コピー。表示レイヤの機能だが、
        // 会話の解決に永続バックエンドが要る（#739 で理由を精緻化）
        key: "tako_chat_copy",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 98 / 115（チャット本文の選択・コピー・索引）",
        ),
    },
    Feature {
        key: "tako_check_health",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: MCP `tako_check_health` が HTTP 200 で healthy=true / tmux_available=true / persist_enabled=true / version_match=true / issues=[] を返す",
        ),
    },
    Feature {
        key: "tako_close_pane",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 6 / 28 / 40 / 40b（cmd+W・tako close・非フォーカス側 close・10 周の fd 検査）",
        ),
    },
    Feature {
        key: "tako_collapse_tab",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako collapse --tab 1 on` が collapsed=true を返し `tako list` の collapsed も true、`off` で戻る",
        ),
    },
    Feature {
        // #513: AI 系設定の git ベース共有。GUI にも tmux にも依存しない
        key: "tako_config_share",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako config init` が共有リポジトリを作って初回コミット（7 ファイル）、`tako config` が差分（same 4）を出し、`tako config pull` が 1 件取り込む",
        ),
    },
    Feature {
        key: "tako_confirm_close",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 73a〜73f（確認ダイアログの表示・Esc・Enter・即 close）",
        ),
    },
    Feature {
        key: "tako_create_tab",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 7 / 25 / 116（cmd+T・tako tab new・--cwd 指定）",
        ),
    },
    Feature {
        key: "tako_equalize_layout",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 23（tako equalize）",
        ),
    },
    Feature {
        key: "tako_fda",
        macos: Support::Supported,
        windows: Support::Unsupported {
            note: notes::WIN_NO_TCC,
        },
        windows_evidence: Evidence::ByDesign(
            "Windows に TCC（フルディスクアクセス）相当の仕組みが無いので許可を求める対象が無い（#515 の判定テストが固定）",
        ),
    },
    Feature {
        key: "tako_file_op",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#617 の Windows 11 実測: 空白 + 日本語名 / 読み取り専用 / ディレクトリ / 315 文字のパスがいずれも復元可能な状態でごみ箱へ入り、reveal で対象が選択され、既定アプリが起動する。実機で緑のテスト: os_integration の windows モジュール（FOF_ALLOWUNDO のフラグ構成 / 絶対化 / /select, の形）",
        ),
    },
    Feature {
        key: "tako_focus_pane",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 4 / 24（方向フォーカス移動・tako focus）",
        ),
    },
    Feature {
        key: "tako_foreground_pane",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako background --pane N` で外したペインが `tako foreground N` で由来タブへ戻る（list の panes が 1 → 1,2）",
        ),
    },
    Feature {
        key: "tako_git_branch_create",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako git branch <名前> --pane <p>` が新ブランチを作ってチェックアウトする（cwd が通常形のペインで実施。verbatim prefix の cwd では解決できない = #970）",
        ),
    },
    Feature {
        key: "tako_git_checkout",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako git checkout topic --pane <p>` が checked_out=true を返し実リポジトリの HEAD が移る（cwd が通常形のペインで実施 = #970）",
        ),
    },
    Feature {
        key: "tako_git_commit",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 79 / 79b / 86（コミットメッセージ入力欄・両経路のコミット・IME。git データを取得できない環境ではこの項目は自己スキップする）",
        ),
    },
    Feature {
        key: "tako_git_conflicts",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 82b / 109（使い捨てリポのコンフリクトを git パネルが認識する。git データを取得できない環境ではこの項目は自己スキップする）",
        ),
    },
    Feature {
        key: "tako_git_diff",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 85 / 79b（変更ファイルの分類と diff。git データを取得できない環境ではこの項目は自己スキップする）+ #520 の parse_diff CRLF 耐性",
        ),
    },
    Feature {
        key: "tako_git_log",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 85（git タブのセクション表示順。git データを取得できない環境ではこの項目は自己スキップする）+ #520 のパス可搬化と CRLF 耐性テストが実機で緑",
        ),
    },
    Feature {
        key: "tako_git_merge",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako git merge topic -y --pane <p>` が merge コミットを作り、`-y` なしのドライランは作業ツリーを変えずに予測（predicted_conflicts）だけ返す",
        ),
    },
    Feature {
        key: "tako_git_merge_abort",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: 衝突する merge が conflicted=true / conflicts=[c.txt] になり、`tako git abort` が aborted=merging を返して HEAD と作業ツリーが元へ戻る",
        ),
    },
    Feature {
        key: "tako_git_pull",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako git pull --pane <p>` が対向 bare リポジトリの新コミットを取り込み merge コミットを作る",
        ),
    },
    Feature {
        key: "tako_git_push",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako git push --pane <p>` で対向 bare リポジトリの main が push 後の HEAD へ進む",
        ),
    },
    Feature {
        key: "tako_git_resolve_agent",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 82b / 109（3 択の開閉と起動。git データを取得できない環境ではこの項目は自己スキップする）+ #867 でエージェントペインの実起動を実機実測",
        ),
    },
    Feature {
        key: "tako_git_show",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 85（コミット詳細。git データを取得できない環境ではこの項目は自己スキップする）+ #520 の to_git_path / repo_relative",
        ),
    },
    Feature {
        key: "tako_git_stage",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 79b（ステージング UI の分類とコミット挙動。git データが取れない環境では自己スキップする項目）",
        ),
    },
    Feature {
        key: "tako_git_unstage",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 79b（ステージング UI の分類とコミット挙動。git データを取得できない環境ではこの項目は自己スキップする）",
        ),
    },
    Feature {
        key: "tako_lang",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 33c（MCP tako_lang: en 適用 → i18n 反映 → system 復帰）",
        ),
    },
    Feature {
        // #813: 上限後の自動復帰。ダイアログへの応答が tmux バックエンド（detached access）
        // 経由なので、Windows は永続バックエンドの移植（#526 のオーケストレーション層）待ち
        key: "tako_limit_resume",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 111 / 117（オプトイン・ダイアログ型 / idle 型の出し分け・試行上限・プロファイル既定）",
        ),
    },
    Feature {
        key: "tako_limit_service",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako limit-service` が現在サービスを返し、claude → codex → claude の切替が反映される",
        ),
    },
    Feature {
        key: "tako_list_panes",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 17 / 33（tako list・MCP tako_list_panes）",
        ),
    },
    Feature {
        key: "tako_logs",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 87 / 104（ペインログのクローズマーカーと発生源）",
        ),
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
        windows_evidence: Evidence::SelfTest(
            "項目 118（in-window メニューバー #657 の open / invoke / close）",
        ),
    },
    Feature {
        // 設定ファイルの読み書きだけで完結する（GUI に触らない）。**壊れた設定で
        // GUI が起動しない環境でこそ要る**ので、両 OS で必ず動くことが要件（#916）
        key: "tako_migrate",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 123（実 dispatch で自動マイグレーション）+ migrations の単体 20 本が実機で緑",
        ),
    },
    Feature {
        key: "tako_move_pane_to_tab",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 26 / 68c（tako tab move-pane・target + direction）",
        ),
    },
    Feature {
        key: "tako_open_dir",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_OPEN_DIR_VERBATIM,
        },
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako open-in dir <path>` は新タブを作り `tako recent list` にも載るが、ペインの cwd が `///?/C:/…` になりそのタブの git 操作が全滅する（#970）",
        ),
    },
    Feature {
        key: "tako_open_file",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 66 / 66b / 68b / 112 / 114 / 116（dispatch・tako open・direction・新しいタブ）",
        ),
    },
    Feature {
        key: "tako_open_remote",
        macos: Support::Supported,
        // #1090: 接続そのものは成立するが、対話ペインでログインしても
        // その接続をツリー（sftp）と共有できない（Windows の OpenSSH に
        // 接続多重化が無い）。#65 の「一度入れば以後追加認証なし」が成立しない
        windows: Support::Degraded {
            note: notes::WIN_SSH_NO_MULTIPLEXING,
        },
        windows_evidence: Evidence::Measured(
            "#1090 の Windows 11 実測（OpenSSH_for_Windows_10.0p2）: ControlMaster 系を渡すと接続の前に `getsockname failed: Not a socket` / exit -1 で死に、渡さないと同じ相手へ exit 255（`Could not resolve hostname` / `Host key verification failed`）まで進む。渡さない形にしたうえで到達不能ホストの 3 経路（split / tab / pane）が理由 + 次の一手を出してローカルのシェルへ戻ることを実測",
        ),
    },
    Feature {
        key: "tako_orchestrator_accounts",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `accounts list`（空）→ `add --inherit` → `show` → `list`（1 件）→ `remove` → `list`（空）の往復",
        ),
    },
    Feature {
        key: "tako_orchestrator_handoff",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 101 / 102 / 102b / 102c / 122（自動通知・後任起動・新旧書式・管轄解決）",
        ),
    },
    Feature {
        // #915: 引き継ぎファイルの管理（一覧 / 読み / 書き / 自動移行）。
        // GUI も IPC も要らないローカルのファイル操作で、実装は両 OS 共通（パスは
        // `PathBuf::join` だけで組み、キーは両 OS で通る文字しか受理しない）。
        // Windows 実機で 13/13 実測済み（移行・冪等・list / show / write・
        // 日本語本文の保存・危険なキーの拒否・`\` 区切りのパス）
        key: "tako_orchestrator_handoffs",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#915 で実機 13/13（移行・冪等・list / show / write・日本語本文・円記号区切りのパス）+ セルフテスト項目 122",
        ),
    },
    Feature {
        key: "tako_orchestrator_layout",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 72（master-reserved の配置と close 後のリフロー）",
        ),
    },
    Feature {
        key: "tako_orchestrator_ledger",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: spawn が作った台帳エントリを `ledger list` が返し、`ledger record --outcome pass` / `ledger amend` が反映され `ledger stats` が pass_rate=100 になる",
        ),
    },
    Feature {
        key: "tako_orchestrator_profiles",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 96 / 99 / 117（設定画面のフォーム・スターターの ▾・limit_resume の既定）",
        ),
    },
    Feature {
        key: "tako_orchestrator_projects",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 117（一時プロジェクトの登録と解除）",
        ),
    },
    Feature {
        key: "tako_orchestrator_report",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako orchestrator report --pane <p>` が source=scrollback で実ペインの出力を返す（第 1 層）。transcript 層は実機の claude が未認証で会話を作れず未実測",
        ),
    },
    Feature {
        key: "tako_orchestrator_respond",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 95 / 102 / 111（選択肢ダイアログの検知と番号 / ラベル確定）",
        ),
    },
    Feature {
        key: "tako_orchestrator_run",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: MCP `tako_orchestrator_run` が run_id を即返して worker を spawn し、CLI の同期版は status=timeout + 出力 + closed=true まで返す（完遂は実機の claude が未認証のため未実測）",
        ),
    },
    Feature {
        key: "tako_orchestrator_run_result",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako orchestrator run-result <run_id>` が status / duration_seconds / output / pane_id を返す",
        ),
    },
    Feature {
        key: "tako_orchestrator_run_status",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako orchestrator run-status` が starting/running → timeout/finished と elapsed_seconds を返す（MCP と CLI の両経路）",
        ),
    },
    Feature {
        key: "tako_orchestrator_self",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 102（後任 master の self がプロファイルと handoff_path を引き継ぐ）",
        ),
    },
    Feature {
        key: "tako_orchestrator_spawn",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 72 / 117（配置エンジンとプロファイル適用）+ #867 で実機の claude 起動と env 到達を PEB で確認",
        ),
    },
    Feature {
        key: "tako_orchestrator_supervisor",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `supervisor status` → `set_mode --mode notify_only` → `status` → `set_mode --mode auto` の往復と `history --lines 5`",
        ),
    },
    Feature {
        key: "tako_orchestrator_worker_status",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 74 / 105（IPC 応答と busy 中の後続 send）+ #877 で agents 経由の status=idle を実機実測",
        ),
    },
    Feature {
        key: "tako_orchestrator_workers",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 105（レジストリの登録・再読込・再解決）",
        ),
    },
    Feature {
        key: "tako_panel",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 49 / 56 / 64 / 64b（fleet ビュー・タブ枠・tako panel roundtrip・ファイルツリー経路）",
        ),
    },
    Feature {
        key: "tako_persist",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 58（tako persist の ON/OFF と状態取得）+ 実機 psmux_backend 16/0",
        ),
    },
    Feature {
        key: "tako_pin_preview",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako pin --pane N on` が pinned へ矩形つきで載り、`off` で消える",
        ),
    },
    Feature {
        // #552: 自動命名された名前の固定（GUI のピン印と 1:1）
        key: "tako_pin_tab_title",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 51b（自動命名直後の「この名前を固定」）",
        ),
    },
    Feature {
        key: "tako_platform",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::UnitTest(
            "platform_parity 13 本と support の単体が実機で緑（判定は純粋関数）",
        ),
    },
    Feature {
        key: "tako_port_detect",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "スライス 9 で tako list が 8123/node.exe を拾い、psmux の偽 listen 21 個を 1 つも報告しない + セルフテスト項目 55（ON/OFF）",
        ),
    },
    Feature {
        key: "tako_preview_apply",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 66d（全文適用）",
        ),
    },
    Feature {
        key: "tako_preview_autosave",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako edit autosave true/false` が状態を往復する。有効化後の CLI / MCP 編集で自動保存が発火しないのは実装が共通なので macOS でも同じ（タイマーを始めるのが GUI 入力経路だけ。#973）",
        ),
    },
    Feature {
        key: "tako_preview_cache",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 33d / 66c（MCP と CLI から同じ LRU 上限へ反映）",
        ),
    },
    Feature {
        key: "tako_preview_changelog",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako preview-changelog on --pane <p>` が changelog=true / commits=2 を返し `off` で戻る",
        ),
    },
    Feature {
        key: "tako_preview_copy_code",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 90 / 114（画面外のコードブロックも含めてコピー）",
        ),
    },
    Feature {
        key: "tako_preview_edit",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 66d（tako edit で開始 → 適用 → 保存）",
        ),
    },
    Feature {
        key: "tako_preview_follow_link",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_PDF_NO_TEXT_LAYER,
        },
        windows_evidence: Evidence::SelfTest(
            "項目 90（Markdown の ⌘+クリックは緑。URL は cmd /C start で開く。PDF 内リンクは不可）",
        ),
    },
    Feature {
        key: "tako_preview_link_list",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_PDF_NO_TEXT_LAYER,
        },
        windows_evidence: Evidence::SelfTest(
            "項目 90 / 114（Markdown リンク索引は緑。PDF 注釈リンクは不可）",
        ),
    },
    Feature {
        key: "tako_preview_outline",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_PDF_NO_TEXT_LAYER,
        },
        windows_evidence: Evidence::SelfTest(
            "項目 114（Markdown 目次のジャンプは緑。PDF 目次は text_layer 不在でスキップ）",
        ),
    },
    Feature {
        key: "tako_preview_redo",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako edit redo --pane <p>` が redone=true を返す（undo と対で実測）",
        ),
    },
    Feature {
        key: "tako_preview_reload",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 66c（実 CLI の ON/OFF と OS イベントでの再生成）",
        ),
    },
    Feature {
        key: "tako_preview_replace",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako edit replace <前> <後>` と `--all` が replaced を返し、`tako edit save` 後の実ファイルが置換後の内容になる",
        ),
    },
    Feature {
        key: "tako_preview_save",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 66d（保存と外部変更の拒否）",
        ),
    },
    Feature {
        key: "tako_preview_search",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako edit search <語>` が index=2 / total=2 を返し、`--direction next/prev` で index が 1 ↔ 2 と動く",
        ),
    },
    Feature {
        key: "tako_preview_undo",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako edit undo --pane <p>` が undone=true を返す（redo と対で実測）",
        ),
    },
    Feature {
        key: "tako_preview_view",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_PDF_NO_TEXT_LAYER,
        },
        windows_evidence: Evidence::SelfTest(
            "項目 66b-2 / 70 / 112 / 114（コード・md・画像は緑。PDF はページ画像だけ通り文字座標の検査はスキップ）",
        ),
    },
    Feature {
        key: "tako_read_pane",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 20 / 33（tako read・MCP tako_read_pane）",
        ),
    },
    Feature {
        key: "tako_recent",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako recent list` が `open-in dir` で開いたディレクトリを返す",
        ),
    },
    Feature {
        key: "tako_remote_agents",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE_SERVE_UNIX,
            issue: 971,
        },
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako remote agents` は agents=[] を返し走査そのものは動く（#877 の境界）が、`tako remote setup` が serve 設定で失敗しデーモンを起動できない（#971）",
        ),
    },
    Feature {
        key: "tako_remote_devices",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE_SERVE_UNIX,
            issue: 971,
        },
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako remote devices list` は running=false の形を返すが、デーモンを起動できないので端末を登録できない（#971）",
        ),
    },
    Feature {
        key: "tako_remote_folder",
        macos: Support::Supported,
        // #1090: 多重化を外したので sftp が通るようになった。残る縮退は
        // 「操作ごとの認証」と「#976 の自動検知が働かない」の 2 つ
        windows: Support::Degraded {
            note: notes::WIN_REMOTE_FOLDER,
        },
        windows_evidence: Evidence::Measured(
            "#1090 の Windows 11 実測: ControlMaster 系を渡した sftp は `getsockname failed: Not a socket` で握手にすら進まないが、渡さないと同じ相手へ SSH の握手が進む（`Host key verification failed` まで到達）。渡さない形で `tako remote-folder open` / `ls` が実 SSH 先の一覧を返すことを実測",
        ),
    },
    Feature {
        key: "tako_remote_messages",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE_UNVERIFIED,
            issue: 528,
        },
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: CLI が <SESSION_ID> を要求するところまで確認。実機の claude が未認証で会話を作れないため本体は未実測（デーモン側は #971 でブロック）",
        ),
    },
    Feature {
        key: "tako_remote_scrollback",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE_SCROLLBACK_BACKEND,
            issue: 972,
        },
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: セッション名でもペイン ID でも `psmux: no server running on session '<socket>__<target>'` になる。同じソケットへ境界経由で叩く `tako tmux list` は成功する（#972）",
        ),
    },
    Feature {
        key: "tako_remote_setup",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE_SERVE_UNIX,
            issue: 971,
        },
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測（#1038 の修正**前**）: 1〜3 段（Tailscale 検出 / ログイン / HTTPS 証明書）は OK で、4 段目の serve 設定が `unix socket serve target is not supported on Windows` で失敗した。#1038 でこの原因は取り除いたが、実機での再測はまだ（#971）",
        ),
    },
    Feature {
        key: "tako_remote_start",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE_UNVERIFIED,
            issue: 528,
        },
        windows_evidence: Evidence::UnitTest(
            "実機の cargo test で remote::tests の 2 件（daemon_stop_impl / is_process_alive）が失敗",
        ),
    },
    Feature {
        key: "tako_remote_status",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE_SERVE_UNIX,
            issue: 971,
        },
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako remote status` は running=false を返すが、デーモンを起動できないので常にこの状態（#971）",
        ),
    },
    Feature {
        key: "tako_remote_stop",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_REMOTE_UNVERIFIED,
            issue: 528,
        },
        windows_evidence: Evidence::UnitTest(
            "同上（daemon_stop_impl はpid再利用時にkillしない が失敗）",
        ),
    },
    Feature {
        key: "tako_rename_tab",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 50（tako tab rename）",
        ),
    },
    Feature {
        key: "tako_reorder_tab",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako tab reorder 1 --index 1` でタブ順が tab2,tab1,tab3 へ入れ替わり `--index 0` で戻る",
        ),
    },
    Feature {
        key: "tako_resize_pane",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 5 / 5b / 22（キーボード・境界ドラッグ・tako resize --share-y）",
        ),
    },
    Feature {
        key: "tako_run",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#875 の実機 before/after: 「PTY を起動できなかった」→ 出力 + __TAKO_EXIT=0。終了コード 4 型・引用符・日本語・psmux 経由まで実測 + セルフテスト項目 91(d) の実行検査が ran=true",
        ),
    },
    Feature {
        key: "tako_run_defaults",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::UnitTest(
            "拡張子既定の登録・削除・一覧は設定ファイル I/O だけで、単体が実機で緑",
        ),
    },
    Feature {
        key: "tako_run_interactive",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako run-interactive --pane <p> <コマンド>` が新ペインで実行し、`--wait` が exit_code=0 / status=exited を返す（ペインが極端に狭いとマーカーが折り返して検出できない = #651。macOS も同様）",
        ),
    },
    Feature {
        key: "tako_run_interactive_status",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako run-interactive-status <pane>` が exit_code=0 / status=exited を返す（狭いペインの折り返しは #651）",
        ),
    },
    Feature {
        key: "tako_run_resolve",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#875 の実機実測で Code Runner の宣言 / 拡張子既定の解決から実行まで通した（3 経路のうちの 1 つ）",
        ),
    },
    Feature {
        key: "tako_scroll_pane",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 43 / 44 / 44b（ホイールの出し分け・tako scroll・ピクセル単位スクロール）",
        ),
    },
    Feature {
        key: "tako_select_tab",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 12 / 27（cmd+1・tako tab select）",
        ),
    },
    Feature {
        key: "tako_send_input",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 19（tako send）。非 ASCII は #907 で器の注入口へ迂回済み",
        ),
    },
    Feature {
        // #1067: エージェントペインの「セッションを引き継いで再起動」2 種。
        // 引き継ぎ再起動（handoff）は master ペインへ定型文を送るだけなので両 OS 共通だが、
        // ハーネス更新（harness）は旧プロセスへ終了要求を出す必要があり、境界 B5 の
        // Windows 実装が未着手（`platform::process::terminate` は明示 Err）。
        // **未実測ではなく構造的に使えない**ので Pending ではなく Degraded
        key: "tako_session_restart",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_SESSION_RESTART_TERMINATE,
        },
        windows_evidence: Evidence::ByDesign(
            "tako_control::platform::process::terminate の Windows 実装は「プロセスの停止は Windows では未対応です」を返す（B5 の制御側が未実装）。handoff は queue_prompt_flow だけを使うので影響を受けない",
        ),
    },
    Feature {
        key: "tako_sessions",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#877 で実機の session_id 解決（resolve_session_id_for_backend -> Some）を実測 + sessions の単体 14 本が実機で緑。resume のペイン起動そのものは未実測だが、経路は #867 で実機実測済みの launch と同じ",
        ),
    },
    Feature {
        key: "tako_set_title",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 21（tako title --role）",
        ),
    },
    Feature {
        key: "tako_settings",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 96 / 120（プロファイルタブ・スリープ防止タブの表示構成）",
        ),
    },
    Feature {
        key: "tako_setup",
        macos: Support::Supported,
        // セルフテスト項目 97 が見ているのは**スターターに setup の行が入ること**で、
        // `tako setup` そのものの実行ではない。dispatch の SetupRun は実機で未実測
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako setup --check` が claude（未認証）/ psmux / git / tailscale / スリープ防止 / MCP 未登録を正しく列挙し、`--changes --json` が revision 17 と未適用一覧を返す。対話の通し（エージェント起動）は実機の claude が未認証のため未実測",
        ),
    },
    Feature {
        key: "tako_setup_bootstrap",
        macos: Support::Supported,
        // #1057 で実行代行（install / path）を Windows へ配線し実機で通した
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#1057 の Windows 11 実測: 隔離 USERPROFILE + PATH 剥ぎで `tako setup` が install（install.ps1 を -ExecutionPolicy Bypass -File で実行）→ path（ユーザー環境変数 Path へ追記・undo-path で完全復帰）→ auth 誘導 まで到達。2 回目は無言で素通り",
        ),
    },
    Feature {
        key: "tako_setup_changes",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::UnitTest(
            "changes.yaml の連番・platforms 絞り込みテストが実機で緑（#525 が platforms: を最初に使う）",
        ),
    },
    Feature {
        key: "tako_setup_deps",
        macos: Support::Supported,
        // 検出は両 OS で動くが、導入の代行は brew（macOS）だけ。
        // Windows は winget のコマンドを案内する（実機実測を経てから代行する）
        windows: Support::Degraded {
            note: notes::WIN_SETUP_DEPS,
        },
        windows_evidence: Evidence::Measured(
            "#1057 の Windows 11 実測: `tako setup deps` が器（psmux）/ git / tailscale を実際の解決結果つきで列挙し、install は winget を代行せず not_delegable で理由 + コマンドを返す",
        ),
    },
    Feature {
        key: "tako_setup_mcp",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako setup-mcp` が claude の設定（スクラッチ HOME 側）へ tako を登録し旧内容を backups へ退避する。別途、実 HOME の登録に対して `claude mcp list` が `tako.exe mcp serve` を Connected と健康判定したので、stdio ブリッジ自体も Windows で通る",
        ),
    },
    Feature {
        key: "tako_setup_models",
        macos: Support::Supported,
        // 取得は境界 B16（`platform::exe::find`）で解決した CLI を子プロセスとして
        // 起動するだけなので構造上は動くが、Windows 実機での取得は**未実測**。
        // #591 の規約どおり Pending + 追跡 #937 のまま置く（過大申告しない）
        windows: Support::Pending {
            note: notes::WIN_UNVERIFIED,
            issue: 937,
        },
        windows_evidence: Evidence::Unverified,
    },
    Feature {
        key: "tako_shell_integration",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_SHELL_INTEGRATION_PSMUX,
        },
        windows_evidence: Evidence::Measured(
            "#766 で側路の state が unknown → idle、exit_code=3、cwd が OSC 7 由来で追従。実機 shell_integration_powershell 7/0（#525）",
        ),
    },
    Feature {
        key: "tako_show_command",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 91 / 91b（カードとカード帯）+ #875 で新規ペイン実行を実機実測",
        ),
    },
    Feature {
        key: "tako_sleep_guard",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "powercfg /requests の SYSTEM に tako のアサーションが出て mode=off で消える。蓋閉じは lid-guard.json の生成まで確認 + セルフテスト項目 120 / 121",
        ),
    },
    Feature {
        key: "tako_split_pane",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 2 / 18 / 34（cmd+D・tako split・MCP tako_split_pane）",
        ),
    },
    Feature {
        key: "tako_ssh_hosts",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::UnitTest(
            "~/.ssh/config の解析は純粋関数で、remote_fs / ssh_hosts の単体が実機で緑（ホーム解決は #870 で一本化）",
        ),
    },
    Feature {
        key: "tako_stale_binary",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_STALE_BINARY_PID,
        },
        windows_evidence: Evidence::UnitTest(
            "stale_binary::tests::test_pidpath_self と ランチャ探索…の 2 件が失敗。PATH 上の探索は #898 で境界 B16 へ寄せて実機実測済み",
        ),
    },
    Feature {
        key: "tako_task_checkpoint",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako task checkpoint --task-id … --phase running` が保存され、`tako task update --phase verifying` が反映される",
        ),
    },
    Feature {
        key: "tako_task_gate",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::UnitTest(
            "acceptance_gates のゲート登録テストが実機で緑（落ちているのは execute_command の 5 件だけ）",
        ),
    },
    Feature {
        key: "tako_task_gate_check",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_GATE_CHECK_SH,
        },
        windows_evidence: Evidence::UnitTest(
            "実機の cargo test で execute_command 系 5 件が失敗（sh 不在）。PR / custom ゲートの判定は動く",
        ),
    },
    Feature {
        key: "tako_task_gate_show",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::UnitTest(
            "acceptance_gates の表示テストが実機で緑",
        ),
    },
    Feature {
        key: "tako_task_list",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako task list --json` が保存したチェックポイントを issue / branch / project / prompt_head / phase つきで返す",
        ),
    },
    Feature {
        key: "tako_task_resume",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako task resume <id> --tab <t>` が PowerShell 方言の env 前置き（`$env:TAKO_ORCHESTRATOR_ROLE=…; claude …`）でペインを立てる",
        ),
    },
    Feature {
        key: "tako_telemetry",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: `tako telemetry status` → `on` → `status`（true）→ `off` → `status`（false）の往復",
        ),
    },
    Feature {
        key: "tako_theme",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 33b（MCP tako_theme: light 適用 → GUI 反映 → toggle）",
        ),
    },
    Feature {
        key: "tako_tmux_cleanup",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: 器へ孤児セッションを 1 つ作ると `tako tmux cleanup` が killed=[tako-orphan937] を返し、使用中の 5 セッションには触らない",
        ),
    },
    Feature {
        key: "tako_tmux_kill",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#866 の製品経路 A/B: 項目 48 で対象だけが消え、隣の tako-test2 が残ることまで実測",
        ),
    },
    Feature {
        key: "tako_tmux_list",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#866 の製品経路 A/B: 項目 48 が既定で通過（TAKO_866_KEEP_EXACT_TARGET=1 では FAILED）",
        ),
    },
    Feature {
        key: "tako_tmux_open",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TMUX_ATTACH,
            issue: 519,
        },
        windows_evidence: Evidence::Measured(
            "セルフテスト項目 68 / 73 は attach / send-keys 前提のため psmux ではスキップ",
        ),
    },
    Feature {
        key: "tako_tmux_resize",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_TMUX_RESIZE,
            issue: 519,
        },
        windows_evidence: Evidence::Measured(
            "psmux が -x / -y を受け取っても反映しないことを #866 の調査で確認",
        ),
    },
    Feature {
        key: "tako_tmux_select_window",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::Measured(
            "#937 の Windows 11 実測: 2 つ目の window を作ってから `tako tmux select-window 0 / 1 --pane 1` でアクティブ window が実際に切り替わる",
        ),
    },
    Feature {
        key: "tako_tree_folder",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 67 / 85（タブ = ワークスペース・TreeFolder 経由の git セクション）",
        ),
    },
    Feature {
        // #694 / #739: 表示レイヤだけの切替で、モードトグルとスターター
        // （プロファイル選択 ▾ を含む）は GUI 本体（#517）が動けばそのまま動く。
        // チャットビューだけは永続バックエンドにも依存するが、それは
        // `tako_chat_copy`（WIN_GUI_CHAT）側で追跡する
        key: "tako_ui_mode",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_STARTER_INJECTION,
        },
        windows_evidence: Evidence::SelfTest(
            "項目 93 / 94 / 97 / 100 / 114 / 115（G1 スターター〜チャット表示と仮想化は緑）。スターターのボタンの投入経路は main が LF + POSIX クォート決め打ちのまま（#899）",
        ),
    },
    Feature {
        key: "tako_update",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_UPDATE_APPLY_UNVERIFIED,
        },
        windows_evidence: Evidence::SelfTest(
            "項目 90（更新画面と Markdown のリリースノート）+ #587 / #723 で実機の配布物生成とバージョン解析を実測。適用そのものは未実測",
        ),
    },
    Feature {
        key: "tako_video_playback",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_VIDEO,
            issue: 521,
        },
        windows_evidence: Evidence::Measured(
            "video_player.rs の非 macOS 実装が Err(\"動画再生は macOS でのみ対応\") を返すスタブ",
        ),
    },
    Feature {
        key: "tako_video_seek",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_VIDEO,
            issue: 521,
        },
        windows_evidence: Evidence::Measured(
            "video_player.rs の非 macOS 実装が Err を返すスタブ",
        ),
    },
    Feature {
        key: "tako_video_volume",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_VIDEO,
            issue: 521,
        },
        windows_evidence: Evidence::Measured(
            "video_player.rs の非 macOS 実装が Err を返すスタブ",
        ),
    },
    Feature {
        key: "tako_web",
        macos: Support::Supported,
        windows: Support::Pending {
            note: notes::WIN_WEBVIEW2_PANIC,
            issue: 724,
        },
        windows_evidence: Evidence::Measured(
            "セルフテスト項目 71 は WebView2 の非巻き戻し panic（wry/src/webview2/mod.rs:910）でアプリごと落ちるためスキップ",
        ),
    },
    Feature {
        key: "tako_welcome",
        macos: Support::Supported,
        windows: Support::Degraded {
            note: notes::WIN_WELCOME_INJECTION,
        },
        windows_evidence: Evidence::SelfTest(
            "項目 88（初回起動バナーと案内コマンドの取得は緑）。ボタンの投入経路は main が LF + POSIX クォート決め打ちのまま（#899）",
        ),
    },
    Feature {
        key: "tako_window",
        macos: Support::Supported,
        windows: Support::Supported,
        windows_evidence: Evidence::SelfTest(
            "項目 77（window new → move-tab）+ #872 で 0 枚化の寿命を Windows 向けに実装（項目 79b）",
        ),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{self, Lang};

    /// T7: **根拠なき判定を構造的に禁止する**（#591）。
    ///
    /// Windows の宣言は `PlatformFacts` 経由で system prompt へ流れる（#516）。
    /// 甘い宣言は「使える」と信じたエージェントを失敗させ、辛い宣言は使える機能を
    /// 回避させる。どちらも実害があるので、**未実測のまま「動く」と言えない**ことを
    /// 型とテストで縛る。実測できたら `Pending` を外すと同時に根拠を書く
    #[test]
    fn t7_windowsの判定には実測根拠が要る() {
        for f in MATRIX {
            match f.windows {
                Support::Supported | Support::Degraded { .. } | Support::Unsupported { .. } => {
                    assert!(
                        f.windows_evidence != Evidence::Unverified,
                        "{} は Windows で「使える / 使えないと分かっている」と宣言しているのに根拠が無い。\
                         実機セルフテストの項目・実機で緑のテスト名・実測の記録のどれかを \
                         windows_evidence へ書くこと（書けないなら Pending のままにする）",
                        f.key
                    );
                }
                // Pending は「未実装」か「未実測」。根拠があってもなくてもよい
                Support::Pending { .. } => {}
            }
            if let Some(citation) = f.windows_evidence.citation() {
                assert!(!citation.trim().is_empty(), "{} の根拠が空文字", f.key);
            }
        }
    }

    /// T7 の対: **未実測を Pending 以外へ倒す退行**は上で落ちるが、
    /// 逆に「根拠を書いたのに Pending のまま」も棚卸し漏れなので数えておく。
    /// 落とさずに件数だけ固定するのは、実測が先行してもよいから
    /// （実測 → 宣言更新の順で 2 回に分けて入れられる）
    #[test]
    fn t7_未実測のpendingは追跡issueを持つ() {
        for f in MATRIX {
            if f.windows_evidence == Evidence::Unverified {
                let Support::Pending { issue, .. } = f.windows else {
                    continue; // 上のテストが落とす
                };
                assert!(issue != 0, "{} は未実測なのに追跡 Issue が無い", f.key);
            }
        }
    }

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
