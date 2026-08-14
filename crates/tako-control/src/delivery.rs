//! 指示送達の経路選択（Issue #790）
//!
//! claude worker への送達は 2 層構成にする。
//!
//! 1. **peer**（[`crate::peer_messaging`]）: claude の Cross-Session Messaging。
//!    socket 直送なので画面解析もキー操作も伴わない
//! 2. **keys**（[`crate::claude_tui::deliver_via_tmux`]）: 従来のキー操作経路
//!
//! 判定材料は claude のバージョン・`peerProtocol`・`kind=interactive`・受信箱 socket の
//! 実在・資格情報の可読性、そして「宛先がエージェント管理下の worker か」。
//! どちらを通ったかは必ず [`crate::diag::persist_log`] に残す（無音で経路が変わらない）。
//!
//! # 二重投函をしない
//!
//! peer 送達は socket へ書き切った時点で受信側のキューに入る。書き切った後に
//! 従来経路へ落ちると同じ指示が 2 回届く。そのため**フォールバックするのは
//! 「1 バイトも送っていないと言える段階」だけ**（可用性判定・接続失敗）に限る。

use crate::peer_messaging::{self, Mode, Transport, Unavailable, Verification};

/// 送達 1 回の結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryOutcome {
    /// 実際に使った経路
    pub transport: Transport,
    /// peer 経路の受信確認（keys 経路では None）
    pub verification: Option<Verification>,
    /// keys 経路になった理由コード（peer を使った場合は None）
    pub fallback_reason: Option<&'static str>,
}

/// peer 送達を試みた結果
pub enum PeerAttempt {
    /// peer で送達済み（**従来経路へ落ちてはならない**）
    Sent(DeliveryOutcome),
    /// peer は使えない / 送る前に失敗した。従来経路へ落ちてよい
    Fallback { reason: &'static str },
    /// `TAKO_PEER_MESSAGING=only` で peer が使えなかった（検証用。呼び出し側はエラーにする）
    Refused { note: String },
}

/// peer 送達の受信確認に待つ時間。生成中はキュー投函の痕跡が即座に出るので短くて足りる
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

/// I/O を伴わない事前判定（#790）。`agent_managed` = 宛先がエージェント管理下の
/// worker か（人間由来の送達では false。前置きの意味が変わるため peer を使わない）
pub fn plan(mode: Mode, agent_managed: bool) -> Result<(), Unavailable> {
    if mode == Mode::Off {
        return Err(Unavailable::Disabled);
    }
    if !agent_managed {
        return Err(Unavailable::NotAgentManaged);
    }
    Ok(())
}

/// バックエンド tmux セッションへ peer 送達を試みる
pub fn try_peer(backend_session: &str, text: &str, agent_managed: bool) -> PeerAttempt {
    let mode = peer_messaging::mode();
    if let Err(reason) = plan(mode, agent_managed) {
        return fallback(backend_session, reason, mode);
    }
    let target = match peer_messaging::resolve_for_backend(backend_session) {
        Ok(t) => t,
        Err(reason) => return fallback(backend_session, reason, mode),
    };

    // 送信の直前に transcript の読み取り位置を控える
    // （追記分だけを今回の証拠にする。時刻文字列では同じ秒の痕跡を取りこぼす）
    let mut cursor = peer_messaging::TranscriptCursor::capture(&target.session.session_id);
    if let Err(e) = peer_messaging::send(&target, text) {
        // 接続・書き込みの失敗。受信側はまだ 1 行も読めていないので従来経路へ落ちてよい
        crate::diag::persist_log(&format!(
            "送達: peer 送信に失敗し従来経路へ（session={backend_session} 理由={e}）"
        ));
        if mode == Mode::Only {
            return PeerAttempt::Refused {
                note: format!("peer 送達に失敗（{ENV_ONLY}）: {e}"),
            };
        }
        return PeerAttempt::Fallback {
            reason: "send_failed",
        };
    }

    let verification =
        peer_messaging::verify_delivered(&mut cursor, std::time::Instant::now() + VERIFY_TIMEOUT);
    crate::diag::persist_log(&format!(
        "送達: peer（session={backend_session} pid={} 状態={} 確認={}）",
        target.session.pid,
        target.session.status.as_deref().unwrap_or("?"),
        verification.as_str()
    ));
    PeerAttempt::Sent(DeliveryOutcome {
        transport: Transport::Peer,
        verification: Some(verification),
        fallback_reason: None,
    })
}

/// `TAKO_PEER_MESSAGING=only` の説明（エラー文で使う）
const ENV_ONLY: &str = "TAKO_PEER_MESSAGING=only";

/// 従来経路へ落ちる（理由を診断ログへ残す）
fn fallback(backend_session: &str, reason: Unavailable, mode: Mode) -> PeerAttempt {
    let code = reason.code();
    // Off / NotAgentManaged は「設計どおりそちらを通る」ので毎回ログへ出さない
    // （2 秒 tick の送達で persist.log を埋めないため）
    if !matches!(reason, Unavailable::Disabled | Unavailable::NotAgentManaged) {
        crate::diag::persist_log(&format!(
            "送達: keys へフォールバック（session={backend_session} 理由={code}: {}）",
            reason.note()
        ));
    }
    if mode == Mode::Only {
        return PeerAttempt::Refused {
            note: format!("peer 送達が使えない（{ENV_ONLY}）: {}", reason.note()),
        };
    }
    PeerAttempt::Fallback { reason: code }
}

/// keys 経路を使ったことを記録して結果を組む
pub fn keys_outcome(reason: &'static str) -> DeliveryOutcome {
    DeliveryOutcome {
        transport: Transport::Keys,
        verification: None,
        fallback_reason: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 人間由来の送達は_peer_を使わない() {
        assert_eq!(
            plan(Mode::Auto, false).unwrap_err(),
            Unavailable::NotAgentManaged
        );
        // worker 宛なら事前判定は通る（この先は実際の可用性次第）
        assert!(plan(Mode::Auto, true).is_ok());
    }

    #[test]
    fn off_は_worker_宛でも従来経路へ落ちる() {
        assert_eq!(plan(Mode::Off, true).unwrap_err(), Unavailable::Disabled);
        // only は事前判定を通す（使えなければ後段で Refused になる）
        assert!(plan(Mode::Only, true).is_ok());
    }

    #[test]
    fn keys_の結果には理由が残る() {
        let outcome = keys_outcome("no_registry_entry");
        assert_eq!(outcome.transport, Transport::Keys);
        assert_eq!(outcome.transport.as_str(), "keys");
        assert_eq!(outcome.fallback_reason, Some("no_registry_entry"));
        assert!(outcome.verification.is_none());
    }
}
