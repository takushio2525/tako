//! 仮想ディスプレイまわりの番犬（#1141 / #1150）
//!
//! 守りたい不変条件は 8 つ。どれも「壊れても動いているように見える」ので、
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

use std::path::{Path, PathBuf};

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
    // シェル側の既定値（TAKO_VD_NAME の :- 右辺）
    let shell_default = src
        .lines()
        .find_map(|l| l.trim().strip_prefix("VD_NAME=${TAKO_VD_NAME:-"))
        .and_then(|rest| rest.strip_suffix('}'))
        .map(str::to_string);
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
