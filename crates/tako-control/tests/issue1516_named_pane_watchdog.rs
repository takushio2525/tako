//! 名指しされたペインについての問いの番犬（#1516）
//!
//! # なぜ要るか
//!
//! `tako orchestrator self` の要求には**種類の違う 2 つの問い**が載る。
//!
//! - 「私は誰か」（引数なし）= 呼び出し元の手掛かり（env の `TAKO_PANE_ID` /
//!   `TAKO_ORCHESTRATOR_ROLE` / 自プロセスの pid）を載せて受け手に解かせる
//! - 「そのペインは何か」（`--pane N` / MCP `pane`）= 名指し
//!
//! CLI と MCP はこの 2 つを**同じ `pane` 欄へ混ぜて**いた（`pane.or_else(caller_pane)`）。
//! 受け手は「`pane` は stale になりうる env 由来」という契約どおり pid 祖先辿りを
//! 先に引くので、名指しは**毎回黙って無視され**、呼び出し元のペインが返っていた
//! （#1516 の実測: pane 1954 から `--pane 1964` → `pane_id: 1954`）。
//!
//! 壊れ方が静かなのが厄介で、応答は**正しい形をしている**（別 master の状態を
//! 見たつもりで自分の状態を読む）。加えて名指しが解けないときに role 検索へ落ちると、
//! たまたま既定 role で動いている**無関係な master**を答える（#1466 と同じ事故）。
//!
//! # 何を縛るか
//!
//! 1. 入口（CLI / MCP mapper）が `Request::OrchestratorSelf` を**直接組まない**
//!    （正本 `Request::orchestrator_self` を通る = 混ぜ方が 2 か所へ割れない）
//! 2. 正本が、名指しが呼び出し元と別なら `caller_role` / `caller_pid` を**落とす**
//! 3. 正本が、自分を名指しした場合は手掛かりを**残す**（solo の profile は env の
//!    `solo:<名前>` にしか無く、ペインのラベルからは解けない）
//! 4. `resolve_caller_pane` が名指しを**既定へ落とさない**（role 検索より前に
//!    `named_pane` を見て `PaneNotFound` を返す）
//! 5. `self` の応答の `role` が、名指しのとき対象ペインの role ラベルを載せる
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜5 はすべて無意味に緑になるので、[`走査が空振りしていない`]で
//! 窓が採れていることを固定し、[`逆戻りを名指しできる`]で**修正前を再現した 6 通りの注入**が
//! file:line で名指しされることを確かめる。範囲取りは #1420 の 1 実装を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const PROTOCOL: &str = "crates/tako-control/src/protocol.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const MAPPER: &str = "crates/tako-control/src/mcp/request.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";

/// 正本（組み立てを 1 本に閉じる入口）
const SOURCE_OF_TRUTH: &str = "Request::orchestrator_self";

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

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 本番コードだけの眺め（テスト領域は空白へ潰す。行番号は保たれる = #1420）
fn production(rel: &'static str, src: &str) -> String {
    // dispatch.rs / main.rs はテストが厚い（後半が丸ごと `#[cfg(test)]`）
    production_range::production_with_floor(src, rel, 0.20)
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。終わりは宣言行と同じ字下げの `}`
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

/// 注釈を落とした窓（アンチパターンを説明した注釈で落ちないようにする）
fn code_only(window: &str) -> String {
    window
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 指定した関数の窓の中でだけ文字列を置き換える（注入用）。
/// ファイル全体を置換すると無関係な実装まで壊れて「何を測ったか」が濁る
fn replace_in_fn(src: &str, needle: &str, from: &str, to: &str) -> String {
    let Some((at, window)) = fn_window(src, needle) else {
        return src.to_string();
    };
    let lines: Vec<&str> = src.lines().collect();
    let start = at - 1;
    let end = start + window.lines().count();
    let mut out: Vec<String> = lines[..start].iter().map(|s| s.to_string()).collect();
    out.push(window.replace(from, to));
    out.extend(lines[end..].iter().map(|s| s.to_string()));
    out.join("\n")
}

/// 1: 入口が正本を通る（`Request::OrchestratorSelf {` を直接組まない）
fn scan_entry_points(cli: &str, mapper: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, src) in [(CLI, cli), (MAPPER, mapper)] {
        let mut direct = Vec::new();
        for (i, line) in src.lines().enumerate() {
            if line.contains("Request::OrchestratorSelf {") {
                direct.push(i + 1);
            }
        }
        for line in direct {
            out.push(Offender {
                file,
                line,
                why: format!(
                    "`Request::OrchestratorSelf` を直接組んでいる。名指しと呼び出し元の \
                     手掛かりの混ぜ方が 2 か所へ割れるので `{SOURCE_OF_TRUTH}` を通す（#1516）"
                ),
            });
        }
        if !src.contains(&format!("{SOURCE_OF_TRUTH}(")) {
            out.push(Offender {
                file,
                line: 0,
                why: format!(
                    "`{SOURCE_OF_TRUTH}` を呼んでいない（走査が空振りか、入口が正本を外れた）"
                ),
            });
        }
    }
    out
}

/// 2 / 3: 正本が名指しで手掛かりを落とし、自分の名指しでは残す
fn scan_source_of_truth(protocol: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: PROTOCOL,
            line,
            why,
        })
    };
    match fn_window(protocol, "pub fn orchestrator_self_with(") {
        None => push(
            0,
            "正本 `orchestrator_self_with` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("named != caller_pane") {
                push(
                    at,
                    "「名指しが呼び出し元自身か」の判定が無い。自分を名指しした solo の \
                     profile は env の `solo:<名前>` にしか無いので、手掛かりを落とすと \
                     既定へ落ちる（#1516）"
                        .into(),
                );
            }
            // 名指しの枝で手掛かりを落としていること（両方 None）
            let drops = code.contains("caller_role: None") && code.contains("caller_pid: None");
            if !drops {
                push(
                    at,
                    "名指しの枝で `caller_role` / `caller_pid` を落としていない。載せると \
                     受け手が pid 祖先辿りを先に引き、呼び出し元のペインを答える（#1516 の症状）"
                        .into(),
                );
            }
            if !code.contains("named.or(caller_pane)") {
                push(
                    at,
                    "名指しが無いときに env 由来の呼び出し元 pane へ落ちる枝が無い \
                     （引数なしの `tako orchestrator self` が解けなくなる）"
                        .into(),
                );
            }
            if !code.contains("legacy") {
                push(
                    at,
                    "A/B（`TAKO_1516_LEGACY=1`）の腕が無い。同一バイナリで症状を再現できないと \
                     「直った」を実測で示せない"
                        .into(),
                );
            }
        }
    }
    out
}

/// 4 / 5: dispatch が名指しを既定へ落とさず、応答へ対象ペインの role を載せる
fn scan_dispatch(dispatch: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: DISPATCH,
            line,
            why,
        })
    };

    match fn_window(dispatch, "fn named_pane(") {
        None => push(
            0,
            "`named_pane`（名指しの判定）が無い（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !(code.contains("caller_role.is_none()") && code.contains("caller_pid.is_none()")) {
                push(
                    at,
                    "名指しの判定が「呼び出し元の手掛かりが 1 つも載っていない」で書かれていない \
                     （正本が作る形と食い違うと、名指しが呼び出し元解決へ落ちる）"
                        .into(),
                );
            }
        }
    }

    match fn_window(dispatch, "fn resolve_caller_pane(") {
        None => push(0, "`resolve_caller_pane` が無い（走査が空振り）".into()),
        Some((at, window)) => {
            let code = code_only(&window);
            let named_at = code.find("named_pane(");
            let role_at = code.find("find_master_pane_strict(");
            match (named_at, role_at) {
                (None, _) => push(
                    at,
                    "名指しの判定（`named_pane`）を見ていない。解けない名指しが role 検索へ落ちて \
                     「たまたま既定 role で動いている無関係な master」を黙って答える（#1466）"
                        .into(),
                ),
                (Some(_), None) => push(
                    at,
                    "role 検索（`find_master_pane_strict`）が消えた = 引数なしの呼び出し元解決が \
                     壊れている（#288）"
                        .into(),
                ),
                (Some(n), Some(r)) if n > r => push(
                    at,
                    "名指しの判定が role 検索より後ろにある = 名指しが既定へ落ちてから見ている \
                     （順序が逆）"
                        .into(),
                ),
                _ => {}
            }
            if !code.contains("PaneNotFound") {
                push(
                    at,
                    "解けない名指しを失敗として返していない（`DispatchError::PaneNotFound`）。\
                     宛先が解けないものを既定へ落とさない = #1466"
                        .into(),
                );
            }
        }
    }

    match fn_window(dispatch, "fn dispatch_orchestrator_self(") {
        None => push(
            0,
            "`dispatch_orchestrator_self` が無い（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("caller_role.or(pane_role.as_deref())") {
                push(
                    at,
                    "応答の `role` が呼び出し元の名乗りだけを見ている。名指しでは名乗りが \
                     載らないので、対象ペインの role ラベルへ落ちる形にする（#1516）"
                        .into(),
                );
            }
            if !code.contains("named") {
                push(
                    at,
                    "名指しかどうかを見ていない = 対象が master / solo でなくても理由を出せない \
                     （既定へ落ちたことが読み手に見えない。#1466）"
                        .into(),
                );
            }
        }
    }
    out
}

fn scan_all(protocol: &str, dispatch: &str, mapper: &str, cli: &str) -> Vec<Offender> {
    let mut out = scan_entry_points(cli, mapper);
    out.extend(scan_source_of_truth(protocol));
    out.extend(scan_dispatch(dispatch));
    out
}

fn sources() -> (String, String, String, String) {
    (
        production(PROTOCOL, &read(PROTOCOL)),
        production(DISPATCH, &read(DISPATCH)),
        production(MAPPER, &read(MAPPER)),
        production(CLI, &read(CLI)),
    )
}

#[test]
fn 名指しされたペインの問いが呼び出し元へすり替わらない() {
    let (protocol, dispatch, mapper, cli) = sources();
    let offenders = scan_all(&protocol, &dispatch, &mapper, &cli);
    assert!(
        offenders.is_empty(),
        "#1516 の逆戻り:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn 走査が空振りしていない() {
    let (protocol, dispatch, mapper, cli) = sources();
    // 窓が採れていること（採れないと 1〜5 はすべて無意味に緑になる）
    for (rel, src, needle) in [
        (PROTOCOL, &protocol, "pub fn orchestrator_self_with("),
        (DISPATCH, &dispatch, "fn named_pane("),
        (DISPATCH, &dispatch, "fn resolve_caller_pane("),
        (DISPATCH, &dispatch, "fn dispatch_orchestrator_self("),
    ] {
        let (at, window) = fn_window(src, needle)
            .unwrap_or_else(|| panic!("{rel} に `{needle}` の窓が無い（走査が空振り）"));
        assert!(
            at > 0 && window.lines().count() > 3,
            "{rel} の窓が薄い: {at}"
        );
    }
    // 入口の本番範囲が残っていること（潰しすぎると「直接組んでいない」が空振りで緑）
    for (rel, src) in [(MAPPER, &mapper), (CLI, &cli)] {
        assert!(
            src.contains("tako_orchestrator_self") || src.contains("OrchestratorCommand::SelfInfo"),
            "{rel} の本番範囲から `self` の入口が消えている（走査が空振り）"
        );
    }
}

#[test]
fn 逆戻りを名指しできる() {
    let (protocol, dispatch, mapper, cli) = sources();

    // 注入 1: 入口（CLI）が正本を通さず直接組む形（= #1516 の修正前）
    let cli_direct = cli.replace(
        "send_request(Request::orchestrator_self(",
        "send_request(Request::OrchestratorSelf {\n                pane,\n                caller_role,\n                caller_pid: Some(std::process::id()),\n            }); let _ = (",
    );
    // 注入 2: MCP mapper が直接組む形
    let mapper_direct = mapper.replace(
        "Request::orchestrator_self(",
        "Request::OrchestratorSelf {\n            pane: None, caller_role: None, caller_pid: None,\n        }; let _ = (",
    );
    // 注入 3: 正本が名指しでも手掛かりを落とさない（混ぜる旧挙動）
    let protocol_merged = replace_in_fn(
        &protocol,
        "pub fn orchestrator_self_with(",
        "let names_other = named.is_some() && named != caller_pane;",
        "let names_other = false;",
    );
    // 注入 4: dispatch が名指しを role 検索へ落とす（既定の master を黙って答える）
    let dispatch_fallback = replace_in_fn(
        &dispatch,
        "fn resolve_caller_pane(",
        "if let Some(raw) = named_pane(pane, caller_role, caller_pid) {",
        "if let Some(raw) = None::<u64> {",
    );
    // 注入 5: 応答の `role` が呼び出し元の名乗りだけを見る
    let dispatch_role = replace_in_fn(
        &dispatch,
        "fn dispatch_orchestrator_self(",
        "\"role\": caller_role.or(pane_role.as_deref()),",
        "\"role\": caller_role,",
    );
    // 注入 6: 名指しの判定が「pane が載っているか」だけになる（正本の形を見ない）
    let dispatch_loose = replace_in_fn(
        &dispatch,
        "fn named_pane(",
        "pane.filter(|_| caller_role.is_none() && caller_pid.is_none())",
        "pane",
    );

    let cases: Vec<(&str, Vec<Offender>)> = vec![
        (
            "1: CLI が直接組む",
            scan_all(&protocol, &dispatch, &mapper, &cli_direct),
        ),
        (
            "2: MCP mapper が直接組む",
            scan_all(&protocol, &dispatch, &mapper_direct, &cli),
        ),
        (
            "3: 正本が手掛かりを混ぜる",
            scan_all(&protocol_merged, &dispatch, &mapper, &cli),
        ),
        (
            "4: 名指しが role 検索へ落ちる",
            scan_all(&protocol, &dispatch_fallback, &mapper, &cli),
        ),
        (
            "5: role が呼び出し元の名乗りだけ",
            scan_all(&protocol, &dispatch_role, &mapper, &cli),
        ),
        (
            "6: 名指しの判定が緩い",
            scan_all(&protocol, &dispatch_loose, &mapper, &cli),
        ),
    ];

    for (label, offenders) in cases {
        assert!(
            !offenders.is_empty(),
            "注入 {label} を番犬が見逃した（検出力が無い）"
        );
        assert!(
            offenders.iter().any(|o| o.line > 0),
            "注入 {label} を file:line で名指しできていない: {:?}",
            offenders.iter().map(Offender::report).collect::<Vec<_>>()
        );
    }
}
