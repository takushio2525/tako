//! `tako lsp` の応答に載る日英の文（#1678）
//!
//! 文言の正本はここ 1 か所（#435 の日英カタログの作法）。テストはこの定数を読んで
//! 比べ、理由文を直書きしない。`{…}` は呼び手が差し込む値で、表の値（サーバ名・
//! 導入コマンド）はここに書かない（言語の追加が表への行追加だけで済むように）。

use tako_core::platform::support::Note;

/// 未導入の理由。`{program}` = 探した実行ファイル名
pub const NOT_INSTALLED_REASON: Note = Note::new(
    "{program} が見つからない（ログインシェルの PATH にも無い）",
    "{program} was not found (not on the login shell PATH)",
);

/// 差し替えの環境変数が実行できないものを指している。`{env}` / `{path}`
pub const OVERRIDE_INVALID_REASON: Note = Note::new(
    "{env} が指す {path} は実行できるファイルではない",
    "{path} set in {env} is not an executable file",
);

/// 未導入のときの次の一手。`{command}` = 導入コマンド
pub const NOT_INSTALLED_NEXT_STEP: Note = Note::new(
    "導入する: {command}（入れたら tako lsp restart）",
    "Install it: {command} (then run tako lsp restart)",
);

/// 諦めた理由。`{count}` = 落ちた回数
pub const GAVE_UP_REASON: Note = Note::new(
    "{count} 回続けて落ちたので起こすのをやめた",
    "Stopped restarting after {count} crashes in a row",
);

/// 諦めたときの次の一手
pub const GAVE_UP_NEXT_STEP: Note = Note::new(
    "原因を tako lsp logs で見てから tako lsp restart で起こし直す",
    "Check tako lsp logs, then run tako lsp restart",
);

/// 利用者が止めた
pub const STOPPED_REASON: Note = Note::new("tako lsp stop で止めた", "Stopped by tako lsp stop");

/// 止めたときの次の一手
pub const STOPPED_NEXT_STEP: Note = Note::new(
    "再開するには tako lsp restart",
    "Run tako lsp restart to resume",
);

/// `TAKO_1007_LEGACY=1` で丸ごと止めている
pub const DISABLED_REASON: Note = Note::new(
    "TAKO_1007_LEGACY=1 で LSP を止めている（編集は LSP 無しで動く）",
    "LSP is disabled by TAKO_1007_LEGACY=1 (editing works without it)",
);

/// まだ 1 つも起きていないときの案内
pub const IDLE_NOTE: Note = Note::new(
    "対応する言語のファイルを編集モードで開くと、その言語のサーバが起きる（一覧は tako lsp servers）",
    "A language server starts when you edit a supported file (see tako lsp servers)",
);

/// stderr の直近の行を返すときの注記
pub const STDERR_NOTE: Note = Note::new(
    "サーバが stderr へ出した直近の行（診断用。ソースコードの断片を含みうる。メモリに保つだけでファイルへは書かない）",
    "Recent stderr lines from the server (diagnostic output; may contain source snippets; kept in memory only)",
);

/// この環境（テストの host 等）に LSP が無い
pub const UNAVAILABLE: Note = Note::new(
    "この環境では LSP を使えない",
    "LSP is not available in this environment",
);

/// 診断を問われたペインが編集モードでない（#1679）
pub const NOT_EDITING_REASON: Note = Note::new(
    "編集モードではない（言語サーバは編集モードに入ったときに起きる）",
    "Not in edit mode (a language server starts when you enter edit mode)",
);

/// 編集モードでないときの次の一手。`{pane}` = ペイン ID
pub const NOT_EDITING_NEXT_STEP: Note = Note::new(
    "tako edit start --pane {pane} で編集モードにする",
    "Run tako edit start --pane {pane} to enter edit mode",
);

/// 編集中だが言語サーバとつながっていない（#1679）
pub const NOT_LINKED_REASON: Note = Note::new(
    "受け持つ言語サーバが無い・未導入・同じファイルを別のペインが編集している",
    "No language server handles this file, it is not installed, or another pane is editing the same file",
);

/// つながっていないときの次の一手
pub const NOT_LINKED_NEXT_STEP: Note = Note::new(
    "理由は tako lsp status と tako lsp servers で見る",
    "See tako lsp status and tako lsp servers for the reason",
);

/// つながっている文書が 1 つも無い（#1679）
pub const NO_DOCUMENTS_NOTE: Note = Note::new(
    "言語サーバにつながった文書が無い（対応する言語のファイルを編集モードで開くと診断が出る）",
    "No document is connected to a language server (edit a supported file to see diagnostics)",
);

/// `{…}` を差し込む
pub fn fill(note: Note, values: &[(&str, &str)]) -> String {
    let mut text = note.text().to_string();
    for (key, value) in values {
        text = text.replace(&format!("{{{key}}}"), value);
    }
    text
}
