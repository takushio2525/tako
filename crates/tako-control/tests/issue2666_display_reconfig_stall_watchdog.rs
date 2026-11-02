//! UI ストールの分類が画面構成の変更（macOS の画面再構成）を名指すかの番犬（#2666）
//!
//! # なぜ要るか
//!
//! 蓋の開閉・画面スリープで内蔵ディスプレイが画面構成から出入りする瞬間に UI が 0.5〜1.8 秒止まる。
//! 塞がりは tako の計測区間の外（AppKit / GPUI の画面の当て直し・Metal の提示）なので、#2665 の分類は
//! 「計測区間の外か再開経路側の遅延」としか書けなかった（本番 perf.log の 29 件中 18 件）。
//! OS の通知（`CGDisplayRegisterReconfigurationCallback` の begin / end）の時刻を記録し、遅延の間に
//! 重なったら「画面構成の変更の処理中（OS / GPUI 側）」と名指す。
//!
//! # 何を縛るか
//!
//! 1. 実経路（`diag::stall_line` = GUI の監視ループが呼ぶ 1 実装・本物の `perf_span` と記録の口）で:
//!    - 再構成が無ければ #2665 の分類のまま（専有・計測区間の外）
//!    - begin → 塞ぐ → end / begin → end → 塞ぐ（end の後の猶予の内）は「画面構成の変更の処理中」
//!    - begin → 計測区間で塞ぐ → end は「tako の専有」と名指したまま「画面構成の変更とも重なる」と併記
//!    - 終わった再構成は 1 回につき 1 行の perf.log の行になる（監視スレッドが取り出す）
//! 2. ソースの形: tako-app が起動時に登録し・コールバックが記録の口を通り・合成が本物のコールバックの
//!    関数を通る / diag の記録が畳み・既定の経路が遅延の間の再構成で分類し・分類に新しい腕があり・
//!    end の後の猶予まで窓を延ばし・監視スレッドが行を取り出し・注入の口が検証プロセスの門の内側にある
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`逆戻りを名指しできる`] で修正前の形を再現した注入を 1 本ずつ当て、`file:line` で名指されることを確かめる。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tako_control::diag::{self, ReconfigEvent, ReconfigPhase};

#[path = "common/production_range.rs"]
mod production_range;

const DIAG: &str = "crates/tako-control/src/diag.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const PLAT: &str = "crates/tako-app/src/platform/display_reconfig.rs";

// ===== 1. 実経路（本物の perf_span・記録の口・監視ループの 1 周の判定） =====

fn sleep_until(t: Instant) {
    if let Some(d) = t.checked_duration_since(Instant::now()) {
        std::thread::sleep(d);
    }
}

/// 画面 2 枚ぶんの通知（GUI のコールバックと同じ口 `record_display_reconfig` を通る）
fn notify(phase: ReconfigPhase) {
    for display in [1, 2] {
        diag::record_display_reconfig(ReconfigEvent {
            phase,
            display,
            flags: if phase == ReconfigPhase::Begin {
                1
            } else {
                1 << 12
            },
            main_thread: true,
            synthetic: true,
        });
    }
}

/// `armed` から `at` ms の位置で `f` を始め、`until` ms まで塞いでから監視ループの 1 周の判定を返す
fn play(at: u64, until: u64, f: impl FnOnce(Instant)) -> String {
    let armed = Instant::now();
    let period = Duration::from_secs(1);
    sleep_until(armed + Duration::from_millis(at));
    f(armed + Duration::from_millis(until));
    sleep_until(armed + Duration::from_millis(until));
    diag::stall_line(armed, period).expect("遅延 0.5 秒以上は記録される")
}

#[test]
fn 実経路_再構成が重なれば名指し_無ければ_2665_の分類のまま() {
    assert!(
        !diag::issue2666_legacy() && !diag::issue2665_legacy() && !diag::issue1968_legacy(),
        "A/B の env（TAKO_2666_LEGACY / TAKO_2665_LEGACY / TAKO_1968_LEGACY）を外して走らせる"
    );
    // この 1 本だけが perf_span と再構成の記録を使う（どちらもプロセス全体に効く）
    diag::mark_main_thread();

    // 再構成なし: 計測区間で 1.5 秒（期限は区間の中）= #2665 の「tako の専有」のまま
    let line = play(500, 2000, |_| {
        let _span = diag::perf_span("t2666-span");
        sleep_until(Instant::now() + Duration::from_millis(1500));
    });
    assert!(line.contains("tako の専有"), "{line}");
    assert!(
        !line.contains("画面構成"),
        "再構成が無いのに併記した: {line}"
    );
    // 再構成なし: 計測区間の外で塞ぐ = #2665 の「計測区間の外」のまま
    let line = play(300, 1800, |_| {});
    assert!(
        line.contains("遅延の間に計測区間なし = 計測区間の外"),
        "{line}"
    );
    assert!(!line.contains("画面構成"), "{line}");
    assert!(
        diag::take_reconfig_reports().is_empty(),
        "再構成が無ければ行も無い"
    );

    // begin → 塞ぐ → end（合成の `begin sleep=… end`）
    let line = play(300, 1800, |until| {
        notify(ReconfigPhase::Begin);
        sleep_until(until);
        notify(ReconfigPhase::End);
    });
    assert!(
        line.contains(
            "遅延の間に計測区間なし = 画面構成の変更の処理中（OS / GPUI 側）: begin → end "
        ),
        "{line}"
    );
    assert!(line.contains("・検証の合成。#2666）"), "{line}");
    assert!(!line.contains("tako の専有"), "{line}");
    // 終わった再構成は settle の後に 1 行（監視スレッドが取り出す口）
    std::thread::sleep(Duration::from_millis(350));
    let reports = diag::take_reconfig_reports();
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert!(
        reports[0].starts_with("画面構成の変更（macOS の画面再構成）: begin → end ")
            && reports[0].contains("画面 1,2・通知 begin 2 件 / end 2 件・変更 desktop_shape・コールバックはメインスレッド・検証の合成。#2666"),
        "{reports:?}"
    );
    assert!(diag::take_reconfig_reports().is_empty(), "1 回につき 1 行");

    // begin → end の直後に塞ぐ（本番の形 = end の後に塞がる）: 猶予の内なので名指す
    let line = play(300, 1800, |until| {
        notify(ReconfigPhase::Begin);
        notify(ReconfigPhase::End);
        sleep_until(until);
    });
    assert!(
        line.contains("画面構成の変更の処理中（OS / GPUI 側）"),
        "{line}"
    );
    assert!(line.contains("end から記録まで "), "{line}");

    // begin → 計測区間で塞ぐ → end: 専有と名指したまま併記する（優先の決まり）
    let line = play(300, 1800, |until| {
        notify(ReconfigPhase::Begin);
        {
            let _span = diag::perf_span("t2666-span-reconfig");
            sleep_until(until);
        }
        notify(ReconfigPhase::End);
    });
    assert!(line.contains("tako の専有"), "{line}");
    assert!(line.contains("最長 t2666-span-reconfig"), "{line}");
    assert!(
        line.contains("。画面構成の変更とも重なる: begin → end "),
        "{line}"
    );

    // 猶予（end の 2 秒後）を過ぎてからのストールは名指さない
    std::thread::sleep(diag::RECONFIG_AFTER_GRACE + Duration::from_millis(200));
    let line = play(300, 1800, |_| {});
    assert!(
        line.contains("遅延の間に計測区間なし = 計測区間の外"),
        "猶予の外: {line}"
    );
}

// ===== 2. ソースの形 =====

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let src = std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    // テスト領域を空白へ潰す（行番号は保たれる。#1420）
    production_range::production(&src, rel)
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// `anchor` を含む行より後ろで最初に `needle` を含む関数の窓（宣言行の 0-based 位置と、宣言行から
/// 閉じ括弧までの行）。終わりは宣言行と同じ字下げの `}`
fn fn_window<'a>(lines: &[&'a str], anchor: &str, needle: &str) -> Option<(usize, Vec<&'a str>)> {
    let from = if anchor.is_empty() {
        0
    } else {
        lines
            .iter()
            .position(|l| !is_comment(l) && l.contains(anchor))?
    };
    let start =
        (from..lines.len()).find(|&i| !is_comment(lines[i]) && lines[i].contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = (start + 1..lines.len())
        .find(|&i| lines[i] == close)
        .unwrap_or(lines.len() - 1);
    Some((start, lines[start..=end].to_vec()))
}

/// 関数の窓の中にコードとして `needle` があるか。無ければ宣言行を名指す
fn require(
    out: &mut Vec<Offender>,
    file: &'static str,
    lines: &[&str],
    (anchor, func): (&str, &str),
    needle: &str,
    why: &str,
) {
    match fn_window(lines, anchor, func) {
        None => out.push(Offender {
            file,
            line: 0,
            why: format!("`{func}` が見つからない（走査が空振り）"),
        }),
        Some((at, w)) => {
            if !w.iter().any(|l| !is_comment(l) && l.contains(needle)) {
                out.push(Offender {
                    file,
                    line: at + 1,
                    why: format!("{why}（`{needle}` が無い。#2666）"),
                });
            }
        }
    }
}

fn scan_plat(src: &str) -> Vec<Offender> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    require(
        &mut out,
        PLAT,
        &lines,
        ("mod imp {", "pub(super) fn watch("),
        "CGDisplayRegisterReconfigurationCallback(on_reconfig,",
        "macOS の `watch` が画面構成の変更の通知を登録していない（記録が来ない = 分類が名指せない）",
    );
    require(
        &mut out,
        PLAT,
        &lines,
        ("mod imp {", "pub(super) fn watch("),
        "issue2666_legacy()",
        "登録が A/B（`TAKO_2666_LEGACY=1`）で外れない",
    );
    require(
        &mut out,
        PLAT,
        &lines,
        ("", "extern \"C\" fn on_reconfig("),
        "diag::record_display_reconfig(",
        "コールバックが記録の口 `diag::record_display_reconfig` を通らない（記録を消している）",
    );
    require(
        &mut out,
        PLAT,
        &lines,
        ("", "extern \"C\" fn on_reconfig("),
        "pthread_main_np()",
        "コールバックが来たスレッドを記録しない（本番でメインスレッドか確かめられない）",
    );
    require(
        &mut out,
        PLAT,
        &lines,
        ("mod imp {", "pub(super) fn synthesize("),
        "on_reconfig(",
        "検証の合成が本物のコールバックの関数を通らない（実経路の検証が測るものが変わる）",
    );
    out
}

fn scan_diag(src: &str) -> Vec<Offender> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    require(
        &mut out,
        DIAG,
        &lines,
        ("", "pub fn record_display_reconfig("),
        "apply_reconfig(&mut q, &ev, now)",
        "記録の口が通知を履歴へ畳んでいない（記録を消している）",
    );
    require(
        &mut out,
        DIAG,
        &lines,
        ("", "pub fn stall_line("),
        ".then(|| reconfig_overlapping_since(armed + period))",
        "既定の経路が遅延の間（期限から）の画面構成の変更で分類していない（分類の腕を外している）",
    );
    require(
        &mut out,
        DIAG,
        &lines,
        ("", "pub fn stall_line("),
        "StallCause::classify(lag, sched_lag, span, delay, reconfig)",
        "既定の経路が画面構成の変更を `StallCause::classify` へ渡していない",
    );
    require(
        &mut out,
        DIAG,
        &lines,
        ("impl StallCause {", "pub fn classify("),
        "Self::DisplayReconfig {",
        "分類に画面構成の変更の腕（`StallCause::DisplayReconfig`）が無い",
    );
    require(
        &mut out,
        DIAG,
        &lines,
        ("impl ReconfigOverlap {", "pub fn summarize("),
        "Some(e) => e + RECONFIG_AFTER_GRACE,",
        "画面構成の変更の窓を end で切っている（本番の塞がりは end の後に来る）",
    );
    require(
        &mut out,
        DIAG,
        &lines,
        ("", "pub fn spawn_stall_watchdog("),
        "take_reconfig_reports()",
        "監視スレッドが終わった画面構成の変更を perf.log の行にしていない",
    );
    // 注入の口は検証プロセスの門の内側（本番の GUI は env があっても UI スレッドを止めない）
    match fn_window(&lines, "impl Issue2666Inject {", "pub fn for_request(") {
        None => out.push(Offender {
            file: DIAG,
            line: 0,
            why: "注入の口 `Issue2666Inject::for_request` が見つからない（走査が空振り）".into(),
        }),
        Some((at, w)) => {
            let code_at =
                |pred: &dyn Fn(&str) -> bool| w.iter().position(|l| !is_comment(l) && pred(l));
            let gate = code_at(&|l| l.contains("is_verification_process()"));
            let env =
                code_at(&|l| l.contains("env::var") && l.contains("ISSUE2666_INJECT_FILE_ENV"));
            match (gate, env) {
                (_, None) => out.push(Offender {
                    file: DIAG,
                    line: at + 1,
                    why: "注入の口が env（`ISSUE2666_INJECT_FILE_ENV`）を読む行が見つからない（走査が空振り）"
                        .into(),
                }),
                (Some(g), Some(e)) if g < e => {}
                (_, Some(e)) => out.push(Offender {
                    file: DIAG,
                    line: at + e + 1,
                    why: "注入の口（`TAKO_2666_INJECT_FILE`）が検証プロセスの門（`is_verification_process()`）を \
                          通らずに env を読む（本番の GUI でも env だけで UI スレッドを止められる。#2666 / #2665）"
                        .into(),
                }),
            }
        }
    }
    out
}

fn scan_main(src: &str) -> Vec<Offender> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let code_at = |needle: &str| -> Vec<usize> {
        lines
            .iter()
            .enumerate()
            .filter(|(_, l)| !is_comment(l) && l.contains(needle))
            .map(|(i, _)| i)
            .collect()
    };
    // GUI が起動時に登録する（UI ストールの監視ループの手前 = メインスレッド）
    let watch = code_at("crate::platform::display_reconfig::watch();");
    let loop_at = code_at("diag::stall_line(armed, period)");
    if !watch
        .iter()
        .any(|&w| loop_at.iter().any(|&l| w < l && l - w <= 12))
    {
        out.push(Offender {
            file: MAIN,
            line: loop_at.first().map_or(0, |l| l + 1),
            why: "GUI が UI ストールの監視ループの手前で画面構成の変更の通知を登録していない \
                  （`crate::platform::display_reconfig::watch();`。記録が来ない。#2666）"
                .into(),
        });
    }
    // 注入の口: `ipc_turn` の前（#2665 の外の塞ぎの手前）で合成する
    let run = code_at(".run(crate::platform::display_reconfig::synthesize);");
    let outside = code_at("inject_2665.block_out_of_span()");
    if !run
        .iter()
        .any(|&r| outside.iter().any(|&o| r < o && o - r <= 3))
    {
        out.push(Offender {
            file: MAIN,
            line: outside.first().map_or(0, |o| o + 1),
            why: "#2666 の注入の口（`Issue2666Inject::for_request(…).run(crate::platform::display_reconfig::synthesize)`）が \
                  `ipc_turn` の前に無い（実経路の検証が測るものが変わる）"
                .into(),
        });
    }
    out
}

fn all_offenders(diag_src: &str, main_src: &str, plat_src: &str) -> Vec<Offender> {
    let mut v = scan_diag(diag_src);
    v.extend(scan_main(main_src));
    v.extend(scan_plat(plat_src));
    v
}

#[test]
fn ソースの形が画面構成の変更を名指す形を守っている() {
    let offenders = all_offenders(&read(DIAG), &read(MAIN), &read(PLAT));
    let list: Vec<String> = offenders.iter().map(Offender::report).collect();
    assert!(
        list.is_empty(),
        "UI ストールの分類が画面構成の変更を名指さない形へ戻っている（#2666）:\n  {}",
        list.join("\n  ")
    );
}

/// 置換 1 つぶんの注入（修正前の形へ戻す）
struct Injection {
    name: &'static str,
    file: &'static str,
    from: &'static str,
    to: &'static str,
    /// 名指しの理由に含まれるべき語
    expect: &'static str,
}

const INJECTIONS: &[Injection] = &[
    Injection {
        name: "記録を消す（コールバックが記録の口を呼ばない）",
        file: PLAT,
        from: "diag::record_display_reconfig(ReconfigEvent {",
        to: "drop(ReconfigEvent {",
        expect: "record_display_reconfig",
    },
    Injection {
        name: "記録を消す（記録の口が畳まない）",
        file: DIAG,
        from: "    apply_reconfig(&mut q, &ev, now);\n",
        to: "    let _ = (&mut q, &ev, now);\n",
        expect: "apply_reconfig",
    },
    Injection {
        name: "記録を消す（macOS で登録しない）",
        file: PLAT,
        from: "CGDisplayRegisterReconfigurationCallback(on_reconfig, std::ptr::null_mut())",
        to: "0",
        expect: "CGDisplayRegisterReconfigurationCallback",
    },
    Injection {
        name: "記録を消す（GUI が登録を呼ばない）",
        file: MAIN,
        from: "        crate::platform::display_reconfig::watch();\n",
        to: "",
        expect: "display_reconfig::watch",
    },
    Injection {
        name: "記録を消す（来たスレッドを書かない）",
        file: PLAT,
        from: "main_thread: unsafe { pthread_main_np() } != 0,",
        to: "main_thread: true,",
        expect: "pthread_main_np",
    },
    Injection {
        name: "分類の腕を外す（既定の経路が再構成を見ない）",
        file: DIAG,
        from: ".then(|| reconfig_overlapping_since(armed + period))",
        to: ".then(|| None::<ReconfigOverlap>)",
        expect: "reconfig_overlapping_since",
    },
    Injection {
        name: "分類の腕を外す（再構成を分類へ渡さない）",
        file: DIAG,
        from: "StallCause::classify(lag, sched_lag, span, delay, reconfig)",
        to: "StallCause::classify(lag, sched_lag, span, delay, None)",
        expect: "StallCause::classify",
    },
    Injection {
        name: "分類の腕を外す（DisplayReconfig を作らない）",
        file: DIAG,
        from: "            (spans, Some(reconfig)) => Self::DisplayReconfig {\n",
        to: "            (spans, Some(reconfig)) => Self::Unmeasured {\n",
        expect: "DisplayReconfig",
    },
    Injection {
        name: "窓を end で切る",
        file: DIAG,
        from: "Some(e) => e + RECONFIG_AFTER_GRACE,",
        to: "Some(e) => e,",
        expect: "RECONFIG_AFTER_GRACE",
    },
    Injection {
        name: "perf.log の再構成の行を出さない",
        file: DIAG,
        from: "for line in take_reconfig_reports() {",
        to: "for line in Vec::<String>::new() {",
        expect: "take_reconfig_reports",
    },
    Injection {
        name: "注入の口の検証プロセスの門を外す",
        file: DIAG,
        from: "            tako_core::paths::is_verification_process()\n                .then(|| std::env::var_os(ISSUE2666_INJECT_FILE_ENV).map(PathBuf::from))\n                .flatten()\n",
        to: "            std::env::var_os(ISSUE2666_INJECT_FILE_ENV).map(PathBuf::from)\n",
        expect: "is_verification_process",
    },
    Injection {
        name: "合成が本物のコールバックを通らない",
        file: PLAT,
        from: "        on_reconfig(\n            display,\n            flags,\n            std::ptr::addr_of!(SYNTHETIC_MARK).cast_mut().cast(),\n        );\n",
        to: "        let _ = (display, flags);\n",
        expect: "on_reconfig(",
    },
    Injection {
        name: "注入の口を ipc_turn の前に置かない",
        file: MAIN,
        from: "                .run(crate::platform::display_reconfig::synthesize);\n",
        to: "                ;\n",
        expect: "synthesize",
    },
];

#[test]
fn 逆戻りを名指しできる() {
    let diag_src = read(DIAG);
    let main_src = read(MAIN);
    let plat_src = read(PLAT);
    assert!(
        all_offenders(&diag_src, &main_src, &plat_src).is_empty(),
        "注入の前提: 現在のソースは緑"
    );
    for inj in INJECTIONS {
        let target = match inj.file {
            DIAG => &diag_src,
            MAIN => &main_src,
            _ => &plat_src,
        };
        assert_eq!(
            target.matches(inj.from).count(),
            1,
            "注入「{}」の置換元がちょうど 1 か所ある",
            inj.name
        );
        let swap = |file: &str, src: &String| {
            if file == inj.file {
                src.replacen(inj.from, inj.to, 1)
            } else {
                src.clone()
            }
        };
        let offenders = all_offenders(
            &swap(DIAG, &diag_src),
            &swap(MAIN, &main_src),
            &swap(PLAT, &plat_src),
        );
        let hit = offenders
            .iter()
            .find(|o| o.file == inj.file && o.line > 0 && o.why.contains(inj.expect));
        assert!(
            hit.is_some(),
            "注入「{}」を名指しできない（違反: {:?}）",
            inj.name,
            offenders.iter().map(Offender::report).collect::<Vec<_>>()
        );
    }
}
