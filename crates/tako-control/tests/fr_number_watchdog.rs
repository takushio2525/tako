//! `.agent/requirements.md` の要件番号の一意性と、コードからの参照の実在を見張る番犬
//! （Issue #1336。再発防止の対象は #1319）。
//!
//! # なぜ要るか
//!
//! #1319 では `FR-2.34` が 2 セクション（#1068 / #1067）、`FR-3.17` / `FR-3.18` が
//! 各 2 件あった。実害は番号から仕様を引く側に出る: コードのコメントが 12 か所で
//! `FR-3.18` を **Code Runner** の意味で使っているのに、`.agent/requirements.md` を
//! `FR-3.18` で引くと先に別要件（コンフリクトカード）へ当たる。原因は別々の worker が
//! 同時期に「末尾の次の番号」を振ったことで、レビューで 480 個超の番号の衝突には
//! 気づけない。だから CI で落とす。
//!
//! # 検査は 2 本立て（片方だけでは穴が残る）
//!
//! 1. [`要件番号は一意`] — 定義の重複を落とす。同じ番号が 2 か所で定義されていると、
//!    引く側（コード・docs・AI）がどちらに当たるかは並び順任せになる
//! 2. [`コードが参照する要件番号は実在する`] — 参照切れを落とす。番号を振り直したときに
//!    コード側の追従が漏れると、1 と同じ「引くと別物 / 何も無い」状態になる
//!
//! # 見逃す側へ倒れないための作り
//!
//! 数え違いが「見逃す」側へ倒れる検査は番犬にならない（`.agent/conventions.md`）ので、
//! 検出力と走査の健全性を同じファイルの中で固定する。
//!
//! - [`重複を名指しできる`] — #1319 の修正前と同じ重複を**現行テキストから作り直して**
//!   3 系統すべてが名指しされることを確かめる
//! - [`参照切れを名指しできる`] — 実在する番号を定義集合から 1 つ落とし、それを
//!   参照している箇所が名指しされることを確かめる
//! - [`走査が空振りしていない`] — 定義の 3 形（章・節・表）と 1〜3 段のそれぞれが
//!   実際に採れていることを確かめる。パーサが壊れて 0 件になれば 1 も 2 も無意味に
//!   緑になるので、そこを塞ぐ
//!
//! # 1 段の番号（`FR-5` / `NFR-8`）も定義として数える
//!
//! 章見出し `## FR-5` と表 ID `| NFR-8 |` は 1 段で、Issue #1336 の文面
//! （`^### (FR|NFR)-\d+\.\d+`）の外にある。**2 段以上だけを見ると 1 段の重複を
//! 見逃す**ので拾う。1 段を定義として持つと、コードにある 1 段の参照（`FR-5` が 20 件、
//! `NFR-8` が 6 件）も実在チェックへ載せられる = 章番号への言及を偽の参照切れに
//! しないで済む、という利点もある。
//!
//! # このファイルへ架空の番号を書かない
//!
//! 走査対象は `crates/**/*.rs` なので**この番犬自身も検査される**。requirements.md に
//! 無い番号を例として置くと、[`コードが参照する要件番号は実在する`] がそれを参照切れとして
//! 拾う（実装中に 2 度拾われた: 単体テストの例と、この注意書きに書いた例の両方）。
//! 例外リストで自分を外すと、このファイルの本物の参照切れも見逃すことになるので、
//! **例はすべて実在する番号から採る**。
//! `NFR-` の多段が実在しないぶんは、接頭辞の分岐（`NFR-8`）と多段の分解
//! （`FR-2.34.1`）を別々に検査して同じコードパスを通す。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read_requirements() -> String {
    let p = repo_root().join(".agent/requirements.md");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{} が読めない: {e}", p.display()))
}

/// 文字列の先頭が要件番号（`FR-5` / `FR-2.34` / `FR-2.34.1` / `NFR-8`）ならその範囲を返す。
///
/// 末尾のドットは番号に含めない（`FR-3.2。` の句点や行末の `FR-3.2.` で別番号へ
/// 化けないように）。段の数は 1 以上（1 段 = 章番号・`NFR-8` のような表 ID）。
fn leading_id(s: &str) -> Option<&str> {
    let prefix_len = if s.starts_with("NFR-") {
        "NFR-".len()
    } else if s.starts_with("FR-") {
        "FR-".len()
    } else {
        return None;
    };
    let rest = &s[prefix_len..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    let num = rest[..end].trim_end_matches('.');
    if num.is_empty() || num.split('.').any(|seg| seg.is_empty()) {
        return None;
    }
    Some(&s[..prefix_len + num.len()])
}

/// 要件番号の**定義**の形は 3 つだけ。
///
/// | 形 | 例 | 段 |
/// |---|---|---|
/// | 章見出し | `## FR-5 セッション永続性` | 1 |
/// | 節見出し | `### FR-2.34 Claude 公式 Remote Control …` | 2 |
/// | 表の ID 列 | `\| FR-2.34.1 \| **プロファイル …** \| M \| ✅ \|` | 1〜3 |
///
/// 本文中の言及（「表示先は FR-2.7.2」等）は定義ではないので拾わない。表の ID 列は
/// **セルが番号ちょうど**のものだけを定義とする（説明が続くセルは本文）。
fn definition_id(line: &str) -> Option<&str> {
    // `### ` を先に見る（`## ` は `### ` 行には前方一致しないが、意図を並び順で示す）
    for prefix in ["### ", "## "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return leading_id(rest);
        }
    }
    let rest = line.strip_prefix('|')?;
    let cell = rest.split('|').next()?.trim();
    let id = leading_id(cell)?;
    (id.len() == cell.len()).then_some(id)
}

/// 定義された番号 → 現れた行番号（1 始まり）
fn collect_definitions(text: &str) -> BTreeMap<String, Vec<usize>> {
    let mut out: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        if let Some(id) = definition_id(line) {
            out.entry(id.to_string()).or_default().push(i + 1);
        }
    }
    out
}

/// 2 か所以上で定義されている番号を、行番号つきで並べる。
fn duplicates(text: &str) -> Vec<String> {
    collect_definitions(text)
        .into_iter()
        .filter(|(_, lines)| lines.len() > 1)
        .map(|(id, lines)| {
            let at: Vec<String> = lines.iter().map(usize::to_string).collect();
            format!(
                "  {id} が {} か所で定義されている（.agent/requirements.md:{}）",
                lines.len(),
                at.join(" / :")
            )
        })
        .collect()
}

/// 行に現れる要件番号をすべて拾う（コメント・文字列リテラルを問わない）。
fn ids_in(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        if !line.is_char_boundary(i) {
            i += 1;
            continue;
        }
        let rest = &line[i..];
        // 直前が英数字・ハイフンなら別の語の一部（`XFR-1.2`）なので拾わない
        let at_boundary = line[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '-');
        if at_boundary {
            if let Some(id) = leading_id(rest) {
                out.push(id);
                i += id.len();
                continue;
            }
        }
        i += 1;
    }
    out
}

/// `crates/**/*.rs` を読む（`target` 配下は見ない）。
fn rust_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if name == "target" || name == "node_modules" {
                    continue;
                }
                walk(&p, root, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                if let Ok(text) = std::fs::read_to_string(&p) {
                    let rel = p
                        .strip_prefix(root)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push((rel, text));
                }
            }
        }
    }
    let root = repo_root();
    let mut out = Vec::new();
    walk(&root.join("crates"), &root, &mut out);
    out.sort();
    out
}

/// コードからの参照を `(ファイル, 行番号, 番号)` で集める。
fn code_references() -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    for (rel, text) in rust_sources() {
        for (i, line) in text.lines().enumerate() {
            for id in ids_in(line) {
                out.push((rel.clone(), i + 1, id.to_string()));
            }
        }
    }
    out
}

/// 定義が無い番号を参照している箇所を並べる。
fn dangling(refs: &[(String, usize, String)], defined: &BTreeSet<String>) -> Vec<String> {
    let mut out: Vec<String> = refs
        .iter()
        .filter(|(_, _, id)| !defined.contains(id))
        .map(|(rel, line, id)| format!("  {rel}:{line} が {id} を参照しているが定義が無い"))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn defined_ids(text: &str) -> BTreeSet<String> {
    collect_definitions(text).into_keys().collect()
}

#[test]
fn 要件番号は一意() {
    let dups = duplicates(&read_requirements());
    assert!(
        dups.is_empty(),
        ".agent/requirements.md で同じ要件番号が 2 か所以上で定義されている\n{}\n\
         直し方: **コード・docs から参照されている側の番号は動かさず**、参照が無い側へ\n\
         未使用の番号を振る（参照数は `grep -rn '<番号>' crates/ docs/ .agent/` で数える）。\n\
         振り直したら本文中の相互参照も追従させる（#1319 の実例: FR-3.18 = Code Runner は\n\
         コード 12 か所が参照していたので不動、#496 の 2 行を FR-3.25 / FR-3.26 へ動かした）",
        dups.join("\n")
    );
}

#[test]
fn コードが参照する要件番号は実在する() {
    let text = read_requirements();
    let found = dangling(&code_references(), &defined_ids(&text));
    assert!(
        found.is_empty(),
        "コードが .agent/requirements.md に無い要件番号を参照している\n{}\n\
         直し方: 番号を振り直したなら参照側を追従させる。要件そのものが無いなら\n\
         requirements.md へ定義を足す（番号だけのコメントは仕様の参照にならない）",
        found.join("\n")
    );
}

/// #1319 の修正前（`84c16c7^`）と同じ重複を現行テキストから作り直し、3 系統すべてを
/// 名指しできることを確かめる。
///
/// 当時の 364 KB を fixture として置く代わりに**逆置換**で作る。#1319 の変更は番号
/// 14 行だけで本文は 1 文字も変えていない（PR #1327 で機械確認済み）ので、置換後は
/// 当時と同値。置換が実際に効いたことと重複ができたことをこのテスト自身が確かめるので、
/// 将来 requirements.md 側からこれらの番号が消えても**空振りで緑にはならない**。
#[test]
fn 重複を名指しできる() {
    let text = read_requirements();
    // #1319 の振り直しを巻き戻す（FR-2.38 は配下の FR-2.38.1〜.10 も前方一致で戻る）
    let before = text
        .replace("FR-2.38", "FR-2.34")
        .replace("| FR-3.25 |", "| FR-3.17 |")
        .replace("| FR-3.26 |", "| FR-3.18 |");
    // `assert_ne!` は落ちたときに 364 KB を 2 本吐いて診断が読めなくなるので使わない
    assert!(
        before != text,
        "逆置換が 1 文字も効いていない（A/B の前提が崩れている）。\
         現行 .agent/requirements.md に FR-2.38 / FR-3.25 / FR-3.26 が在るか確認する"
    );

    let dups = duplicates(&before);
    let report = dups.join("\n");
    for id in ["FR-2.34", "FR-3.17", "FR-3.18"] {
        assert!(
            dups.iter().any(|line| line.contains(&format!(" {id} が"))),
            "#1319 の重複 {id} を名指しできていない:\n{report}"
        );
    }
    // 行番号まで出る（どこを直せばいいか分かる形で落ちる）
    assert!(
        report.contains(".agent/requirements.md:"),
        "重複の場所を行番号で名指しする:\n{report}"
    );
    assert!(
        duplicates(&text).is_empty(),
        "現行 main は重複なし（振り直し後の状態）"
    );
}

/// 実在する番号を定義集合から 1 つ落とし、それを参照している箇所が名指しされることを
/// 確かめる。
///
/// 偽の番号をこのファイルへ書くと**この番犬自身が参照切れとして拾う**ので、
/// 定義側を削る形にしてある（例外リストを作らずに検出力を示す）。
#[test]
fn 参照切れを名指しできる() {
    let text = read_requirements();
    let defined = defined_ids(&text);
    let refs = code_references();
    assert!(!refs.is_empty(), "コードから要件番号の参照が採れている");

    let victim = refs
        .iter()
        .map(|(_, _, id)| id.clone())
        .find(|id| defined.contains(id))
        .expect("実在する番号を参照している箇所がある");
    let mut narrowed = defined.clone();
    narrowed.remove(&victim);

    let found = dangling(&refs, &narrowed);
    assert!(
        found.iter().any(|line| line.contains(&victim)),
        "定義を落とした {victim} の参照切れを名指しできていない"
    );
    assert!(
        found.iter().any(|line| line.contains(".rs:")),
        "参照切れは ファイル:行 で名指しする:\n{}",
        found.join("\n")
    );
    assert!(
        dangling(&refs, &defined).is_empty(),
        "現行 main は参照切れなし"
    );
}

#[test]
fn 走査が空振りしていない() {
    let text = read_requirements();
    let defs = collect_definitions(&text);
    // 定義の 3 形 × 段の数がそれぞれ採れている（どれか 1 つでも壊れれば上の 2 本が
    // 無意味に緑になる）
    for (id, shape) in [
        ("FR-1", "章見出し `## FR-1`（1 段）"),
        ("NFR-8", "表の ID 列（1 段）"),
        ("FR-2.34", "節見出し `### FR-2.34`（2 段）"),
        ("FR-3.18", "表の ID 列（2 段）"),
        ("FR-2.34.1", "表の ID 列（3 段）"),
    ] {
        assert!(defs.contains_key(id), "{shape} を採れていない（{id}）");
    }
    assert!(
        defs.len() >= 400,
        "定義が {} 件しか採れていない（実測 482 / 2026-09-11）。パーサか見出しの書式が変わった",
        defs.len()
    );

    let refs = code_references();
    assert!(
        refs.len() >= 400,
        "コードの参照が {} 件しか採れていない（実測 613 / 2026-09-11）",
        refs.len()
    );
    // 本文中の言及を定義として拾っていない（拾うと重複検査が誤爆する）
    assert_eq!(
        definition_id("表示先は FR-2.7.2（AI 成果物プレゼンテーション）"),
        None,
        "本文中の言及は定義ではない"
    );
    assert_eq!(
        definition_id("| FR-3.18 の spawn_command_pane 経路を再利用 | M |"),
        None,
        "説明が続くセルは ID 列ではない"
    );
}

#[test]
fn 番号の切り出しは末尾のドットと語境界を見る() {
    // 例は実在する番号だけを使う（理由はこのファイルの冒頭）。`NFR-` の接頭辞の分岐と
    // 多段の分解は別々に検査して同じコードパスを通す
    assert_eq!(leading_id("FR-2.34 タイトル"), Some("FR-2.34"));
    assert_eq!(leading_id("FR-2.34.1 タイトル"), Some("FR-2.34.1"));
    assert_eq!(leading_id("NFR-8 の目標値"), Some("NFR-8"));
    assert_eq!(leading_id("FR-5 セッション永続性"), Some("FR-5"));
    // 文末の句点・ピリオドは番号に含めない
    assert_eq!(leading_id("FR-3.2。"), Some("FR-3.2"));
    assert_eq!(leading_id("FR-3.2."), Some("FR-3.2"));
    // 形が崩れたものは番号として扱わない
    assert_eq!(leading_id("FR-.2"), None);
    assert_eq!(leading_id("FR-"), None);
    assert_eq!(leading_id("ID"), None);
    // 行中の拾い方: 範囲表記の後半（`3.3`）は接頭辞が無いので拾わない
    assert_eq!(ids_in("FR-3.1〜3.3 実装メモ"), vec!["FR-3.1"]);
    assert_eq!(
        ids_in("Code Runner (FR-3.18, #453) は FR-2.22.4 を再利用"),
        vec!["FR-3.18", "FR-2.22.4"]
    );
    // `NFR-` を `FR-` として二重に拾わない
    assert_eq!(ids_in("NFR-8 の目標値"), vec!["NFR-8"]);
    // 語の一部は拾わない
    assert!(ids_in("XFR-1.2").is_empty());
}
