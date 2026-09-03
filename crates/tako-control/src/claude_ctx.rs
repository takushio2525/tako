//! claude_ctx — ctx% / モデル名の解決を **1 実装**に集める（Issue #1021）
//!
//! `claude agents --json` から `contextPercentUsed` / `model` が消えた（2.1.258 実測）ため、
//! ctx% は「画面 → transcript → none」の優先順で解決する。判断そのものは
//! [`tako_core::ctx_usage`]（純関数）が持ち、このモジュールは**材料の集め方**だけを担う:
//!
//! - 画面: 生きたペインの表示行（GUI 層が持っている）→ `agent_metrics_from_*`
//! - transcript: session_id → 所在解決 → 最後の assistant の usage
//!
//! これを `orchestrator self` / `worker_status` / #749 の tick / チャットヘッダの
//! **4 経路が同じ関数**で通るようにするのが目的（1 箇所で直せば全部直る）。
//!
//! なぜ画面が先かは `tako_core::ctx_usage` のモジュールコメント参照
//! （画面の数値は claude 自身の計算結果で文脈窓の推定が要らない）。

use serde_json::{json, Value};
use tako_core::ctx_usage::{self, CtxResolution, ScreenCtx};

/// 画面から材料を採る（行の列。生きたペインが無ければ `None` を渡す）
pub fn screen_ctx_from_lines(lines: &[String]) -> Option<ScreenCtx> {
    let m = tako_core::terminal::agent_metrics_from_lines(lines)?;
    let ctx = ScreenCtx {
        percent: m.ctx_percent,
        model: m.model,
    };
    (!ctx.is_empty()).then_some(ctx)
}

/// 画面から材料を採る（1 本のテキスト版。dispatch はこの形で持っている）
pub fn screen_ctx_from_text(text: &str) -> Option<ScreenCtx> {
    let m = tako_core::terminal::agent_metrics_from_text(text)?;
    let ctx = ScreenCtx {
        percent: m.ctx_percent,
        model: m.model,
    };
    (!ctx.is_empty()).then_some(ctx)
}

/// ctx% とモデル名を解決する（**この 1 本を 4 経路が通る**）。
///
/// - `session_id`: transcript を引く鍵。`None` なら画面だけで解く
/// - `screen`: 生きたペインの画面から採った材料（無ければ `None`）
///
/// `TAKO_1021_LEGACY=1` が置かれていれば #1021 前の挙動（= `agents --json` の値だけ
/// なので claude では常に `null`）へ戻る。A/B を同一バイナリで取るための口
pub fn resolve(session_id: Option<&str>, screen: Option<&ScreenCtx>) -> CtxResolution {
    resolve_with(session_id, screen, ctx_usage::legacy_env())
}

/// [`resolve`] の legacy 判定を引数で受ける版（テストが env グローバルを触らないため。
/// 規約「グローバルを読む処理には引数で受ける版を必ず添える」= `.agent/conventions.md`）
pub fn resolve_with(
    session_id: Option<&str>,
    screen: Option<&ScreenCtx>,
    legacy: bool,
) -> CtxResolution {
    if legacy {
        return ctx_usage::legacy_resolution();
    }
    let Some(sid) = session_id else {
        return ctx_usage::resolve_without_session(screen);
    };
    let transcript = crate::transcript::last_context_usage(sid);
    ctx_usage::resolve(screen, transcript.as_ref())
}

/// 応答 JSON へ載せる形（`orchestrator self` / `worker_status` が同じキーを出す）。
///
/// `ctx_percent` が `null` のときに**理由が読める**ようにするのが要件（#1021）
pub fn response_fields(r: &CtxResolution) -> Value {
    json!({
        "ctx_percent": r.percent,
        "ctx_source": r.source.as_str(),
        "ctx_model": r.model,
        "ctx_window": r.window,
        "ctx_reason": r.reason.map(|x| x.as_str()),
        // 画面と transcript の突き合わせ（丸め誤差の範囲を超えたら窓の宣言がずれている）
        "ctx_screen_delta": r.screen_delta,
    })
}

/// 応答 JSON へ ctx 系のキーを混ぜ込む（オブジェクトでなければ何もしない）
pub fn merge_response_fields(target: &mut Value, r: &CtxResolution) {
    let Some(fields) = response_fields(r).as_object().cloned() else {
        return;
    };
    if let Some(obj) = target.as_object_mut() {
        for (k, v) in fields {
            obj.insert(k, v);
        }
    }
}

/// 窓の宣言が実態とずれていたら警告文を返す（`warnings` へ載せる。#1021 の自己検証）。
///
/// 上流が文脈窓の決め方を変えたら差として現れるので、静かに嘘をつくのではなく気づける
pub fn window_warning(r: &CtxResolution) -> Option<String> {
    if !r.window_looks_wrong() {
        return None;
    }
    let delta = r.screen_delta?;
    let window = r.window?;
    Some(format!(
        "ctx% の画面と transcript が {delta} ポイントずれています\
         （transcript 側で使った文脈窓 {window}。上流が窓の決め方を変えた合図。\
         画面の値を採用しています）"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tako_core::ctx_usage::CtxSource;

    #[test]
    fn 画面の行から材料を採る() {
        // この機の実物（ユーザー statusline の出力）
        let lines = vec![
            "  [Opus 5 · MAX]  worker".to_string(),
            "  ctx  33% ███░░░░░░░".to_string(),
            "  5h   75% ███████░░░".to_string(),
        ];
        let s = screen_ctx_from_lines(&lines).unwrap();
        assert_eq!(s.percent, Some(33));
        assert_eq!(s.model.as_deref(), Some("Opus 5"));
        // テキスト版でも同じ
        let s2 = screen_ctx_from_text(&lines.join("\n")).unwrap();
        assert_eq!(s2, s);
    }

    #[test]
    fn 材料が無い画面はnoneになる() {
        assert!(screen_ctx_from_lines(&[]).is_none());
        assert!(screen_ctx_from_lines(&["ただの出力".to_string()]).is_none());
        assert!(screen_ctx_from_text("").is_none());
    }

    #[test]
    fn 応答フィールドは理由まで載る() {
        let lines = vec!["  ctx  41% ████░░░░░░".to_string()];
        let screen = screen_ctx_from_lines(&lines).unwrap();
        // セッション不明でも画面があれば採れる
        let r = resolve(None, Some(&screen));
        let v = response_fields(&r);
        assert_eq!(v["ctx_percent"], json!(41));
        assert_eq!(v["ctx_source"], json!("screen"));
        assert_eq!(v["ctx_reason"], Value::Null);

        // 材料ゼロなら理由が入る（**無説明の null にしない**）
        let r = resolve(None, None);
        let v = response_fields(&r);
        assert_eq!(v["ctx_percent"], Value::Null);
        assert_eq!(v["ctx_source"], json!("none"));
        assert_eq!(v["ctx_reason"], json!("no_session"));
    }

    #[test]
    fn 応答へ混ぜ込む() {
        let lines = vec!["  ctx  12% █░░░░░░░░░".to_string()];
        let screen = screen_ctx_from_lines(&lines).unwrap();
        let r = resolve(None, Some(&screen));
        let mut target = json!({"pane_id": 7, "ctx_percent": Value::Null});
        merge_response_fields(&mut target, &r);
        assert_eq!(target["pane_id"], json!(7));
        assert_eq!(target["ctx_percent"], json!(12));
        assert_eq!(target["ctx_source"], json!("screen"));
        // オブジェクトでなければ何もしない（panic しない）
        let mut arr = json!([1, 2]);
        merge_response_fields(&mut arr, &r);
        assert_eq!(arr, json!([1, 2]));
    }

    #[test]
    fn legacyで旧挙動へ戻る() {
        let lines = vec!["  ctx  33% ███░░░░░░░".to_string()];
        let screen = screen_ctx_from_lines(&lines).unwrap();
        // 通常は画面から採れる
        let now = resolve_with(None, Some(&screen), false);
        assert_eq!(now.percent, Some(33));
        assert_eq!(now.source, CtxSource::Screen);
        // #1021 前は画面も transcript も見ないので null（= 報告された症状そのもの）
        let legacy = resolve_with(None, Some(&screen), true);
        assert_eq!(legacy.percent, None);
        assert_eq!(legacy.source, CtxSource::None);
        assert_eq!(legacy.reason.map(|x| x.as_str()), Some("legacy_env"));
    }

    #[test]
    fn 窓のずれは警告文になる() {
        // 画面 7% / トークン 334286（実態は 33% 相当）= 窓の宣言が実態とずれている形
        let screen = ScreenCtx {
            percent: Some(7),
            model: None,
        };
        let t = tako_core::ctx_usage::TranscriptCtx {
            total_input_tokens: 334_286,
            model: Some("claude-opus-5".into()),
        };
        let r = tako_core::ctx_usage::resolve(Some(&screen), Some(&t));
        let w = window_warning(&r).expect("ずれていれば警告が出る");
        assert!(w.contains("1000000"), "{w}");
        assert!(w.contains("画面の値を採用"), "{w}");
        // 一致しているときは警告しない
        let screen_ok = ScreenCtx {
            percent: Some(33),
            model: None,
        };
        let r = tako_core::ctx_usage::resolve(Some(&screen_ok), Some(&t));
        assert!(window_warning(&r).is_none());
    }
}
