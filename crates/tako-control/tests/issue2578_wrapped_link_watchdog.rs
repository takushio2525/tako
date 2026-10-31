//! 番犬: 折り返しをまたぐ URL / パスは cmd+クリック・ホバー・cmd+右クリックのメニュー・
//! `tako links` のすべてで 1 本のリンクになる（#2578）
//!
//! ## なぜ止めるのか
//!
//! ユーザー報告（2026-10-31）: ペインの幅で改行された URL / パスを cmd+クリックすると、行の
//! 切れ目までで切れたものが開く。実測の真因は**アプリ自身の折り返し**だった。claude は
//! 長い URL / パスを行の上限の桁で語の途中でも改行し、続きを本文の列まで字下げして書く
//! （2.1.301・124 桁で実測）。検出は「右端まで埋まった行だけを次の行の先頭へ直結する」
//! 作りだったので、字下げの空白でトークンが終わり、1 行目までのリンクになっていた
//! （直結のペインでも器 = tmux 越しのペインでも同じ）。
//!
//! ## 3 本立て
//!
//! 1. [`リンクの検出はguiとdispatchの1か所ずつだけ`] — GUI の 3 経路（cmd+クリック・ホバー・
//!    cmd+右クリック）は `refresh_pane_links` の 1 か所、CLI / MCP は dispatch の `Links` の
//!    1 か所から `tako_core::links` を呼ぶ。2 つ目の検出が生えると、片方だけ直る・片方だけ
//!    壊れる形に戻る
//! 2. [`claudeの形で折り返したurlとパスを1本で返す`] — 公開の入口（dispatch と同じ関数）で、
//!    claude の形の折り返しが 1 本になる。修正前の結合（`TAKO_2578_LEGACY=1` と同じ判断）では
//!    切れることも同じ材料で確かめる（材料が症状を再現していないと何も見ていない番犬になる）
//! 3. [`たまたま行末で終わったurlは次の行とつながない`] — つなぐ判定の確かめ（URL は行の幅より
//!    長いときだけ）が抜けて、地の文まで URL に取り込む形へ倒れていないこと
//!
//! 落ちたときは判定の実装（`crates/tako-core/src/links.rs` の関数）を `file:line` で名指しする

use std::path::{Path, PathBuf};
use tako_core::links::{self, DetectedLink};

const LINKS_RS: &str = "crates/tako-core/src/links.rs";
const APP_MAIN: &str = "crates/tako-app/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";

/// リンクを検出する tako-core の入口（呼び出しの形で探す）
const DETECT_CALLS: [&str; 6] = [
    "detect_links_with_cwd(",
    "detect_links_with_home(",
    "detect_links_with_home_in(",
    "detect_wrapped_in(",
    "detect_in_lines(",
    "detect_links(",
];

/// 許す呼び出し（ファイルと、その行に必ず在る文字列）
const ALLOWED: [(&str, &str); 2] = [
    (
        APP_MAIN,
        "tako_core::detect_links_with_cwd(&screen, cwd.as_deref())",
    ),
    (DISPATCH, "tako_core::links::detect_links_with_home(&screen"),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// `needle` が在る行の `file:line`（1-indexed）
fn line_of(rel: &str, needle: &str) -> String {
    let src = read(rel);
    let line = src
        .lines()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{rel}: {needle:?} が見つからない（改名したら番犬も直す）"));
    format!("{rel}:{}", line + 1)
}

#[test]
fn リンクの検出はguiとdispatchの1か所ずつだけ() {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in ["crates/tako-app/src", "crates/tako-control/src"] {
        rs_files(&root.join(dir), &mut files);
    }
    files.sort();
    let mut hits: Vec<(String, usize, String)> = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let src = std::fs::read_to_string(file).unwrap();
        for (idx, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if DETECT_CALLS.iter().any(|call| code.contains(call)) {
                hits.push((rel.clone(), idx + 1, line.trim().to_string()));
            }
        }
    }
    let extra: Vec<String> = hits
        .iter()
        .filter(|(rel, _, line)| {
            !ALLOWED
                .iter()
                .any(|(file, needle)| rel == file && line.contains(needle))
        })
        .map(|(rel, line_no, line)| format!("{rel}:{line_no}: {line}"))
        .collect();
    assert!(
        extra.is_empty(),
        "リンクの検出が 2 か所目から呼ばれている。GUI は `refresh_pane_links`（{APP_MAIN}）、\n\
         CLI / MCP は dispatch の `Links`（{DISPATCH}）の 1 か所ずつから `tako_core::links` を呼び、\n\
         cmd+クリック・ホバー・cmd+右クリックはその結果（`pane_links`）を読む（#2578）:\n  {}",
        extra.join("\n  ")
    );
    for (file, needle) in ALLOWED {
        let n = hits
            .iter()
            .filter(|(rel, _, line)| rel == file && line.contains(needle))
            .count();
        assert_eq!(
            n, 1,
            "{file}: 許した呼び出し {needle:?} が {n} か所（走査先を取り違えている / 改名した）"
        );
    }
    // GUI の 3 経路が読むキャッシュは `refresh_pane_links` だけが書く
    let main = read(APP_MAIN);
    let writers: Vec<usize> = main
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("self.pane_links.insert("))
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(
        writers.len(),
        1,
        "{APP_MAIN}: `pane_links` を書く場所が {writers:?} 行目（cmd+クリック・ホバー・\
         cmd+右クリックが別々の検出結果を読む形になっている）"
    );
}

/// claude の返答と同じ形（`* ` + 前置き、続きは 2 桁の字下げで右端まで詰める）。
/// 印を ASCII にしているのは `screen_from_lines` の列の数え方（非 ASCII = 2 桁）に左右されないため
fn fold(lead: &str, token: &str, cols: usize) -> Vec<String> {
    let mut rows = vec![format!("* {lead}")];
    for ch in token.chars() {
        if rows.last().unwrap().chars().count() == cols {
            rows.push("  ".to_string());
        }
        rows.last_mut().unwrap().push(ch);
    }
    rows
}

fn whole<'a>(links: &'a [DetectedLink], want: &str) -> Option<&'a DetectedLink> {
    let cut = links
        .iter()
        .any(|l| l.target != want && want.starts_with(&l.target));
    links.iter().find(|l| l.target == want).filter(|_| !cut)
}

#[test]
fn claudeの形で折り返したurlとパスを1本で返す() {
    let dir = std::env::temp_dir().join(format!("tako_2578_watchdog_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir
        .join("a-very-long-directory-name-for-the-wrap-test-2578")
        .join("target-file-for-the-wrapped-link-check.txt");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "").unwrap();
    let url = format!(
        "https://example.com/tako/issues/2578?first={}&second={}",
        "a".repeat(40),
        "b".repeat(40)
    );
    let path = file.display().to_string();
    let cols = 50;
    let mut lines = fold("See ", &url, cols);
    lines.extend(fold("Open ", &path, cols));
    lines.push("tako %".to_string());
    let screen = links::screen_from_lines(&lines, cols);

    // 材料が症状を再現している（修正前の結合では 1 行目までで切れる）
    let legacy = links::detect_wrapped_in(&screen, None, None, false, true);
    assert!(
        whole(&legacy, &url).is_none() && whole(&legacy, &path).is_none(),
        "材料が #2578 の症状を再現していない（修正前の結合でも 1 本になる）= 何も見ていない:\n{legacy:#?}"
    );

    // 公開の入口（dispatch の `Links` と同じ関数。GUI の `detect_links_with_cwd` もここを通る）
    let found = links::detect_links_with_home(&screen, None, None);
    let missing: Vec<&str> = [url.as_str(), path.as_str()]
        .into_iter()
        .filter(|want| whole(&found, want).is_none())
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        missing.is_empty(),
        "claude の形（改行 + 字下げ）で折り返した URL / パスが 1 本にならない: {missing:?}\n\
         続きの形: {}\n  行の上限: {}\n  2 回目の走査と置き換え: {}\n  links={found:#?}",
        line_of(LINKS_RS, "fn app_continuation("),
        line_of(LINKS_RS, "fn app_wraps("),
        line_of(LINKS_RS, "fn detect_all("),
    );
    for want in [&url, &path] {
        let link = whole(&found, want).unwrap();
        assert_eq!(
            link.wrap,
            Some(links::LinkWrap::App),
            "{want}: 折り返しの種類（`tako links` の wrap）が app でない"
        );
        assert!(link.spans.len() >= 2, "{want}: またがった各行の位置が無い");
    }
}

#[test]
fn たまたま行末で終わったurlは次の行とつながない() {
    let url = "https://example.com/docs/page-0123456789";
    let head = format!("* The doc is {url}");
    let cols = head.len();
    let lines = vec![
        head,
        "  and the rest of the sentence.".to_string(),
        "tako %".to_string(),
    ];
    let screen = links::screen_from_lines(&lines, cols);
    let found = links::detect_links_with_home(&screen, None, None);
    let targets: Vec<&str> = found.iter().map(|l| l.target.as_str()).collect();
    assert_eq!(
        targets,
        vec![url],
        "行の幅に収まる URL（アプリが途中で切る理由が無い）を次の行の地の文とつないだ。\n\
         URL の確かめ: {}",
        line_of(LINKS_RS, "fn app_wrapped_url_plausible(")
    );
}
