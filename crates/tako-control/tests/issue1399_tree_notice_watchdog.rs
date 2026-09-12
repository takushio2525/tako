//! ファイルツリーのローカル操作が失敗を黙って捨てていないかの番犬（#1399）
//!
//! `crates/tako-app/src/sidebar.rs` のローカル行の操作 13 か所は dispatch の結果を
//! `let _ =` / `if let Ok(` / `if result.is_ok()` / `eprintln!` で捨てていて、
//! **ごみ箱移動やリネームが失敗しても画面が無反応**だった。同じファイルのリモート行
//! （#919）は同じ失敗を共有の通知欄（`remote_notice`）へ出していたので、1 つの
//! サイドバーに 2 つのエラー方針が同居していた。
//!
//! `.agent/conventions.md`（境界 B8 の節）が「**弾いたら黙って捨てない**。GUI の
//! `eprintln!` は誰も読めないので『押しても無言』になる（#1283 と同じ穴）」と
//! 明記しているのに、そこから逸脱していたのが本体。#1283 / #1376 が同型の前例。
//!
//! #1417 で、この走査が見つけた**別画面の 3 件**（右パネルの tmux window 切替 /
//! プレビューの目次ジャンプ・ページジャンプ）も同じ 1 実装へ寄せた。
//! `KNOWN_DISCARDED` は空になり、番犬は全 UI モジュールを素で縛っている。
//!
//! 走査対象は tako-app の UI モジュール（`main.rs` を除く）。`main.rs` を外すのは
//! `ui_dispatch_attach_watchdog` と同じ理由で、production と隔離セルフテスト /
//! visual-test が同居しており**ソース走査では両者を区別できない**ため
//! （セルフテストは意図的に結果を捨てて「失敗しても進む」項目を持つ）。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// まだ通知欄へ寄せていない「dispatch の結果を捨てる」既知の箇所。
///
/// #1399 が直したのは**ファイルツリー**（`sidebar.rs`）の 13 か所で、別の画面に
/// 残っていた 3 件（右パネルの tmux window 切替 / プレビューの目次ジャンプ・
/// ページジャンプ）は段階導入のためここに宣言していた。**#1417 で 3 件とも
/// 通知欄へ寄せたので空**。
///
/// リストは**減る方向にしか動かさない**（下のテストが、直したのに残っている
/// エントリを落とす）。空のまま保つこと = 新しい画面で `Err` を捨てたら、
/// ここへ足して素通りさせるのではなく [`TakoApp::notify_ui_dispatch_failed`]
/// を通す。キーは `ファイル名:Request 名`
const KNOWN_DISCARDED: &[&str] = &[];

/// `Err` を扱ったと認めるしるし。`match` のアーム / `if let Err(` / `map_err` の
/// どれかが**その呼び出しの結果と一緒に**現れること（#1422 で「窓の中に在るだけ」から
/// 「結果の束縛名と同じ行に在る」へ寄せた。理由は [`scan_discarded`] の doc）
const HANDLED_MARKS: &[&str] = &["Err(", "map_err", "is_err()"];

/// `let <name> = tako_control::dispatch(` の `<name>` を拾う。
/// `let _ =` と分割代入は束縛名なし（`None`）として扱う
fn bound_name(line: &str) -> Option<String> {
    let (head, _) = line.split_once("= tako_control::dispatch(")?;
    // 型注釈つき（`let r: Result<_, _> = dispatch(..)`）も名前だけ取る
    let name = head.trim().strip_prefix("let ")?.split(':').next()?.trim();
    if name == "_" || name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(name.to_string())
}

/// `line` の `at` から始まる `name` が**識別子として独立**しているか
/// （`opened` が `opened_at` の一部で当たらないようにする）
fn is_ident_at(line: &str, at: usize, name: &str) -> bool {
    let b = line.as_bytes();
    let before = at == 0 || !(b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'_');
    let end = at + name.len();
    let after = end >= b.len() || !(b[end].is_ascii_alphanumeric() || b[end] == b'_');
    before && after
}

/// 束縛式の終端（最初に `;` で終わる行の次）。`.map_err(..)` のように**呼び出し式へ
/// 続けて**扱う形をここで拾う
fn stmt_end(window: &[String]) -> usize {
    window
        .iter()
        .position(|l| l.trim_end().ends_with(';'))
        .map(|i| i + 1)
        .unwrap_or(window.len())
}

/// 束縛された結果を**その名前で**扱っているか（#1422）。
///
/// 認めるのは 4 つだけ:
/// 1. 束縛式そのものに `map_err` / `?` が付いている（`let r = dispatch(..).map_err(..)`）
/// 2. `<name>` と [`HANDLED_MARKS`] が同じ行に在る（`if let Err(e) = r` / `r.is_err()`）
/// 3. `match <name>` の下に `Err(` アームが在る
/// 4. `<name>` をそのまま呼び出し元へ返している（`return r;` / `Ok(r)` / 末尾の `r`）
fn handled_by_name(window: &[String], name: &str) -> bool {
    let end = stmt_end(window);
    if window[..end]
        .iter()
        .any(|l| l.contains("map_err") || l.contains("?;") || l.trim_end().ends_with('?'))
    {
        return true;
    }
    for (i, line) in window.iter().enumerate() {
        let Some(at) = line.find(name) else { continue };
        if !is_ident_at(line, at, name) {
            continue;
        }
        // 束縛式そのもの（`let r = dispatch(`）は「扱った」の証拠にしない
        if i < end && line.contains("= tako_control::dispatch(") {
            continue;
        }
        if HANDLED_MARKS.iter().any(|m| line.contains(m)) {
            return true;
        }
        if line.contains(&format!("match {name}")) && window[i..].iter().any(|l| l.contains("Err("))
        {
            return true;
        }
        let bare = line
            .trim()
            .trim_end_matches(',')
            .trim_end_matches(';')
            .trim();
        if bare == name
            || bare == format!("return {name}")
            || bare == format!("Ok({name})")
            || bare == format!("Some({name})")
        {
            return true;
        }
    }
    false
}

/// 呼び出し行そのものが「結果を捨てる形」かどうか
fn discard_shape(line: &str) -> Option<&'static str> {
    if line.contains("let _ = tako_control::dispatch(") {
        return Some("結果を `let _ =` で捨てている");
    }
    if line.contains("if let Ok(") && line.contains("tako_control::dispatch(") {
        return Some("`if let Ok(` で `Err` を落としている");
    }
    None
}

/// まだ通知欄 / persist.log へ寄せていない `eprintln!` の**件数**（ファイル単位）。
///
/// #1399 は `sidebar.rs` だけを見ていたので、他の UI モジュールの `eprintln!` は
/// 誰も落とさなかった（#1422）。検査を全 UI モジュールへ広げるにあたり、本 Issue の
/// スコープ外の箇所は**件数**で宣言して段階導入する（行番号だと無関係な編集で
/// 壊れる。`KNOWN_DISCARDED` と同じく**減る方向にしか動かさない** = 直したら数を減らす）。
///
/// ここに残っているものは別 Issue で振り分ける:
/// `drawer.rs` はバックグラウンド復帰（`right_panel.rs` と同型）、
/// `open_files.rs` / `command_card_ui.rs` / `chat_view.rs` / `update_window.rs` は
/// それぞれユーザー操作の失敗。`autorename.rs` は env で明示的に有効化する診断出力
const KNOWN_EPRINTLN: &[(&str, usize)] = &[
    ("autorename.rs", 1),
    ("chat_view.rs", 1),
    ("command_card_ui.rs", 2),
    ("drawer.rs", 2),
    ("open_files.rs", 3),
    ("update_window.rs", 1),
];

/// 行コメントを**行数を保ったまま**落とす（近くの説明文を「扱った証拠」と
/// 誤認しないため。`ui_dispatch_attach_watchdog` が実際にこの空振りを踏んだ）
fn strip_comment_lines(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .map(|l| {
            if l.trim_start().starts_with("//") {
                String::new()
            } else {
                (*l).to_string()
            }
        })
        .collect()
}

/// 1 つの違反を表す（`ファイル名:行 Request名 — 理由`）
#[derive(Debug, PartialEq)]
struct Offender {
    file: String,
    line: usize,
    variant: String,
    why: &'static str,
}

impl Offender {
    fn key(&self) -> String {
        format!("{}:{}", self.file, self.variant)
    }
    fn report(&self) -> String {
        format!(
            "{}:{} Request::{} — {}",
            self.file, self.line, self.variant, self.why
        )
    }
}

/// ソース 1 本を走査して「dispatch の結果を扱っていない呼び出し」を返す。
///
/// 判定の窓は **[呼び出し行, 次の dispatch 呼び出し行) を上限 45 行で切ったもの**。
/// 「呼び出しから下へ N 行」だけで見ると、**隣の呼び出しの `Err` 処理を自分のものと
/// 数えて**しまう（`commit_inline_edit` の `if result.is_ok()` が、その下にある
/// copy-abs の `Err` アームで素通りする）。
///
/// 窓の中に `Err(` が**在るかどうか**だけを見ると、まだ穴が残る（#1422）。
/// `right_panel.rs` の `TmuxOpen` は結果を `if opened.is_ok()` でしか見ていないのに、
/// 窓の中の**無関係な** `if let Err(e) = this.attach_pending_sessions(cx)` を
/// 「扱った」と数えて素通りしていた。そこで**結果の束縛名を追い**、`Err` がその名前と
/// 一緒に現れることを要求する（[`handled_by_name`]）。束縛名が無い形
/// （`match dispatch(..)` / `if let Err(..) = dispatch(..)`）はその場で扱うしかないので
/// 従来どおり窓の中のしるしで見る
///
/// 戻り値の 2 番目は走査した dispatch 呼び出しの数（空振りの検出に使う）
fn scan_discarded(file: &str, src: &str) -> (Vec<Offender>, usize) {
    let raw: Vec<&str> = src.lines().collect();
    let lines = strip_comment_lines(&raw);
    let call_lines: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("tako_control::dispatch("))
        .map(|(i, _)| i)
        .collect();
    let mut offenders = Vec::new();
    for (n, &i) in call_lines.iter().enumerate() {
        // 呼び出しの直後にある Request 名を拾う（`ui_dispatch_attach_watchdog` と同じ形）
        let head_to = (i + 35).min(lines.len());
        let variant = lines[i..head_to]
            .join("\n")
            .split("Request::")
            .nth(1)
            .map(|rest| {
                rest.chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect::<String>()
            })
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "?".to_string());
        let make = |why| Offender {
            file: file.to_string(),
            line: i + 1,
            variant: variant.clone(),
            why,
        };
        if let Some(why) = discard_shape(&lines[i]) {
            offenders.push(make(why));
            continue;
        }
        let next_call = call_lines.get(n + 1).copied().unwrap_or(lines.len());
        let to = next_call.min(i + 45).min(lines.len());
        let window = &lines[i..to];
        let handled = match bound_name(&lines[i]) {
            Some(name) => handled_by_name(window, &name),
            None => HANDLED_MARKS.iter().any(|m| window.join("\n").contains(m)),
        };
        if !handled {
            offenders.push(make(
                "結果の `Err` を一度も見ていない（通知欄へ出していない）",
            ));
        }
    }
    (offenders, call_lines.len())
}

/// 関数の中身だけを切り出す（次の `fn` の手前まで。上限 1400 文字）。
///
/// 「見出しから下へ N 文字」で切ると**隣の関数の `notify_…` を自分のものと数える**
/// （実測: 目次のハンドラから通知を外しても、その下のページのハンドラの
/// `notify_ui_dispatch_failed` を拾って素通りした）。`scan_discarded` の窓の
/// 上限と同じ理由で、隣へ食い込ませない
fn fn_body(rest: &str) -> String {
    let capped: String = rest.chars().take(1400).collect();
    match capped
        .find("\n    fn ")
        .into_iter()
        .chain(capped.find("\n    pub(crate) fn "))
        .min()
    {
        Some(end) => capped[..end].to_string(),
        None => capped,
    }
}

fn ui_modules(root: &Path) -> Vec<PathBuf> {
    let dir = root.join("crates/tako-app/src");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .expect("tako-app/src が読める")
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        // main.rs は自己検証コードが同居するので除外（理由はモジュール doc）
        if path.file_name().and_then(|n| n.to_str()) == Some("main.rs") {
            continue;
        }
        out.push(path);
    }
    out.sort();
    out
}

/// `#[cfg(test)] mod ..` のブロックを**行数を保ったまま**空にする（#1422）。
///
/// 番犬が見るのは production の経路だけ。テストの中の `eprintln!`（`[perf]` の計測・
/// `[skip]` の理由）は誰も読めない出力ではないので違反ではない。
/// ファイル末尾とは限らない（`preview_render.rs` は中ほどに `mod pdf_hit_test_tests`）ので、
/// `#[cfg(test)]` と**同じ深さの閉じ括弧**までを切る
fn strip_test_mods(lines: &[String]) -> Vec<String> {
    let mut out = lines.to_vec();
    let mut i = 0;
    while i < out.len() {
        if out[i].trim() == "#[cfg(test)]" {
            let indent = out[i].len() - out[i].trim_start().len();
            let mut j = i + 1;
            while j < out.len() && out[j].trim().is_empty() {
                j += 1;
            }
            if j < out.len() && out[j].trim_start().starts_with("mod ") {
                let close = format!("{}}}", " ".repeat(indent));
                let mut k = j + 1;
                while k < out.len() && out[k] != close {
                    k += 1;
                }
                for line in out.iter_mut().take((k + 1).min(lines.len())).skip(i) {
                    line.clear();
                }
                i = k + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// production のソース行（コメントとテストモジュールを行数を保ったまま抜いたもの）
fn production_lines(src: &str) -> Vec<String> {
    let raw: Vec<&str> = src.lines().collect();
    strip_test_mods(&strip_comment_lines(&raw))
}

fn sidebar_src(root: &Path) -> String {
    std::fs::read_to_string(root.join("crates/tako-app/src/sidebar.rs"))
        .expect("sidebar.rs が読める")
}

#[test]
fn ツリーのローカル操作はdispatchの結果を捨てていない() {
    let root = workspace_root();
    let mut offenders = Vec::new();
    for path in ui_modules(&root) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let src = std::fs::read_to_string(&path).expect("UI モジュールが読める");
        let (found, _) = scan_discarded(&name, &src);
        offenders.extend(found);
    }
    let unknown: Vec<String> = offenders
        .iter()
        .filter(|o| !KNOWN_DISCARDED.contains(&o.key().as_str()))
        .map(Offender::report)
        .collect();
    assert!(
        unknown.is_empty(),
        "UI から dispatch を呼んで **`Err` を捨てている**箇所がある（#1399）。\
         データを消す操作（ごみ箱移動）や名前を打つ操作（リネーム）の失敗が無言になり、\
         ユーザーには「押せていないのか / 消えたのに表示が古いのか」が区別できない。\
         `.agent/conventions.md`（境界 B8）の「弾いたら黙って捨てない」に従い、\
         `notify_ui_dispatch_failed`（画面は `NoticeArea`）で共有の通知欄\
         （`remote_notice`）へ `tr!` を通した 1 行を出すこと:\n  {}",
        unknown.join("\n  ")
    );
    let stale: Vec<&str> = KNOWN_DISCARDED
        .iter()
        .copied()
        .filter(|known| !offenders.iter().any(|o| o.key() == *known))
        .collect();
    assert!(
        stale.is_empty(),
        "KNOWN_DISCARDED の {stale:?} はもう違反していない。\
         既知リストは減る方向にしか動かさないので、直したらエントリを消すこと（#1399）"
    );
}

/// GUI の `eprintln!` は誰も読めない（#1399 → #1422 で全 UI モジュールへ）。
///
/// #1399 の検査は `sidebar.rs` 限定だったので、同じ「押しても無言」が
/// `right_panel.rs`（tmux の復元・バックグラウンド復帰）と `preview_render.rs`
/// （コードのコピー・Code Runner・PDF 再ラスタライズ）に残っていた。
/// スコープ外のファイルは [`KNOWN_EPRINTLN`] の件数で段階導入する
#[test]
fn uiモジュールにeprintlnだけで終わる経路が無い() {
    let root = workspace_root();
    let mut hits: Vec<String> = Vec::new();
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for path in ui_modules(&root) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let src = std::fs::read_to_string(&path).expect("UI モジュールが読める");
        let found: Vec<String> = production_lines(&src)
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains("eprintln!"))
            .map(|(i, _)| format!("{name}:{}", i + 1))
            .collect();
        let allowed = KNOWN_EPRINTLN
            .iter()
            .find(|(f, _)| *f == name)
            .map(|(_, n)| *n)
            .unwrap_or(0);
        counts.insert(name.clone(), found.len());
        if found.len() > allowed {
            hits.extend(found.into_iter().skip(allowed));
        }
    }
    assert!(
        hits.is_empty(),
        "{hits:?} が `eprintln!` で失敗を報告している（#1399 / #1422）。GUI の stderr は\
         誰も読めないので、ユーザーの操作に対する失敗は通知欄\
         （`notify_ui_dispatch_failed` / `notify_ui_op_failed`）へ、背景処理の失敗は\
         `TakoApp::log_ui_failure` で persist.log へ出すこと"
    );
    let stale: Vec<&str> = KNOWN_EPRINTLN
        .iter()
        .filter(|(f, n)| counts.get(*f).copied().unwrap_or(0) < *n)
        .map(|(f, _)| *f)
        .collect();
    assert!(
        stale.is_empty(),
        "KNOWN_EPRINTLN の {stale:?} は宣言した件数より減っている（#1422）。\
         既知リストは減る方向にしか動かさないので、直したら数も減らすこと\
         （実測: {counts:?}）"
    );
}

#[test]
fn ローカル操作の失敗は1実装から出ている() {
    let root = workspace_root();
    let src = sidebar_src(&root);
    // 出し口の本体（persist.log + 共有の通知欄 + A/B の逃げ道）
    let body = src
        .split_once("fn notify_ui_failure(")
        .map(|(_, rest)| rest.chars().take(1200).collect::<String>())
        .expect("`notify_ui_failure` が sidebar.rs に無い（#1399 / #1417 の出し口）");
    for mark in [
        "Self::log_ui_failure(",
        "self.set_remote_notice(",
        "arm.suppressed()",
    ] {
        assert!(
            body.contains(mark),
            "`notify_ui_failure` が {mark} を通っていない（#1399 / #1417 / #1422）。\
             通知欄と persist.log と A/B の逃げ道は 1 実装が担うこと"
        );
    }
    // 診断の書式は背景処理と共有する（grep が 1 通りで済む）
    let log = src
        .split_once("fn log_ui_failure(")
        .map(|(_, rest)| rest.chars().take(700).collect::<String>())
        .expect("`log_ui_failure` が sidebar.rs に無い（#1422 の背景処理の出し口）");
    for mark in ["diag::persist_log", "arm.suppressed()", "area.tag()"] {
        assert!(
            log.contains(mark),
            "`log_ui_failure` が {mark} を通っていない（#1422）。背景処理の失敗も\
             A/B の逃げ道と `area=` つきの 1 書式で診断へ残すこと"
        );
    }
    // 薄い包み 4 本はすべてこの出し口を呼ぶ（画面へ出す経路を 2 本にしない）
    for wrapper in [
        "fn notify_ui_dispatch_failed(",
        "fn notify_ui_op_failed(",
        "fn notify_tree_open_failed(",
    ] {
        let after = src
            .split_once(wrapper)
            .map(|(_, rest)| rest.chars().take(700).collect::<String>())
            .unwrap_or_else(|| panic!("{wrapper} が sidebar.rs に無い（#1399 / #1417）"));
        assert!(
            after.contains("self.notify_ui_failure("),
            "{wrapper} が `notify_ui_failure` を通っていない（#1399 / #1417）"
        );
    }
    for (wrapper, inner) in [
        (
            "fn notify_tree_dispatch_failed(",
            "self.notify_ui_dispatch_failed(",
        ),
        ("fn notify_tree_op_failed(", "self.notify_ui_op_failed("),
    ] {
        let tree = src
            .split_once(wrapper)
            .map(|(_, rest)| rest.chars().take(700).collect::<String>())
            .unwrap_or_else(|| panic!("{wrapper} が sidebar.rs に無い（#1399）"));
        assert!(
            tree.contains(inner),
            "{wrapper} が画面横断の出し口（{inner}）を通っていない（#1417 / #1422）"
        );
    }
    // 文言は ui_text のカタログ経由（render へ直書きしない = i18n の規約）
    for key in ["notice_op_failed(", "notice_open_failed("] {
        assert!(
            src.contains(&format!("crate::ui_text::sidebar::{key}")),
            "通知文言 {key} が ui_text のカタログを通っていない（#1399 / #435）"
        );
    }
}

/// ツリー以外の画面（右パネル・プレビュー）も**同じ 1 実装**から出していること（#1417）。
///
/// 画面ごとに `set_remote_notice` を直に呼ぶと、出し方（persist.log を残すか /
/// A/B の逃げ道があるか）が画面ごとにずれて「片方だけ無言」がまた生まれる。
/// #1399 の 1 実装を**再利用**して 3 実装目を作らないことを機械で縛る
#[test]
fn 別画面の失敗も同じ出し口から出ている() {
    let root = workspace_root();
    for (file, handlers) in [
        (
            "crates/tako-app/src/right_panel.rs",
            &[
                "fn tmux_window_row_clicked(",
                "fn tmux_restore_clicked(",
                "fn shelved_restore_clicked(",
            ][..],
        ),
        (
            "crates/tako-app/src/preview_render.rs",
            &[
                "fn preview_outline_item_clicked(",
                "fn preview_page_item_clicked(",
                "fn preview_code_copy_clicked(",
            ][..],
        ),
    ] {
        let src = std::fs::read_to_string(root.join(file)).expect("UI モジュールが読める");
        let name = file.rsplit('/').next().unwrap_or(file);
        for handler in handlers {
            let body = src
                .split_once(handler)
                .map(|(_, rest)| fn_body(rest))
                .unwrap_or_else(|| {
                    panic!(
                        "{name} に `{handler}` が無い（#1417）。\
                         クリックの中身は `render` のクロージャから切り出しておくこと\
                         （合成マウスイベントは GPUI へ届かないので、セルフテストが\
                         呼べる名前が無いと前後比較が取れない）"
                    )
                });
            assert!(
                body.contains("self.notify_ui_dispatch_failed(")
                    || body.contains("self.notify_ui_op_failed("),
                "{name} の `{handler}` が共有の出し口（`notify_ui_dispatch_failed` /\
                 `notify_ui_op_failed`）を通っていない（#1417 / #1422）"
            );
            assert!(
                body.contains("NoticeArea::"),
                "{name} の `{handler}` が画面（`NoticeArea`）を渡していない（#1417）。\
                 persist.log の `area=` がどの画面か分からなくなる"
            );
        }
        // 通知欄も診断も画面モジュールから直に触らない（#1417 / #1422）。
        // `persist_log` の直呼びは `area=` の書式と A/B の逃げ道を素通りするので、
        // 背景処理であっても `TakoApp::log_ui_failure` を通す
        for (mark, why) in [
            (
                "set_remote_notice(",
                "共有の通知欄を直に呼んでいる（#1417）。出し口は `notify_ui_dispatch_failed` /                  `notify_ui_op_failed` の 1 つに保つこと",
            ),
            (
                "diag::persist_log(",
                "診断へ直に書いている（#1422）。背景処理の失敗も `TakoApp::log_ui_failure`                  を通すこと（直呼びは `area=` の書式と A/B の逃げ道を素通りする）",
            ),
        ] {
            let raw: Vec<&str> = src.lines().collect();
            let direct: Vec<String> = strip_comment_lines(&raw)
                .iter()
                .enumerate()
                .filter(|(_, l)| l.contains(mark))
                .map(|(i, _)| format!("{name}:{}", i + 1))
                .collect();
            assert!(direct.is_empty(), "{direct:?} が{why}");
        }
    }
    // 出し口の家（`sidebar.rs`）でも、診断へ書くのは 1 実装の中だけ
    let sidebar = sidebar_src(&root);
    let raw: Vec<&str> = sidebar.lines().collect();
    let writes: Vec<String> = strip_comment_lines(&raw)
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("diag::persist_log("))
        .map(|(i, _)| format!("sidebar.rs:{}", i + 1))
        .collect();
    assert_eq!(
        writes.len(),
        1,
        "sidebar.rs が診断へ {} 箇所から書いている（#1422）。`log_ui_failure` の 1 実装に\
         保つこと（実測: {writes:?}）",
        writes.len()
    );
}

/// dispatch を通らない失敗（OS のアプリ選択ダイアログ）も捨てていないこと（#1399）。
///
/// 「このアプリで開く…」は B8 の `open_with_dialog` を直に呼ぶので dispatch の走査には
/// 引っかからない。`let _ =` で捨てられていた最後の 1 か所がここだった
#[test]
fn osダイアログの失敗も捨てていない() {
    let root = workspace_root();
    let src = sidebar_src(&root);
    let raw: Vec<&str> = src.lines().collect();
    let lines = strip_comment_lines(&raw);
    let hits: Vec<String> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("let _ = pick_app_and_open("))
        .map(|(i, _)| format!("sidebar.rs:{}", i + 1))
        .collect();
    assert!(
        hits.is_empty(),
        "{hits:?} が `pick_app_and_open` の結果を捨てている（#1399）。\
         背景タスクからモデルへ戻して `notify_tree_op_failed` で通知欄へ出すこと"
    );
    assert!(
        src.contains("pick_app_and_open("),
        "`pick_app_and_open` の呼び出しが消えた（この検査が空振りしている。#1399）"
    );
}

#[test]
fn リネームの失敗で打った名前を捨てていない() {
    let root = workspace_root();
    let src = sidebar_src(&root);
    let body = src
        .split_once("fn commit_inline_edit(")
        .map(|(_, rest)| rest.chars().take(2600).collect::<String>())
        .expect("`commit_inline_edit` が sidebar.rs に無い");
    assert!(
        !body.contains("self.inline_edit.take()"),
        "`commit_inline_edit` が先頭で `inline_edit.take()` している（#1399）。\
         `Err` のときに入力欄が閉じて**打った名前ごと消える**（既存名へのリネームが\
         打ち直しになる）。`clone()` で読み、閉じるのは成功したときだけにすること"
    );
    assert!(
        body.contains("self.inline_edit = None;"),
        "`commit_inline_edit` が成功時に入力欄を閉じていない（#1399）"
    );
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let src = sidebar_src(&root);
    let (offenders, calls) = scan_discarded("sidebar.rs", &src);
    assert!(
        calls >= 13,
        "sidebar.rs の dispatch 呼び出しが {calls} 件しか見えていない（#1399 の表は 13 行）。\
         走査の当たり判定が壊れている可能性がある"
    );
    assert!(
        offenders.is_empty(),
        "sidebar.rs に捨てている箇所が残っている: {:?}",
        offenders.iter().map(Offender::report).collect::<Vec<_>>()
    );
}

/// 診断へ出す**理由の分類**がバリアントごとに分かれていて、本文を含まないこと（#1399）。
///
/// 分類が重複すると「どの種類で断られたか」が診断から消える。逆に本文
/// （パス・OS のエラー文）が混ざると、通知欄にだけ出すつもりの文字列が
/// persist.log へ溜まる（#1376 と同じ作法を守れていない）
#[test]
fn dispatchエラーの分類は重複せず本文を含まない() {
    use tako_control::DispatchError as E;
    let all = [
        E::PaneNotFound(1),
        E::TabNotFound(2),
        E::NoTargetPane,
        E::NoSession(3),
        E::InvalidParams("SECRET-BODY".into()),
        E::Operation("SECRET-BODY".into()),
    ];
    let classes: Vec<&str> = all.iter().map(E::class).collect();
    for c in &classes {
        assert!(
            !c.is_empty() && c.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'),
            "分類 {c:?} が識別子の形でない（#1399）"
        );
        assert!(
            !c.contains("SECRET"),
            "分類 {c:?} にエラー本文が混ざっている（#1399。診断へ載せるのは分類だけ）"
        );
    }
    let mut uniq = classes.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(
        uniq.len(),
        classes.len(),
        "分類が重複している（#1399）: {classes:?}"
    );
}

/// 検出力の担保: 番犬自身が空振りしないこと（#1399 で直した 3 つの形そのものを与える）
#[test]
fn 番犬は3つの捨て方を見逃さず扱った形は許す() {
    let call = concat!("tako_control::", "dispatch(");
    // ① `let _ =`
    let a = format!(
        "fn f(&mut self) {{\n    let _ = {call}\n        self,\n        Request::FileOp {{ op }},\n        PaneOrigin::User,\n    );\n}}\n"
    );
    // ② `if let Ok(`
    let b = format!(
        "fn f(&mut self) {{\n    if let Ok(result) = {call}\n        self,\n        Request::FileOp {{ op }},\n        PaneOrigin::User,\n    ) {{\n        use_it(result);\n    }}\n}}\n"
    );
    // ③ `if result.is_ok()`（`Err` を一度も見ない）
    let c = format!(
        "fn f(&mut self) {{\n    let result = {call}\n        self,\n        Request::FileOp {{ op }},\n        PaneOrigin::User,\n    );\n    if result.is_ok() {{\n        refresh();\n    }}\n}}\n"
    );
    // ④ 扱った形（`match` の `Err` アーム）
    let ok = format!(
        "fn f(&mut self) {{\n    match {call}\n        self,\n        Request::FileOp {{ op }},\n        PaneOrigin::User,\n    ) {{\n        Ok(_) => refresh(),\n        Err(e) => self.notify_tree_dispatch_failed(label, None, &e),\n    }}\n}}\n"
    );
    for (label, src, want) in [
        ("let _ =", &a, true),
        ("if let Ok(", &b, true),
        ("is_ok() だけ", &c, true),
        ("match の Err アーム", &ok, false),
    ] {
        let (found, calls) = scan_discarded("synthetic.rs", src);
        assert_eq!(calls, 1, "{label}: 呼び出しを 1 件と数えられていない");
        assert_eq!(
            !found.is_empty(),
            want,
            "{label}: 判定が想定と違う（found={:?}）",
            found.iter().map(Offender::report).collect::<Vec<_>>()
        );
        if want {
            assert_eq!(
                found[0].variant, "FileOp",
                "{label}: Request 名を拾えていない"
            );
            assert_eq!(
                found[0].line, 2,
                "{label}: 行番号が呼び出し行を指していない"
            );
        }
    }

    // ⑤ 窓が**隣の呼び出しへ食い込まない**（③ の直後に ④ があっても ③ は落ちる）
    let mixed = format!("{c}{ok}");
    let (found, calls) = scan_discarded("synthetic.rs", &mixed);
    assert_eq!(calls, 2, "混在: 呼び出しを 2 件と数えられていない");
    assert_eq!(
        found.len(),
        1,
        "隣の `Err` 処理を自分のものと数えている（窓の上限が効いていない）: {:?}",
        found.iter().map(Offender::report).collect::<Vec<_>>()
    );

    // ⑥ コメントの中の `Err(` を証拠と数えない
    let commented = format!(
        "fn f(&mut self) {{\n    let result = {call}\n        self,\n        Request::FileOp {{ op }},\n        PaneOrigin::User,\n    );\n    // Err(e) はここでは扱わない\n    if result.is_ok() {{ refresh(); }}\n}}\n"
    );
    let (found, _) = scan_discarded("synthetic.rs", &commented);
    assert_eq!(
        found.len(),
        1,
        "コメント中の `Err(` を「扱った証拠」と数えている"
    );
}

/// #1422 の本体: 窓の中の**無関係な** `Err(` を「扱った証拠」と数えないこと。
///
/// `right_panel.rs:2022` はこの形で素通りしていた（結果は `if opened.is_ok()` でしか
/// 見ていないのに、その中の `if let Err(e) = this.attach_pending_sessions(cx)` で緑）。
/// 束縛名を追えば落ちる
#[test]
fn 番犬は無関係なエラーに騙されない() {
    let call = concat!("tako_control::", "dispatch(");
    let head = format!(
        "fn f(&mut self) {{\n    let opened = {call}\n        self,\n        Request::TmuxOpen {{ session }},\n        PaneOrigin::User,\n    );\n"
    );
    // ① 他人の Err を窓に置いても落ちる（#1422 の実物）
    let decoy = format!(
        "{head}    if opened.is_ok() {{\n        if let Err(e) = this.attach_pending_sessions(cx) {{\n            eprintln!(\"{{e}}\");\n        }}\n    }}\n}}\n"
    );
    // ② 似た名前（`opened_at`）を証拠と数えない
    let lookalike = format!(
        "{head}    if let Err(e) = opened_at.check() {{\n        report(e);\n    }}\n    if opened.is_ok() {{ refresh(); }}\n}}\n"
    );
    // ③ 自分の結果を扱っていれば許す（3 つの正しい形）
    let by_if_let = format!("{head}    if let Err(e) = opened {{\n        self.notify_ui_dispatch_failed(area, arm, op, None, &e);\n    }}\n}}\n");
    let by_match = format!("{head}    match opened {{\n        Ok(_) => refresh(),\n        Err(e) => self.notify_ui_dispatch_failed(area, arm, op, None, &e),\n    }}\n}}\n");
    let by_is_err = format!("{head}    if opened.is_err() {{\n        self.notify_ui_op_failed(area, arm, op, None, \"ng\");\n    }}\n}}\n");
    // ④ 呼び出し式に続けて扱う形（`command_card_ui.rs` の `.map_err(..)` + 呼び出し元へ返す）
    let by_map_err = format!(
        "fn f(&mut self) -> Result<Value, String> {{\n    let result = {call}\n        self,\n        Request::ShowCommand {{ action }},\n        PaneOrigin::User,\n    )\n    .map_err(|e| e.to_string());\n    for (pane, options) in take(&mut self.pending_attach) {{\n        if let Err(e) = self.spawn_session(pane, options, cx) {{\n            drop(e);\n        }}\n    }}\n    result\n}}\n"
    );
    for (label, src, want) in [
        ("無関係な Err（#1422 の実物）", &decoy, true),
        ("似た名前の識別子", &lookalike, true),
        ("if let Err で自分を見る", &by_if_let, false),
        ("match の Err アーム", &by_match, false),
        ("is_err() で自分を見る", &by_is_err, false),
        ("map_err で呼び出し元へ返す", &by_map_err, false),
    ] {
        let (found, calls) = scan_discarded("synthetic.rs", src);
        assert_eq!(calls, 1, "{label}: 呼び出しを 1 件と数えられていない");
        assert_eq!(
            !found.is_empty(),
            want,
            "{label}: 判定が想定と違う（found={:?}）",
            found.iter().map(Offender::report).collect::<Vec<_>>()
        );
    }
}

/// `eprintln!` の走査が**テストモジュールを見ない**こと（#1422）。
///
/// `[perf]` の計測や `[skip]` の理由は誰も読めない出力ではないので違反ではない。
/// 逆に production の 1 行は、同じファイルの下にテストが在っても落とす
#[test]
fn eprintlnの走査はテストモジュールを見ない() {
    let prod = "fn f() {\n    eprintln!(\"warning: 失敗\");\n}\n";
    let test_mod = "#[cfg(test)]\nmod tests {\n    #[test]\n    fn perf() {\n        eprintln!(\"[perf] 12ms\");\n    }\n}\n";
    let nested = "impl A {\n    #[cfg(test)]\n    mod inner {\n        fn g() {\n            eprintln!(\"[perf] x\");\n        }\n    }\n}\n";
    let count = |src: &str| {
        production_lines(src)
            .iter()
            .filter(|l| l.contains("eprintln!"))
            .count()
    };
    assert_eq!(
        count(test_mod),
        0,
        "テストモジュールの `eprintln!` を数えている"
    );
    assert_eq!(count(nested), 0, "入れ子のテストモジュールを切れていない");
    assert_eq!(count(prod), 1, "production の `eprintln!` を見落としている");
    assert_eq!(
        count(&format!("{prod}{test_mod}")),
        1,
        "テストが同居するファイルで production の 1 行を見落としている"
    );
    // `#[cfg(test)]` の関数（`mod` ではない）は切らない
    let cfg_fn = "impl A {\n    #[cfg(test)]\n    fn probe(&self) -> u8 {\n        1\n    }\n}\nfn f() {\n    eprintln!(\"warning: 失敗\");\n}\n";
    assert_eq!(
        count(cfg_fn),
        1,
        "`#[cfg(test)] fn` をモジュールと誤認して切っている"
    );
}
