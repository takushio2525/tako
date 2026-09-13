//! **#1442 の番犬**: 窓の位置・寸法を tako 自身の口で決められる形を保つ。
//!
//! ## なぜ止めるのか
//!
//! `TAKO_DISPLAY` を付けた隔離起動は保存フレームを無視して**必ず置き先の中央
//! 960x600** で開き、位置・寸法を外から決める手段が無かった。回避策として
//! System Events（AX）で `first application process whose unix id is <隔離 pid>` を
//! 掴んで動かすと、**AX は複数の tako-app を unix id にかかわらず同一プロセスとして
//! 返す**ので本番 `/Applications/tako.app` の窓が動く（ユーザーの窓を動かす事故が
//! 実際に起きた = #1442 の症状 2）。
//!
//! 実測（2026-09-13・tako-vd = グローバル 1512,0 の 2560x1440）:
//!
//! | | 窓の矩形（CGWindowList） |
//! |---|---|
//! | 修正前（保存フレーム 200,100,1400,900 あり） | `2312,420,960,600`（= 中央 960x600。保存値は無視） |
//! | 修正後 `TAKO_WINDOW_BOUNDS=100,50,1400,900` | `1612,50,1400,900`（= ディスプレイ内 100,50） |
//! | 修正後 `TAKO_1442_LEGACY=1` で同じ指定 | `2312,420,960,600`（旧挙動を再現） |
//!
//! ## 何を固定するか
//!
//! 1. [`起動時の矩形は指定を先に見る`] — env を読まない / 置き先の中央より後ろへ回る
//! 2. [`既定の寸法は1つの定数`] — 960x600 の直書きが戻ると片方だけ変わる
//! 3. [`撥ねる条件が2つとも在る`] — 最小寸法・置き先からのはみ出しの検査
//! 4. [`座標の解釈は1実装`] — env / CLI / MCP が同じ `window_bounds::resolve` を通る
//! 5. [`cliとmcpは1対1`] — 片方にしか無い操作を作らない（設計原則 5）
//! 6. [`abの逃げ道は1つ`] — `TAKO_1442_LEGACY` が消える / 2 本目が生える
//! 7. [`window_listが矩形を返す`] — 指定した結果を読む口（AX の代わり）
//! 8. [`axで動かす道をtakoへ向けない`] — `virtual-display.sh` の当座しのぎが tako を拒む

use std::path::{Path, PathBuf};

// 本番コードだけの眺めは 1 実装を通す（#1420）
#[path = "common/production_range.rs"]
mod production_range;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const APP_REL: &str = "crates/tako-app/src/main.rs";
const CORE_REL: &str = "crates/tako-core/src/platform/window_bounds.rs";
const DISPATCH_REL: &str = "crates/tako-control/src/dispatch.rs";
const PROTOCOL_REL: &str = "crates/tako-control/src/protocol.rs";
const CLI_REL: &str = "crates/tako-cli/src/main.rs";
const MCP_REQUEST_REL: &str = "crates/tako-control/src/mcp/request.rs";
const MCP_CATALOG_REL: &str = "crates/tako-control/src/mcp/catalog.rs";
const VD_REL: &str = "scripts/lib/virtual-display.sh";

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with("///")
}

/// テスト領域だけを空白へ潰した眺め（バイト長と行番号は保たれる）
fn production(rel: &str) -> String {
    production_range::production(&read(rel), rel)
}

/// 関数 1 本の本体（署名の行から、同じ字下げの `}` まで）。
/// **見つからないことも FAILED**（走査範囲が空だとどんな回帰でも通る）
fn fn_body(rel: &str, src: &str, signature: &str) -> (Vec<(usize, String)>, usize) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(signature))
        .unwrap_or_else(|| panic!("{rel}: 目印 {signature:?} が消えている（走査範囲を作れない）"));
    let indent = " ".repeat(lines[start].len() - lines[start].trim_start().len());
    let close = format!("{indent}}}");
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("{rel}: {signature:?} の本体を閉じる `}}` が見つからない"));
    let body: Vec<(usize, String)> = (start..end)
        .filter(|i| !is_comment(lines[*i]))
        .map(|i| (i + 1, lines[i].to_string()))
        .collect();
    assert!(
        body.len() >= 3,
        "{rel}:{}: {signature:?} の走査範囲が {} 行しかない（範囲取りが壊れている）",
        start + 1,
        body.len()
    );
    (body, start + 1)
}

/// 本体の中で `needle` が最初に出る行（1 起点）
fn find(body: &[(usize, String)], needle: &str) -> Option<usize> {
    body.iter()
        .find(|(_, l)| l.contains(needle))
        .map(|(n, _)| *n)
}

/// ファイル中で `needle` が最初に出る行（1 起点）。無ければ `0`
/// （落ちるときに `file:line` を名指しできるようにするためだけの補助）
fn line_of(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// 本体を 1 本の文字列へ（複数行にまたがる呼び出しを見るとき用）
fn joined(body: &[(usize, String)]) -> String {
    body.iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

const INITIAL_FN: &str = "fn initial_window_bounds(";
const REQUESTED_FN: &str = "fn requested_window_bounds(";
const APPLY_FN: &str = "fn apply_window_geometry(";
const RESOLVE_FN: &str = "pub fn resolve(";
const GEOMETRY_OP_FN: &str = "fn window_geometry_op(";

// --------------------------------------------- 1. 指定を先に見る

#[test]
fn 起動時の矩形は指定を先に見る() {
    let src = production(APP_REL);
    let (body, at) = fn_body(APP_REL, &src, INITIAL_FN);
    let requested = find(&body, "requested_window_bounds()").unwrap_or_else(|| {
        panic!(
            "{APP_REL}:{at}: {INITIAL_FN} が {REQUESTED_FN} を通っていない \
             = TAKO_WINDOW_BOUNDS を読まずに置き先の中央へ開く（#1442 の症状 1 が戻る）"
        )
    });
    let centered = find(&body, "target_display_id().is_some()").unwrap_or_else(|| {
        panic!("{APP_REL}:{at}: 置き先の中央へ落ちる分岐（#1141）が見つからない")
    });
    assert!(
        requested < centered,
        "{APP_REL}:{requested}: 明示の指定が置き先の中央（{APP_REL}:{centered}）より後ろに在る \
         = TAKO_DISPLAY を付けた隔離起動では指定が一生効かない（#1442）"
    );

    // 読む口は 1 つ（`std::env::var(\"TAKO_WINDOW_BOUNDS\")` の直書きを散らさない）
    let (req_body, req_at) = fn_body(APP_REL, &src, REQUESTED_FN);
    let req = joined(&req_body);
    assert!(
        req.contains("wb::env_spec()"),
        "{APP_REL}:{req_at}: {REQUESTED_FN} が `wb::env_spec()` を通っていない（読む口が散る）"
    );
    assert!(
        req.contains("wb::parse(") && req.contains("wb::resolve("),
        "{APP_REL}:{req_at}: 指定の解釈が `window_bounds` の 1 実装を通っていない（#1442）"
    );
    // 撥ねたことを黙らせない（persist.log を読むまで気づけないのが #1160 の症状）
    assert!(
        req.contains("persist_log("),
        "{APP_REL}:{req_at}: 撥ねた指定の理由を persist.log へ残していない（#1442 / #1160）"
    );
}

// --------------------------------------------- 2. 既定の寸法

#[test]
fn 既定の寸法は1つの定数() {
    let core = production(CORE_REL);
    for (name, needle, why) in [
        (
            "DEFAULT_WIDTH",
            "pub const DEFAULT_WIDTH: f32 = 960.0;",
            "既定の幅",
        ),
        (
            "DEFAULT_HEIGHT",
            "pub const DEFAULT_HEIGHT: f32 = 600.0;",
            "既定の高さ",
        ),
        ("MIN_WIDTH", "pub const MIN_WIDTH: f32 = 200.0;", "最小の幅"),
        (
            "MIN_HEIGHT",
            "pub const MIN_HEIGHT: f32 = 150.0;",
            "最小の高さ",
        ),
    ] {
        // 定数そのものが在る行を名指す（値だけ変わった注入でも場所が分かる）
        let at = line_of(&core, &format!("pub const {name}"));
        assert!(
            core.contains(needle),
            "{CORE_REL}:{at}: {why}（{needle}）が変わっている = 既定の窓の大きさが変わる（#1442）"
        );
    }

    // 既定サイズの直書きが UI 層へ戻っていないか（戻ると片方だけ変わる）
    let app = production(APP_REL);
    let stray: Vec<String> = app
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("px(960.") && l.contains("px(600."))
        .map(|(i, l)| format!("{APP_REL}:{}: {}", i + 1, l.trim()))
        .collect();
    assert!(
        stray.is_empty(),
        "既定の窓サイズが直書きへ戻っている（`default_window_size()` = \
         `wb::DEFAULT_WIDTH/HEIGHT` の 1 実装を通す。#1442）:\n{}",
        stray.join("\n")
    );
    assert!(
        app.contains("fn default_window_size()") && app.contains("wb::DEFAULT_WIDTH"),
        "{APP_REL}: `default_window_size()` が `wb::DEFAULT_WIDTH` を見ていない（#1442）"
    );

    // 保存フレームの健全性検査も同じ下限を見る（下限が 2 か所へ割れない）
    let src = production(APP_REL);
    let (body, at) = fn_body(APP_REL, &src, INITIAL_FN);
    let text = joined(&body);
    assert!(
        text.contains("wb::MIN_WIDTH") && text.contains("wb::MIN_HEIGHT"),
        "{APP_REL}:{at}: 保存フレームの下限が `wb::MIN_*` を見ていない \
         = `window resize` が撥ねる下限と割れる（#1442）"
    );
}

// --------------------------------------------- 3. 撥ねる条件

#[test]
fn 撥ねる条件が2つとも在る() {
    let src = production(CORE_REL);
    let (body, at) = fn_body(CORE_REL, &src, RESOLVE_FN);
    let text = joined(&body);
    assert!(
        text.contains("MIN_WIDTH") && text.contains("MIN_HEIGHT"),
        "{CORE_REL}:{at}: 最小寸法の検査が無い = 1x1 の窓を作れてしまう（#1442）"
    );
    assert!(
        text.contains("d.contains("),
        "{CORE_REL}:{at}: 置き先からのはみ出しの検査が無い \
         = 画面の外へ窓を出して「消えた」に見える（#1442）"
    );
    // 撥ねたら理由を返す（呼び出し側が診断へ出せる形）
    assert!(
        text.contains("return Err(") && text.contains("Err(format!("),
        "{CORE_REL}:{at}: 撥ねた理由を返していない（#1442）"
    );
}

// --------------------------------------------- 4. 解釈は 1 実装

#[test]
fn 座標の解釈は1実装() {
    // 置き先の原点を足すのは `resolve` だけ。UI 層・dispatch が自前で足すと、
    // env と CLI / MCP で「ディスプレイ内の座標」の意味がズレる
    let core = production(CORE_REL);
    let (body, at) = fn_body(CORE_REL, &core, RESOLVE_FN);
    let text = joined(&body);
    assert!(
        text.contains("(d.x + rx, d.y + ry)"),
        "{CORE_REL}:{at}: 置き先の原点を足す変換が無い \
         = Windows のグローバル座標で「ディスプレイ内の座標」にならない（#1442）"
    );

    let dispatch = production(DISPATCH_REL);
    let (op_body, op_at) = fn_body(DISPATCH_REL, &dispatch, GEOMETRY_OP_FN);
    let op = joined(&op_body);
    assert!(
        op.contains("wb::resolve("),
        "{DISPATCH_REL}:{op_at}: {GEOMETRY_OP_FN} が `window_bounds::resolve` を通っていない \
         = CLI / MCP だけ別の解釈になる（#1442）"
    );
    assert!(
        op.contains("host.target_display_rect()"),
        "{DISPATCH_REL}:{op_at}: 置き先のディスプレイを見ていない（#1442）"
    );

    // dispatch の 2 つの入口は同じ実体を呼ぶ（片方だけ直す形を落とす）
    let move_arm = dispatch
        .lines()
        .position(|l| l.contains("Request::WindowMove { window, x, y } =>"))
        .unwrap_or_else(|| panic!("{DISPATCH_REL}: WindowMove の分岐が無い（#1442）"));
    let resize_arm = dispatch
        .lines()
        .position(|l| l.contains("Request::WindowResize {"))
        .unwrap_or_else(|| panic!("{DISPATCH_REL}: WindowResize の分岐が無い（#1442）"));
    for (arm, name) in [(move_arm, "WindowMove"), (resize_arm, "WindowResize")] {
        let window: String = dispatch
            .lines()
            .skip(arm)
            .take(14)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            window.contains("window_geometry_op("),
            "{DISPATCH_REL}:{}: {name} が {GEOMETRY_OP_FN} を通っていない（#1442）",
            arm + 1
        );
    }
}

// --------------------------------------------- 5. CLI / MCP は 1 対 1

#[test]
fn cliとmcpは1対1() {
    let protocol = production(PROTOCOL_REL);
    for req in ["WindowMove {", "WindowResize {"] {
        assert!(
            protocol.contains(req),
            "{PROTOCOL_REL}: Request::{req} が無い（#1442）"
        );
    }

    let cli = production(CLI_REL);
    let mcp = production(MCP_REQUEST_REL);
    let catalog = production(MCP_CATALOG_REL);
    // (何を, CLI 側の目印, MCP 側の目印)
    let pairs: &[(&str, &str, &str)] = &[
        (
            "move",
            "WindowCommand::Move {",
            "\"move\" => Request::WindowMove {",
        ),
        (
            "resize",
            "WindowCommand::Resize {",
            "\"resize\" => Request::WindowResize {",
        ),
    ];
    let missing: Vec<String> = pairs
        .iter()
        .flat_map(|(name, cli_needle, mcp_needle)| {
            let mut out = Vec::new();
            // 落ちたときの手がかりは「その口を並べている場所」を名指す
            let cli_at = line_of(&cli, "enum WindowCommand {");
            let mcp_at = line_of(&mcp, "\"tako_window\" => {");
            let catalog_at = line_of(&catalog, "\"name\": \"tako_window\",");
            if !cli.contains(cli_needle) {
                out.push(format!(
                    "{CLI_REL}:{cli_at}: `tako window {name}` が無い（MCP にだけ在る = 設計原則 5 違反。#1442）"
                ));
            }
            if !mcp.contains(mcp_needle) {
                out.push(format!(
                    "{MCP_REQUEST_REL}:{mcp_at}: MCP の action `{name}` が無い（CLI にだけ在る = 設計原則 5 違反。#1442）"
                ));
            }
            if !catalog.contains(&format!("\"{name}\"")) {
                out.push(format!(
                    "{MCP_CATALOG_REL}:{catalog_at}: tako_window の enum に `{name}` が無い（AI から引けない。#1442）"
                ));
            }
            out
        })
        .collect();
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

// --------------------------------------------- 6. A/B の逃げ道

#[test]
fn abの逃げ道は1つ() {
    let app = production(APP_REL);
    assert!(
        app.contains("fn legacy_1442()") && app.contains("TAKO_1442_LEGACY"),
        "{APP_REL}: #1442 の A/B（TAKO_1442_LEGACY）が消えている \
         = 同じバイナリで回帰を隠していないことを示せない"
    );
    // 逃げ道は「起動時」と「起動後」の両方へ配線されている
    let src = production(APP_REL);
    for (sig, why) in [
        (REQUESTED_FN, "起動時の指定"),
        (APPLY_FN, "起動後の move / resize"),
    ] {
        let (body, at) = fn_body(APP_REL, &src, sig);
        assert!(
            find(&body, "legacy_1442()").is_some(),
            "{APP_REL}:{at}: {sig} が legacy_1442() を見ていない（{why}だけ A/B が効かない。#1442）"
        );
    }
    // 逃げ道が 2 本目を生やしていないか（env 名は 1 つ）
    let extra: Vec<String> = ["TAKO_WINDOW_BOUNDS_LEGACY", "TAKO_1442_LEGACY2"]
        .iter()
        .filter(|n| app.contains(**n))
        .map(|n| format!("{APP_REL}: 2 本目の逃げ道 {n} が生えている（#1442）"))
        .collect();
    assert!(extra.is_empty(), "{}", extra.join("\n"));
}

// --------------------------------------------- 7. 結果を読む口

#[test]
fn window_listが矩形を返す() {
    let src = production(DISPATCH_REL);
    let (body, at) = fn_body(DISPATCH_REL, &src, "fn windows_json(");
    let text = joined(&body);
    assert!(
        text.contains("\"bounds\""),
        "{DISPATCH_REL}:{at}: window list に矩形（bounds）が載っていない \
         = 指定した結果を AX なしで読めない（#1442 の検証手段そのもの）"
    );
    assert!(
        text.contains("host.window_frame("),
        "{DISPATCH_REL}:{at}: 矩形の供給元（host.window_frame）を通っていない（#1442）"
    );
    assert!(
        text.contains("\"display\""),
        "{DISPATCH_REL}:{at}: 置き先のディスプレイが載っていない \
         = bounds を「ディスプレイ内の座標」として読み直せない（#1442）"
    );
}

// --------------------------------------------- 9. 連続操作の合成

/// `window move` の直後に `window resize` を撃つと、resize が**古い位置**を土台にして
/// 移動を打ち消す（`window_frames` は render でしか更新されないため）。
/// 実測: セルフテスト項目 77b が `指定 40,40,1000,700 → 実測 800,420,1000,700` で落ちた
#[test]
fn 連続操作は合成される() {
    let src = production(APP_REL);
    let (body, at) = fn_body(APP_REL, &src, "fn request_window_geometry(");
    let text = joined(&body);
    assert!(
        text.contains("self.pending_window_geometry.push("),
        "{APP_REL}:{at}: 依頼が UI 層の消費待ちへ積まれていない（#1442）"
    );
    assert!(
        text.contains("self.window_frames.insert("),
        "{APP_REL}:{at}: 依頼した矩形をその場で記録していない \
         = `window move` の直後の `window resize` が古い位置を土台にして移動を打ち消す \
         （実測でセルフテスト項目 77b が落ちた形。#1442）"
    );
}

// --------------------------------------------- 8. AX を tako へ向けない

#[test]
fn axで動かす道をtakoへ向けない() {
    let vd = read(VD_REL);
    assert!(
        vd.contains("vd_is_tako_process"),
        "{VD_REL}: AX で窓を動かす道が tako を拒んでいない \
         （#1442: AX は複数の tako-app を同一プロセスとして返し本番の窓に当たる）"
    );
    let start = vd
        .lines()
        .position(|l| l.starts_with("vd_move_window()"))
        .unwrap_or_else(|| panic!("{VD_REL}: vd_move_window が見つからない"));
    let guard = vd
        .lines()
        .skip(start)
        .take(12)
        .any(|l| l.contains("vd_is_tako_process"));
    assert!(
        guard,
        "{VD_REL}:{}: vd_move_window が osascript を撃つ前に tako を弾いていない（#1442）",
        start + 1
    );
    assert!(
        vd.contains("vd_tako_window_env") && vd.contains("TAKO_WINDOW_BOUNDS"),
        "{VD_REL}: tako 側の口（TAKO_WINDOW_BOUNDS）への案内が無い（#1442）"
    );
}
