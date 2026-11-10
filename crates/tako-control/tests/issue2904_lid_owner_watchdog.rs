//! #2904 の番犬: macOS の蓋閉じ継続（`pmset disablesleep`）の腕が「**倒した主を確かめてから戻す**」形から
//! 外れていないことをソースの構造で固定する（どの OS の CI でも走る）。
//!
//! 振る舞いは `tests/issue2904_lid_owner_pmset.rs`（macOS の実経路・偽の `sudo` / `pmset`）と
//! `platform::lid::pmset` の単体テスト（判定の表）が見ている。ここが名指すのは次の 2 つの崩れ:
//!
//! 1. **所有者を見ずに戻す形**: `set_disablesleep(` を呼んでよいのは、記録を確かめた後の実行
//!    （`run_pmset_action`）・起動時の残留チェック（`clear_startup_residual`。隔離と他の tako の稼働の
//!    門の後ろ）・A/B の旧い腕（`legacy_2904_*`）だけ。#2904 より前の `restore_on_exit` の
//!    `if is_sudoers_installed() && sleep_disabled() { set_disablesleep(false) }` のような呼び出しが
//!    他の関数に現れたら `file:line` で名指す（`crates/*/src` の全体を見る）
//! 2. **終了時の門を外す形**: `restore_on_exit` / `apply_override` の macOS の腕は判定の 1 実装
//!    （`settle_pmset`）へ**起動時と同じ門**（`residual_gate_now` = `pmset::residual_gate` に隔離と
//!    他の tako の稼働を渡す）を渡す。起動時の残留チェック（`should_clear_residual`）も同じ
//!    `residual_gate` を通る = 門は 1 実装。判定（`pmset::decide`）は持ち主の死んだ記録の 2 つの枝で門を引く
//!
//! 見逃す側へ倒れないよう、[`注入した崩れを名指しする`] がメモリ上の注入（1 と 2 の形）を名指しすることを
//! 確かめ、[`走査が空振りしていない`] が許した呼び手の実在を確かめる

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const LID: &str = "crates/tako-control/src/platform/lid.rs";
const PMSET: &str = "crates/tako-control/src/platform/lid/pmset.rs";

/// `set_disablesleep(` を呼んでよい関数（ファイル, 関数名）
const ALLOWED_CALLERS: &[(&str, &str)] = &[
    // 記録をロックの下で確かめた後に `pmset::decide` が返した操作を実行する（#2904）
    (LID, "run_pmset_action"),
    // 起動時の残留チェック（#449。隔離・他の tako の稼働の門の後ろ）
    (LID, "clear_startup_residual"),
    // A/B の旧い腕（`TAKO_2904_LEGACY=1`）
    (LID, "legacy_2904_apply"),
    (LID, "legacy_2904_restore_on_exit"),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 本番コードの眺め（コメント・文字列を空白へ潰し、末尾の `#[cfg(test)] mod tests` から先を落とす。
/// 行番号は保たれる）。
///
/// `production_range::scan` は使わない: `#[cfg(any(target_os = "macos", test))]` の項目（`pmset::decide` /
/// `should_clear_residual`）まで「テストの領域」として潰すので、見たい本体が消える
fn product(src: &str) -> String {
    let view = production_range::code_view::code_view(src);
    match view.find("#[cfg(test)]\nmod tests {") {
        Some(at) => view[..at].to_string(),
        None => view,
    }
}

/// `offset` を含む関数の名前（直前の `fn 名前(` / `fn 名前<` の宣言）
fn enclosing_fn(view: &str, offset: usize) -> Option<String> {
    let head = &view[..offset];
    let at = head.rfind("fn ")?;
    let rest = &head[at + 3..];
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// `fn <name>` の本体（宣言行の行番号, 本文）。無ければ `None`
fn body_of(view: &str, name: &str) -> Option<(usize, String)> {
    let at = [format!("fn {name}("), format!("fn {name}<")]
        .iter()
        .filter_map(|head| view.find(head.as_str()))
        .min()?;
    let line = view[..at].matches('\n').count() + 1;
    let open = at + view[at..].find('{')?;
    let mut depth = 0usize;
    for (i, c) in view[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((line, view[open..=open + i].to_string()));
                }
            }
            _ => {}
        }
    }
    None
}

/// 崩れ 1: 許した関数の外の `set_disablesleep(` の呼び出し（`file:line: fn 名前`）と、見つけた呼び手の一覧
fn stray_setters(files: &[(String, String)]) -> (Vec<String>, Vec<(String, String)>) {
    let mut stray = Vec::new();
    let mut seen = Vec::new();
    for (rel, src) in files {
        let view = product(src);
        for (offset, _) in view.match_indices("set_disablesleep(") {
            // 定義そのもの（`fn set_disablesleep(`）は呼び出しではない
            if view[..offset].ends_with("fn ") {
                continue;
            }
            let line = view[..offset].matches('\n').count() + 1;
            let caller = enclosing_fn(&view, offset).unwrap_or_else(|| "?".into());
            if ALLOWED_CALLERS
                .iter()
                .any(|(file, name)| file == rel && *name == caller)
            {
                seen.push((rel.clone(), caller));
            } else {
                stray.push(format!(
                    "{rel}:{line}: fn {caller} が set_disablesleep を呼んでいる（記録を確かめずに \
                     pmset を打つ = #2904 の「他の主の上書きを戻す」が戻る。macOS の腕は \
                     settle_pmset → pmset::decide → run_pmset_action を通すこと）"
                ));
            }
        }
    }
    (stray, seen)
}

/// 崩れ 2: 判定の 1 実装と門の配線（違反の文の一覧）
fn broken_wiring(lid_src: &str, pmset_src: &str) -> Vec<String> {
    let lid = product(lid_src);
    let pmset = product(pmset_src);
    let mut out = Vec::new();
    let mut need = |view: &str, rel: &str, name: &str, needle: &str, times: usize, why: &str| {
        match body_of(view, name) {
            None => out.push(format!(
                "{rel}: fn {name} が無い（改名したら番犬も直すこと）"
            )),
            Some((line, body)) => {
                let got = body.matches(needle).count();
                if got < times {
                    out.push(format!(
                        "{rel}:{line}: fn {name} が `{needle}` を {times} 回通っていない（{got} 回）。{why}"
                    ));
                }
            }
        }
    };
    need(
        &lid,
        LID,
        "restore_on_exit",
        "settle_pmset(false, residual_gate_now)",
        1,
        "終了時は判定の 1 実装へ起動時と同じ門を渡す（門を外すと、検証起動の終了が前回の tako の記録を \
         理由に本番の上書きを戻す = #2904）",
    );
    need(
        &lid,
        LID,
        "apply_override",
        "settle_pmset(enable, residual_gate_now)",
        1,
        "周期（設定 off を含む）も終了と同じ判定と門を通す",
    );
    need(
        &lid,
        LID,
        "residual_gate_now",
        "pmset::residual_gate(",
        1,
        "門の実体は起動時の残留チェックと同じ関数",
    );
    need(
        &lid,
        LID,
        "residual_gate_now",
        "is_isolated_value(",
        1,
        "門の材料: 隔離（TAKO_ISOLATED）",
    );
    need(
        &lid,
        LID,
        "residual_gate_now",
        "other_tako_running",
        1,
        "門の材料: 他の tako の稼働",
    );
    need(
        &lid,
        LID,
        "settle_pmset",
        "with_record(",
        1,
        "何かすると決まったらロックの下で記録を読み直す（#1373 の作法）",
    );
    need(
        &lid,
        LID,
        "settle_pmset",
        "pmset::decide(",
        2,
        "写しとロックの下の両方で同じ判定を通す",
    );
    need(
        &pmset,
        PMSET,
        "should_clear_residual",
        "residual_gate(",
        1,
        "起動時の残留チェックも同じ門（門を 2 本にしない）",
    );
    need(
        &pmset,
        PMSET,
        "decide",
        "gate()",
        2,
        "持ち主の死んだ記録の 2 つの枝（引き取る・戻す）で門を引く",
    );
    out
}

fn product_files() -> Vec<(String, String)> {
    production_range::product_sources()
        .into_iter()
        .map(|rel| {
            let src = read(&rel);
            (rel, src)
        })
        .collect()
}

#[test]
fn 戻すのは倒した主を確かめた経路だけ() {
    let (stray, _) = stray_setters(&product_files());
    assert!(
        stray.is_empty(),
        "記録を確かめずに pmset disablesleep を打つ呼び出しがある:\n{}",
        stray.join("\n")
    );
}

#[test]
fn 終了と周期は判定の1実装へ起動時と同じ門を渡す() {
    let broken = broken_wiring(&read(LID), &read(PMSET));
    assert!(
        broken.is_empty(),
        "判定と門の配線が崩れている:\n{}",
        broken.join("\n")
    );
}

/// 許した呼び手が全部実在する（改名・削除で走査が空振りしたまま緑にならない）
#[test]
fn 走査が空振りしていない() {
    let (_, seen) = stray_setters(&product_files());
    for (file, name) in ALLOWED_CALLERS {
        assert!(
            seen.iter().any(|(f, n)| f == file && n == name),
            "{file}: fn {name} が set_disablesleep を呼んでいない（改名したら ALLOWED_CALLERS も直すこと）"
        );
    }
}

/// メモリ上の注入で、2 つの崩れが名指しされる（番犬が生きている証拠）
#[test]
fn 注入した崩れを名指しする() {
    let lid = read(LID);
    let pmset = read(PMSET);

    // 1. #2904 より前の終了時の形を戻す（所有者を見ずに戻す）
    let anchor = "        invalidate_cache();\n";
    assert!(lid.contains(anchor), "注入の目印が lid.rs に無い");
    let injected = lid.replacen(
        anchor,
        "        if is_sudoers_installed() && pmset_disabled_now() {\n            \
         let _ = set_disablesleep(false);\n        }\n        invalidate_cache();\n",
        1,
    );
    let (stray, _) = stray_setters(&[(LID.to_string(), injected)]);
    assert!(
        stray.len() == 1 && stray[0].contains("fn restore_on_exit が set_disablesleep"),
        "所有者を見ずに戻す形を名指ししない: {stray:?}"
    );

    // 2. 終了時の門を外す（判定へ常に開いた門を渡す）
    let gated = "settle_pmset(false, residual_gate_now)";
    assert!(lid.contains(gated), "注入の目印が lid.rs に無い");
    let injected = lid.replacen(gated, "settle_pmset(false, || Ok(()))", 1);
    let broken = broken_wiring(&injected, &pmset);
    assert!(
        broken.len() == 1 && broken[0].contains("fn restore_on_exit"),
        "終了時の門を外した形を名指ししない: {broken:?}"
    );

    // 2'. 判定の枝から門を外す（持ち主の死んだ記録を門なしで戻す）
    let arm = "(false, true, Some(Abandoned | Unowned)) => match gate() {";
    assert!(pmset.contains(arm), "注入の目印が pmset.rs に無い");
    let injected = pmset.replacen(
        arm,
        "(false, true, Some(Abandoned | Unowned)) => match Ok::<(), &str>(()) {",
        1,
    );
    let broken = broken_wiring(&lid, &injected);
    assert!(
        broken.len() == 1 && broken[0].contains("fn decide"),
        "判定の枝から門を外した形を名指ししない: {broken:?}"
    );
}
