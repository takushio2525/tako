//! **#1376 の番犬**: OS の既定ハンドラへ渡す前にスキームを検査する。
//!
//! ## なぜ止めるのか
//!
//! tako が「開く」URL の出どころは第三者が書ける場所しかない（ターミナル画面 /
//! Markdown / **PDF のリンク注釈** / 提案チップ）。OS の既定ハンドラ（macOS の `open` /
//! Windows の `ShellExecuteW`）は **URL でない文字列も開く**ので、`file:` URL・UNC パス
//! （`\\host\share\payload.exe`）・ローカルの実行ファイルパスを渡せば、受け取った PDF の
//! リンクを 1 回クリックするだけで任意のプログラムが起動する。
//!
//! 画面リンク（`links.rs` が http / https しか拾わない）と Markdown
//! （`md_links::browser_url`）には検査があったのに、**PDF と提案チップだけ素通り**
//! だったのが #1376。#1371 で cmd.exe は経路から消えたが、それは「URL が途中で切れない」
//! 話であってスキームの検査ではない。
//!
//! ## 何を固定するか
//!
//! 規則そのもの（何を通し何を弾くか）は `tako_core::url_guard` の単体テストが持つ。
//! ここが止めるのは**構造**で、1 行で戻せてしまう形が 4 通りある:
//!
//! 1. [`pdfリンクの経路がスキーム検査を通っている`] — 経路側（(a)）が外れる
//! 2. [`提案チップの経路がスキーム検査を通っている`] — 同上
//! 3. [`境界b8がスキーム検査を通っている`] — 保険側（(b)）が外れる
//! 4. [`main_rsのurl起動は検査を通る関数の中だけにある`] — 検査を持たない
//!    新しい呼び出し元が増える
//!
//! 加えて、弾いた事実を診断へ残すときに**リンク文字列そのもの**（PDF / ペインの内容に
//! 相当）を載せていないこと（[`弾いた理由にリンク文字列を載せていない`]）と、
//! 判定が 1 実装のままであること（[`md_linksは判定を再実装していない`] /
//! [`許可集合の正本は1か所にある`]）も見る。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中のテスト用ヘルパで走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

const APP: &str = "crates/tako-app/src/main.rs";
const B8: &str = "crates/tako-control/src/platform/os_integration.rs";
const GUARD: &str = "crates/tako-core/src/url_guard.rs";
const MD_LINKS: &str = "crates/tako-core/src/md_links.rs";

/// 検査を通った目印。判定の正本は `url_guard::check_browser_url` で、Markdown 経路は
/// 従来名の再公開（`md_links::browser_url`）で同じ 1 実装を呼ぶ。
/// どちらの綴りも「規則を通した」証拠なので、部分一致で見る
const CHECK: &str = "browser_url";

/// 本番コード（`#[cfg(test)]` の付いた item を空白へ潰した眺め）だけを返す。
/// 切らない理由と「黙って縮んだ」の検出は `common/production_range.rs`（#1420）
fn production(rel: &str) -> String {
    production_range::production(&read(rel), rel)
}

/// 関数 1 本の本文（シグネチャ行から同インデントの `}` まで）。
/// 戻り値は `(シグネチャ行の行番号, コメントを落とした本文)`。
/// コメントを落とすのは、説明文中の `open_url` で落ちないようにするため
fn fn_body(rel: &str, signature: &str) -> (usize, String) {
    let source = production(rel);
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{rel} に `{signature}` が見つからない（改名したら番犬も直す）"));
    let head_line = source[..start].lines().count() + 1;
    // シグネチャは重複回避のため `\n` で始めることがある（`\npub fn open_url(` 等）。
    // インデントは**その行**から読む
    let sig_line = signature.trim_start_matches('\n');
    let indent = " ".repeat(sig_line.len() - sig_line.trim_start().len());
    let close = format!("\n{indent}}}");
    let end = source[start..]
        .find(&close)
        .map(|i| start + i)
        .unwrap_or(source.len());
    let body = source[start..end]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    (head_line, body)
}

/// 「検査してから開く」を本文の**並び**で見る。
/// 検査が無い / 開いたあとにしか無い、のどちらも同じ穴なので 1 本で判定する
fn checked_before_open(rel: &str, signature: &str, checker: &str) {
    let (line, body) = fn_body(rel, signature);
    let open_at = body
        .find("os_integration::open_url")
        .unwrap_or_else(|| panic!("{rel}:{line}: `{signature}` が URL を開いていない（改名?）"));
    match body.find(CHECK) {
        Some(check_at) if check_at < open_at => {}
        Some(_) => panic!(
            "{rel}:{line}: `{signature}` が **開いたあとに**スキーム検査をしている。\n\
             OS の既定ハンドラは URL でない文字列（`file:` / UNC / ローカルパス）も開くので、\n\
             `{checker}` を通してから `open_url` を呼んでください（#1376）"
        ),
        None => panic!(
            "{rel}:{line}: `{signature}` がスキーム検査を通さずに OS の既定ハンドラへ渡している。\n\
             PDF のリンク注釈・提案チップの中身は第三者が書ける任意の文字列で、\n\
             `file:` URL・UNC パス（\\\\host\\share\\payload.exe）・ローカルの実行ファイルパスを\n\
             渡すとクリック 1 回で任意のプログラムが起動する。\n\
             `{checker}` を通してください（#1376）"
        ),
    }
}

/// 1: PDF のリンク注釈は UI の ⌘+クリックも dispatch（CLI / MCP）も検査を通る
#[test]
fn pdfリンクの経路がスキーム検査を通っている() {
    checked_before_open(
        APP,
        "    fn follow_pdf_link(",
        "url_guard::check_browser_url",
    );
    checked_before_open(
        APP,
        "    fn follow_preview_pdf_link(",
        "url_guard::check_browser_url",
    );
}

/// 2: 提案チップ（= プレビューを開く差し替え点）も検査を通る
#[test]
fn 提案チップの経路がスキーム検査を通っている() {
    checked_before_open(APP, "\nfn open_preview(", "url_guard::check_browser_url");
}

/// 3: 境界 B8 の保険。`open_url` / `open_url_wait` の**両方**が検査を通り、
/// 判定は tako-core の 1 実装を呼ぶだけであること（B8 側に規則を書き写さない）
#[test]
fn 境界b8がスキーム検査を通っている() {
    for signature in [
        "\npub fn open_url(url: &str) -> Result<(), String> {",
        "\npub fn open_url_wait(url: &str) -> Result<(), String> {",
    ] {
        let (line, body) = fn_body(B8, signature);
        assert!(
            body.contains("guarded_url(url)?"),
            "{B8}:{line}: 境界 B8 が検査（`guarded_url`）を通さずに OS のハンドラへ渡している。\n\
             経路側（#1376 の (a)）を 1 行外されても穴にならないための保険なので、\n\
             ここも必ず通してください:\n{body}"
        );
    }

    let (line, guard) = fn_body(B8, "\nfn guarded_url(url: &str)");
    assert!(
        guard.contains("tako_core::url_guard::check_os_handler_url"),
        "{B8}:{line}: 検査が tako-core の正本を呼んでいない（規則の写しを B8 に作らない）:\n{guard}"
    );

    // 規則の写し = スキーム名の直書きが B8 側に生えていないこと
    let src = production(B8);
    let mut offenders = Vec::new();
    for (i, l) in src.lines().enumerate() {
        if l.trim_start().starts_with("//") {
            continue;
        }
        for needle in [
            "starts_with(\"http",
            "eq_ignore_ascii_case(\"http",
            "\"x-apple.",
            "\"file:",
        ] {
            if l.contains(needle) {
                offenders.push(format!("{B8}:{}: {}", i + 1, l.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "境界 B8 にスキーム判定の写しがある（許可集合の正本は `url_guard`。#1376）:\n  {}",
        offenders.join("\n  ")
    );
}

/// 4: `main.rs` から OS の URL ハンドラを叩くのは、同じ関数の中で検査している場所だけ。
///
/// 免除は 1 件だけで、根拠つき（`links.rs` が検出の時点で http / https しか拾わない =
/// FR-3.3.2 の表。境界 B8 の保険もかかる）。**増やすときは根拠をここへ書く**
#[test]
fn main_rsのurl起動は検査を通る関数の中だけにある() {
    /// 検査を持たなくてよい関数（シグネチャの一部 → 根拠）
    const EXEMPT: &[(&str, &str)] = &[(
        "    fn open_link(",
        "ターミナル画面のリンク。`tako_core::links` が検出の時点で http / https しか\
         拾わないので、ここは種別（`LinkKind::Url`）で振り分けるだけ（FR-3.3.2）",
    )];

    let src = production(APP);
    // 呼び出し行 → それを含む関数のシグネチャ（直前の `fn ` 行）を逆引きする
    let lines: Vec<&str> = src.lines().collect();
    let mut offenders = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("//") || !line.contains("os_integration::open_url") {
            continue;
        }
        // 呼び出しを含む関数 = 直前の `fn ` 行（物差しは 1 本に揃える）
        let from = lines[..i]
            .iter()
            .rposition(|l| l.contains("fn "))
            .unwrap_or(0);
        if EXEMPT
            .iter()
            .any(|(sig, _)| lines[from].contains(sig.trim()))
        {
            continue;
        }
        // 同じ関数の中に検査があるか（その `fn ` 行から呼び出し行まで）
        if !lines[from..i].iter().any(|l| l.contains(CHECK)) {
            offenders.push(format!("{APP}:{}: {}", i + 1, line.trim()));
        }
    }
    assert!(
        offenders.is_empty(),
        "スキーム検査を通さずに OS の既定ハンドラへ URL を渡している場所がある:\n  {}\n\
         → `tako_core::url_guard::check_browser_url` を通すか、通さない根拠を\n\
         このテストの EXEMPT へ書いてください（#1376）",
        offenders.join("\n  ")
    );
    // 免除が「実在する関数」を指したままであること（改名で空振りしない）
    for (sig, why) in EXEMPT {
        assert!(
            src.contains(sig),
            "{APP}: 免除が指す `{sig}` が無い（根拠: {why}）。改名したら免除も直す"
        );
    }
}

/// 弾いたときに残す診断へ、リンク文字列そのものを載せない（AGENTS.md の絶対ルール。
/// PDF の中身はペイン内容に相当する）。載せてよいのは分類（`UrlBlocked`）だけ
#[test]
fn 弾いた理由にリンク文字列を載せていない() {
    let checks: &[(&str, &str)] = &[
        (APP, "    fn follow_pdf_link("),
        (APP, "    fn follow_preview_pdf_link("),
        (APP, "\nfn open_preview("),
        (B8, "\nfn guarded_url(url: &str)"),
    ];
    for (rel, signature) in checks {
        let (line, body) = fn_body(rel, signature);
        let mut offenders = Vec::new();
        for (i, l) in body.lines().enumerate() {
            // 診断・エラー文の組み立て行だけを見る
            if !(l.contains("persist_log") || l.contains("format!") || l.contains("eprintln!")) {
                continue;
            }
            if l.contains("{url}") || l.contains(", url)") || l.contains("link.url") {
                offenders.push(format!("{rel}:{}: {}", line + i, l.trim()));
            }
        }
        assert!(
            offenders.is_empty(),
            "弾いた理由にリンク文字列そのものを載せている（診断ログへ流れる）:\n  {}\n\
             → 分類（`UrlBlocked`）だけを載せてください（#1376）",
            offenders.join("\n  ")
        );
    }
}

/// 判定は 1 実装。Markdown 経路が自前の規則を持ち直していないこと
#[test]
fn md_linksは判定を再実装していない() {
    let src = production(MD_LINKS);
    let line = src
        .lines()
        .position(|l| l.contains("browser_url"))
        .map(|at| at + 1)
        .unwrap_or(1);
    assert!(
        src.contains("pub use crate::url_guard::browser_url;"),
        "{MD_LINKS}:{line}: `browser_url` が `url_guard` の再公開になっていない。\n\
         PDF / 提案チップ / 境界 B8 と同じ 1 実装でなければ、片方だけ緩む形で\n\
         また穴が開く（#1376）"
    );
    assert!(
        !src.contains("fn browser_url"),
        "{MD_LINKS}:{line}: `browser_url` の実装が md_links 側へ戻っている（正本は url_guard）"
    );
}

/// 許可集合（OS 固有スキーム）の正本が 1 か所にあること
#[test]
fn 許可集合の正本は1か所にある() {
    let guard = production(GUARD);
    assert!(
        guard.contains("pub const OS_HANDLER_SCHEMES: &[&str]"),
        "{GUARD}: 許可集合の正本（`OS_HANDLER_SCHEMES`）が無い"
    );
    // 定義以外の場所で集合を組み直していないか（`url_guard.rs` 以外での定義を禁ずる）
    for rel in [APP, B8, MD_LINKS] {
        let src = production(rel);
        assert!(
            !src.contains("OS_HANDLER_SCHEMES: &[&str]"),
            "{rel}: 許可集合の定義が 2 か所にある（正本は {GUARD}。#1376）"
        );
    }
}

/// 番犬が名指しする行番号がファイルと一致すること。
/// 1 行ずれると直す人が別の行を見に行くので、**物差しそのもの**を固定する
#[test]
fn 名指しする行番号がファイルと一致する() {
    const SIGNATURE: &str = "    fn follow_pdf_link(";
    let (reported, _) = fn_body(APP, SIGNATURE);
    let actual = read(APP)
        .lines()
        .position(|l| l.starts_with(SIGNATURE))
        .expect("follow_pdf_link がある")
        + 1;
    assert_eq!(reported, actual, "番犬が名指しする行番号がずれている");
}
