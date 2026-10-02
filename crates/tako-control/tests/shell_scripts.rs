//! シェルスクリプトの静的検査（番犬）
//!
//! `bash -n` では見つからず、その行が実行されるまで潜伏する種類の欠陥を
//! **CI で落とす**ための検査。`.agent/conventions.md` の
//! 「シェルスクリプトで日本語を出すときの変数展開（Issue #837）」と
//! 「シェルスクリプトは macOS 同梱の bash 3.2 で通す（Issue #1499 / #1518）」に
//! 書いてある規約の機械化（同じ節の「`set -e` と EXIT trap を併用するなら番人を通す」= #1864 も）。

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
