//! 起動待ちの帯の文言（#2425。起動待ちそのものは #2413）
//!
//! 帯は 3 行で、狭い worker のペインでも行ごとに読めるようにしてある:
//! ① 何を待っているか + 順番 ② 待つ理由（メモリ圧・空き）と起きる条件 ③ 待っている間の打鍵の扱い。
//! 型（`launch_gate::WaitBand`）から表示言語で組む。`tako list` の `launch_wait_band.lines` も同じ文

use tako_control::launch_gate::{BandState, WaitBand};
use tako_core::queue::WaitReason;

fn gb(mb: u64) -> String {
    format!("{:.1} GB", mb as f64 / 1024.0)
}

fn agent_name(band: &WaitBand) -> &str {
    if band.agent.is_empty() {
        "agent"
    } else {
        &band.agent
    }
}

/// 1 行目: 何を待っているか + 順番（起動中は起動していること）
pub fn title(band: &WaitBand) -> String {
    let agent = agent_name(band);
    match &band.state {
        BandState::Waiting {
            position: Some(position),
            waiting,
            ..
        } => tr!(
            format!("{agent} の起動待ち · {waiting} 本中 {position} 番目"),
            format!("Waiting to start {agent} · {position} of {waiting}")
        ),
        BandState::Waiting { position: None, .. } => tr!(
            format!("{agent} の起動待ち · 順番を決めています"),
            format!("Waiting to start {agent} · deciding the order")
        ),
        BandState::Starting => tr!(
            format!("{agent} を起動しています"),
            format!("Starting {agent}")
        ),
    }
}

/// 待つ理由の短い 1 句（メモリ不足の句だけは空きも含む）
fn reason_phrase(reason: &WaitReason) -> String {
    match reason {
        WaitReason::PressureCritical => {
            tr!("メモリ圧 critical", "Memory pressure critical").to_string()
        }
        WaitReason::PressureWarn => tr!("メモリ圧 warn", "Memory pressure warn").to_string(),
        WaitReason::Swap { used_mb } => tr!(
            format!("swap 使用 {}", gb(*used_mb)),
            format!("{} of swap in use", gb(*used_mb))
        ),
        WaitReason::Memory {
            available_mb,
            need_mb,
            ..
        } => tr!(
            format!(
                "空きメモリ不足（空き {}・必要 {}）",
                gb(*available_mb),
                gb(*need_mb)
            ),
            format!(
                "Not enough free memory ({} free, {} needed)",
                gb(*available_mb),
                gb(*need_mb)
            )
        ),
        WaitReason::TargetDir => {
            tr!("復元は 1 本ずつ", "Restored agents start one at a time").to_string()
        }
        WaitReason::Ahead => tr!("前に並んでいる起動の後", "Behind earlier launches").to_string(),
        // 起動待ちでは出ない（重い処理の順番待ちだけの理由）。綴りのまま出す
        other => other.code().to_string(),
    }
}

/// 2 行目: 待つ理由（メモリ圧・空き）と起きる条件
pub fn reason(band: &WaitBand) -> String {
    let BandState::Waiting { available_mb, .. } = &band.state else {
        return tr!("起動コマンドを送っています", "Sending the start command").to_string();
    };
    let Some(shown) = band.shown_reason() else {
        return tr!("メモリの状態を確かめています", "Checking memory").to_string();
    };
    let mut text = reason_phrase(shown);
    // メモリ不足の句は空きを含む。それ以外は判断に使った空きを添える
    if let (false, Some(mb)) = (matches!(shown, WaitReason::Memory { .. }), available_mb) {
        text.push_str(&tr!(
            format!(" · 空き {}", gb(*mb)),
            format!(" · {} free", gb(*mb))
        ));
    }
    let wake = match shown {
        WaitReason::TargetDir => tr!(
            "（前の agent が立ち上がると起動）",
            " (starts after the previous agent)"
        ),
        _ => tr!("（戻ると自動で起動）", " (starts when memory recovers)"),
    };
    text.push_str(wake);
    text
}

/// 3 行目: 待っている間の打鍵の扱い（受け付けなかった入力があれば回数）
pub fn input_note(ignored: u32) -> String {
    if ignored == 0 {
        tr!(
            "起動待ちのため、入力は起動してから受け付けます",
            "Input is accepted once the agent has started"
        )
        .to_string()
    } else {
        tr!(
            format!("起動待ちのため、入力は起動してから受け付けます（受け付けなかった入力 {ignored} 回）"),
            format!("Input is accepted once the agent has started ({ignored} inputs ignored)")
        )
    }
}

/// 帯の 3 行（描画と `tako list` の `lines` が同じ 1 実装を通る）
pub fn lines(band: &WaitBand, ignored: u32) -> [String; 3] {
    [title(band), reason(band), input_note(ignored)]
}

/// 「今すぐ起こす」ボタン（`tako queue start <ペイン>` と同じ口）
pub fn start_now_button() -> &'static str {
    tr!("今すぐ起こす", "Start now")
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;
    use tako_control::launch_gate::LaunchKind;

    fn band(state: BandState) -> WaitBand {
        WaitBand {
            pane: 7,
            kind: LaunchKind::Spawn,
            agent: "claude".into(),
            state,
        }
    }

    fn waiting(
        position: Option<usize>,
        reason: Option<WaitReason>,
        holding: Option<WaitReason>,
    ) -> WaitBand {
        band(BandState::Waiting {
            position,
            waiting: 3,
            reason,
            holding,
            pressure: None,
            available_mb: Some(1229),
        })
    }

    fn all_lines() -> Vec<String> {
        let reasons = [
            WaitReason::PressureCritical,
            WaitReason::PressureWarn,
            WaitReason::Swap { used_mb: 3174 },
            WaitReason::Memory {
                available_mb: 1229,
                need_mb: 2048,
                reserve_mb: 1024,
                pending_mb: 0,
                estimate_mb: 640,
            },
            WaitReason::TargetDir,
        ];
        let mut out = Vec::new();
        for r in reasons {
            out.extend(lines(&waiting(Some(1), Some(r), None), 0));
        }
        out.extend(lines(
            &waiting(
                Some(2),
                Some(WaitReason::Ahead),
                Some(WaitReason::PressureWarn),
            ),
            3,
        ));
        out.extend(lines(&waiting(None, None, None), 0));
        out.extend(lines(&band(BandState::Starting), 1));
        out.push(start_now_button().to_string());
        out
    }

    #[test]
    fn 日英が揃い絵文字が無い() {
        tests_support::check_ja_en(all_lines);
    }

    /// 前に並んでいるだけの件は、並びを止めている理由（warn）と空きを見せる。順番は N 本中 M 番目
    #[test]
    fn 順番と理由と空きが帯の文に出る() {
        let _guard = tests_support::lang_guard();
        tako_core::i18n::set_lang(tako_core::i18n::Lang::Ja);
        let b = waiting(
            Some(2),
            Some(WaitReason::Ahead),
            Some(WaitReason::PressureWarn),
        );
        let [t, r, n] = lines(&b, 0);
        assert_eq!(t, "claude の起動待ち · 3 本中 2 番目");
        assert_eq!(r, "メモリ圧 warn · 空き 1.2 GB（戻ると自動で起動）");
        assert_eq!(n, "起動待ちのため、入力は起動してから受け付けます");
        assert!(input_note(4).contains("4 回"));
        tako_core::i18n::set_lang(tako_core::i18n::Lang::En);
        let [t, r, n] = lines(&b, 2);
        assert_eq!(t, "Waiting to start claude · 2 of 3");
        assert_eq!(
            r,
            "Memory pressure warn · 1.2 GB free (starts when memory recovers)"
        );
        assert!(n.contains("2 inputs ignored"), "{n}");
    }
}
