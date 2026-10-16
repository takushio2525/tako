//! ネット復帰の即時再開で「次の停止」を取りこぼさない番犬（#2157 / FR-2.27.17）
//!
//! # なぜ要るか
//!
//! #2112 は「同じ停止に 2 回送らない」ために、送った停止の鍵（直前の人の発言 + エラーの
//! 1 論理行 + ターンの締めの行）をペインごとに覚える。ところが tako の「続き」を受けて
//! すぐ同じ形で止まり直すと、次の停止の鍵は前と同じになる（直前の人の発言 = 同じ「続き」・
//! 締めの行は分の精度）。覚えた鍵を捨てる契機が無いと、2 回目以降の停止を「もう送った停止」と
//! 読んで永久に送らない。捨てる契機は 2 つ:
//!
//! 1. 画面で**会話が先へ進んだ**のを見た（`NetScreen::Progressed` = 最後の項目が人の発言・生成中）
//! 2. 送った「続き」の**送達が届いたと決着した**（2 秒 tick が進んだ画面を見ないまま止まり直す速い失敗）
//!
//! どちらも 1 行で外せるのに、回線断が続いて止まり直すまで何も変わらない。ここで構造を見張る。
//! 画面の判定そのもの（通知行の読み飛ばし・中断の案内の範囲）は `tako_control::net_stop` の
//! 単体テストが旧判定との A/B で固定している。
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`逆戻りを名指しできる`] で、**現行ソースから作り直した注入**が file:line で名指しされる
//! ことを確かめる。

#[path = "common/code_view.rs"]
mod code_view;

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const DRIVER: &str = "crates/tako-app/src/net_autoresume.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// `open` の位置の開き括弧に対応する閉じ括弧の位置
fn matching(code: &str, open: usize) -> Option<usize> {
    let (o, c) = match code[open..].chars().next()? {
        '{' => ('{', '}'),
        '(' => ('(', ')'),
        _ => return None,
    };
    let mut depth = 0usize;
    for (i, ch) in code[open..].char_indices() {
        if ch == o {
            depth += 1;
        } else if ch == c {
            depth -= 1;
            if depth == 0 {
                return Some(open + i);
            }
        }
    }
    None
}

/// コードだけの眺めで `fn <name>(` の頭の位置と、本体（波括弧の中）の範囲を返す
fn fn_body(code: &str, name: &str) -> Option<(usize, usize, usize)> {
    let head = format!("fn {name}(");
    let start = code.find(&head)?;
    let open = start + code[start..].find('{')?;
    Some((start, open, matching(code, open)?))
}

/// ① `drive_net_autoresume` の `NetScreen::Progressed` の腕で、送った停止の鍵を捨てる
fn check_progressed_forgets(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let Some((head, open, close)) = fn_body(&code, "drive_net_autoresume") else {
        return vec![format!("{file} — fn drive_net_autoresume が無い")];
    };
    let body = &code[open..close];
    let Some(arm) = body.find("NetScreen::Progressed =>") else {
        return vec![format!(
            "{file}:{} — drive_net_autoresume が NetScreen::Progressed（会話が進んだ）を見分けていない",
            line_of(src, head)
        )];
    };
    let at = open + arm;
    let Some(block_open) = code[at..].find('{').map(|i| at + i) else {
        return vec![format!(
            "{file}:{} — Progressed の腕が読めない",
            line_of(src, at)
        )];
    };
    let block = matching(&code, block_open).map_or("", |end| &code[block_open..end]);
    if block.contains("sent_keys.remove(&pane)") {
        Vec::new()
    } else {
        vec![format!(
            "{file}:{} — 会話が進んだのを見ても送った停止の鍵を捨てていない（次の停止を「送った停止」と読む）",
            line_of(src, at)
        )]
    }
}

/// ② 送達が届いた後の同じ形の停止を次の停止と読む: `answered_since_sent(pane)` を
/// 「前に送った停止か」の判定（`let already`）より前で見て、鍵を捨てる
fn check_answered_before_already(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let Some((head, open, close)) = fn_body(&code, "drive_net_autoresume") else {
        return vec![format!("{file} — fn drive_net_autoresume が無い")];
    };
    let body = &code[open..close];
    let mut v = Vec::new();
    let call = body.find("self.answered_since_sent(pane)");
    let already = body.find("let already");
    match (call, already) {
        (Some(c), Some(a)) if c < a => {
            let at = open + c;
            let block = code[at..]
                .find('{')
                .map(|i| at + i)
                .and_then(|o| matching(&code, o).map(|e| &code[o..e]))
                .unwrap_or("");
            if !block.contains("sent_keys.remove(&pane)") {
                v.push(format!(
                    "{file}:{} — 送達が届いた後も送った停止の鍵を捨てていない",
                    line_of(src, at)
                ));
            }
        }
        (Some(c), _) => v.push(format!(
            "{file}:{} — 送達の決着を「前に送った停止か」の判定の後で見ている",
            line_of(src, open + c)
        )),
        (None, _) => v.push(format!(
            "{file}:{} — drive_net_autoresume が送達の決着（answered_since_sent）を見ていない",
            line_of(src, head)
        )),
    }
    v
}

/// ③ 「届いた」の決着は、送達フローが残っていない・状態が Delivered・送った後、の 3 つで決める
/// （送達中・諦めた送達・送る前の決着を「届いた」と読むと、同じ停止へ 2 回送る）
fn check_answered_is_delivery(file: &str, src: &str) -> Vec<String> {
    let code = code_view::code_view(src);
    let Some((head, open, close)) = fn_body(&code, "answered_since_sent") else {
        return vec![format!("{file} — fn answered_since_sent が無い")];
    };
    let body = &code[open..close];
    let mut v = Vec::new();
    for (needle, why) in [
        (
            "!self.prompt_flows.iter().any(|f| f.pane == pane)",
            "送達が走っているあいだも決着とみなしている",
        ),
        (
            "State::Delivered",
            "届いていない決着（諦めた等）も数えている",
        ),
        ("at > sent", "送る前の決着も数えている"),
    ] {
        if !body.contains(needle) {
            v.push(format!("{file}:{} — {why}", line_of(src, head)));
        }
    }
    v
}

#[test]
fn 会話が進んだら送った停止の鍵を捨てる() {
    let v = check_progressed_forgets(DRIVER, &read(DRIVER));
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn 送達が届いた後の同じ形の停止は次の停止() {
    let v = check_answered_before_already(DRIVER, &read(DRIVER));
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn 届いたの決着は送達の状態で決める() {
    let v = check_answered_is_delivery(DRIVER, &read(DRIVER));
    assert!(v.is_empty(), "{v:#?}");
}

/// 注入した逆戻りが file:line で名指しされる（番犬が見逃す側へ倒れていない）
#[test]
fn 逆戻りを名指しできる() {
    let driver = read(DRIVER);
    // ① Progressed の腕で鍵を捨てない（追跡だけ消す）
    let arm = driver
        .find("NetScreen::Progressed =>")
        .expect("Progressed の腕");
    let rel = driver[arm..]
        .find("self.net_resume.sent_keys.remove(&pane);")
        .expect("Progressed の腕で鍵を捨てる行");
    let at = arm + rel;
    let injected = format!(
        "{}let _ = &pane;{}",
        &driver[..at],
        &driver[at + "self.net_resume.sent_keys.remove(&pane);".len()..]
    );
    let v = check_progressed_forgets(DRIVER, &injected);
    assert!(
        v.iter().any(|m| m.starts_with(&format!("{DRIVER}:"))),
        "① Progressed で鍵を捨てない注入を名指せない: {v:?}"
    );
    // ② 送達の決着を見ない
    assert_eq!(
        driver.matches("self.answered_since_sent(pane)").count(),
        1,
        "注入の目印が 1 つ"
    );
    let injected = driver.replacen("self.answered_since_sent(pane)", "false", 1);
    let v = check_answered_before_already(DRIVER, &injected);
    assert!(
        v.iter().any(|m| m.contains("見ていない")),
        "② 送達の決着を見ない注入を名指せない: {v:?}"
    );
    // ③ 諦めた送達も「届いた」と読む
    let injected = driver.replacen(
        "status.state == tako_core::prompt_delivery::State::Delivered",
        "status.state != tako_core::prompt_delivery::State::Queued",
        1,
    );
    assert_ne!(injected, driver, "注入の目印がある");
    let v = check_answered_is_delivery(DRIVER, &injected);
    assert!(
        v.iter().any(|m| m.contains("諦めた")),
        "③ 届いていない決着を数える注入を名指せない: {v:?}"
    );
}
