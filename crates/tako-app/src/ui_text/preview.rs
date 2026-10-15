//! プレビューペイン（コード / Markdown / PDF / 画像 / 動画 / 履歴）の文言（キー: preview.*）
//!
//! dispatch / CLI へ返すエラー文字列は対象外（技術情報のため現状維持）。
//! ここはプレビュー画面に描画される文言のみ

// --- ヘッダー・トグル（キー: preview.header_*） ---

pub fn view_as_code() -> &'static str {
    tr!("コードとして表示", "View as code")
}
pub fn view_as_markdown() -> &'static str {
    tr!("md レンダリング表示", "Render as Markdown")
}
pub fn history() -> &'static str {
    tr!("履歴", "History")
}
pub fn editing() -> &'static str {
    tr!("編集中", "Editing")
}
pub fn edit() -> &'static str {
    tr!("編集", "Edit")
}
/// 保存ボタン。打鍵は正本（`tako_core::platform::keys::save_preview`）から
/// 受け取る（Windows は `Ctrl+Shift+S`。#1203）
pub fn save_with_key(key: &str) -> String {
    tr!(format!("保存 {key}"), format!("Save {key}"))
}
pub fn outline_button() -> &'static str {
    tr!("目次", "Outline")
}

// --- Code Runner（#453 M4。キー: preview.run_*） ---

pub fn run_button() -> &'static str {
    tr!("実行", "Run")
}
pub fn run_no_command() -> &'static str {
    tr!(
        "実行コマンド未定義。ファイル先頭に tako:run: <コマンド> を書くか tako run-default で拡張子既定を設定",
        "No run command. Add tako:run: <command> at file top or set extension default with tako run-default"
    )
}
pub fn run_profile_default() -> &'static str {
    tr!("実行", "Run")
}

// --- 実行ボタン横のドロップダウン（#1733。キー: preview.run_menu_*） ---

/// 再生ボタンのツールチップ（実行されるコマンドの全文）
pub fn run_tooltip(command: &str) -> String {
    tr!(
        format!("実行されるコマンド: {command}"),
        format!("Command to run: {command}")
    )
}
pub fn run_menu_command_title() -> &'static str {
    tr!("実行されるコマンド", "Command to run")
}
/// コマンドの下に出す作業ディレクトリ
pub fn run_menu_in_dir(dir: &str) -> String {
    tr!(format!("{dir} で実行"), format!("Runs in {dir}"))
}
pub fn run_menu_loading() -> &'static str {
    tr!("読み込み中…", "Loading…")
}
pub fn run_menu_profiles() -> &'static str {
    tr!("プロファイル", "Profiles")
}
/// 実行環境の見出し（`kind` は実行環境の種類の ID = `python` 等）
pub fn run_menu_runtime(kind: &str) -> String {
    tr!(format!("実行環境（{kind}）"), format!("Runtime ({kind})"))
}
/// 自動選択の候補に添える印
pub fn run_menu_auto_mark() -> &'static str {
    tr!("自動", "auto")
}
pub fn run_menu_redetect() -> &'static str {
    tr!("再検出", "Re-detect")
}
pub fn run_menu_redetecting() -> &'static str {
    tr!("確かめています…", "Checking…")
}
pub fn run_menu_pick_path() -> &'static str {
    tr!("パスを指定…", "Choose a path…")
}
/// 畳んだ実行環境の候補を広げるボタン
pub fn run_menu_more(n: usize) -> String {
    tr!(format!("他 {n} 件を表示"), format!("Show {n} more"))
}
pub fn run_menu_no_candidates() -> &'static str {
    tr!("候補が見つかりません", "No runtimes found")
}
/// 行の × のツールチップ（その項目の保存した値を消して既定 / 自動へ戻す）
pub fn run_menu_reset() -> &'static str {
    tr!("既定に戻す", "Back to the default")
}
pub fn run_menu_args() -> &'static str {
    tr!("引数", "Arguments")
}
pub fn run_menu_env() -> &'static str {
    tr!("環境変数", "Environment")
}
pub fn run_menu_cwd() -> &'static str {
    tr!("作業ディレクトリ", "Working directory")
}
pub fn run_menu_before() -> &'static str {
    tr!("実行前コマンド", "Before run")
}
pub fn run_menu_unset() -> &'static str {
    tr!("未設定", "Not set")
}
pub fn run_menu_env_add() -> &'static str {
    tr!("追加", "Add")
}
pub fn run_menu_open_settings() -> &'static str {
    tr!("Code Runner の設定を開く", "Open Code Runner settings")
}
/// 編集中の欄の下に出す操作の案内
pub fn run_menu_edit_hint() -> &'static str {
    tr!(
        "Enter で保存 / Esc で取り消し",
        "Enter to save / Esc to cancel"
    )
}
/// 空のまま Enter を押したとき（保存しない）
pub fn run_menu_empty() -> &'static str {
    tr!(
        "空のままでは保存しません（既定へ戻すときは行の右端の「既定に戻す」を押す）",
        "Empty values are not saved (use \"Back to the default\" at the right of the row instead)"
    )
}
/// 環境変数の欄に `=` が無いとき（保存しない）
pub fn run_menu_env_format() -> &'static str {
    tr!("名前=値 の形で入れてください", "Enter it as NAME=value")
}
/// 出典タグ（項目の値がどこから来たか = `RunResolve` の `config.*.source`）
pub fn run_menu_source(source: &str) -> &'static str {
    match source {
        "file" => tr!("ファイル", "File"),
        "project" => tr!("プロジェクト", "Project"),
        "declaration" => tr!("宣言", "Declared"),
        "auto" => tr!("自動", "Auto"),
        _ => tr!("既定", "Default"),
    }
}
/// 出典タグのツールチップ（クリックで保存先を移す）
pub fn run_menu_move_to(source: &str) -> &'static str {
    match source {
        "file" => tr!(
            "クリックでプロジェクト全体の設定へ移す",
            "Click to move it to the project"
        ),
        _ => tr!(
            "クリックでこのファイルだけの設定へ移す",
            "Click to move it to this file only"
        ),
    }
}

// --- 基板とポートの欄（#1952。`.ino` だけ。キー: preview.run_menu_board_* / run_menu_port_*） ---

pub fn run_menu_board() -> &'static str {
    tr!("基板", "Board")
}
pub fn run_menu_port() -> &'static str {
    tr!("ポート", "Port")
}
/// 基板・ポートの出典タグ（正本は Arduino の sketch.yaml = IDE とも共有）
pub fn run_menu_board_source() -> &'static str {
    "sketch.yaml"
}
/// 出典タグのツールチップ
pub fn run_menu_board_source_tip() -> &'static str {
    tr!(
        "選ぶと arduino-cli board attach が sketch.yaml へ書きます（Arduino IDE とも共有）",
        "Selecting runs arduino-cli board attach, which writes sketch.yaml (shared with Arduino IDE)"
    )
}
pub fn run_menu_board_unset() -> &'static str {
    tr!("未選択", "Not selected")
}
/// 基板を名前か FQBN で探す欄を開くボタン
pub fn run_menu_board_search() -> &'static str {
    tr!("基板を探す…", "Find a board…")
}
/// 欄の下（探す語に当たる基板が無い）
pub fn run_menu_board_no_match() -> &'static str {
    tr!(
        "入っているコアの基板に当たるものがありません（FQBN をそのまま入れて Enter でも選べます）",
        "No installed board matches (you can also type an FQBN and press Enter)"
    )
}
/// 基板の欄の操作の案内
pub fn run_menu_board_hint() -> &'static str {
    tr!(
        "名前か FQBN で絞り込み、行を押すか Enter で選ぶ / Esc で取り消し",
        "Type a name or FQBN to filter, then click a row or press Enter / Esc to cancel"
    )
}
/// 繋がっている基板に添える印
pub fn run_menu_board_connected() -> &'static str {
    tr!("接続中", "connected")
}
/// コアが入っていない基板に添える印（選ぶと導入のコマンドを示すだけ）
pub fn run_menu_board_core_missing() -> &'static str {
    tr!("コア未導入", "core not installed")
}
/// 書いている間（arduino-cli board attach）
pub fn run_menu_board_attaching() -> &'static str {
    tr!("sketch.yaml へ書いています…", "Writing sketch.yaml…")
}
/// 入っているコアの基板の件数（探す欄の見出し）
pub fn run_menu_board_installed(n: usize) -> String {
    tr!(
        format!("入っているコアの基板 {n} 件"),
        format!("{n} boards in the installed cores")
    )
}
/// ポートを入れる欄を開くボタン
pub fn run_menu_port_pick() -> &'static str {
    tr!("ポートを指定…", "Enter a port…")
}
/// 選んであるポートが今は見つからない（繋いでいない）
pub fn run_menu_port_absent() -> &'static str {
    tr!("繋がっていない", "not connected")
}
/// ポートが 1 つも見つからない
pub fn run_menu_port_none() -> &'static str {
    tr!(
        "ポートが見つかりません（基板を繋いで再検出）",
        "No ports found (connect the board and re-detect)"
    )
}

// --- 編集ステータス（キー: preview.status_*） ---

pub fn saved_suffix() -> &'static str {
    tr!(" \u{00B7} 保存済", " \u{00B7} saved")
}
pub fn conflict_suffix() -> &'static str {
    tr!(" \u{00B7} 競合", " \u{00B7} conflict")
}
pub fn error_suffix() -> &'static str {
    tr!(" \u{00B7} エラー", " \u{00B7} error")
}
pub fn saved_message() -> &'static str {
    tr!("保存しました", "Saved")
}

// --- 外部変更の競合（#1659。キー: preview.conflict_*） ---

/// 帯の文面（ディスク上で書き換わった）
pub fn conflict_changed() -> &'static str {
    tr!(
        "ディスク上のファイルが変更されました。編集中の内容は保持しています",
        "The file changed on disk. Your edits are kept"
    )
}
/// 帯の文面（ディスク上で消された）
pub fn conflict_deleted() -> &'static str {
    tr!(
        "ディスク上のファイルが削除されました。編集中の内容は保持しています",
        "The file was deleted on disk. Your edits are kept"
    )
}
/// 自動保存 ON のときだけ添える（競合中は止めている）
pub fn conflict_autosave_paused() -> &'static str {
    tr!("自動保存は停止中", "Autosave paused")
}
pub fn conflict_show_diff() -> &'static str {
    tr!("差分", "Diff")
}
pub fn conflict_hide_diff() -> &'static str {
    tr!("差分を閉じる", "Hide diff")
}
/// 自分の変更で上書きする（`tako edit save --force` と同じ）
pub fn conflict_overwrite() -> &'static str {
    tr!("上書き保存", "Overwrite")
}
/// 消されたファイルを自分の変更で作り直す（`tako edit save --force` と同じ）
pub fn conflict_recreate() -> &'static str {
    tr!("作り直して保存", "Save anyway")
}
/// ディスクの中身を採る（`tako edit reload` と同じ。自分の変更は undo で戻せる）
pub fn conflict_reload() -> &'static str {
    tr!("ディスクから読み直す", "Reload from disk")
}
/// 差分の向きの凡例（`-` がディスク、`+` が編集中）
pub fn conflict_diff_legend(added: usize, removed: usize) -> String {
    tr!(
        format!("- ディスク {removed} 行 / + 編集中 {added} 行"),
        format!("- disk {removed} lines / + your edits {added} lines")
    )
}
pub fn conflict_diff_truncated() -> &'static str {
    tr!(
        "差分が長いため先頭だけを表示しています",
        "Diff is long; showing the beginning only"
    )
}
/// ヘッダの一時表示（「保存しました」と同じ場所）。ファイル名を押し潰さない長さに留める
pub fn reloaded_message() -> &'static str {
    tr!("ディスクから読み直しました", "Reloaded from disk")
}
/// リモート由来の写しは読み直しても最新にならない（#966）
pub fn remote_revert_unsupported() -> &'static str {
    tr!(
        "リモートのファイルは読み直せません。開き直してリモートから取り直してください",
        "Remote files cannot be reloaded here. Reopen the file to fetch it again"
    )
}

// --- 目次ポップオーバー（#232。キー: preview.outline_*） ---

pub fn outline_section() -> &'static str {
    tr!("アウトライン", "Outline")
}
pub fn goto_page_section() -> &'static str {
    tr!("ページへ移動", "Go to page")
}
pub fn page_n(page: usize) -> String {
    tr!(format!("ページ {page}"), format!("Page {page}"))
}
/// 目次の行を押したときに通知欄へ出す**操作名**（#1417）。
/// 節の見出し（[`outline_section`]）とは別に持つ（見出しの文言を変えても
/// 失敗の文が「何をしようとしたか」を言い続けるため）
pub fn op_outline_jump() -> &'static str {
    tr!("目次へ移動", "Jump to outline item")
}
/// ページ一覧の行を押したときに通知欄へ出す**操作名**（#1417）
pub fn op_goto_page() -> &'static str {
    tr!("ページ移動", "Go to page")
}
/// コードブロックの「コピー」を押したときに通知欄へ出す**操作名**（#1422）
pub fn op_copy_code_block() -> &'static str {
    tr!("コードブロックのコピー", "Copy code block")
}
/// 失敗した対象を指すラベル（何番目のコードブロックか。#1422）
pub fn code_block_n(index: usize) -> String {
    tr!(
        format!("コードブロック {}", index + 1),
        format!("Code block {}", index + 1)
    )
}
/// Code Runner でファイルを実行しようとしたときの**操作名**（#453 / #1422）
pub fn op_run_file() -> &'static str {
    tr!("ファイルの実行", "Run file")
}
/// 「履歴」ボタンで変更履歴を開こうとしたときの**操作名**（#2014。git が上限まで返らなかった等）
pub fn op_changelog() -> &'static str {
    tr!("変更履歴の表示", "Show file history")
}
pub fn item_count(n: usize) -> String {
    tr!(format!("{n} 件"), format!("{n} items"))
}

// --- 本文・状態表示（キー: preview.body_*） ---

pub fn loading() -> &'static str {
    tr!("読み込み中…", "Loading…")
}
/// 上限を超えて末尾を省略したときのフッタ（#1660）。**理由と値を必ず出す**
/// （何を超えたか・上限はいくつか・編集できないこと）。値の文面は
/// `tako_core::preview_limit::Truncation` の 1 実装から取るので `tako edit` の応答と一致する
pub fn tail_omitted(limit: &tako_core::preview_limit::Truncation) -> String {
    tr!(
        format!(
            "…（上限を超えるため末尾を省略して表示し、編集できません: {}）",
            limit.detail_ja()
        ),
        format!(
            "… (tail omitted and editing disabled: {})",
            limit.detail_en()
        )
    )
}
/// 動画の再生ボタンの**語**（#1536）。
/// 印（三角）は `file_icons::ui_icon::PLAY` を `svg()` で描くので、ここは文字だけ持つ
/// （もとは `\u{25b6}\u{fe0e}` = 絵文字を異体字セレクタでテキスト表示へ倒す書き方だった）
pub fn video_play() -> &'static str {
    tr!("再生", "Play")
}
pub fn video_resolution(w: u32, h: u32) -> String {
    tr!(
        format!("解像度: {w} x {h}"),
        format!("Resolution: {w} x {h}")
    )
}
pub fn video_duration(mins: u64, secs: u64) -> String {
    tr!(
        format!("長さ: {mins}:{secs:02}"),
        format!("Duration: {mins}:{secs:02}")
    )
}
pub fn video_codec(codec: &str) -> String {
    tr!(format!("コーデック: {codec}"), format!("Codec: {codec}"))
}
pub fn video_size_mb(mb: f64) -> String {
    tr!(format!("サイズ: {mb:.1} MB"), format!("Size: {mb:.1} MB"))
}
pub fn video_size_kb(kb: f64) -> String {
    tr!(format!("サイズ: {kb:.0} KB"), format!("Size: {kb:.0} KB"))
}

// --- 履歴（チェンジログ）ビュー（#338。キー: preview.changelog_*） ---

pub fn not_in_git() -> &'static str {
    tr!(
        "git 管理外のファイルです",
        "This file is not tracked by git"
    )
}
pub fn no_history() -> &'static str {
    tr!(
        "このファイルの変更履歴はありません",
        "No change history for this file"
    )
}
pub fn no_diff() -> &'static str {
    tr!("(変更なし)", "(no changes)")
}

// --- プレビューエラー表示（キー: preview.error_*） ---

pub fn unsupported_image() -> &'static str {
    tr!("対応していない画像形式", "Unsupported image format")
}
pub fn image_too_large(mb: f64) -> String {
    tr!(
        format!("画像が大きすぎる（{mb:.1} MB、上限 50 MB）"),
        format!("Image too large ({mb:.1} MB, limit 50 MB)")
    )
}
pub fn cannot_read(e: &str) -> String {
    tr!(format!("読み込めない: {e}"), format!("Cannot read: {e}"))
}
/// PDF レンダラを持たないプラットフォーム向け（#521）。macOS は Core Graphics、
/// Windows は Windows.Data.Pdf を使うので、実際に出るのはそれ以外の OS だけ。
/// 「対応 OS の列挙」ではなく「この環境では描けない」と言う（対応 OS が増えても文言が腐らない）
pub fn pdf_unsupported_platform() -> &'static str {
    tr!(
        "この環境には PDF レンダラが無いため表示できない",
        "This platform has no PDF renderer, so the file cannot be displayed"
    )
}
pub fn binary_file() -> &'static str {
    tr!(
        "バイナリファイル（テキストとして表示できない）",
        "Binary file (cannot display as text)"
    )
}

// --- Markdown コードブロックのコピーボタン（#680。キー: preview.code_copy_*） ---

/// コピー直後の成功フィードバック（待機中はアイコンのみで文言を出さない）
pub fn code_copied() -> &'static str {
    tr!("コピーしました", "Copied")
}

// --- リンクを開かなかったときの通知（#1376。キー: preview.notice_link_*） ---

/// PDF のリンク注釈・提案チップを開かなかった理由（#1376）。
///
/// PDF のリンク注釈は **PDF ファイルが持つ任意の文字列**で、OS の既定ハンドラは
/// `file:` URL やローカルの実行ファイルパスも開く。開かないと決めたときに
/// `eprintln!` だけで済ませると GUI では**押しても無言**になる（#1283 と同じ穴）ので、
/// 共有の通知欄（`remote_notice`）へ出す。**リンク文字列そのものは載せない**
/// （PDF の内容 = ペイン内容に相当）
pub fn notice_link_blocked() -> &'static str {
    tr!(
        "このリンクは開けません（http / https のみ対応）",
        "This link cannot be opened (http / https only)"
    )
}

// --- 検索欄のトグル（#1653。キー: preview.search_toggle_*） ---

/// 印は SVG で描くので、語だけを持つ（#1536）。オン / オフは状態で言い分ける
pub fn search_toggle_case(on: bool) -> &'static str {
    if on {
        tr!("大文字と小文字を区別する（オン）", "Match case (on)")
    } else {
        tr!("大文字と小文字を区別する（オフ）", "Match case (off)")
    }
}
pub fn search_toggle_word(on: bool) -> &'static str {
    if on {
        tr!("単語単位で探す（オン）", "Match whole word (on)")
    } else {
        tr!("単語単位で探す（オフ）", "Match whole word (off)")
    }
}

// --- 定義ジャンプ（キー: preview.goto_*。#1680） ---
// 理由・次の一手の日英は dispatch の応答（`tako_control::lsp::text`）が持つ。ここは画面だけの語

/// 操作の名前（通知欄の「〜に失敗」と診断の op）
pub fn goto_op() -> &'static str {
    tr!("定義へ移動", "Go to definition")
}

/// 問い合わせ中の印（プレビューのヘッダに出す）
pub fn goto_searching() -> &'static str {
    tr!("定義を探しています…", "Finding definition…")
}

/// 候補が複数のときの一覧の見出し
pub fn goto_choose(count: usize) -> String {
    tr!(
        format!("候補が {count} か所あります"),
        format!("{count} candidates")
    )
}

/// 通知欄へ出す 1 行（理由 + 次の一手。どちらも応答の文をそのまま使う）
pub fn goto_notice(reason: &str, next_step: &str) -> String {
    if next_step.is_empty() {
        return reason.to_string();
    }
    tr!(
        format!("{reason}。{next_step}"),
        format!("{reason}. {next_step}")
    )
}

// --- 整形（キー: preview.format_*。#1683） ---
// 結果の注記・理由・次の一手の日英は dispatch の応答（`tako_control::lsp::text`）が持つ。
// ここは画面だけの語

/// 問い合わせ中の印（プレビューのヘッダに出す）
pub fn format_running() -> &'static str {
    tr!("整形しています…", "Formatting…")
}

/// 範囲の整形を頼んだのに選択が無い
pub fn format_no_selection() -> &'static str {
    tr!(
        "範囲が選ばれていない（全体の整形は「コードを整形」）",
        "Nothing is selected (use Format Document for the whole file)"
    )
}

/// 編集できるコードのプレビューではないので整形できない
pub fn format_not_code() -> &'static str {
    tr!(
        "整形できるのはコードのプレビューだけ",
        "Only code previews can be formatted"
    )
}

// --- 名前の変更（キー: preview.rename_*。#1686） ---
// 結果の注記・理由・次の一手の日英は dispatch の応答（`tako_control::lsp::text`）が持つ。
// ここは画面だけの語（帯の見出しとキーの案内）

/// 帯の見出し。`{name}` = 今の名前
pub fn rename_title(name: &str) -> String {
    tr!(
        format!("「{name}」の名前を変える"),
        format!("Rename \"{name}\"")
    )
}

/// 帯のキーの案内
pub fn rename_hint() -> &'static str {
    tr!(
        "Enter で書き換える（undo 1 回で戻る・保存はしない）・Esc でやめる",
        "Enter to rename (one undo reverts it; not saved) · Esc to cancel"
    )
}

/// 問い合わせ中の印（帯の中に出す）
pub fn rename_running() -> &'static str {
    tr!(
        "言語サーバに問い合わせています…",
        "Asking the language server…"
    )
}

// --- ホバー（キー: preview.hover_*。#1681） ---
// 理由・次の一手の日英は dispatch の応答（`tako_control::lsp::text`）が持つ。ここは画面だけの語

/// 操作の名前（カードのリンクを開けなかったときの通知欄の「〜に失敗」と診断の op）
pub fn hover_link_op() -> &'static str {
    tr!("ホバーのリンクを開く", "Open a link in the hover card")
}

/// コードのプレビューではないのでホバーを出せない
pub fn hover_not_code() -> &'static str {
    tr!(
        "ホバー情報を出せるのはコードのプレビューだけ",
        "Hover is available only in code previews"
    )
}

/// 位置（編集カーソル / 選択）が無い
pub fn hover_no_cursor() -> &'static str {
    tr!(
        "カーソルが無い（編集モードに入るか、文字を選んでから）",
        "No cursor (enter edit mode or select some text first)"
    )
}

/// メニュー / キーで頼んだホバーの答えを待っているあいだの印（プレビューのヘッダに出す。#1893）。
/// サーバが読み込み中なら `tako_control::lsp::text::HOVER_LOADING_NOTE` の方を出す
pub fn hover_searching() -> &'static str {
    tr!("ホバー情報を問い合わせています…", "Fetching hover info…")
}

/// 本文を上限で切ったカードの下の注記
pub fn hover_truncated(total: usize) -> String {
    let limit = tako_core::lsp::hover::MAX_CHARS;
    tr!(
        format!("長いので先頭 {limit} 字まで（全 {total} 字）"),
        format!("Showing the first {limit} of {total} characters")
    )
}

// --- 言語サーバの状態（#1944。キー: preview.lsp_*）。エディタのタイトルの 1 行と、未導入・失敗の帯 ---

/// 取得中（取得物が決まっていれば `Some((取得物, 受けた MB, 全体の MB))`。Node.js を取るなら先に Node.js）
pub fn lsp_fetching(name: &str, progress: Option<(&str, f64, f64)>) -> String {
    match progress {
        Some((item, done_mb, total_mb)) => tr!(
            format!("{name} を取得中（{item} {done_mb:.1} / {total_mb:.1} MB）"),
            format!("Fetching {name} ({item} {done_mb:.1} / {total_mb:.1} MB)")
        ),
        None => tr!(format!("{name} を取得中"), format!("Fetching {name}")),
    }
}
pub fn lsp_starting(name: &str) -> String {
    tr!(format!("{name} 起動中"), format!("{name} starting"))
}
pub fn lsp_loading(name: &str) -> String {
    tr!(format!("{name} 読み込み中"), format!("{name} loading"))
}
pub fn lsp_running(name: &str) -> String {
    tr!(format!("{name} 動作中"), format!("{name} running"))
}
/// 帯の見出し: 入っていない（補完・診断が出ないことを先に言う = 黙って出ない状態を無くす）
pub fn lsp_not_installed(name: &str) -> String {
    tr!(
        format!("言語サーバ {name} が入っていないので、補完・診断・ホバーは出ません"),
        format!("Language server {name} is not installed, so completion, diagnostics and hover are unavailable")
    )
}
/// 帯の見出し: 取りに行って失敗した
pub fn lsp_fetch_failed(name: &str) -> String {
    tr!(
        format!("言語サーバ {name} を取得できませんでした（補完・診断は出ません）"),
        format!("Could not fetch language server {name} (no completion or diagnostics)")
    )
}
/// 帯の見出し: 諦めた・止めた
pub fn lsp_unavailable(name: &str) -> String {
    tr!(
        format!("言語サーバ {name} が止まっています（補完・診断は出ません）"),
        format!("Language server {name} is not running (no completion or diagnostics)")
    )
}
/// 取って入れるボタン（グローバルへは入れない）
pub fn lsp_install() -> &'static str {
    tr!("入れる", "Install")
}
pub fn lsp_retry() -> &'static str {
    tr!("もう一度取得", "Retry")
}
pub fn lsp_restart() -> &'static str {
    tr!("起こし直す", "Restart")
}
/// 帯の見出し: 起こす前提が整っていない（#1951。Arduino の基板が未選択）。理由と選ぶコマンドは帯の下の行
pub fn lsp_unconfigured(name: &str) -> String {
    tr!(
        format!("言語サーバ {name} は準備ができるまで起こしていません（補完・診断・ホバーは出ません）"),
        format!("Language server {name} is not started until it is set up (no completion, diagnostics or hover)")
    )
}
/// 前提を満たしたかをもう一度確かめるボタン（manager の restart = CLI `tako lsp restart` と同じ口）
pub fn lsp_recheck() -> &'static str {
    tr!("もう一度確かめる", "Check again")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                view_as_code().to_string(),
                view_as_markdown().to_string(),
                history().to_string(),
                editing().to_string(),
                edit().to_string(),
                save_with_key(tako_core::platform::keys::save_preview(
                    tako_core::platform::support::Platform::MacOs,
                )),
                outline_button().to_string(),
                run_button().to_string(),
                run_no_command().to_string(),
                run_profile_default().to_string(),
                run_tooltip("python3 a.py"),
                run_menu_command_title().to_string(),
                run_menu_in_dir("/tmp"),
                run_menu_loading().to_string(),
                run_menu_profiles().to_string(),
                run_menu_runtime("python"),
                run_menu_auto_mark().to_string(),
                run_menu_redetect().to_string(),
                run_menu_redetecting().to_string(),
                run_menu_pick_path().to_string(),
                run_menu_more(4),
                run_menu_no_candidates().to_string(),
                run_menu_reset().to_string(),
                run_menu_args().to_string(),
                run_menu_env().to_string(),
                run_menu_cwd().to_string(),
                run_menu_before().to_string(),
                run_menu_unset().to_string(),
                run_menu_env_add().to_string(),
                run_menu_open_settings().to_string(),
                run_menu_edit_hint().to_string(),
                run_menu_empty().to_string(),
                run_menu_env_format().to_string(),
                run_menu_source("file").to_string(),
                run_menu_source("project").to_string(),
                run_menu_source("declaration").to_string(),
                run_menu_source("auto").to_string(),
                run_menu_source("default").to_string(),
                run_menu_move_to("file").to_string(),
                run_menu_move_to("project").to_string(),
                run_menu_board().to_string(),
                run_menu_port().to_string(),
                run_menu_board_source_tip().to_string(),
                run_menu_board_unset().to_string(),
                run_menu_board_search().to_string(),
                run_menu_board_no_match().to_string(),
                run_menu_board_hint().to_string(),
                run_menu_board_connected().to_string(),
                run_menu_board_core_missing().to_string(),
                run_menu_board_attaching().to_string(),
                run_menu_board_installed(27),
                run_menu_port_pick().to_string(),
                run_menu_port_absent().to_string(),
                run_menu_port_none().to_string(),
                saved_suffix().to_string(),
                conflict_suffix().to_string(),
                error_suffix().to_string(),
                saved_message().to_string(),
                outline_section().to_string(),
                goto_page_section().to_string(),
                page_n(3),
                op_outline_jump().to_string(),
                op_goto_page().to_string(),
                op_copy_code_block().to_string(),
                code_block_n(0),
                op_run_file().to_string(),
                item_count(12),
                loading().to_string(),
                tail_omitted(&tako_core::preview_limit::Truncation::Lines { lines: 123_456 }),
                tail_omitted(&tako_core::preview_limit::Truncation::Bytes { size: 12_345_678 }),
                video_play().to_string(),
                video_resolution(1920, 1080),
                video_duration(3, 5),
                video_codec("h264"),
                video_size_mb(1.5),
                video_size_kb(200.0),
                not_in_git().to_string(),
                no_history().to_string(),
                no_diff().to_string(),
                unsupported_image().to_string(),
                image_too_large(60.0),
                cannot_read("io error"),
                pdf_unsupported_platform().to_string(),
                binary_file().to_string(),
                code_copied().to_string(),
                notice_link_blocked().to_string(),
                search_toggle_case(true).to_string(),
                search_toggle_case(false).to_string(),
                search_toggle_word(true).to_string(),
                search_toggle_word(false).to_string(),
                goto_op().to_string(),
                goto_searching().to_string(),
                goto_choose(3),
                goto_notice("r", "n"),
                format_running().to_string(),
                format_no_selection().to_string(),
                format_not_code().to_string(),
                rename_title("value"),
                rename_hint().to_string(),
                rename_running().to_string(),
                hover_link_op().to_string(),
                hover_not_code().to_string(),
                hover_no_cursor().to_string(),
                hover_searching().to_string(),
                hover_truncated(40_000),
                lsp_fetching("pyright", Some(("Node.js 24.21.0", 1.0, 52.9))),
                lsp_fetching("pyright", None),
                lsp_starting("pyright"),
                lsp_loading("pyright"),
                lsp_running("pyright"),
                lsp_not_installed("clangd"),
                lsp_fetch_failed("pyright"),
                lsp_unavailable("pyright"),
                lsp_install().to_string(),
                lsp_retry().to_string(),
                lsp_restart().to_string(),
                lsp_unconfigured("srv"),
                lsp_recheck().to_string(),
            ]
        });
    }
}
