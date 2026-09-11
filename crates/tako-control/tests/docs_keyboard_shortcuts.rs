//! docs のショートカット表とキーバインド表の一致を見る番犬（Issue #1323）。
//!
//! **何を止めるか**: `docs/src/content/docs/guides/keyboard-shortcuts.md` と
//! `crates/tako-app/src/keybindings.rs` が食い違うこと。#1323 の時点で docs は
//! 全 8 表が macOS のキーだけで書かれており、`non_macos_bindings()` の 45 本は
//! **1 つも載っていなかった**（Windows 利用者がキーを知る手段が docs 上に無い）。
//!
//! 手で書いた表は必ずずれる。ずれ方は 2 方向あり、どちらも実害が違う:
//!
//! - **載っていない**（バインドはあるのに docs に無い）= 機能を発見できない
//! - **載っているのに無い**（docs にあるのにバインドが無い）= 書いてあるとおりに
//!   押しても効かない。とくに Windows 列へ `cmd-` 由来の打鍵を書くと、Win キーは
//!   OS が奪うので**別のことが起きる**（#585 / #1203）
//!
//! そこで**両方向**を検査する。
//!
//! # なぜ tako-control に置くか
//!
//! 正本の `keybindings.rs` は tako-app（GPUI）にあるが、`bindings_for` は非公開で、
//! docs の突き合わせに GPUI のビルドを要求したくない。`ui_key_notation.rs` と同じく
//! **ソースを読む**形にして、docs 系の番犬の置き場（ここ）へ揃える。
//!
//! バインド表そのものの不変条件（張るキーの集合・案内文との一致）は tako-app 側の
//! `keybindings::tests` が持つ。ここが見るのは **docs との一致だけ**。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// リポジトリルート（`crates/tako-control` から 2 つ上）
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

const DOC: &str = "docs/src/content/docs/guides/keyboard-shortcuts.md";
const SRC: &str = "crates/tako-app/src/keybindings.rs";

/// 修飾キーとして扱うトークン
const MODIFIERS: &[&str] = &["cmd", "ctrl", "alt", "shift"];

/// **docs にだけ載ってよい打鍵**。キーバインド表に無い理由を必ず添える。
///
/// ここを増やすときは「なぜ `keybindings.rs` に無いのか」を書く。
/// 書けないなら、それは docs 側の間違い（= この番犬が止めたいもの）
const DOC_ONLY: &[(&str, &str)] = &[
    (
        "right",
        "入力予測の確定。GPUI のキーバインドではなく端末入力の処理側が見るので \
         keybindings.rs には現れない",
    ),
    (
        "tab",
        "入力予測の確定（→ と同じ）。同じ理由でキーバインド表には無い",
    ),
];

/// 打鍵の正規形。修飾を並べ替えて `alt+cmd+left` のような 1 本の文字列にする
/// （`cmd-alt-left` と `alt-cmd-left` を同じものとして比べるため）
fn canon(mods: &BTreeSet<String>, key: &str) -> String {
    let mut parts: Vec<String> = mods.iter().cloned().collect();
    parts.push(key.to_string());
    parts.join("+")
}

/// `keybindings.rs` の spec 文字列（`ctrl-shift-d` / `cmd--` 等）を正規形へ。
///
/// 先頭から既知の修飾を剥がし、**残り全部**をキーとして扱う
/// （`cmd--` の残りは `-`、`shift-insert` の残りは `insert`）
fn canon_spec(spec: &str) -> String {
    let mut rest = spec;
    let mut mods = BTreeSet::new();
    while let Some((m, tail)) = MODIFIERS
        .iter()
        .find_map(|m| rest.strip_prefix(&format!("{m}-")).map(|tail| (*m, tail)))
    {
        mods.insert(m.to_string());
        rest = tail;
    }
    assert!(!rest.is_empty(), "キーの無い spec: {spec}");
    canon(&mods, rest)
}

/// 指定した関数の本体から `KeyBinding::new("<spec>", …)` の spec を拾う
fn specs_in_fn(src: &str, name: &str) -> Vec<String> {
    let head = format!("\nfn {name}() -> Vec<KeyBinding> {{\n");
    let start = src
        .find(&head)
        .unwrap_or_else(|| panic!("{SRC} に {name}() が見つからない（構造が変わった？）"));
    let body = &src[start + head.len()..];
    let end = body
        .find("\n}\n")
        .unwrap_or_else(|| panic!("{name}() の終わりが見つからない"));
    let specs: Vec<String> = body[..end]
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("KeyBinding::new(\"")?;
            Some(rest.split('"').next()?.to_string())
        })
        .collect();
    assert!(!specs.is_empty(), "{name}() から 1 本も拾えていない");
    specs
}

/// `<kbd>X</kbd>` の中身を出現順に拾う
fn kbd_tokens(cell: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = cell;
    while let Some(i) = rest.find("<kbd>") {
        rest = &rest[i + "<kbd>".len()..];
        let Some(j) = rest.find("</kbd>") else { break };
        out.push(rest[..j].trim().to_string());
        rest = &rest[j + "</kbd>".len()..];
    }
    out
}

/// docs の表記（`Cmd` / `←` / `F11`）をキーバインドの語彙へ寄せる
fn token(t: &str) -> String {
    match t {
        "\u{2190}" => "left",
        "\u{2192}" => "right",
        "\u{2191}" => "up",
        "\u{2193}" => "down",
        _ => return t.to_lowercase(),
    }
    .to_string()
}

/// 1 本ぶんの打鍵（`<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd>`）を正規形へ。
/// `<kbd>` が無いセル（`（macOS のみ）`）は `None`
fn parse_one(s: &str) -> Option<(BTreeSet<String>, String)> {
    let tokens: Vec<String> = kbd_tokens(s).iter().map(|t| token(t)).collect();
    let (key, mods) = tokens.split_last()?;
    let mods: BTreeSet<String> = mods.iter().cloned().collect();
    assert!(
        mods.iter().all(|m| MODIFIERS.contains(&m.as_str())),
        "修飾でないものが修飾の位置にある: {s}"
    );
    assert!(
        !MODIFIERS.contains(&key.as_str()),
        "キーの位置に修飾だけが置かれている: {s}"
    );
    Some((mods, key.clone()))
}

/// セル 1 つから打鍵を拾う。`/` 区切りは「どれでも同じ」、`〜` は数字の範囲
fn parse_cell(cell: &str) -> Vec<String> {
    if let Some((lhs, rhs)) = cell.split_once('\u{301c}') {
        let (mods, from) = parse_one(lhs).unwrap_or_else(|| panic!("範囲の左辺が読めない: {cell}"));
        let (extra, to) = parse_one(rhs).unwrap_or_else(|| panic!("範囲の右辺が読めない: {cell}"));
        assert!(extra.is_empty(), "範囲の右辺に修飾がある: {cell}");
        let (from, to) = (
            from.parse::<u32>().expect("範囲の左辺が数字でない"),
            to.parse::<u32>().expect("範囲の右辺が数字でない"),
        );
        return (from..=to).map(|n| canon(&mods, &n.to_string())).collect();
    }
    cell.split(" / ")
        .filter_map(parse_one)
        .map(|(mods, key)| canon(&mods, &key))
        .collect()
}

/// docs の 3 列表（`| 操作 | macOS | Windows |`）から両列の打鍵を拾う
fn doc_specs(md: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut mac, mut win) = (BTreeSet::new(), BTreeSet::new());
    let mut in_table = false;
    let mut tables = 0usize;
    for line in md.lines() {
        let t = line.trim();
        if !t.starts_with('|') {
            in_table = false;
            continue;
        }
        let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
        if cells == ["操作", "macOS", "Windows"] {
            in_table = true;
            tables += 1;
            continue;
        }
        if !in_table {
            continue;
        }
        if cells
            .iter()
            .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':'))
        {
            continue; // 区切り行
        }
        assert_eq!(cells.len(), 3, "3 列表の行が 3 セルでない: {t}");
        mac.extend(parse_cell(cells[1]));
        win.extend(parse_cell(cells[2]));
    }
    assert!(tables >= 8, "3 列のショートカット表が {tables} 個しか無い");
    (mac, win)
}

/// docs / keybindings.rs を読んで (macOS の期待, Windows の期待, docs の macOS 列, docs の Windows 列)
fn sets() -> (
    BTreeSet<String>,
    BTreeSet<String>,
    BTreeSet<String>,
    BTreeSet<String>,
) {
    let root = repo_root();
    let src = std::fs::read_to_string(root.join(SRC)).expect("keybindings.rs を読めない");
    let md = std::fs::read_to_string(root.join(DOC)).expect("keyboard-shortcuts.md を読めない");

    let base = specs_in_fn(&src, "base_bindings");
    let macos_only = specs_in_fn(&src, "macos_only_bindings");
    let non_macos = specs_in_fn(&src, "non_macos_bindings");

    let want_mac: BTreeSet<String> = base
        .iter()
        .chain(macos_only.iter())
        .map(|s| canon_spec(s))
        .collect();
    // Windows は `non_macos_bindings()` だけ。`base_bindings()` の `cmd-` は
    // Windows では Win キーへ解決されて OS に奪われるため、案内に出してはいけない
    // （`keybindings::shortcut_hint_for` が platform 修飾のバインドを落とすのと同じ規則）
    let want_win: BTreeSet<String> = non_macos.iter().map(|s| canon_spec(s)).collect();

    let (doc_mac, doc_win) = doc_specs(&md);
    (want_mac, want_win, doc_mac, doc_win)
}

/// docs にだけ載ってよいものを除いた差分を人が読める形にする
fn report(label: &str, want: &BTreeSet<String>, got: &BTreeSet<String>) -> Vec<String> {
    let allowed: BTreeSet<&str> = DOC_ONLY.iter().map(|(k, _)| *k).collect();
    let mut problems = Vec::new();
    for m in want.difference(got) {
        problems.push(format!(
            "{label}: {m} が keybindings.rs にあるのに docs に無い"
        ));
    }
    for e in got.difference(want) {
        if allowed.contains(e.as_str()) {
            continue;
        }
        problems.push(format!(
            "{label}: {e} が docs にあるのに keybindings.rs に無い（押しても効かない）"
        ));
    }
    problems
}

#[test]
fn docsのmacos列はバインド表と一致する() {
    let (want, _, got, _) = sets();
    let problems = report("macOS", &want, &got);
    assert!(
        problems.is_empty(),
        "docs のショートカット表が keybindings.rs とずれている:\n{}",
        problems.join("\n")
    );
}

#[test]
fn docsのwindows列はバインド表と一致する() {
    let (_, want, _, got) = sets();
    let problems = report("Windows", &want, &got);
    assert!(
        problems.is_empty(),
        "docs のショートカット表が keybindings.rs とずれている:\n{}",
        problems.join("\n")
    );
}

/// #585 / #1203: Windows 列へ `cmd-` 由来の打鍵を書くと、Win キーは OS が奪うので
/// 「書いてあるとおりに押すと別のことが起きる」。集合の一致でも落ちるが、
/// **何が起きるか**を名指しできるよう独立させる
#[test]
fn docsのwindows列にcmd由来の打鍵が無い() {
    let (_, _, _, got) = sets();
    let offenders: Vec<&String> = got.iter().filter(|s| s.contains("cmd")).collect();
    assert!(
        offenders.is_empty(),
        "Windows 列に Cmd の打鍵が載っている（Windows では Win キーになり OS に奪われる）: {offenders:?}"
    );
}

// ---- コピー行の説明と実装の一致（Issue #1349） ----
//
// #1323 の時点の docs は「選択テキストをコピー（**選択なしの場合は Ctrl+C を
// ペインへ送信**）」と書いていたが、実装（`copy_selection`）は選択が無ければ
// 何もしない。隔離 GUI（tako-vd）の実測でも、走っている `sleep` は生き残り、
// 入力途中の行も消えず、クリップボードも変わらなかった（同じ経路で撃った
// **本物の Ctrl+C** は sleep を殺したので、観測に検出力はある）。
//
// キー集合の一致を見る上の 3 本は**説明文を見ない**ので、この種の食い違いは
// すり抜ける。そこで「ペインへ送ると書いてあるか」と「実装が送るか」を
// **両方向**で突き合わせる。

const APP_SRC: &str = "crates/tako-app/src/main.rs";

/// 「ペインへ打鍵を送る」と読める言い回し。#1349 の誤記はこの形だった
const SEND_CLAIM_PHRASES: &[&str] = &[
    "ペインへ送信",
    "ペインへ送る",
    "ペインに送信",
    "ペインに送る",
    "C をペイン",
];

/// `fn <name>(` の本体を波括弧の対応で切り出す。返すのは (1 始まりの行番号, 本体)
fn fn_body(src: &str, name: &str) -> (usize, String) {
    let head = format!("fn {name}(");
    let at = src
        .find(&head)
        .unwrap_or_else(|| panic!("{APP_SRC} に {name}( が見つからない（構造が変わった？）"));
    // 行番号は改行の数で数える（`lines().count()` はインデントの有無で 1 ずれる）
    let line = src[..at].matches('\n').count() + 1;
    assert!(
        src.lines().nth(line - 1).is_some_and(|l| l.contains(&head)),
        "行番号の計算がずれている（{APP_SRC}:{line} に {head} が無い）"
    );
    let open = at + src[at..].find('{').expect("本体の { が無い");
    let mut depth = 0usize;
    for (i, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return (line, src[open + 1..open + i].to_string());
                }
            }
            _ => {}
        }
    }
    panic!("{name}() の本体が閉じていない");
}

/// `copy_selection` の本体のうち**ペイン（PTY）へバイトを書く**行。
/// クリップボードへの書き込み（`cx.write_to_clipboard`）は PTY ではないので外す
fn pane_write_lines(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .filter(|l| {
            let l = l.replace("write_to_clipboard", "");
            l.contains(".write(")
                || l.contains("0x03")
                || l.contains("\\x03")
                || l.contains("send_input")
        })
        .map(str::to_string)
        .collect()
}

/// docs の「選択テキストをコピー」の表の行（行番号, 操作セル）。
/// **無い / 複数あるのも FAILED**（行ごと消えたのに黙って通るのを避ける）
fn copy_row(md: &str) -> (usize, String) {
    let rows: Vec<(usize, String)> = md
        .lines()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with('|') && l.contains("選択テキストをコピー"))
        .map(|(i, l)| {
            let cells: Vec<&str> = l
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            (i + 1, cells[0].to_string())
        })
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "{DOC} の「選択テキストをコピー」の行が {} 本ある（1 本であること。\
         文言を変える・行を消すなら実装との一致も見直す）",
        rows.len()
    );
    rows.into_iter().next().expect("1 本ある")
}

/// Issue #1349: コピー行の説明が `copy_selection` の実装と食い違わないこと。
///
/// 実装が送らないのに docs が「送る」と書いていたら落とし、逆に実装が
/// 送るようになったのに docs が黙っていても落とす（= 挙動を iTerm2 流へ
/// 寄せるなら、同じコミットでこの行を直すことになる）
#[test]
fn コピー行の説明がcopy_selectionの実装と一致する() {
    let root = repo_root();
    let md = std::fs::read_to_string(root.join(DOC)).expect("keyboard-shortcuts.md を読めない");
    let app = std::fs::read_to_string(root.join(APP_SRC)).expect("main.rs を読めない");

    let (fn_line, body) = fn_body(&app, "copy_selection");
    let writes = pane_write_lines(&body);
    let impl_sends = !writes.is_empty();

    let (row_line, op) = copy_row(&md);
    let claims: Vec<&&str> = SEND_CLAIM_PHRASES
        .iter()
        .filter(|p| op.contains(**p))
        .collect();
    let doc_claims = !claims.is_empty();

    assert_eq!(
        doc_claims,
        impl_sends,
        "docs のコピー行と実装が食い違っている\n\
         - docs（{DOC}:{row_line}）: {} → {op}\n\
         - 実装（{APP_SRC}:{fn_line} の copy_selection）: {}\n\
         直し方は 2 通り: docs を実態へ寄せる（選択が無いときは何も起きない）か、\
         実装を docs へ寄せて（選択なしなら Ctrl+C を送る）同じコミットで行も直す",
        if doc_claims {
            format!("ペインへ送ると書いてある（{claims:?}）")
        } else {
            "ペインへ送るとは書いていない".to_string()
        },
        if impl_sends {
            format!("ペインへ書いている（{writes:?}）")
        } else {
            "ペインへは何も書いていない".to_string()
        }
    );

    // 表のセル以外（注記・本文）に同じ言い回しが残っていないか。
    // 実装が送らないときだけ見る（送るようになったら書いてあって当然）
    if !impl_sends {
        let elsewhere: Vec<String> = md
            .lines()
            .enumerate()
            .filter(|(i, _)| i + 1 != row_line)
            .flat_map(|(i, l)| {
                SEND_CLAIM_PHRASES
                    .iter()
                    .filter(move |p| l.contains(**p))
                    .map(move |p| format!("{DOC}:{}: 「{p}」", i + 1))
            })
            .collect();
        assert!(
            elsewhere.is_empty(),
            "コピーの打鍵はペインへ何も送らないのに、表の外にそう読める記述が残っている:\n{}",
            elsewhere.join("\n")
        );
    }
}

/// docs の注記「中断の <kbd>Ctrl</kbd>+<kbd>C</kbd> は tako が横取りしない」が
/// 本当であること。素の `ctrl-c` をバインドへ足すと注記が嘘になる
/// （そのうえ端末の中断が効かなくなる。`keybindings.rs` 側の
/// `端末へ流すべきキーを奪っていない` は非 macOS ビルドだけで走るので、
/// ここは**どの OS でも**ソースを読んで見る）
#[test]
fn 素のctrl_cはバインドされていない() {
    let root = repo_root();
    let src = std::fs::read_to_string(root.join(SRC)).expect("keybindings.rs を読めない");
    let bound: BTreeSet<String> = ["base_bindings", "macos_only_bindings", "non_macos_bindings"]
        .iter()
        .flat_map(|f| specs_in_fn(&src, f))
        .map(|s| canon_spec(&s))
        .collect();
    assert!(
        !bound.contains("ctrl+c"),
        "素の Ctrl+C がバインドされた（docs は「中断の Ctrl+C はそのままペインへ届く」と \
         書いている。バインドするなら {DOC} の注記も直す）"
    );
    // コピーの打鍵そのものは在ること（行ごと消えたのに気づけるように）
    for spec in ["cmd+c", "ctrl+shift+c"] {
        assert!(
            bound.contains(spec),
            "{spec} のバインドが無い（docs のコピー行と食い違う）"
        );
    }
}

// ---- 全選択行の説明と実装の一致（Issue #1362） ----
//
// #1323 の時点の docs は「全選択」とだけ書いており、どこに効くかを言っていなかった。
// 実装（`select_all_text`）が見るのは **プレビューの編集バッファ → チャット本文 →
// プレビュー本文**の 3 つだけで、素のターミナルペインでは早期 return する。
//
// 隔離 GUI（tako-vd）の実測でも、ターミナルペインで ⌘A → ⌘C はクリップボードを
// 変えず（sentinel のまま 3/3）、⌘A が PTY へ届くこともなかった（`abc` に ⌘A →
// `x` で `abcx`。同じ経路で撃った**本物の Ctrl+A** は行頭へ入って `xabc` になるので、
// 観測に検出力はある）。同じ経路でプレビューペインへ撃つと本文 2 行が入る（3/3）。
//
// キー集合を見る 3 本も、コピー行を見る 2 本もこの行の説明は見ないので、
// 「どこに効くか」を**両方向**で突き合わせる。

/// docs が「ターミナルには効かない」と書いていると読める言い回し
const TERMINAL_DENY_PHRASES: &[&str] = &[
    "ターミナルの画面には効かない",
    "ターミナルの画面には効きません",
    "ターミナルでは効かない",
    "ターミナルでは効きません",
];

/// docs が「ターミナルにも効く」と書いていると読める言い回し。
/// 網羅はできないので、主役は下の DENY 側（実装が対応したら**消さないと落ちる**）
const TERMINAL_CLAIM_PHRASES: &[&str] = &[
    "ターミナルの画面を全選択",
    "ターミナルの画面も全選択",
    "画面全体を全選択",
    "スクロールバックを全選択",
];

/// `select_all_text` の本体のうち**端末（PTY / TerminalSession）に触る**行。
/// プレビュー・チャットの選択（`preview_selections` / `select_all_chat`）は端末ではない
fn terminal_touch_lines(body: &str) -> Vec<String> {
    const MARKERS: &[&str] = &[
        "focused_session",
        "session",
        "terminal",
        "scrollback",
        "start_selection",
        "extend_selection",
    ];
    body.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .filter(|l| {
            let lower = l.to_lowercase();
            MARKERS.iter().any(|m| lower.contains(m))
        })
        .map(str::to_string)
        .collect()
}

/// docs の「全選択」の表の行（行番号, 操作セル）。
/// **無い / 複数あるのも FAILED**（行ごと消えたのに黙って通るのを避ける）
fn select_all_row(md: &str) -> (usize, String) {
    let rows: Vec<(usize, String)> = md
        .lines()
        .enumerate()
        .filter(|(_, l)| l.trim_start().starts_with('|') && l.contains("全選択"))
        .map(|(i, l)| {
            let cells: Vec<&str> = l
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            (i + 1, cells[0].to_string())
        })
        .collect();
    assert_eq!(
        rows.len(),
        1,
        "{DOC} の「全選択」の行が {} 本ある（1 本であること。\
         文言を変える・行を消すなら実装との一致も見直す）",
        rows.len()
    );
    rows.into_iter().next().expect("1 本ある")
}

/// 全選択について docs が語っている本文 = 表の行 + 見出しに「全選択」を含む `:::note` の中身。
/// 「プレビュー」「チャット」はこの文書のあちこちに出るので、**この範囲だけ**を見る
fn select_all_prose(md: &str) -> String {
    let (_, row) = select_all_row(md);
    let mut prose = row;
    let mut in_note = false;
    for line in md.lines() {
        if in_note {
            if line.trim() == ":::" {
                in_note = false;
                continue;
            }
            prose.push('\n');
            prose.push_str(line);
            continue;
        }
        if line.starts_with(":::note[") && line.contains("全選択") {
            in_note = true;
        }
    }
    prose
}

/// Issue #1362: 全選択行の説明が `select_all_text` の実装と食い違わないこと。
///
/// 実装が端末に触らないのに docs が「ターミナルにも効く」と読めたら落とし、
/// 逆に実装が端末へ対応したのに docs が「効かない」と言い続けていても落とす
/// （= ターミナルの全選択を実装するなら、同じコミットでこの行と注記を直すことになる）
#[test]
fn 全選択行の説明がselect_all_textの実装と一致する() {
    let root = repo_root();
    let md = std::fs::read_to_string(root.join(DOC)).expect("keyboard-shortcuts.md を読めない");
    let app = std::fs::read_to_string(root.join(APP_SRC)).expect("main.rs を読めない");

    let (fn_line, body) = fn_body(&app, "select_all_text");
    let touches = terminal_touch_lines(&body);
    let impl_covers_terminal = !touches.is_empty();

    let (row_line, op) = select_all_row(&md);
    let prose = select_all_prose(&md);
    let denies: Vec<&&str> = TERMINAL_DENY_PHRASES
        .iter()
        .filter(|p| prose.contains(**p))
        .collect();
    let doc_denies = !denies.is_empty();

    assert_eq!(
        doc_denies,
        !impl_covers_terminal,
        "docs の全選択行と実装が食い違っている\n\
         - docs（{DOC}:{row_line}）: {} → {op}\n\
         - 実装（{APP_SRC}:{fn_line} の select_all_text）: {}\n\
         直し方は 2 通り: docs を実態へ寄せる（ターミナルの画面には効かない）か、\
         実装を docs へ寄せて（端末の画面 / スクロールバックを選択する）同じコミットで行も直す",
        if doc_denies {
            format!("ターミナルには効かないと書いてある（{denies:?}）")
        } else {
            "ターミナルには効かないとは書いていない".to_string()
        },
        if impl_covers_terminal {
            format!("端末に触っている（{touches:?}）")
        } else {
            "端末には触っていない（プレビュー / チャットのみ）".to_string()
        }
    );

    // 表の行だけを読む人が誤解しないこと。注記を読まないと分からない状態にしない
    if !impl_covers_terminal {
        assert!(
            op.contains("ターミナル"),
            "{DOC}:{row_line} の全選択行が「ターミナル」に触れていない（{op}）。\
             表だけ見た人が押しても効かないので、行の側にも効かない先を書く"
        );
    }

    // 「ターミナルにも効く」と読める記述が文書のどこにも残っていないこと
    let claims: Vec<String> = md
        .lines()
        .enumerate()
        .flat_map(|(i, l)| {
            TERMINAL_CLAIM_PHRASES
                .iter()
                .filter(move |p| l.contains(**p))
                .map(move |p| format!("{DOC}:{}: 「{p}」", i + 1))
        })
        .collect();
    assert_eq!(
        !claims.is_empty(),
        impl_covers_terminal,
        "全選択がターミナルに効くという記述と実装が食い違っている:\n{}",
        if claims.is_empty() {
            "（記述なし。実装は端末に触っているので docs へ書く）".to_string()
        } else {
            claims.join("\n")
        }
    );
}

/// Issue #1362: 「どこに効くか」の列挙が実装と一致すること。
///
/// プレビュー（`preview_*`）とチャット（`select_all_chat`）は実装が持っている経路なので、
/// docs から落ちたら落とす。逆に実装から経路が消えたのに docs が残っていても落とす
#[test]
fn 全選択が効く先の列挙がselect_all_textの実装と一致する() {
    let root = repo_root();
    let md = std::fs::read_to_string(root.join(DOC)).expect("keyboard-shortcuts.md を読めない");
    let app = std::fs::read_to_string(root.join(APP_SRC)).expect("main.rs を読めない");

    let (fn_line, body) = fn_body(&app, "select_all_text");
    let prose = select_all_prose(&md);

    for (label, marker, word) in [
        ("プレビュー", "preview", "プレビュー"),
        ("チャット", "select_all_chat", "チャット"),
    ] {
        let impl_has = body
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("//"))
            .any(|l| l.contains(marker));
        let doc_has = prose.contains(word);
        assert_eq!(
            doc_has,
            impl_has,
            "全選択の効く先（{label}）で docs と実装が食い違っている\n\
             - docs（{DOC} の全選択行 + 注記）: {}\n\
             - 実装（{APP_SRC}:{fn_line} の select_all_text）: {}",
            if doc_has {
                "書いてある"
            } else {
                "書いていない"
            },
            if impl_has {
                format!("{marker} を見ている")
            } else {
                format!("{marker} を見ていない")
            }
        );
    }
}
