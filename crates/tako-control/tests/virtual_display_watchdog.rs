//! 仮想ディスプレイまわりの番犬（#1141 / #1150 / #1160 / #1697 / #1783 / #1817）
//!
//! 守りたい不変条件は 12 個。どれも「壊れても動いているように見える」ので、
//! 人間の記憶ではなくテストで固定する。
//!
//! 1. **ヘルパは消す機能を持たない**。`tako-vd` は常設で、検証のたびに作り直さない
//!    （消す手段があると、いつか「後片付け」のつもりで消され、次の検証で
//!    ユーザーの画面に窓が出る）
//! 2. **既定名が Rust とシェルでずれない**（ずれると隔離起動だけ黙ってメイン画面へ落ちる）
//! 3. **tako が開く窓は全部同じ面へ出す**。1 枚でも素の中央寄せが残ると、
//!    セルフテストが開く設定画面などがユーザーの画面へ飛び出す
//! 4. **`ensure` が成功で返る道はすべて締め（増殖の検査 → 起きているかの確認 →
//!    Main 保護）を通る**（#1150 / #1160）。片方の道だけ通していると
//!    「既に在るから何もしない」の側から漏れる
//! 5. **孤児の後片付けは実行条件を確かめてから器を再起動する**（#1150）。順番が逆・
//!    条件を見ないと、内蔵が居ない機で**画面が 0 枚になり機械が眠る**（走っている
//!    worker が全部巻き添えになる）
//! 6. **見張りのモックテストが CI で走る**（画面の無いランナーでも回るようスタブ実装）
//! 7. **置き先は列挙を 1 回引いただけで諦めない**（#1160）。ディスプレイスリープ中は
//!    `cx.displays()`（= `CGGetActiveDisplayList`）が 0 件になるので、1 回で諦めると
//!    面が在るのにユーザーのメイン画面へ落ちる
//! 8. **置き先は窓を 1 枚も開く前に決まる**（#1160）。決める前に開くと、あとから
//!    「開かずに終わる」ことができない（出てしまった窓は取り返せない）
//! 9. **名前が読めない瞬間に面を見失わない**（#1697）。`ensure` の締めが面の uuid を
//!    記録し（置き場は Rust とシェルで同じ）、`resolve_target_display` は記録済み uuid と
//!    「明示の指定か」を核から取る。どちらかが外れると、名前だけが読めない起動で
//!    生きている `tako-vd` を見失い、ユーザーのメイン画面へ窓が出る
//! 10. **名前が一致しても物理画面なら使わない**（#1783）。`ensure` の締めが**最初に**
//!     「器が作った仮想ディスプレイか」を確かめ、物理画面（内蔵 / 器の一覧に無い面）なら
//!     終了コード `VD_RC_NOT_VIRTUAL` で断る。後ろに置くと物理画面の uuid を記録し、
//!     内蔵なら Main 保護が器へ main を撃つ（`TAKO_VD_NAME` の取り違え 1 つで起きる）
//! 11. **器に作らせる・繋がせるのは常設の `tako-vd` だけ**（#1817）。OS の一覧に居ない名前を
//!     何でも作らせると、存在しない名前・打ち間違い・蓋閉じで一覧に居ない内蔵の名前で器に
//!     仮想スクリーンが増える（孤児は器から消せない = #1150）。名前の判定は器に触る前
//! 12. **`bounds` / `status` / `move-window` も締めと同じ判定を通す**（#1817）。座標は窓の置き場に
//!     使われるので、物理画面の名前に座標や「使用可」を返すとユーザーの画面へ窓を出しうる。
//!     読むだけの口なので器（BetterDisplay）を起動しない

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn helper_source() -> String {
    let p = repo_root().join("scripts/lib/virtual-display.sh");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("読めない {}: {e}", p.display()))
}

/// 行コメントを落とした実行部分だけ（説明文の中の語で誤検知しないため）
fn code_lines(src: &str) -> Vec<(usize, &str)> {
    src.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with('#'))
        .collect()
}

#[test]
fn 仮想ディスプレイのヘルパは消す機能を持たない() {
    // 器へ「消す / 切る」を頼む語。ヘルパの実行部分に 1 つも現れてはいけない
    const DESTRUCTIVE: &[&str] = &["discard", "-connected=off", "connected=off"];
    let src = helper_source();
    let mut hits = Vec::new();
    for (line_no, line) in code_lines(&src) {
        for needle in DESTRUCTIVE {
            if line.contains(needle) {
                hits.push(format!(
                    "scripts/lib/virtual-display.sh:{line_no}: {needle}"
                ));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "仮想ディスプレイを消す / 切る操作がヘルパに入っている:\n  {}\n\
         → tako-vd は常設（#1141）。消す手段を持たせない。\n\
           後片付けのつもりで消されると、次の検証でユーザーの画面に窓が出る",
        hits.join("\n  "),
    );
}

#[test]
fn ヘルパは必要なサブコマンドをそろえている() {
    let src = helper_source();
    for sub in [
        "ensure)",
        "bounds)",
        "status)",
        "move-window)",
        "cleanup-orphans)",
    ] {
        assert!(
            src.contains(sub),
            "ヘルパにサブコマンド {sub} が無い（#1141 の受け入れ条件）"
        );
    }
}

#[test]
fn 既定の仮想ディスプレイ名がrustとシェルでそろっている() {
    let name = tako_core::platform::display::DEFAULT_VIRTUAL_DISPLAY_NAME;
    let src = helper_source();
    // シェル側の既定値（VD_DEFAULT_NAME。#1817 から器に作らせてよい唯一の名前も兼ねる）
    let shell_default = src
        .lines()
        .find_map(|l| l.trim().strip_prefix("VD_DEFAULT_NAME="))
        .map(str::to_string);
    assert!(
        src.lines()
            .any(|l| l.trim() == "VD_NAME=${TAKO_VD_NAME:-$VD_DEFAULT_NAME}"),
        "scripts/lib/virtual-display.sh:{}: TAKO_VD_NAME の既定が VD_DEFAULT_NAME を指していない（#1817）。\n\
         → 既定名と「器に作らせてよい名前」が 2 か所に分かれると、片方だけ直したときにずれる",
        line_of(&src, "VD_NAME=${TAKO_VD_NAME:-")
    );
    assert_eq!(
        shell_default.as_deref(),
        Some(name),
        "既定の仮想ディスプレイ名が Rust（{name}）とシェルでずれている。\n\
         → ずれると隔離起動だけが黙ってメイン画面へ落ちる（症状が出るのは\n\
           「窓が邪魔」という報告だけで、ログは正常に見える）"
    );
    // 案内する docs も同じ名前であること（#1139 以降、コマンドの全文は
    // `.agent/commands.md` に移り AGENTS.md は 1 行の索引だけになった）
    for doc in [".agent/commands.md", ".agent/conventions.md"] {
        let text = std::fs::read_to_string(repo_root().join(doc)).unwrap_or_default();
        assert!(
            text.contains(name),
            "{doc} が既定名 {name} を案内していない（#1141）"
        );
    }
}

#[test]
fn takoが開く窓は全部置き先を通る() {
    let p = repo_root().join("crates/tako-app/src/main.rs");
    let src = std::fs::read_to_string(&p).expect("main.rs を読む");
    let mut hits = Vec::new();
    for (i, line) in src.lines().enumerate() {
        // doc コメントの中の見本は対象外（説明でこの形を書けるようにしておく）
        if line.trim_start().starts_with("///") || line.trim_start().starts_with("//") {
            continue;
        }
        if line.contains("Bounds::centered(None") {
            hits.push(format!("crates/tako-app/src/main.rs:{}", i + 1));
        }
    }
    assert!(
        hits.is_empty(),
        "置き先を通さない中央寄せが残っている:\n  {}\n\
         → `centered_on_target()` を使うこと（#1141 / FR-4.8.5）。\n\
           1 枚でも素の中央寄せが残ると、セルフテストが開く設定画面などが\n\
           ユーザーのメイン画面へ飛び出す",
        hits.join("\n  "),
    );
}

/// シェル関数の本体（`名前() {` から列 0 の `}` まで）を取り出す。
/// ヘルパの関数はすべてトップレベルなので、この素朴な切り出しで足りる
fn shell_fn_body(src: &str, name: &str) -> String {
    let head = format!("{name}() {{");
    let start = src
        .find(&head)
        .unwrap_or_else(|| panic!("シェル関数 {name} が見つからない"));
    let rest = &src[start + head.len()..];
    let end = rest
        .find("\n}")
        .unwrap_or_else(|| panic!("シェル関数 {name} の終わりが見つからない"));
    rest[..end].to_string()
}

/// 行コメントを落とした実行部分だけを 1 本の文字列に
fn code_only(body: &str) -> String {
    body.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `ensure` は**成功で返るすべての道**で締め（増殖の検査 → Main 保護）を通る。
///
/// 「既に在るから何もしない」の早期 return を締めの外に置くと、
/// **増殖したまま / Main が仮想のまま**返る道が残る（#1150 で実際に踏んだ形。
/// 蓋閉じのあいだに Main が `tako-vd` へ移り、以降の ensure は何も気づかなかった）。
#[test]
fn ensureが成功で返る道はすべて締めを通る() {
    let src = helper_source();
    let body = code_only(&shell_fn_body(&src, "vd_ensure"));
    let finishes = body.matches("vd_finish_ensure").count();
    assert!(
        finishes >= 2,
        "vd_ensure の中で締め（vd_finish_ensure）を通る箇所が {finishes} 個しかない。\n\
         → 「既に在る」道と「繋いだ」道の両方が通ること（#1150）"
    );
    let bare: Vec<&str> = body
        .lines()
        .filter(|l| l.trim() == "return 0")
        .map(str::trim)
        .collect();
    assert!(
        bare.is_empty(),
        "vd_ensure に締めを通らない `return 0` がある（{} 箇所）。\n\
         → 成功で返るときは vd_finish_ensure の結果を返すこと（#1150）",
        bare.len()
    );
}

/// 締めは**増殖の検査と Main 保護の両方**を呼ぶ（片方だけ残ると症状が半分だけ直る）
#[test]
fn 締めは増殖の検査とmain保護の両方を通す() {
    let src = helper_source();
    let body = code_only(&shell_fn_body(&src, "vd_finish_ensure"));
    for needle in ["vd_assert_single", "vd_protect_main"] {
        assert!(
            body.contains(needle),
            "vd_finish_ensure が {needle} を呼んでいない（#1150）。\n\
             → 増殖の検査（同名が 2 枚以上なら止まる）と Main 保護（内蔵が居るときだけ\n\
               内蔵へ戻す）は両方が締めに要る"
        );
    }
}

/// Main 保護は**内蔵が居るときだけ**動く（戻す先が無い機で仮想を Main に据え直さない）
#[test]
fn main保護は内蔵が居ないとき何もしない() {
    let src = helper_source();
    let body = code_only(&shell_fn_body(&src, "vd_main_protection_plan"));
    assert!(
        body.contains("builtin_id") && body.contains("noop"),
        "vd_main_protection_plan が内蔵の有無で分岐していない（#1150）"
    );
    // 判定は純関数（stdin のテーブルだけを見る）= 器へ触らない
    assert!(
        !body.contains("vd_bd"),
        "Main 保護の判定が器（vd_bd）を呼んでいる。判定は純関数のままにする\n\
         （テストがスタブ無しで 3 ケースを回せる形を壊さない）"
    );
}

/// 孤児の後片付けは**実行条件を確かめてから**器を再起動する。
///
/// 器の再起動は一瞬すべての仮想ディスプレイを落とすので、内蔵が NSScreen に居ない機で
/// 走らせると**画面が 0 枚になり機械が眠る**（#1150: 蓋閉じで `tako-vd` が唯一の画面。
/// 走っている worker が全部巻き添えになる）。
#[test]
fn 孤児の後片付けは実行条件を確かめてから器を再起動する() {
    let src = helper_source();
    let body = code_only(&shell_fn_body(&src, "vd_cleanup_orphans"));
    let gate = body
        .find("vd_cleanup_gate")
        .expect("vd_cleanup_orphans が実行条件（vd_cleanup_gate）を見ていない（#1150）");
    let restart = body
        .find("restartApp")
        .expect("vd_cleanup_orphans に器の再起動が無い（掃除の手段はこれだけ）");
    assert!(
        gate < restart,
        "器の再起動が実行条件の確認より先にある（#1150）。\n\
         → 内蔵が居ない / 蓋が閉じているときは撃ってはいけない"
    );
    // 器を再起動するのは apply のときだけ（既定は下見）
    assert!(
        body.contains("--dry-run") && body.contains("--apply"),
        "cleanup-orphans が下見（--dry-run）と実行（--apply）を分けていない（#1150）"
    );
}

/// 見張りのモックテストが CI で走る（実ディスプレイの無いランナーでも回る）
#[test]
fn 見張りのモックテストがciで走る() {
    const SCRIPT: &str = "scripts/test-virtual-display-guard.sh";
    assert!(
        repo_root().join(SCRIPT).is_file(),
        "{SCRIPT} が無い（#1150 の番犬の実体）"
    );
    let ci = std::fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect("ci.yml を読む");
    assert!(
        ci.contains(SCRIPT),
        "{SCRIPT} が CI に載っていない（載っていないと壊れても誰も気づかない）"
    );
}

/// 締めは**面が起きている（描画可能）ことまで**確かめる（#1160）。
///
/// `NSScreen` に居る = 窓を置ける、ではない: ディスプレイスリープ中は NSScreen に
/// 残ったまま CoreGraphics の active 一覧から落ち、GPUI（`cx.displays()` =
/// `CGGetActiveDisplayList`）から見えなくなる。ここを完了条件に入れないと
/// `ensure` が「用意できた」と嘘をつき、検証が始まらない理由が分からなくなる。
#[test]
fn 締めは面が起きていることまで確かめる() {
    let src = helper_source();
    let body = code_only(&shell_fn_body(&src, "vd_finish_ensure"));
    assert!(
        body.contains("vd_ensure_drawable"),
        "vd_finish_ensure が起きているかの確認（vd_ensure_drawable）を通っていない（#1160）。\n\
         → NSScreen に居るだけで通すと、面は在るのに tako から見えず検証が始まらない"
    );
    // 眠っていたら起こす手立てを持ち、結果は読み戻して確かめる（応答を当てにしない作法）
    let drawable = code_only(&shell_fn_body(&src, "vd_ensure_drawable"));
    for needle in ["vd_wake_displays", "vd_row_drawable"] {
        assert!(
            drawable.contains(needle),
            "vd_ensure_drawable が {needle} を呼んでいない（#1160）。\n\
             → 起こす手立てと、起きたかどうかの読み戻しの両方が要る"
        );
    }
}

/// 描画可能かの判定は **CoreGraphics に聞く**（#1160）。
///
/// NSScreen で代用すると #1160 の症状そのもの（眠っている面を「置ける」と読む）になる。
/// tako 側の物差しは `CGGetActiveDisplayList` なので、シェル側も同じ物差しを使う。
#[test]
fn 描画可能かはcoregraphicsに聞く() {
    let src = helper_source();
    let body = shell_fn_body(&src, "vd_drawable");
    assert!(
        body.contains("CGDisplayIsActive"),
        "vd_drawable が CGDisplayIsActive を見ていない（#1160）。\n\
         → NSScreen（vd_screens）は眠っている面も出し続けるので代用できない"
    );
    // 起こす手立ては「ユーザー活動の宣言」だけ。構成（解像度・配置・Main）は触らない
    let wake = code_only(&shell_fn_body(&src, "vd_wake_displays"));
    assert!(
        wake.contains("caffeinate"),
        "vd_wake_displays が caffeinate 以外の手段で起こそうとしている（#1160）"
    );
    for forbidden in ["vd_bd", "-main=on", "restartApp", "resolution"] {
        assert!(
            !wake.contains(forbidden),
            "起こすために {forbidden} を撃っている。\n\
             → 起こすのはユーザー活動の宣言だけ（ユーザーのディスプレイ構成は変えない）"
        );
    }
}

/// Rust 関数の本体（`シグネチャ` から列 0 の `}` まで）を取り出す。
/// 対象はトップレベル関数だけなので、この素朴な切り出しで足りる
fn rust_fn_body(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("Rust 関数 {signature} が見つからない"));
    let rest = &src[start + signature.len()..];
    let end = rest
        .find("\n}")
        .unwrap_or_else(|| panic!("Rust 関数 {signature} の終わりが見つからない"));
    rest[..end].to_string()
}

fn app_main_source() -> String {
    let p = repo_root().join("crates/tako-app/src/main.rs");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("読めない {}: {e}", p.display()))
}

/// 置き先の解決は**列挙を 1 回引いただけで諦めない**（#1160）。
///
/// macOS の `cx.displays()` は `CGGetActiveDisplayList` なので、ディスプレイスリープ中は
/// 面が在っても 0 件になる（実測: `NSScreen` に 2 枚残ったまま `CGDisplayIsActive` が
/// 両方 0）。1 回引いて `NotFound` にすると、**面が在るのにユーザーのメイン画面へ
/// 窓が出る**（#1160 の症状）。
#[test]
fn 置き先の解決は列挙を1回で諦めない() {
    let src = app_main_source();
    let body = rust_fn_body(
        &src,
        "fn resolve_target_display(cx: &App) -> Option<gpui::DisplayId> {",
    );
    // 構えは核（tako_core::platform::display）から取る = 判定を UI 層へ散らさない
    for needle in ["retry_policy", "miss_for"] {
        assert!(
            body.contains(needle),
            "resolve_target_display が {needle} を通していない（#1160）。\n\
             → 「やり直す回数」「外したときどうするか」は核の 1 か所で決める"
        );
    }
    let loop_at = body
        .find("loop {")
        .expect("resolve_target_display にやり直しの loop が無い（#1160）");
    let enumerate_at = body
        .find("cx.displays()")
        .expect("resolve_target_display が cx.displays() を引いていない");
    assert!(
        loop_at < enumerate_at,
        "cx.displays() がやり直しの loop の外にある（#1160）。\n\
         → 起動の瞬間だけ列挙が空になる（ディスプレイスリープ）ので、\n\
           1 回引いただけで諦めるとユーザーのメイン画面へ落ちる"
    );
    // 落とさない道（窓を開かずに終わる）と、落としたときに黙らない道の両方が在ること
    for needle in ["refusal_notice", "REFUSED_EXIT_CODE", "fallback_notice"] {
        assert!(
            body.contains(needle),
            "resolve_target_display に {needle} が無い（#1160）。\n\
             → 検証用 GUI は置き先が無いとき既定の面（= ユーザーの画面）へ落ちず、\n\
               理由を見せて窓を開かずに終わる"
        );
    }
}

/// 置き先は**窓を 1 枚も開く前**に決まる（#1160）。
///
/// 決める前に開くと「開かずに終わる」ができない（出てしまった窓は取り返せない）。
#[test]
fn 置き先は窓を開く前に決まる() {
    let src = app_main_source();
    let body = rust_fn_body(&src, "fn main() {");
    let resolve_at = body
        .find("resolve_target_display(cx)")
        .expect("main が resolve_target_display を呼んでいない（#1141）");
    let open_at = body
        .find("open_primary_window(")
        .expect("main が最初のウインドウを開いていない");
    assert!(
        resolve_at < open_at,
        "窓を開いたあとで置き先を決めている（#1160）。\n\
         → 置き先が無いときに「開かずに終わる」ことができなくなる"
    );
}

// ── #1697: 名前が読めない瞬間に面を見失わない ─────────────────────────

/// ソース中でその文字列が最初に現れる行（1 始まり。file:line の名指し用）
fn line_of(src: &str, needle: &str) -> usize {
    // 改行の数で数える（`lines()` は行の途中で切った末尾も 1 行と数えるので、
    // 行の途中に在る目印では 1 行ずれる）
    src.find(needle)
        .map(|at| src[..at].matches('\n').count() + 1)
        .unwrap_or(0)
}

/// 記録済み uuid の置き場が **Rust とシェルで同じ**（#1697）。
///
/// ずれると `ensure` は書いたつもりで tako は読めず、名前が読めない瞬間に
/// 面を見失う（= #1697 の症状がそのまま残る。ログは「記録も無い」と言うだけ）
#[test]
fn 記録済みuuidの置き場がrustとシェルでそろっている() {
    use tako_core::platform::display::{ENV_RECORD_DIR, RECORD_SUBDIR};
    let src = helper_source();
    let code = code_lines(&src)
        .into_iter()
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n");
    let at = line_of(&src, "VD_RECORD_DIR=");
    let env_line = format!("VD_RECORD_DIR=${{{ENV_RECORD_DIR}:-}}");
    assert!(
        code.contains(&env_line),
        "scripts/lib/virtual-display.sh:{at}: 記録の置き場を {ENV_RECORD_DIR} で差し替えられない（#1697）。\n\
         → Rust 側（platform::display::ENV_RECORD_DIR）と同じ env 名を読むこと: {env_line}"
    );
    let home_line = format!("VD_RECORD_DIR=\"$HOME/{RECORD_SUBDIR}\"");
    assert!(
        code.contains(&home_line),
        "scripts/lib/virtual-display.sh:{at}: 記録の既定の置き場が Rust（~/{RECORD_SUBDIR}）とずれている（#1697）。\n\
         → 期待: {home_line}"
    );
}

/// 締めは**面が起きていると確かめたあとで uuid を記録する**（#1697）。
/// 記録は失敗しても ensure を失敗させない（面は用意できている = 検証は進められる）
#[test]
fn 締めは起きている面のuuidを記録する() {
    let src = helper_source();
    let body = code_only(&shell_fn_body(&src, "vd_finish_ensure"));
    let at = line_of(&src, "vd_finish_ensure() {");
    let drawable = body.find("vd_ensure_drawable");
    let record = body.find("vd_record_uuid");
    assert!(
        record.is_some(),
        "scripts/lib/virtual-display.sh:{at}: vd_finish_ensure が vd_record_uuid を呼んでいない（#1697）。\n\
         → 記録が無いと、名前が読めない瞬間に tako が tako-vd を見失う"
    );
    assert!(
        drawable < record,
        "scripts/lib/virtual-display.sh:{at}: uuid の記録が「起きているかの確認」より前に在る（#1697）。\n\
         → 眠っていて ensure が止まる道で記録を書き換えない"
    );
    let rec_body = code_only(&shell_fn_body(&src, "vd_record_uuid"));
    let rec_at = line_of(&src, "vd_record_uuid() {");
    assert!(
        !rec_body.contains("return 1"),
        "scripts/lib/virtual-display.sh:{rec_at}: vd_record_uuid が失敗を返す（#1697）。\n\
         → 記録できなくても ensure は成功させる（面は用意できている）"
    );
    // 無効な displayID を JXA へ渡すと osascript が SIGSEGV で落ちる（実測）ので、
    // uuid は NSScreen に居る面にだけ聞く
    let uuid_body = shell_fn_body(&src, "vd_display_uuid");
    assert!(
        uuid_body.contains("NSScreen.screens")
            && uuid_body.contains("CGDisplayCreateUUIDFromDisplayID"),
        "scripts/lib/virtual-display.sh:{}: vd_display_uuid が NSScreen に居る面に絞らずに \
         CGDisplayCreateUUIDFromDisplayID を呼んでいる（#1697。無効な ID で osascript が落ちる）",
        line_of(&src, "vd_display_uuid() {")
    );
}

/// 置き先の解決は **記録済み uuid と「明示の指定か」を核から取る**（#1697）。
///
/// `disp::select`（記録を見ない）へ戻る・`miss_for` に明示を渡さない、のどちらでも
/// 名前だけが読めない起動で生きている `tako-vd` を見失い、面が見えているので
/// 既定の面（= ユーザーのメイン画面）へ窓が出る（2026-09-24 の実測）
#[test]
fn 置き先の解決は記録済みuuidと明示の指定を核から取る() {
    const REL: &str = "crates/tako-app/src/main.rs";
    let src = app_main_source();
    let sig = "fn resolve_target_display(cx: &App) -> Option<gpui::DisplayId> {";
    let at = line_of(&src, sig);
    // 肯定の存在確認はコメントを潰した眺めへ当てる（#1609: 説明文の綴りで緑にしない）
    let body = code_view::without_comments_checked(&rust_fn_body(&src, sig), REL);
    for (needle, why) in [
        (
            "disp::select_with_record(",
            "名前で当たらないとき記録済み uuid で当てる照合",
        ),
        ("disp::recorded_uuid(", "ensure が残した uuid の読み込み"),
        ("disp::explicit_request(", "TAKO_DISPLAY が明示かの判定"),
        (
            "disp::miss_reason(",
            "外した理由の文（名前が読めない / 記録の有無）",
        ),
        (
            "disp::inject_no_names()",
            "名前が読めない瞬間を再現する診断 env",
        ),
    ] {
        assert!(
            body.contains(needle),
            "{REL}:{at}: resolve_target_display が {needle} を通していない（#1697: {why}）"
        );
    }
    assert!(
        !body.contains("disp::select("),
        "{REL}:{}: resolve_target_display が記録を見ない disp::select で照合している（#1697）。\n\
         → 名前だけが読めない起動で tako-vd を見失う。disp::select_with_record を使う",
        line_of(&src, "disp::select(&spec")
    );
    // 引数の中に `is_empty()` のような括弧が入るので、対応する閉じ括弧まで数えて切る
    let call = body
        .find("disp::miss_for(")
        .map(|i| &body[i..])
        .and_then(|rest| {
            let mut depth = 0usize;
            rest.char_indices().find_map(|(at, c)| match c {
                '(' => {
                    depth += 1;
                    None
                }
                ')' => {
                    depth -= 1;
                    (depth == 0).then(|| &rest[..=at])
                }
                _ => None,
            })
        })
        .unwrap_or_default();
    assert!(
        call.contains("explicit"),
        "{REL}:{}: miss_for に「明示の指定か」を渡していない（#1697。呼び出し: {call:?}）。\n\
         → TAKO_DISPLAY で明示した面を見失った検証用 GUI が既定の面へ落ちる",
        line_of(&src, "disp::miss_for(")
    );
}

// ── #1783: 名前が一致しても仮想ディスプレイとは限らない ─────────────────────

/// 締めは**最初に**「器が作った仮想ディスプレイか」を確かめる（#1783）。
///
/// 後ろに置くと、`TAKO_VD_NAME` に物理画面の名前を渡されたときに uuid の記録（物理画面の
/// uuid を「その名前の面」の記録として残す）や Main 保護（物理画面を仮想と読んで器へ
/// main を撃つ）へ先に進む。終了コードは潰さずに返す（`|| return 1` だとヘルパが
/// 物理画面の拒否を「面を用意できない = 未実測」と読む）
#[test]
fn 締めは最初に仮想ディスプレイかを確かめる() {
    const REL: &str = "scripts/lib/virtual-display.sh";
    let src = helper_source();
    let body = code_only(&shell_fn_body(&src, "vd_finish_ensure"));
    let at = line_of(&src, "vd_finish_ensure() {");
    let virt = body.find("vd_assert_virtual").unwrap_or_else(|| {
        panic!(
            "{REL}:{at}: vd_finish_ensure が仮想ディスプレイかの確認（vd_assert_virtual）を通っていない（#1783）。\n\
             → TAKO_VD_NAME に物理画面の名前を渡すと、その面を tako-vd と見なして成功を返す"
        )
    });
    for later in [
        "vd_assert_single",
        "vd_ensure_drawable",
        "vd_record_uuid",
        "vd_protect_main",
    ] {
        let pos = body
            .find(later)
            .unwrap_or_else(|| panic!("{REL}:{at}: vd_finish_ensure が {later} を呼んでいない"));
        assert!(
            virt < pos,
            "{REL}:{at}: 仮想ディスプレイかの確認が {later} より後ろに在る（#1783）。\n\
             → 物理画面の名前で {later} へ先に進む（uuid の記録・Main 保護は物理画面に触る）"
        );
    }
    assert!(
        body.contains("vd_assert_virtual || return $?"),
        "{REL}:{at}: vd_assert_virtual の終了コードを潰している（#1783）。\n\
         → `vd_assert_virtual || return $?` で VD_RC_NOT_VIRTUAL をそのまま返す"
    );
    // 判定は純関数（器・画面に触らない）で、内蔵は CoreGraphics の列（builtin）で見る
    let verdict = code_only(&shell_fn_body(&src, "vd_virtual_verdict"));
    let vat = line_of(&src, "vd_virtual_verdict() {");
    for banned in ["vd_bd", "vd_screens", "vd_backend_list", "osascript"] {
        assert!(
            !verdict.contains(banned),
            "{REL}:{vat}: 仮想ディスプレイかの判定（vd_virtual_verdict）が {banned} を呼んでいる（#1783）。\n\
             → 判定は stdin のテーブルと引数だけを見る純関数のままにする（スタブ無しで回せる形）"
        );
    }
    assert!(
        verdict.contains("builtin"),
        "{REL}:{vat}: vd_virtual_verdict が内蔵（CGDisplayIsBuiltin の列）を見ていない（#1783）"
    );
}

/// 実際に `/bin/bash`（macOS 同梱の 3.2）で `virtual-display.sh` を走らせ、物理画面の名前では
/// `ensure` が断ることを見る（#1783）。画面の列挙・器・起こす手立てはスタブにして
/// **実ディスプレイに一切触らない**
#[cfg(unix)]
#[test]
fn 物理画面の名前ではensureが断る() {
    const REL: &str = "scripts/lib/virtual-display.sh";
    let src = helper_source();
    let at = line_of(&src, "vd_assert_virtual() {");
    // 判定の本体（内蔵 / 器の一覧）。理由の中身が違うときはこちらを名指す
    let vat = line_of(&src, "vd_virtual_verdict() {");
    let rc_not_virtual: i32 = src
        .lines()
        .find_map(|l| l.strip_prefix("VD_RC_NOT_VIRTUAL="))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("{REL}: `VD_RC_NOT_VIRTUAL=<数>` が見つからない"));

    // 抜けるとき（assert で落ちたときも）一時 dir を消す。**消す前に一時 dir の下であることを確かめる**
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            if self.0.starts_with(std::env::temp_dir()) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
    let scratch =
        Scratch(std::env::temp_dir().join(format!("tako-1783-vd-{}", std::process::id())));
    let dir = &scratch.0;
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).expect("一時 dir を作る");
    // 蓋を開けた MacBook + 外部モニタ + tako-vd（名前は一般的な既定名）
    let script = r#"
. "$HELPER"
vd_screens() { printf 'Color LCD\t0\t0\t1512\t982\t1\t1\t1\nExternal 4K\t1512\t0\t3840\t2160\t7\t0\t0\ntako-vd\t5352\t0\t2560\t1440\t13\t0\t0\n'; }
vd_drawable() { printf '1\t1\t0\n7\t1\t0\n13\t1\t0\n'; }
vd_display_uuid() { printf 'AAAAAAAA-0000-4000-8000-%012d' "${1:-0}"; }
vd_wake_displays() { echo wake >> "$STUB_LOG"; }
vd_backend_ready() { echo ready >> "$STUB_LOG"; return 0; }
vd_backend_list() { printf '17\t13\ttako-vd\n'; }
vd_bd() { echo "bd $*" >> "$STUB_LOG"; }
sleep() { :; }
vd_ensure 2>"$STUB_ERR"
"#;
    let run = |name: &str| {
        let case = dir.join(name.replace(' ', "_"));
        let _ = std::fs::remove_dir_all(&case);
        std::fs::create_dir_all(&case).expect("場面の dir を作る");
        let log = case.join("log");
        let err = case.join("err");
        let record = case.join("record");
        let status = std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(script)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &case)
            .env("HELPER", repo_root().join(REL))
            .env("TAKO_VD_NAME", name)
            .env("TAKO_VD_RECORD_DIR", &record)
            .env(
                "TAKO_VD_BETTERDISPLAY_APP",
                case.join("NoBetterDisplay.app"),
            )
            .env("STUB_LOG", &log)
            .env("STUB_ERR", &err)
            .status()
            .expect("/bin/bash を起こす");
        let recorded: Vec<String> = std::fs::read_dir(&record)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        (
            status.code().unwrap_or(-1),
            std::fs::read_to_string(&log).unwrap_or_default(),
            std::fs::read_to_string(&err).unwrap_or_default(),
            recorded,
        )
    };

    for (name, why) in [
        ("Color LCD", "内蔵ディスプレイ"),
        ("External 4K", "仮想スクリーン一覧に External 4K が無い"),
    ] {
        let (rc, log, err, recorded) = run(name);
        assert_eq!(
            rc, rc_not_virtual,
            "{REL}:{at}: TAKO_VD_NAME={name}（物理画面）で ensure が VD_RC_NOT_VIRTUAL（{rc_not_virtual}）で断らない（#1783）\n\
             stderr: {err}"
        );
        let lines: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).collect();
        assert!(
            lines.len() == 1
                && lines[0].starts_with(&format!("ERROR: {name} は仮想ディスプレイではない"))
                && lines[0].contains(why)
                && lines[0].contains("#1783"),
            "{REL}:{vat}: TAKO_VD_NAME={name} の理由が「ERROR: {name} は仮想ディスプレイではない…{why}…#1783」の 1 行ではない\
             （呼び口は {REL}:{at}）: {lines:?}"
        );
        assert!(
            !log.contains("main=on"),
            "{REL}:{at}: TAKO_VD_NAME={name} で Main 保護が器へ main を撃った（#1783。物理画面を仮想と読んでいる）\n{log}"
        );
        assert!(
            recorded.is_empty(),
            "{REL}:{at}: TAKO_VD_NAME={name} で物理画面の uuid を記録した（#1783）: {recorded:?}"
        );
        assert!(
            !log.contains("wake"),
            "{REL}:{at}: TAKO_VD_NAME={name} で面を起こす手立てを撃った（#1783。確認より先に進んでいる）"
        );
    }
    // 内蔵は器に聞かずに決まる（器を起こさない）
    let (_, log, _, _) = run("Color LCD");
    assert!(
        !log.contains("ready"),
        "{REL}:{vat}: 内蔵の名前で器に聞いている（#1783。内蔵は CoreGraphics の列だけで決まる。呼び口は {REL}:{at}）"
    );

    // 対照: 本物の tako-vd（器の一覧に居る非内蔵の面）は従来どおり成功して記録する
    let (rc, _, err, recorded) = run("tako-vd");
    assert_eq!(
        rc, 0,
        "{REL}:{at}: 本物の tako-vd で ensure が失敗した（#1783 で既定の道を壊した）: {err}"
    );
    assert_eq!(
        recorded,
        vec!["tako-vd.uuid".to_string()],
        "{REL}:{at}: 本物の tako-vd の uuid を記録していない（#1697 の道を壊した）"
    );
    assert!(
        err.trim().is_empty(),
        "{REL}:{at}: 本物の tako-vd で余計な出力: {err}"
    );
}

// ── #1817: 器に作らせるのは常設の 1 枚だけ・座標と状態も物理画面を断る ──────────────

/// `VD_RC_NOT_VIRTUAL=<数>` の値（ヘルパと番犬で番号をずらさない）
fn rc_not_virtual(src: &str) -> i32 {
    src.lines()
        .find_map(|l| l.strip_prefix("VD_RC_NOT_VIRTUAL="))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| {
            panic!("scripts/lib/virtual-display.sh: `VD_RC_NOT_VIRTUAL=<数>` が見つからない")
        })
}

/// 抜けるとき（assert で落ちたときも）一時 dir を消す。**消す前に一時 dir の下であることを確かめる**
#[cfg(unix)]
struct VdScratch(PathBuf);

#[cfg(unix)]
impl VdScratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tako-1817-vd-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時 dir を作る");
        Self(dir)
    }
}

#[cfg(unix)]
impl Drop for VdScratch {
    fn drop(&mut self) {
        if self.0.starts_with(std::env::temp_dir()) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// 本物の `virtual-display.sh` を source し、画面の列挙・器・AX をスタブにしたうえで
/// サブコマンドを 1 つ走らせる **スクリプト**（`/bin/bash` = macOS 同梱の 3.2 で走らせる）。
/// 偽の器は `create` で一覧へ足し、tagID 指定の `connected=on` でその面を OS のテーブルへ足す
/// （本物の作法 = 応答文ではなく読み戻しで確かめる、をスタブ側でも再現する）。
/// **実ディスプレイ・本物の BetterDisplay・System Events に一切触らない**
#[cfg(unix)]
const VD_STUBBED_RUN: &str = r#"
. "$HELPER"
vd_screens() { cat "$STUB_DIR/table"; }
vd_drawable() { awk -F'\t' 'NF { printf "%s\t1\t0\n", $6 }' "$STUB_DIR/table"; }
vd_display_uuid() { printf 'AAAAAAAA-0000-4000-8000-%012d' "${1:-0}"; }
vd_wake_displays() { echo wake >> "$STUB_DIR/log"; }
vd_backend_ready() { echo ready >> "$STUB_DIR/log"; return 0; }
vd_backend_responding() { echo responding >> "$STUB_DIR/log"; return 0; }
vd_backend_list() { cat "$STUB_DIR/list"; }
vd_backend_instances() { echo 1; }
vd_clamshell_raw() { :; }
vd_is_tako_process() { return 1; }
osascript() { echo "osascript $*" >> "$STUB_DIR/log"; }
vd_bd() {
    local all="$*" name tag
    echo "bd $all" >> "$STUB_DIR/log"
    case "$all" in
        create*-virtualScreenName=*)
            name=${all#*-virtualScreenName=}; name=${name%% -aspectWidth*}
            printf '90\t\t%s\n' "$name" >> "$STUB_DIR/list" ;;
        *-tagID=*-connected=on*)
            tag=${all#*-tagID=}; tag=${tag%% *}
            name=$(awk -F'\t' -v t="$tag" '$1 == t { print $3; exit }' "$STUB_DIR/list")
            [ -n "$name" ] && printf '%s\t9000\t0\t2560\t1440\t77\t0\t0\n' "$name" >> "$STUB_DIR/table" ;;
    esac
    return 0
}
sleep() { :; }
case "$SUB" in
    ensure) vd_ensure ;;
    bounds) vd_bounds ;;
    status) vd_status ;;
    move-window) vd_move_window 12345 ;;
    *) exit 99 ;;
esac
"#;

#[cfg(unix)]
struct VdRun {
    rc: i32,
    stdout: String,
    stderr: String,
    /// 器・起こす手立て・AX へ撃ったもの（1 行 1 件）
    log: String,
}

/// `VD_STUBBED_RUN` を 1 場面ぶん走らせる。`table` は OS に見えている面
/// （name<TAB>x<TAB>y<TAB>w<TAB>h<TAB>displayID<TAB>builtin<TAB>main）、`list` は器の仮想スクリーン一覧
#[cfg(unix)]
fn run_vd_stubbed(scratch: &VdScratch, name: &str, sub: &str, table: &str, list: &str) -> VdRun {
    let case = scratch
        .0
        .join(format!("{sub}-{}", name.replace([' ', '/'], "_")));
    let _ = std::fs::remove_dir_all(&case);
    std::fs::create_dir_all(&case).expect("場面の dir を作る");
    std::fs::write(case.join("table"), table).expect("テーブルを書く");
    std::fs::write(case.join("list"), list).expect("器の一覧を書く");
    let out = std::process::Command::new("/bin/bash")
        .arg("-c")
        .arg(VD_STUBBED_RUN)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &case)
        .env("HELPER", repo_root().join("scripts/lib/virtual-display.sh"))
        .env("TAKO_VD_NAME", name)
        .env("TAKO_VD_RECORD_DIR", case.join("record"))
        .env(
            "TAKO_VD_BETTERDISPLAY_APP",
            case.join("NoBetterDisplay.app"),
        )
        .env("STUB_DIR", &case)
        .env("SUB", sub)
        .output()
        .expect("/bin/bash を起こす");
    VdRun {
        rc: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        log: std::fs::read_to_string(case.join("log")).unwrap_or_default(),
    }
}

/// `ensure` が器に作らせる・繋がせるのは**常設の `tako-vd`（完全一致）だけ**（#1817）。
///
/// 以前は OS の一覧に居ない名前なら何でも器に作らせて繋いでいた（存在しない名前・打ち間違い・
/// 大文字違い・蓋閉じで一覧に居ない内蔵の名前でも）。作られた面は締めの判定（#1783）から見ると
/// 「器が作った仮想スクリーン」なので ensure は成功し、器には孤児が残る（#1150: 器から消せない）。
/// 名前の判定は**器に触る前**（BetterDisplay の起動も一覧の読み出しもしない）
#[cfg(unix)]
#[test]
fn ensureが器に作らせるのは常設の名前だけ() {
    const REL: &str = "scripts/lib/virtual-display.sh";
    let src = helper_source();
    let rc_nv = rc_not_virtual(&src);
    let at = line_of(&src, "vd_ensure() {");
    let cat = line_of(&src, "vd_create_verdict() {");

    // ① 作る・繋ぐ道の頭で名前を確かめる（器を起こす・作る・繋ぐより前）
    let body = code_only(&shell_fn_body(&src, "vd_ensure"));
    let gate = body.find("vd_assert_creatable || return $?").unwrap_or_else(|| {
        panic!(
            "{REL}:{at}: vd_ensure の作る・繋ぐ道が名前の確認（`vd_assert_creatable || return $?`）を通っていない（#1817）。\n\
             → OS の一覧に居ない名前なら何でも器に仮想スクリーンを作らせて繋ぐ"
        )
    });
    for later in [
        "vd_backend_ready",
        "create -type=VirtualScreen",
        "-connected=on",
    ] {
        let pos = body
            .find(later)
            .unwrap_or_else(|| panic!("{REL}:{at}: vd_ensure に {later} が見つからない"));
        assert!(
            gate < pos,
            "{REL}:{at}: 名前の確認が {later} より後ろに在る（#1817）。\n\
             → 器に触ってから断ると、BetterDisplay の起動・作成・接続のどれかが先に走る"
        );
    }
    let assert_body = code_only(&shell_fn_body(&src, "vd_assert_creatable"));
    assert!(
        assert_body.contains("vd_create_verdict")
            && assert_body.contains("$VD_DEFAULT_NAME")
            && assert_body.contains("return \"$VD_RC_NOT_VIRTUAL\""),
        "{REL}:{}: vd_assert_creatable が判定（vd_create_verdict \"$VD_NAME\" \"$VD_DEFAULT_NAME\"）を通して \
         VD_RC_NOT_VIRTUAL で断る形になっていない（#1817）",
        line_of(&src, "vd_assert_creatable() {")
    );

    // ② 判定は純関数で、通すのは常設の名前の完全一致だけ（大文字小文字・空白・接頭辞を広げない）
    let verdict = code_only(&shell_fn_body(&src, "vd_create_verdict"));
    for banned in ["vd_bd", "vd_screens", "vd_backend", "osascript", "pgrep"] {
        assert!(
            !verdict.contains(banned),
            "{REL}:{cat}: 作ってよい名前の判定（vd_create_verdict）が {banned} を呼んでいる（#1817）。\n\
             → 引数だけを見る純関数のままにする（器に触る前に決まる）"
        );
    }
    let names = [
        "tako-vd",
        "TAKO-VD",
        "Tako-Vd",
        " tako-vd",
        "tako-vd ",
        "tako-vd（1）",
        "tako-vd-ab1141",
        "tako-vd-watchdog-1817-absent",
        "Color LCD",
        "no-such-vd",
        "",
    ];
    let probe = std::process::Command::new("/bin/bash")
        .arg("-c")
        .arg(r#". "$HELPER"; for n in "$@"; do vd_create_verdict "$n" "$VD_DEFAULT_NAME" >/dev/null && printf '[%s]\n' "$n"; done; exit 0"#)
        .arg("probe")
        .args(names)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HELPER", repo_root().join(REL))
        .output()
        .expect("/bin/bash を起こす");
    let allowed = String::from_utf8_lossy(&probe.stdout).into_owned();
    assert_eq!(
        allowed.trim(),
        "[tako-vd]",
        "{REL}:{cat}: 器に作らせてよい名前が常設の tako-vd（完全一致）だけになっていない（#1817）。\n\
         → 試した名前: {names:?}"
    );

    // ③ 実際に走らせる: 蓋閉じ（内蔵は OS の一覧に居ない）で tako-vd だけが見えている構成
    let scratch = VdScratch::new("create");
    let lid_closed = "tako-vd\t0\t0\t2560\t1440\t6\t0\t1\n";
    let list = "17\t6\ttako-vd\n";
    for name in [
        "Color LCD",
        "no-such-vd",
        "TAKO-VD",
        "tako-vd-watchdog-1817-absent",
    ] {
        let r = run_vd_stubbed(&scratch, name, "ensure", lid_closed, list);
        assert_eq!(
            r.rc, rc_nv,
            "{REL}:{at}: TAKO_VD_NAME={name}（OS の一覧に居ない常設以外の名前）で ensure が \
             VD_RC_NOT_VIRTUAL（{rc_nv}）で断らない（#1817）\nstderr: {}\n器へ撃ったもの:\n{}",
            r.stderr, r.log
        );
        for shot in ["bd create", "connected=on", "ready", "responding"] {
            assert!(
                !r.log.contains(shot),
                "{REL}:{at}: TAKO_VD_NAME={name} で器に触った（{shot}。#1817。名前の確認より先に器へ進んでいる）\n{}",
                r.log
            );
        }
        let lines: Vec<&str> = r.stderr.lines().filter(|l| !l.trim().is_empty()).collect();
        assert!(
            lines.len() == 1
                && lines[0].starts_with(&format!("ERROR: {name} は仮想ディスプレイではない"))
                && lines[0].contains("#1817"),
            "{REL}:{}: TAKO_VD_NAME={name} の理由が「ERROR: {name} は仮想ディスプレイではない…#1817」の 1 行ではない: {lines:?}",
            line_of(&src, "vd_assert_creatable() {")
        );
    }

    // 対照: 常設の tako-vd は従来どおり器に作らせて繋ぐ
    let r = run_vd_stubbed(
        &scratch,
        "tako-vd",
        "ensure",
        "Color LCD\t0\t0\t1512\t982\t1\t1\t1\n",
        "",
    );
    assert_eq!(
        r.rc, 0,
        "{REL}:{at}: 器に無い tako-vd で ensure が失敗した（#1817 で常設の面の作成を壊した）: {}\n{}",
        r.stderr, r.log
    );
    for shot in [
        "bd create -type=VirtualScreen -virtualScreenName=tako-vd",
        "bd set -tagID=90 -connected=on",
    ] {
        assert!(
            r.log.contains(shot),
            "{REL}:{at}: 器に無い tako-vd で {shot} を撃っていない（#1817 で常設の面の作成を壊した）\n{}",
            r.log
        );
    }
}

/// `bounds` / `status` / `move-window` も締めと同じ判定（`vd_virtual_verdict`）を通す（#1817）。
///
/// 以前の 3 つは名前で面を引くだけで、`TAKO_VD_NAME` に物理画面の名前を渡すと座標・「使用可」を返し、
/// 窓をその面へ動かした。座標は検証スクリプトが窓の置き場に使うので、ユーザーの画面へ窓が出うる。
/// 3 つは読むだけ（と当座の AX）なので**器（BetterDisplay）を起動しない**（`--no-launch`）
#[cfg(unix)]
#[test]
fn boundsとstatusとmove_windowも物理画面を断る() {
    const REL: &str = "scripts/lib/virtual-display.sh";
    let src = helper_source();
    let rc_nv = rc_not_virtual(&src);

    for func in ["vd_bounds", "vd_status", "vd_move_window"] {
        let at = line_of(&src, &format!("{func}() {{"));
        let body = code_only(&shell_fn_body(&src, func));
        assert!(
            body.contains("vd_assert_virtual --no-launch")
                || body.contains("vd_check_virtual --no-launch"),
            "{REL}:{at}: {func} が仮想ディスプレイかの判定（vd_assert_virtual / vd_check_virtual --no-launch）を通っていない（#1817）。\n\
             → 物理画面の名前に座標・使用可を返す / その面へ窓を動かす"
        );
        assert!(
            !body.contains("vd_backend_ready"),
            "{REL}:{at}: {func} が器を起こしている（vd_backend_ready。#1817）。読むだけの口は BetterDisplay を起動しない"
        );
    }
    let responding = code_only(&shell_fn_body(&src, "vd_backend_responding"));
    assert!(
        responding.contains("pgrep") && !responding.contains("open "),
        "{REL}:{}: vd_backend_responding が「走っていれば 1 回聞く」だけになっていない（#1817。器を起動しない・\
         走っていない器の CLI は応答せず固まる = 癖 ①）",
        line_of(&src, "vd_backend_responding() {")
    );
    let check = code_only(&shell_fn_body(&src, "vd_check_virtual"));
    assert!(
        check.contains("--no-launch") && check.contains("vd_backend_responding"),
        "{REL}:{}: vd_check_virtual の --no-launch が器を起こさない口（vd_backend_responding）へ分かれていない（#1817）",
        line_of(&src, "vd_check_virtual() {")
    );

    // 実際に走らせる: 蓋を開けた MacBook + 外部モニタ + tako-vd
    let scratch = VdScratch::new("read");
    let table = "Color LCD\t0\t0\t1512\t982\t1\t1\t1\n\
                 External 4K\t1512\t0\t3840\t2160\t7\t0\t0\n\
                 tako-vd\t5352\t0\t2560\t1440\t13\t0\t0\n";
    let list = "17\t13\ttako-vd\n";
    for name in ["Color LCD", "External 4K"] {
        let at = line_of(&src, "vd_bounds() {");
        let r = run_vd_stubbed(&scratch, name, "bounds", table, list);
        assert!(
            r.rc == rc_nv && r.stdout.trim().is_empty(),
            "{REL}:{at}: TAKO_VD_NAME={name}（物理画面）で bounds が座標を返した / VD_RC_NOT_VIRTUAL（{rc_nv}）で断らない（#1817）\n\
             rc={} stdout={:?} stderr={:?}",
            r.rc, r.stdout, r.stderr
        );
        assert!(
            r.stderr.contains(&format!("{name} は仮想ディスプレイではない")),
            "{REL}:{at}: TAKO_VD_NAME={name} の bounds が「仮想ディスプレイではない」と言わない: {:?}",
            r.stderr
        );

        let at = line_of(&src, "vd_status() {");
        let r = run_vd_stubbed(&scratch, name, "status", table, list);
        let first = r.stdout.lines().next().unwrap_or_default().to_string();
        assert!(
            r.rc == rc_nv
                && first.starts_with(&format!("{name}: 仮想ディスプレイではない"))
                && !r.stdout.contains("使用可")
                && !r.stdout.contains("要（内蔵"),
            "{REL}:{at}: TAKO_VD_NAME={name}（物理画面）の status が「仮想ディスプレイではない」で \
             VD_RC_NOT_VIRTUAL（{rc_nv}）にならない / 使用可・Main 保護の「要」を言う（#1817）\nrc={} stdout={:?}",
            r.rc, r.stdout
        );

        let at = line_of(&src, "vd_move_window() {");
        let r = run_vd_stubbed(&scratch, name, "move-window", table, list);
        assert!(
            r.rc == rc_nv && !r.log.contains("osascript"),
            "{REL}:{at}: TAKO_VD_NAME={name}（物理画面）で move-window が窓を動かした / 断らない（#1817）\nrc={} log={:?}",
            r.rc, r.log
        );
    }
    // 器を起こさない（内蔵は器に聞かずに決まり、外部モニタは起こさない口で聞く）
    for sub in ["bounds", "status", "move-window"] {
        let r = run_vd_stubbed(&scratch, "External 4K", sub, table, list);
        assert!(
            !r.log.lines().any(|l| l == "ready"),
            "{REL}:{}: {sub} が器を起こした（vd_backend_ready。#1817。読むだけの口は BetterDisplay を起動しない）\n{}",
            line_of(&src, "vd_check_virtual() {"),
            r.log
        );
    }

    // 対照: 本物の tako-vd は従来どおり座標・使用可を返し、窓を動かす
    let r = run_vd_stubbed(&scratch, "tako-vd", "bounds", table, list);
    assert_eq!(
        (r.rc, r.stdout.trim()),
        (0, "5352 0 2560 1440"),
        "{REL}:{}: 本物の tako-vd で bounds が座標を返さない（#1817 で既定の道を壊した）: {:?}",
        line_of(&src, "vd_bounds() {"),
        r.stderr
    );
    let r = run_vd_stubbed(&scratch, "tako-vd", "status", table, list);
    assert!(
        r.rc == 0 && r.stdout.starts_with("tako-vd: 使用可（5352 0 2560 1440）"),
        "{REL}:{}: 本物の tako-vd で status が使用可を返さない（#1817 で既定の道を壊した）: {:?}",
        line_of(&src, "vd_status() {"),
        r.stdout
    );
    let r = run_vd_stubbed(&scratch, "tako-vd", "move-window", table, list);
    assert!(
        r.rc == 0 && r.log.contains("{5392, 60}"),
        "{REL}:{}: 本物の tako-vd で move-window が窓を内側（+40,+60）へ動かさない（#1817 で既定の道を壊した）: {:?}",
        line_of(&src, "vd_move_window() {"),
        r.log
    );
}
