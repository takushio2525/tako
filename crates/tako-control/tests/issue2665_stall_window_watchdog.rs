//! UI ストールの分類が「遅延の間」に重なった区間だけで決まるかの番犬（#2665）
//!
//! # なぜ要るか
//!
//! UI ストールは 1 秒タイマーの再開遅延で測る。監視ループ自身が UI スレッドで走るので、記録の時点で
//! 塞いでいた区間は終わっている（#1968）。そこで終わった区間の履歴で分類するが、#2665 より前は
//! その窓を「1 秒 + 遅延」= **タイマーを掛けた時点まで**遡って取り、区間の全長を数えていた。
//! 期限より前に終わった区間はタイマーの再開を遅らせていない（その間メインスレッドは空いていた）のに、
//! それが遅延の半分を超えると「tako の専有」と名指していた。隔離 GUI の注入（計測区間の中 300ms の
//! 直後に計測区間の外 900ms）では、記録された 4 件が 4/4 とも「ipc_turn 302ms = tako の専有」だった
//! （本当の原因は計測区間の外の 900ms）。また半分に届かないと内訳を捨てて「計測区間の外」とだけ書いていた。
//!
//! # 何を縛るか
//!
//! 1. 実経路（`diag::stall_line` = GUI の監視ループが呼ぶ 1 実装・本物の `perf_span`）で:
//!    - 中 300ms → 外 900ms（期限は外の中）を「tako の専有」と名指さない
//!    - 中 1500ms（期限は区間の中）は従来どおり「tako の専有」と区間名で名指す
//!    - 中 3000ms は 2 秒の時点で「長時間専有（継続中）」の中間報告が出て、記録は「tako の専有」
//! 2. 半分に届かないときは内訳（区間・遅延の何 %）と残り（計測区間の外）を書く（騙らない・捨てない）
//! 3. ソースの形: 既定の経路の窓は期限（`armed + period`）から・数えるのははみ出した分だけ・
//!    修正前の窓（1 秒 + 遅延）は A/B（`TAKO_2665_LEGACY=1`）の枝の中だけ・監視スレッドが中間報告を
//!    取り出す・GUI の注入の口（`TAKO_2665_INJECT_FILE`）が計測区間の外 / 中の正しい側にある
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`逆戻りを名指しできる`] で修正前の形を再現した注入を 1 本ずつ当て、`file:line` で名指されることを確かめる。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tako_control::diag::{self, DelaySpans, RecentSpans, StallCause};
use tako_core::i18n::Lang;

#[path = "common/production_range.rs"]
mod production_range;

const DIAG: &str = "crates/tako-control/src/diag.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";

// ===== 1. 実経路（本物の perf_span と監視ループの 1 周の判定） =====

fn sleep_until(t: Instant) {
    if let Some(d) = t.checked_duration_since(Instant::now()) {
        std::thread::sleep(d);
    }
}

/// 注入「中 `in_ms` → 外 `out_ms`」を `armed` からの位置で再現し、監視ループの 1 周の判定を返す。
/// 区間が期限より前に終わらなかった（負荷で押された）ときは `None`（前提が崩れたので数えない）
fn play_in_then_out(
    armed: Instant,
    period: Duration,
    in_at: u64,
    in_ms: u64,
    out_ms: u64,
) -> Option<String> {
    sleep_until(armed + Duration::from_millis(in_at));
    {
        let _span = diag::perf_span("t2665-in-then-out");
        sleep_until(armed + Duration::from_millis(in_at + in_ms));
    }
    let span_end = Instant::now();
    // 計測区間の外で塞ぐ（GUI の注入の `out=` と同じ = 区間を開かずに UI スレッドを止める）
    sleep_until(span_end + Duration::from_millis(out_ms));
    let line = diag::stall_line(armed, period);
    (span_end < armed + period).then_some(line).flatten()
}

#[test]
fn 実経路_期限より前に終わった区間を専有と名指さず_期限をまたぐ区間は名指す() {
    assert!(
        !diag::issue2665_legacy() && !diag::issue1968_legacy(),
        "A/B の env（TAKO_2665_LEGACY / TAKO_1968_LEGACY）を外して走らせる"
    );
    // この 1 本だけが perf_span を使う（登録はプロセス全体に効くので、他のテストは区間を開かない）
    diag::mark_main_thread();
    let period = Duration::from_secs(1);

    // 中 300ms（0.35〜0.65 秒）→ 外 900ms（期限 1.0 秒は外の 0.35 秒の位置 = 遅延 0.55 秒）。負荷で前提が崩れたら 3 回まで
    let line = (0..3)
        .find_map(|_| play_in_then_out(Instant::now(), period, 350, 300, 900))
        .expect("3 回とも区間が期限の後ろへ押された（負荷が高すぎる）");
    assert!(
        !line.contains("tako の専有"),
        "期限より前に終わった区間で騙った: {line}"
    );
    assert!(!line.contains("t2665-in-then-out"), "{line}");
    assert!(
        line.contains("遅延の間に計測区間なし = 計測区間の外"),
        "{line}"
    );

    // 中 1500ms（0.5 秒の位置から = 期限は区間の中）は従来どおり名指す
    let armed = Instant::now();
    sleep_until(armed + Duration::from_millis(500));
    {
        let _span = diag::perf_span("t2665-in1500");
        sleep_until(armed + Duration::from_millis(2000));
    }
    let line = diag::stall_line(armed, period).expect("遅延 1 秒は記録される");
    assert!(line.contains("tako の専有"), "{line}");
    assert!(line.contains("最長 t2665-in1500"), "{line}");

    // 中 3000ms: 2 秒の時点で中間報告（監視スレッドが取り出す口）が 1 回だけ出て、記録は専有
    let armed = Instant::now();
    sleep_until(armed + Duration::from_millis(200));
    let watcher = std::thread::spawn(move || {
        sleep_until(armed + Duration::from_millis(2600));
        let first = diag::take_hang_report();
        let second = diag::take_hang_report();
        (first, second)
    });
    {
        let _span = diag::perf_span("t2665-in3000");
        sleep_until(armed + Duration::from_millis(3200));
    }
    let (first, second) = watcher.join().expect("中間報告の係");
    let first = first.expect("2 秒を超えた区間の中間報告が出る");
    assert!(
        first.starts_with("メインスレッド長時間専有（継続中）: t2665-in3000 が "),
        "{first}"
    );
    assert_eq!(second, None, "中間報告は区間ごとに 1 回だけ");
    let line = diag::stall_line(armed, period).expect("遅延 2.2 秒は記録される");
    assert!(line.contains("tako の専有"), "{line}");
    assert!(line.contains("最長 t2665-in3000"), "{line}");
    assert_eq!(
        diag::take_hang_report(),
        None,
        "区間が終われば中間報告は出ない"
    );
}

// ===== 2. 分類と文（時刻を渡す純粋関数） =====

#[test]
fn 期限より前に終わった区間は旧の窓では専有_新しい窓では外() {
    let armed = Instant::now() + Duration::from_secs(5);
    let deadline = armed + Duration::from_secs(1);
    // 中 300ms（0.3〜0.6 秒）→ 外 900ms（期限は外の中）。記録は期限の 0.55 秒後
    let ended = armed + Duration::from_millis(600);
    let lag = Duration::from_millis(550);
    let spans = [("ipc_turn", ended, 300)];

    let new = StallCause::classify(
        lag,
        Duration::ZERO,
        None,
        DelaySpans::summarize(spans, deadline),
    );
    assert!(!new.names_tako(), "{new:?}");
    assert!(new.text_in(Lang::Ja).contains("計測区間の外"));

    // 修正前の窓（タイマーを掛けた時点から）と文で同じ材料を読むと専有と名指す = この番犬が防ぐ逆戻り
    let old = diag::classify_stall(
        lag,
        Duration::ZERO,
        None,
        Some(&RecentSpans {
            longest_tag: "ipc_turn".into(),
            longest_ms: 300,
            total_ms: 300,
            count: 1,
        }),
    );
    assert!(old.contains("tako の専有"), "{old}");
}

#[test]
fn 半分に届かなければ内訳と残りを書く() {
    let armed = Instant::now() + Duration::from_secs(5);
    let deadline = armed + Duration::from_secs(1);
    // 外 1200ms（期限は 0.3 秒の位置）の直後に中 400ms = 遅延 1.3 秒のうち 400ms
    let spans = [("ipc_turn", deadline + Duration::from_millis(1300), 400)];
    let c = StallCause::classify(
        Duration::from_millis(1300),
        Duration::ZERO,
        None,
        DelaySpans::summarize(spans, deadline),
    );
    let ja = c.text_in(Lang::Ja);
    assert!(
        !c.names_tako() && !ja.contains("tako の専有"),
        "騙らない: {ja}"
    );
    assert!(ja.contains("計 400ms・1 件 = 遅延の 30%"), "{ja}");
    assert!(ja.contains("最長 ipc_turn"), "{ja}");
    assert!(ja.contains("残り 900ms は計測区間の外"), "捨てない: {ja}");
}

// ===== 3. ソースの形 =====

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

/// 関数の窓（宣言行の 0-based 位置と、宣言行から閉じ括弧までの行）。終わりは宣言行と同じ字下げの `}`
fn fn_window<'a>(lines: &[&'a str], needle: &str) -> Option<(usize, Vec<&'a str>)> {
    let start = lines
        .iter()
        .position(|l| !is_comment(l) && l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = (start + 1..lines.len())
        .find(|&i| lines[i] == close)
        .unwrap_or(lines.len() - 1);
    Some((start, lines[start..=end].to_vec()))
}

fn scan_diag(src: &str) -> Vec<Offender> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let off = |line: usize, why: &str| Offender {
        file: DIAG,
        line,
        why: why.into(),
    };

    // 監視ループの 1 周の判定: 既定の経路は期限からの窓・修正前の窓は A/B の枝の中だけ
    match fn_window(&lines, "pub fn stall_line(") {
        None => out.push(off(
            0,
            "`pub fn stall_line(` が見つからない（走査が空振り）",
        )),
        Some((at, w)) => {
            let legacy = w.iter().position(|l| l.contains("if issue2665_legacy() {"));
            let legacy_end = legacy.and_then(|i| {
                let indent = w[i].len() - w[i].trim_start().len();
                let close = format!("{}}}", " ".repeat(indent));
                (i + 1..w.len()).find(|&j| w[j] == close)
            });
            let in_legacy =
                |i: usize| matches!((legacy, legacy_end), (Some(a), Some(b)) if a < i && i < b);
            if legacy.is_none() || legacy_end.is_none() {
                out.push(off(
                    at + 1,
                    "A/B（`TAKO_2665_LEGACY=1`）の枝 `if issue2665_legacy() {` が見つからない",
                ));
            }
            let mut deadline_window = false;
            for (i, l) in w.iter().enumerate() {
                if is_comment(l) {
                    continue;
                }
                if l.contains("recent_spans_within(") && !in_legacy(i) {
                    out.push(off(
                        at + i + 1,
                        "既定の経路が修正前の窓（`recent_spans_within` = 1 秒 + 遅延・タイマーを掛けた時点まで \
                         遡る）を使っている（期限より前に終わった区間を専有と名指す = #2665）",
                    ));
                }
                if l.contains("spans_overlapping_since(") {
                    if l.contains("spans_overlapping_since(armed + period)") {
                        deadline_window = true;
                    } else {
                        out.push(off(
                            at + i + 1,
                            "遅延の間の窓が期限（`armed + period`）から始まらない（期限より前に終わった区間まで \
                             数える = #2665）",
                        ));
                    }
                }
            }
            if !deadline_window && !out.iter().any(|o| o.why.contains("armed + period")) {
                out.push(off(
                    at + 1,
                    "既定の経路が遅延の間の区間（`spans_overlapping_since(armed + period)`）で分類していない",
                ));
            }
            if !w
                .iter()
                .any(|l| !is_comment(l) && l.contains("StallCause::classify("))
            {
                out.push(off(
                    at + 1,
                    "既定の経路が `StallCause::classify(` を通らない",
                ));
            }
        }
    }

    // はみ出した分だけ数える（期限より前の部分を切る）
    match fn_window(&lines, "pub fn summarize<") {
        None => out.push(off(
            0,
            "`DelaySpans::summarize` が見つからない（走査が空振り）",
        )),
        Some((at, w)) => {
            let code = |needle: &str| w.iter().any(|l| !is_comment(l) && l.contains(needle));
            if !code("if ended <= since") {
                out.push(off(
                    at + 1,
                    "期限より前に終わった区間を飛ばしていない（`if ended <= since`。#2665）",
                ));
            }
            if !code("started.max(since)") {
                out.push(off(
                    at + 1,
                    "期限をまたぐ区間の全長を数えている（はみ出した分 `started.max(since)` に切らない。#2665）",
                ));
            }
        }
    }

    // 監視スレッドが中間報告を取り出す
    match fn_window(&lines, "pub fn spawn_stall_watchdog(") {
        None => out.push(off(
            0,
            "`spawn_stall_watchdog` が見つからない（走査が空振り）",
        )),
        Some((at, w)) => {
            if !w
                .iter()
                .any(|l| !is_comment(l) && l.contains("take_hang_report()"))
            {
                out.push(off(
                    at + 1,
                    "監視スレッドが長時間専有の中間報告（`take_hang_report()`）を取り出していない",
                ));
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
    // GUI が窓を自前で組まない（窓の正本は `diag::stall_line`）
    for i in code_at("recent_spans_within(")
        .into_iter()
        .chain(code_at("spans_overlapping_since("))
    {
        out.push(Offender {
            file: MAIN,
            line: i + 1,
            why: "UI ストールの窓を GUI 側で組んでいる（`recent_spans_within` / `spans_overlapping_since`）。\
                  窓の正本は `diag::stall_line`（#2665）"
                .into(),
        });
    }
    if code_at("diag::stall_line(armed, period)").is_empty() {
        out.push(Offender {
            file: MAIN,
            line: 0,
            why: "監視ループの `diag::stall_line(armed, period)` が見つからない（走査が空振り）"
                .into(),
        });
    }
    // 注入の口: 外は `ipc_turn` の前、中は `ipc_turn` の区間を開いた直後
    let span = code_at("perf_span(\"ipc_turn\")");
    let outside = code_at("inject_2665.block_out_of_span()");
    let inside = code_at("inject_2665.block_in_span()");
    let after = code_at("inject_2665.block_after_span()");
    let placed = inside.iter().any(|&i| {
        span.iter().any(|&s| s < i && i - s <= 2)
            && outside.iter().any(|&o| o < i && i - o <= 20)
            && after.iter().any(|&a| a > i)
    });
    if !placed {
        let at = span.first().map_or(0, |s| s + 1);
        out.push(Offender {
            file: MAIN,
            line: at,
            why: "#2665 の注入の口（`inject_2665.block_out_of_span()` を `ipc_turn` の前・\
                  `inject_2665.block_in_span()` を区間を開いた直後・`inject_2665.block_after_span()` を\
                  区間が閉じた後）が揃っていない（実経路の検証が測るものが変わる）"
                .into(),
        });
    }
    out
}

fn all_offenders(diag_src: &str, main_src: &str) -> Vec<Offender> {
    let mut v = scan_diag(diag_src);
    v.extend(scan_main(main_src));
    v
}

#[test]
fn ソースの形が遅延の間の窓を守っている() {
    let offenders = all_offenders(&read(DIAG), &read(MAIN));
    let list: Vec<String> = offenders.iter().map(Offender::report).collect();
    assert!(
        list.is_empty(),
        "UI ストールの分類が修正前の形へ戻っている（#2665）:\n  {}",
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
        name: "窓をタイマーを掛けた時点まで戻す",
        file: DIAG,
        from: "spans_overlapping_since(armed + period)",
        to: "spans_overlapping_since(armed)",
        expect: "armed + period",
    },
    Injection {
        name: "既定の経路で修正前の窓を使う",
        file: DIAG,
        from: ".then(|| spans_overlapping_since(armed + period))",
        to: ".then(|| recent_spans_within(period + lag).and(spans_overlapping_since(armed + period)))",
        expect: "1 秒 + 遅延",
    },
    Injection {
        name: "期限をまたぐ区間の全長を数える",
        file: DIAG,
        from: "ended.duration_since(started.max(since))",
        to: "ended.duration_since(started)",
        expect: "started.max(since)",
    },
    Injection {
        name: "期限より前に終わった区間を飛ばさない",
        file: DIAG,
        from: "            if ended <= since {\n                continue;\n            }\n",
        to: "",
        expect: "if ended <= since",
    },
    Injection {
        name: "長時間専有の中間報告を出さない",
        file: DIAG,
        from: "if let Some(msg) = take_hang_report() {",
        to: "if let Some(msg) = None::<String> {",
        expect: "take_hang_report",
    },
    Injection {
        name: "GUI が窓を自前で組む",
        file: MAIN,
        from: "            if let Some(line) = tako_control::diag::stall_line(armed, period) {",
        to: "            let _ = tako_control::diag::recent_spans_within(period);\n            if let Some(line) = tako_control::diag::stall_line(armed, period) {",
        expect: "recent_spans_within",
    },
    Injection {
        name: "注入を計測区間の中に置かない",
        file: MAIN,
        from: "                    inject_2665.block_in_span();\n",
        to: "",
        expect: "block_in_span",
    },
];

#[test]
fn 逆戻りを名指しできる() {
    let diag_src = read(DIAG);
    let main_src = read(MAIN);
    assert!(
        all_offenders(&diag_src, &main_src).is_empty(),
        "注入の前提: 現在のソースは緑"
    );
    for inj in INJECTIONS {
        let target = if inj.file == DIAG {
            &diag_src
        } else {
            &main_src
        };
        assert_eq!(
            target.matches(inj.from).count(),
            1,
            "注入「{}」の置換元がちょうど 1 か所ある",
            inj.name
        );
        let (d, m) = if inj.file == DIAG {
            (diag_src.replacen(inj.from, inj.to, 1), main_src.clone())
        } else {
            (diag_src.clone(), main_src.replacen(inj.from, inj.to, 1))
        };
        let offenders = all_offenders(&d, &m);
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
