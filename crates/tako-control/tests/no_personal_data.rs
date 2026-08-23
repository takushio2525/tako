//! 個人情報（実ユーザー名・実ホームパス・実ホスト名）の再発防止番犬（#927）。
//!
//! tako は public リポで、個人情報をソース・ドキュメント・スクリプト・設定サンプルへ
//! 置かないことは全リポ共通の最重要ルール。混入経路はほぼ 1 本しかない:
//! **実機で採取した出力（ペインの capture・プロンプト行・env の値）をそのまま貼る**。
//! 2026-08-24 に現行コードから 4 ファイル分を除去したので、同じ経路で戻ってこないように
//! ここで止める。
//!
//! # 検査は 2 本立て（片方だけでは穴が残る）
//!
//! 1. [`ホームパス形の名前はプレースホルダだけ`] — CI で効く。
//!    `/Users/<名前>` / `/home/<名前>` / `C:\Users\<名前>` の `<名前>` は
//!    [`PLACEHOLDER_NAMES`] のどれかでなければならない。実機の採取物を貼ると
//!    ほぼ必ずこの形になるので、**誰のマシン由来でも**捕まる。
//! 2. [`このマシンの識別子がリポに出ていない`] — 手元で効く。`HOME` / `USERPROFILE` の
//!    basename・`USER` / `USERNAME`・ホスト名が、パス形でなく素の語で出ていても落とす
//!    （#927 で除去した 2 箇所目 `contains("<実ユーザー名>")` は 1 では捕まらない形だった）。
//!    値を漏らすのは「自分の値を貼った人」なので、**その人の手元で必ず落ちる**。
//!
//! # なぜ検出語のハッシュをリポに置かないか
//!
//! Issue #927 は「検出語はハッシュ化 or 環境変数で持つ」を案として挙げていたが、
//! **ハッシュは採らなかった**。ユーザー名のような短く形の決まった語の SHA-256 は
//! 総当たりで戻せるので、除去したはずの値を別の形で public リポへ置くことになる。
//! 代わりに 1 は値を持たない許可リスト方式、2 は実行時の環境から取る形にした。
//! CI で特定の語も見張りたい場合は `TAKO_PII_TERMS`（`,` 区切り）で外から渡す
//! （GitHub Actions なら secret 経由。リポには何も残らない）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// ホームパスの `<名前>` として置いてよいプレースホルダ。
///
/// **実在の人名・アカウント名は絶対に足さない。** テストが新しい名前を必要とするなら
/// まずここにある値を使い回す。どうしても増やすなら「架空だと一目で分かる語」にする。
const PLACEHOLDER_NAMES: &[&str] = &[
    // 1〜2 文字の記号的なもの（既存テストが大量に使っている）
    "a",
    "b",
    "u",
    "x",
    "me",
    // 汎用語
    "dev",
    "devuser",
    "foo",
    "someone",
    "tako",
    "test",
    "testuser",
    "user",
    "winuser",
    // 人名っぽく見せたい枝（前方一致の対照 alice2 を含む）
    "alice",
    "alice2",
    "bob",
    "山田",
    // 空白入り（Windows のユーザー名に空白が入る枝の検証用）
    "a b",
    "First Last",
    "John Smith",
    "My Name",
    // ドキュメント中の省略
    "...",
    "…",
    // パス操作（名前ではない）
    ".",
    "..",
];

/// 検査 2 で「これは個人情報ではない」と分かっている語。
///
/// CI ランナーのシステムユーザーや、リポジトリの公開 URL に必要な GitHub アカウント名。
const NON_PERSONAL_TERMS: &[&str] = &[
    "runner", // GitHub Actions の実行ユーザー
    "root",   // コンテナ
    "admin",
    "administrator",
    "ubuntu",
    "localhost",
    "unknown",      // remote::hostname() の失敗時の値
    "takushio2525", // リポジトリの公開 URL に必要（公開情報・除去対象ではない）
];

/// 走査しないディレクトリ（生成物・依存物）
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".astro",
    ".playwright-mcp",
    ".venv",
    ".wrangler",
    "dist",
    "node_modules",
    "target",
];

/// 1 ファイルの読み込み上限。これを超えるものは走査しない（生成物対策）
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// リポジトリルート（`crates/tako-control` から 2 つ上）
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// 走査対象のテキストファイルを集める。
///
/// 拡張子の許可リストは持たない（`.iss` / `.command` / 拡張子なしを取りこぼすため）。
/// **UTF-8 として読めたものだけ**を対象にするので、画像・アイコンは自然に外れる。
fn text_files(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    collect(root, root, &mut out, &mut skipped);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    // 上限で読み飛ばしたものは黙らせない（「全部見た」と読み違えないため）
    if !skipped.is_empty() {
        skipped.sort();
        eprintln!("[#927] {MAX_FILE_BYTES} バイトを超えるため走査しなかったファイル: {skipped:?}");
    }
    out
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<(PathBuf, String)>, skipped: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // worktree の `.git` は**ファイル**（`gitdir: <本体の絶対パス>` が入っている）なので、
        // ディレクトリかどうかを見る前に名前で外す
        if SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        if path.is_dir() {
            collect(&path, root, out, skipped);
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        if std::fs::metadata(&path)
            .map(|m| m.len() > MAX_FILE_BYTES)
            .unwrap_or(false)
        {
            skipped.push(rel);
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            out.push((rel, text));
        }
    }
}

// --- 検査 1: ホームパス形の名前 ---

/// ホームパスの前置き（`/Users/` / `/home/` / `C:\Users\` / url-encode 形）を
/// `line` の `from` 以降から 1 つ探し、`(前置きの開始, 名前の開始)` を返す。
///
/// 区切りは**連続を許す**（Rust の文字列リテラルは `C:\\Users\\` になる）。
fn find_home_prefix(line: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let lower = line.to_ascii_lowercase();
    let mut i = from;
    while i < bytes.len() {
        if !line.is_char_boundary(i) {
            i += 1;
            continue;
        }
        // `/Users/` / `/home/`
        for tag in ["/users/", "/home/"] {
            if lower[i..].starts_with(tag) {
                let mut end = i + tag.len();
                while lower[end..].starts_with('/') {
                    end += 1;
                }
                return Some((i, end));
            }
        }
        // `<ドライブ>:\Users\` と `<ドライブ>%3A%5CUsers%5C`
        if bytes[i].is_ascii_alphabetic() {
            for (sep, tag) in [("\\", "users"), ("/", "users"), ("%5c", "users")] {
                let colon: &str = if sep == "%5c" { "%3a" } else { ":" };
                let after_drive = i + 1;
                if !lower[after_drive..].starts_with(colon) {
                    continue;
                }
                let mut p = after_drive + colon.len();
                let mut seps = 0;
                while lower[p..].starts_with(sep) {
                    p += sep.len();
                    seps += 1;
                }
                if seps == 0 || !lower[p..].starts_with(tag) {
                    continue;
                }
                p += tag.len();
                let mut seps = 0;
                while lower[p..].starts_with(sep) {
                    p += sep.len();
                    seps += 1;
                }
                if seps == 0 {
                    continue;
                }
                return Some((i, p));
            }
        }
        i += 1;
    }
    None
}

/// 前置きの**直前**が識別子の一部なら、それはパスの先頭ではない
/// （例: `"$PROMO_DEMO/home/Library"` は偽のホームではなく変数の続き）。
fn starts_a_path(line: &str, prefix_start: usize) -> bool {
    line[..prefix_start]
        .chars()
        .next_back()
        .map(|c| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '-'))
        .unwrap_or(true)
}

/// 前置きの直後から「ユーザー名として使える文字」を取れるだけ取る。
///
/// `$` / `{` / `<` / `%` などは含まないので、`/Users/$USER` や `/Users/<name>` は
/// 空セグメント = 具体値なし として扱われる。
fn name_segment(line: &str, from: usize) -> &str {
    let mut end = from;
    for (off, ch) in line[from..].char_indices() {
        let ok = ch.is_alphanumeric() || ch == '_' || ch == '.' || ch == '-' || ch == ' ';
        if !ok {
            break;
        }
        end = from + off + ch.len_utf8();
    }
    line[from..end].trim_end()
}

/// セグメントがプレースホルダか。
///
/// 完全一致に加えて「プレースホルダで始まり、その直後が語の途切れ」も許す
/// （`alice の前方一致…` のように後ろへ日本語の説明が続く行があるため）。
/// `alice2` のように英数が続く形は別物として扱うので、実名の混入は拾える。
fn is_placeholder_segment(seg: &str) -> bool {
    if seg.is_empty() {
        return true;
    }
    PLACEHOLDER_NAMES.iter().any(|p| {
        if seg == *p {
            return true;
        }
        let Some(rest) = seg.strip_prefix(*p) else {
            return false;
        };
        rest.chars()
            .next()
            .map(|c| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(true)
    })
}

/// 1 行からプレースホルダでないホームパス名を拾う
fn offending_segments(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some((prefix_start, name_start)) = find_home_prefix(line, at) {
        at = name_start;
        if starts_a_path(line, prefix_start) {
            let seg = name_segment(line, name_start);
            if !is_placeholder_segment(seg) {
                out.push(seg.to_string());
            }
        }
    }
    out
}

/// **受け入れ条件 3**: `/Users/<実名>` 形のリテラルを足すとここが落ちる。
#[test]
fn ホームパス形の名前はプレースホルダだけ() {
    let root = repo_root();
    let mut offenders = Vec::new();
    for (rel, text) in text_files(&root) {
        for (ln, line) in text.lines().enumerate() {
            for seg in offending_segments(line) {
                offenders.push(format!("{}:{} → {:?}", rel.display(), ln + 1, seg));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "実在しそうなユーザー名がホームパスに書かれている（#927）:\n  {}\n\
         → 実機の採取物をそのまま貼っていないか確認し、\n\
         crates/tako-control/tests/no_personal_data.rs の PLACEHOLDER_NAMES にある\n\
         プレースホルダ（testuser / winuser / 山田 等）へ置き換えてください。\n\
         架空だと一目で分かる名前を新しく増やす場合だけ PLACEHOLDER_NAMES に追記します",
        offenders.join("\n  ")
    );
}

// --- 検査 2: このマシンの識別子 ---

/// このマシン由来の、リポに出てはいけない語を環境から組み立てる（リポには残らない）
fn machine_terms() -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    let mut push = |s: String| {
        let s = s.trim().to_string();
        // 3 文字以下は誤検出が多すぎる（`u` / `me` 等はプレースホルダでもある）
        if s.chars().count() < 4 {
            return;
        }
        let lower = s.to_ascii_lowercase();
        if NON_PERSONAL_TERMS.contains(&lower.as_str()) {
            return;
        }
        if PLACEHOLDER_NAMES.iter().any(|p| p.eq_ignore_ascii_case(&s)) {
            return;
        }
        terms.insert(s);
    };
    for key in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(key) {
            if let Some(base) = Path::new(&home).file_name() {
                push(base.to_string_lossy().to_string());
            }
        }
    }
    for key in ["USER", "USERNAME", "LOGNAME"] {
        if let Ok(v) = std::env::var(key) {
            push(v);
        }
    }
    // ホスト名は短縮形（最初のラベル）で見る
    let host = tako_control::remote::hostname();
    push(host.split('.').next().unwrap_or("").to_string());
    // CI 等から追加で見張りたい語（リポには置かない）
    if let Ok(extra) = std::env::var("TAKO_PII_TERMS") {
        for t in extra.split(',') {
            push(t.to_string());
        }
    }
    terms
}

/// **受け入れ条件 3**: 自分のユーザー名・ホスト名を素の語で貼るとここが落ちる。
///
/// CI（GitHub ランナー）では検出語が `runner` 等だけになり実質空回りするのが正常。
/// 値を漏らすのは「自分の値を貼った人」なので、**その人の手元で**落ちれば足りる。
#[test]
fn このマシンの識別子がリポに出ていない() {
    let terms = machine_terms();
    if terms.is_empty() {
        eprintln!("[#927] このマシンからは検出語を作れなかった（CI では正常）");
        return;
    }
    let lowered: Vec<String> = terms.iter().map(|t| t.to_ascii_lowercase()).collect();
    let root = repo_root();
    let mut offenders = Vec::new();
    for (rel, text) in text_files(&root) {
        // このテスト自身は検出語を持たない（環境から作る）ので走査対象のままでよい
        for (ln, line) in text.lines().enumerate() {
            let lower = line.to_ascii_lowercase();
            for (i, term) in lowered.iter().enumerate() {
                if lower.contains(term.as_str()) {
                    offenders.push(format!(
                        "{}:{}（検出語の長さ {}）",
                        rel.display(),
                        ln + 1,
                        terms.iter().nth(i).map(|t| t.chars().count()).unwrap_or(0)
                    ));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "このマシンの識別子（ユーザー名 / ホスト名）がリポに出ている（#927）:\n  {}\n\
         → public リポなので除去してください。値そのものはここに出しません\n\
         （出すとテスト出力・CI ログ経由で再び漏れるため）。\n\
         該当行を開いて、自分のユーザー名・ホスト名をプレースホルダへ置き換えます",
        offenders.join("\n  ")
    );
}

// --- 番犬自身の検出力（実名を 1 つも書かずに固定する） ---

/// 架空の名前を**実行時に組み立てる**（このファイル自身も走査対象なので、
/// `/Users/<実名らしい語>` の形をリテラルで置くと自分の検査で落ちる）
fn joined(prefix: &str, name: &str, suffix: &str) -> String {
    format!("{prefix}{name}{suffix}")
}

#[test]
fn 判定はプレースホルダと実名らしい語を見分ける() {
    // プレースホルダは通す
    for ok in [
        "/Users/testuser/dev",
        "/Users/u/Library/Application Support/tako",
        r"C:\Users\winuser\dev",
        r"C:\\Users\\My Name\\.claude",
        "/Users/山田",
        "file:///C:/Users/me/a.md",
        "/Users/alice の前方一致だが別ユーザー",
        "/Users/<name>/dev",
        "/Users/$USER/dev",
        "\"$PROMO_DEMO/home/Library/Preferences\"", // 変数の続き = ホームではない
    ] {
        assert!(
            offending_segments(ok).is_empty(),
            "プレースホルダを誤検出した: {ok}"
        );
    }
    // 実名らしい語は落とす。**架空の語を実行時に組み立てる**
    let fake = "kanenashi";
    let prefixed = "alice2x"; // プレースホルダ alice2 の前方一致だが別物
    for (line, expect) in [
        (joined("/Users/", fake, "/dev/tako"), fake),
        (joined(r"HOME(Process)=C:\Users\", fake, ""), fake),
        (joined("/home/", fake, "/.cargo"), fake),
        (joined("/Users/", prefixed, "/dev"), prefixed),
        (joined("file:///C%3A%5CUsers%5C", fake, "/a.md"), fake),
        // Rust の文字列リテラル形（区切りが 2 個）でも拾う
        (joined(r"C:\\Users\\", fake, r"\\.claude"), fake),
    ] {
        assert_eq!(
            offending_segments(&line),
            vec![expect.to_string()],
            "実名らしい語を見逃した: {line}"
        );
    }
}

#[test]
fn 検出語の組み立ては汎用語とプレースホルダを外す() {
    // NON_PERSONAL_TERMS / PLACEHOLDER_NAMES / 3 文字以下 は検出語にしない。
    // 環境を汚さずに規則だけを固定する
    for generic in ["runner", "root", "testuser", "winuser", "tako", "u", "me"] {
        let too_short = generic.chars().count() < 4;
        let excluded = NON_PERSONAL_TERMS.contains(&generic)
            || PLACEHOLDER_NAMES
                .iter()
                .any(|p| p.eq_ignore_ascii_case(generic));
        assert!(
            too_short || excluded,
            "{generic} が検出語に残ると誤検出になる"
        );
    }
    // 逆に、架空の実名らしい語は除外条件に一致しない
    let real_ish = "kanenashi";
    assert!(
        !NON_PERSONAL_TERMS.contains(&real_ish)
            && !PLACEHOLDER_NAMES
                .iter()
                .any(|p| p.eq_ignore_ascii_case(real_ish)),
        "実名らしい語が除外リストに入っている"
    );
}
