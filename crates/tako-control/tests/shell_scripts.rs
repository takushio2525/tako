//! シェルスクリプトの静的検査（番犬）
//!
//! `bash -n` では見つからず、その行が実行されるまで潜伏する種類の欠陥を
//! **CI で落とす**ための検査。`.agent/conventions.md` の
//! 「シェルスクリプトで日本語を出すときの変数展開（Issue #837）」と
//! 「シェルスクリプトは macOS 同梱の bash 3.2 で通す（Issue #1499 / #1518）」に
//! 書いてある規約の機械化（同じ節の「`set -e` と EXIT trap を併用するなら番人を通す」= #1864 と
//! 「`"$( … )"` の中のダブルクォートに `{…,…}` を置かない」= #1924 も）。

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 走査対象の .sh を集める（リポジトリ全体を再帰）
///
/// `scripts/` 以外にも `.sh` は居る（`distribution/build-pkg.sh`）ので置き場では絞らない。
/// 生成物・依存物だけディレクトリ名で外す（中身はリポジトリの規約の対象外）。
fn shell_scripts() -> Vec<PathBuf> {
    const SKIP_DIRS: &[&str] = &["target", ".git", "node_modules", "dist", ".wrangler"];
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skip = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| SKIP_DIRS.contains(&n));
                if !skip {
                    walk(&path, out);
                }
            } else if path.extension().is_some_and(|e| e == "sh") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo_root(), &mut out);
    out.sort();
    assert!(!out.is_empty(), "リポジトリに .sh が 1 つも見つからない");
    out
}

/// リポジトリルートからの相対パスで file:line を作る
fn located(path: &Path, line_no: usize) -> String {
    let rel = path
        .strip_prefix(repo_root())
        .unwrap_or(path)
        .display()
        .to_string();
    format!("{rel}:{line_no}")
}

/// `$var` の直後に非 ASCII が続く箇所を探す（`${var}` は対象外）。
///
/// UTF-8 ロケールの bash は全角 `（` などのバイトを**変数名の一部として取り込む**ので、
/// `$var（` は `var\xef…` という名前の参照になり `set -u` の下で即死する。
/// `bash -n` では検出できず、日本語を出すその行が実行されるまで潜伏する
/// （`build-app.sh` の「不明な引数」案内は #837 まで壊れたままだった）。
fn unbraced_before_multibyte(line: &str) -> bool {
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != '$' {
            i += 1;
            continue;
        }
        // `${...}` / `$(...)` は対象外。素の名前だけを見る。
        // **先頭が英字か `_` のときだけ**が対象: `$1（` のような位置パラメータは
        // 1 文字で確定するので全角を取り込まない（bash が数字を 1 桁しか読まない）
        let mut j = i + 1;
        let starts_name = bytes
            .get(j)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_');
        if starts_name {
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == '_') {
                j += 1;
            }
            if j < bytes.len() && !bytes[j].is_ascii() {
                return true;
            }
        }
        i = j.max(i + 1);
    }
    false
}

#[test]
fn 日本語の直前の変数展開は波括弧で括られている() {
    let mut violations: Vec<String> = Vec::new();
    for path in shell_scripts() {
        let src = std::fs::read_to_string(&path).expect("読める .sh");
        for (i, line) in src.lines().enumerate() {
            // 行コメントは展開されないので対象外（規約の説明文そのものが引っかかる）
            if line.trim_start().starts_with('#') {
                continue;
            }
            if unbraced_before_multibyte(line) {
                violations.push(format!("{}: {}", located(&path, i + 1), line.trim()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "変数の直後に全角文字が続いている（bash が変数名へ取り込み set -u で落ちる。\n\
         `${{var}}` で括ること。.agent/conventions.md の Issue #837 節を参照）:\n{}",
        violations.join("\n")
    );
}

/// 検査そのものの検出力。実際に踏んだ形（`$tag）`）を見逃さないこと
#[test]
fn 検査は実際に踏んだ形を検出する() {
    // #965 の実装中に踏んだ行そのもの（`$tag）` で `tag\xef\xbc\x89` を参照して即死した）
    assert!(unbraced_before_multibyte(
        r#"  echo "警告: 片肺リリース（$tag）— 配布物が無い OS: x" >&2"#
    ));
    assert!(unbraced_before_multibyte(
        r#"echo "        $registered（$note）""#
    ));
    // 波括弧で括ってあれば問題なし
    assert!(!unbraced_before_multibyte(
        r#"  echo "警告: 片肺リリース（${tag}）— 配布物が無い OS: x" >&2"#
    ));
    assert!(!unbraced_before_multibyte(
        r#"echo "        ${registered}（${note}）""#
    ));
    // ASCII が続くだけなら境界は曖昧にならない
    assert!(!unbraced_before_multibyte(r#"echo "tag=$tag ok""#));
    // コマンド置換・数値引数は名前を取り込まない
    assert!(!unbraced_before_multibyte(r#"echo "$(date)（now）""#));
    assert!(!unbraced_before_multibyte(r#"echo "${1}（引数）""#));
    // 位置パラメータは 1 桁で確定するので全角を取り込まない（誤検出しないこと）
    assert!(!unbraced_before_multibyte(
        r#"echo "不明な引数: $1（--publish）" >&2"#
    ));
}

// ---------------------------------------------------------------------------
// bash 3.2 の空配列展開（Issue #1499 → #1518）
// ---------------------------------------------------------------------------

/// 監査済みの例外宣言。違反行そのものか**直前の行**に
/// `# tako:bash32-ok <理由>` があれば見逃す。
///
/// **理由の無い宣言は無効**（黙って口をふさげる印にはしない）。
fn audited_exception(line: &str) -> bool {
    line.split("tako:bash32-ok")
        .nth(1)
        .is_some_and(|reason| !reason.trim().is_empty())
}

/// `set -u`（`set -euo pipefail` や `set -o nounset` を含む）を宣言しているか。
///
/// **宣言していないライブラリも対象にする**（`scripts/lib/*.sh` は `set -u` を
/// 宣言した側から source されるので同じ条件で動く）。この関数は診断メッセージで
/// 「自前で宣言しているか / 呼ばれて継ぐか」を言い分けるためだけに使う。
fn declares_set_u(src: &str) -> bool {
    src.lines().any(|line| {
        let t = line.trim_start();
        let Some(rest) = t.strip_prefix("set ") else {
            return false;
        };
        let rest = rest.trim_start();
        if let Some(opts) = rest.strip_prefix('-') {
            if !opts.starts_with('o') {
                let flags = opts.split_whitespace().next().unwrap_or("");
                return flags.contains('u');
            }
        }
        rest.strip_prefix("-o ")
            .is_some_and(|w| w.trim_start().starts_with("nounset"))
    })
}

/// bash 3.2 の `set -u` で落ちる**素の配列展開**を列挙する（見つけた綴りを返す）。
///
/// 実測（`/bin/bash` 3.2.57・空配列・`set -u`）で落ちるのは**演算子の無い**
/// `${a[@]}` / `${a[*]}` だけ:
///
/// | 書き方 | 空配列のとき |
/// |---|---|
/// | `"${a[@]}"` / `"${a[*]}"` | **`a[@]: unbound variable` で即死** |
/// | `${a[@]+"${a[@]}"}` | 何も渡さない（= 望みの挙動） |
/// | `${#a[@]}`（長さ）/ `${!a[@]}`（添字） | 通る |
/// | 演算子つき（`:-` `:+` `:1` `#pat` `/pat/rep`） | 通る |
///
/// だから検査も「演算子の無い展開」1 点に絞る（過大にも過小にも申告しない）。
/// 防御済みの `${a[@]+…}` は**中身ごと飛ばす**ので、慣用句の内側にある
/// `"${a[@]}"` を二重に数えない。
fn bare_array_expansions(line: &str) -> Vec<String> {
    // 名前・記号はすべて ASCII なので、バイト走査で多バイト文字を誤って拾うことはない
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        if !(b[i] == b'$' && b.get(i + 1) == Some(&b'{')) {
            i += 1;
            continue;
        }
        // 名前の先頭は英字か `_`。`${#a[@]}`（長さ）と `${!a[@]}`（添字）はここで外れる
        let mut j = i + 2;
        if !b
            .get(j)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        {
            i += 1;
            continue;
        }
        while b
            .get(j)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            j += 1;
        }
        // 添字が `[@]` / `[*]` のときだけが対象（`${a[0]}` は「空配列」とは別の話）
        let sigil = match (b.get(j), b.get(j + 1), b.get(j + 2)) {
            (Some(b'['), Some(s @ (b'@' | b'*')), Some(b']')) => *s as char,
            _ => {
                i += 1;
                continue;
            }
        };
        let name = &line[i + 2..j];
        let after = j + 3;
        if b.get(after) == Some(&b'}') {
            out.push(format!("${{{name}[{sigil}]}}"));
            i = after + 1;
        } else {
            // 演算子つき = 防御済み。`}` の対応を取って中身ごと飛ばす
            let mut depth = 1usize;
            let mut k = after;
            while k < b.len() && depth > 0 {
                match b[k] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                k += 1;
            }
            i = k;
        }
    }
    out
}

/// ヒアドキュメントの 1 本。`expanded` が false ならその本文は囲む側で展開されない
#[derive(Clone, Debug, PartialEq, Eq)]
struct Heredoc {
    delimiter: String,
    expanded: bool,
    strip_tabs: bool,
}

/// この行が開くヒアドキュメントを、bash が処理する順に返す。
///
/// **クォートされた語（`<<'X'` / `<<"X"` / `<<\X`）の本文は囲む側で 1 文字も
/// 展開されない**ので、そこに `"${a[@]}"` が在っても囲む側の `set -u` では落ちない
/// （いまリポジトリに在る 3 箇所は `scripts/promo/lib.sh` が書き出すデモ用スクリプトの
/// 本文で、書き出された側は `set -u` を宣言していない）。素の `<<X` は展開されるので
/// 本文も検査する。
fn heredocs_opened(line: &str) -> Vec<Heredoc> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 1 < b.len() {
        if !(b[i] == b'<' && b[i + 1] == b'<') {
            i += 1;
            continue;
        }
        // `<<<` はヒアストリング（ヒアドキュメントではない）
        if b.get(i + 2) == Some(&b'<') {
            i += 3;
            continue;
        }
        let mut j = i + 2;
        let strip_tabs = b.get(j) == Some(&b'-');
        if strip_tabs {
            j += 1;
        }
        while b.get(j) == Some(&b' ') || b.get(j) == Some(&b'\t') {
            j += 1;
        }
        let (quote, expanded) = match b.get(j) {
            Some(b'\'') => (Some(b'\''), false),
            Some(b'"') => (Some(b'"'), false),
            Some(b'\\') => (None, false),
            _ => (None, true),
        };
        if quote.is_some() || !expanded {
            j += 1; // 開きクォート、または `\`
        }
        let start = j;
        while b
            .get(j)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            j += 1;
        }
        if j == start {
            i += 2;
            continue;
        }
        out.push(Heredoc {
            delimiter: line[start..j].to_string(),
            expanded,
            strip_tabs,
        });
        i = if quote.is_some() { j + 1 } else { j };
    }
    out
}

#[test]
fn 空配列の展開はbash32の慣用句で守られている() {
    let mut violations: Vec<String> = Vec::new();
    for path in shell_scripts() {
        let src = std::fs::read_to_string(&path).expect("読める .sh");
        let own_set_u = declares_set_u(&src);
        let lines: Vec<&str> = src.lines().collect();
        // ヒアドキュメントの本文は「囲む側が展開するか」で扱いを変える
        let mut active: Option<Heredoc> = None;
        let mut queued: Vec<Heredoc> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if let Some(hd) = active.clone() {
                let probe = if hd.strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    line
                };
                if probe == hd.delimiter {
                    active = if queued.is_empty() {
                        None
                    } else {
                        Some(queued.remove(0))
                    };
                    continue;
                }
                if !hd.expanded {
                    continue; // 囲む側は展開しない = 落ちない
                }
            } else {
                // 行コメントは展開されないので対象外（規約の説明文そのものが引っかかる）
                if line.trim_start().starts_with('#') {
                    continue;
                }
            }
            let hits = bare_array_expansions(line);
            if !hits.is_empty() {
                let exempt = audited_exception(line)
                    || i.checked_sub(1)
                        .and_then(|p| lines.get(p))
                        .is_some_and(|prev| audited_exception(prev));
                if !exempt {
                    let how = if own_set_u {
                        "set -u を宣言"
                    } else {
                        "set -u を宣言した側から source される"
                    };
                    for spelling in hits {
                        violations.push(format!(
                            "{}: {spelling} （{how}） {}",
                            located(&path, i + 1),
                            line.trim()
                        ));
                    }
                }
            }
            if active.is_none() {
                let mut opened = heredocs_opened(line);
                if !opened.is_empty() {
                    active = Some(opened.remove(0));
                    queued.extend(opened);
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "空配列のまま展開されると macOS 同梱の bash 3.2（`/bin/bash` 3.2.57）が\n\
         `set -u` の下で `名前[@]: unbound variable` で即死する。\n\
         `${{arr[@]+\"${{arr[@]}}\"}}` の慣用句で書くこと\n\
         （`.agent/conventions.md`「シェルスクリプトは macOS 同梱の bash 3.2 で通す」節。\n\
         配列が絶対に空にならないと**監査して確かめた**箇所だけ、その行か直前の行に\n\
         `# tako:bash32-ok <理由>` を書いて外せる）:\n{}",
        violations.join("\n")
    );
}

#[test]
fn 検査は素の配列展開だけを違反とする() {
    let bare = |s: &str| !bare_array_expansions(s).is_empty();

    // #1518 で直した実物そのもの
    assert!(bare(
        r#"  cargo xwin check --workspace --target "$TARGET" "${ALL_TARGETS[@]}" "$@""#
    ));
    assert!(bare(
        r#"          echo "  未登録のまま: $(join_names "${MISSING[@]}")" >&2"#
    ));
    assert!(bare(r#"    env "${PROMO_ENV_CLEAN[@]}" \"#));
    // `[*]`（IFS で 1 語に繋ぐ形）も同じく落ちる
    assert!(bare(r#"    echo "!! 素材が無いシーン: ${missing[*]}" >&2"#));

    // 直したあとの慣用句は違反ではない（**内側の `"${a[@]}"` を二重に数えない**）
    assert_eq!(
        bare_array_expansions(
            r#"  cargo xwin check --target "$TARGET" ${ALL_TARGETS[@]+"${ALL_TARGETS[@]}"} "$@""#
        ),
        Vec::<String>::new()
    );
    assert!(!bare(
        r#"env ${PROMO_ENV_CLEAN[@]+"${PROMO_ENV_CLEAN[@]}"} \"#
    ));
    assert!(!bare(r#"echo ": ${missing[*]+${missing[*]}}" >&2"#));
    // release.sh が以前から使っている「外側もクォートする」変種も防御済み
    assert!(!bare(
        r#"  table=$(build_download_table "${names[@]+"${names[@]}"}")"#
    ));

    // 実測で落ちない形（過大申告しない）
    assert!(!bare(r#"if [ ${#missing[@]} -gt 0 ]; then"#)); // 長さ
    assert!(!bare(r#"for i in "${!parts[@]}"; do"#)); // 添字
    assert!(!bare(r#"  for d in "${HELD_LOCKS[@]:-}"; do"#)); // `:-`
    assert!(!bare(r#"echo "${a[@]:+ある}""#)); // `:+`
    assert!(!bare(r#"f "${a[@]:1}""#)); // 部分列
    assert!(!bare(r#"f "${a[@]/x/y}""#)); // 置換
    assert!(!bare(r#"echo "${a[0]}""#)); // 要素参照は別の話
    assert!(!bare(r#"echo "$@" "$*" "${var}""#)); // 配列ではない

    // 1 行に 2 つあれば 2 件返す（片方だけ直して緑にならないこと）
    assert_eq!(
        bare_array_expansions(r#"cp "${srcs[@]}" "${dsts[@]}""#),
        vec!["${srcs[@]}".to_string(), "${dsts[@]}".to_string()]
    );
}

#[test]
fn ヒアドキュメントはクォートの有無で扱いを分ける() {
    // クォートされた語 = 囲む側では展開されない（本文は別スクリプトのソース）
    assert_eq!(
        heredocs_opened(r#"    cat > "$DEMO/scripts/build.sh" <<'BLD'"#),
        vec![Heredoc {
            delimiter: "BLD".to_string(),
            expanded: false,
            strip_tabs: false,
        }]
    );
    assert!(!heredocs_opened(r#"cat <<"EOF""#)[0].expanded);
    assert!(!heredocs_opened(r#"cat <<\EOF"#)[0].expanded);
    // 素の語 = 囲む側が展開する = 本文も検査の対象
    assert_eq!(
        heredocs_opened(r#"cat > "$APP/Contents/Info.plist" <<PLIST"#),
        vec![Heredoc {
            delimiter: "PLIST".to_string(),
            expanded: true,
            strip_tabs: false,
        }]
    );
    assert!(heredocs_opened(r#"  cat <<-EOF"#)[0].strip_tabs);
    // ヒアストリングはヒアドキュメントではない
    assert!(heredocs_opened(r#"IFS='|' read -r a b <<< "$spec""#).is_empty());
    // 1 行で 2 本開くときは bash が処理する順
    assert_eq!(
        heredocs_opened("diff <<'A' <<B")
            .iter()
            .map(|h| (h.delimiter.as_str(), h.expanded))
            .collect::<Vec<_>>(),
        vec![("A", false), ("B", true)]
    );
    assert!(heredocs_opened("echo done").is_empty());
}

#[test]
fn 例外宣言は理由つきのときだけ効く() {
    assert!(audited_exception(
        r#"  f "${a[@]}"  # tako:bash32-ok 直前で空でないことを確かめている"#
    ));
    assert!(audited_exception("# tako:bash32-ok 固定の非空リテラル"));
    // 理由が無い宣言は無効（黙って口をふさげる印にはしない）
    assert!(!audited_exception("# tako:bash32-ok"));
    assert!(!audited_exception("# tako:bash32-ok   "));
    assert!(!audited_exception("# ふつうのコメント"));
}

#[test]
fn set_uの宣言を読み分ける() {
    assert!(declares_set_u("#!/bin/bash\nset -euo pipefail\n"));
    assert!(declares_set_u("set -uo pipefail\n"));
    assert!(declares_set_u("  set -u\n"));
    assert!(declares_set_u("set -o nounset\n"));
    // `set -e` だけ・`set -o pipefail` だけでは nounset は入らない
    assert!(!declares_set_u("set -e\n"));
    assert!(!declares_set_u("set -o pipefail\n"));
    assert!(!declares_set_u("echo 'set -u'\nsettings=1\n"));
}

// ---------------------------------------------------------------------------
// set -e と EXIT trap の併用（Issue #1864）
// ---------------------------------------------------------------------------

/// 番人の 1 実装。ここだけは素の `trap … EXIT` を張ってよい（中身そのものなので）
const EXIT_GUARD_LIB: &str = "scripts/lib/exit-guard.sh";

/// ファイルの中の 1 つの単純コマンド（クォートは外した語の並び）
#[derive(Clone, Debug, PartialEq, Eq)]
struct ShellCommand {
    /// 先頭の語がある行（1 始まり）
    line: usize,
    /// `if` / `then` / `{` などの予約語を先頭から外した語
    words: Vec<String>,
    /// `( … )` / `$( … )` の中（= サブシェル。親の EXIT trap は走らない）
    in_subshell: bool,
}

/// 単純コマンドの先頭に来ても「コマンド名」ではない予約語
const LEADING_KEYWORDS: &[&str] = &[
    "{", "}", "!", "if", "then", "else", "elif", "do", "while", "until", "time",
];

/// `.sh` 全体を単純コマンドへ分ける（`set -e` / `trap` / `exit` を見つけるための粗い字句解析）。
///
/// 行単位では読み違える形があるので**ファイル全体をクォートの状態ごと追う**:
/// 複数行にまたがる文字列（`NOTES="…` が次の行へ続く）の中身・ヒアドキュメントの本文
/// （テストが書き出す偽の CLI の `exit 0`）・コメントは命令として数えない。
/// `;` `&` `|` `(` `)` と改行で区切る。`( … )` と `$( … )` の中はサブシェルとして印を付ける。
fn shell_commands(src: &str) -> Vec<ShellCommand> {
    #[derive(PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }
    let mut out = Vec::new();
    let mut quote = Quote::None;
    let mut depth = 0usize;
    let mut word = String::new();
    let mut word_started = false;
    let mut words: Vec<String> = Vec::new();
    let mut start_line = 0usize;
    let mut cmd_in_subshell = false;
    let mut active: Option<Heredoc> = None;
    let mut queued: Vec<Heredoc> = Vec::new();

    fn end_word(word: &mut String, started: &mut bool, words: &mut Vec<String>) {
        if *started {
            words.push(std::mem::take(word));
            *started = false;
        }
    }
    let end_command =
        |words: &mut Vec<String>, out: &mut Vec<ShellCommand>, line: usize, in_subshell: bool| {
            let skip = words
                .iter()
                .take_while(|w| LEADING_KEYWORDS.contains(&w.as_str()))
                .count();
            let rest: Vec<String> = words.drain(..).skip(skip).collect();
            if !rest.is_empty() {
                out.push(ShellCommand {
                    line,
                    words: rest,
                    in_subshell,
                });
            }
        };

    for (idx, line) in src.lines().enumerate() {
        let line_no = idx + 1;
        if let Some(hd) = active.clone() {
            let probe = if hd.strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            if probe == hd.delimiter {
                active = if queued.is_empty() {
                    None
                } else {
                    Some(queued.remove(0))
                };
            }
            continue; // 本文は別のスクリプト（またはデータ）。この .sh の命令ではない
        }
        let starts_in_quote = quote != Quote::None;
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0usize;
        let mut continued = false;
        while i < chars.len() {
            let c = chars[i];
            match quote {
                Quote::Single => {
                    if c == '\'' {
                        quote = Quote::None;
                    } else {
                        word.push(c);
                    }
                }
                Quote::Double => {
                    if c == '\\' && i + 1 < chars.len() {
                        word.push(chars[i + 1]);
                        i += 1;
                    } else if c == '"' {
                        quote = Quote::None;
                    } else {
                        word.push(c);
                    }
                }
                Quote::None => {
                    if words.is_empty() && !word_started {
                        start_line = line_no;
                        cmd_in_subshell = depth > 0;
                    }
                    match c {
                        '#' if !word_started => break, // 行コメント
                        '\\' if i + 1 == chars.len() => continued = true,
                        '\\' => {
                            word.push(chars[i + 1]);
                            word_started = true;
                            i += 1;
                        }
                        '\'' => {
                            quote = Quote::Single;
                            word_started = true;
                        }
                        '"' => {
                            quote = Quote::Double;
                            word_started = true;
                        }
                        ' ' | '\t' => end_word(&mut word, &mut word_started, &mut words),
                        ';' | '&' | '|' | '(' | ')' => {
                            end_word(&mut word, &mut word_started, &mut words);
                            end_command(&mut words, &mut out, start_line, cmd_in_subshell);
                            match c {
                                '(' => depth += 1,
                                ')' => depth = depth.saturating_sub(1),
                                _ => {}
                            }
                        }
                        _ => {
                            word.push(c);
                            word_started = true;
                        }
                    }
                }
            }
            i += 1;
        }
        if quote != Quote::None {
            word.push('\n'); // 文字列が次の行へ続く
            continue;
        }
        if continued {
            continue; // 行末の `\` = 同じコマンドが次の行へ続く
        }
        end_word(&mut word, &mut word_started, &mut words);
        end_command(&mut words, &mut out, start_line, cmd_in_subshell);
        // 文字列の途中から始まった行の `<<` は文字列の中身なので数えない
        if !starts_in_quote {
            let mut opened = heredocs_opened(line);
            if !opened.is_empty() {
                active = Some(opened.remove(0));
                queued.extend(opened);
            }
        }
    }
    out
}

/// `set` の 1 コマンドがオプション（短い名前 `short` / 長い名前 `long`）を**入れる**か
fn set_command_enables(words: &[String], short: char, long: &str) -> bool {
    if words.first().map(String::as_str) != Some("set") {
        return false;
    }
    let mut args = words[1..].iter();
    while let Some(a) = args.next() {
        if a == "-o" {
            if args.next().is_some_and(|n| n == long) {
                return true;
            }
        } else if let Some(flags) = a.strip_prefix('-') {
            if !flags.starts_with('-') && flags.contains(short) {
                return true;
            }
        }
    }
    false
}

/// `set -e`（`set -euo pipefail` / `set -o errexit` / shebang の `-e` を含む）を宣言しているか
fn declares_errexit(src: &str, commands: &[ShellCommand]) -> bool {
    let shebang_e = src.lines().next().is_some_and(|first| {
        first.starts_with("#!")
            && first
                .split_whitespace()
                .skip(1)
                .any(|w| w.starts_with('-') && !w.starts_with("--") && w.contains('e'))
    });
    shebang_e
        || commands
            .iter()
            .any(|c| set_command_enables(&c.words, 'e', "errexit"))
}

/// この単純コマンドが EXIT の trap を**張る**か（`trap - EXIT` は外す方なので false）
fn installs_exit_trap(words: &[String]) -> bool {
    if words.first().map(String::as_str) != Some("trap") {
        return false;
    }
    let mut args: &[String] = &words[1..];
    if args.first().is_some_and(|a| a == "--") {
        args = &args[1..];
    }
    let Some((action, sigspecs)) = args.split_first() else {
        return false; // 素の `trap` = 一覧の表示
    };
    // `trap - EXIT`（外す）/ `trap -p` / `trap -l`（表示）
    if action.starts_with('-') {
        return false;
    }
    sigspecs.iter().any(|s| {
        let s = s.to_ascii_uppercase();
        s == "EXIT" || s == "SIGEXIT" || s == "0"
    })
}

/// 印を立てずに 0 で抜ける `exit`（`exit 0` / 引数なしの `exit`）か
fn exits_zero_unmarked(words: &[String]) -> bool {
    words.first().map(String::as_str) == Some("exit")
        && matches!(words.get(1).map(String::as_str), None | Some("0"))
}

/// 監査済みの例外宣言 `# tako:exit-guard-ok <理由>`（違反行か直前の行。理由の無い宣言は無効）
fn exit_guard_exception(lines: &[&str], line_no: usize) -> bool {
    let has = |l: &str| {
        l.split("tako:exit-guard-ok")
            .nth(1)
            .is_some_and(|reason| !reason.trim().is_empty())
    };
    let at = |n: usize| n.checked_sub(1).and_then(|i| lines.get(i)).copied();
    at(line_no).is_some_and(has) || at(line_no.wrapping_sub(1)).is_some_and(has)
}

/// 呼び手の `set -e` を継ぐライブラリ（自分では宣言しなくても、宣言した側から source される）
fn is_sourced_library(rel: &str) -> bool {
    rel.starts_with("scripts/lib/") || rel.ends_with("/lib.sh")
}

/// 1 ファイルぶんの違反（file:line で名指しする文言）
fn exit_guard_violations(rel: &str, src: &str) -> Vec<String> {
    if rel == EXIT_GUARD_LIB {
        return Vec::new();
    }
    let commands = shell_commands(src);
    let lines: Vec<&str> = src.lines().collect();
    let text = |c: &ShellCommand| {
        lines
            .get(c.line - 1)
            .map(|l| l.trim().to_string())
            .unwrap_or_default()
    };
    let mut out = Vec::new();

    // 規則 1: set -e の下で素の EXIT trap を張っている（印の共通実装を通っていない）
    let errexit = declares_errexit(src, &commands);
    if errexit || is_sourced_library(rel) {
        let how = if errexit {
            "set -e を宣言"
        } else {
            "set -e を宣言した側から source される"
        };
        for c in commands.iter().filter(|c| installs_exit_trap(&c.words)) {
            if !exit_guard_exception(&lines, c.line) {
                out.push(format!(
                    "{rel}:{}: 素の EXIT trap（{how}）→ tako_exit_trap で張る: {}",
                    c.line,
                    text(c)
                ));
            }
        }
    }

    let first_guard = commands
        .iter()
        .find(|c| c.words.first().is_some_and(|w| w == "tako_exit_trap"));
    if let Some(guard) = first_guard {
        // 規則 2: tako_exit_trap を呼ぶのに番人の 1 実装を source していない
        let sourced = commands.iter().any(|c| {
            matches!(c.words.first().map(String::as_str), Some("source" | "."))
                && c.words.get(1).is_some_and(|p| p.ends_with("exit-guard.sh"))
        });
        if !sourced {
            out.push(format!(
                "{rel}:{}: tako_exit_trap を呼ぶのに {EXIT_GUARD_LIB} を source していない: {}",
                guard.line,
                text(guard)
            ));
        }
        // 規則 3: trap を張った後の素の `exit 0` は印が立たないので 1 に化ける
        for c in commands
            .iter()
            .filter(|c| c.line > guard.line && !c.in_subshell && exits_zero_unmarked(&c.words))
        {
            if !exit_guard_exception(&lines, c.line) {
                out.push(format!(
                    "{rel}:{}: tako_exit_trap の後の素の exit（印が立たず 1 になる）→ tako_exit 0: {}",
                    c.line,
                    text(c)
                ));
            }
        }
    }
    out
}

#[test]
fn set_eとexit_trapの併用は番人の共通実装を通す() {
    let root = repo_root();
    let mut violations: Vec<String> = Vec::new();
    for path in shell_scripts() {
        let src = std::fs::read_to_string(&path).expect("読める .sh");
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        violations.extend(exit_guard_violations(&rel, &src));
    }
    assert!(
        violations.is_empty(),
        "macOS 同梱の bash 3.2（`/bin/bash` と `/bin/sh`）は、set -e の下で未定義の変数などを\n\
         踏んで死ぬと EXIT trap の中の $? が 0 になり、途中で死んだスクリプトが exit 0 で終わる\n\
         （夜間リリースが黙って成功扱いになる型）。{EXIT_GUARD_LIB} を source して\n\
         `tako_exit_trap <後始末>` で張り、成功で抜ける所は `tako_exit 0` にすること\n\
         （.agent/conventions.md「シェルスクリプトは macOS 同梱の bash 3.2 で通す」節。\n\
         監査して確かめた箇所だけ、その行か直前の行に `# tako:exit-guard-ok <理由>` で外せる）:\n{}",
        violations.join("\n")
    );
}

#[test]
fn 字句解析はクォートとヒアドキュメントとサブシェルを読み分ける() {
    let words_of = |src: &str| -> Vec<Vec<String>> {
        shell_commands(src).into_iter().map(|c| c.words).collect()
    };
    let w = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    // 区切りと予約語。クォートは外して 1 語にする（混ぜ書きも 1 語）
    assert_eq!(
        words_of("if true; then exit 0; fi"),
        vec![w(&["true"]), w(&["exit", "0"]), w(&["fi"])]
    );
    assert_eq!(
        words_of(r#"trap 'promo_stop_isolated '"$socket" EXIT"#),
        vec![w(&["trap", "promo_stop_isolated $socket", "EXIT"])]
    );
    assert_eq!(
        words_of(r#"acquire_lock "$D" "x" || exit 0 # 説明"#),
        vec![w(&["acquire_lock", "$D", "x"]), w(&["exit", "0"])]
    );
    // 複数行の文字列の中身は命令ではない（先頭の行で 1 語になる）
    let multi = shell_commands("NOTES=\"a\nexit 0\ntrap c EXIT\"\necho done\n");
    assert_eq!(multi.len(), 2);
    assert_eq!(multi[0].line, 1);
    assert_eq!(multi[1].words, w(&["echo", "done"]));
    assert_eq!(multi[1].line, 4);
    // ヒアドキュメントの本文（偽の CLI）は数えない。終わりの行の後は数える
    let hd = shell_commands("cat > f <<'FAKE'\nset -e\ntrap c EXIT\nexit 0\nFAKE\nexit 0\n");
    assert_eq!(
        hd.iter()
            .map(|c| (c.line, c.words.clone()))
            .collect::<Vec<_>>(),
        vec![(1, w(&["cat", ">", "f", "<<FAKE"])), (6, w(&["exit", "0"]))]
    );
    // サブシェル・コマンド置換の中の exit は親の trap を走らせない
    let sub = shell_commands("( exit 0 )\nx=$(exit 0)\nexit 0\n");
    assert_eq!(
        sub.iter()
            .filter(|c| exits_zero_unmarked(&c.words))
            .map(|c| (c.line, c.in_subshell))
            .collect::<Vec<_>>(),
        vec![(1, true), (2, true), (3, false)]
    );
    // case の腕の `)` で深さが崩れない
    let arm = shell_commands("case $1 in\n  --x) install; exit 0 ;;\nesac\nexit 0\n");
    assert!(arm.iter().all(|c| !c.in_subshell));
    // 行末の `\` は同じコマンドの続き
    assert_eq!(
        words_of("trap 'rm -rf x' \\\n  EXIT\n"),
        vec![w(&["trap", "rm -rf x", "EXIT"])]
    );
}

#[test]
fn exit_trapとset_eとexitを読み分ける() {
    let cmd = |s: &str| shell_commands(s).remove(0).words;
    // 張る形（どの書き方でも EXIT を名指したら対象）
    assert!(installs_exit_trap(&cmd("trap cleanup EXIT")));
    assert!(installs_exit_trap(&cmd(
        r#"trap 'rm -rf "$TMP"' EXIT HUP INT TERM"#
    )));
    assert!(installs_exit_trap(&cmd("trap 'x' 0")));
    assert!(installs_exit_trap(&cmd("trap -- c exit")));
    // 外す・表示する・EXIT 以外は対象外
    assert!(!installs_exit_trap(&cmd("trap - EXIT")));
    assert!(!installs_exit_trap(&cmd("trap -p EXIT")));
    assert!(!installs_exit_trap(&cmd("trap 'exit 130' INT TERM")));
    assert!(!installs_exit_trap(&cmd(r#"echo "trap c EXIT""#)));
    assert!(shell_commands("# trap c EXIT").is_empty());

    let errexit = |s: &str| declares_errexit(s, &shell_commands(s));
    assert!(errexit("set -euo pipefail\n"));
    assert!(errexit("set -eu\n"));
    assert!(errexit("  set -e\n"));
    assert!(errexit("set -o errexit\n"));
    assert!(errexit("#!/bin/bash -e\necho x\n"));
    // set -u だけ・外す側・文字列の中は宣言ではない（set -u だけなら死ねば 1 で化けない）
    assert!(!errexit("set -uo pipefail\n"));
    assert!(!errexit("set +e\n"));
    assert!(!errexit("set -o pipefail\n"));
    assert!(!errexit("echo 'set -e'\n"));
    assert!(!errexit("cat <<'X'\nset -e\nX\n"));

    assert!(exits_zero_unmarked(&cmd("exit 0")));
    assert!(exits_zero_unmarked(&cmd("exit")));
    assert!(!exits_zero_unmarked(&cmd("exit 1")));
    assert!(!exits_zero_unmarked(&cmd(r#"exit "$rc""#)));
    assert!(!exits_zero_unmarked(&cmd("tako_exit 0")));
}

#[test]
fn 番犬は併用の3つの崩れ方をfile_lineで名指しする() {
    let guard = "set -euo pipefail\n. \"$R/scripts/lib/exit-guard.sh\"\n";
    // 直したあとの形は通る（trap の後でも、サブシェルの exit 0 と tako_exit 0 は良い）
    let ok = format!(
        "{guard}tako_exit_trap cleanup\nx=$(exit 0)\n[ \"$F\" -eq 0 ] || exit 1\ntako_exit 0\n"
    );
    assert_eq!(
        exit_guard_violations("scripts/x.sh", &ok),
        Vec::<String>::new()
    );

    // 規則 1: 素の EXIT trap（#1864 の前の nightly-release.sh の形）
    let v = exit_guard_violations(
        "scripts/x.sh",
        "set -euo pipefail\n\ntrap 'cleanup_all' EXIT\n",
    );
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(
        v[0].starts_with("scripts/x.sh:3: 素の EXIT trap（set -e を宣言）"),
        "{v:?}"
    );
    // set -u だけなら化けない（死ねば 1）ので名指さない = 過大申告しない
    assert!(exit_guard_violations("scripts/y.sh", "set -uo pipefail\ntrap c EXIT\n").is_empty());
    // ライブラリは自分で宣言しなくても対象（呼び手の set -e を継ぐ）
    let lib = exit_guard_violations("scripts/lib/z.sh", "f() {\n  trap c EXIT\n}\n");
    assert_eq!(lib.len(), 1, "{lib:?}");
    assert!(lib[0].starts_with(
        "scripts/lib/z.sh:2: 素の EXIT trap（set -e を宣言した側から source される）"
    ));
    // 番人の 1 実装そのものは対象外
    assert!(exit_guard_violations(EXIT_GUARD_LIB, "trap '_x' EXIT\n").is_empty());

    // 規則 2: source を忘れた
    let v = exit_guard_violations("scripts/x.sh", "set -eu\ntako_exit_trap c\ntako_exit 0\n");
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(
        v[0].starts_with("scripts/x.sh:2: tako_exit_trap を呼ぶのに"),
        "{v:?}"
    );

    // 規則 3: trap の後の素の exit 0 / 引数なしの exit（印が立たず 1 に化ける）
    let v = exit_guard_violations(
        "scripts/x.sh",
        &format!("{guard}exit 0\ntako_exit_trap c\nlock || exit 0\nexit\ntako_exit 0\n"),
    );
    assert_eq!(v.len(), 2, "{v:?}");
    assert!(
        v[0].starts_with("scripts/x.sh:5: tako_exit_trap の後の素の exit"),
        "{v:?}"
    );
    assert!(v[1].starts_with("scripts/x.sh:6: "), "{v:?}");

    // 例外宣言は理由つきのときだけ効く
    let src =
        "set -e\n# tako:exit-guard-ok trap の中で exit 1 固定にしてある\ntrap 'exit 1' EXIT\n";
    assert!(exit_guard_violations("scripts/x.sh", src).is_empty());
    let src = "set -e\n# tako:exit-guard-ok\ntrap 'exit 1' EXIT\n";
    assert_eq!(exit_guard_violations("scripts/x.sh", src).len(), 1);
}

// ---------------------------------------------------------------------------
// bash 3.2 の波括弧展開がクォートを取り違える（Issue #1924）
// ---------------------------------------------------------------------------

/// 1 つの語（クォート込みの原文）
#[derive(Clone, Debug, PartialEq, Eq)]
struct RawWord {
    /// ファイル先頭からの文字位置（`chars()` の添字）
    start: usize,
    text: String,
    /// bash が波括弧展開をかける語か（素の代入・`[[ ]]`・case の対象と腕のパターン・
    /// ヒアストリングは false。`/bin/bash` 3.2.57 で 1 つずつ確かめた）
    brace_expanded: bool,
}

/// case 文の中のどこに居るか（腕の `)` をサブシェルの閉じと取り違えないため）
#[derive(Clone, Copy, PartialEq, Eq)]
enum CaseState {
    Subject,
    ExpectIn,
    Pattern,
    Body,
}

/// `.sh` を**語**へ分ける字句解析（クォートは外さず原文のまま）。
///
/// `$( … )` の中のコマンドも 1 つずつ語へ分ける（入れ子の深さを問わない）。
/// クォートされた語のヒアドキュメント（`<<'X'`）の本文は読まない。素の `<<X` の本文は
/// 語ではない（展開されるが波括弧展開はかからない）が、中の `$( … )` は読む。
struct WordLexer {
    c: Vec<char>,
    i: usize,
    words: Vec<RawWord>,
    pending: Vec<Heredoc>,
}

const WORD_END: &[char] = &[' ', '\t', '\n', ';', '&', '|', '(', ')', '<', '>'];

impl WordLexer {
    fn new(text: &str) -> Self {
        WordLexer {
            c: text.chars().collect(),
            i: 0,
            words: Vec::new(),
            pending: Vec::new(),
        }
    }

    fn peek(&self, k: usize) -> Option<char> {
        self.c.get(self.i + k).copied()
    }

    fn at(&self, s: &str) -> bool {
        s.chars()
            .enumerate()
            .all(|(k, ch)| self.peek(k) == Some(ch))
    }

    /// コマンドの並び。`until_close` なら対応する `)` を食べて戻る（`$( … )` / `( … )`）
    fn list(&mut self, until_close: bool) {
        let mut cmd_start = true;
        let mut dbl_bracket = false;
        let mut dbl_paren = 0usize; // `[[ … ]]` の中の `( … )`（サブシェルではない）
        let mut next_exempt = false;
        let mut case: Vec<CaseState> = Vec::new();
        while let Some(ch) = self.peek(0) {
            match ch {
                '\n' => {
                    self.i += 1;
                    cmd_start = true;
                    self.heredoc_bodies();
                }
                ' ' | '\t' => self.i += 1,
                '\\' if self.peek(1) == Some('\n') => self.i += 2, // 行末の `\` = 続き
                '#' => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.i += 1;
                    }
                }
                ';' => {
                    // `;;` / `;&` / `;;&` は case の腕の終わり
                    if self.at(";;") || self.at(";&") {
                        self.i += 2;
                        if self.peek(0) == Some('&') {
                            self.i += 1;
                        }
                        if let Some(s) = case.last_mut() {
                            *s = CaseState::Pattern;
                        }
                    } else {
                        self.i += 1;
                    }
                    cmd_start = true;
                }
                '&' | '|' => {
                    self.i += 1;
                    cmd_start = true;
                }
                ')' => {
                    self.i += 1;
                    if dbl_bracket && dbl_paren > 0 {
                        dbl_paren -= 1;
                    } else if case.last() == Some(&CaseState::Pattern) {
                        *case.last_mut().expect("case") = CaseState::Body;
                        cmd_start = true;
                    } else if until_close {
                        return;
                    }
                }
                '(' => {
                    if self.at("((") {
                        self.arith();
                    } else if dbl_bracket {
                        self.i += 1;
                        dbl_paren += 1;
                    } else if case.last() == Some(&CaseState::Pattern) {
                        self.i += 1; // 腕のパターンの先頭の `(`
                    } else {
                        self.i += 1;
                        self.list(true);
                    }
                    cmd_start = false;
                }
                '<' | '>' => {
                    if self.at("<<<") {
                        self.i += 3;
                        next_exempt = true; // ヒアストリングは波括弧展開されない
                    } else if self.at("<<") {
                        self.heredoc_op();
                    } else if self.at("<(") || self.at(">(") {
                        self.i += 2;
                        self.list(true);
                    } else {
                        self.i += 1;
                        while matches!(self.peek(0), Some('>' | '&' | '|')) {
                            self.i += 1;
                        }
                    }
                }
                _ => {
                    let start = self.i;
                    let array = self.word(cmd_start);
                    let text: String = self.c[start..self.i].iter().collect();
                    let mut expanded = true;
                    if next_exempt {
                        next_exempt = false;
                        expanded = false;
                        if case.last() == Some(&CaseState::Subject) {
                            *case.last_mut().expect("case") = CaseState::ExpectIn;
                        }
                    } else if case.last() == Some(&CaseState::ExpectIn) && text == "in" {
                        *case.last_mut().expect("case") = CaseState::Pattern;
                        expanded = false;
                    } else if case.last() == Some(&CaseState::Pattern) {
                        expanded = false; // 腕のパターン（`esac` で閉じる）
                        if text == "esac" {
                            case.pop();
                        }
                    } else if dbl_bracket {
                        expanded = false;
                        if text == "]]" {
                            dbl_bracket = false;
                        }
                    } else if cmd_start {
                        match text.as_str() {
                            "[[" => {
                                dbl_bracket = true;
                                expanded = false;
                                cmd_start = false;
                            }
                            "case" => {
                                case.push(CaseState::Subject);
                                next_exempt = true;
                                cmd_start = false;
                            }
                            "esac" if case.last() == Some(&CaseState::Body) => {
                                case.pop();
                            }
                            "if" | "then" | "elif" | "else" | "do" | "while" | "until" | "!"
                            | "{" | "}" | "time" | "fi" | "done" | "esac" => {}
                            _ if is_assignment_word(&text) => expanded = array,
                            _ => cmd_start = false,
                        }
                    } else {
                        cmd_start = false;
                    }
                    if !array {
                        self.words.push(RawWord {
                            start,
                            text,
                            brace_expanded: expanded,
                        });
                    }
                }
            }
        }
    }

    /// 1 語を読む。`NAME=( … )` の配列代入なら要素を 1 語ずつ積んで true を返す
    /// （要素は波括弧展開される = 代入の右辺でも素通りしない）
    fn word(&mut self, cmd_start: bool) -> bool {
        let start = self.i;
        let mut array = false;
        while let Some(ch) = self.peek(0) {
            match ch {
                '\\' => self.i += 2,
                '\'' => self.single(),
                '$' if self.peek(1) == Some('\'') => {
                    self.i += 1;
                    self.ansi_c();
                }
                '"' => self.dquote(),
                '`' => self.backtick(),
                '$' if self.peek(1) == Some('(') => self.cmdsub(),
                '$' if self.peek(1) == Some('{') => self.param(false),
                '(' if cmd_start && {
                    let so_far: String = self.c[start..self.i].iter().collect();
                    so_far.ends_with('=') && is_assignment_word(&so_far)
                } =>
                {
                    self.i += 1;
                    self.array_elements();
                    array = true;
                }
                c if WORD_END.contains(&c) => break,
                _ => self.i += 1,
            }
        }
        // 末尾の `\` や閉じていないクォートで読み過ぎても、語の切り出しで落ちない
        self.i = self.i.min(self.c.len());
        array
    }

    /// `NAME=(` の後ろ、対応する `)` まで
    fn array_elements(&mut self) {
        while let Some(ch) = self.peek(0) {
            match ch {
                ')' => {
                    self.i += 1;
                    return;
                }
                ' ' | '\t' | '\n' => self.i += 1,
                '#' => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.i += 1;
                    }
                }
                _ => {
                    let start = self.i;
                    self.word(false);
                    if self.i == start {
                        self.i += 1; // 区切り文字の取りこぼし（無限ループにしない）
                        continue;
                    }
                    let text: String = self.c[start..self.i].iter().collect();
                    self.words.push(RawWord {
                        start,
                        text,
                        brace_expanded: true,
                    });
                }
            }
        }
    }

    fn single(&mut self) {
        self.i += 1;
        while self.peek(0).is_some_and(|c| c != '\'') {
            self.i += 1;
        }
        self.i += 1;
    }

    /// `$'…'`（`\'` で閉じない）。`$` は呼び手が食べてある
    fn ansi_c(&mut self) {
        self.i += 1;
        while let Some(ch) = self.peek(0) {
            self.i += 1;
            match ch {
                '\\' => self.i += 1,
                '\'' => return,
                _ => {}
            }
        }
    }

    fn dquote(&mut self) {
        self.i += 1;
        while let Some(ch) = self.peek(0) {
            match ch {
                '\\' => self.i += 2,
                '"' => {
                    self.i += 1;
                    return;
                }
                '`' => self.backtick(),
                '$' if self.peek(1) == Some('(') => self.cmdsub(),
                '$' if self.peek(1) == Some('{') => self.param(true),
                _ => self.i += 1,
            }
        }
    }

    fn backtick(&mut self) {
        self.i += 1;
        while let Some(ch) = self.peek(0) {
            self.i += 1;
            match ch {
                '\\' => self.i += 1,
                '`' => return,
                _ => {}
            }
        }
    }

    /// `$( … )`（中のコマンドも語へ分ける）か `$(( … ))`
    fn cmdsub(&mut self) {
        if self.at("$((") {
            self.i += 1;
            self.arith();
        } else {
            self.i += 2;
            self.list(true);
        }
    }

    /// `(( … ))`（入れ子の括弧を数えるだけ）
    fn arith(&mut self) {
        let mut depth = 0usize;
        while let Some(ch) = self.peek(0) {
            self.i += 1;
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    /// `${ … }`（中のクォート・コマンド置換も読む）
    fn param(&mut self, in_dquote: bool) {
        self.i += 2;
        let mut depth = 1usize;
        while let Some(ch) = self.peek(0) {
            match ch {
                '\\' => self.i += 2,
                '\'' if !in_dquote => self.single(),
                '"' => self.dquote(),
                '`' => self.backtick(),
                '$' if self.peek(1) == Some('(') => self.cmdsub(),
                '$' if self.peek(1) == Some('{') => {
                    self.i += 2;
                    depth += 1;
                }
                '}' => {
                    self.i += 1;
                    depth -= 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => self.i += 1,
            }
        }
    }

    /// `<<` / `<<-` の区切りの語を読み、次の改行で本文を読む予約をする
    fn heredoc_op(&mut self) {
        self.i += 2;
        let strip_tabs = self.peek(0) == Some('-');
        if strip_tabs {
            self.i += 1;
        }
        while matches!(self.peek(0), Some(' ' | '\t')) {
            self.i += 1;
        }
        let start = self.i;
        while self.peek(0).is_some_and(|c| !WORD_END.contains(&c)) {
            self.i += 1;
        }
        let raw: String = self.c[start..self.i].iter().collect();
        let expanded = !raw.contains(['\'', '"', '\\']);
        let delimiter: String = raw.chars().filter(|c| !"'\"\\".contains(*c)).collect();
        if !delimiter.is_empty() {
            self.pending.push(Heredoc {
                delimiter,
                expanded,
                strip_tabs,
            });
        }
    }

    /// 改行の直後に、予約されたヒアドキュメントの本文を順に読む
    fn heredoc_bodies(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        for hd in pending {
            while self.i < self.c.len() {
                let end = self.c[self.i..]
                    .iter()
                    .position(|&c| c == '\n')
                    .map_or(self.c.len(), |p| self.i + p);
                let line: String = self.c[self.i..end].iter().collect();
                let probe = if hd.strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    line.as_str()
                };
                if probe == hd.delimiter {
                    self.i = (end + 1).min(self.c.len());
                    break;
                }
                if !hd.expanded {
                    self.i = (end + 1).min(self.c.len());
                    continue;
                }
                // 本文は語ではない（`"` もただの文字）が、中の `$( … )` は語へ分ける
                while let Some(ch) = self.peek(0) {
                    match ch {
                        '\n' => break,
                        '\\' => self.i += 2,
                        '`' => self.backtick(),
                        '$' if self.peek(1) == Some('(') => self.cmdsub(),
                        '$' if self.peek(1) == Some('{') => self.param(true),
                        _ => self.i += 1,
                    }
                }
                self.i += 1;
            }
        }
    }
}

/// `NAME=…` / `NAME+=…` / `NAME[…]=…`（コマンドの先頭に並ぶ代入の語）か
fn is_assignment_word(text: &str) -> bool {
    let name_end = text
        .char_indices()
        .find(|&(k, c)| !(c == '_' || c.is_ascii_alphabetic() || (k > 0 && c.is_ascii_digit())))
        .map_or(text.len(), |(k, _)| k);
    if name_end == 0 {
        return false;
    }
    let mut rest = &text[name_end..];
    if rest.starts_with('[') {
        match rest.find(']') {
            Some(close) => rest = &rest[close + 1..],
            None => return false,
        }
    }
    rest.starts_with('=') || rest.starts_with("+=")
}

/// ファイル全体を語へ分ける
fn raw_words(src: &str) -> Vec<RawWord> {
    let mut lx = WordLexer::new(src);
    lx.list(false);
    lx.words
}

/// 正しい構文で読んだとき、語の中のその位置が**どのクォートにもコマンド置換にも入っていない**か
fn top_level_unquoted(word: &[char]) -> Vec<bool> {
    let mut top = vec![false; word.len()];
    let mut lx = WordLexer {
        c: word.to_vec(),
        i: 0,
        words: Vec::new(),
        pending: Vec::new(),
    };
    while let Some(ch) = lx.peek(0) {
        let before = lx.i;
        match ch {
            '\\' => lx.i += 2,
            '\'' => lx.single(),
            '$' if lx.peek(1) == Some('\'') => {
                lx.i += 1;
                lx.ansi_c();
            }
            '"' => lx.dquote(),
            '`' => lx.backtick(),
            '$' if lx.peek(1) == Some('(') => lx.cmdsub(),
            '$' if lx.peek(1) == Some('{') => lx.param(false),
            _ => {
                top[lx.i] = true;
                lx.i += 1;
            }
        }
        if lx.i == before {
            lx.i += 1;
        }
    }
    top
}

/// bash 3.2 の `brace_gobbler`（braces.c）の写し: `from` から `satisfy` を探す。
///
/// **3.2 は `"…"` の中を「次の `"` まで」としか見ず、`$( … )` の入れ子を知らない**。
/// だから `"$(f "{a,b}")"` は内側の `"` で外側のクォートが閉じたと読み、`{a,b}` を
/// クォートの外と取り違える（4.0 以降は `"` の中の `$(` を読み飛ばすので起きない）。
fn gobble32(t: &[char], from: usize, satisfy: char) -> Option<usize> {
    let ws = |c: Option<&char>| matches!(c, Some(' ' | '\t' | '\n'));
    let mut level = 0usize;
    let mut quoted: Option<char> = None;
    let mut i = from;
    while i < t.len() {
        let c = t[i];
        if c == '\\' && matches!(quoted, None | Some('"') | Some('`')) {
            i += 2;
            continue;
        }
        if c == '$' && t.get(i + 1) == Some(&'{') && quoted != Some('\'') {
            // 3.2 は `${` の次の 1 文字も読み飛ばす
            i += 3;
            if quoted.is_none() {
                level += 1;
            }
            continue;
        }
        if let Some(q) = quoted {
            if c == q {
                quoted = None;
            }
            i += 1;
            continue;
        }
        if matches!(c, '"' | '\'' | '`') {
            quoted = Some(c);
            i += 1;
            continue;
        }
        if c == '$' && t.get(i + 1) == Some(&'(') {
            // クォートの外の `$( … )` は 3.2 も正しく読み飛ばす
            let mut lx = WordLexer {
                c: t.to_vec(),
                i,
                words: Vec::new(),
                pending: Vec::new(),
            };
            lx.cmdsub();
            i = lx.i;
            continue;
        }
        if c == satisfy && level == 0 {
            // 空白で囲まれた `{`（と `{}`）は波括弧展開の始まりと見ない
            let lone = c == '{'
                && (i == from || ws(t.get(i - 1)))
                && (ws(t.get(i + 1)) || t.get(i + 1) == Some(&'}'));
            if !lone {
                return Some(i);
            }
        } else if c == '{' {
            level += 1;
        } else if c == '}' && level > 0 {
            level -= 1;
        }
        i += 1;
    }
    None
}

/// 3.2 がこの中身（`{` と `}` の間）を展開するか: エスケープされていない `,` があるか、
/// 連番（`1..3` / `a..c`。3.2 は刻み `1..5..2` を知らない）
fn amble_expands32(amble: &[char]) -> bool {
    let mut i = 0;
    while i < amble.len() {
        match amble[i] {
            '\\' => i += 2,
            ',' => return true,
            _ => i += 1,
        }
    }
    let s: String = amble.iter().collect();
    let Some((a, b)) = s.split_once("..") else {
        return false;
    };
    let int = |x: &str| {
        let d = x.strip_prefix('-').unwrap_or(x);
        !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())
    };
    let letter = |x: &str| x.chars().count() == 1 && x.chars().all(|c| c.is_ascii_alphabetic());
    (int(a) && int(b)) || (letter(a) && letter(b))
}

/// 語の中で、**bash 3.2 だけが**波括弧展開してしまう `{` の位置（語の中の文字位置）。
///
/// 3.2 が展開する `{` のうち、正しく読めばクォートかコマンド置換の中に在るものだけを返す
/// （クォートの外の `{a,b}` は意図した展開なので返さない = 過大申告しない）。
fn brace32_misreads(word: &str) -> Vec<usize> {
    let t: Vec<char> = word.chars().collect();
    let top = top_level_unquoted(&t);
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(open) = gobble32(&t, i, '{') {
        let Some(close) = gobble32(&t, open + 1, '}') else {
            i = open + 1;
            continue;
        };
        if amble_expands32(&t[open + 1..close]) {
            if !top[open] {
                out.push(open);
            }
            i = close + 1;
        } else {
            i = open + 1;
        }
    }
    out
}

/// 1 ファイルぶんの違反（file:line で名指しする文言）
fn brace32_violations(rel: &str, src: &str) -> Vec<String> {
    let chars: Vec<char> = src.chars().collect();
    let lines: Vec<&str> = src.lines().collect();
    let line_of = |pos: usize| chars[..pos].iter().filter(|&&c| c == '\n').count() + 1;
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for w in raw_words(src).into_iter().filter(|w| w.brace_expanded) {
        for p in brace32_misreads(&w.text) {
            let pos = w.start + p;
            if !seen.insert(pos) {
                continue;
            }
            let line_no = line_of(pos);
            let exempt = lines.get(line_no - 1).is_some_and(|l| audited_exception(l))
                || line_no
                    .checked_sub(2)
                    .and_then(|k| lines.get(k))
                    .is_some_and(|l| audited_exception(l));
            if !exempt {
                out.push(format!(
                    "{rel}:{line_no}: \"$( … \"{{…,…}}\" … )\" の波括弧（`{{…,…}}` / `{{a..c}}`）を 3.2 が展開する: {}",
                    lines.get(line_no - 1).map_or("", |l| l.trim())
                ));
            }
        }
    }
    out
}

#[test]
fn コマンド置換の中のダブルクォートの波括弧はbash32で割れない() {
    let root = repo_root();
    let mut violations: Vec<String> = Vec::new();
    for path in shell_scripts() {
        let src = std::fs::read_to_string(&path).expect("読める .sh");
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        violations.extend(brace32_violations(&rel, &src));
    }
    assert!(
        violations.is_empty(),
        "macOS 同梱の bash 3.2（`/bin/bash` 3.2.57）は `\"$( … )\"` の中のダブルクォートの文字列を\n\
         クォートの外と取り違え、`{{…,…}}` / `{{a..c}}` を波括弧展開して語を割る\n\
         （`f \"$(g \"{{\\\"op\\\":\\\"x\\\",\\\"y\\\":1}}\")\"` の g は `\"op\":\"x\"` だけを受け取る）。\n\
         JSON は `jq -n --arg …` で組むこと（単一引用符でも、値の中の `{{…,…}}` は割れる）\n\
         （.agent/conventions.md「シェルスクリプトは macOS 同梱の bash 3.2 で通す」節。\n\
         監査して確かめた箇所だけ、その行か直前の行に `# tako:bash32-ok <理由>` で外せる）:\n{}",
        violations.join("\n")
    );
}

#[test]
fn 検査はbash32が割る形だけを違反とする() {
    let hits = |s: &str| brace32_violations("scripts/x.sh", &format!("{s}\n")).len();

    // #1924 で直した実物そのもの（3.2 では mcp_call が `"op":"copy_cancel"` だけを受け取った）
    assert_eq!(
        hits(
            r#"CANCEL="$(mcp_text "$(mcp_call "{\"op\":\"copy_cancel\",\"name\":\"${ID:-0}\"}")")""#
        ),
        1
    );
    assert_eq!(
        hits(r#"OUT="$(mcp_text "$(mcp_call "{\"op\":\"copy\",\"paths\":[\"$E/src/a.txt\"]}")")""#),
        1
    );
    // 直したあとの形（jq で組む）は割れない
    assert_eq!(
        hits(
            r#"CANCEL="$(mcp_text "$(mcp_call "$(jq -n -c --arg id "${ID:-0}" '{op:"copy_cancel",name:$id}')")")""#
        ),
        0
    );

    // 3.2 で割れる形（`/bin/bash` 3.2.57 で 1 つずつ確かめた。bash 5 ではどれも割れない）
    assert_eq!(hits(r#"echo "$(show "{a,b}")""#), 1);
    assert_eq!(hits(r#"x=$(wrap "$(show "{a,b}")")"#), 1); // 外側が代入でも内側の語は割れる
    assert_eq!(hits(r#"local x="$(show "{a,b}")""#), 1); // local / export の引数は代入ではない
    assert_eq!(hits(r#"export X="$(show "{a,b}")""#), 1);
    assert_eq!(hits(r#"arr=("$(show "{a,b}")")"#), 1); // 配列の要素は展開される
    assert_eq!(hits(r#"for i in "$(show "{a,b}")"; do :; done"#), 1);
    assert_eq!(hits(r#"[ "$(show "{a,b}")" = x ]"#), 1);
    assert_eq!(hits(r#"show > "$(echo "/dev/null{a,b}")""#), 1); // リダイレクト先も
    assert_eq!(hits(r#"echo "${U:-$(show "{a,b}")}""#), 1);
    assert_eq!(hits(r#"echo "$(show $(echo "{a,b}"))""#), 1); // 3.2 は `"` を数えるだけ
    assert_eq!(hits(r#"echo "$(show "x" "{a,b}")""#), 1);
    assert_eq!(hits(r#"echo "$(show "{1..3}")""#), 1); // 連番も
    assert_eq!(hits(r#"echo "$(show "{a..c}")""#), 1);
    assert_eq!(hits(r#"echo "$(show "{ a,b }")""#), 1);
    assert_eq!(hits("cat <<X\n$(printf '%s' \"$(show \"{a,b}\")\")\nX"), 1);

    // 3.2 でも割れない形（過大申告しない）
    assert_eq!(hits(r#"x="$(show "{a,b}")""#), 0); // 素の代入は波括弧展開されない
    assert_eq!(hits(r#"x+="$(show "{a,b}")""#), 0);
    assert_eq!(hits(r#"V="$(show "{a,b}")" cmd"#), 0); // 前置の代入も
    assert_eq!(hits(r#"[[ "$(show "{a,b}")" == x ]]"#), 0);
    // `[[ ]]` の中の `( … )` はサブシェルではない（中の語も展開されず、`)` で閉じを取り違えない）
    assert_eq!(hits(r#"[[ ( "$(show "{a,b}")" == x ) ]]"#), 0);
    assert_eq!(
        hits(r#"x=$( [[ ( a == a ) ]] && echo "$(show "{a,b}")" )"#),
        1
    );
    assert_eq!(hits(r#"case "$(show "{a,b}")" in x) : ;; esac"#), 0);
    assert_eq!(hits(r#"cat <<< "$(show "{a,b}")""#), 0); // ヒアストリング
    assert_eq!(
        hits("cat <<X\n$(show \"{a,b}\")\n\"$(show \"{a,b}\")\"\nX"),
        0
    ); // 本文は語ではない
    assert_eq!(hits("cat <<'X'\necho \"$(show \"{a,b}\")\"\nX"), 0); // 別スクリプトの本文
    assert_eq!(hits(r#"echo $(show "{a,b}")"#), 0); // 外側がクォートされていない
    assert_eq!(hits(r#"echo "$(show '{a,b}')""#), 0); // 単一引用符
    assert_eq!(hits(r#"show "$(show '{"op":"x","y":1}')""#), 0); // 単一引用符の JSON
                                                                 // …ただし値の中の `{…,…}` は 3.2 が `"` を数えた結果クォートの外になって割れる
    assert_eq!(hits(r#"show "$(show '{"q":"{a,b}"}')""#), 1);
    assert_eq!(hits(r#"echo "$(show "{a}")""#), 0); // `,` も連番も無い
    assert_eq!(hits(r#"echo "$(show "{\"a\":1}")""#), 0);
    assert_eq!(hits(r#"echo "$(show "[{\"a\":1},{\"b\":2}]")""#), 0); // `,` が波括弧の外
    assert_eq!(hits(r#"echo "$(show "{1..5..2}")""#), 0); // 刻みつきの連番は 3.2 が知らない
    assert_eq!(hits(r#"echo "$(show "\{a,b}")""#), 0);
    assert_eq!(hits(r#"echo "$(show "{a\,b}")""#), 0);
    assert_eq!(hits(r#"echo "$(show "${U:-{a,b}}")""#), 0);
    assert_eq!(hits(r#"echo "$(show "it's {a,b}")""#), 0);
    assert_eq!(hits(r#"mkdir -p "$D"/{a,b}"#), 0); // クォートの外の展開は意図したもの
    assert_eq!(hits(r#"show {a,b}"$(show x)""#), 0);
    assert_eq!(hits(r#"# echo "$(show "{a,b}")""#), 0); // コメント
}

#[test]
fn 番犬は波括弧の取り違えをfile_lineで名指しする() {
    // 複数行のコマンドでも `{` の在る行を名指しする
    let src = "set -u\nOUT=\"$(mcp_text \\\n  \"$(mcp_call \"{\\\"op\\\":\\\"x\\\",\\\"y\\\":1}\")\")\"\n";
    let v = brace32_violations("scripts/x.sh", src);
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(v[0].starts_with("scripts/x.sh:3: "), "{v:?}");
    // 閉じていないクォート・末尾の `\` でも panic しない（構文エラーの .sh で番犬が落ちない）
    for broken in ["echo \"$(show \"{a,b}", "echo x\\", "a=(x 'y", "x=${a"] {
        let _ = brace32_violations("scripts/x.sh", broken);
    }
    // 1 行に 2 つあれば 2 件
    let v = brace32_violations(
        "scripts/x.sh",
        "echo \"$(show \"{a,b}\")\" \"$(show \"{c,d}\")\"\n",
    );
    assert_eq!(v.len(), 2, "{v:?}");
    // case の腕の `)` をコマンド置換の閉じと取り違えない（後ろの語も読める）
    let v = brace32_violations(
        "scripts/x.sh",
        "x=$(case \"$1\" in a) echo 1 ;; esac)\necho \"$(show \"{a,b}\")\"\n",
    );
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(v[0].starts_with("scripts/x.sh:2: "), "{v:?}");
    // 例外宣言は理由つきのときだけ効く
    let bad = "echo \"$(show \"{a,b}\")\"\n";
    assert!(brace32_violations(
        "scripts/x.sh",
        &format!("# tako:bash32-ok show は 1 語目しか見ない\n{bad}")
    )
    .is_empty());
    assert_eq!(
        brace32_violations("scripts/x.sh", &format!("# tako:bash32-ok\n{bad}")).len(),
        1
    );
}

#[test]
fn 語の字句解析は代入と展開されない文脈を読み分ける() {
    let words = |s: &str| -> Vec<(String, bool)> {
        raw_words(s)
            .into_iter()
            .map(|w| (w.text, w.brace_expanded))
            .collect()
    };
    let w = |t: &str, e: bool| (t.to_string(), e);
    // 代入はコマンドの先頭に並ぶ間だけ。コマンド名の後ろは引数
    assert_eq!(
        words(r#"A=1 B+="x y" cmd C=2"#),
        vec![
            w("A=1", false),
            w(r#"B+="x y""#, false),
            w("cmd", true),
            w("C=2", true)
        ]
    );
    // コマンド置換の中のコマンドも語へ分ける（外側の語は原文のまま 1 語）
    assert_eq!(
        words(r#"x="$(f "a b")""#),
        vec![
            w("f", true),
            w(r#""a b""#, true),
            w(r#"x="$(f "a b")""#, false)
        ]
    );
    // 配列代入は要素が語
    assert_eq!(
        words("a=(x \"y z\")"),
        vec![w("x", true), w("\"y z\"", true)]
    );
    // [[ ]] と case の対象・腕のパターンとヒアストリングは展開されない
    assert!(words(r#"[[ "$a" == b ]]"#).iter().all(|(_, e)| !e));
    assert_eq!(
        words("case \"$x\" in a|b) run ;; esac"),
        vec![
            w("case", true),
            w("\"$x\"", false),
            w("in", false),
            w("a", false),
            w("b", false),
            w("run", true),
            w("esac", false)
        ]
    );
    assert_eq!(
        words("cat <<< \"$x\" y"),
        vec![w("cat", true), w("\"$x\"", false), w("y", true)]
    );
    // $'…' の `\'` で閉じない・ヒアドキュメントの本文は語にならない
    assert_eq!(
        words(r"echo $'it\'s' x"),
        vec![w("echo", true), w(r"$'it\'s'", true), w("x", true)]
    );
    assert_eq!(
        words("cat <<X\n\"q\" $(f)\nX\necho y\n"),
        vec![w("cat", true), w("f", true), w("echo", true), w("y", true)]
    );
}
