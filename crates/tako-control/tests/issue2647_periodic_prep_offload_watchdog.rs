//! **#2647 の番犬**: 2 秒 tick の判定がペインの画面を読むのは背景のスレッド。UI スレッドの段は
//! 背景で採った 1 枚を受け取って判定するだけにする。
//!
//! ## なぜ縛るのか
//!
//! 2 秒ごとの定期更新（`periodic_prep`）の 5 つの段 — エージェントのメトリクス（#357）・キュー滞留の
//! 救出（#572）・master の引き継ぎの知らせ（#749）・利用制限からの自動復帰（#813）・ネット復帰の
//! 再開（#2112）— は、以前はペインごとに UI スレッドで端末のロックを取って画面を組み、行を解析していた
//! （エージェントのペイン 1 本あたり 1 tick に約 6 回）。ペインの数に比例して重く、PTY のスレッドが
//! ロックを握っていればその間も待つ（本番の perf.log 10/28〜11/02: `periodic_prep` の専有 247 件・
//! 平均 76ms・最大 492ms）。いまは UI スレッドはペインへの手を集めるだけ（`tick_screens::plan`）で、
//! ロック・組み立て・解析・引き継ぎの閾値の設定（profiles/*.yaml）の読み直しは背景
//! （`tick_screens::read`）が済ませる。「その場で端末を読む」は最短の書き方なので戻りやすい
//! （見た目には壊れず、ペインの数に比例して重くなるだけ）。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. 5 つの段（と、段から呼ぶ画面の外の読み・送るかの判断）の本体は端末を直に引かない
//!    （`self.terminals.get(` を書かない）。画面は `screens.source(&self.terminals, …)` から読む
//! 2. 定期更新のループは、`periodic_prep` の前に UI スレッドで計画（`tick_screens::plan(`）を作り、
//!    背景（`background_executor().spawn(`）で `tick_screens::read(` を走らせ、5 つの段へ `&screens` を渡す。
//!    背景で読み直した設定（`absorb_handoff_policies(`）は引き継ぎの段より前に控えへ入れる
//! 3. 計画（`fn plan(` / `fn handoff_policy_wants(`）は端末もファイルも触らない（メモリの操作だけ）
//! 4. 引き継ぎの閾値の設定を読む I/O（`orchestrator::handoff_policy_for(`）を呼ぶのは背景の `fn read(` と、
//!    控えの外れの受け口（main.rs の `fn handoff_policy_for(`）だけ
//! 5. A/B（`TAKO_2647_LEGACY`）を読むのは 1 か所（`tick_screens::legacy()`）で、それを引くのは計画だけ

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const LIMIT: &str = "crates/tako-app/src/limit_autoresume.rs";
const NET: &str = "crates/tako-app/src/net_autoresume.rs";
const TICK: &str = "crates/tako-app/src/tick_screens.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので行番号はずれない
#[path = "common/production_range.rs"]
mod production_range;

/// 本番コードだけの眺め（テスト領域とコメントを同じバイト数の空白へ潰す）
fn view(src: &str, rel: &str) -> String {
    production_range::code_view::without_comments(&production_range::production(src, rel))
}

fn read_raw(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// バイト位置の行番号（1 始まり）
fn line_at(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// `open` の位置から始まる `{` に対応する `}` の位置（文字列の中の括弧は数えない）
fn block_end(src: &str, open: usize) -> Option<usize> {
    let code = production_range::code_view_of(src);
    let bytes = code.as_bytes();
    let start = open + code[open..].find('{')?;
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `sig` で始まる関数の範囲（シグネチャの位置〜閉じ括弧）
fn span_of(src: &str, sig: &str) -> Option<(usize, usize)> {
    let at = src.find(sig)?;
    Some((at, block_end(src, at)?))
}

fn all_at(hay: &str, needle: &str) -> Vec<usize> {
    hay.match_indices(needle).map(|(i, _)| i).collect()
}

/// 1: 段の本体は端末を直に引かない
fn check_stages_read_screens(sources: &[(&str, String)]) -> Vec<String> {
    const STAGES: &[(&str, &str)] = &[
        (
            MAIN,
            "    fn refresh_agent_metrics(&mut self, screens: &tick_screens::TickScreens)",
        ),
        (
            MAIN,
            "    fn drive_queued_message_recovery(&mut self, screens: &tick_screens::TickScreens)",
        ),
        (
            MAIN,
            "    fn drive_handoff_nudge(&mut self, screens: &tick_screens::TickScreens)",
        ),
        (
            LIMIT,
            "    pub(crate) fn drive_limit_autoresume(&mut self, screens: &TickScreens)",
        ),
        (
            NET,
            "    pub(crate) fn drive_net_autoresume(&mut self, screens: &TickScreens)",
        ),
        (NET, "    fn offscreen_screen("),
        (NET, "    fn net_resume_decide("),
    ];
    let mut v = Vec::new();
    for (file, sig) in STAGES {
        let src = &sources.iter().find(|(f, _)| f == file).expect("走査する").1;
        let Some((start, end)) = span_of(src, sig) else {
            v.push(format!(
                "{file}:0 — `{}` が無い（構造が変わったらこの番犬も見直すこと）",
                sig.trim()
            ));
            continue;
        };
        let body = &src[start..end];
        for needle in [
            "self.terminals.get(",
            "self.terminals.get_mut(",
            ".terminals.get(&",
        ] {
            for at in all_at(body, needle) {
                v.push(format!(
                    "{file}:{} — `{}` の中で端末を直に引いている（`{needle}`）。UI スレッドで端末のロックを取る = \
                     #2647 が戻る。画面は `screens.source(&self.terminals, pane)` から読む",
                    line_at(src, start + at),
                    sig.trim()
                ));
            }
        }
        // 整形で `screens` と `.source(` の間に改行が入るので、空白を落として見る
        let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        if !compact.contains("screens.source(&self.terminals") {
            v.push(format!(
                "{file}:{} — `{}` が画面を `screens.source(&self.terminals, …)` から読んでいない",
                line_at(src, start),
                sig.trim()
            ));
        }
    }
    v
}

/// 2: 定期更新のループの配線
fn check_loop_wiring(main: &str) -> Vec<String> {
    let mut v = Vec::new();
    let Some(prep) = main.find("perf_span(\"periodic_prep\")") else {
        return vec![format!(
            "{MAIN}:0 — `periodic_prep` の計測区間が無い（構造が変わったらこの番犬も見直すこと）"
        )];
    };
    // 計画は periodic_prep より前（同じループの中の直前の UI スレッドの区間）
    let plan = main[..prep].rfind("tick_screens::plan(app)");
    let read = main[..prep].rfind("tick_screens::read(plan)");
    match (plan, read) {
        (Some(p), Some(r)) if p < r => {
            // 背景で読む: 直前の `background_executor()` から `.spawn(` を経て呼び出しまで
            let spawn_ok = main[..r]
                .rfind(".spawn(async move")
                .is_some_and(|s| main[..s].rfind("background_executor()").is_some_and(|b| s - b < 120) && r - s < 80);
            if !spawn_ok {
                v.push(format!(
                    "{MAIN}:{} — `tick_screens::read(` を背景（`cx.background_executor().spawn(async move …)`）の外で呼んでいる \
                     = UI スレッドで全ペインの画面を読む（#2647 が戻る）",
                    line_at(main, r)
                ));
            }
        }
        _ => v.push(format!(
            "{MAIN}:{} — `periodic_prep` の前に計画（`tick_screens::plan(app)`）→ 背景の読み（`tick_screens::read(plan)`）が無い",
            line_at(main, prep)
        )),
    }
    // periodic_prep の区間（UI スレッドのクロージャ）の中で、5 つの段へ背景で採った 1 枚を渡す
    let Some(prep_end) = main[prep..]
        .find("let prep_ms = t0.elapsed().as_millis();")
        .map(|i| prep + i)
    else {
        return vec![format!(
            "{MAIN}:{} — `periodic_prep` の区間の終わり（`let prep_ms = t0.elapsed()…`）が無い（構造が変わったらこの番犬も見直すこと）",
            line_at(main, prep)
        )];
    };
    let body = &main[prep..prep_end];
    for call in [
        "app.refresh_agent_metrics(&screens)",
        "app.drive_queued_message_recovery(&screens)",
        "app.drive_handoff_nudge(&screens)",
        "app.drive_limit_autoresume(&screens)",
        "app.drive_net_autoresume(&screens)",
    ] {
        if !body.contains(call) {
            v.push(format!(
                "{MAIN}:{} — `periodic_prep` の中に `{call}` が無い（段へ背景で採った 1 枚を渡していない = その場で端末を読む）",
                line_at(main, prep)
            ));
        }
    }
    for at in all_at(body, "TickScreens::Live") {
        v.push(format!(
            "{MAIN}:{} — `periodic_prep` の中で `TickScreens::Live`（その場で端末を読む）を段へ渡している（#2647 が戻る）",
            line_at(main, prep + at)
        ));
    }
    match (body.find("absorb_handoff_policies("), body.find("app.drive_handoff_nudge(&screens)")) {
        (Some(a), Some(d)) if a < d => {}
        _ => v.push(format!(
            "{MAIN}:{} — 背景で読み直した引き継ぎの閾値の設定（`absorb_handoff_policies(`）を段より前に控えへ入れていない \
             （控えが外れて UI スレッドで profiles/*.yaml を読む）",
            line_at(main, prep)
        )),
    }
    v
}

/// 3: 計画はメモリの操作だけ
fn check_plan_memory_only(tick: &str) -> Vec<String> {
    let mut v = Vec::new();
    for sig in ["pub(crate) fn plan(", "    fn handoff_policy_wants("] {
        let Some((start, end)) = span_of(tick, sig) else {
            v.push(format!(
                "{TICK}:0 — `{}` が無い（構造が変わったらこの番犬も見直すこと）",
                sig.trim()
            ));
            continue;
        };
        let body = &tick[start..end];
        for needle in [
            ".visible_lines(",
            ".tail_lines(",
            ".is_alt_screen(",
            ".analyze_input(",
            ".agent_metrics(",
            ".alt_and_lines(",
            ".lines_with_history(",
            "handoff_policy_for(",
            "std::fs::",
        ] {
            for at in all_at(body, needle) {
                v.push(format!(
                    "{TICK}:{} — 計画（`{}`）が UI スレッドで `{needle}` を呼んでいる（計画は手を集めるだけ。読むのは背景の `read`）",
                    line_at(tick, start + at),
                    sig.trim()
                ));
            }
        }
    }
    v
}

/// 4: 引き継ぎの閾値の設定を読む I/O の呼び手
fn check_policy_io(main: &str, tick: &str) -> Vec<String> {
    let mut v = Vec::new();
    let needle = "orchestrator::handoff_policy_for(";
    let allowed: [(&str, &str, &str); 2] = [
        (
            MAIN,
            main,
            "    fn handoff_policy_for(&mut self, profile: &str)",
        ),
        (TICK, tick, "pub(crate) fn read(plan: TickPlan)"),
    ];
    for (file, src, sig) in allowed {
        let span = span_of(src, sig);
        if span.is_none() {
            v.push(format!(
                "{file}:0 — `{}` が無い（構造が変わったらこの番犬も見直すこと）",
                sig.trim()
            ));
        }
        for at in all_at(src, needle) {
            if !span.is_some_and(|(s, e)| at > s && at < e) {
                v.push(format!(
                    "{file}:{} — 引き継ぎの閾値の設定の読み（`{needle}`）を背景の `read` と控えの受け口の外で呼んでいる \
                     （2 秒 tick の UI スレッドで profiles/*.yaml を読む = #2647）",
                    line_at(src, at)
                ));
            }
        }
    }
    v
}

/// 5: A/B のゲートは 1 か所
fn check_single_gate(sources: &[(&str, String)]) -> Vec<String> {
    let mut v = Vec::new();
    let tick = &sources
        .iter()
        .find(|(f, _)| *f == TICK)
        .expect("走査する")
        .1;
    let gate_fn = span_of(tick, "pub(crate) fn legacy() -> bool");
    if gate_fn.is_none() {
        v.push(format!(
            "{TICK}:0 — A/B のゲート `fn legacy()` が無い（構造が変わったらこの番犬も見直すこと）"
        ));
    }
    for (file, src) in sources {
        for at in all_at(src, "TAKO_2647_LEGACY") {
            let in_gate = *file == TICK && gate_fn.is_some_and(|(s, e)| at > s && at < e);
            if !in_gate {
                v.push(format!(
                    "{file}:{} — A/B の env（TAKO_2647_LEGACY）を `tick_screens::legacy()` の外で読んでいる（ゲートは 1 か所）",
                    line_at(src, at)
                ));
            }
        }
    }
    let plan = span_of(tick, "pub(crate) fn plan(");
    for (file, src) in sources {
        for at in all_at(src, "legacy()") {
            // `tick_screens::legacy()` と、tick_screens.rs の中の素の `legacy()` だけを数える
            // （`c2_legacy()` / `limit_resume_all::legacy()` のような別のゲートは数えない）
            let before = &src[..at];
            let ident = before
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
            let qualified = before.ends_with("tick_screens::");
            let local =
                *file == TICK && !ident && !before.ends_with("::") && !before.ends_with("fn ");
            if !(qualified || local) {
                continue;
            }
            let in_plan = *file == TICK && plan.is_some_and(|(s, e)| at > s && at < e);
            if !in_plan {
                v.push(format!(
                    "{file}:{} — A/B のゲート（`tick_screens::legacy()`）を計画の外で引いている（ゲートは 1 か所 = 計画が背景で読むかを決める）",
                    line_at(src, at)
                ));
            }
        }
    }
    v
}

fn sources_now() -> Vec<(&'static str, String)> {
    [MAIN, LIMIT, NET, TICK]
        .into_iter()
        .map(|f| (f, view(&read_raw(f), f)))
        .collect()
}

fn all(sources: &[(&str, String)]) -> Vec<String> {
    let get = |f: &str| &sources.iter().find(|(n, _)| *n == f).expect("走査する").1;
    let mut v = check_stages_read_screens(sources);
    v.extend(check_loop_wiring(get(MAIN)));
    v.extend(check_plan_memory_only(get(TICK)));
    v.extend(check_policy_io(get(MAIN), get(TICK)));
    v.extend(check_single_gate(sources));
    v
}

#[test]
fn issue2647_定期更新の判定は背景で採った画面を読む() {
    let v = all(&sources_now());
    assert!(v.is_empty(), "\n{}", v.join("\n"));
}

/// 注入した逆戻りが file:line で名指しされる（番犬が見逃す側へ倒れていない）
#[test]
fn issue2647_逆戻りを名指しできる() {
    let base = sources_now();
    assert!(all(&base).is_empty(), "注入の前の状態は綺麗");
    let inject = |file: &str, from: &str, to: &str| -> Vec<String> {
        let mut s = base.clone();
        let body = &mut s.iter_mut().find(|(f, _)| *f == file).expect("走査する").1;
        assert_eq!(body.matches(from).count(), 1, "注入の目印が 1 つ: {from}");
        *body = body.replacen(from, to, 1);
        all(&s)
    };
    let named = |v: &[String], file: &str, what: &str| {
        assert!(
            v.iter().any(
                |m| m.starts_with(&format!("{file}:")) && !m.starts_with(&format!("{file}:0 "))
            ),
            "{what} を file:line で名指せない: {v:#?}"
        );
    };
    // 1. 利用制限の段がその場で端末を読む（修正前の形）
    named(
        &inject(
            LIMIT,
            "let Some(session) = screens.source(&self.terminals, pane) else {",
            "let Some(session) = self.terminals.get(&pane) else {",
        ),
        LIMIT,
        "1. 利用制限の段がその場で端末を読む注入",
    );
    // 1. メトリクスの段がその場で端末を読む
    named(
        &inject(
            MAIN,
            "if let Some(session) = screens.source(&self.terminals, *pid) {",
            "if let Some(session) = self.terminals.get(pid) {",
        ),
        MAIN,
        "1. メトリクスの段がその場で端末を読む注入",
    );
    // 2. 背景の読みを UI スレッドで呼ぶ
    named(
        &inject(
            MAIN,
            "cx.background_executor()\n                            .spawn(async move { tick_screens::read(plan) })\n                            .await",
            "tick_screens::read(plan)",
        ),
        MAIN,
        "2. 背景の読みを UI スレッドで呼ぶ注入",
    );
    // 2. 段へその場の読みを渡す
    named(
        &inject(
            MAIN,
            "app.drive_limit_autoresume(&screens)",
            "app.drive_limit_autoresume(&tick_screens::TickScreens::Live)",
        ),
        MAIN,
        "2. 段へその場の読みを渡す注入",
    );
    // 3. 計画が UI スレッドで画面を読む
    named(
        &inject(
            TICK,
            "            let session = app.terminals.get(&pane)?;",
            "            let session = app.terminals.get(&pane)?;\n            let _ = session.visible_lines();",
        ),
        TICK,
        "3. 計画が画面を読む注入",
    );
    // 4. 引き継ぎの段が UI スレッドで設定を読む
    named(
        &inject(
            MAIN,
            "let (threshold, auto_handoff) = self.handoff_policy_for(&profile);",
            "let (threshold, auto_handoff) = { let (r, a) = tako_control::orchestrator::handoff_policy_for(&profile); (r.value, a) };",
        ),
        MAIN,
        "4. 段が設定を直に読む注入",
    );
    // 5. A/B の env を別の場所で読む
    named(
        &inject(
            NET,
            "    pub(crate) fn drive_net_autoresume(&mut self, screens: &TickScreens) {",
            "    pub(crate) fn drive_net_autoresume(&mut self, screens: &TickScreens) {\n        let _ = std::env::var_os(\"TAKO_2647_LEGACY\");",
        ),
        NET,
        "5. A/B の env を 2 か所目で読む注入",
    );
}
