//! 番犬: 子の走査が「busy の集合が変わった回」を確かめ直す配線を外さない（#2477）
//!
//! ## なぜ止めるのか
//!
//! 子の走査（`RunningChildrenScanState`）はプロセス表の 1 瞬間の写しで、採り直すのは指紋
//! （OSC 133 の状態など）が動いたときと 60 秒の保険だけ（#779）。シェルがプロンプトのたびに
//! 起こす数 ms の子を拾うと、短いコマンドは C と D が同じ tick の間に終わって指紋が Idle の
//! まま動かないので、アイドルのシェルが最大 60 秒「子が動いている」扱いになる（隔離セルフ
//! テストの項目 93 (e) = 694(e) が `busy_children=true` のまま 15 秒で落ちた）。
//!
//! 直し方は 2 か所の組で効く: ① GUI の定期走査が採り直した結果を前回と突き合わせる
//! （`settled_against`）② 判定が「変わった回」を次の tick で採り直す（`prev.unconfirmed`）。
//! どちらか 1 つが外れると、単体テストは緑のまま確かめ直しだけが黙って消える。

use std::path::{Path, PathBuf};

#[path = "common/byte_window.rs"]
mod byte_window;

fn read(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート");
    let path: PathBuf = root.join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// コメント行を落とし、空白を全部除いた本文（rustfmt の折り方に依存させない）
fn compact(source: &str) -> String {
    source
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .flat_map(|l| l.chars())
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// `text[open]` が `(` のとき、対応する `)` の次の位置
fn after_call(text: &str, open: usize) -> usize {
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return open + i + 1;
                }
            }
            _ => {}
        }
    }
    text.len()
}

const APP: &str = "crates/tako-app/src/main.rs";
const AGENTS: &str = "crates/tako-control/src/agents.rs";

/// ① GUI が子の走査を採り直す箇所はすべて、結果を前回と突き合わせている
#[test]
fn guiの子の走査は採り直した結果を前回と突き合わせる() {
    let app = compact(&read(APP));
    let needle = "agents::scan_running_children(";
    let calls: Vec<usize> = app.match_indices(needle).map(|(i, _)| i).collect();
    assert!(
        !calls.is_empty(),
        "{APP}: 定期走査の `{needle}` が見つからない（改名したら番犬も直す）"
    );
    for at in calls {
        let end = after_call(&app, at + needle.len() - 1);
        assert!(
            app[end..].starts_with(".settled_against(&running_prev)"),
            "{APP}: `{needle}…)` の結果が `.settled_against(&running_prev)` を通っていない\n\
             （busy の集合が変わった回の確かめ直しが消え、1 瞬間の子が 60 秒の保険まで残る = #2477）\n\
             直後: {}",
            byte_window::after(&app, end, 60)
        );
    }
}

/// ② 判定は「前回 busy の集合が変わった回」を採り直す（A/B の旧の腕だけが外す）
#[test]
fn 採り直しの判定は変わった回を確かめ直す() {
    let agents = compact(&read(AGENTS));
    let sig = "pubfnshould_rescan_running_children_in(";
    let start = agents
        .find(sig)
        .unwrap_or_else(|| panic!("{AGENTS}: `should_rescan_running_children_in` が見つからない"));
    // 本文 = 次の公開関数の手前まで（空白を落としているので行ではなく `pubfn` で区切る）
    let rest = &agents[start + sig.len()..];
    let body = &rest[..rest.find("pubfn").unwrap_or(rest.len())];
    assert!(
        body.contains("ifprev.unconfirmed&&!legacy_unconfirmed{returntrue;}"),
        "{AGENTS}: should_rescan_running_children_in が `prev.unconfirmed` で採り直していない（#2477）"
    );
    // 突き合わせは busy の集合（器あり = セッション名 / 器なし = ペイン ID）の両方を見る
    let settle = &agents[agents
        .find("pubfnsettled_against(")
        .unwrap_or_else(|| panic!("{AGENTS}: `settled_against` が見つからない"))..];
    let tail = "self}";
    let settle = &settle[..settle
        .find(tail)
        .map(|i| i + tail.len())
        .unwrap_or(settle.len())];
    assert!(
        settle.contains("self.busy_sessions!=prev.busy_sessions")
            && settle.contains("self.busy_panes!=prev.busy_panes"),
        "{AGENTS}: settled_against が busy_sessions / busy_panes の片方しか突き合わせていない\n{settle}"
    );
}
