//! シェルスクリプトの静的検査（番犬）
//!
//! `bash -n` では見つからず、その行が実行されるまで潜伏する種類の欠陥を
//! **CI で落とす**ための検査。`.agent/conventions.md` の
//! 「シェルスクリプトで日本語を出すときの変数展開（Issue #837）」と
//! 「シェルスクリプトは macOS 同梱の bash 3.2 で通す（Issue #1499 / #1518）」に
//! 書いてある規約の機械化。

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
