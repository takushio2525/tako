//! 右パネルの `resources` ビューとステータスバーの区画の文言（#1995。エピック #1992 の S3）
//!
//! 数値は `tako queue` / MCP `tako_queue` の status と同じ答えを、表示言語で文にする。
//! 区切り方（GB は 1 桁・1 GB 未満は MB・時間は時間 / 分 / 秒）は CLI の
//! `queue_wrapper::human_mb` / `human_secs` / `eta::approx` にそろえる。
//! 優先度の段の名前（top / high / normal / low / default）は CLI の値と同じ語を両言語で使う。

use tako_core::queue::Priority;
use tako_core::resources::Pressure;

/// MiB → 「24.1 GB」/「512 MB」（言語に依らない。`queue_wrapper::human_mb` と同じ区切り）
pub fn size(mb: u64) -> String {
    if mb >= 1024 {
        format!("{:.1} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

/// 経過・待ちの時間（`queue_wrapper::human_secs` と同じ区切り）
pub fn secs(s: u64) -> String {
    if s >= 3600 {
        tr!(
            format!("{} 時間 {} 分", s / 3600, (s % 3600) / 60),
            format!("{}h {}m", s / 3600, (s % 3600) / 60)
        )
    } else if s >= 60 {
        tr!(
            format!("{} 分 {} 秒", s / 60, s % 60),
            format!("{}m {}s", s / 60, s % 60)
        )
    } else {
        tr!(format!("{s} 秒"), format!("{s}s"))
    }
}

/// 見込みのおおよその長さ（`eta::approx` と同じ丸め: 1 分未満は秒、以上は分へ丸める）
pub fn approx(s: u64) -> String {
    if s < 60 {
        tr!(format!("{s} 秒"), format!("{s}s"))
    } else {
        let m = (s + 30) / 60;
        tr!(format!("{m} 分"), format!("{m} min"))
    }
}

pub fn title() -> &'static str {
    tr!("PC の負荷", "PC load")
}
pub fn loading() -> &'static str {
    tr!("読んでいます…", "Loading…")
}
pub fn updating() -> &'static str {
    tr!("更新中", "Updating")
}
pub fn refresh() -> &'static str {
    tr!("読み直す", "Refresh")
}
pub fn failed(reason: &str) -> String {
    tr!(
        format!("読めませんでした: {reason}"),
        format!("Could not load: {reason}")
    )
}

// ===== ① 機 =====

pub fn section_machine() -> &'static str {
    tr!("機", "Machine")
}
pub fn unsupported() -> &'static str {
    tr!("未対応", "Not supported")
}
pub fn machine_unsupported() -> &'static str {
    tr!(
        "この OS では機の状態を採れない（未対応）",
        "Machine stats are not supported on this OS"
    )
}
pub fn memory_label() -> &'static str {
    tr!("メモリ", "Memory")
}
pub fn memory_used(used_mb: u64, total_mb: u64) -> String {
    format!("{} / {}", size(used_mb), size(total_mb))
}
pub fn memory_detail(compressed_mb: u64, available_mb: u64, swap: Option<(u64, u64)>) -> String {
    let swap = swap
        .map(|(used, total)| format!(" · swap {} / {}", size(used), size(total)))
        .unwrap_or_default();
    tr!(
        format!(
            "圧縮 {} · 空き {}{swap}",
            size(compressed_mb),
            size(available_mb)
        ),
        format!(
            "compressed {} · free {}{swap}",
            size(compressed_mb),
            size(available_mb)
        )
    )
}
/// メモリ圧の段の名前（`kern.memorystatus_vm_pressure_level` の 3 段。CLI と同じ語）
pub fn pressure(p: Option<Pressure>) -> String {
    let level = p.map_or("-", Pressure::as_str);
    tr!(format!("メモリ圧 {level}"), format!("pressure {level}"))
}
pub fn cpu_line(cpu: Option<f64>, cpus: u32, load: Option<[f64; 3]>) -> String {
    let pct = cpu.map_or_else(|| "-".to_string(), |c| format!("{c:.0}%"));
    let load = load
        .map(|l| format!(" · load {:.1} / {:.1} / {:.1}", l[0], l[1], l[2]))
        .unwrap_or_default();
    tr!(
        format!("CPU {pct}（{cpus} コア）{load}"),
        format!("CPU {pct} ({cpus} cores){load}")
    )
}

// ===== ② 実行中 / ③ 待ち =====

pub fn section_running(n: usize, max: Option<u64>) -> String {
    match max {
        Some(max) => tr!(
            format!("実行中 {n} / 上限 {max}"),
            format!("Running {n} / max {max}")
        ),
        None => tr!(format!("実行中 {n}"), format!("Running {n}")),
    }
}
pub fn section_waiting(n: usize) -> String {
    tr!(format!("待ち {n}"), format!("Waiting {n}"))
}
pub fn no_arbiter() -> &'static str {
    tr!(
        "この tako は順番待ちの調停役ではない（隔離 / セルフテスト / セカンダリ）",
        "This tako does not run the queue (isolated / self-test / secondary)"
    )
}
pub fn running_empty() -> &'static str {
    tr!("走っている処理はない", "Nothing running")
}
pub fn waiting_empty() -> &'static str {
    tr!("待っている処理はない", "Nothing waiting")
}
pub fn unmanaged(n: u64) -> String {
    tr!(
        format!("順番待ちを通っていない重い処理 {n} 件"),
        format!("{n} heavy process(es) outside the queue")
    )
}
/// 誰の処理か（ペイン · タブ · ペイン名 · 役割）。ペインの外（launchd・別の端末）は「ペインの外」
pub fn owner(
    pane: Option<u64>,
    tab: Option<&str>,
    title: Option<&str>,
    role: Option<&str>,
) -> String {
    let Some(pane) = pane else {
        return tr!("ペインの外", "Outside panes").to_string();
    };
    let mut parts = vec![tr!(format!("ペイン {pane}"), format!("pane {pane}"))];
    for part in [tab, title, role].into_iter().flatten() {
        if !part.is_empty() && !parts.iter().any(|p| p == part) {
            parts.push(part.to_string());
        }
    }
    parts.join(" · ")
}
pub fn shelved() -> &'static str {
    tr!("退避中", "shelved")
}
/// 実行中の 1 行の数値（CPU% · メモリ · 経過）
pub fn running_detail(cpu: Option<f64>, memory_mb: Option<u64>, ran_s: Option<u64>) -> String {
    let mut parts = Vec::new();
    if let Some(c) = cpu {
        parts.push(format!("CPU {c:.0}%"));
    }
    if let Some(m) = memory_mb {
        parts.push(size(m));
    }
    if let Some(s) = ran_s {
        parts.push(tr!(
            format!("経過 {}", secs(s)),
            format!("{} elapsed", secs(s))
        ));
    }
    parts.join(" · ")
}
/// 終わりの見込み（status の `eta` の欄から。`eta::Eta::text` と同じ 4 通り。経過は同じ行の
/// [`running_detail`] が出すので、ここでは繰り返さない）
pub fn eta(
    state: &str,
    remaining_s: Option<u64>,
    previous_s: Option<u64>,
    from_progress: Option<(u32, u32)>,
) -> String {
    match (state, remaining_s) {
        ("remaining", Some(rem)) => match (from_progress, previous_s) {
            (Some((done, total)), _) => tr!(
                format!("あと約 {}（進み {done}/{total} からの見込み）", approx(rem)),
                format!("about {} left (from {done}/{total})", approx(rem))
            ),
            (None, Some(prev)) => tr!(
                format!("あと約 {}（前回 {}）", approx(rem), approx(prev)),
                format!("about {} left (last time {})", approx(rem), approx(prev))
            ),
            (None, None) => tr!(
                format!("あと約 {}", approx(rem)),
                format!("about {} left", approx(rem))
            ),
        },
        ("overrun", _) => {
            let prev = previous_s.map(approx).unwrap_or_else(|| "-".into());
            tr!(
                format!("前回より長くかかっている（前回 {prev}）"),
                format!("taking longer than last time (last {prev})")
            )
        }
        _ => tr!("見込みなし".to_string(), "no estimate".to_string()),
    }
}
pub fn stalled() -> &'static str {
    tr!("停滞（枠を返した）", "stalled (slot released)")
}
pub fn cancelling() -> &'static str {
    tr!("取り消し中", "cancelling")
}
pub fn position(n: u64) -> String {
    tr!(format!("{n} 番目"), format!("#{n}"))
}
pub fn waited(s: u64) -> String {
    tr!(format!("待ち {}", secs(s)), format!("waited {}", secs(s)))
}
/// 待ちの開始の見込み（`eta::start_text` と同じ 2 通り）
pub fn start_eta(start_s: u64) -> String {
    if start_s == 0 {
        tr!(
            "空き次第すぐ開始の見込み".to_string(),
            "starts as soon as a slot frees".to_string()
        )
    } else {
        tr!(
            format!("開始まで約 {}", approx(start_s)),
            format!("starts in about {}", approx(start_s))
        )
    }
}
/// 待つ理由。日本語は調停役の 1 文（CLI と同じ = 数値の内訳つき）、英語は理由の種類から短く
pub fn wait_reason(code: Option<&str>, text_ja: Option<&str>) -> Option<String> {
    tr!(
        text_ja.map(str::to_string),
        code.map(|c| {
            match c {
                "regather" => "the queue just restarted (re-registering jobs)",
                "target_dir" => "a job using the same target dir is running",
                "pressure_critical" => "memory pressure critical (new jobs are held)",
                "pressure_warn" => "memory pressure warn (one at a time)",
                "swap" => "swap in use (one at a time)",
                "max_jobs" => "the concurrent job limit is reached",
                "memory" => "not enough free memory",
                "ahead" => "behind jobs queued earlier",
                "overtake_limit" => "a job waiting for its target dir hit the overtake limit",
                other => other,
            }
            .to_string()
        })
    )
}

// ===== 優先度・取り消し =====

/// 段の出どころ（処理に付けた段 / ペインの既定 / 役割の既定。待ちで上がったら元の段も）
pub fn priority_from(from: Option<&str>, aged: bool, base: Option<Priority>) -> Option<String> {
    let from = match from? {
        "job" => tr!("処理に付けた段", "set on the job"),
        "pane" => tr!("ペインの既定", "pane default"),
        "role" => tr!("役割の既定", "role default"),
        _ => return None,
    };
    Some(if aged {
        let base = base.map_or("?", Priority::as_str);
        tr!(
            format!("{from} {base} から待ちで上がった"),
            format!("{from}, raised from {base} by waiting")
        )
    } else {
        from.to_string()
    })
}
pub fn priority_tip() -> &'static str {
    tr!(
        "優先度を変える（tako queue priority と同じ）",
        "Change priority (same as tako queue priority)"
    )
}
pub fn priority_job_row() -> &'static str {
    tr!("この処理", "This job")
}
pub fn priority_pane_row(pane: u64) -> String {
    tr!(
        format!("ペイン {pane} の既定"),
        format!("Pane {pane} default")
    )
}
/// 段の選択肢の `default`（既定へ戻す）
pub fn priority_default() -> &'static str {
    "default"
}
pub fn priority_changed(level: &str, position: Option<u64>) -> String {
    match position {
        Some(p) => tr!(
            format!("優先度を {level} にした（待ち {p} 番目）"),
            format!("Priority set to {level} (#{p} in the queue)")
        ),
        None => tr!(
            format!("優先度を {level} にした"),
            format!("Priority set to {level}")
        ),
    }
}
pub fn pane_priority_changed(pane: u64, level: &str) -> String {
    tr!(
        format!("ペイン {pane} の優先度の既定を {level} にした"),
        format!("Pane {pane} default priority set to {level}")
    )
}
pub fn running_note() -> &'static str {
    tr!(
        "走っている処理は止めない（優先度は待ちの並びにだけ効く）",
        "Running jobs keep running (priority only orders the waiting jobs)"
    )
}
pub fn cancel() -> &'static str {
    tr!("取り消し", "Cancel")
}
pub fn cancel_tip() -> &'static str {
    tr!(
        "取り消す（tako queue cancel と同じ。確かめてから）",
        "Cancel (same as tako queue cancel; asks first)"
    )
}
pub fn cancel_confirm(label: &str, running: bool) -> String {
    if running {
        tr!(
            format!("「{label}」を止めて取り消しますか？"),
            format!("Stop and cancel “{label}”?")
        )
    } else {
        tr!(
            format!("「{label}」を待ちから外しますか？"),
            format!("Remove “{label}” from the queue?")
        )
    }
}
pub fn cancel_yes() -> &'static str {
    tr!("取り消す", "Cancel job")
}
pub fn cancel_no() -> &'static str {
    tr!("やめる", "Keep")
}
pub fn cancelled(label: &str) -> String {
    tr!(
        format!("「{label}」を取り消した"),
        format!("Cancelled “{label}”")
    )
}
pub fn op_failed(reason: &str) -> String {
    tr!(
        format!("できませんでした: {reason}"),
        format!("Failed: {reason}")
    )
}
pub fn focus_tip() -> &'static str {
    tr!("押すとこのペインへ移る", "Click to focus this pane")
}

// ===== ④ 常駐の内訳 =====

pub fn section_resident() -> &'static str {
    tr!(
        "常駐の内訳（メモリの多いペイン）",
        "Resident memory by pane"
    )
}
pub fn scan_unsupported() -> &'static str {
    tr!(
        "この OS ではプロセスの内訳を採れない（未対応）",
        "Per-process breakdown is not supported on this OS"
    )
}
pub fn resident_empty() -> &'static str {
    tr!("まだ走査していない", "Not scanned yet")
}
/// ペインの内訳（agent + MCP · 重い処理 · その他 = dev サーバー等 · CPU%）
pub fn resident_detail(
    agent: Option<&str>,
    agent_mb: u64,
    heavy_mb: u64,
    other_mb: u64,
    cpu: f64,
) -> String {
    let mut parts = Vec::new();
    if agent.is_some() || agent_mb > 0 {
        let name = agent.unwrap_or("agent");
        parts.push(tr!(
            format!("{name} + MCP {}", size(agent_mb)),
            format!("{name} + MCP {}", size(agent_mb))
        ));
    }
    if heavy_mb > 0 {
        parts.push(tr!(
            format!("重い処理 {}", size(heavy_mb)),
            format!("heavy {}", size(heavy_mb))
        ));
    }
    if other_mb > 0 {
        parts.push(tr!(
            format!("その他 {}", size(other_mb)),
            format!("other {}", size(other_mb))
        ));
    }
    parts.push(format!("CPU {cpu:.0}%"));
    parts.join(" · ")
}
pub fn more_panes(n: usize) -> String {
    tr!(
        format!("ほか {n} ペイン（全件は tako queue --json）"),
        format!("{n} more pane(s) (all in tako queue --json)")
    )
}
pub fn external(memory_mb: u64, processes: u64, heavy: u64) -> String {
    tr!(
        format!(
            "tako の外: {} · {processes} 本（重い処理 {heavy} 件）",
            size(memory_mb)
        ),
        format!(
            "Outside tako: {} · {processes} processes ({heavy} heavy)",
            size(memory_mb)
        )
    )
}
pub fn scan_line(processes: u64, age_ms: u64) -> String {
    let age = age_ms as f64 / 1000.0;
    tr!(
        format!("走査 {processes} 本 · {age:.1} 秒前"),
        format!("scanned {processes} processes · {age:.1}s ago")
    )
}

// ===== ステータスバーの区画 =====

/// 実行中・待ちの件数（1 件以上あるときだけ出す）
pub fn bar_counts(running: usize, waiting: usize) -> String {
    tr!(
        format!("実行 {running} · 待ち {waiting}"),
        format!("run {running} · wait {waiting}")
    )
}
/// 区画のツールチップ（使用 / 全体・メモリ圧・押すと開く）
pub fn bar_tip(used_mb: Option<u64>, total_mb: Option<u64>, p: Option<Pressure>) -> String {
    match (used_mb, total_mb) {
        (Some(used), Some(total)) => tr!(
            format!(
                "メモリ {} · {} · 押すと PC の負荷を開く",
                memory_used(used, total),
                pressure(p)
            ),
            format!(
                "Memory {} · {} · click to open PC load",
                memory_used(used, total),
                pressure(p)
            )
        ),
        _ => tr!(
            format!("メモリ {} · 押すと PC の負荷を開く", unsupported()),
            format!("Memory {} · click to open PC load", unsupported())
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests_support;
    use super::*;

    #[test]
    fn catalog_has_both_languages_and_no_emoji() {
        tests_support::check_ja_en(|| {
            vec![
                secs(5),
                secs(130),
                secs(7300),
                approx(20),
                approx(200),
                title().to_string(),
                loading().to_string(),
                updating().to_string(),
                refresh().to_string(),
                failed("x"),
                section_machine().to_string(),
                unsupported().to_string(),
                machine_unsupported().to_string(),
                memory_label().to_string(),
                memory_detail(3000, 12000, Some((1024, 2048))),
                pressure(Some(Pressure::Warn)),
                cpu_line(Some(23.0), 18, Some([4.1, 3.9, 3.5])),
                section_running(2, Some(3)),
                section_running(2, None),
                section_waiting(3),
                no_arbiter().to_string(),
                running_empty().to_string(),
                waiting_empty().to_string(),
                unmanaged(1),
                owner(Some(7), Some("dev"), Some("build"), Some("worker")),
                owner(None, None, None, None),
                shelved().to_string(),
                running_detail(Some(120.0), Some(1300), Some(200)),
                eta("remaining", Some(180), Some(420), None),
                eta("remaining", Some(180), None, Some((34, 120))),
                eta("remaining", Some(180), None, None),
                eta("overrun", None, Some(420), None),
                eta("unknown", None, None, None),
                stalled().to_string(),
                cancelling().to_string(),
                position(2),
                waited(130),
                start_eta(0),
                start_eta(240),
                wait_reason(Some("memory"), Some("空きメモリ不足")).unwrap_or_default(),
                priority_from(Some("pane"), false, None).unwrap_or_default(),
                priority_from(Some("role"), true, Some(Priority::Normal)).unwrap_or_default(),
                priority_tip().to_string(),
                priority_job_row().to_string(),
                priority_pane_row(7),
                priority_changed("high", Some(1)),
                priority_changed("high", None),
                pane_priority_changed(7, "low"),
                running_note().to_string(),
                cancel().to_string(),
                cancel_tip().to_string(),
                cancel_confirm("cargo test", true),
                cancel_confirm("cargo test", false),
                cancel_yes().to_string(),
                cancel_no().to_string(),
                cancelled("cargo test"),
                op_failed("x"),
                focus_tip().to_string(),
                section_resident().to_string(),
                scan_unsupported().to_string(),
                resident_empty().to_string(),
                resident_detail(Some("claude"), 2100, 800, 500, 12.0),
                more_panes(3),
                external(8200, 412, 1),
                scan_line(1320, 1200),
                bar_counts(2, 3),
                bar_tip(Some(24000), Some(36000), Some(Pressure::Normal)),
                bar_tip(None, None, None),
            ]
        });
    }

    /// 数値の区切りは CLI（`queue_wrapper::human_mb` / `human_secs` / `eta::approx`）と同じ
    #[test]
    fn 数値の区切りはcliと同じ() {
        let _g = tests_support::lang_guard();
        tako_core::i18n::set_lang(tako_core::i18n::Lang::Ja);
        for mb in [0, 512, 1023, 1024, 36864, 123_456] {
            assert_eq!(size(mb), tako_control::queue_wrapper::human_mb(mb), "{mb}");
        }
        for s in [0, 59, 60, 61, 3599, 3600, 7300] {
            assert_eq!(secs(s), tako_control::queue_wrapper::human_secs(s), "{s}");
        }
        for s in [0, 29, 59, 60, 89, 90, 600] {
            assert_eq!(approx(s), tako_core::queue::eta::approx(s), "{s}");
        }
        assert_eq!(start_eta(0), tako_core::queue::eta::start_text(0));
        assert_eq!(start_eta(240), tako_core::queue::eta::start_text(240));
    }

    /// 実行中の行は「数値（経過つき）· 見込み」を 1 行に並べるので、見込みの文で経過を繰り返さない
    /// （隔離 GUI の 1 回目で「経過 7 秒 · 経過 7 秒（見込みなし）」と二重に出た）
    #[test]
    fn 経過は見込みの文で繰り返さない() {
        let _g = tests_support::lang_guard();
        for (lang, word) in [
            (tako_core::i18n::Lang::Ja, "経過"),
            (tako_core::i18n::Lang::En, "elapsed"),
        ] {
            tako_core::i18n::set_lang(lang);
            for state in ["unknown", "overrun", "remaining"] {
                let line = [
                    running_detail(Some(0.0), Some(7), Some(420)),
                    eta(state, Some(60), Some(300), None),
                ]
                .join(" · ");
                assert_eq!(line.matches(word).count(), 1, "{lang:?} {state}: {line}");
            }
        }
    }

    /// 誰の欄は同じ語を重ねない（タブ名とペイン名が同じとき）・空の欄は飛ばす
    #[test]
    fn 誰の欄は重ねず空を飛ばす() {
        let _g = tests_support::lang_guard();
        tako_core::i18n::set_lang(tako_core::i18n::Lang::Ja);
        assert_eq!(
            owner(Some(7), Some("dev"), Some("dev"), Some("")),
            "ペイン 7 · dev"
        );
        assert_eq!(owner(None, Some("dev"), None, None), "ペインの外");
        tako_core::i18n::set_lang(tako_core::i18n::Lang::En);
        assert_eq!(
            owner(Some(7), Some("dev"), Some("w1"), Some("worker")),
            "pane 7 · dev · w1 · worker"
        );
    }
}
