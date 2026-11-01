//! GUI 側の口（ControlHost の実装）が自分で組む断りの文（#2365）。
//!
//! dispatch は `host.*` の `Err(String)` を `Operation` / `InvalidParams` の中身として**そのまま**載せる。
//! 以前は GUI（tako-app の `impl *Host for TakoApp`）がその文を日本語で固定していたので、英語表示でも
//! CLI / MCP の応答の理由だけ日本語で返った。ここに日英の対を 1 か所に置き、GUI は
//! `HostRefusal::…` を返す（`String` への変換 = `Display` は組んだその場の表示言語）。
//! 口の既定の実装（`host.rs` の `Default*`。GUI の居ないホスト = `examples/mcp_host` だけに届く）も同じ。
//!
//! 形は [`crate::Reason`]（dispatch が組む理由の文）と同じ: (バリアント, 言語) の網羅 match なので、
//! バリアントを足すと日英の両方を書くまでコンパイルが通らない。日本語は #2365 より前の固定文と
//! バイト一致（`tests/issue1762_dispatch_error_lang.rs` が旧文の表と突き合わせる）。
//! 「見つからない」系は英語にも ` not found` を含め、[`crate::DispatchError::says_not_found`] の
//! 当たり方を日英でそろえる（リモートの HTTP 層が 404 へ寄せる判定）。
//!
//! 中身に他モジュールの文を載せる断り（更新の確認の失敗 = `update_checker::CheckError` ほか）は、
//! 枠だけ移すと 1 つの断りに日英が混ざるので、中身と一緒に移すまでここへ載せない（Issue #2365 の表）

use tako_core::i18n::Lang;

/// GUI 側の口が返す断りの文（#2365）。組み立ては GUI、日英の対はここ 1 か所
#[derive(Debug, Clone, PartialEq)]
pub enum HostRefusal {
    // ---- プレビュー（表示・行・アウトライン・リンク・コードブロック・ズーム） ----
    /// `プレビューペインではない`
    NotPreviewPane,
    /// `ズーム操作は PDF・画像プレビューだけに対応する`
    ZoomNeedsPdfOrImage,
    /// `ページ範囲外: <page>（全 <total> ページ）`
    PageOutOfRange { page: usize, total: usize },
    /// `画像プレビューの page は 1 だけ指定できる`
    ImagePageOnlyOne,
    /// `テキストとして行を数えられない`
    LinesUncountable,
    /// `<line> 行目は無い（全 <count> 行）`（line は 1 始まり）
    LineMissing { line: usize, count: usize },
    /// `コード表示のプレビューではない（Markdown のレンダリング表示・画像・PDF には原文の行が無い）`
    NotCodePreview,
    /// `Markdown アウトラインの対象ではない`
    NotMarkdownOutline,
    /// `PDF アウトラインの対象ではない`
    NotPdfOutline,
    /// `Markdown プレビューではない`
    NotMarkdownPreview,
    /// `PDF プレビューではない`
    NotPdfPreview,
    /// `リンクインデックス範囲外: <index>（全 <total> 件）`（total が無ければ括弧ごと無い）
    LinkIndexOutOfRange { index: usize, total: Option<usize> },
    /// `ブラウザで開けないリンク（http / https のみ対応）: <link>`（link は URL か理由の分類）
    LinkNotBrowsable(String),
    /// `コードブロックが無い`
    NoCodeBlock,
    /// `コードブロック範囲外: <index>（全 <total> 個。0 始まり）`
    CodeBlockOutOfRange { index: usize, total: usize },

    // ---- 編集・検索欄・変更履歴 ----
    /// `編集モードを開始していない`
    EditNotStarted,
    /// `編集モードが無効`
    EditingOff,
    /// `テキスト以外のプレビューは編集できない`
    NotTextForEdit,
    /// `バイナリファイルは編集できない`
    BinaryNotEditable,
    /// `テキスト以外のプレビュー（PDF・画像・動画）は検索欄を開けない`
    SearchBarNeedsText,
    /// `チェンジログビューが有効ではない`
    ChangelogOff,
    /// `コミット <hash> が見つからない`
    CommitNotFound(String),

    // ---- 動画 ----
    /// `対象ペインにプレビューが無い`
    NoPreviewInPane,
    /// `動画プレイヤーが起動していない`
    VideoPlayerNotRunning,
    /// `不正な速度値: <value>`
    BadRate(String),
    /// `速度は 0.1〜4.0 の範囲: <rate>`
    RateOutOfRange(f32),
    /// `不明なアクション: <action>`
    UnknownVideoAction(String),

    // ---- Web ビュー ----
    /// `Web ビュー <id> が見つからない`（`list_hint` なら `（web list で確認）` を添える）
    WebViewNotFound { id: u64, list_hint: bool },
    /// `Web ビュー <id> は表示中`
    WebViewShown(u64),
    /// `ペイン <pane> に Web ビューは表示されていない`
    NoWebViewInPane(u64),
    /// `表示中の Web ビューが無い（id か pane で指定）`
    NoShownWebView,
    /// `表示中の Web ビューが <n> 個ある（id か pane で指定）`
    ManyShownWebViews(usize),
    /// `ウィンドウ初期化前のため Web ビューを作れない（直後に再試行）`
    WindowNotReady,
    /// `Web ビュー <id> はまだ開いていない（state = <state>。…）`（#724。wry の Web ビューがまだ無い。
    /// `state` は `web list` の `state` = queued / creating / held / failed）
    WebViewNotReady { id: u64, state: String },
    /// `back 失敗: <cause>`（cause は wry のエラー文）
    WebBackFailed(String),
    /// `forward 失敗: <cause>`
    WebForwardFailed(String),
    /// `reload 失敗: <cause>`
    WebReloadFailed(String),
    /// `URL 読み込み失敗: <cause>`
    WebLoadFailed(String),
    /// `eval 失敗: <cause>`
    WebEvalFailed(String),
    /// `URL が空です`（Web ビューの URL の検査 = `webview::validate_url`。dispatch の `Reason::UrlEmpty` と同じ文）
    UrlEmpty,
    /// `URL に空白文字が含まれています（NSURL が解釈できない）: <url>`
    UrlHasWhitespace(String),
    /// `URL に制御文字が含まれています（NSURL が解釈できない）: <url>`
    UrlHasControlChar(String),
    /// `Web ビューの親（tako の窓）が閉じているので Web ビューを作れない。…（#1837）`
    /// （GUI は persist.log へは日本語の文で残す = 診断）
    WebViewParentDetached,

    // ---- 動画のプレビュー ----
    /// 動画プレイヤーを用意できない（`video_player::VideoPlayer::open` の断り。枠を持たずに中身をそのまま）
    VideoOpen(VideoRefusal),

    // ---- 設定の保存 ----
    /// `設定を保存できない: <cause>`（中身も枠と同じ表示言語 = [`crate::config_io::IoCause`]）
    SettingsNotSaved(crate::config_io::IoCause),

    // ---- 器（tmux）の再 attach ----
    /// `ペイン <pane> は tmux のセッションを持たない（再 attach できるのは tmux 永続化のペインだけ）`
    NoBackendSession(u64),
    /// `ペイン <pane> の attach クライアントは動いている（再 attach は不要）`
    AttachClientAlive(u64),
    /// `ペイン <pane> のセッション <name> は終わっている（再 attach できない。ペインは閉じてよい）`
    BackendSessionEnded { pane: u64, name: String },

    // ---- 貼り付けの確認・クリック・チャット・ファイルツリー ----
    /// `確認待ちの貼り付けが無い`
    NoPendingPaste,
    /// `送り先のペイン <pane> が閉じている（貼り付けは捨てた）`
    PasteTargetClosed(u64),
    /// `ペイン <pane> は端末ではない`
    NotTerminalPane(u64),
    /// `チャット表示のペインではない`
    NotChatPane,
    /// `会話がまだ読めていない`
    ChatNotLoaded,
    /// `assistant の発話がまだ無い（--message で指定できる）`
    NoAssistantMessage,
    /// `メッセージ範囲外: <index>（全 <total> 件。0 始まり）`
    MessageOutOfRange { index: usize, total: usize },
    /// `その発話はもう表示されていない`
    MessageGone,
    /// `この発話にコードブロックが無い`
    NoCodeBlockInMessage,
    /// `コピーする本文が無い`
    NothingToCopy,
    /// `ファイルツリーが閉じている`
    FileTreeClosed,

    // ---- 口の既定の実装（`host.rs`。GUI の居ないホスト = `examples/mcp_host` だけに届く） ----
    /// `このホストはクリックを渡せない`
    DefaultNoClick,
    /// `このホストはファイルツリーを持たない`
    DefaultNoFileTree,
    /// `tako アプリ（GUI）が居ないので再 attach できない`
    DefaultReattachNeedsGui,
    /// `チャットビューのコピーは未対応`
    DefaultChatCopy,
    /// `ボタンを押す GUI が居ない`
    DefaultWelcomeNeedsGui,
    /// `PDF・画像プレビューのズームは未対応`
    DefaultZoom,
    /// `行ジャンプは未対応`
    DefaultLineJump,
    /// `定義ジャンプは未対応`
    DefaultGotoDefinition,
    /// `整形は未対応`（整形の結果を当てる口も同じ文）
    DefaultFormat,
    /// `アウトラインナビゲーションは未対応`
    DefaultOutline,
    /// `PDF リンクフォローは未対応`
    DefaultPdfLink,
    /// `Markdown リンクフォローは未対応`
    DefaultMarkdownLink,
    /// `コードブロックのコピーは未対応`
    DefaultCodeBlockCopy,
    /// `プレビュー編集は未対応`
    DefaultPreviewEdit,
    /// `チェンジログビューは未対応`
    DefaultChangelog,
    /// `このホストは OS のクリップボードを使わない`
    DefaultNoOsClipboard,
    /// `動画再生は未対応`
    DefaultVideo,
    /// `Web ビューはこの環境では使えない`
    DefaultWebView,
    /// `リモートアクセス API はこの環境では使えない`
    DefaultRemoteApi,
    /// `リモートアクセス API サーバーが起動していない`
    DefaultRemoteApiNotRunning,
    /// `この環境では更新を実行できない`
    DefaultUpdate,
    /// `この環境では修復を実行できない`
    DefaultRepair,
    /// `保存時整形の設定は未対応`
    DefaultFormatOnSave,
    /// `括弧の自動閉じの設定は未対応`
    DefaultAutoClose,
    /// `この tako は mod を扱わない`
    DefaultNoMod,
}

impl HostRefusal {
    /// 表示言語を明示しての文言（#2365）。
    ///
    /// (バリアント, 言語) の網羅 match なので、バリアントを足すと日英の両方を書くまで
    /// コンパイルが通らない。日本語は #2365 より前の固定文とバイト一致
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::NotPreviewPane, Ja) => "プレビューペインではない".into(),
            (Self::NotPreviewPane, En) => "not a preview pane".into(),
            (Self::ZoomNeedsPdfOrImage, Ja) => {
                "ズーム操作は PDF・画像プレビューだけに対応する".into()
            }
            (Self::ZoomNeedsPdfOrImage, En) => "zoom works only in PDF and image previews".into(),
            (Self::PageOutOfRange { page, total }, Ja) => {
                format!("ページ範囲外: {page}（全 {total} ページ）")
            }
            (Self::PageOutOfRange { page, total }, En) => {
                format!("page out of range: {page} (total pages: {total})")
            }
            (Self::ImagePageOnlyOne, Ja) => "画像プレビューの page は 1 だけ指定できる".into(),
            (Self::ImagePageOnlyOne, En) => "an image preview accepts only page 1".into(),
            (Self::LinesUncountable, Ja) => "テキストとして行を数えられない".into(),
            (Self::LinesUncountable, En) => "cannot count lines as text".into(),
            (Self::LineMissing { line, count }, Ja) => {
                format!("{line} 行目は無い（全 {count} 行）")
            }
            (Self::LineMissing { line, count }, En) => {
                format!("line {line} does not exist (total lines: {count})")
            }
            (Self::NotCodePreview, Ja) => {
                "コード表示のプレビューではない\
                 （Markdown のレンダリング表示・画像・PDF には原文の行が無い）"
                    .into()
            }
            (Self::NotCodePreview, En) => {
                "not a code preview (rendered Markdown, images and PDFs have no source lines)"
                    .into()
            }
            (Self::NotMarkdownOutline, Ja) => "Markdown アウトラインの対象ではない".into(),
            (Self::NotMarkdownOutline, En) => "not a Markdown outline target".into(),
            (Self::NotPdfOutline, Ja) => "PDF アウトラインの対象ではない".into(),
            (Self::NotPdfOutline, En) => "not a PDF outline target".into(),
            (Self::NotMarkdownPreview, Ja) => "Markdown プレビューではない".into(),
            (Self::NotMarkdownPreview, En) => "not a Markdown preview".into(),
            (Self::NotPdfPreview, Ja) => "PDF プレビューではない".into(),
            (Self::NotPdfPreview, En) => "not a PDF preview".into(),
            (Self::LinkIndexOutOfRange { index, total }, Ja) => match total {
                Some(total) => format!("リンクインデックス範囲外: {index}（全 {total} 件）"),
                None => format!("リンクインデックス範囲外: {index}"),
            },
            (Self::LinkIndexOutOfRange { index, total }, En) => match total {
                Some(total) => format!("link index out of range: {index} (total: {total})"),
                None => format!("link index out of range: {index}"),
            },
            (Self::LinkNotBrowsable(link), Ja) => {
                format!("ブラウザで開けないリンク（http / https のみ対応）: {link}")
            }
            (Self::LinkNotBrowsable(link), En) => {
                format!("cannot open the link in a browser (only http / https): {link}")
            }
            (Self::NoCodeBlock, Ja) => "コードブロックが無い".into(),
            (Self::NoCodeBlock, En) => "no code block".into(),
            (Self::CodeBlockOutOfRange { index, total }, Ja) => {
                format!("コードブロック範囲外: {index}（全 {total} 個。0 始まり）")
            }
            (Self::CodeBlockOutOfRange { index, total }, En) => {
                format!("code block out of range: {index} (total: {total}, 0-based)")
            }

            (Self::EditNotStarted, Ja) => "編集モードを開始していない".into(),
            (Self::EditNotStarted, En) => "edit mode has not been started".into(),
            (Self::EditingOff, Ja) => "編集モードが無効".into(),
            (Self::EditingOff, En) => "edit mode is off".into(),
            (Self::NotTextForEdit, Ja) => "テキスト以外のプレビューは編集できない".into(),
            (Self::NotTextForEdit, En) => "a non-text preview cannot be edited".into(),
            (Self::BinaryNotEditable, Ja) => "バイナリファイルは編集できない".into(),
            (Self::BinaryNotEditable, En) => "a binary file cannot be edited".into(),
            (Self::SearchBarNeedsText, Ja) => {
                "テキスト以外のプレビュー（PDF・画像・動画）は検索欄を開けない".into()
            }
            (Self::SearchBarNeedsText, En) => {
                "a non-text preview (PDF, image, video) cannot open the search bar".into()
            }
            (Self::ChangelogOff, Ja) => "チェンジログビューが有効ではない".into(),
            (Self::ChangelogOff, En) => "the changelog view is not on".into(),
            (Self::CommitNotFound(hash), Ja) => format!("コミット {hash} が見つからない"),
            (Self::CommitNotFound(hash), En) => format!("commit {hash} not found"),

            (Self::NoPreviewInPane, Ja) => "対象ペインにプレビューが無い".into(),
            (Self::NoPreviewInPane, En) => "the pane has no preview".into(),
            (Self::VideoPlayerNotRunning, Ja) => "動画プレイヤーが起動していない".into(),
            (Self::VideoPlayerNotRunning, En) => "the video player is not running".into(),
            (Self::BadRate(value), Ja) => format!("不正な速度値: {value}"),
            (Self::BadRate(value), En) => format!("invalid speed: {value}"),
            (Self::RateOutOfRange(rate), Ja) => format!("速度は 0.1〜4.0 の範囲: {rate}"),
            (Self::RateOutOfRange(rate), En) => {
                format!("speed must be between 0.1 and 4.0: {rate}")
            }
            (Self::UnknownVideoAction(action), Ja) => format!("不明なアクション: {action}"),
            (Self::UnknownVideoAction(action), En) => format!("unknown action: {action}"),

            (Self::WebViewNotFound { id, list_hint }, Ja) => match list_hint {
                true => format!("Web ビュー {id} が見つからない（web list で確認）"),
                false => format!("Web ビュー {id} が見つからない"),
            },
            (Self::WebViewNotFound { id, list_hint }, En) => match list_hint {
                true => format!("web view {id} not found (check with web list)"),
                false => format!("web view {id} not found"),
            },
            (Self::WebViewShown(id), Ja) => format!("Web ビュー {id} は表示中"),
            (Self::WebViewShown(id), En) => format!("web view {id} is already shown"),
            (Self::NoWebViewInPane(pane), Ja) => {
                format!("ペイン {pane} に Web ビューは表示されていない")
            }
            (Self::NoWebViewInPane(pane), En) => format!("pane {pane} shows no web view"),
            (Self::NoShownWebView, Ja) => "表示中の Web ビューが無い（id か pane で指定）".into(),
            (Self::NoShownWebView, En) => "no web view is shown (pass id or pane)".into(),
            (Self::ManyShownWebViews(n), Ja) => {
                format!("表示中の Web ビューが {n} 個ある（id か pane で指定）")
            }
            (Self::ManyShownWebViews(n), En) => {
                format!("{n} web views are shown (pass id or pane)")
            }
            (Self::WindowNotReady, Ja) => {
                "ウィンドウ初期化前のため Web ビューを作れない（直後に再試行）".into()
            }
            (Self::WindowNotReady, En) => {
                "cannot create a web view before the window is ready (retry shortly)".into()
            }
            (Self::WebViewNotReady { id, state }, Ja) => format!(
                "Web ビュー {id} はまだ開いていない（state = {state}。web list の state が ready になってから。\
                 held / failed は navigate で開き直す）"
            ),
            (Self::WebViewNotReady { id, state }, En) => format!(
                "Web view {id} is not open yet (state = {state}). Wait until its state in web list is ready; \
                 reopen a held or failed one with navigate"
            ),
            (Self::WebBackFailed(cause), Ja) => format!("back 失敗: {cause}"),
            (Self::WebBackFailed(cause), En) => format!("back failed: {cause}"),
            (Self::WebForwardFailed(cause), Ja) => format!("forward 失敗: {cause}"),
            (Self::WebForwardFailed(cause), En) => format!("forward failed: {cause}"),
            (Self::WebReloadFailed(cause), Ja) => format!("reload 失敗: {cause}"),
            (Self::WebReloadFailed(cause), En) => format!("reload failed: {cause}"),
            (Self::WebLoadFailed(cause), Ja) => format!("URL 読み込み失敗: {cause}"),
            (Self::WebLoadFailed(cause), En) => format!("failed to load the URL: {cause}"),
            (Self::WebEvalFailed(cause), Ja) => format!("eval 失敗: {cause}"),
            (Self::WebEvalFailed(cause), En) => format!("eval failed: {cause}"),
            (Self::UrlEmpty, Ja) => "URL が空です".into(),
            (Self::UrlEmpty, En) => "the URL is empty".into(),
            (Self::UrlHasWhitespace(url), Ja) => {
                format!("URL に空白文字が含まれています（NSURL が解釈できない）: {url}")
            }
            (Self::UrlHasWhitespace(url), En) => {
                format!("the URL contains whitespace (NSURL cannot parse it): {url}")
            }
            (Self::UrlHasControlChar(url), Ja) => {
                format!("URL に制御文字が含まれています（NSURL が解釈できない）: {url}")
            }
            (Self::UrlHasControlChar(url), En) => {
                format!("the URL contains control characters (NSURL cannot parse it): {url}")
            }
            (Self::WebViewParentDetached, Ja) => {
                "Web ビューの親（tako の窓）が閉じているので Web ビューを作れない。\
                 tako の窓を開いてから開き直す（#1837）"
                    .into()
            }
            (Self::WebViewParentDetached, En) => {
                "cannot create the web view because its parent (the tako window) is closed. \
                 Open a tako window and open it again (#1837)"
                    .into()
            }

            (Self::VideoOpen(cause), Ja | En) => cause.text_in(lang),

            (Self::SettingsNotSaved(cause), Ja) => {
                format!("設定を保存できない: {}", cause.text_in(Ja))
            }
            (Self::SettingsNotSaved(cause), En) => {
                format!("cannot save the settings: {}", cause.text_in(En))
            }

            (Self::NoBackendSession(pane), Ja) => format!(
                "ペイン {pane} は tmux のセッションを持たない\
                 （再 attach できるのは tmux 永続化のペインだけ）"
            ),
            (Self::NoBackendSession(pane), En) => format!(
                "pane {pane} has no tmux session (only panes kept by tmux persistence can be reattached)"
            ),
            (Self::AttachClientAlive(pane), Ja) => {
                format!("ペイン {pane} の attach クライアントは動いている（再 attach は不要）")
            }
            (Self::AttachClientAlive(pane), En) => {
                format!("the attach client of pane {pane} is running (no need to reattach)")
            }
            (Self::BackendSessionEnded { pane, name }, Ja) => format!(
                "ペイン {pane} のセッション {name} は終わっている\
                 （再 attach できない。ペインは閉じてよい）"
            ),
            (Self::BackendSessionEnded { pane, name }, En) => format!(
                "session {name} of pane {pane} has ended (cannot reattach; the pane can be closed)"
            ),

            (Self::NoPendingPaste, Ja) => "確認待ちの貼り付けが無い".into(),
            (Self::NoPendingPaste, En) => "no paste is waiting for confirmation".into(),
            (Self::PasteTargetClosed(pane), Ja) => {
                format!("送り先のペイン {pane} が閉じている（貼り付けは捨てた）")
            }
            (Self::PasteTargetClosed(pane), En) => {
                format!("the target pane {pane} is closed (the paste was discarded)")
            }
            (Self::NotTerminalPane(pane), Ja) => format!("ペイン {pane} は端末ではない"),
            (Self::NotTerminalPane(pane), En) => format!("pane {pane} is not a terminal"),
            (Self::NotChatPane, Ja) => "チャット表示のペインではない".into(),
            (Self::NotChatPane, En) => "not a chat view pane".into(),
            (Self::ChatNotLoaded, Ja) => "会話がまだ読めていない".into(),
            (Self::ChatNotLoaded, En) => "the conversation has not been read yet".into(),
            (Self::NoAssistantMessage, Ja) => {
                "assistant の発話がまだ無い（--message で指定できる）".into()
            }
            (Self::NoAssistantMessage, En) => {
                "no assistant message yet (pick one with --message)".into()
            }
            (Self::MessageOutOfRange { index, total }, Ja) => {
                format!("メッセージ範囲外: {index}（全 {total} 件。0 始まり）")
            }
            (Self::MessageOutOfRange { index, total }, En) => {
                format!("message out of range: {index} (total: {total}, 0-based)")
            }
            (Self::MessageGone, Ja) => "その発話はもう表示されていない".into(),
            (Self::MessageGone, En) => "that message is no longer shown".into(),
            (Self::NoCodeBlockInMessage, Ja) => "この発話にコードブロックが無い".into(),
            (Self::NoCodeBlockInMessage, En) => "this message has no code block".into(),
            (Self::NothingToCopy, Ja) => "コピーする本文が無い".into(),
            (Self::NothingToCopy, En) => "nothing to copy".into(),
            (Self::FileTreeClosed, Ja) => "ファイルツリーが閉じている".into(),
            (Self::FileTreeClosed, En) => "the file tree is closed".into(),

            (Self::DefaultNoClick, Ja) => "このホストはクリックを渡せない".into(),
            (Self::DefaultNoClick, En) => "this host cannot pass clicks".into(),
            (Self::DefaultNoFileTree, Ja) => "このホストはファイルツリーを持たない".into(),
            (Self::DefaultNoFileTree, En) => "this host has no file tree".into(),
            (Self::DefaultReattachNeedsGui, Ja) => {
                "tako アプリ（GUI）が居ないので再 attach できない".into()
            }
            (Self::DefaultReattachNeedsGui, En) => {
                "cannot reattach because the tako app (GUI) is not running".into()
            }
            (Self::DefaultChatCopy, Ja) => "チャットビューのコピーは未対応".into(),
            (Self::DefaultChatCopy, En) => "copying from the chat view is not supported".into(),
            (Self::DefaultWelcomeNeedsGui, Ja) => "ボタンを押す GUI が居ない".into(),
            (Self::DefaultWelcomeNeedsGui, En) => "no GUI is running to press the button".into(),
            (Self::DefaultZoom, Ja) => "PDF・画像プレビューのズームは未対応".into(),
            (Self::DefaultZoom, En) => "zooming PDF and image previews is not supported".into(),
            (Self::DefaultLineJump, Ja) => "行ジャンプは未対応".into(),
            (Self::DefaultLineJump, En) => "jumping to a line is not supported".into(),
            (Self::DefaultGotoDefinition, Ja) => "定義ジャンプは未対応".into(),
            (Self::DefaultGotoDefinition, En) => "jumping to a definition is not supported".into(),
            (Self::DefaultFormat, Ja) => "整形は未対応".into(),
            (Self::DefaultFormat, En) => "formatting is not supported".into(),
            (Self::DefaultOutline, Ja) => "アウトラインナビゲーションは未対応".into(),
            (Self::DefaultOutline, En) => "outline navigation is not supported".into(),
            (Self::DefaultPdfLink, Ja) => "PDF リンクフォローは未対応".into(),
            (Self::DefaultPdfLink, En) => "following PDF links is not supported".into(),
            (Self::DefaultMarkdownLink, Ja) => "Markdown リンクフォローは未対応".into(),
            (Self::DefaultMarkdownLink, En) => "following Markdown links is not supported".into(),
            (Self::DefaultCodeBlockCopy, Ja) => "コードブロックのコピーは未対応".into(),
            (Self::DefaultCodeBlockCopy, En) => "copying code blocks is not supported".into(),
            (Self::DefaultPreviewEdit, Ja) => "プレビュー編集は未対応".into(),
            (Self::DefaultPreviewEdit, En) => "editing previews is not supported".into(),
            (Self::DefaultChangelog, Ja) => "チェンジログビューは未対応".into(),
            (Self::DefaultChangelog, En) => "the changelog view is not supported".into(),
            (Self::DefaultNoOsClipboard, Ja) => "このホストは OS のクリップボードを使わない".into(),
            (Self::DefaultNoOsClipboard, En) => "this host does not use the OS clipboard".into(),
            (Self::DefaultVideo, Ja) => "動画再生は未対応".into(),
            (Self::DefaultVideo, En) => "video playback is not supported".into(),
            (Self::DefaultWebView, Ja) => "Web ビューはこの環境では使えない".into(),
            (Self::DefaultWebView, En) => "the web view is not available in this environment".into(),
            (Self::DefaultRemoteApi, Ja) => "リモートアクセス API はこの環境では使えない".into(),
            (Self::DefaultRemoteApi, En) => {
                "the remote access API is not available in this environment".into()
            }
            (Self::DefaultRemoteApiNotRunning, Ja) => {
                "リモートアクセス API サーバーが起動していない".into()
            }
            (Self::DefaultRemoteApiNotRunning, En) => {
                "the remote access API server is not running".into()
            }
            (Self::DefaultUpdate, Ja) => "この環境では更新を実行できない".into(),
            (Self::DefaultUpdate, En) => "updates cannot run in this environment".into(),
            (Self::DefaultRepair, Ja) => "この環境では修復を実行できない".into(),
            (Self::DefaultRepair, En) => "repair cannot run in this environment".into(),
            (Self::DefaultFormatOnSave, Ja) => "保存時整形の設定は未対応".into(),
            (Self::DefaultFormatOnSave, En) => "the format-on-save setting is not supported".into(),
            (Self::DefaultAutoClose, Ja) => "括弧の自動閉じの設定は未対応".into(),
            (Self::DefaultAutoClose, En) => {
                "the bracket auto-close setting is not supported".into()
            }
            (Self::DefaultNoMod, Ja) => "この tako は mod を扱わない".into(),
            (Self::DefaultNoMod, En) => "this tako does not handle mods".into(),
        }
    }
}

/// 動画プレイヤーを用意できない理由（#2365。tako-app の `video_player` が組む。macOS は AVFoundation の
/// 生成の失敗、その他の OS は未対応）。実装の `cfg` の外に置くので、Windows の文も macOS の単体テストで
/// 日英を検査できる。日本語は #2365 より前の固定文とバイト一致
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoRefusal {
    /// `パスが UTF-8 でない`
    PathNotUtf8,
    /// `CString 変換失敗`（Objective-C へ渡す文字列に NUL がある）
    NulInString,
    /// `<class> <selector> が null を返した`
    ObjcNull { class: String, selector: String },
    /// `URL を生成できない`
    UrlUnbuildable,
    /// `AVPlayerItem を生成できない`
    PlayerItemUnbuildable,
    /// `AVPlayerItemVideoOutput を生成できない`
    VideoOutputUnbuildable,
    /// `AVPlayer を生成できない`
    PlayerUnbuildable,
    /// `動画の読み込みに失敗した`（AVPlayerItem の status が failed）
    LoadFailed,
    /// `動画再生は macOS でのみ対応`
    MacOnly,
}

impl VideoRefusal {
    /// (バリアント, 言語) の網羅 match
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::PathNotUtf8, Ja) => "パスが UTF-8 でない".into(),
            (Self::PathNotUtf8, En) => "the path is not UTF-8".into(),
            (Self::NulInString, Ja) => "CString 変換失敗".into(),
            (Self::NulInString, En) => {
                "cannot convert the string to a C string (it contains NUL)".into()
            }
            (Self::ObjcNull { class, selector }, Ja) => {
                format!("{class} {selector} が null を返した")
            }
            (Self::ObjcNull { class, selector }, En) => format!("{class} {selector} returned null"),
            (Self::UrlUnbuildable, Ja) => "URL を生成できない".into(),
            (Self::UrlUnbuildable, En) => "cannot create the file URL".into(),
            (Self::PlayerItemUnbuildable, Ja) => "AVPlayerItem を生成できない".into(),
            (Self::PlayerItemUnbuildable, En) => "cannot create the AVPlayerItem".into(),
            (Self::VideoOutputUnbuildable, Ja) => "AVPlayerItemVideoOutput を生成できない".into(),
            (Self::VideoOutputUnbuildable, En) => {
                "cannot create the AVPlayerItemVideoOutput".into()
            }
            (Self::PlayerUnbuildable, Ja) => "AVPlayer を生成できない".into(),
            (Self::PlayerUnbuildable, En) => "cannot create the AVPlayer".into(),
            (Self::LoadFailed, Ja) => "動画の読み込みに失敗した".into(),
            (Self::LoadFailed, En) => "failed to load the video".into(),
            (Self::MacOnly, Ja) => "動画再生は macOS でのみ対応".into(),
            (Self::MacOnly, En) => "video playback is supported only on macOS".into(),
        }
    }
}

impl std::fmt::Display for HostRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 表示言語は ui_text の `tr!` と同じグローバル（tako_core::i18n）。読むのは組んだその場の 1 回
        f.write_str(&self.text_in(tako_core::i18n::lang()))
    }
}

impl std::error::Error for HostRefusal {}

/// ControlHost の口は `Result<_, String>` なので、GUI は `?` / `.into()` でそのまま返せる
/// （文はその場の表示言語）
impl From<HostRefusal> for String {
    fn from(r: HostRefusal) -> Self {
        r.to_string()
    }
}
