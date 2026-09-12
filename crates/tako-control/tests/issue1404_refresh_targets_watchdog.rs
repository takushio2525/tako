//! ファイルツリーの**スキャン対象**が重複・亡霊を抱えていないかの番犬（#1404）
//!
//! # なぜ要るか
//!
//! `crates/tako-app/src/filetree.rs` の `refresh_targets` は、ルート列と展開中
//! ディレクトリ（`HashSet`）を連結して `Vec::dedup()` していた。`dedup` は
//! **隣り合う重複しか落とさない**のに、ルートは `set_roots` / `toggle_dir` の
//! 自動展開で**両方の器に入る**。結果、2 秒ごとのポーリングが同じディレクトリを
//! 2 回 `read_dir` していた（実測: ルート 2 本 + 展開 1 つで len=4 / uniq=3。
//! 4 なのは `HashSet` の走査順が偶然 1 組だけ隣接したから = 落とせるかが運次第）。
//!
//! 同じ関数の (b) 側は `set_roots`。ルートが外れたときルート自身しか畳まず、
//! 「配下の展開・キャッシュは refresh が掃除する」と注釈していたが**事実と違った**:
//! 掃除役の `apply_refresh` が落とすのは `scan_dirs` が `None` を返したとき
//! = ディレクトリが**実在しなくなった**ときだけ。実在する子孫は永久に残り、
//! 画面に出ていないディレクトリを読み続ける（実測: 表示行 1 / スキャン対象 2）。
//!
//! # 何を縛るか
//!
//! 1. `refresh_targets` が重複を返さない作りである（`dedup()` だけに頼らない）
//! 2. `set_roots` が外れたルートを**配下ごと**忘れる（`forget_under` を通る）
//! 3. `forget_under` は**生きているルートの下**を巻き込まない（入れ子のルート）
//! 4. 事実と違う注釈（「配下は refresh が掃除する」）が戻っていない
//! 5. セルフテスト項目 135 が**検出力のある形**で書かれている
//!
//! 5 がこの番犬の本体。項目 135 は `targets.len() > git_roots.len()` を見ていたが、
//! これは (a) の重複だけで常に真になり、「展開ディレクトリが載っているか」を
//! **何も検査していなかった**。重複を直すと同時に assert も直す必要がある =
//! 片方だけ戻ると静かに検出力が消える組み合わせなので、静的に縛る。
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜5 はすべて無意味に緑になるので、
//! [`走査が空振りしていない`] で窓が採れていることを固定し、
//! [`逆戻りを名指しできる`] で**修正前を再現した 6 通りの注入**が file:line で
//! 名指しされることを確かめる。範囲取りは #1420 の 1 実装を通す
//! （`filetree.rs` はメソッドの途中に `#[cfg(test)]` が 2 つあるので、
//! 雑に切ると `refresh_targets` が丸ごと視界から消える）。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const FILETREE: &str = "crates/tako-app/src/filetree.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";

/// 項目 135 の (e) 節の目印（ここから `);` までが窓）
const ITEM135_HEAD: &str = "// (e) git へ渡す起点はルートだけ";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。
///
/// 終わりは**宣言行と同じ字下げの `}`**（自由関数 = 0 桁 / メソッド = 4 桁）。
/// 「宣言行から N 行」で切ると隣の関数の実装を自分のものと数えてしまう（#1417 が踏んだ形）
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 目印の行から、指定の字下げの `);` までの窓（関数の中の 1 ブロックを採る）
fn block_window(src: &str, head: &str, close: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(head))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 注釈（`//` で始まる行）を落とした窓。検査は**コードだけ**を見る
/// （アンチパターンを説明した注釈で落ちると、理由を書くほど落ちることになる）
fn code_only(window: &str) -> String {
    window
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の**コードの**行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

/// ツリー側（`filetree.rs`）の検査
fn scan_filetree(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: FILETREE,
            line,
            why,
        })
    };

    // 1. スキャン対象に重複が入らない作りか
    match fn_window(src, "pub fn refresh_targets(") {
        None => push(0, "`refresh_targets` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            let dedup = code_line_with(&window, ".dedup()");
            let sorted = code_line_with(&window, ".sort()");
            if let Some(i) = dedup {
                // 並べてからの `dedup` は正しい（#1404 の直し方の候補の 1 つ）。
                // 落とすのは**並べ替えなしの** `dedup` = 隣接しか落とせない形
                if sorted.is_none_or(|s| s > i) {
                    push(
                        at + i,
                        "並べ替えずに `Vec::dedup()` している（隣り合う重複しか落ちない。\
                         `expanded` は `HashSet` = 順序が任意なので全ルートが 2 回 \
                         read_dir される。#1404）"
                            .into(),
                    );
                }
            }
            let by_set = code.contains("seen.insert(") || code.contains("HashSet");
            if !by_set && dedup.is_none() {
                push(
                    at,
                    "重複を落とす仕掛けが無い（集合で組むか、並べてから `dedup` する）".into(),
                );
            }
        }
    }

    // 2. ルートが外れたら配下ごと忘れる
    match fn_window(src, "pub fn set_roots(") {
        None => push(0, "`set_roots` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("forget_under(") {
                push(
                    at + code_line_with(&window, "self.expanded.remove(").unwrap_or(0),
                    "外れたルートの**配下**を忘れていない（`apply_refresh` が落とすのは \
                     ディレクトリが実在しなくなったときだけなので、実在する子孫は \
                     永久に読まれ続ける。#1404）"
                        .into(),
                );
            }
        }
    }

    // 3. 生きているルートの配下まで巻き込まない
    match fn_window(src, "fn forget_under(") {
        None => push(
            0,
            "`forget_under` が無い（外れたルート配下を忘れる 1 実装が消えている）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            for (needle, why) in [
                (
                    "starts_with(",
                    "配下の判定（`starts_with`）をしていない = ルート自身しか落ちない",
                ),
                (
                    "self.expanded.retain(",
                    "`expanded` から落としていない（スキャン対象に残り続ける）",
                ),
                (
                    "self.cache.retain(",
                    "`cache` から落としていない（表示しない中身を抱え続ける）",
                ),
            ] {
                if !code.contains(needle) {
                    push(at, why.into());
                }
            }
            if !code.contains("alive.iter().any(") {
                push(
                    at,
                    "生きているルートを見ていない（ルートは入れ子にできるので、外側が \
                     外れたときに内側のルート配下まで巻き込む）"
                        .into(),
                );
            }
        }
    }

    // 4. 事実と違う注釈が戻っていない
    if let Some(i) = src
        .lines()
        .position(|l| l.contains("配下の展開・キャッシュは refresh が掃除する"))
    {
        push(
            i + 1,
            "事実と違う注釈が戻っている（`apply_refresh` はディレクトリが消えたときしか \
             掃除しない。#1404 の症状 (b) の出どころ）"
                .into(),
        );
    }
    out
}

/// セルフテスト項目 135（`main.rs`）の検出力の検査
fn scan_item135(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: MAIN,
            line,
            why,
        })
    };
    let Some((at, window)) = block_window(src, ITEM135_HEAD, "                );") else {
        push(0, "項目 135 の (e) 節が見つからない（走査が空振り）".into());
        return out;
    };
    let code = code_only(&window);
    if let Some(i) = code_line_with(&window, "targets1009.len() > git_roots1009.len()") {
        push(
            at + i,
            "件数の大小だけを見ている（修正前は重複で常に真になった = 展開ディレクトリが \
             載っているかを何も検査していない。#1404）"
                .into(),
        );
    }
    if !code.contains("expand_dir(") {
        push(
            at,
            "ルートではないディレクトリを展開していない（重複が無い実装では \
             targets == roots になり、検査が何も見なくなる）"
                .into(),
        );
    }
    if !code.contains("uniq1009") || !code.contains("dedup()") {
        push(at, "スキャン対象の重複を検査していない（#1404 (a)）".into());
    }
    if !code.contains("extra1009.contains(") {
        push(
            at,
            "展開ディレクトリが載っていることを検査していない（ルート以外の 1 件を名指す）".into(),
        );
    }
    out
}

fn all(filetree: &str, main: &str) -> Vec<String> {
    scan_filetree(filetree)
        .iter()
        .chain(scan_item135(main).iter())
        .map(Offender::report)
        .collect()
}

/// 本番コードだけの眺め（`filetree.rs` はメソッドの途中に `#[cfg(test)]` があり、
/// `main.rs` はテストが厚いので下限を明示して読む）
fn sources(root: &Path) -> (String, String) {
    (
        production_range::production(&read(root, FILETREE), FILETREE),
        production_range::production(&read(root, MAIN), MAIN),
    )
}

#[test]
fn スキャン対象は重複も亡霊も抱えない() {
    let root = workspace_root();
    let (filetree, main) = sources(&root);
    let offenders = all(&filetree, &main);
    assert!(
        offenders.is_empty(),
        "ファイルツリーの背景ポーリングが無駄なディレクトリを読んでいる（#1404）。\
         同じディレクトリを 2 回・表示していないディレクトリを永久に read_dir する形へ \
         戻っている:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let (filetree, main) = sources(&root);
    for needle in [
        "pub fn refresh_targets(",
        "pub fn set_roots(",
        "fn forget_under(",
    ] {
        let (at, window) = fn_window(&filetree, needle)
            .unwrap_or_else(|| panic!("{FILETREE} の {needle} の窓が採れない（走査が空振り）"));
        assert!(at > 0, "{FILETREE} の {needle} の行番号が採れていない");
        assert!(
            window.lines().count() > 2,
            "{FILETREE} の {needle} の窓が 2 行以下（字下げの規約が変わって窓が切れている）"
        );
    }
    let (at, window) = block_window(&main, ITEM135_HEAD, "                );")
        .expect("項目 135 の (e) 節の窓が採れる");
    assert!(
        at > 0 && window.lines().count() > 10,
        "項目 135 の窓が小さい"
    );
    assert!(
        window.contains("135: git の走査起点はワークスペースルートだけ"),
        "項目 135 の窓に assert のメッセージが入っていない（窓の切り方がずれた）"
    );
    // A/B の腕は別関数に隔離してある（窓へ紛れ込むと注入の検査が無意味になる）
    assert!(
        filetree.contains("fn legacy_refresh_targets("),
        "A/B の腕（`TAKO_1404_LEGACY=1`）が消えている"
    );
    let (_, targets) = fn_window(&filetree, "pub fn refresh_targets(").expect("窓");
    assert!(
        !code_only(&targets).contains("targets.extend(self.expanded.iter().cloned());"),
        "旧アームの本体が `refresh_targets` の窓に入っている（注入の検査が壊れる）"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let (filetree, main) = sources(&root);

    // 注入 1: 修正前の組み立て（連結 + 並べ替えなしの dedup）へ戻す
    let concat = filetree.replace(
        "        if legacy_1404() {\n            return self.legacy_refresh_targets();\n        }",
        "        let mut targets: Vec<PathBuf> = self.roots.clone();\n\
         \x20       targets.extend(self.expanded.iter().cloned());\n\
         \x20       targets.dedup();\n        return targets;",
    );
    assert!(concat != filetree, "注入 1 の対象が見つからない");
    let want = concat
        .lines()
        .position(|l| l.trim() == "targets.dedup();")
        .expect("注入した行がある")
        + 1;
    let found = all(&concat, &main);
    assert!(
        found
            .iter()
            .any(|o| o.contains(&format!("{FILETREE}:{want}"))),
        "並べ替えなしの `dedup` への逆戻りを名指しできていない: {found:?}"
    );

    // 注入 2: 外れたルート自身だけ畳む（修正前の `set_roots`）
    let self_only = filetree.replace(
        "            self.forget_under(&dropped, &ordered);",
        "            for old in &dropped {\n                self.expanded.remove(old);\n            }",
    );
    assert!(self_only != filetree, "注入 2 の対象が見つからない");
    let found = all(&self_only, &main);
    assert!(
        found.iter().any(|o| o.contains(FILETREE)),
        "配下を忘れない形への逆戻りを名指しできていない: {found:?}"
    );

    // 注入 3: 生きているルートを見ない（入れ子のルートで内側まで巻き込む）
    let blind = filetree.replace(
        "                && !alive.iter().any(|root| p.starts_with(root))",
        "",
    );
    assert!(blind != filetree, "注入 3 の対象が見つからない");
    assert!(
        !all(&blind, &main).is_empty(),
        "生きているルートを見ない形が緑のまま"
    );

    // 注入 4: 事実と違う注釈を戻す
    let lying = filetree.replace(
        "        // 消えたルートの状態は**配下ごと**畳む（#1404）。",
        "        // 消えたルートの状態は畳む（配下の展開・キャッシュは refresh が掃除する）",
    );
    assert!(lying != filetree, "注入 4 の対象が見つからない");
    let want = lying
        .lines()
        .position(|l| l.contains("配下の展開・キャッシュは refresh が掃除する"))
        .expect("注入した行がある")
        + 1;
    let found = all(&lying, &main);
    assert!(
        found
            .iter()
            .any(|o| o.contains(&format!("{FILETREE}:{want}"))),
        "事実と違う注釈の復活を名指しできていない: {found:?}"
    );

    // 注入 5: 項目 135 を件数の大小（= 重複で常に真）へ戻す
    let vacuous = main.replace(
        "                    git_roots1009 == roots1009\n\
         \x20                       && uniq1009.len() == targets1009.len()",
        "                    git_roots1009 == roots1009\n\
         \x20                       && targets1009.len() > git_roots1009.len()",
    );
    assert!(vacuous != main, "注入 5 の対象が見つからない");
    let want = vacuous
        .lines()
        .position(|l| l.contains("targets1009.len() > git_roots1009.len()"))
        .expect("注入した行がある")
        + 1;
    let found = all(&filetree, &vacuous);
    assert!(
        found.iter().any(|o| o.contains(&format!("{MAIN}:{want}"))),
        "項目 135 の検出力が消えた形を名指しできていない: {found:?}"
    );

    // 注入 6: 展開ディレクトリを作らずに読む（targets == roots で何も見ない）
    let no_expand = main.replace(
        "                        app.filetree.expand_dir(&expanded1009);\n",
        "",
    );
    assert!(no_expand != main, "注入 6 の対象が見つからない");
    let found = all(&filetree, &no_expand);
    assert!(
        found.iter().any(|o| o.contains(MAIN)),
        "展開 0 件で検査する形を名指しできていない: {found:?}"
    );
}
