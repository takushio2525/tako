//! 起動時の自己疎通（#1038）が接続元検証（#841）で断られたことの見分けと、その理由・次の一手（#2786）。
//!
//! 自己疎通は serve 越しに daemon 自身の `/api/health` を 1 回 GET する。401 / 403 / 404 は「serve が backend へ
//! 届いている」証拠として正常に数えてきたが、403 のうち**接続元検証の断り**は「serve の中継元（tailscale
//! デーモン）を信じられていない」状態で、スマホからの要求も全部 403 になる。修正前はこれも到達と数えたので、
//! start が成功を名乗り、利用者には原因の手がかりが無かった。
//!
//! daemon はこの断りの本文に `kind` = [`crate::remote_refusal::KIND_PEER_UNVERIFIED`] と理由コード `code` を添える
//! （`RemoteRefusal::http_body`）ので、ここで字面（daemon の表示言語）に依らず型へ戻す。`kind` の無い 403・
//! 理由コードを知らない 403・JSON でない本文は従来どおり到達と数える（別のゲートの 403 を巻き込まない）。
//!
//! 見分けても **daemon は止めない**（502 とは違う）: serve は backend へ届いていて、中継元を認めるかどうか
//! （所有者ゲート #1438・名前のゲート）は環境の側で直すもの。止めると status から理由を読めなくなるうえ、
//! 正当な構成を止めかねない。start の応答の `self_check`（[`start_field`]）と status の
//! `peer_verification.self_check_rejected` に理由コードを残し、人向けの表示は表示言語で組む（#2365 の型 =
//! [`detail_of_start_in`] / [`detail_of_status_in`]）。応答の JSON の文（`reason` / `next_step` / `warnings`）は
//! 他の remote の欄と同じく日本語のまま。

use serde_json::{json, Value};
use tako_core::i18n::Lang;

use crate::platform::local_endpoint::PeerReject;

/// A/B: 修正前（403 は本文に依らず到達と数える）へ戻す
pub const LEGACY_ENV: &str = "TAKO_2786_LEGACY";

/// 検証用の注入。`direct` なら自己疎通を serve を通さず daemon のループバックの待ち受けへ直に打ち、serve と
/// 同じ転送ヘッダ（`X-Forwarded-For` / `X-Forwarded-Host`）を付ける。接続元は daemon 自身（tailscale ではない）
/// なので、本物の接続元検証が `not_tailscale_daemon` で断る = 「全部が断られる環境」を偽の tailscale のまま
/// 再現できる。**検証プロセス**（`tako_core::paths::is_verification_process`）でだけ読む
pub const INJECT_ENV: &str = "TAKO_2786_INJECT";

/// [`INJECT_ENV`] の値: 自己疎通を daemon のループバックへ直に打つ
pub const INJECT_DIRECT: &str = "direct";

/// 修正前の分類へ戻すか（A/B）
pub fn legacy_mode() -> bool {
    std::env::var(LEGACY_ENV).is_ok_and(|v| v == "1")
}

/// 自己疎通を daemon のループバックへ直に打つ注入が効いているか（検証プロセスだけ）
pub fn inject_direct() -> bool {
    tako_core::paths::is_verification_process()
        && std::env::var(INJECT_ENV).is_ok_and(|v| v == INJECT_DIRECT)
}

/// 403 の本文が接続元検証の断りなら、その理由（純関数）。`kind` が違う・理由コードが無い / 知らない・
/// JSON でない本文は `None`（= 従来どおり到達と数える）
pub fn peer_reject_of_body(body: &str) -> Option<PeerReject> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value["kind"].as_str() != Some(crate::remote_refusal::KIND_PEER_UNVERIFIED) {
        return None;
    }
    PeerReject::from_code(value["code"].as_str()?)
}

/// 断られた理由の 1 文（言語ごとの網羅 match）
pub fn reason_in(reject: PeerReject, lang: Lang) -> String {
    match lang {
        Lang::Ja => format!(
            "リモート公開の自己疎通は daemon へ届きましたが、接続元検証で断られました（理由: {}）。\
             このままではスマホからの要求もすべて 403 になります",
            reject.describe_in(lang)
        ),
        Lang::En => format!(
            "the remote self-check reached the daemon but was refused by the peer verification \
             (reason: {}). As it is, every request from your phone gets 403 too",
            reject.describe_in(lang)
        ),
    }
}

/// 理由ごとの次の一手（(理由, 言語) の網羅 match）
pub fn next_step_in(reject: PeerReject, lang: Lang) -> String {
    use PeerReject::{
        ImageUnknown, NoPeerAddr, NotLoopback, NotTailscaleDaemon, OwnerUntrusted, Unresolved,
    };
    match (reject, lang) {
        (NotTailscaleDaemon, Lang::Ja) => {
            "serve の中継元が tailscale のプロセスと認められていません。tailscale を別の名前の実行ファイルで\
             動かしているなら、`tako remote stop` の後に `TAKO_REMOTE_TRUSTED_PEER_NAMES=<その名前> tako remote start` \
             で起こし直してください"
                .to_string()
        }
        (NotTailscaleDaemon, Lang::En) => {
            "the process relaying serve is not recognized as tailscale. If you run tailscale under an \
             executable with another name, run `tako remote stop`, then start again with \
             `TAKO_REMOTE_TRUSTED_PEER_NAMES=<that name> tako remote start`"
                .to_string()
        }
        (OwnerUntrusted, Lang::Ja) => {
            "tailscale デーモンが別の OS ユーザーで動いています。システムのサービス（root）かこのユーザーで\
             動かしてから、`tako remote stop && tako remote start` で起こし直してください"
                .to_string()
        }
        (OwnerUntrusted, Lang::En) => {
            "the tailscale daemon runs as another OS user. Run it as a system service (root) or as this \
             user, then restart with `tako remote stop && tako remote start`"
                .to_string()
        }
        (ImageUnknown, Lang::Ja) => {
            "接続元プロセスの実行ファイル名を読めませんでした。`tako remote stop && tako remote start` で\
             起こし直し、続くなら `tako remote status` の `peer_verification` を添えて報告してください"
                .to_string()
        }
        (ImageUnknown, Lang::En) => {
            "could not read the executable name of the peer process. Restart with \
             `tako remote stop && tako remote start`; if it keeps happening, report it with \
             `peer_verification` from `tako remote status`"
                .to_string()
        }
        (Unresolved | NoPeerAddr | NotLoopback, Lang::Ja) => {
            "`tako remote stop && tako remote start` で起こし直してください。続くなら \
             `tako remote status` の `peer_verification` を添えて報告してください"
                .to_string()
        }
        (Unresolved | NoPeerAddr | NotLoopback, Lang::En) => {
            "restart with `tako remote stop && tako remote start`; if it keeps happening, report it with \
             `peer_verification` from `tako remote status`"
                .to_string()
        }
    }
}

/// 理由と次の一手の 2 行（CLI の stderr・status の `warnings`。次の一手の字下げは
/// `remote_serve::degraded_warning_in` と同じ）
pub fn warning_in(reject: PeerReject, lang: Lang) -> String {
    let (reason, next) = (reason_in(reject, lang), next_step_in(reject, lang));
    match lang {
        Lang::Ja => format!("{reason}\n  次の一手: {next}"),
        Lang::En => format!("{reason}\n  next step: {next}"),
    }
}

/// start の応答の `self_check`（daemon の起動情報に載せる）。`peer_rejected` が理由コード、
/// `reason` / `next_step` は日本語（他の remote の欄と同じ）
pub fn start_field(reject: PeerReject) -> Value {
    json!({
        "ok": false,
        "status": 403,
        "peer_rejected": reject.code(),
        "reason": reason_in(reject, Lang::Ja),
        "next_step": next_step_in(reject, Lang::Ja),
    })
}

/// start の応答から理由を読む（無い・知らないコード = `None`）
pub fn reject_of_start(info: &Value) -> Option<PeerReject> {
    PeerReject::from_code(info["self_check"]["peer_rejected"].as_str()?)
}

/// status の応答から理由を読む（`peer_verification.self_check_rejected`。無い・知らないコード = `None`）
pub fn reject_of_status(status: &Value) -> Option<PeerReject> {
    PeerReject::from_code(status["peer_verification"]["self_check_rejected"].as_str()?)
}

/// `tako remote start` の人向けの 1 件（表示言語。応答の構造の欄から組む）
pub fn detail_of_start_in(info: &Value, lang: Lang) -> Option<String> {
    reject_of_start(info).map(|reject| warning_in(reject, lang))
}

/// `tako remote status` の人向けの 1 件（表示言語。応答の構造の欄から組む）
pub fn detail_of_status_in(status: &Value, lang: Lang) -> Option<String> {
    reject_of_status(status).map(|reject| warning_in(reject, lang))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 日本語の字を含むか（英語の文に日本語が混ざっていないかを見る）
    fn has_japanese(s: &str) -> bool {
        s.chars().any(|c| {
            matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x9fff | 0xff00..=0xffef | 0x3000..=0x303f)
        })
    }

    #[test]
    fn 接続元検証の断りの本文だけを理由へ戻す() {
        for reject in PeerReject::ALL {
            let body = crate::RemoteRefusal::PeerUnverified(reject)
                .http_body()
                .to_string();
            assert_eq!(peer_reject_of_body(&body), Some(reject), "{body}");
        }
        // 理由の無い 403・知らない理由・別の kind・JSON でない本文は見分けない（到達のまま）
        for body in [
            r#"{"error":"x"}"#,
            r#"{"error":"x","kind":"peer_unverified"}"#,
            r#"{"error":"x","kind":"peer_unverified","code":"zzz"}"#,
            r#"{"error":"x","kind":"device_not_registered","code":"owner_untrusted"}"#,
            "Forbidden",
            "",
        ] {
            assert_eq!(peer_reject_of_body(body), None, "{body}");
        }
    }

    #[test]
    fn 理由と次の一手は日英で言い英語に日本語が混ざらない() {
        for reject in PeerReject::ALL {
            let ja = warning_in(reject, Lang::Ja);
            let en = warning_in(reject, Lang::En);
            assert!(
                ja.contains("接続元検証") && ja.contains("次の一手: "),
                "{ja}"
            );
            assert!(ja.contains(reject.describe_in(Lang::Ja)), "{ja}");
            assert!(
                en.contains("peer verification") && en.contains("next step: "),
                "{en}"
            );
            assert!(en.contains(reject.describe_in(Lang::En)), "{en}");
            assert!(!has_japanese(&en), "英語に日本語が混ざる: {en}");
            // どの理由も起こし直しの道を言う（名前のゲートは信頼名を足して起こし直す）
            assert!(ja.contains("tako remote start"), "{ja}");
            assert!(en.contains("tako remote start"), "{en}");
        }
        // 名前のゲートは信頼名の足し方を、所有者ゲートは動かすユーザーを言う
        let name = next_step_in(PeerReject::NotTailscaleDaemon, Lang::En);
        assert!(name.contains("TAKO_REMOTE_TRUSTED_PEER_NAMES"), "{name}");
        let owner = next_step_in(PeerReject::OwnerUntrusted, Lang::Ja);
        assert!(owner.contains("別の OS ユーザー"), "{owner}");
    }

    #[test]
    fn startとstatusの応答の欄から表示言語で組む() {
        let info =
            json!({ "running": true, "self_check": start_field(PeerReject::OwnerUntrusted) });
        assert_eq!(reject_of_start(&info), Some(PeerReject::OwnerUntrusted));
        assert_eq!(info["self_check"]["ok"], json!(false));
        assert_eq!(info["self_check"]["status"], json!(403));
        assert_eq!(
            detail_of_start_in(&info, Lang::En).as_deref(),
            Some(warning_in(PeerReject::OwnerUntrusted, Lang::En).as_str())
        );
        // 到達（ok: true）・測れなかった（unknown）は何も言わない
        for other in [
            json!({ "self_check": { "ok": true, "status": 401 } }),
            json!({ "self_check": { "ok": false, "unknown": "timeout", "note": "x" } }),
            json!({ "running": true }),
        ] {
            assert_eq!(detail_of_start_in(&other, Lang::Ja), None, "{other}");
        }
        let status = json!({
            "running": true,
            "peer_verification": { "enforced": true, "self_check_rejected": "not_tailscale_daemon" },
        });
        assert_eq!(
            detail_of_status_in(&status, Lang::Ja).as_deref(),
            Some(warning_in(PeerReject::NotTailscaleDaemon, Lang::Ja).as_str())
        );
        for other in [
            json!({ "running": true, "peer_verification": { "enforced": true, "rejected": 3 } }),
            json!({ "running": true, "peer_verification": { "self_check_rejected": "zzz" } }),
            json!({ "running": false }),
        ] {
            assert_eq!(detail_of_status_in(&other, Lang::En), None, "{other}");
        }
    }
}
