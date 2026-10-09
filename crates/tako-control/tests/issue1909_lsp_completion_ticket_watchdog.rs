//! 取り消しの番号を UI スレッドで先に取る形（#1909。ホバーの #1893 と同じ口）の構造の番犬
//!
//! # なぜ要るか
//!
//! 取り消し合う要求（打鍵の補完・説明の補い・マウスのホバー）は、GUI が背景へ渡してから背景が
//! 走り出すまでに隙間がある。番号を**背景で**取ると、その隙間に UI が出した取り消し（一覧を閉じた・
//! 語の外へ出た・語から外れた）を追い越して自分が最新になり、読み込みが済むまで待ち続ける
//! （サーバが答えずに待たせていれば待ちの表にも残る）。どれも**1 行で戻せる**のに、戻しても
//! 隙間は普段は数マイクロ秒なので、ふつうの e2e も手で触っても気付かない（GUI の UI は版の照合で
//! 古い答えを出さないので、見た目も変わらない）。
//!
//! 挙動は e2e（`issue1909_lsp_completion_ticket`。順序をテストの手と注入 `TAKO_1909_INJECT_HOLD` で
//! 作る）と visual-test `completion-cancel`（GUI の打鍵経路）が測り、ここは**構造**を縛って file:line で
//! 名指す（GUI を立てない CI でも落ちる）。
//!
//! # 縛ること
//!
//! - manager の背景の問い合わせは列へ `enter_lane` の 1 つの口から入り、UI の番号があればそれを使う
//! - 番号を取る（`supersede(`）のは UI スレッドの口（`reserve_*` / `cancel_*`）と `enter_lane` だけ。
//!   **それ以外の関数が取ったら**（背景で取る形を新しく足したら）その行を名指す
//! - GUI の打鍵の補完・説明の補いは番号を UI スレッドで取ってから背景へ渡す（A/B の旧腕だけ `None`）
//! - 走っている数（`inflight`）は置き換わっても消さず、抜けたら減らし、`tako lsp status` に載せる
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const MANAGER: &str = "crates/tako-control/src/lsp/manager.rs";
const GUI: &str = "crates/tako-app/src/lsp_completion_ui.rs";
const GUI_HOVER: &str = "crates/tako-app/src/lsp_hover_ui.rs";

const FILES: &[&str] = &[MANAGER, GUI, GUI_HOVER];

/// manager で番号を取って（`supersede(`）よい関数。UI スレッドの口と、背景が列へ入る唯一の口だけ
const MAY_SUPERSEDE: &[&str] = &[
    "enter_lane",
    "reserve_completion",
    "reserve_resolve",
    "reserve_hover",
    "cancel_completion",
    "cancel_hover",
];

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let pad = " ".repeat(indent);
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == format!("{pad}}}"))
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 行コメントを落とす（doc と注記に名前が出てくるので、実行されるコードだけを見る）
fn code_only(text: &str) -> String {
    text.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の行（1-based）
fn line_in(at: usize, window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| at + i)
}

/// 行が関数の宣言か（`fn 名前(` の名前を返す）
fn fn_name(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = ["pub(crate) fn ", "pub fn ", "fn "]
        .iter()
        .find_map(|head| trimmed.strip_prefix(head))?;
    let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    Some(&rest[..end])
}

/// 規則 1 つ: `file` の関数 `decl` のコードが `must` をすべて含み、`must_not` を含まない
struct Rule {
    file: &'static str,
    decl: &'static str,
    must: &'static [&'static str],
    must_not: &'static [&'static str],
    why: &'static str,
}

const RULES: &[Rule] = &[
    // --- manager: 背景は 1 つの口から列へ入る ---
    Rule {
        file: MANAGER,
        decl: "    fn enter_lane(&self, lane: Lane, reserved: Option<u64>)",
        must: &[
            ".running += 1",
            "hold_for_injection();",
            "reserved.unwrap_or_else(|| self.supersede(lane))",
        ],
        must_not: &[],
        why: "列の口が UI の番号を使っていない（背景で取り直して取り消しを追い越す）/ 走っている数を数えない / 注入の止まり場所が番号を決める前に無い",
    },
    Rule {
        file: MANAGER,
        decl: "    fn completion(&self, request: &CompletionRequest)",
        must: &["self.enter_lane(Lane::Completion, request.ticket)"],
        must_not: &["self.supersede("],
        why: "打鍵の補完が UI の番号で列へ入っていない（背景で取ると一覧を閉じた取り消しを追い越す）",
    },
    Rule {
        file: MANAGER,
        decl: "    fn resolve_completion(",
        must: &["self.enter_lane(Lane::Resolve, ticket)"],
        must_not: &["self.supersede("],
        why: "説明の補いが UI の番号で列へ入っていない（閉じた一覧の説明をサーバへ問い合わせる）",
    },
    Rule {
        file: MANAGER,
        decl: "    fn hover(&self, request: &HoverRequest)",
        must: &["self.enter_lane(Lane::Hover, request.ticket)"],
        must_not: &["self.supersede("],
        why: "マウスのホバーが UI の番号で列へ入っていない（#1893 の競合に戻る）",
    },
    // --- manager: 走っている数（`inflight`）---
    Rule {
        file: MANAGER,
        decl: "    fn supersede(&self, lane: Lane) -> u64 {",
        must: &["inflight.call.take()", "cancel_request("],
        must_not: &["Inflight {"],
        why: "置き換えで列を作り直している（走っている古い要求の数が消え、残っていても `inflight` が 0 に見える）",
    },
    Rule {
        file: MANAGER,
        decl: "impl Drop for LaneCall<'_> {",
        must: &["inflight.running = inflight.running.saturating_sub(1)"],
        must_not: &[],
        why: "問い合わせが抜けても `inflight` から外れない",
    },
    Rule {
        file: MANAGER,
        decl: "    fn status(&self, name: Option<&str>) -> Value {",
        must: &["\"inflight\": inflight"],
        must_not: &[],
        why: "`tako lsp status` に `inflight` が載らない（CLI / MCP から残りを観測できない）",
    },
    // --- GUI: 番号は UI スレッドで取ってから背景へ渡す ---
    Rule {
        file: GUI,
        decl: "fn fire_completion(",
        must: &["self.lsp.reserve_completion()", "legacy_1909()"],
        must_not: &[],
        why: "打鍵の補完の取り消しの番号を UI スレッドで先に取っていない（背景が一覧を閉じた取り消しを追い越し、読み込みが済むまで待ち続ける）",
    },
    Rule {
        file: GUI,
        decl: "fn resolve_selected_completion(",
        must: &[
            "self.lsp.reserve_resolve()",
            "manager.resolve_completion(&path, &raw, ticket)",
        ],
        must_not: &[],
        why: "説明の補いの取り消しの番号を UI スレッドで先に取っていない（閉じた一覧の説明をサーバへ問い合わせる）",
    },
    Rule {
        file: GUI_HOVER,
        decl: "fn fire_lsp_hover(",
        must: &["ticket: self.lsp.reserve_hover()"],
        must_not: &[],
        why: "マウスのホバーの取り消しの番号を UI スレッドで先に取っていない（#1893）",
    },
];

/// `file:line — 理由` の一覧（違反が無ければ空）
fn scan(sources: &[(&'static str, String)]) -> Vec<String> {
    let src_of = |file: &str| &sources.iter().find(|(f, _)| *f == file).unwrap().1;
    let mut out = Vec::new();
    for rule in RULES {
        let Some((at, window)) = fn_window(src_of(rule.file), rule.decl) else {
            out.push(format!(
                "{}:0 — `{}` が見つからない（走査が空振り）",
                rule.file, rule.decl
            ));
            continue;
        };
        let code = code_only(&window);
        for needle in rule.must {
            if !code.contains(needle) {
                out.push(format!(
                    "{}:{at} — {}（`{needle}` が無い）",
                    rule.file, rule.why
                ));
            }
        }
        for needle in rule.must_not {
            if code.contains(needle) {
                let line = line_in(at, &window, needle).unwrap_or(at);
                out.push(format!(
                    "{}:{line} — {}（`{needle}` が在る）",
                    rule.file, rule.why
                ));
            }
        }
    }
    // manager の番号の取り口: `supersede(` を呼んでよいのは UI スレッドの口と列の口だけ。ほかの関数が
    // 呼んだ = 背景で番号を取る形を足した（その行を名指す）
    let code = code_only(src_of(MANAGER));
    let lines: Vec<&str> = code.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("supersede(") || line.contains("fn supersede(") {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let owner = lines[..i]
            .iter()
            .rev()
            .filter(|l| l.len() - l.trim_start().len() < indent)
            .find_map(|l| fn_name(l));
        if !owner.is_some_and(|name| MAY_SUPERSEDE.contains(&name)) {
            out.push(format!(
                "{MANAGER}:{} — `{}` が取り消しの番号を取っている（背景で取ると UI の取り消しを追い越す。番号は UI スレッドの `reserve_*` で取り、背景は `enter_lane` で列へ入る）",
                i + 1,
                owner.unwrap_or("?")
            ));
        }
    }
    out
}

fn current() -> Vec<(&'static str, String)> {
    FILES.iter().map(|f| (*f, read(f))).collect()
}

#[test]
fn 取り消しの番号はuiスレッドで取る() {
    let reports = scan(&current());
    assert!(reports.is_empty(), "違反:\n{}", reports.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let sources = current();
    for rule in RULES {
        let src = &sources.iter().find(|(f, _)| *f == rule.file).unwrap().1;
        let (_, window) = fn_window(src, rule.decl)
            .unwrap_or_else(|| panic!("{}: `{}` の窓が採れない", rule.file, rule.decl));
        assert!(
            window.lines().count() >= 3,
            "{}: `{}` の窓が短すぎる",
            rule.file,
            rule.decl
        );
    }
    // 取り口の走査が見ている呼び出し: 許した口の数だけ在る（0 なら走査が何も見ていない）
    let manager = code_only(&read(MANAGER));
    let calls = manager
        .lines()
        .filter(|l| l.contains("supersede(") && !l.contains("fn supersede("))
        .count();
    assert!(
        calls >= MAY_SUPERSEDE.len(),
        "`supersede(` の呼び出しが {calls} 件しか見えない（走査が空振り）"
    );
}

/// 注入 1 つぶん: `from` を `to` へ置き換えたソースで、`needle` の行が名指しされること
fn assert_named(label: &str, file: &'static str, (from, to): (&str, &str), needle: &str) {
    let mut sources = current();
    let slot = sources.iter_mut().find(|(f, _)| *f == file).unwrap();
    assert!(
        slot.1.contains(from),
        "{label}: 注入元 {from:?} が現行ソースに無い"
    );
    slot.1 = slot.1.replacen(from, to, 1);
    let line = slot
        .1
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("{label}: 名指し先 {needle:?} が無い"));
    let reports = scan(&sources);
    let expected = format!("{file}:{line} ");
    let named = reports.iter().find(|r| r.starts_with(&expected));
    assert!(
        named.is_some(),
        "{label}: {expected} を名指ししていない: {reports:?}"
    );
    println!("{label}: FAILED {}", named.unwrap_or(&String::new()));
}

#[test]
fn 逆戻りを名指しできる() {
    // A. 列の口が UI の番号を捨てて背景で取り直す
    assert_named(
        "A 口が取り直す",
        MANAGER,
        (
            "        call.ticket = reserved.unwrap_or_else(|| self.supersede(lane));",
            "        call.ticket = {\n            let _ = reserved;\n            self.supersede(lane)\n        };",
        ),
        "    fn enter_lane(&self, lane: Lane, reserved: Option<u64>)",
    );
    // B. 打鍵の補完が背景で番号を取る（#1909 の前の形）
    assert_named(
        "B 補完が背景で取る",
        MANAGER,
        (
            "            .then(|| self.enter_lane(Lane::Completion, request.ticket));",
            "            .then(|| self.enter_lane(Lane::Completion, Some(self.supersede(Lane::Completion))));",
        ),
        "Some(self.supersede(Lane::Completion))",
    );
    // C. 説明の補いが UI の番号を使わない
    assert_named(
        "C 説明が UI の番号を使わない",
        MANAGER,
        (
            "self.enter_lane(Lane::Resolve, ticket)",
            "self.enter_lane(Lane::Resolve, None)",
        ),
        "    fn resolve_completion(",
    );
    // D. ホバーが UI の番号を使わない（#1893 の前の形）
    assert_named(
        "D ホバーが UI の番号を使わない",
        MANAGER,
        (
            "self.enter_lane(Lane::Hover, request.ticket)",
            "self.enter_lane(Lane::Hover, None)",
        ),
        "    fn hover(&self, request: &HoverRequest)",
    );
    // E. ほかの背景の問い合わせが番号を取る形を新しく足す（取り口の走査が名指す）
    assert_named(
        "E 背景の取り口を足す",
        MANAGER,
        (
            "    fn resolve_items(\n        &self,",
            "    fn resolve_items_1909(&self) {\n        let _ = self.supersede(Lane::Resolve);\n    }\n\n    fn resolve_items(\n        &self,",
        ),
        "let _ = self.supersede(Lane::Resolve);",
    );
    // F. 置き換えで列を作り直す（走っている数が消える）
    assert_named(
        "F 列を作り直す",
        MANAGER,
        (
            "            inflight.ticket = ticket;\n            (ticket, inflight.call.take())",
            "            let previous = std::mem::replace(inflight, Inflight { ticket, call: None, running: 0 });\n            (ticket, previous.call)",
        ),
        "std::mem::replace(inflight, Inflight {",
    );
    // G. 抜けても数から外さない
    assert_named(
        "G 数から外さない",
        MANAGER,
        (
            "        inflight.running = inflight.running.saturating_sub(1);",
            "        let _ = inflight;",
        ),
        "impl Drop for LaneCall<'_> {",
    );
    // H. status に載せない
    assert_named(
        "H status に載せない",
        MANAGER,
        ("            \"inflight\": inflight,\n", ""),
        "    fn status(&self, name: Option<&str>) -> Value {",
    );
    // I. GUI の打鍵が番号を取らずに背景へ渡す
    assert_named(
        "I GUI の打鍵が番号を取らない",
        GUI,
        (
            "                self.lsp.reserve_completion()",
            "                None",
        ),
        "fn fire_completion(",
    );
    // J. GUI の説明の補いが番号を取らずに背景へ渡す
    assert_named(
        "J GUI の説明が番号を取らない",
        GUI,
        ("            self.lsp.reserve_resolve()", "            None"),
        "fn resolve_selected_completion(",
    );
    // K. GUI のホバーが番号を取らずに背景へ渡す
    assert_named(
        "K GUI のホバーが番号を取らない",
        GUI_HOVER,
        (
            "            ticket: self.lsp.reserve_hover(),",
            "            ticket: None,",
        ),
        "fn fire_lsp_hover(",
    );
}
