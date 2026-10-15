//! paste_guard — 貼り付けの確認の経路をまたぐ状態（Issue #2050）
//!
//! 判定は `tako_core::paste_guard` の 1 実装（[`scan_with`](tako_core::paste_guard::scan_with)）。
//! ここは GUI のプロセスの中で、経路（GUI の貼り付け・`tako send` / MCP `tako_send_input`・
//! スマホの入力・mod の送信時の確認）をまたいで共有する 2 つだけを持つ:
//!
//! - **確認済みの値**（[`Acks`](tako_core::paste_guard::Acks)）: tako が自分でペインへ流した値
//!   （worker への初回プロンプト・AI の送信）と、人が確かめて送った値の指紋。mod の送信時の確認は
//!   ここに載った値を二度確かめない（自動化の送信で worker の claude を止めない・GUI の確認の
//!   直後の Enter でもう一度聞かない）
//! - **経路ごとの累計**（[`Route`]）: 判定した数・警告した数・確かめた数（2 回読んだ差で
//!   効いているかを外から確かめる口）
//!
//! **本文・検出値を持たない**（指紋と数だけ）。診断ログへ書くのは
//! `tako_core::paste_guard::log_line` の 1 行だけ（番犬 `issue2046_paste_guard_watchdog`）

use std::sync::Mutex;
use std::time::Instant;

use serde_json::{json, Value};
use tako_core::paste_guard::{self as pg, Acks};

/// 判定する経路（累計の区分。GUI の貼り付けの累計は GUI が持つ `PasteGuardStats`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `tako send` / MCP `tako_send_input`（AI・スクリプト。止めずに警告だけ）
    Send,
    /// スマホ（PWA）の入力（人。段に従う）
    Remote,
    /// mod の送信時の確認（claude の入力欄に入った後。人。段に従う）
    Mod,
}

impl Route {
    const ALL: [Route; 3] = [Route::Send, Route::Remote, Route::Mod];

    pub fn as_str(self) -> &'static str {
        match self {
            Route::Send => "send",
            Route::Remote => "remote",
            Route::Mod => "mod",
        }
    }

    fn index(self) -> usize {
        match self {
            Route::Send => 0,
            Route::Remote => 1,
            Route::Mod => 2,
        }
    }
}

/// 1 経路の累計
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteCounts {
    /// 判定した数
    pub checked: u64,
    /// 当たって警告だけ出した数（送った）
    pub warned: u64,
    /// 当たって確かめた数（送る前に止めた）
    pub prompted: u64,
    /// 確認済みの値だけだったので通した数
    pub acknowledged: u64,
}

#[derive(Default)]
struct State {
    acks: Acks,
    routes: [RouteCounts; 3],
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(State::default))
}

/// tako がそのペインへ流した文字・人が確かめて送った文字の当たりを、そのペインの確認済みとして
/// 覚える（指紋だけ・[`ACK_TTL`](pg::ACK_TTL)）。当たりが無ければ何も覚えない
pub fn note_delivered(pane: u64, text: &str) {
    if pg::legacy_2050() || text.is_empty() {
        return;
    }
    let now = Instant::now();
    with_state(|s| s.acks.note_text(pane, text, now));
}

/// その指紋がそのペインの確認済みか（`fingerprints` と同じ並びで返す）
pub fn acknowledged(pane: u64, fingerprints: &[String]) -> Vec<bool> {
    let now = Instant::now();
    with_state(|s| {
        fingerprints
            .iter()
            .map(|fp| s.acks.covers(pane, fp, now))
            .collect()
    })
}

/// 経路の累計を 1 つ進める
pub fn count(route: Route, f: impl FnOnce(&mut RouteCounts)) {
    with_state(|s| f(&mut s.routes[route.index()]));
}

/// 経路の累計の写し（テスト・応答用）
pub fn counts(route: Route) -> RouteCounts {
    with_state(|s| s.routes[route.index()])
}

/// 状態の応答に載せる部分（経路ごとの累計と、覚えている確認済みの値の数）
pub fn status_json() -> Value {
    let now = Instant::now();
    with_state(|s| {
        let mut routes = serde_json::Map::new();
        for r in Route::ALL {
            let c = s.routes[r.index()];
            routes.insert(
                r.as_str().to_string(),
                json!({
                    "checked": c.checked,
                    "warned": c.warned,
                    "prompted": c.prompted,
                    "acknowledged": c.acknowledged,
                }),
            );
        }
        json!({
            "routes": Value::Object(routes),
            "acknowledged_values": s.acks.len(now),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 流した値はそのペインだけの確認済みになる() {
        // 他のテストと静的な状態を分け合うので、ペイン番号を固有にする
        let key = format!("{}{}", "ghp_", "EXAMPLE0123456789abcdefghijklmnopqrst");
        let fp = pg::fingerprint(&key);
        assert_eq!(acknowledged(9_050_001, std::slice::from_ref(&fp)), [false]);
        note_delivered(9_050_001, &format!("worker への指示: {key}"));
        note_delivered(9_050_001, "");
        assert_eq!(acknowledged(9_050_001, std::slice::from_ref(&fp)), [true]);
        assert_eq!(acknowledged(9_050_002, std::slice::from_ref(&fp)), [false]);
        let before = counts(Route::Mod).checked;
        count(Route::Mod, |c| c.checked += 1);
        assert_eq!(counts(Route::Mod).checked, before + 1);
        let v = status_json();
        assert!(v["routes"]["send"]["checked"].is_u64());
        assert!(v["acknowledged_values"].as_u64().unwrap() >= 1);
        // 応答に指紋も値も載せない（数だけ）
        let text = v.to_string();
        assert!(!text.contains(&fp) && !text.contains("EXAMPLE"), "{text}");
    }
}
