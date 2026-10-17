//! シェルスクリプトの静的検査（番犬）
//!
//! `bash -n` では見つからず、その行が実行されるまで潜伏する種類の欠陥を
//! **CI で落とす**ための検査。`.agent/conventions.md` の
//! 「シェルスクリプトで日本語を出すときの変数展開（Issue #837）」と
//! 「シェルスクリプトは macOS 同梱の bash 3.2 で通す（Issue #1499 / #1518）」に
//! 書いてある規約の機械化（同じ節の「`set -e` と EXIT trap を併用するなら番人を通す」= #1864 と
//! 「`"$( … )"` の中のダブルクォートに `{…,…}` を置かない」= #1924・
//! 「pipefail の下で早く抜ける読み手（`grep -q` / `head`）へパイプしない」= #1633 / #2272 も）。

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
///
/// ついでに**パイプライン**（`|` / `|&` で繋がった単純コマンドの並び）も深さごとに集め、
/// その終了コードの行き先（条件・`&&` / `||`・素の文・`x=$( … )` の代入・引数の中の `$( … )`）を
/// 添える（#1633 / #2272 の「早く抜ける読み手」の検査が使う）。
struct WordLexer {
    c: Vec<char>,
    i: usize,
    words: Vec<RawWord>,
    pending: Vec<Heredoc>,
    /// 読んでいる最中の `list` の深さごとのパイプライン（`$( … )` / `( … )` で 1 段積む）
    levels: Vec<PipeLevel>,
    /// 読み終えた、2 段以上のパイプライン
    pipelines: Vec<LexPipeline>,
    /// 読んでいる最中の語の始まりと、それがコマンドの先頭（代入を置ける位置）か。
    /// `x=$( … )` の置換が素の代入の値かを `cmdsub` が判じるのに使う（`list` の間は None）
    word_ctx: Option<(usize, bool)>,
}

/// パイプラインの終わり方（終了コードを誰が見るか）
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PipeEnd {
    /// `;` / 改行 / `)` / 終端（素の文。`set -e` ならその非 0 でスクリプトが止まる）
    #[default]
    Plain,
    /// `&&` の左辺
    And,
    /// `||` の左辺。`discard` は右辺が `true` / `:` だけ（= 終了コードを捨てる）
    Or { discard: bool },
    /// `&`（終了コードは誰も見ない）
    Background,
}

/// パイプラインが置かれた list の種類
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ListKind {
    /// ファイル直下・関数の本体・`( … )`
    #[default]
    Statement,
    /// `$( … )`。`assign` はその置換が素の代入（`x=$( … )` / `x="$( … )"`）の値か。
    /// `local x=$( … )` や引数の中の置換は false（終了コードは捨てられる）
    CmdSub { assign: bool },
    /// `<( … )` / `>( … )`（終了コードは誰も見ない）
    ProcSub,
}

/// 読み終えたパイプライン（2 段以上）と、その終了コードの行き先
#[derive(Clone, Debug, PartialEq, Eq)]
struct LexPipeline {
    stages: Vec<LexCommand>,
    kind: ListKind,
    /// 先頭に `if` / `elif` / `while` / `until` / `!` がある（= 条件・否定）
    cond: bool,
    end: PipeEnd,
    /// `kind` が `CmdSub { assign: true }` のとき、その代入のコマンドの (cond, end)
    outer: Option<(bool, PipeEnd)>,
}

/// パイプラインの 1 段 = 単純コマンド 1 つ（語は原文のまま）
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LexCommand {
    /// 先頭の語のファイル先頭からの文字位置
    start: usize,
    words: Vec<String>,
}

/// 1 つの深さで組み立て中のパイプライン
#[derive(Default)]
struct PipeLevel {
    cur: Option<LexCommand>,
    stages: Vec<LexCommand>,
    /// 直前が `|`（その後の改行は同じパイプラインの続き）
    after_pipe: bool,
    kind: ListKind,
    /// いまのコマンドの `x=$( … )` の中で読み終えたパイプライン（`pipelines` の添字）。
    /// このコマンドの終わり方が決まったら `outer` へ写す
    assign_inner: Vec<usize>,
}

const WORD_END: &[char] = &[' ', '\t', '\n', ';', '&', '|', '(', ')', '<', '>'];

impl WordLexer {
    fn new(text: &str) -> Self {
        WordLexer {
            c: text.chars().collect(),
            i: 0,
            words: Vec::new(),
            pending: Vec::new(),
            levels: Vec::new(),
            pipelines: Vec::new(),
            word_ctx: None,
        }
    }

    /// 組み立て中の単純コマンドへ語を足す
    fn pipe_word(&mut self, start: usize, text: &str) {
        if let Some(lv) = self.levels.last_mut() {
            lv.after_pipe = false;
            lv.cur
                .get_or_insert_with(|| LexCommand {
                    start,
                    words: Vec::new(),
                })
                .words
                .push(text.to_string());
        }
    }

    /// `|` / `|&`: いまの単純コマンドを段として積み、次の段を待つ
    fn pipe_stage(&mut self) {
        let at = self.i;
        if let Some(lv) = self.levels.last_mut() {
            let cmd = lv.cur.take().unwrap_or(LexCommand {
                start: at,
                words: Vec::new(),
            });
            lv.stages.push(cmd);
            lv.after_pipe = true;
        }
    }

    /// `;` / `&` / `&&` / `||` / `)` / 改行 / 終端: パイプラインを閉じる
    fn pipe_end(&mut self, end: PipeEnd) {
        let Some(lv) = self.levels.last_mut() else {
            return;
        };
        if let Some(cmd) = lv.cur.take() {
            lv.stages.push(cmd);
        }
        lv.after_pipe = false;
        let stages = std::mem::take(&mut lv.stages);
        let kind = lv.kind;
        let cond = stages.first().is_some_and(|s| {
            s.words
                .iter()
                .take_while(|w| LEADING_KEYWORDS.contains(&w.as_str()))
                .any(|w| matches!(w.as_str(), "if" | "elif" | "while" | "until" | "!"))
        });
        // このコマンドの `x=$( … )` の中のパイプラインは、このコマンドの終わり方に従う
        for k in std::mem::take(&mut lv.assign_inner) {
            if let Some(p) = self.pipelines.get_mut(k) {
                p.outer = Some((cond, end));
            }
        }
        if stages.len() >= 2 {
            let k = self.pipelines.len();
            self.pipelines.push(LexPipeline {
                stages,
                kind,
                cond,
                end,
                outer: None,
            });
            if kind == (ListKind::CmdSub { assign: true }) {
                let parent = self.levels.len().checked_sub(2);
                if let Some(parent) = parent.and_then(|d| self.levels.get_mut(d)) {
                    parent.assign_inner.push(k);
                }
            }
        }
    }

    /// 改行。`|` の直後（行末の `|`・`|` の後のコメント）なら同じパイプラインが次の行へ続く
    fn pipe_newline(&mut self) {
        let continues = self
            .levels
            .last()
            .is_some_and(|lv| lv.cur.is_none() && lv.after_pipe);
        if !continues {
            self.pipe_end(PipeEnd::Plain);
        }
    }

    /// `||` の直後（空白を飛ばした次の語）が `true` / `:` だけか = 左辺の終了コードを捨てる
    fn or_discards_status(&self) -> bool {
        let mut k = 0;
        while matches!(self.peek(k), Some(' ' | '\t')) {
            k += 1;
        }
        let mut w = String::new();
        while let Some(c) = self.peek(k).filter(|c| !WORD_END.contains(c)) {
            w.push(c);
            k += 1;
        }
        w == "true" || w == ":"
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
        self.list_of(until_close, ListKind::Statement);
    }

    fn list_of(&mut self, until_close: bool, kind: ListKind) {
        self.levels.push(PipeLevel {
            kind,
            ..PipeLevel::default()
        });
        // list の中は語の外（ヒアドキュメントの本文の `$( … )` を外の語の代入と取り違えない）
        let saved = self.word_ctx.take();
        self.list_body(until_close);
        self.pipe_end(PipeEnd::Plain);
        self.word_ctx = saved;
        self.levels.pop();
    }

    fn list_body(&mut self, until_close: bool) {
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
                    self.pipe_newline();
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
                    self.pipe_end(PipeEnd::Plain);
                }
                '&' | '|' => {
                    // `|` / `|&` は段の区切り。`||` / `&&` / `&` はパイプラインを閉じる
                    if self.at("|&") {
                        self.i += 2;
                        self.pipe_stage();
                    } else if self.at("||") {
                        self.i += 2;
                        let discard = self.or_discards_status();
                        self.pipe_end(PipeEnd::Or { discard });
                    } else if self.at("&&") {
                        self.i += 2;
                        self.pipe_end(PipeEnd::And);
                    } else {
                        self.i += 1;
                        if ch == '|' {
                            self.pipe_stage();
                        } else {
                            self.pipe_end(PipeEnd::Background);
                        }
                    }
                    cmd_start = true;
                }
                ')' => {
                    self.i += 1;
                    if dbl_bracket && dbl_paren > 0 {
                        dbl_paren -= 1;
                    } else if case.last() == Some(&CaseState::Pattern) {
                        *case.last_mut().expect("case") = CaseState::Body;
                        cmd_start = true;
                        self.pipe_end(PipeEnd::Plain);
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
                        // `name() {` の `()` の後ろは本体 = 別の単純コマンド（空の `( )` は構文エラー）
                        let fn_def = self.at("()");
                        self.i += 1;
                        self.list(true);
                        if fn_def {
                            self.pipe_end(PipeEnd::Plain);
                        }
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
                        self.list_of(true, ListKind::ProcSub);
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
                    // 腕のパターン・配列代入はパイプラインの段の語ではない
                    if !array && case.last() != Some(&CaseState::Pattern) {
                        self.pipe_word(start, &text);
                    }
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
        let saved = self.word_ctx.replace((start, cmd_start));
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
        self.word_ctx = saved;
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
            // コマンドの先頭の `x=` / `x="` に続く置換 = 素の代入の値（終了コードが代入の結果になる）
            let assign = self.word_ctx.is_some_and(|(ws, cmd_start)| {
                cmd_start && is_assignment_word(&self.c[ws..self.i].iter().collect::<String>())
            });
            self.i += 2;
            self.list_of(true, ListKind::CmdSub { assign });
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
        ..WordLexer::new("")
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
/// `start` = いま展開している文字列の先頭（3.2 の `brace_expand` は対の後ろを新しい文字列として
/// 再帰するので、「空白で囲まれた `{`」の判定の先頭（`!i`）はそこ。読み直しの位置ではない）。
///
/// **3.2 は `"…"` の中を「次の `"` まで」としか見ず、`$( … )` の入れ子を知らない**。
/// だから `"$(f "{a,b}")"` は内側の `"` で外側のクォートが閉じたと読み、`{a,b}` を
/// クォートの外と取り違える（4.0 以降は `"` の中の `$(` を読み飛ばすので起きない）。
///
/// 閉じの `}` を探すときは、深さ 0 の区切り（`,` か、直後が `}` でない `..`）を見るまで `}` を
/// 閉じにしない（braces.c の `commas`。#2301）。だから 3.2 は `{x}` の `}` を読み飛ばし、
/// 後ろの `,` と `}` まで 1 つの対にする（`"$(g "{\"a\":1}" "x,y}")"` が割れる）
fn gobble32(t: &[char], start: usize, from: usize, satisfy: char) -> Option<usize> {
    // braces.c の `brace_whitespace`（文字列の終わりも空白に数える）
    let ws = |c: Option<&char>| matches!(c, None | Some(' ' | '\t' | '\n'));
    let mut level = 0usize;
    let mut quoted: Option<char> = None;
    let mut commas = usize::from(satisfy != '}');
    let mut i = from;
    while i < t.len() {
        let c = t[i];
        if c == '\\' && matches!(quoted, None | Some('"') | Some('`')) {
            i += 2;
            continue;
        }
        if c == '$' && t.get(i + 1) == Some(&'{') && quoted != Some('\'') {
            // `${` は `\{` と同じく 2 文字を読み飛ばし、クォートの外なら深さを上げる
            i += 2;
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
                ..WordLexer::new("")
            };
            lx.cmdsub();
            i = lx.i;
            continue;
        }
        if c == satisfy && level == 0 && commas > 0 {
            // 空白で囲まれた `{`（と `{}`）は波括弧展開の始まりと見ない
            let lone = c == '{'
                && (i == start || ws(t.get(i - 1)))
                && (ws(t.get(i + 1)) || t.get(i + 1) == Some(&'}'));
            if !lone {
                return Some(i);
            }
        } else if c == '{' {
            level += 1;
        } else if c == '}' && level > 0 {
            level -= 1;
        } else if satisfy == '}'
            && level == 0
            && (c == ',' || (c == '.' && t.get(i + 1) == Some(&'.') && t.get(i + 2) != Some(&'}')))
        {
            commas += 1;
        }
        i += 1;
    }
    None
}

/// 3.2 の `legal_number`（general.c）: `strtoimax` が読める整数（先頭の空白・符号を許す）の後ろに
/// 空白（` ` / タブ）しか無い。何も読めなくても空白だけなら通る（3.2 の実装どおり）
fn legal_number32(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while b
        .get(i)
        .is_some_and(|c| matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
    {
        i += 1;
    }
    let mut j = i;
    if b.get(j).is_some_and(|c| matches!(c, b'+' | b'-')) {
        j += 1;
    }
    let digits = j;
    while b.get(j).is_some_and(u8::is_ascii_digit) {
        j += 1;
    }
    if j > digits && s[i..j].parse::<i64>().is_err() {
        return false; // 桁あふれ（errno）
    }
    // 数字が 1 つも無ければ strtoimax は何も読まない（読み終わりの位置 = 先頭）
    let mut end = if j > digits { j } else { 0 };
    while b.get(end).is_some_and(|c| matches!(c, b' ' | b'\t')) {
        end += 1;
    }
    !s.is_empty() && end == b.len()
}

/// 3.2 がこの中身（`{` と `}` の間）を展開するか（braces.c の `brace_expand`）: エスケープされて
/// いない `,` があるか、連番（最初の `..` で左右に分け、左右とも整数か英字 1 文字。3.2 は刻み
/// `1..5..2` を知らない）
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
    let letter = |x: &str| x.len() == 1 && x.bytes().all(|c| c.is_ascii_alphabetic());
    !a.is_empty()
        && !b.is_empty()
        && ((legal_number32(a) && legal_number32(b)) || (letter(a) && letter(b)))
}

/// 語の中で bash 3.2 が波括弧展開する対（`{` と `}` の語の中の文字位置。いちばん外側の対だけ）。
/// クォートの外の意図した `{a,b}` も含む（実の `/bin/bash` との突き合わせ = #1931 が全体を見る）。
///
/// braces.c の `brace_expand` の写し: 閉じの見つかる最初の `{` を対とし、展開できる対なら
/// その後ろを新しい文字列として続きを探す。**展開できない対（`,` も正しい連番も無い）に
/// 当たったら、3.2 は語の残りを一切展開しない**（#2301。`"S{1..5..2}" "{a,b}"` の `{a,b}` は割れない）
fn brace32_pairs(word: &str) -> Vec<(usize, usize)> {
    let t: Vec<char> = word.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    'text: loop {
        // 閉じの無い `{` は読み飛ばして次の `{` を探す（3.2 の do-while）
        let mut i = start;
        let (open, close) = loop {
            let Some(open) = gobble32(&t, start, i, '{') else {
                break 'text;
            };
            match gobble32(&t, start, open + 1, '}') {
                Some(close) => break (open, close),
                None => i = open + 1,
            }
        };
        if !amble_expands32(&t[open + 1..close]) {
            break;
        }
        out.push((open, close));
        start = close + 1;
    }
    out
}

/// 語の中で、**bash 3.2 だけが**波括弧展開してしまう対（語の中の文字位置）。
///
/// 3.2 が展開する対のうち、正しく読めば `{` か `}` がクォートかコマンド置換の中に在るものだけを
/// 返す（クォートの外の `{a,b}` は意図した展開なので返さない = 過大申告しない。
/// `{x}"…,…}"` のようにクォートの外の `{` からクォートの中の `}` までを対にするものは返す）。
fn brace32_misreads(word: &str) -> Vec<(usize, usize)> {
    let t: Vec<char> = word.chars().collect();
    let top = top_level_unquoted(&t);
    brace32_pairs(word)
        .into_iter()
        .filter(|&(open, close)| !top[open] || !top[close])
        .collect()
}

/// ファイルの中で、波括弧展開を受ける語（コマンド置換の中の語も）に bash 3.2 が展開する対
/// （ファイル先頭からの文字位置）。`misread_only` なら 3.2 だけが展開するもの = 番犬が名指すもの
fn brace32_file_pairs(src: &str, misread_only: bool) -> std::collections::BTreeSet<(usize, usize)> {
    let mut out = std::collections::BTreeSet::new();
    for w in raw_words(src).into_iter().filter(|w| w.brace_expanded) {
        let found = if misread_only {
            brace32_misreads(&w.text)
        } else {
            brace32_pairs(&w.text)
        };
        out.extend(
            found
                .into_iter()
                .map(|(open, close)| (w.start + open, w.start + close)),
        );
    }
    out
}

/// 1 ファイルぶんの違反（file:line で名指しする文言）
fn brace32_violations(rel: &str, src: &str) -> Vec<String> {
    let chars: Vec<char> = src.chars().collect();
    let lines: Vec<&str> = src.lines().collect();
    let line_of = |pos: usize| chars[..pos].iter().filter(|&&c| c == '\n').count() + 1;
    let mut out = Vec::new();
    let opens: std::collections::BTreeSet<usize> = brace32_file_pairs(src, true)
        .into_iter()
        .map(|(open, _)| open)
        .collect();
    for pos in opens {
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
    // 3.2 は区切り（`,` / `..`）を見るまで `}` を閉じにしないので、`{x}` の `}` を読み飛ばして
    // 後ろの `,` と `}` まで 1 つの対にする（#2301。bash 5 はどちらも割れない）
    assert_eq!(hits(r#"f "$(show "{x}" ",}")""#), 1);
    assert_eq!(hits(r#"f "$(show "{\"a\":1}" "x,y}")""#), 1);

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
    assert_eq!(hits(r#"f "$(show "S{1..5..2}" "{a,b}")""#), 0); // その対で語の残りも止まる（#2301）
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

// ---------------------------------------------------------------------------
// 波括弧の写しを実の bash 3.2 と突き合わせる（Issue #1931）
// ---------------------------------------------------------------------------
//
// 上の写し（`raw_words` / `gobble32` / `amble_expands32` / `brace32_pairs` / `top_level_unquoted`）は
// 3.2 の `braces.c` を読んで書いたもので、PR #1929 では乱数の語 48,000 通りを実の `/bin/bash` に通して
// 確かめた。その差分試験の常設版: 固定の種で組んだ語を `/bin/bash`（3.2）へ `set +B`（波括弧展開
// なし）と `set -B` の 2 回通し、波括弧ごとの目印が出力から消えたか（= 3.2 が展開したか）を
// 写しの読みと 1 つずつ比べる。bash 5 は要らない（正しい読みは語を組むときに分かっている）。

/// 突き合わせに使う bash 3.2 の場所を差し替える env（既定 `/bin/bash`）。
/// 3.2 を自分で建てた環境で回す・「3.2 が無い」を注入して門を確かめるときに使う
const BASH32_ENV: &str = "TAKO_TEST_BASH32";

/// 突き合わせの門の判定
#[derive(Debug, PartialEq, Eq)]
enum Bash32Gate {
    Run,
    /// 3.2 の bash が無い OS（Linux / Windows）。理由を出して飛ばす
    Skip(String),
    /// macOS なのに 3.2 が使えない。黙って緑にせず落とす
    Fail(String),
}

/// 門（純関数）。`probe` = `bash` の `$BASH_VERSION`、または起動できなかった理由。
/// macOS は `/bin/bash` 3.2 を必ず同梱する（SIP の下で差し替えられない）ので、使えなければ
/// 飛ばさずに落とす。CI の macOS で突き合わせが黙って消えないため（#2106 の門と同じ考え）
fn bash32_gate(on_macos: bool, bash: &str, probe: Result<String, String>) -> Bash32Gate {
    let reason = match probe {
        Ok(v) if v.starts_with("3.2.") => return Bash32Gate::Run,
        Ok(v) => format!("{bash} は bash {v}（3.2 ではない）"),
        Err(e) => format!("{bash} を起動できない（{e}）"),
    };
    if on_macos {
        Bash32Gate::Fail(format!(
            "波括弧の写しを実の bash 3.2 と突き合わせられない: {reason}。macOS は /bin/bash 3.2 を\n\
             同梱するので飛ばさない（#1931）。別の 3.2 を使うなら {BASH32_ENV}=<path> で指す"
        ))
    } else {
        Bash32Gate::Skip(format!(
            "{reason}。3.2 の bash が無い OS なので波括弧の写しの突き合わせを飛ばす\n\
             （macOS の CI で回る。3.2 を建てたなら {BASH32_ENV}=<path> で指す。#1931）"
        ))
    }
}

/// 起動ファイル（`BASH_ENV`）・利用者のロケール・HOME を持ち込まない bash
fn clean_bash(bash: &str) -> std::process::Command {
    let mut cmd = std::process::Command::new(bash);
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null());
    cmd
}

fn bash_version(bash: &str) -> Result<String, String> {
    let out = clean_bash(bash)
        .args(["-c", "printf %s \"$BASH_VERSION\""])
        .output()
        .map_err(|e| e.to_string())?;
    let v = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        return Err(format!("終了コード {}", out.status));
    }
    if v.is_empty() {
        return Err("$BASH_VERSION が空（bash ではない）".to_string());
    }
    Ok(v)
}

/// 突き合わせる波括弧の種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BraceKind {
    Comma,
    TrailingComma,
    LeadingComma,
    NoComma,
    EscapedComma,
    /// `{ a,b }`（空白で囲まれた `{` は展開の始まりにならない。クォートの中にだけ置く）
    Spaced,
    SeqNum,
    SeqAlpha,
    /// 刻みつき（3.2 は知らない）
    SeqStep,
    SeqNeg,
}

const BRACE_KINDS: &[BraceKind] = &[
    BraceKind::Comma,
    BraceKind::TrailingComma,
    BraceKind::LeadingComma,
    BraceKind::NoComma,
    BraceKind::EscapedComma,
    BraceKind::Spaced,
    BraceKind::SeqNum,
    BraceKind::SeqAlpha,
    BraceKind::SeqStep,
    BraceKind::SeqNeg,
];

impl BraceKind {
    /// (原文, 原文の中の `{` の位置, 目印)。`k` = 語の中の通し番号（1〜9。目印を語の中で一意に
    /// する）。目印は展開されなければ出力にそのまま残り、展開されれば消える部分文字列
    fn atom(self, k: usize) -> (String, usize, String) {
        use BraceKind::*;
        assert!((1..=9).contains(&k), "目印の番号は 1 桁（{k}）");
        let comma = |text: String, witness: String| (text, 0, witness);
        let seq = |amble: &str| (format!("S{k}{{{amble}}}"), 2, format!("S{k}{{"));
        match self {
            Comma => comma(format!("{{Q{k},R{k}}}"), format!("{{Q{k}")),
            TrailingComma => comma(format!("{{Q{k},}}"), format!("{{Q{k}")),
            LeadingComma => comma(format!("{{,Q{k}}}"), format!("{{,Q{k}")),
            NoComma => comma(format!("{{Q{k}}}"), format!("{{Q{k}")),
            EscapedComma => comma(format!("{{Q{k}\\,R{k}}}"), format!("{{Q{k}")),
            Spaced => comma(format!("{{ Q{k},R{k} }}"), format!("{{ Q{k}")),
            SeqNum => seq("1..3"),
            SeqAlpha => seq("a..c"),
            SeqStep => seq("1..5..2"),
            SeqNeg => seq("-1..1"),
        }
    }
}

/// 語の中の差し込み口の文脈（正しく読んだとき）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// 外側の語のクォートの外（`f @`）。引数なら波括弧展開を受け、代入の値なら受けない
    Outer,
    /// コマンド置換の中のコマンドの語のクォートの外（`"$(show @)"`）。いつも受ける
    Inner,
    /// ダブルクォートの中（入れ子の深さを問わない）
    Dq,
    /// 単一引用符の中
    Sq,
    /// `$'…'` の中。3.2 の構文解析は `$'…'` のエスケープを波括弧展開より前に解いて `'…'` へ
    /// 置き換える（`$'\\{'` は `'\{'`）が、写しはまだそれを知らない（#2304）ので `\` の飾りは置かない
    AnsiC,
    /// クォートの外の `${U:-@}`（`${` が波括弧展開を止める）
    Param,
    /// `"${U:-@}"`
    DqParam,
    /// バッククォートの中のダブルクォート（`` `show "@"` ``）。バッククォートの中は `\` を 1 段
    /// 外してから読むので、`\` の飾りは置かない（`\\"` が閉じのクォートを食う）
    BacktickDq,
}

impl Slot {
    /// 波括弧の前後に置く飾り（3.2 の読み違えの入口: クォート・`\`・`${`・`$(`・空白）。
    /// その文脈で構文を壊さないものだけ。末尾の `\` は閉じのクォートを食うので後ろには置かない
    fn fillers(self, before: bool) -> &'static [&'static str] {
        use Slot::*;
        match (self, before) {
            (Outer | Inner, true) => &["", "x", "\\\"", "${U}", "$(show z)", "\\\\", "\\", "\\ "],
            (Outer | Inner, false) => &["", "y", "\\\"", "${U}", "$(show z)", "\\\\"],
            (Dq, true) => &[
                "",
                "x",
                " ",
                "it's ",
                "\\\"",
                "${U}",
                "$(show z)",
                "\\\\",
                "\\",
            ],
            (Dq, false) => &["", "y", " ", " it's", "\\\"", "${U}", "$(show z)", "\\\\"],
            (Sq, true) => &["", "x", " ", "\\\"", "${U}", "\\\\", "\\"],
            (Sq, false) => &["", "y", " ", "\\\"", "${U}"],
            (AnsiC, true) => &["", "x", " ", "${U}"],
            (AnsiC, false) => &["", "y", " ", "${U}"],
            (Param, true) => &["", "x", "${U}", "$(show z)"],
            (Param, false) => &["", "y", "${U}"],
            (DqParam, true) => &["", "x", " ", "${U}", "$(show z)"],
            (DqParam, false) => &["", "y", " "],
            (BacktickDq, true) => &["", "x", " ", "it's ", "${U}", "$(show z)"],
            (BacktickDq, false) => &["", "y", " ", " it's", "${U}", "$(show z)"],
        }
    }

    /// 空白を含む波括弧（`{ a,b }`）を置けるか（クォートの外では空白で語が割れる）
    fn allows(self, kind: BraceKind) -> bool {
        kind != BraceKind::Spaced
            || matches!(
                self,
                Slot::Dq | Slot::Sq | Slot::AnsiC | Slot::DqParam | Slot::BacktickDq
            )
    }
}

/// 語の形（`@` が差し込み口。並び順に `Slot` が対応する）。#1924 の表の文脈と、3.2 の
/// `brace_gobbler` の分かれ道（`"` / `'` / `` ` `` / `\` / `${` / クォートの外の `$(`）を一通り
const BRACE_SHAPES: &[(&str, &[Slot])] = {
    use Slot::*;
    &[
        ("@", &[Outer]),
        ("@\"$(show \"x\")\"", &[Outer]),
        ("\"@\"", &[Dq]),
        ("'@'", &[Sq]),
        ("$'@'", &[AnsiC]),
        ("\"$(show \"@\")\"", &[Dq]),
        ("\"$(show '@')\"", &[Sq]),
        ("\"$(show @)\"", &[Inner]),
        ("$(show \"@\")", &[Dq]),
        ("$(show @)", &[Inner]),
        ("\"$(show $(show \"@\"))\"", &[Dq]),
        ("\"$(show \"x\" \"@\")\"", &[Dq]),
        ("\"$(show \"it's @\")\"", &[Dq]),
        ("\"$(show \"`show z`@\")\"", &[Dq]),
        ("\"${U:-@}\"", &[DqParam]),
        ("${U:-@}", &[Param]),
        ("`show \"@\"`", &[BacktickDq]),
        ("\"$(show \\\"@\\\")\"", &[Inner]),
        ("\"$(show \"$U\"@)\"", &[Inner]),
        ("\"${U:-$(show \"@\")}\"", &[Dq]),
        ("\"$(show \"$(show \"@\")\")\"", &[Dq]),
        ("\"$(show \"@\" \"@\")\"", &[Dq, Dq]),
        ("\"$(show \"@\" @)\"", &[Dq, Inner]),
        ("\"$(show \"@\")$(show \"@\")\"", &[Dq, Dq]),
        ("\"$(show \"@\"'@')\"", &[Dq, Sq]),
    ]
};

/// 突き合わせの 1 行に置いた波括弧
#[derive(Debug)]
struct OracleBrace {
    /// 行の中の `{` の位置（文字位置）
    pos: usize,
    witness: String,
    /// 正しく読んだとき波括弧展開を受ける位置か（クォート・`${…}`・代入の値の外）
    eligible: bool,
}

/// 突き合わせの 1 行（`f <語>` か `x=<語>; f "$x"`）
#[derive(Debug)]
struct OracleCase {
    line: String,
    braces: Vec<OracleBrace>,
}

/// 決定的な擬似乱数（xorshift64。種を固定して毎回同じ語を組む）
struct Xorshift(u64);

impl Xorshift {
    fn below(&mut self, n: usize) -> usize {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x % n as u64) as usize
    }

    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len())]
    }
}

/// 突き合わせる行の表。形 × 波括弧の種類の総当たり（1 つ目の差し込み口）に、形を 2〜3 個
/// つないだ語（3.2 がクォートの数え違いを次の形へ持ち越す読み）を固定の種で足す。
/// 4 行に 1 行は代入の値（`x=<語>`。外側の語は波括弧展開を受けない）にする
fn bash32_oracle_cases() -> Vec<OracleCase> {
    /// 語を組む途中
    struct Word {
        text: String,
        /// (語の中の `{` の位置, 目印, 文脈, 直前の飾りの `\` が `{` をエスケープしているか)
        braces: Vec<(usize, String, Slot, bool)>,
    }
    fn push_shape(
        w: &mut Word,
        rng: &mut Xorshift,
        (template, slots): (&str, &[Slot]),
        first: Option<BraceKind>,
    ) {
        let mut parts = template.split('@');
        w.text.push_str(parts.next().unwrap_or(""));
        for (n, (part, &slot)) in parts.zip(slots).enumerate() {
            let kind = match first {
                Some(kind) if n == 0 => kind,
                _ => loop {
                    let kind = rng.pick(BRACE_KINDS);
                    if slot.allows(kind) {
                        break kind;
                    }
                },
            };
            let (atom, open, witness) = kind.atom(w.braces.len() + 1);
            let before = rng.pick(slot.fillers(true));
            w.text.push_str(before);
            let at = w.text.chars().count() + open;
            w.text.push_str(&atom);
            w.text.push_str(rng.pick(slot.fillers(false)));
            w.text.push_str(part);
            w.braces
                .push((at, witness, slot, before == "\\" && open == 0));
        }
    }

    let mut rng = Xorshift(0x1931_2026_1018_0001);
    let mut words = Vec::new();
    for &shape in BRACE_SHAPES {
        for &kind in BRACE_KINDS.iter().filter(|&&k| shape.1[0].allows(k)) {
            let mut w = Word {
                text: String::new(),
                braces: Vec::new(),
            };
            push_shape(&mut w, &mut rng, shape, Some(kind));
            words.push(w);
        }
    }
    for _ in 0..240 {
        let mut w = Word {
            text: String::new(),
            braces: Vec::new(),
        };
        for _ in 0..2 + rng.below(2) {
            let shape = rng.pick(BRACE_SHAPES);
            if w.braces.len() + shape.1.len() > 9 {
                break;
            }
            push_shape(&mut w, &mut rng, shape, None);
        }
        words.push(w);
    }
    words
        .into_iter()
        .enumerate()
        .map(|(n, w)| {
            let assign = n % 4 == 3;
            let (head, tail) = if assign {
                ("x=", "; f \"$x\"")
            } else {
                ("f ", "")
            };
            let shift = head.chars().count();
            OracleCase {
                line: format!("{head}{}{tail}", w.text),
                braces: w
                    .braces
                    .into_iter()
                    .map(|(at, witness, slot, escaped)| OracleBrace {
                        pos: shift + at,
                        witness,
                        eligible: !escaped
                            && (slot == Slot::Inner || (slot == Slot::Outer && !assign)),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// 行ごとの出力。`brace` = 波括弧展開をするか（`set -B` / `set +B`）。
/// 1 つの bash で全行を `eval` する（構文の誤りはその行だけで止まる）。
/// `show` / `f` は引数を空白 1 つでつないで出す（クォートの外で割れた `{ Q1,R1 }` も目印
/// `{ Q1` の形に戻る）。`set -f` はクォートの外の展開結果をファイル名に当てないため。
/// 出力の区切りが揃わない（途中で bash が終わった）ときは Err
fn bash32_outputs(bash: &str, lines: &[String], brace: bool) -> Result<Vec<String>, String> {
    let mut script = format!(
        "set -f {}B\nshow() {{ printf '%s ' \"$@\"; }}\nf() {{ printf '%s ' \"$@\"; }}\n",
        if brace { '-' } else { '+' }
    );
    for (i, line) in lines.iter().enumerate() {
        script.push_str(&format!(
            "printf '\\n@@{i}@@\\n'; eval '{}'\n",
            line.replace('\'', "'\\''")
        ));
    }
    script.push_str(&format!("printf '\\n@@{}@@\\n'\n", lines.len()));
    let out = clean_bash(bash)
        .args(["-c", &script])
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut outputs = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let (start, end) = (format!("\n@@{i}@@\n"), format!("\n@@{}@@\n", i + 1));
        let body = stdout
            .split_once(&start)
            .and_then(|(_, rest)| rest.split_once(&end))
            .map(|(body, _)| body.to_string())
            .ok_or_else(|| {
                format!(
                    "{}行目（{line}）の出力の区切りが無い（bash が途中で終わった？ 終了コード {}）",
                    i + 1,
                    out.status
                )
            })?;
        outputs.push(body);
    }
    Ok(outputs)
}

#[test]
fn 波括弧の写しは実のbash32と食い違わない() {
    let bash = std::env::var(BASH32_ENV)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/bin/bash".to_string());
    match bash32_gate(cfg!(target_os = "macos"), &bash, bash_version(&bash)) {
        Bash32Gate::Run => {}
        Bash32Gate::Skip(reason) => {
            eprintln!("skip: {reason}");
            return;
        }
        Bash32Gate::Fail(reason) => panic!("{reason}"),
    }

    let cases = bash32_oracle_cases();
    let lines: Vec<String> = cases.iter().map(|c| c.line.clone()).collect();
    // 展開なし（目印がそのまま出ることの確かめ）と展開ありを並べて回す
    let (plain, braced) = std::thread::scope(|s| {
        let plain = s.spawn(|| bash32_outputs(&bash, &lines, false));
        let braced = bash32_outputs(&bash, &lines, true);
        (plain.join().expect("set +B の bash"), braced)
    });
    let (plain, braced) = (
        plain.expect("set +B の出力"),
        braced.expect("set -B の出力"),
    );

    let says = |b: bool| if b { "展開する" } else { "展開しない" };
    let mut problems = Vec::new();
    // 波括弧展開なし（set +B）でも目印が出ない = 表の組み方の誤り（その語は突き合わせに使えない）
    let mut table_errors = Vec::new();
    // [3.2 だけが展開した, 意図どおり展開した, 展開しなかった]（表の検出力の自己検査）
    let mut tally = [0usize; 3];
    // 写しが「クォートや別の波括弧をまたぐ対」を読んだ行の数
    let mut tangled_lines = 0usize;
    for (i, case) in cases.iter().enumerate() {
        let chars: Vec<char> = case.line.chars().collect();
        let pairs = brace32_file_pairs(&case.line, false);
        let opens: std::collections::BTreeSet<usize> = pairs.iter().map(|&(o, _)| o).collect();
        let misread: std::collections::BTreeSet<usize> = brace32_file_pairs(&case.line, true)
            .into_iter()
            .map(|(o, _)| o)
            .collect();
        // 対の中身にクォート・`$`・括弧・波括弧が入る = 3.2 がクォートや別の波括弧をまたいで組んだ対
        // （`{x}` の `}` を読み飛ばす = #2301）。展開後の語は構文が崩れ、行の出力がどう壊れるか
        // （行ごと失敗することもある）は写しの外なので、その行は「開きの目印が消える」ことだけを見る
        let tangled = pairs.iter().any(|&(o, c)| {
            chars[o + 1..c]
                .iter()
                .any(|ch| matches!(ch, '"' | '\'' | '`' | '$' | '(' | ')' | '{' | '}'))
        });
        tangled_lines += usize::from(tangled);
        for &p in &opens {
            if !case.braces.iter().any(|b| b.pos == p) {
                problems.push(format!(
                    "{}\n  写しが目印の無い `{{`（{p} 文字目）を 3.2 が展開すると読んだ",
                    case.line
                ));
            }
        }
        for b in &case.braces {
            if !plain[i].contains(&b.witness) {
                table_errors.push(format!(
                    "{}\n  目印 `{}` / set +B の出力: {:?}",
                    case.line, b.witness, plain[i]
                ));
                continue;
            }
            let real = !braced[i].contains(&b.witness);
            let real_misread = real && !b.eligible;
            let copy = opens.contains(&b.pos);
            let copy_misread = misread.contains(&b.pos);
            if tangled {
                if copy && !real {
                    problems.push(format!(
                        "{}\n  目印 `{}`（{} 文字目）: 写しは 3.2 がここから対を組んで展開すると読んだが、\n  \
                         実の 3.2 の出力に目印が残った\n  set -B の出力: {:?}",
                        case.line, b.witness, b.pos, braced[i]
                    ));
                }
                continue;
            }
            if real != copy || real_misread != copy_misread {
                problems.push(format!(
                    "{}\n  目印 `{}`（{} 文字目）: 実の 3.2 は{}・写しは{}（3.2 だけが展開する = 番犬が\n  \
                     名指すべき: 実 {real_misread} / 写し {copy_misread}）\n  set -B の出力: {:?}",
                    case.line,
                    b.witness,
                    b.pos,
                    says(real),
                    says(copy),
                    braced[i]
                ));
            }
            tally[if real_misread {
                0
            } else if real {
                1
            } else {
                2
            }] += 1;
        }
    }
    assert!(
        table_errors.is_empty(),
        "表の誤り: 波括弧展開なし（set +B）でも目印が出ない語が {} 個（飾りがその文脈の構文を\n\
         壊している。BRACE_SHAPES / Slot::fillers を直す）:\n{}",
        table_errors.len(),
        table_errors.join("\n")
    );
    eprintln!(
        "突き合わせ（{bash}）: {} 行・波括弧 {} 個（3.2 だけが展開 {} / 意図どおり展開 {} / 展開しない {}）・\
         またぐ対の行 {tangled_lines}",
        cases.len(),
        tally.iter().sum::<usize>(),
        tally[0],
        tally[1],
        tally[2]
    );
    assert!(
        tally.iter().all(|&n| n > 0),
        "表の検出力が無い（3.2 だけが展開 / 意図どおり展開 / 展開しない = {tally:?}）。\n\
         どれかが 0 なら語の組み方か show / f の定義が壊れている"
    );
    // またぐ対の行は開きしか見ないので、写しが壊れてそれを乱発すると突き合わせが空になる
    assert!(
        tangled_lines * 10 <= cases.len(),
        "写しがクォートや別の波括弧をまたぐ対を {tangled_lines} 行（全 {} 行）で読んだ。多すぎる\n\
         （その行は開きの目印しか確かめないので、突き合わせが空洞になる）",
        cases.len()
    );
    assert!(
        problems.is_empty(),
        "波括弧の写し（shell_scripts.rs の gobble32 / amble_expands32 / brace32_pairs /\n\
         top_level_unquoted / raw_words）が実の bash 3.2（{bash}）と {} 箇所で食い違う（{} 行・波括弧 {} 個のうち）。\n\
         写しを直して #1924 の番犬の読みを実物へ合わせること（.agent/conventions.md の #1924 / #1931）:\n{}",
        problems.len(),
        cases.len(),
        tally.iter().sum::<usize>(),
        problems
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn bash32の突き合わせはmacosでは飛ばさない() {
    let v32 = || Ok("3.2.57(1)-release".to_string());
    assert_eq!(bash32_gate(true, "/bin/bash", v32()), Bash32Gate::Run);
    assert_eq!(bash32_gate(false, "/bin/bash", v32()), Bash32Gate::Run);
    // macOS で 3.2 が使えなければ落とす（飛ばさない）
    let missing = || Err("No such file or directory (os error 2)".to_string());
    let v5 = || Ok("5.3.15(1)-release".to_string());
    for probe in [missing(), v5()] {
        match bash32_gate(true, "/bin/bash", probe) {
            Bash32Gate::Fail(reason) => assert!(reason.contains("飛ばさない"), "{reason}"),
            other => panic!("macOS で 3.2 が無いのに {other:?}"),
        }
    }
    // macOS 以外は理由を出して飛ばす
    match bash32_gate(false, "/bin/bash", missing()) {
        Bash32Gate::Skip(reason) => {
            assert!(reason.contains("起動できない"), "{reason}");
            assert!(reason.contains(BASH32_ENV), "{reason}");
        }
        other => panic!("macOS 以外で {other:?}"),
    }
    match bash32_gate(false, "/bin/bash", v5()) {
        Bash32Gate::Skip(reason) => assert!(reason.contains("3.2 ではない"), "{reason}"),
        other => panic!("macOS 以外で {other:?}"),
    }
}

#[test]
fn 突き合わせの表は決定的で波括弧の位置が原文と合う() {
    let a = bash32_oracle_cases();
    let b = bash32_oracle_cases();
    assert_eq!(
        a.iter().map(|c| &c.line).collect::<Vec<_>>(),
        b.iter().map(|c| &c.line).collect::<Vec<_>>(),
        "種が固定なので毎回同じ表"
    );
    for case in &a {
        let chars: Vec<char> = case.line.chars().collect();
        for br in &case.braces {
            assert_eq!(chars.get(br.pos), Some(&'{'), "{}: {br:?}", case.line);
            // 目印は `{` から始まる（`{Q1`）か `{` で終わる（`S1{`）
            let from: String = chars[br.pos..].iter().collect();
            let upto: String = chars[..=br.pos].iter().collect();
            assert!(
                from.starts_with(&br.witness) || upto.ends_with(&br.witness),
                "目印は `{{` にかかる: {}: {br:?}",
                case.line
            );
        }
    }
}

// ---------------------------------------------------------------------------
// pipefail の下の `X | grep -q` / 終了コードを見る `X | head`（Issue #1633 / #2272）
// ---------------------------------------------------------------------------

/// `set` の 1 コマンドが `-o <name>`（`set -euo pipefail` のように短い名前の束の末尾の `o` を含む）を入れるか
fn set_command_enables_long(words: &[String], name: &str) -> bool {
    if words.first().map(String::as_str) != Some("set") {
        return false;
    }
    let mut args = words[1..].iter();
    while let Some(a) = args.next() {
        let takes_name =
            a == "-o" || (a.starts_with('-') && !a.starts_with("--") && a.ends_with('o'));
        if takes_name && args.next().is_some_and(|n| n == name) {
            return true;
        }
    }
    false
}

/// `set -o pipefail`（`set -euo pipefail` / `set -o errexit -o pipefail` を含む）を宣言しているか
fn declares_pipefail(src: &str) -> bool {
    shell_commands(src)
        .iter()
        .any(|c| set_command_enables_long(&c.words, "pipefail"))
}

/// 段の語から、予約語・前置の代入・`builtin` / `command` を外したコマンド名（パスは basename）と、その添字。
/// `function name {` の形は本体の始まりまで飛ばす（`name() {` は字句解析が `()` で区切ってある）
fn stage_command_name(words: &[String]) -> Option<(usize, &str)> {
    let skip = if words.first().is_some_and(|w| w == "function") {
        2
    } else {
        0
    };
    let k = skip
        + words.iter().skip(skip).position(|w| {
            !(LEADING_KEYWORDS.contains(&w.as_str())
                || is_assignment_word(w)
                || w == "builtin"
                || w == "command")
        })?;
    let w = words[k].as_str();
    Some((k, w.rsplit('/').next().unwrap_or(w)))
}

/// grep の引数に**最初の一致で読むのをやめる**指定（`-q` / `--quiet` / `--silent`）があるか。
///
/// 引数を取る短い名前（`-e PAT` / `-f FILE` / `-m N` / `-A N` など）の値は名前として読まない
/// （`grep -e q` の `q` はパターン）。`--` の後ろはすべてオペランド。
fn grep_quits_early(args: &[String]) -> bool {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--" {
            break;
        }
        if a == "--quiet" || a == "--silent" {
            return true;
        }
        if a.starts_with("--") {
            continue;
        }
        let Some(flags) = a.strip_prefix('-') else {
            continue;
        };
        for (k, c) in flags.char_indices() {
            if c == 'q' {
                return true;
            }
            if "efmABCDd".contains(c) {
                if k + c.len_utf8() == flags.len() {
                    it.next(); // 値は次の語
                }
                break; // 値は同じ語の残り
            }
        }
    }
    false
}

/// 監査済みの例外宣言 `# tako:pipefail-ok <理由>`（違反行か直前の行。理由の無い宣言は無効）
fn pipefail_exception(lines: &[&str], line_no: usize) -> bool {
    let has = |l: &str| {
        l.split("tako:pipefail-ok")
            .nth(1)
            .is_some_and(|reason| !reason.trim().is_empty())
    };
    let at = |n: usize| n.checked_sub(1).and_then(|i| lines.get(i)).copied();
    at(line_no).is_some_and(has) || at(line_no.wrapping_sub(1)).is_some_and(has)
}

/// パイプラインの終了コードが判定に効くか（効くならその理由）。
///
/// `head` の出力は左辺が SIGPIPE で死んでも正しい（先頭の N 行は読めている）。壊れるのは
/// **終了コードだけ**なので、それを誰かが見る位置だけが問題になる:
/// 条件（`if` / `while` / `!`）・`&&` / `||` の左辺（右辺が `true` / `:` だけなら捨てる）・
/// `set -e` の下の素の文・`x=$( … )` の素の代入（代入の終了コード = 置換の終了コード）。
/// 引数の中の `$( … )`（`echo "…$(X | head -1)"`・`local x=$( … )`）と `<( … )`・`&` は誰も見ない。
fn status_matters(p: &LexPipeline, errexit: bool) -> Option<&'static str> {
    // (条件, &&・||, 素の文) のそれぞれの理由
    type Why = (&'static str, &'static str, Option<&'static str>);
    let by = |cond: bool, end: PipeEnd, why: Why| match end {
        PipeEnd::Or { discard: true } | PipeEnd::Background => None,
        _ if cond => Some(why.0),
        PipeEnd::And | PipeEnd::Or { .. } => Some(why.1),
        PipeEnd::Plain => why.2,
    };
    let own = ("条件", "&& / || の左辺", None);
    match p.kind {
        ListKind::Statement => by(
            p.cond,
            p.end,
            (own.0, own.1, errexit.then_some("set -e の下の文")),
        ),
        // 置換の中の判定（`$(X | grep -q y && echo 1)`）。置換の中は `set -e` を継がない（bash の既定）
        ListKind::CmdSub { .. } | ListKind::ProcSub if by(p.cond, p.end, own).is_some() => {
            by(p.cond, p.end, own)
        }
        // 素の文 = 置換の終了コード。素の代入の値なら、その代入のコマンドの終わり方に従う
        ListKind::CmdSub { assign: true } if p.end == PipeEnd::Plain => {
            let (cond, end) = p.outer?;
            by(
                cond,
                end,
                (
                    "x=$( … ) が条件",
                    "x=$( … ) が && / || の左辺",
                    errexit.then_some("set -e の下の x=$( … )"),
                ),
            )
        }
        _ => None,
    }
}

/// 読むのを途中でやめて抜ける読み手の種類
#[derive(Clone, Copy, PartialEq, Eq)]
enum EarlyQuit {
    /// `grep -q` / `--quiet` / `--silent`（終了コードそのものが目的 = いつも判定）
    GrepQ,
    /// `head`（出力は正しい。終了コードを誰かが見るときだけ判定に効く）
    Head,
}

/// 段が早く抜ける読み手か
fn early_quit(stage: &LexCommand) -> Option<EarlyQuit> {
    let (k, name) = stage_command_name(&stage.words)?;
    match name {
        "grep" | "egrep" | "fgrep" if grep_quits_early(&stage.words[k + 1..]) => {
            Some(EarlyQuit::GrepQ)
        }
        "head" => Some(EarlyQuit::Head),
        _ => None,
    }
}

/// 1 ファイルぶんの違反（file:line で名指しする文言）。
///
/// `grep -q` は最初の一致で、`head` は N 行（N バイト）で読むのをやめて抜ける。左辺が書き終わる前に
/// パイプが閉じると書き込みが EPIPE（または SIGPIPE 死 = 141）になり、`pipefail` がその非 0 を
/// パイプライン全体の結果にする。`grep -q` なら**含まれているのに「無い」**（`!` や
/// assert_not_contains では偽の合格）、`head` なら `set -e` の下の `x=$(X | head -1)` で
/// スクリプトが止まる・`if` の条件が偽になる。左辺の出力がパイプの容量（64 KB）を超えたとき・
/// 書き込みが割れたときだけ出るので、手元では緑・負荷下や大きい出力で赤になる（#1633 / #2272）。
fn pipefail_early_quit_violations(rel: &str, src: &str) -> Vec<String> {
    if !(declares_pipefail(src) || is_sourced_library(rel)) {
        return Vec::new();
    }
    let how = if declares_pipefail(src) {
        "pipefail を宣言"
    } else {
        "pipefail を宣言した側から source される"
    };
    // ライブラリは呼び手の `set -e` も継ぐ
    let errexit = is_sourced_library(rel) || declares_errexit(src, &shell_commands(src));
    let chars: Vec<char> = src.chars().collect();
    let lines: Vec<&str> = src.lines().collect();
    let line_of = |pos: usize| {
        chars[..pos.min(chars.len())]
            .iter()
            .filter(|&&c| c == '\n')
            .count()
            + 1
    };
    let mut lx = WordLexer::new(src);
    lx.list(false);
    let mut out = Vec::new();
    for p in &lx.pipelines {
        let quits: Vec<EarlyQuit> = p.stages[1..].iter().filter_map(early_quit).collect();
        let (what, hint) = if quits.contains(&EarlyQuit::GrepQ) {
            (
                "grep -q".to_string(),
                "out=$(X) && grep -q … <<<\"$out\" か [[ \"$out\" == *\"…\"* ]]",
            )
        } else if quits.contains(&EarlyQuit::Head) {
            let Some(why) = status_matters(p, errexit) else {
                continue;
            };
            (
                format!("head・{why}"),
                "最後まで読む sed -n '1,Np' か out=$(X) の後で切る",
            )
        } else {
            continue;
        };
        let line_no = line_of(p.stages[0].start);
        if pipefail_exception(&lines, line_no) {
            continue;
        }
        // `( … )` / `{ … }` / `while … done` が左辺のときは段の語にコマンド名が無い
        let producer = stage_command_name(&p.stages[0].words).map_or("複合コマンド", |(_, n)| n);
        out.push(format!(
            "{rel}:{line_no}: {producer} … | {what}（{how}）→ {hint}: {}",
            lines.get(line_no - 1).map_or("", |l| l.trim())
        ));
    }
    out
}

#[test]
fn pipefailの下で早く抜ける読み手へパイプしない() {
    let root = repo_root();
    let mut violations: Vec<String> = Vec::new();
    for path in shell_scripts() {
        let src = std::fs::read_to_string(&path).expect("読める .sh");
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        violations.extend(pipefail_early_quit_violations(&rel, &src));
    }
    assert!(
        violations.is_empty(),
        "`set -o pipefail` の下の `X | grep -q …` / 終了コードを見る `X | head …` は、読み手が途中で\n\
         抜けた後に左辺の書き込みが EPIPE（SIGPIPE = 141）で落ち、pipefail でパイプライン全体が非 0 になる\n\
         （含まれているのに「無い」・set -e の下の x=$(X | head -1) で止まる。左辺の出力がパイプの容量を\n\
         超えたときだけ出る間欠。#1633 / #2272）。grep -q は `out=$(X) && grep -q … <<<\"$out\"`\n\
         （固定文字列なら `[[ \"$out\" == *\"$needle\"* ]]`）、head は最後まで読む `sed -n '1,Np'` か\n\
         `out=$(X)` の後で切ること（どれも bash 3.2 で通る）\n\
         （.agent/conventions.md「シェルスクリプトは macOS 同梱の bash 3.2 で通す」節。\n\
         出力が小さいと監査して確かめた箇所だけ、その行か直前の行に `# tako:pipefail-ok <理由>` で外せる）:\n{}",
        violations.join("\n")
    );
}

#[test]
fn 番犬はpipefailの下のgrep_qを左辺を問わずfile_lineで名指しする() {
    let hits = |body: &str| {
        pipefail_early_quit_violations("scripts/x.sh", &format!("set -euo pipefail\n{body}\n"))
            .len()
    };

    // #1633 で CI を赤にした行そのもの（test-release-retry.sh の assert_contains）
    let v = pipefail_early_quit_violations(
        "scripts/x.sh",
        "set -euo pipefail\n\nf() {\n  if echo \"$haystack\" | grep -qF -- \"$needle\"; then\n    :\n  fi\n}\n",
    );
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(
        v[0].starts_with("scripts/x.sh:4: echo … | grep -q（pipefail を宣言）"),
        "{v:?}"
    );

    // 名指す形: printf・-q の位置と綴りの違い・`!`・コマンド置換の中・複数行
    assert_eq!(hits(r#"printf '%s' "$out" | grep -q 'x'"#), 1);
    assert_eq!(hits(r#"printf '%s\n' "$a" | grep -Eq '^x$'"#), 1);
    assert_eq!(hits(r#"echo "$a" | grep -q -E 'x|y'"#), 1);
    assert_eq!(hits(r#"echo "$a" | grep -qx 'x'"#), 1);
    assert_eq!(hits(r#"echo "$a" | grep --quiet x"#), 1);
    assert_eq!(hits(r#"echo "$a" | LC_ALL=C command grep -q x"#), 1);
    assert_eq!(hits(r#"echo "$a" |& grep -q x"#), 1);
    assert_eq!(hits(r#"! printf '%s' "$a" | grep -q x"#), 1);
    assert_eq!(
        hits(r#"[ "$rc" -ne 0 ] && printf '%s' "$e" | grep -q '読めない'"#),
        1
    );
    assert_eq!(
        hits(r#"check "$(printf '%s\n' "$S" | grep -qx '  2 行目' && echo 1)" "説明""#),
        1
    );
    assert_eq!(hits(r#"x=$(echo "$a" | grep -q y && echo 1)"#), 1);
    assert_eq!(hits(r#"printf '%s' "$a" | tr a b | grep -q x"#), 1); // 間の段も EPIPE で落ちる
    assert_eq!(hits("printf '%s' \"$a\" \\\n  | grep -q x"), 1);
    assert_eq!(hits("printf '%s' \"$a\" |\n  grep -q x"), 1);
    assert_eq!(hits("printf '%s' \"$a\" | # 説明\n  grep -q x"), 1);
    assert_eq!(hits("case $1 in\n  a) echo \"$x\" | grep -q y ;;\nesac"), 1);
    // 1 行の関数定義の本体（test-nightly-footprint-watch-2023.sh の assert_contains の形）
    assert_eq!(
        hits(r#"ac() { if printf '%s' "$2" | grep -qF -- "$3"; then ok; fi; }"#),
        1
    );
    assert_eq!(hits(r#"ac () { printf '%s' "$2" | grep -q x; }"#), 1);
    assert_eq!(hits(r#"function ac { printf '%s' "$2" | grep -q x; }"#), 1);
    assert_eq!(hits(r#"function ac() { echo "$2" | grep -q x; }"#), 1);
    assert_eq!(
        hits(r#"if printf '%s' "$a" | grep -q x && printf '%s' "$a" | grep -q y; then :; fi"#),
        2
    );
    // 素のヒアドキュメントの本文の `$( … )` は囲む側が展開する（= 囲む側の pipefail で動く）
    assert_eq!(hits("cat <<X\n$(echo \"$a\" | grep -q y && echo 1)\nX"), 1);

    // 左辺が echo / printf 以外でも同じ（#2272。#2272 の棚卸しで直した形そのもの）
    let v = pipefail_early_quit_violations(
        "scripts/x.sh",
        "set -euo pipefail\nif TAKO_SELF_TEST=1 \"$APP/tako-app\" | grep -q \"OK\"; then :; fi\n",
    );
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(
        v[0].starts_with("scripts/x.sh:2: tako-app\" … | grep -q（pipefail を宣言）"),
        "{v:?}"
    );
    assert_eq!(hits(r#"cat f | grep -q x"#), 1);
    assert_eq!(hits(r#"echo "$(cat f | grep -q x)""#), 1);
    assert_eq!(hits(r#""$TAKO" list | grep -q '"split"' || fail "x""#), 1);
    assert_eq!(hits(r#"screen_has() { cap "$1" | grep -q -- "$2"; }"#), 1);
    assert_eq!(
        hits(r#"ioreg -d1 | grep -A1 Locked | grep -q '<true/>'"#),
        1
    );
    assert_eq!(hits(r#"x | head -1 | grep -q y"#), 1); // head と grep -q で 1 件
    let group = pipefail_early_quit_violations(
        "scripts/x.sh",
        "set -euo pipefail\nok=\"$( (c a; c b) | grep -q true && echo 1)\"\n",
    );
    assert_eq!(group.len(), 1, "{group:?}");
    assert!(
        group[0].starts_with("scripts/x.sh:2: 複合コマンド … | grep -q"),
        "{group:?}"
    );
    assert_eq!(hits("{ a; b; } | grep -q x"), 1);
    assert_eq!(hits("while read -r l; do :; done < f | grep -q x"), 1);

    // 名指さない形（過大申告しない）
    assert_eq!(hits(r#"grep -qF -- "$n" <<<"$h""#), 0); // 直したあとの形
    assert_eq!(hits(r#"[[ "$h" == *"$n"* ]]"#), 0);
    assert_eq!(hits(r#"echo "$a" | grep -c x"#), 0); // 最後まで読む
    assert_eq!(hits(r#"echo "$a" | grep -e q"#), 0); // `q` は -e の値
    assert_eq!(hits(r#"echo "$a" | grep -eq"#), 0);
    assert_eq!(hits(r#"echo "$a" | grep -- -q"#), 0); // `--` の後ろはオペランド
    assert_eq!(hits(r#"echo "$a" | grep -m1q x"#), 0); // `1q` は -m の値
    assert_eq!(hits(r#"echo "$a" || grep -q x f"#), 0); // `||` はパイプではない
    assert_eq!(hits(r#"echo "$a" && grep -q x f"#), 0);
    assert_eq!(hits("echo \"$a\"\ngrep -q x f"), 0);
    assert_eq!(hits(r#"echo "$a | grep -q x""#), 0); // 文字列の中
    assert_eq!(hits(r#"# echo "$a" | grep -q x"#), 0); // コメント
    assert_eq!(
        hits("cat > stub <<X\nif printf '%s' \"\\$*\" | grep -q y; then :; fi\nX"),
        0
    ); // 別スクリプトの本文
    assert_eq!(hits("cat > stub <<'X'\necho \"$a\" | grep -q y\nX"), 0);
    // `x=(…)` の要素・腕のパターンは段の語ではない
    assert_eq!(hits("a=(echo x)\ncase $1 in echo) grep -q x f ;; esac"), 0);

    // pipefail を宣言しないファイルは対象外（終了コードは grep のものになる）
    assert!(
        pipefail_early_quit_violations("scripts/x.sh", "set -eu\necho \"$a\" | grep -q x\n")
            .is_empty()
    );
    // ライブラリは自分で宣言しなくても対象（呼び手の pipefail を継ぐ）
    let lib =
        pipefail_early_quit_violations("scripts/lib/z.sh", "f() {\n  echo \"$a\" | grep -q x\n}\n");
    assert_eq!(lib.len(), 1, "{lib:?}");
    assert!(lib[0].starts_with(
        "scripts/lib/z.sh:2: echo … | grep -q（pipefail を宣言した側から source される）"
    ));

    // 例外宣言は理由つきのときだけ効く
    let bad = "echo \"$a\" | grep -q x";
    assert_eq!(
        hits(&format!(
            "# tako:pipefail-ok 左辺は 1 バイトで割れない\n{bad}"
        )),
        0
    );
    assert_eq!(hits(&format!("# tako:pipefail-ok\n{bad}")), 1);

    // 閉じていないクォート・末尾の `\` でも panic しない
    for broken in [
        "echo \"$(printf x | grep -q",
        "echo x |",
        "printf x \\",
        "a=(x 'y",
    ] {
        let _ = pipefail_early_quit_violations("scripts/lib/x.sh", broken);
    }
}

#[test]
fn 番犬はpipefailの下で終了コードを見るheadだけを名指しする() {
    let with = |set: &str, body: &str| {
        pipefail_early_quit_violations("scripts/x.sh", &format!("{set}\n{body}\n"))
    };
    let e = |body: &str| with("set -euo pipefail", body).len();
    let no_e = |body: &str| with("set -uo pipefail", body).len();

    // #2272 で直した形そのもの（nightly-release.sh の最新タグ: タグが増えると git が書き終わる前に head が抜ける）
    let v = with(
        "set -euo pipefail",
        "LATEST_TAG=$(git tag --list 'v*' --sort=-v:refname | head -1)",
    );
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(
        v[0].starts_with("scripts/x.sh:2: git … | head・set -e の下の x=$( … )（pipefail を宣言）"),
        "{v:?}"
    );

    // 名指す形 = 終了コードを誰かが見る
    assert_eq!(e(r#"x="$(X | head -1)""#), 1);
    assert_eq!(e("a=1 b=$(X | head -1)"), 1);
    assert_eq!(e("local x; x=$(X | head -1)"), 1);
    assert_eq!(e("X | head -1"), 1); // set -e の下の素の文
    assert_eq!(e("f() { X | sed -n 's/a/b/p' | head -1; }"), 1); // 関数の終了コード
    assert_eq!(e("X | head -5 | while read -r l; do :; done"), 1);
    assert_eq!(no_e("if X | head -1; then :; fi"), 1);
    assert_eq!(no_e("while ! X | head -1; do :; done"), 1);
    assert_eq!(no_e("if x=$(X | head -1); then :; fi"), 1);
    assert_eq!(no_e("x=$(X | head -1) && y"), 1);
    assert_eq!(no_e("x=$(X | head -1) || return 1"), 1);
    assert_eq!(no_e("X | head -1 || fail x"), 1);
    assert_eq!(
        no_e(r#"echo "$([ -x c ] && c --version | head -1 || echo 無い)""#),
        1
    );
    let cond = with("set -uo pipefail", "if x=$(X | head -1); then :; fi");
    assert!(cond[0].contains("| head・x=$( … ) が条件"), "{cond:?}");

    // 名指さない形（過大申告しない）: 出力は正しく、終了コードを誰も見ない
    assert_eq!(e("x=$(X | sed -n 1p)"), 0); // 直したあとの形（最後まで読む）
    assert_eq!(no_e("x=$(X | head -1)"), 0); // set -e が無ければ代入の終了コードは捨てられる
    assert_eq!(no_e("X | head -1"), 0);
    assert_eq!(e("x=$(X | head -1) || true"), 0);
    assert_eq!(e("x=$(X | head -1) || :"), 0);
    assert_eq!(e("x=$(X | head -1 || true)"), 0);
    assert_eq!(e("X | head -1 || true"), 0);
    assert_eq!(e("X | head -1 &"), 0);
    assert_eq!(e("local x=$(X | head -1)"), 0); // local の終了コード
    assert_eq!(e(r#"echo "失敗: $(X | head -c 300)""#), 0); // 引数の中の置換
    assert_eq!(
        e(r#"fail "x: $(grep -o 'FAILED: .*' "$log" | head -1)""#),
        0
    );
    assert_eq!(e(r#"[ "$(X | head -1)" = y ]"#), 0);
    assert_eq!(e(r#"case "$(X | head -1)" in y) : ;; esac"#), 0);
    assert_eq!(e(r#"x="$(f "$(X | head -1)")""#), 0); // 置換の中の引数の置換
    assert_eq!(e("while read -r l; do :; done < <(X | head -5)"), 0);
    assert_eq!(e("head -1 f | grep x"), 0); // head が先頭の段（前に死ぬ段が無い）
    assert_eq!(e("y=$(cat <<X\n$(X | head -1)\nX\n)"), 0); // ヒアドキュメントの本文の置換は代入の値ではない
    assert_eq!(e("cat > stub <<'X'\nx=$(X | head -1)\nX"), 0); // 別スクリプトの本文

    // ライブラリは呼び手の set -e を継ぐ
    let lib = pipefail_early_quit_violations("scripts/lib/z.sh", "f() {\n  r=$(X | head -1)\n}\n");
    assert_eq!(lib.len(), 1, "{lib:?}");
    assert!(lib[0].starts_with(
        "scripts/lib/z.sh:2: X … | head・set -e の下の x=$( … )（pipefail を宣言した側から source される）"
    ));

    // 例外宣言は理由つきのときだけ効く
    assert_eq!(
        e("# tako:pipefail-ok 左辺は 1 行で 1 度に書く\nx=$(X | head -1)"),
        0
    );
    assert_eq!(e("# tako:pipefail-ok\nx=$(X | head -1)"), 1);
}

#[test]
fn pipefailの宣言を読み分ける() {
    assert!(declares_pipefail("set -euo pipefail\n"));
    assert!(declares_pipefail("set -o pipefail\n"));
    assert!(declares_pipefail("  set -eo pipefail\n"));
    assert!(declares_pipefail("set -Eeuo pipefail\n"));
    assert!(declares_pipefail("set -o errexit -o pipefail\n"));
    assert!(declares_pipefail("set -e -o pipefail\n"));
    assert!(!declares_pipefail("set -eu\n"));
    assert!(!declares_pipefail("set -o errexit\n"));
    assert!(!declares_pipefail("set +o pipefail\n"));
    assert!(!declares_pipefail("echo 'set -o pipefail'\n"));
    assert!(!declares_pipefail("# set -euo pipefail\n"));
    assert!(!declares_pipefail("cat <<'X'\nset -euo pipefail\nX\n"));
}
