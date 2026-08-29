//! remote_serve — tailscale serve 設定の自己検査と張り直し（#1049）
//!
//! #1038 は「起動時に 1 回」自己疎通を測るところまでだった。起動後に serve 設定が
//! 失われても tako は `running: true` を返し続け、ユーザーには「起動しているのに
//! スマホから開けない」としか見えない。ここが引き受けるのは 3 つ:
//!
//! 1. **定期自己検査** — serve 設定が自分の到達先を向いているかを一定間隔で確かめる
//! 2. **自動 re-assert** — 消えていれば張り直す（**上限つき**。消し合いのループを作らない）
//! 3. **正直な劣化表示** — 直せないときは `running` だけを名乗らず、理由と次の一手を残す
//!
//! 判断はすべて純関数 + [`ServeOps`] 抽象に閉じてあるので、実 tailscale なしで
//! 「消える → 検知 → 張り直す → 上限で諦める」の全経路を機械検証できる。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::tailscale::{is_reclaimable_target, ServeState};

/// 自己検査の間隔（既定）。`tailscale serve status --json` を 1 回叩くだけで、
/// LocalAPI の呼び出しは実測数十 ms。daemon スレッドなので UI は専有しない
pub const WATCH_INTERVAL_SECS: u64 = 30;

/// 張り直しの上限（この daemon の生存中）。**無限ループ防止**:
/// 誰かが消し続けている状況で張り合っても解決しないので、上限で止めて劣化を表に出す
pub const REASSERT_MAX: u32 = 5;

/// 上限の回復に必要な「連続で健全だった検査回数」。
/// 長寿命の daemon が一度上限に達したきり二度と自己修復しなくなるのを防ぐ
/// （既定間隔なら 10 回 = 5 分ぶん静かなら、独立した事象として数え直す）
pub const REASSERT_BUDGET_RESET_OK: u32 = 10;

/// A/B: #1049 前へ戻す（定期自己検査もノード固定もしない）
pub const LEGACY_ENV: &str = "TAKO_1049_LEGACY";
/// 検証用の間隔上書き（秒）。下限 1 秒
pub const WATCH_SECS_ENV: &str = "TAKO_1049_WATCH_SECS";

/// #1049 前の挙動を再現するか
pub fn legacy_mode() -> bool {
    std::env::var(LEGACY_ENV).is_ok_and(|v| v == "1" || v == "true" || v == "on")
}

/// 自己検査の間隔（env で上書き可能。検証用）
pub fn watch_interval() -> std::time::Duration {
    let secs = std::env::var(WATCH_SECS_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|v| v.max(1))
        .unwrap_or(WATCH_INTERVAL_SECS);
    std::time::Duration::from_secs(secs)
}

/// serve 設定の照合結果（純関数の判断）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeCheck {
    /// 自分の到達先を向いている
    Ok,
    /// 設定そのものが無い
    Missing,
    /// tako 形状の別の到達先（世代違い / 前回のポートの残骸）→ 張り替えてよい
    Stale(String),
    /// tako 管理外の設定 → **触らない**（ユーザーの設定を壊さない）
    Foreign(String),
}

/// serve 設定と自分の到達先を突き合わせる（純関数）。
/// 「張り替えてよいか」の判定は起動時の張り替えと同じ [`is_reclaimable_target`] を使う
/// = 起動時に掴めるものだけを、稼働中も掴む
pub fn classify_serve(state: &ServeState, ours: &str) -> ServeCheck {
    match state {
        ServeState::Proxy(target) if target == ours => ServeCheck::Ok,
        ServeState::NotConfigured => ServeCheck::Missing,
        ServeState::Proxy(target) if is_reclaimable_target(target, Some(ours)) => {
            ServeCheck::Stale(target.clone())
        }
        ServeState::Proxy(target) => ServeCheck::Foreign(target.clone()),
        ServeState::Other => ServeCheck::Foreign("tako 管理外の serve 設定".to_string()),
    }
}

/// serve の読み書き（実 tailscale / テスト用の偽物を差し替えるための抽象）
pub trait ServeOps {
    /// いまの serve 設定を読む
    fn read_state(&mut self) -> Result<ServeState, String>;
    /// serve を自分の到達先へ張る
    fn assert_target(&mut self, target: &str) -> Result<(), String>;
    /// その到達先で**誰かが応答している**か。
    /// 生きている別世代の tako が :443 を持っているときに張り合わないための安全弁
    fn target_alive(&mut self, target: &str) -> bool;
    /// どの tailscaled へ話しているか（記録・表示用）
    fn describe(&self) -> String;
    /// 応答したノードの MagicDNS 名（分かれば）
    fn node(&self) -> Option<String>;
}

/// 1 回の検査で起きたこと（監査記録に載せる）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// 健全（記録しない）
    Healthy,
    /// 消えていたので張り直した
    Reasserted { was: String },
    /// 張り直そうとして失敗した
    ReassertFailed { was: String, error: String },
    /// 上限に達したので張り直さなかった
    GaveUp { was: String },
    /// ユーザー設定なので触らなかった
    LeftForeign { target: String },
    /// **生きている別の tako** が :443 を持っているので譲った
    TakenOver { by: String },
    /// 検査そのものができなかった
    CheckFailed { error: String },
    /// 上限の予算を回復した
    BudgetReset,
}

/// serve 自己検査の状態（daemon の生存中だけ持つ）
#[derive(Debug, Clone)]
pub struct ServeWatch {
    /// 自分の到達先（`http://127.0.0.1:<port>` / `unix:<path>`）
    pub target: String,
    /// 公開中の ts.net ホスト名
    pub host: String,
    /// これまでに張り直した回数
    pub reasserts: u32,
    /// 上限
    pub max_reasserts: u32,
    /// 連続で健全だった回数（上限の予算回復に使う）
    consecutive_ok: u32,
}

impl ServeWatch {
    pub fn new(target: String, host: String) -> Self {
        Self {
            target,
            host,
            reasserts: 0,
            max_reasserts: REASSERT_MAX,
            consecutive_ok: 0,
        }
    }

    /// 1 周期ぶんの検査。健全なら何もせず、消えていれば上限の範囲で張り直す
    pub fn tick<O: ServeOps>(&mut self, ops: &mut O, now: u64) -> (ServeHealth, WatchEvent) {
        let state = match ops.read_state() {
            Ok(s) => s,
            Err(e) => {
                self.consecutive_ok = 0;
                return (
                    self.health(now, "unknown", None, ops, Some(unknown_reason(&e))),
                    WatchEvent::CheckFailed { error: e },
                );
            }
        };
        match classify_serve(&state, &self.target) {
            ServeCheck::Ok => {
                self.consecutive_ok = self.consecutive_ok.saturating_add(1);
                let mut event = WatchEvent::Healthy;
                if self.reasserts > 0 && self.consecutive_ok >= REASSERT_BUDGET_RESET_OK {
                    self.reasserts = 0;
                    self.consecutive_ok = 0;
                    event = WatchEvent::BudgetReset;
                }
                (
                    self.health(now, "ok", Some(self.target.clone()), ops, None),
                    event,
                )
            }
            ServeCheck::Missing | ServeCheck::Stale(_) => {
                self.consecutive_ok = 0;
                let was = match classify_serve(&state, &self.target) {
                    ServeCheck::Stale(t) => t,
                    _ => "（設定なし）".to_string(),
                };
                // tako 形状でも**応答している相手**なら残骸ではなく現役の別 daemon。
                // 張り合うと :443 を奪い合って両方が上限まで暴れるので、譲って報告する
                if matches!(classify_serve(&state, &self.target), ServeCheck::Stale(_))
                    && ops.target_alive(&was)
                {
                    return (
                        self.health(
                            now,
                            "taken_over",
                            Some(was.clone()),
                            ops,
                            Some(taken_over_reason(&was)),
                        ),
                        WatchEvent::TakenOver { by: was },
                    );
                }
                if self.reasserts >= self.max_reasserts {
                    return (
                        self.health(
                            now,
                            "missing",
                            None,
                            ops,
                            Some(gave_up_reason(self.max_reasserts)),
                        ),
                        WatchEvent::GaveUp { was },
                    );
                }
                self.reasserts += 1;
                match ops.assert_target(&self.target.clone()) {
                    Ok(()) => (
                        self.health(now, "reasserted", Some(self.target.clone()), ops, None),
                        WatchEvent::Reasserted { was },
                    ),
                    Err(e) => (
                        self.health(
                            now,
                            "unreachable",
                            None,
                            ops,
                            Some(assert_failed_reason(&e)),
                        ),
                        WatchEvent::ReassertFailed { was, error: e },
                    ),
                }
            }
            ServeCheck::Foreign(target) => {
                self.consecutive_ok = 0;
                (
                    self.health(
                        now,
                        "foreign",
                        Some(target.clone()),
                        ops,
                        Some(foreign_reason(&target)),
                    ),
                    WatchEvent::LeftForeign { target },
                )
            }
        }
    }

    fn health<O: ServeOps>(
        &self,
        now: u64,
        state: &str,
        actual: Option<String>,
        ops: &O,
        reason: Option<(String, String)>,
    ) -> ServeHealth {
        ServeHealth {
            ok: matches!(state, "ok" | "reasserted"),
            checked_at: now,
            state: state.to_string(),
            target: self.target.clone(),
            actual,
            host: Some(self.host.clone()),
            node: ops.node(),
            handle: Some(ops.describe()),
            reasserts: self.reasserts,
            reason: reason.as_ref().map(|(r, _)| r.clone()),
            next_step: reason.map(|(_, n)| n),
        }
    }
}

fn unknown_reason(error: &str) -> (String, String) {
    (
        format!("serve 設定を確認できませんでした: {error}"),
        "Tailscale が動いているかを `tailscale status` で確認してください".to_string(),
    )
}

fn gave_up_reason(max: u32) -> (String, String) {
    (
        format!(
            "tailscale serve の HTTPS:443 設定が繰り返し消えています\
             （{max} 回張り直しても消されたため中止しました）"
        ),
        "`tailscale serve status` で誰が消しているかを確認してください\
         （古い世代の tako が動いていないか / 別の常駐が :443 を触っていないか）。\
         `tako remote stop && tako remote start` で公開し直せます"
            .to_string(),
    )
}

fn assert_failed_reason(error: &str) -> (String, String) {
    (
        format!("tailscale serve の張り直しに失敗しました: {error}"),
        "`tailscale serve --https=443 off` で一度解除してから \
         `tako remote stop && tako remote start` を試してください"
            .to_string(),
    )
}

fn taken_over_reason(target: &str) -> (String, String) {
    (
        format!(
            "HTTPS:443 が別の稼働中プロセスの到達先（{target}）に切り替わっています。             奪い合いを避けるため tako は張り直しません"
        ),
        "古い方を `tako remote stop` で止めてから `tako remote start` をやり直してください         （`tako remote status` の endpoint が今どちらを指しているかで見分けられます）"
            .to_string(),
    )
}

fn foreign_reason(target: &str) -> (String, String) {
    (
        format!(
            "HTTPS:443 が tako 管理外の設定になっています（{target}）。\
             ユーザー設定を壊さないため tako は張り直しません"
        ),
        "`tailscale serve status` を確認し、tako で公開したい場合は\
         `tailscale serve --https=443 off` の後に `tako remote start` をやり直してください"
            .to_string(),
    )
}

/// serve 設定を解決できなかった（= ノードが見つからない等）ときの health
pub fn health_for_handle_error(now: u64, target: &str, host: &str, err: &str) -> ServeHealth {
    ServeHealth {
        ok: false,
        checked_at: now,
        state: "node_missing".to_string(),
        target: target.to_string(),
        actual: None,
        host: Some(host.to_string()),
        node: None,
        handle: None,
        reasserts: 0,
        reason: Some(err.to_string()),
        next_step: None,
    }
}

/// 自己検査の結果。daemon が state ファイルへ書き、`tako remote status` が読む
/// （status は別プロセスなので、daemon のメモリを覗く代わりのいちばん軽い手段）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServeHealth {
    /// serve が自分の到達先を向いているか
    pub ok: bool,
    /// 最後に検査した時刻（unix epoch 秒）
    pub checked_at: u64,
    /// ok / reasserted / missing / foreign / unreachable / node_missing / unknown
    pub state: String,
    /// 自分の到達先
    pub target: String,
    /// 実際の serve 設定の向き先
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// 公開中の ts.net ホスト名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// 話しかけた tailscaled のノード名
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// 話しかけた相手の説明（`--socket …` / 既定探索）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    /// この daemon が張り直した回数
    #[serde(default)]
    pub reasserts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
}

/// 自己検査が止まっているとみなすまでの猶予（間隔の何倍か + 固定の余裕）
pub const STALE_INTERVAL_FACTOR: u64 = 3;
pub const STALE_GRACE_SECS: u64 = 30;

/// `tako remote status` に載せる形へ変換する（純関数）。
///
/// **古い検査結果を「健全」と言わない**のが要点: daemon の検査スレッドが
/// 止まっていれば、最後の ok は現在の保証にならない
pub fn status_fields(health: Option<&ServeHealth>, now: u64, interval_secs: u64) -> Value {
    let Some(h) = health else {
        // 自己検査を持たない daemon（古い世代 / TAKO_1049_LEGACY=1）
        return json!({ "serve_ok": Value::Null });
    };
    let age = now.saturating_sub(h.checked_at);
    let stale = age > interval_secs * STALE_INTERVAL_FACTOR + STALE_GRACE_SECS;
    let mut out = json!({
        "serve_ok": h.ok && !stale,
        "serve_state": if stale { "stale".to_string() } else { h.state.clone() },
        "serve_checked_at": h.checked_at,
        "serve_checked_age_secs": age,
        "serve_reasserts": h.reasserts,
    });
    if let Some(v) = &h.actual {
        out["serve_actual"] = json!(v);
    }
    if let Some(v) = &h.node {
        out["serve_node"] = json!(v);
    }
    if let Some(v) = &h.handle {
        out["serve_handle"] = json!(v);
    }
    let degraded = if stale {
        Some((
            format!(
                "serve 設定の自己検査が {age} 秒前から更新されていません\
                 （daemon の検査スレッドが止まっている可能性）"
            ),
            "`tako remote stop && tako remote start` で立て直してください".to_string(),
        ))
    } else if !h.ok {
        h.reason
            .clone()
            .map(|r| (r, h.next_step.clone().unwrap_or_default()))
    } else {
        None
    };
    if let Some((reason, next_step)) = degraded {
        out["degraded"] = json!({ "reason": reason, "next_step": next_step });
    }
    out
}

/// 劣化していれば warnings に載せる 1 行（GUI のリモートカードにも出る）
pub fn degraded_warning(fields: &Value) -> Option<String> {
    let d = fields.get("degraded")?;
    let reason = d.get("reason").and_then(|v| v.as_str()).unwrap_or("");
    let next = d.get("next_step").and_then(|v| v.as_str()).unwrap_or("");
    if reason.is_empty() {
        return None;
    }
    Some(if next.is_empty() {
        format!("リモート公開が機能していません: {reason}")
    } else {
        format!("リモート公開が機能していません: {reason}\n  次の一手: {next}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用の偽 serve。消える・張り直せない・ユーザー設定などを決定的に再現する
    struct FakeOps {
        state: ServeState,
        /// 張り直しても即座に消される（消し合いの相手を模す）
        wipe_after_assert: bool,
        assert_error: Option<String>,
        read_error: Option<String>,
        /// この到達先は応答する（= 現役の別 daemon が持っている）
        alive_targets: Vec<String>,
        pub asserts: u32,
    }

    impl FakeOps {
        fn new(state: ServeState) -> Self {
            Self {
                state,
                wipe_after_assert: false,
                assert_error: None,
                read_error: None,
                alive_targets: Vec::new(),
                asserts: 0,
            }
        }
    }

    impl ServeOps for FakeOps {
        fn read_state(&mut self) -> Result<ServeState, String> {
            match &self.read_error {
                Some(e) => Err(e.clone()),
                None => Ok(self.state.clone()),
            }
        }
        fn assert_target(&mut self, target: &str) -> Result<(), String> {
            self.asserts += 1;
            if let Some(e) = &self.assert_error {
                return Err(e.clone());
            }
            self.state = if self.wipe_after_assert {
                ServeState::NotConfigured
            } else {
                ServeState::Proxy(target.to_string())
            };
            Ok(())
        }
        fn target_alive(&mut self, target: &str) -> bool {
            self.alive_targets.iter().any(|t| t == target)
        }
        fn describe(&self) -> String {
            "fake（--socket /fake.sock）".to_string()
        }
        fn node(&self) -> Option<String> {
            Some("fake.ts.net".to_string())
        }
    }

    const OURS: &str = "http://127.0.0.1:51047";

    fn watch() -> ServeWatch {
        ServeWatch::new(OURS.to_string(), "fake.ts.net".to_string())
    }

    #[test]
    fn 自分の到達先を向いていれば何もしない() {
        let mut ops = FakeOps::new(ServeState::Proxy(OURS.into()));
        let (h, ev) = watch().tick(&mut ops, 100);
        assert_eq!(ev, WatchEvent::Healthy);
        assert_eq!(ops.asserts, 0);
        assert!(h.ok);
        assert_eq!(h.state, "ok");
    }

    #[test]
    fn 消えていれば張り直す() {
        let mut ops = FakeOps::new(ServeState::NotConfigured);
        let mut w = watch();
        let (h, ev) = w.tick(&mut ops, 100);
        assert!(matches!(ev, WatchEvent::Reasserted { .. }));
        assert_eq!(ops.asserts, 1);
        assert!(h.ok);
        assert_eq!(h.state, "reasserted");
        assert_eq!(h.reasserts, 1);
        // 次の周期は健全に戻る
        let (h2, ev2) = w.tick(&mut ops, 130);
        assert_eq!(ev2, WatchEvent::Healthy);
        assert_eq!(ops.asserts, 1);
        assert!(h2.ok);
    }

    #[test]
    fn 前世代の残骸なら張り替える() {
        // 別ポート = tako 形状の残骸
        let mut ops = FakeOps::new(ServeState::Proxy("http://127.0.0.1:40000".into()));
        let (_, ev) = watch().tick(&mut ops, 100);
        assert_eq!(
            ev,
            WatchEvent::Reasserted {
                was: "http://127.0.0.1:40000".into()
            }
        );
        assert_eq!(ops.asserts, 1);
    }

    #[test]
    fn 生きている別のtakoとは張り合わない() {
        let other = "http://127.0.0.1:60000";
        let mut ops = FakeOps::new(ServeState::Proxy(other.into()));
        ops.alive_targets.push(other.to_string());
        let mut w = watch();
        let (h, ev) = w.tick(&mut ops, 100);
        assert_eq!(
            ev,
            WatchEvent::TakenOver {
                by: other.to_string()
            }
        );
        assert_eq!(ops.asserts, 0, "奪い返しに行ってはいけない");
        assert!(!h.ok);
        assert_eq!(h.state, "taken_over");
        assert!(h.next_step.as_deref().unwrap_or("").contains("remote stop"));
        // 何周回しても張り合わない（ping-pong 防止）
        for i in 0..5 {
            let (_, ev) = w.tick(&mut ops, 200 + i);
            assert!(matches!(ev, WatchEvent::TakenOver { .. }));
        }
        assert_eq!(ops.asserts, 0);
    }

    #[test]
    fn 応答しない残骸なら張り替える() {
        // 同じ形でも応答が無ければ前世代の残骸 = 掴んでよい
        let mut ops = FakeOps::new(ServeState::Proxy("http://127.0.0.1:60000".into()));
        let (_, ev) = watch().tick(&mut ops, 100);
        assert!(matches!(ev, WatchEvent::Reasserted { .. }));
        assert_eq!(ops.asserts, 1);
    }

    #[test]
    fn ユーザーの設定は触らない() {
        let mut ops = FakeOps::new(ServeState::Proxy("http://192.168.1.5:8080".into()));
        let (h, ev) = watch().tick(&mut ops, 100);
        assert!(matches!(ev, WatchEvent::LeftForeign { .. }));
        assert_eq!(ops.asserts, 0, "ユーザー設定を上書きしてはいけない");
        assert!(!h.ok);
        assert_eq!(h.state, "foreign");
        assert!(h.next_step.is_some());
    }

    #[test]
    fn カスタム設定も触らない() {
        let mut ops = FakeOps::new(ServeState::Other);
        let (h, ev) = watch().tick(&mut ops, 100);
        assert!(matches!(ev, WatchEvent::LeftForeign { .. }));
        assert_eq!(ops.asserts, 0);
        assert_eq!(h.state, "foreign");
    }

    #[test]
    fn 張り合いは上限で止まる() {
        let mut ops = FakeOps::new(ServeState::NotConfigured);
        ops.wipe_after_assert = true;
        let mut w = watch();
        for i in 0..REASSERT_MAX {
            let (h, ev) = w.tick(&mut ops, 100 + i as u64);
            assert!(matches!(ev, WatchEvent::Reasserted { .. }), "i={i}");
            assert!(h.ok, "張り直した直後は ok（次の周期で消えたと分かる）");
        }
        let (h, ev) = w.tick(&mut ops, 200);
        assert!(matches!(ev, WatchEvent::GaveUp { .. }));
        assert_eq!(ops.asserts, REASSERT_MAX, "上限を超えて張り直さない");
        assert!(!h.ok);
        assert!(h.reason.as_deref().unwrap_or("").contains("繰り返し消え"));
        // さらに回しても増えない（無限ループ防止）
        let _ = w.tick(&mut ops, 230);
        assert_eq!(ops.asserts, REASSERT_MAX);
    }

    #[test]
    fn 長く健全なら上限の予算が戻る() {
        let mut ops = FakeOps::new(ServeState::NotConfigured);
        let mut w = watch();
        let (_, _) = w.tick(&mut ops, 100);
        assert_eq!(w.reasserts, 1);
        let mut event = WatchEvent::Healthy;
        for i in 0..REASSERT_BUDGET_RESET_OK {
            let (_, ev) = w.tick(&mut ops, 200 + i as u64);
            event = ev;
        }
        assert_eq!(event, WatchEvent::BudgetReset);
        assert_eq!(w.reasserts, 0);
    }

    #[test]
    fn 張り直せなければ理由と次の一手を出す() {
        let mut ops = FakeOps::new(ServeState::NotConfigured);
        ops.assert_error = Some("tailscaled が応答しない".into());
        let (h, ev) = watch().tick(&mut ops, 100);
        assert!(matches!(ev, WatchEvent::ReassertFailed { .. }));
        assert!(!h.ok);
        assert_eq!(h.state, "unreachable");
        assert!(h.reason.as_deref().unwrap_or("").contains("tailscaled"));
        assert!(h.next_step.is_some());
    }

    #[test]
    fn 検査自体ができなければ健全と言わない() {
        let mut ops = FakeOps::new(ServeState::Proxy(OURS.into()));
        ops.read_error = Some("接続できない".into());
        let (h, ev) = watch().tick(&mut ops, 100);
        assert!(matches!(ev, WatchEvent::CheckFailed { .. }));
        assert!(!h.ok);
        assert_eq!(h.state, "unknown");
    }

    #[test]
    fn 古い検査結果は健全と言わない() {
        let h = ServeHealth {
            ok: true,
            checked_at: 1000,
            state: "ok".into(),
            target: OURS.into(),
            actual: Some(OURS.into()),
            host: Some("fake.ts.net".into()),
            node: Some("fake.ts.net".into()),
            handle: None,
            reasserts: 0,
            reason: None,
            next_step: None,
        };
        let fresh = status_fields(Some(&h), 1010, 30);
        assert_eq!(fresh["serve_ok"], json!(true));
        assert!(fresh.get("degraded").is_none());

        let stale = status_fields(Some(&h), 1000 + 30 * 3 + 31, 30);
        assert_eq!(stale["serve_ok"], json!(false));
        assert_eq!(stale["serve_state"], json!("stale"));
        assert!(stale["degraded"]["reason"].as_str().is_some());
        assert!(degraded_warning(&stale).is_some());
    }

    #[test]
    fn 自己検査を持たない世代は判定を名乗らない() {
        let out = status_fields(None, 100, 30);
        assert_eq!(out["serve_ok"], Value::Null);
        assert!(out.get("degraded").is_none());
        assert!(degraded_warning(&out).is_none());
    }

    #[test]
    fn ノードが見つからないときは理由が出る() {
        let h = health_for_handle_error(100, OURS, "mac.ts.net", "ノードが見つかりません");
        let out = status_fields(Some(&h), 110, 30);
        assert_eq!(out["serve_ok"], json!(false));
        assert_eq!(out["serve_state"], json!("node_missing"));
        assert!(degraded_warning(&out).is_some());
    }

    #[test]
    fn health_は往復できる() {
        let h = ServeHealth {
            ok: false,
            checked_at: 42,
            state: "missing".into(),
            target: OURS.into(),
            actual: None,
            host: Some("mac.ts.net".into()),
            node: Some("mac.ts.net".into()),
            handle: Some("--socket /x".into()),
            reasserts: 3,
            reason: Some("r".into()),
            next_step: Some("n".into()),
        };
        let s = serde_json::to_string(&h).expect("serialize");
        let back: ServeHealth = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(h, back);
    }

    #[test]
    fn classify_serveは張り替えてよいものだけを掴む() {
        assert_eq!(
            classify_serve(&ServeState::Proxy(OURS.into()), OURS),
            ServeCheck::Ok
        );
        assert_eq!(
            classify_serve(&ServeState::NotConfigured, OURS),
            ServeCheck::Missing
        );
        assert_eq!(
            classify_serve(&ServeState::Proxy("unix:/x/tako-remote.sock".into()), OURS),
            ServeCheck::Stale("unix:/x/tako-remote.sock".into())
        );
        assert!(matches!(
            classify_serve(&ServeState::Proxy("unix:/srv/other.sock".into()), OURS),
            ServeCheck::Foreign(_)
        ));
    }
}
