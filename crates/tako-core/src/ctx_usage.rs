//! ctx_usage — claude のコンテキスト使用率（ctx%）とモデル名を**複数ソースから**解決する
//! （Issue #1021。#749 の自動ハンドオフ / #702 の残量バーの拠り所）
//!
//! ## 何を解決するか
//!
//! `claude agents --json` は **2.1.258 では `contextPercentUsed` / `model` を返さない**。
//! この経路だけに頼っていたので ctx% 依存機能（`orchestrator self` の `ctx_percent` /
//! `worker_status` / #749 の閾値判定 / チャットヘッダの残量バー）が静かに全滅していた。
//!
//! ## 実物調査（claude 2.1.258 / macOS / 2026-09-04 実測）
//!
//! 1. **`agents --json` からは消えた**。出力キーは `cwd` / `kind` / `name` / `pid` /
//!    `sessionId` / `startedAt` / `status` の 7 つだけ。`contextPercentUsed` は
//!    **バイナリの文字列表からも消えている**ので改名でもない（= 復活を待てない）
//! 2. **セッション台帳（#1011 の `<config dir>/sessions/<pid>.json`）にも無い**。
//!    全キー和集合を採取して確認した
//! 3. **claude 自身の式は transcript から再現できる**。statusLine フックの stdin JSON
//!    （`.context_window.used_percentage`）の算出は
//!
//!    ```text
//!    used% = clamp(round(total_input_tokens / context_window_size * 100), 0, 100)
//!    total_input_tokens = input_tokens + cache_creation_input_tokens + cache_read_input_tokens
//!    ```
//!
//!    分子は**最後の API 応答の usage** = transcript の最後の assistant 行の
//!    `message.usage` そのもの。`message.model` も同じ行にある
//! 4. **`context_window_size` はどこにもファイルとして残っていない**。transcript の
//!    全行種別にも `~/.claude.json`（`autoCompactWindowsCache` = null）にも無く、
//!    上流は「`[1m]` 接尾辞」「組織モデルカタログの `native_1m` フラグ」
//!    「`CLAUDE_CODE_MAX_CONTEXT_TOKENS`」「auto-compact の窓」から実行時に解決している。
//!    **つまり静的な表は原理的に権威になれない**
//! 5. **画面（screen）は「claude 自身の答え」だが、既定構成では出ない**。組み込み表示
//!    （`NN% context used` / `NN% until auto-compact` / `Context low (NN% remaining)`）は
//!    `warn` 閾値 = `窓 - 33000` トークンを超えるまで**何も描かない**（1M 窓なら ~96.7%）。
//!    `ctx NN%` が普段見えているのはユーザーが設定した statusLine スクリプトの出力
//!
//! ## だから優先順は「画面 → transcript」
//!
//! - **画面**が数値を出しているならそれが正（claude 自身が計算した答え。窓の推定が要らない）
//! - 画面が黙っていれば **transcript**（式は厳密。ただし窓の解決が要る）
//! - どちらも駄目なら **none + 理由**（`null` を無説明で返さない）
//!
//! 両方あるときは**突き合わせて差を申告する**（`screen_delta`）。上流が窓の決め方を
//! 変えたら差として現れるので、静かに嘘をつくのではなく気づける（#1011 の
//! 「自己検証つきの内部レイアウト利用」と同じ型）。
//!
//! ## 窓の宣言表の根拠
//!
//! `declared_context_window` は**実測できたものと、その族**だけを返し、それ以外は
//! `None`（= 推測しない）。実測は本番の稼働セッションで
//! **8 ペイン / 3 モデル（`claude-opus-5` / `claude-fable-5` / `claude-fable-5-1`）**が
//! 窓 1,000,000 で画面の `ctx NN%` と**すべて一致**した（200,000 を要求する値は 1 つも無い）。

/// ctx% の取得元（応答の `ctx_source`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtxSource {
    /// ペインの画面（TUI フッター / statusLine の出力）= claude 自身の答え
    Screen,
    /// transcript の最後の assistant 行の usage から上流と同じ式で算出
    Transcript,
    /// 採れなかった（`CtxResolution::reason` に理由が入る）
    None,
}

impl CtxSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Transcript => "transcript",
            Self::None => "none",
        }
    }
}

/// ctx% が採れなかった理由（応答の `ctx_reason`。**無説明の null を返さない**ため）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtxUnavailable {
    /// セッションを特定できなかった（transcript を引く鍵が無い）
    NoSession,
    /// セッションに対応する transcript が見つからない
    NoTranscript,
    /// transcript にまだ assistant の usage が無い（1 ターンも走っていない）
    NoUsage,
    /// モデルに対する文脈窓が分からない（**推測しない**）
    UnknownContextWindow,
    /// `TAKO_1021_LEGACY=1`（#1021 前の挙動 = `agents --json` の値だけ）
    Legacy,
}

impl CtxUnavailable {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoSession => "no_session",
            Self::NoTranscript => "no_transcript",
            Self::NoUsage => "no_usage",
            Self::UnknownContextWindow => "unknown_context_window",
            Self::Legacy => "legacy_env",
        }
    }
}

/// 画面から読めた材料（`AgentMetrics` の ctx / model 部分）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScreenCtx {
    /// 画面が出していた使用率（0–100）
    pub percent: Option<u32>,
    /// 画面が出していたモデル表示名（例: `Opus 5`）
    pub model: Option<String>,
}

impl ScreenCtx {
    pub fn is_empty(&self) -> bool {
        self.percent.is_none() && self.model.is_none()
    }
}

/// transcript から読めた材料（最後の assistant 行の usage）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranscriptCtx {
    /// `input + cache_creation + cache_read`（上流の `total_input_tokens` と同じ）
    pub total_input_tokens: u64,
    /// `message.model`（モデル **id**。例: `claude-opus-5`）
    pub model: Option<String>,
}

/// 解決結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CtxResolution {
    pub percent: Option<u32>,
    pub source: CtxSource,
    /// モデル名（画面由来なら表示名、transcript 由来ならモデル id）
    pub model: Option<String>,
    /// transcript から算出したときに使った文脈窓
    pub window: Option<u64>,
    /// `source == None` のときの理由
    pub reason: Option<CtxUnavailable>,
    /// 画面と transcript の両方が数値を出したときの差（画面 − transcript）。
    /// 丸め誤差の範囲（±1）を超えたら窓の解決が実態とずれている合図
    pub screen_delta: Option<i32>,
}

impl CtxResolution {
    fn unavailable(reason: CtxUnavailable, model: Option<String>) -> Self {
        Self {
            percent: None,
            source: CtxSource::None,
            model,
            window: None,
            reason: Some(reason),
            screen_delta: None,
        }
    }

    /// 画面と transcript の差が丸め誤差（±1）を超えているか。
    /// 超えていれば窓の宣言が実態と食い違っている（申告して気づけるようにする）
    pub fn window_looks_wrong(&self) -> bool {
        self.screen_delta.is_some_and(|d| d.abs() > 1)
    }
}

/// 上流と同じ式で使用率を出す（`czt` の実装。実測で画面の値と一致することを確認済み）
pub fn used_percent(total_input_tokens: u64, window: u64) -> Option<u32> {
    if window == 0 {
        return None;
    }
    // round(t / w * 100) を整数演算で（f64 の丸めに依存しない）
    let scaled = total_input_tokens.checked_mul(200)? / window;
    let pct = (scaled + 1) / 2;
    Some(pct.min(100) as u32)
}

/// 画面の使用率と実トークン数から文脈窓を逆算する（自己検証用）。
/// 画面の % は整数へ丸められているので、**候補のどれかへ収まるか**だけを見る
pub fn implied_window(total_input_tokens: u64, percent: u32) -> Option<u64> {
    if percent == 0 || percent >= 100 {
        return None;
    }
    WINDOW_CANDIDATES
        .iter()
        .copied()
        .find(|w| used_percent(total_input_tokens, *w) == Some(percent))
}

/// 実運用で観測される文脈窓の候補（逆算のスナップ先）
pub const WINDOW_CANDIDATES: [u64; 3] = [1_000_000, 500_000, 200_000];

/// モデル id → 文脈窓。**分からないものは `None`**（推測して 5 倍ずれた ctx% を
/// 出すより、採れなかったと言う方が安全 = 早すぎる自動ハンドオフを起こさない）。
///
/// 根拠:
/// - `[1m]` 接尾辞は上流が無条件に 1M と解釈する（`/\[1m\]/i` の判定を実測）
/// - Claude 5 族（opus-5 / sonnet-5 / fable-5*）は本番の稼働セッションで
///   **8 ペイン / 3 モデルが窓 1M で画面の値と一致**（`claude-sonnet-5` は同族からの
///   宣言で、単体では未実測）
/// - それ以前の族は**未実測**なので返さない（画面が出しているときはそちらが使われる）
pub fn declared_context_window(model_id: &str) -> Option<u64> {
    let m = model_id.to_ascii_lowercase();
    if m.contains("[1m]") || m.contains("-1m") {
        return Some(1_000_000);
    }
    // `claude-opus-5` / `claude-fable-5-1` / `claude-sonnet-5-20260101` 等
    for family in ["opus-5", "sonnet-5", "fable-5"] {
        if let Some(rest) = m.split_once(family).map(|(_, rest)| rest) {
            // 族名の直後は終端か区切り（`-` / `.`）。`opus-50` のような別物を拾わない
            if rest.is_empty() || rest.starts_with('-') || rest.starts_with('.') {
                return Some(1_000_000);
            }
        }
    }
    None
}

/// 画面 → transcript → none の優先順で解決する。
///
/// **画面を先に見る**のは、画面の数値が claude 自身の計算結果であり文脈窓の推定を
/// 一切要らないため。transcript は式は厳密だが窓の解決が推定を含む
pub fn resolve(screen: Option<&ScreenCtx>, transcript: Option<&TranscriptCtx>) -> CtxResolution {
    let screen_percent = screen.and_then(|s| s.percent);
    let screen_model = screen.and_then(|s| s.model.clone());

    // transcript 側の算出（窓が分かるときだけ）
    let mut window = None;
    let mut transcript_percent = None;
    let mut transcript_reason = CtxUnavailable::NoTranscript;
    if let Some(t) = transcript {
        if t.total_input_tokens == 0 {
            transcript_reason = CtxUnavailable::NoUsage;
        } else {
            // 画面が数値を出しているなら、まず**実測から窓を逆算**して使う
            // （宣言表が古くても実態に追従する）。合わなければ宣言表へ落ちる
            let calibrated = screen_percent.and_then(|p| implied_window(t.total_input_tokens, p));
            let declared = t.model.as_deref().and_then(declared_context_window);
            match calibrated.or(declared) {
                Some(w) => {
                    window = Some(w);
                    transcript_percent = used_percent(t.total_input_tokens, w);
                }
                None => transcript_reason = CtxUnavailable::UnknownContextWindow,
            }
        }
    }

    // モデル名は画面の表示名を優先し、無ければ transcript のモデル id
    let model = screen_model.or_else(|| transcript.and_then(|t| t.model.clone()));

    let screen_delta = match (screen_percent, transcript_percent) {
        (Some(s), Some(t)) => Some(i64::from(s) as i32 - i64::from(t) as i32),
        _ => None,
    };

    if let Some(p) = screen_percent {
        return CtxResolution {
            percent: Some(p),
            source: CtxSource::Screen,
            model,
            window,
            reason: None,
            screen_delta,
        };
    }
    if let Some(p) = transcript_percent {
        return CtxResolution {
            percent: Some(p),
            source: CtxSource::Transcript,
            model,
            window,
            reason: None,
            screen_delta: None,
        };
    }
    let reason = if transcript.is_none() {
        CtxUnavailable::NoTranscript
    } else {
        transcript_reason
    };
    CtxResolution::unavailable(reason, model)
}

/// セッションを特定できなかったとき（transcript を引く鍵が無い）でも、
/// 画面が数値を出していればそれを使う
pub fn resolve_without_session(screen: Option<&ScreenCtx>) -> CtxResolution {
    let mut r = resolve(screen, None);
    if r.source == CtxSource::None {
        r.reason = Some(CtxUnavailable::NoSession);
    }
    r
}

/// `TAKO_1021_LEGACY=1` のときの結果（#1021 前 = `agents --json` の値だけ）
pub fn legacy_resolution() -> CtxResolution {
    CtxResolution::unavailable(CtxUnavailable::Legacy, None)
}

/// `TAKO_1021_LEGACY=1` が置かれているか（A/B 用。同一バイナリで旧挙動へ戻せる）
pub fn legacy_env() -> bool {
    std::env::var_os("TAKO_1021_LEGACY").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 上流と同じ式で使用率を出す() {
        // 実測（2026-09-04 / claude 2.1.258）: 画面が ctx 33% のとき
        // transcript の total_input_tokens は 334286、窓は 1,000,000
        assert_eq!(used_percent(334_286, 1_000_000), Some(33));
        // 丸めは round（0.5 は切り上げ）
        assert_eq!(used_percent(5_000, 1_000_000), Some(1)); // 0.5 → 1
        assert_eq!(used_percent(4_999, 1_000_000), Some(0)); // 0.4999 → 0
        assert_eq!(used_percent(335_000, 1_000_000), Some(34)); // 33.5 → 34
        // clamp（窓を超えても 100 で止まる）
        assert_eq!(used_percent(2_000_000, 1_000_000), Some(100));
        assert_eq!(used_percent(0, 1_000_000), Some(0));
        // 窓 0 は答えを出さない（0 除算を作らない）
        assert_eq!(used_percent(1_000, 0), None);
    }

    #[test]
    fn 本番で実測した8組がすべて窓1mで画面と一致する() {
        // 2026-09-04 本番の稼働ペイン（画面の ctx% ↔ transcript の total_input_tokens）。
        // 窓 1,000,000 で 8/8 一致し、200,000 を要求する組は 1 つも無かった
        let measured: [(u64, u32, &str); 8] = [
            (373_836, 37, "claude-opus-5"),
            (375_966, 38, "claude-opus-5"),
            (417_323, 42, "claude-fable-5-1"),
            (471_550, 47, "claude-fable-5-1"),
            (288_617, 29, "claude-fable-5"),
            (269_902, 27, "claude-fable-5"),
            (215_905, 22, "claude-fable-5"),
            (122_429, 12, "claude-fable-5"),
        ];
        for (tokens, screen_pct, model) in measured {
            let window = declared_context_window(model)
                .unwrap_or_else(|| panic!("{model} の窓が宣言表に無い"));
            assert_eq!(window, 1_000_000, "{model}");
            assert_eq!(
                used_percent(tokens, window),
                Some(screen_pct),
                "{model} tokens={tokens}"
            );
            // 逆算でも同じ窓へ収まる（自己検証が空振りしない）
            assert_eq!(implied_window(tokens, screen_pct), Some(1_000_000));
        }
    }

    #[test]
    fn 宣言表は実測できた族と1m接尾辞だけを返す() {
        for id in [
            "claude-opus-5",
            "claude-sonnet-5",
            "claude-fable-5",
            "claude-fable-5-1",
            "claude-opus-5-20260101",
            "claude-sonnet-4-5[1m]",
        ] {
            assert_eq!(declared_context_window(id), Some(1_000_000), "{id}");
        }
        // **未実測の族は推測しない**（5 倍ずれた ctx% で早すぎる自動ハンドオフを起こさない）
        for id in [
            "claude-opus-4-6",
            "claude-sonnet-4-5",
            "claude-haiku-4-5-20251001",
            "claude-3-opus",
            "",
            "gpt-5.6-sol",
        ] {
            assert_eq!(declared_context_window(id), None, "{id}");
        }
        // 族名の途中一致で拾わない
        assert_eq!(declared_context_window("claude-opus-50"), None);
    }

    #[test]
    fn 画面が数値を出していれば画面が勝つ() {
        let screen = ScreenCtx {
            percent: Some(33),
            model: Some("Opus 5".into()),
        };
        let t = TranscriptCtx {
            total_input_tokens: 334_286,
            model: Some("claude-opus-5".into()),
        };
        let r = resolve(Some(&screen), Some(&t));
        assert_eq!(r.percent, Some(33));
        assert_eq!(r.source, CtxSource::Screen);
        assert_eq!(r.source.as_str(), "screen");
        // モデルは画面の表示名を優先
        assert_eq!(r.model.as_deref(), Some("Opus 5"));
        // 自己検証: 一致していれば差 0
        assert_eq!(r.screen_delta, Some(0));
        assert!(!r.window_looks_wrong());
    }

    #[test]
    fn 画面が黙っていればtranscriptから算出する() {
        let t = TranscriptCtx {
            total_input_tokens: 334_286,
            model: Some("claude-opus-5".into()),
        };
        let r = resolve(None, Some(&t));
        assert_eq!(r.percent, Some(33));
        assert_eq!(r.source, CtxSource::Transcript);
        assert_eq!(r.window, Some(1_000_000));
        // 画面にモデル名が無いときはモデル id が入る
        assert_eq!(r.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(r.screen_delta, None);
        assert_eq!(r.reason, None);
    }

    #[test]
    fn 画面のモデル名だけでも拾う() {
        // statusline が伸びて ctx 行だけ窓から押し出された / 逆に model 行だけ残った、を両方
        let screen = ScreenCtx {
            percent: None,
            model: Some("Fable 5.1".into()),
        };
        let t = TranscriptCtx {
            total_input_tokens: 417_323,
            model: Some("claude-fable-5-1".into()),
        };
        let r = resolve(Some(&screen), Some(&t));
        assert_eq!(r.percent, Some(42));
        assert_eq!(r.source, CtxSource::Transcript);
        assert_eq!(r.model.as_deref(), Some("Fable 5.1"));
    }

    #[test]
    fn 窓が分からないモデルは画面が無ければ採らない() {
        let t = TranscriptCtx {
            total_input_tokens: 100_000,
            model: Some("claude-opus-4-6".into()),
        };
        let r = resolve(None, Some(&t));
        assert_eq!(r.percent, None);
        assert_eq!(r.source, CtxSource::None);
        assert_eq!(r.reason, Some(CtxUnavailable::UnknownContextWindow));
        assert_eq!(r.reason.unwrap().as_str(), "unknown_context_window");
        // モデル名は分かっているので返す（理由の切り分けに使える）
        assert_eq!(r.model.as_deref(), Some("claude-opus-4-6"));
    }

    #[test]
    fn 窓が分からないモデルでも画面があれば逆算して補える() {
        // 宣言表に無い族でも、画面が数値を出していれば窓を逆算できる（表が古くても追従）
        let screen = ScreenCtx {
            percent: Some(50),
            model: None,
        };
        let t = TranscriptCtx {
            total_input_tokens: 100_000,
            model: Some("claude-opus-4-6".into()),
        };
        let r = resolve(Some(&screen), Some(&t));
        assert_eq!(r.percent, Some(50));
        assert_eq!(r.source, CtxSource::Screen);
        assert_eq!(r.window, Some(200_000));
        assert_eq!(r.screen_delta, Some(0));
    }

    #[test]
    fn 窓の宣言が実態とずれていれば差として現れる() {
        // 画面 10% / トークン 100_000 → 実態の窓は 1M。宣言が 200K だと transcript は 50%
        // になるので差 -40 が出る（**画面が勝つので答えは間違わない**）
        let screen = ScreenCtx {
            percent: Some(10),
            model: None,
        };
        let t = TranscriptCtx {
            total_input_tokens: 100_000,
            model: Some("claude-opus-4-6".into()),
        };
        // 逆算が効くケースなので差は出ない
        let r = resolve(Some(&screen), Some(&t));
        assert_eq!(r.window, Some(1_000_000));
        assert_eq!(r.screen_delta, Some(0));

        // 逆算がどの候補にも収まらない（= 上流が未知の窓を使った）ときは宣言表へ落ち、差が出る
        let screen = ScreenCtx {
            percent: Some(7),
            model: None,
        };
        let t = TranscriptCtx {
            total_input_tokens: 334_286,
            model: Some("claude-opus-5".into()),
        };
        let r = resolve(Some(&screen), Some(&t));
        assert_eq!(r.percent, Some(7), "画面が勝つ");
        assert_eq!(r.window, Some(1_000_000), "宣言表へ落ちる");
        assert_eq!(r.screen_delta, Some(7 - 33));
        assert!(r.window_looks_wrong());
    }

    #[test]
    fn 材料が無ければ理由を返す() {
        let r = resolve(None, None);
        assert_eq!(r.source, CtxSource::None);
        assert_eq!(r.reason, Some(CtxUnavailable::NoTranscript));

        // 1 ターンも走っていない（usage がまだ無い）
        let t = TranscriptCtx {
            total_input_tokens: 0,
            model: None,
        };
        let r = resolve(None, Some(&t));
        assert_eq!(r.reason, Some(CtxUnavailable::NoUsage));

        // セッションが特定できない
        let r = resolve_without_session(None);
        assert_eq!(r.reason, Some(CtxUnavailable::NoSession));
        assert_eq!(r.reason.unwrap().as_str(), "no_session");

        // セッション不明でも画面が出していれば採れる
        let screen = ScreenCtx {
            percent: Some(20),
            model: None,
        };
        let r = resolve_without_session(Some(&screen));
        assert_eq!(r.percent, Some(20));
        assert_eq!(r.source, CtxSource::Screen);
        assert_eq!(r.reason, None);
    }

    #[test]
    fn legacyは理由つきでnullを返す() {
        let r = legacy_resolution();
        assert_eq!(r.percent, None);
        assert_eq!(r.source, CtxSource::None);
        assert_eq!(r.reason, Some(CtxUnavailable::Legacy));
        assert_eq!(r.reason.unwrap().as_str(), "legacy_env");
    }

    #[test]
    fn 逆算は候補の外なら答えを出さない() {
        assert_eq!(implied_window(334_286, 33), Some(1_000_000));
        // 0% / 100% は窓を決められない（トークン数が小さすぎる・飽和している）
        assert_eq!(implied_window(0, 0), None);
        assert_eq!(implied_window(2_000_000, 100), None);
        // どの候補でも説明できない組み合わせ
        assert_eq!(implied_window(334_286, 7), None);
    }
}
