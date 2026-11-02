//! **#2663 の番犬**: tako mod の IPC（報告・status・on / off・帯の切り替え）は、UI スレッドで端末の
//! ロックを取らない・ui.json を読み書きしない。
//!
//! ## なぜ縛るのか
//!
//! mod の報告（`tako mod report`）は claude のターンごとに全ペインから来る。以前は受け口が UI スレッドで
//! 帯の材料を組み、worker ごとの画面（`visible_lines`）と自分のペインの画面の末尾（`tail_lines`）を
//! 端末のロックを取って採り、ui.json を 2〜3 回読んでいた（帯のトグルがあれば排他を取って書く）。
//! PTY の読み手がロックを持ったまま止まると、その間 UI スレッドも止まる（本番 perf.log 10/26〜11/02 の
//! `dispatch:Mod` の専有 67 件・平均 50〜80ms・最大 253ms。段 1 の注入で 31〜77ms が採取の区間に乗った）。
//! いまは UI スレッドの段は受け付け・設定の切り替え・材料と画面の手（`PaneAccess`）の写しまでで、
//! 帯のトグルの取り込みと ui.json の読み・画面の採取・帯の組み立ては続きの段（IPC では background）、
//! `cards_place` / `chat_place` の写しは着地（読みの通し番号が新しいときだけ）。「その場で画面を読む」は
//! 最短の書き方なので戻りやすい（見た目には壊れず、ロックの待ちが UI に戻るだけ）。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. UI スレッドの段（`accept_report` / `snapshot` / `band_seed` / `worker_seed` / `status_seed` /
//!    `pane_row` / `action_prepare` / `land`）は画面を採らない・ui.json と skills を読み書きしない
//! 2. `status_ctx` が画面を採るのは A/B（`core::offload_legacy()`）の枝だけ
//! 3. 続きの段が仕事を持つ: `ReportJob` は取り込みと読みを錠（`UI_READS`）の内側で行い、`BandSeed` /
//!    `WorkerSeed` / `with_screens` が画面を採り、`finish_status_in_background` と `ActionJob` は採取を通す
//! 4. 着地は番号の守り（`copy_places`）を通す。置き場を直に書く行を本番コードに置かない
//! 5. dispatch の配線: 準備（`prepare_offload`）が報告と on / off・帯を段分けし、続き（`run_staged`）は
//!    着地へ戻し、着地（`land_offload` / `finish_offload`）は `claude_mod::land` を通す
//! 6. A/B の env（`TAKO_2663_LEGACY`）を読むのは tako-core の 1 か所、ゲートを引くのは準備と `status_ctx` だけ

use std::path::{Path, PathBuf};

const CONTROL_MOD: &str = "crates/tako-control/src/claude_mod.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CORE_MOD: &str = "crates/tako-core/src/claude_mod.rs";

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

/// `sig` で始まる関数・impl の範囲（シグネチャの位置〜閉じ括弧）
fn span_of(src: &str, sig: &str) -> Option<(usize, usize)> {
    let at = src.find(sig)?;
    Some((at, block_end(src, at)?))
}

fn all_at(hay: &str, needle: &str) -> Vec<usize> {
    hay.match_indices(needle).map(|(i, _)| i).collect()
}

fn get<'a>(sources: &'a [(&str, String)], file: &str) -> &'a str {
    &sources
        .iter()
        .find(|(f, _)| *f == file)
        .expect("走査する")
        .1
}

fn missing(file: &str, sig: &str) -> String {
    format!(
        "{file}:0 — `{}` が無い（構造が変わったらこの番犬も見直すこと）",
        sig.trim()
    )
}

/// UI スレッドの段（シグネチャ）
const UI_STAGES: &[&str] = &[
    "pub fn accept_report(",
    "pub(crate) fn snapshot(",
    "fn band_seed(",
    "fn worker_seed(",
    "fn status_seed(",
    "fn pane_row(",
    "pub fn action_prepare(",
    "pub fn land(",
];

/// UI スレッドの段に書いてはいけないもの（右は理由）
const UI_FORBIDDEN: &[(&str, &str)] = &[
    (".visible_lines(", "画面の採取（端末のロック）"),
    (".tail_lines(", "画面の採取（端末のロック）"),
    (".screen(", "画面の採取（端末のロック）"),
    ("lines_and_input(", "画面の採取（端末のロック）"),
    (".with_screens(", "全ペインの画面の採取"),
    (".band_view(", "帯の組み立て（画面の採取を含む）"),
    ("WorkerSeed::facts", "worker の画面の採取"),
    ("claude_mod_ui::view", "ui.json の読み"),
    ("claude_mod_ui::load(", "ui.json の読み"),
    (
        "claude_mod_ui::import_band(",
        "ui.json の取り込み（排他 + 書き込み）",
    ),
    ("claude_mod_ui::run(", "ui.json の書き込み"),
    (
        "finish_status(",
        "status の仕上げ（ui.json と skills の読み）",
    ),
    ("claude_mod_install::", "skills の読み"),
    ("UI_READS", "続きの段の錠"),
    ("std::fs::", "ファイルの読み書き"),
];

/// 1: UI スレッドの段は画面を採らない・ui.json と skills を読み書きしない
fn check_ui_stages(control: &str) -> Vec<String> {
    let mut v = Vec::new();
    for sig in UI_STAGES {
        let Some((start, end)) = span_of(control, sig) else {
            v.push(missing(CONTROL_MOD, sig));
            continue;
        };
        let body = &control[start..end];
        for (needle, why) in UI_FORBIDDEN {
            for at in all_at(body, needle) {
                v.push(format!(
                    "{CONTROL_MOD}:{} — UI スレッドの段 `{}` が{why}をしている（`{needle}`）= #2663 が戻る。\
                     続きの段（`ReportJob::run` / `BandSeed::band_view` / `WorkerSeed::facts` / \
                     `StatusCtx::with_screens` / `ActionJob::run`）へ置く",
                    line_at(control, start + at),
                    sig.trim()
                ));
            }
        }
    }
    v
}

/// 2: status_ctx が画面を採るのは A/B の枝だけ
fn check_status_ctx(control: &str) -> Vec<String> {
    let sig = "pub fn status_ctx(";
    let Some((start, end)) = span_of(control, sig) else {
        return vec![missing(CONTROL_MOD, sig)];
    };
    let body = &control[start..end];
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    if !compact.contains("Ok(ifcore::offload_legacy(){ctx.with_screens()}else{ctx})") {
        return vec![format!(
            "{CONTROL_MOD}:{} — `status_ctx` が A/B（`core::offload_legacy()`）の枝の外で画面を採りうる \
             （準備の段 = UI スレッドで全ペインの画面を採る = #2663 が戻る）。形は \
             `Ok(if core::offload_legacy() {{ ctx.with_screens() }} else {{ ctx }})`",
            line_at(control, start)
        )];
    }
    Vec::new()
}

/// 3: 続きの段が仕事を持つ
fn check_background_stages(control: &str) -> Vec<String> {
    const MUSTS: &[(&str, &[&str], &str)] = &[
        (
            "impl ReportJob {",
            &[
                "UI_READS.lock()",
                "claude_mod_ui::import_band(",
                "claude_mod_ui::view",
                "snapshot.finish(",
            ],
            "報告の続きが ui.json の取り込みと読み（錠の内側）・帯の仕上げを持っていない",
        ),
        (
            "impl BandSeed {",
            &[".tail_lines(", "WorkerSeed::facts"],
            "帯の続きが画面を採っていない",
        ),
        (
            "impl WorkerSeed {",
            &[".visible_lines()"],
            "worker の続きが画面を採っていない",
        ),
        (
            "fn with_screens(",
            &[".visible_lines()"],
            "status の続きが画面を採っていない",
        ),
        (
            "pub fn finish_status_in_background(",
            &[".with_screens()", "probe_no_report("],
            "status の背景の段が画面の採取を通していない",
        ),
        (
            "impl ActionJob {",
            &["claude_mod_ui::run(", ".with_screens()"],
            "on / off・帯の続きが ui.json の書き込みと画面の採取を持っていない",
        ),
    ];
    let mut v = Vec::new();
    for (sig, needles, why) in MUSTS {
        let Some((start, end)) = span_of(control, sig) else {
            v.push(missing(CONTROL_MOD, sig));
            continue;
        };
        let body = &control[start..end];
        for needle in *needles {
            if !body.contains(needle) {
                v.push(format!(
                    "{CONTROL_MOD}:{} — {why}（`{needle}` が無い）",
                    line_at(control, start)
                ));
            }
        }
    }
    v
}

/// 4: 着地は番号の守りを通す。置き場を直に書く行を本番コードに置かない
fn check_landing(control: &str, core: &str) -> Vec<String> {
    let mut v = Vec::new();
    match span_of(control, "pub fn land(") {
        Some((start, end)) if !control[start..end].contains(".copy_places(") => v.push(format!(
            "{CONTROL_MOD}:{} — 着地が置き場を番号の守り（`copy_places`）を通さずに写している \
             = 並行した続きの古い読みが新しい値を上書きする",
            line_at(control, start)
        )),
        Some(_) => {}
        None => v.push(missing(CONTROL_MOD, "pub fn land(")),
    }
    let guard = span_of(core, "    pub fn copy_places(");
    if guard.is_none() {
        v.push(missing(CORE_MOD, "pub fn copy_places("));
    }
    for (file, src) in [(CONTROL_MOD, control), (CORE_MOD, core)] {
        for needle in ["cards_place = ", "chat_place = "] {
            for at in all_at(src, needle) {
                let in_guard = file == CORE_MOD && guard.is_some_and(|(s, e)| at > s && at < e);
                if !in_guard {
                    v.push(format!(
                        "{file}:{} — 置き場を `copy_places` の外で書いている（`{needle}`）。古い読みで上書きしうる",
                        line_at(src, at)
                    ));
                }
            }
        }
    }
    v
}

/// 5: dispatch の配線
fn check_dispatch(dispatch: &str, control: &str) -> Vec<String> {
    const MUSTS: &[(&str, &[&str], &str)] = &[
        (
            "pub fn prepare_offload(",
            &[
                "claude_mod::accept_report(",
                "OffloadJob::ModReport(",
                "claude_mod::action_prepare(",
                "OffloadJob::ModAction(",
                "claude_mod::status_ctx(",
                "OffloadJob::ModStatus",
            ],
            "準備が tako mod の報告・on / off・帯・status を段分けしていない（同期の dispatch = UI スレッドで採る）",
        ),
        (
            "    pub fn run_staged(self)",
            &[
                "OffloadJob::ModReport(job) =>",
                "OffloadJob::ModAction(job) =>",
                "OffloadContinuation::Mod(",
            ],
            "続きが着地（置き場の写しと描き直し）へ戻っていない",
        ),
        (
            "pub fn land_offload(",
            &["OffloadContinuation::Mod(landing)", "claude_mod::land("],
            "着地が `claude_mod::land` を通っていない",
        ),
        (
            "pub fn finish_offload(",
            &["OffloadContinuation::Mod(landing)", "claude_mod::land("],
            "直の呼び手の着地が `claude_mod::land` を通っていない",
        ),
    ];
    let mut v = Vec::new();
    for (sig, needles, why) in MUSTS {
        let Some((start, end)) = span_of(dispatch, sig) else {
            v.push(missing(DISPATCH, sig));
            continue;
        };
        let body = &dispatch[start..end];
        for needle in *needles {
            if !body.contains(needle) {
                v.push(format!(
                    "{DISPATCH}:{} — {why}（`{needle}` が無い）",
                    line_at(dispatch, start)
                ));
            }
        }
    }
    // 同期の腕（テスト・直呼び・A/B）も同じ 3 段を順に通す（実装を 2 本にしない）
    match span_of(control, "pub fn run(") {
        Some((start, end)) => {
            let compact: String = control[start..end]
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            if !compact.contains(
                "\"report\"=>accept_report(host,report,pane,ancestry.as_deref()).map(ReportJob::run).and_then(|landing|land(host,landing))",
            ) {
                v.push(format!(
                    "{CONTROL_MOD}:{} — 同期の腕の報告が 3 段（`accept_report` → `ReportJob::run` → `land`）を通っていない",
                    line_at(control, start)
                ));
            }
        }
        None => v.push(missing(CONTROL_MOD, "pub fn run(")),
    }
    v
}

/// 6: A/B のゲートは 1 か所
fn check_single_gate(sources: &[(&str, String)]) -> Vec<String> {
    let mut v = Vec::new();
    let core = get(sources, CORE_MOD);
    let gate = span_of(core, "pub fn offload_legacy() -> bool");
    if gate.is_none() {
        v.push(missing(CORE_MOD, "pub fn offload_legacy() -> bool"));
    }
    let allowed: &[(&str, &str)] = &[
        (DISPATCH, "pub fn prepare_offload("),
        (CONTROL_MOD, "pub fn status_ctx("),
    ];
    for (file, src) in sources {
        for at in all_at(src, "\"TAKO_2663_LEGACY\"") {
            let is_const = *file == CORE_MOD
                && src[..at]
                    .rsplit('\n')
                    .next()
                    .is_some_and(|l| l.contains("pub const OFFLOAD_LEGACY_ENV"));
            if !is_const {
                v.push(format!(
                    "{file}:{} — A/B の env（TAKO_2663_LEGACY）を定数 `OFFLOAD_LEGACY_ENV` の外で書いている（名前は 1 か所）",
                    line_at(src, at)
                ));
            }
        }
        for at in all_at(src, "offload_legacy()") {
            let before = &src[..at];
            // 別のゲート（`orchestrator_offload_legacy()` 等）は数えない
            if before
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                continue;
            }
            if before.ends_with("fn ") {
                continue;
            }
            let ok = allowed.iter().any(|(f, sig)| {
                f == file && span_of(src, sig).is_some_and(|(s, e)| at > s && at < e)
            });
            if !ok {
                v.push(format!(
                    "{file}:{} — A/B のゲート（`offload_legacy()`）を準備（`prepare_offload`）と `status_ctx` の外で引いている",
                    line_at(src, at)
                ));
            }
        }
    }
    v
}

fn sources_now() -> Vec<(&'static str, String)> {
    [CONTROL_MOD, DISPATCH, CORE_MOD]
        .into_iter()
        .map(|f| (f, view(&read_raw(f), f)))
        .collect()
}

fn all(sources: &[(&str, String)]) -> Vec<String> {
    let control = get(sources, CONTROL_MOD);
    let dispatch = get(sources, DISPATCH);
    let core = get(sources, CORE_MOD);
    let mut v = check_ui_stages(control);
    v.extend(check_status_ctx(control));
    v.extend(check_background_stages(control));
    v.extend(check_landing(control, core));
    v.extend(check_dispatch(dispatch, control));
    v.extend(check_single_gate(sources));
    v
}

#[test]
fn issue2663_tako_modのipcはuiスレッドで画面を採らずuijsonを読み書きしない() {
    let v = all(&sources_now());
    assert!(v.is_empty(), "\n{}", v.join("\n"));
}

/// 注入した逆戻りが file:line で名指しされる（番犬が見逃す側へ倒れていない）
#[test]
fn issue2663_逆戻りを名指しできる() {
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
    // 1. 帯の写しの段がその場で自分のペインの画面を採る（修正前の形）
    named(
        &inject(
            CONTROL_MOD,
            "            screen_shows_usage: false,",
            "            screen_shows_usage: session\n                .is_some_and(|s| core::screen_shows_usage(&s.tail_lines(core::USAGE_SCAN_LINES))),",
        ),
        CONTROL_MOD,
        "1. 帯の写しの段で画面を採る注入",
    );
    // 1. worker の写しの段がその場で画面を採る（修正前の `worker_facts` の形）
    named(
        &inject(
            CONTROL_MOD,
            "    let session = host.session(id);\n    WorkerSeed {",
            "    let session = host.session(id);\n    let _lines = session.map(|s| s.visible_lines()).unwrap_or_default();\n    WorkerSeed {",
        ),
        CONTROL_MOD,
        "1. worker の写しの段で画面を採る注入",
    );
    // 1. 受け付けの段がその場で帯のトグルを ui.json へ取り込む（修正前の形）
    named(
        &inject(
            CONTROL_MOD,
            "    let import = parsed.band.clone().filter(|_| !core::s3_legacy());",
            "    let import = parsed.band.clone().filter(|_| !core::s3_legacy());\n    if let (Some(b), Some(p)) = (import.as_ref(), host.claude_mod_ui_path()) {\n        crate::claude_mod_ui::import_band(&p, b);\n    }",
        ),
        CONTROL_MOD,
        "1. 受け付けの段で ui.json へ取り込む注入",
    );
    // 1. status の写しの段がその場で画面を採る
    named(
        &inject(
            CONTROL_MOD,
            "            let session = host.session(id);\n            rows.push(RowSeed {",
            "            let session = host.session(id);\n            let _ = session.map(|s| s.visible_lines());\n            rows.push(RowSeed {",
        ),
        CONTROL_MOD,
        "1. status の写しの段で画面を採る注入",
    );
    // 2. status_ctx が A/B の外でも画面を採る
    named(
        &inject(
            CONTROL_MOD,
            "    Ok(if core::offload_legacy() {\n        ctx.with_screens()\n    } else {\n        ctx\n    })",
            "    Ok(ctx.with_screens())",
        ),
        CONTROL_MOD,
        "2. status_ctx が常に画面を採る注入",
    );
    // 3. 報告の続きが錠を取らずに読む
    named(
        &inject(
            CONTROL_MOD,
            "let mut reads = UI_READS.lock().unwrap_or_else(|e| e.into_inner());",
            "let mut reads = 0u64; let reads = &mut reads;",
        ),
        CONTROL_MOD,
        "3. 錠の外で読む注入",
    );
    // 4. 着地が置き場を直に書く（番号の守りを外す）
    named(
        &inject(
            CONTROL_MOD,
            "        hub.copy_places(seq, cards, chat);",
            "        let _ = seq;\n        hub.cards_place = cards;\n        hub.chat_place = chat;",
        ),
        CONTROL_MOD,
        "4. 着地が置き場を直に書く注入",
    );
    // 5. 準備が報告を段分けしない（同期の dispatch へ戻す）
    named(
        &inject(
            DISPATCH,
            "                .map(|job| OffloadJob::ModReport(Box::new(job))),",
            "                .map(|_| unreachable!()),",
        ),
        DISPATCH,
        "5. 準備が報告を段分けしない注入",
    );
    // 5. 続きが着地へ戻らず background から直に応答する（置き場が写らない）
    named(
        &inject(
            DISPATCH,
            "            OffloadJob::ModReport(job) => {",
            "            OffloadJob::ModReportDirect(job) => {",
        ),
        DISPATCH,
        "5. 続きが着地へ戻らない注入",
    );
    // 6. A/B の env を別の場所で読む
    named(
        &inject(
            CONTROL_MOD,
            "pub fn land(host: &mut dyn ControlHost, landing: Landing) -> Result<Value, DispatchError> {",
            "fn leak() -> bool { std::env::var_os(\"TAKO_2663_LEGACY\").is_some() }\npub fn land(host: &mut dyn ControlHost, landing: Landing) -> Result<Value, DispatchError> {",
        ),
        CONTROL_MOD,
        "6. A/B の env を 2 か所目で読む注入",
    );
    // 6. ゲートを準備と status_ctx の外で引く
    named(
        &inject(
            CONTROL_MOD,
            "    let import = parsed.band.clone().filter(|_| !core::s3_legacy());",
            "    let import = parsed.band.clone().filter(|_| !core::s3_legacy() && !core::offload_legacy());",
        ),
        CONTROL_MOD,
        "6. ゲートを受け付けの段で引く注入",
    );
}
