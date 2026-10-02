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

/// 解決のログインシェルを打ち切った（#1769。見つからないのではなく、確かめられなかった）
pub const RESOLVE_TIMEOUT_REASON: Note = Note::new(
    "{program} を探すログインシェルが {secs} 秒で返らなかったので打ち切った",
    "The login shell looking up {program} did not return within {secs}s, so it was stopped",
);

/// 打ち切ったときの次の一手
pub const RESOLVE_TIMEOUT_NEXT_STEP: Note = Note::new(
    "ログインシェルの設定（~/.zprofile 等）が入力を待っていないか確かめ、tako lsp restart で探し直す",
    "Check that your login shell profile (e.g. ~/.zprofile) is not waiting for input, then run tako lsp restart",
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
    "受け持つ言語サーバが無い・未導入",
    "No language server handles this file, or it is not installed",
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

// --- 定義ジャンプ（#1680）-------------------------------------------------------
// 「見つからない」「未応答」「未導入」を**区別して**出す（#983。無言で死なない）。
// 未導入は上の NOT_INSTALLED_* をそのまま使う（言語サーバの状態として同じもの）

/// ジャンプの種類の呼び名（`tako_core::lsp::goto::GotoKind::ALL` と同じ順）
pub const GOTO_KIND_LABELS: [Note; 4] = [
    Note::new("定義", "definition"),
    Note::new("宣言", "declaration"),
    Note::new("型の定義", "type definition"),
    Note::new("実装", "implementation"),
];

/// 見つからない（サーバは答えたが場所が 0 件）。`{kind}` = 種類の呼び名
pub const GOTO_NOT_FOUND_REASON: Note = Note::new(
    "{kind}が見つからなかった（言語サーバはこの位置に{kind}を返さなかった）",
    "No {kind} found (the language server returned none for this position)",
);

/// 見つからないときの次の一手
pub const GOTO_NOT_FOUND_NEXT_STEP: Note = Note::new(
    "識別子の上で試す（コメント・文字列の中の語や、読み込み中のプロジェクトでは見つからないことがある）",
    "Try on an identifier (words in comments or strings, or a project still loading, may have none)",
);

/// 未応答（問い合わせの上限を超えた）。`{server}` / `{secs}`
pub const GOTO_TIMEOUT_REASON: Note = Note::new(
    "{server} が {secs} 秒以内に応答しなかった",
    "{server} did not answer within {secs} seconds",
);

/// 未応答（起動と握手が上限までに済まなかった）。`{server}` / `{secs}`
pub const GOTO_START_TIMEOUT_REASON: Note = Note::new(
    "{server} の起動が {secs} 秒以内に終わらなかった（プロジェクトの読み込み中の可能性）",
    "{server} did not finish starting within {secs} seconds (the project may still be loading)",
);

/// 未応答のときの次の一手
pub const GOTO_TIMEOUT_NEXT_STEP: Note = Note::new(
    "少し待ってからもう一度試す。続くなら tako lsp logs で様子を見る",
    "Wait a moment and try again; if it keeps happening, check tako lsp logs",
);

/// この種類のファイルを受け持つサーバが検出表に無い
pub const GOTO_NO_SERVER_REASON: Note = Note::new(
    "この種類のファイルを受け持つ言語サーバが無い",
    "No language server handles this kind of file",
);

/// 受け持つサーバが無いときの次の一手
pub const GOTO_NO_SERVER_NEXT_STEP: Note = Note::new(
    "対応している拡張子は tako lsp servers で見る",
    "See tako lsp servers for the supported extensions",
);

/// サーバがその種類のジャンプに対応していない。`{server}` / `{kind}`
pub const GOTO_UNSUPPORTED_REASON: Note = Note::new(
    "{server} は{kind}へのジャンプに対応していない",
    "{server} does not support going to the {kind}",
);

/// 対応していないときの次の一手
pub const GOTO_UNSUPPORTED_NEXT_STEP: Note = Note::new(
    "サーバの能力（capabilities）は tako lsp status で見る",
    "See the server capabilities in tako lsp status",
);

/// 問い合わせの途中でサーバが落ちた。`{server}`
pub const GOTO_CRASHED_REASON: Note = Note::new(
    "問い合わせの途中で {server} が落ちた（自動で起こし直す）",
    "{server} exited while answering (it will be restarted automatically)",
);

/// 落ちたときの次の一手
pub const GOTO_CRASHED_NEXT_STEP: Note = Note::new(
    "原因は tako lsp logs で見る。起き直したらもう一度試す",
    "See tako lsp logs for the cause, then try again once it is back",
);

/// 問い合わせの途中で文書が閉じられた（編集していたペインを閉じた等）
pub const GOTO_CLOSED_REASON: Note = Note::new(
    "問い合わせの途中で文書が閉じられた",
    "The document was closed while waiting for the answer",
);

/// 文書が閉じられたときの次の一手
pub const GOTO_RETRY_NEXT_STEP: Note = Note::new("もう一度試す", "Try again");

/// サーバがエラーで答えた。`{server}` / `{code}` / `{detail}`
pub const GOTO_SERVER_ERROR_REASON: Note = Note::new(
    "{server} がエラーを返した（{code}）: {detail}",
    "{server} returned an error ({code}): {detail}",
);

/// ファイルを読めず、言語サーバへ渡せない。`{error}`
pub const GOTO_UNREADABLE_REASON: Note = Note::new(
    "ファイルを読めない（{error}）",
    "Cannot read the file ({error})",
);

/// 候補が複数ある（CLI / MCP の次の一手。GUI は一覧を出す）。`{count}`
pub const GOTO_CHOOSE_NEXT_STEP: Note = Note::new(
    "候補が {count} か所ある。同じ引数に --choice N（1 始まり）を足すとその場所へ飛ぶ",
    "There are {count} candidates; add --choice N (1-based) to the same arguments to go there",
);

/// 問い合わせたペインが待っているあいだに閉じられた / 別のファイルへ差し替わった
pub const GOTO_SOURCE_GONE_REASON: Note = Note::new(
    "問い合わせたペインが閉じられたか、別のファイルへ差し替わった",
    "The pane that asked was closed or now shows another file",
);

// --- 整形（#1683）---------------------------------------------------------------
// 起動・未導入・未応答・落ちた等は定義ジャンプと同じ文（言語サーバの状態として同じもの）

/// 整形した。`{count}` = 書き換えた箇所の数
pub const FORMAT_DONE_NOTE: Note = Note::new(
    "整形した（{count} か所。undo 1 回で戻せる）",
    "Formatted ({count} changes; one undo reverts it)",
);

/// 変える所が無かった。**「整形済み」と言い切らない**: rust-analyzer は rustfmt が読めない
/// （構文エラー）ときも空で答えるので、整形済みと区別できない（実測 #1683。診断の publish は
/// 整形の答えより遅れうるので、エラーの数が 0 でも構文エラーがありうる）
pub const FORMAT_UNCHANGED_NOTE: Note = Note::new(
    "変える所が無かった（構文エラーがあるとサーバは整形しないことがある）",
    "Nothing changed (servers often skip formatting when the code has syntax errors)",
);

/// 変える所が無かったが、エラーの診断がある。`{count}` = エラーの数
pub const FORMAT_UNCHANGED_ERRORS_NOTE: Note = Note::new(
    "変える所が無かった。エラーの診断が {count} 件あり、構文エラーがあるとサーバは整形しないことがある（tako lsp diagnostics）",
    "Nothing changed. There are {count} error diagnostics; servers often skip formatting when the code has syntax errors (tako lsp diagnostics)",
);

/// 範囲の外の空白だけの書き換えを捨てた。`{count}`
pub const FORMAT_DROPPED_NOTE: Note = Note::new(
    "範囲の外にかかる空白だけの書き換え {count} か所は当てなかった（範囲の外は変えない）",
    "Skipped {count} whitespace-only changes outside the range (nothing outside the range changes)",
);

/// サーバが整形に対応していない。`{server}`
pub const FORMAT_UNSUPPORTED_REASON: Note = Note::new(
    "{server} は整形に対応していない",
    "{server} does not support formatting",
);

/// サーバが範囲の整形に対応していない。`{server}`
pub const FORMAT_RANGE_UNSUPPORTED_REASON: Note = Note::new(
    "{server} は範囲の整形に対応していない",
    "{server} does not support formatting a range",
);

/// 範囲の整形に対応していないときの次の一手
pub const FORMAT_RANGE_UNSUPPORTED_NEXT_STEP: Note = Note::new(
    "範囲を付けずに文書全体を整形する: tako lsp format",
    "Format the whole document instead: tako lsp format",
);

/// 待つあいだに本文が変わった
pub const FORMAT_STALE_REASON: Note = Note::new(
    "整形の答えを待つあいだに本文が変わったので当てなかった（古い本文への答えは位置がずれる）",
    "The document changed while waiting for the formatter, so nothing was applied",
);

/// サーバの答えを当てられない（重なり・範囲外・形が違う）。`{server}` / `{detail}`
pub const FORMAT_INVALID_REASON: Note = Note::new(
    "{server} の整形の答えを当てられない（{detail}）ので何も変えなかった",
    "Could not apply the formatting from {server} ({detail}), so nothing changed",
);

/// 範囲の外へ空白以外の書き換えが来た。`{server}` / `{count}`
pub const FORMAT_OUTSIDE_RANGE_REASON: Note = Note::new(
    "{server} が範囲の外（{count} か所）まで書き換えようとしたので何も変えなかった（範囲の外は変えない）",
    "{server} tried to change {count} places outside the range, so nothing changed (nothing outside the range is changed)",
);

/// 範囲の外へ来たときの次の一手
pub const FORMAT_OUTSIDE_RANGE_NEXT_STEP: Note = Note::new(
    "範囲を行の頭から行の終わりまでに広げるか、範囲を付けずに全体を整形する",
    "Widen the range to whole lines, or format the whole document",
);

/// 答えを当てられないときの次の一手
pub const FORMAT_INVALID_NEXT_STEP: Note = Note::new(
    "サーバの不具合の可能性がある。tako lsp logs で様子を見る",
    "This may be a server bug; check tako lsp logs",
);

/// 保存時整形を飛ばして保存した。`{reason}` = 整形できなかった理由
pub const FORMAT_ON_SAVE_SKIPPED_NOTE: Note = Note::new(
    "整形せずに保存した（{reason}）",
    "Saved without formatting ({reason})",
);

/// 保存時整形の設定の説明（`tako lsp format-on-save` の応答）。`{save_key}` = 保存の打鍵
/// （`tako_core::platform::keys::save_preview`。OS ごとに違うので直書きしない = #1203）
pub const FORMAT_ON_SAVE_SCOPE_NOTE: Note = Note::new(
    "明示的な保存（{save_key}・tako edit save）の前にだけ整形する。自動保存では整形しない",
    "Formats only before an explicit save ({save_key} / tako edit save), never on autosave",
);
