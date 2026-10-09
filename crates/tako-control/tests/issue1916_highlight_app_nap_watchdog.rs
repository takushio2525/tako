//! **#1916 の番犬**: 構文の塗り（background）が、App Nap に間引かれたまま走る形へ戻らないようにする。
//!
//! ## 何が起きていたのか（実測・修正前・release・Apple M5 Max）
//!
//! 「GUI 内の塗りはテストスレッドの 4〜8 倍遅い」は 2 つの重なりだった。
//!
//! 1. **比べた入力が違っていた**: テストスレッド側（`perf_大きいファイルの編集計測`）は main.rs 由来の
//!    行で 3.3 秒・64 G 命令、GUI 側（visual-test 節 `large-file-decor`）は `let value_N = N; // ~~~` の
//!    行で 11.9 秒・243.6 G 命令。**同じ入力ならテストスレッドも 11.9 秒・243.6 G 命令**で、
//!    間引かれる前の GUI（12.0 秒・243.7 G 命令・P コア 99.7%）と同じだった
//! 2. **App Nap**: 隔離 GUI は前面に出ないので、起動から約 30 秒でプロセスの優先度が 46 → 4 に
//!    落ち、塗りは実行器（GCD の既定キュー・専用スレッド）や QoS（0x15 / 0x19）に関わらず E コアへ
//!    寄せられた（26.9〜33.4 秒・P コア 0〜9%・塗り 1 本の命令数は同じ約 244 G）。「回を追うごとに
//!    伸びる」は時間が経って間引かれたから。起動時の `sleep_guard::disable_app_nap`（#173）は
//!    `defaults write /proc/<pid>/Info …` で、macOS には `/proc` が無いので一度も効いていない
//!
//! 修正: 塗りの background の closure が `platform::user_work::UserWork` を握る（握っている間は
//! `NSProcessInfo beginActivityWithOptions:reason:` の UserInitiated を保つ）。間引かれた後でも
//! 12.1〜12.2 秒・P コア 99% に戻る。
//!
//! ## ここで止める 3 つ
//!
//! 1. [`塗りのbackgroundはuser_workを握って回す`] — 読み取り表示の塗り / 編集開始の全文の塗りの
//!    closure が `UserWork` を握らない・塗りの後で握る・`let _ =` で握った直後に落とす形
//! 2. [`user_workはactivityを始めて最後の1つで終える`] — 実装が `beginActivityWithOptions:reason:` /
//!    `endActivity:` を呼ばない・依頼の種類（UserInitiated、アイドルスリープは妨げない）が変わる・
//!    落としても終えない・A/B の腕の名前がずれる形
//! 3. [`実経路のabの腕はコードと同じ名前で戻す`] — 実経路テストの A/B の腕と判定がコードとずれる形
//!
//! 落ちるときは **file:line で名指し**する。実 GUI での A/B は
//! `scripts/test-highlight-app-nap-1916.sh`（`TAKO_1916_LEGACY=1` で③が名指しで FAILED）。

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";
const USER_WORK: &str = "crates/tako-app/src/platform/user_work.rs";
const SCRIPT: &str = "scripts/test-highlight-app-nap-1916.sh";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// 本番コードのコメントを潰した眺め（バイト位置は元のファイルと同じ）
fn production_view(rel: &str) -> String {
    let src = read(rel);
    let prod = production_range::production(&src, rel);
    let code = code_view(&prod);
    let view = without_comments_checked(&prod, rel);
    assert_eq!(
        code.len(),
        view.len(),
        "{rel}:1 2 つの眺めのバイト長が食い違う（範囲を使い回せない）"
    );
    view
}

/// `at`（バイト位置）が何行目か
fn line_at(text: &str, at: usize) -> usize {
    text[..at].bytes().filter(|b| *b == b'\n').count() + 1
}

/// 関数 1 本の本体（開始のバイト位置と本文）。名前で最初に見つかったもの
fn body(view: &str, rel: &str, name: &str) -> (usize, String) {
    let mut offset = 0;
    for line in view.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if fn_head_name(line.trim_start()) != Some(name) {
            continue;
        }
        let open = view[start..].find('{').expect("関数の本体") + start;
        let mut depth = 0usize;
        for (i, byte) in view[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return (open, view[open..open + i + 1].to_string());
                    }
                }
                _ => {}
            }
        }
    }
    panic!("{rel}:1 `fn {name}` が見つからない（改名したならこの番犬も直す）");
}

// --- 1) 塗りの background は UserWork を握る ---------------------------------------

/// 読み取り表示の塗り（`spawn_highlight`）と編集開始の全文の塗り（`spawn_editor_seed`）は、
/// background の closure の中で**塗る前に** `UserWork` を名前のある束縛で握る。
/// 握らない / 塗った後で握る / `let _ =`（その場で落ちる）は、App Nap に間引かれた GUI で
/// 塗りが E コアへ寄せられる（10 MB で 12.0 秒 → 26.9〜33.4 秒）
#[test]
fn 塗りのbackgroundはuser_workを握って回す() {
    let view = production_view(SIDEBAR);
    for (name, work) in [
        ("spawn_highlight", "preview::highlight_text("),
        ("spawn_editor_seed", "preview::seed_editor_highlight("),
    ] {
        let (at, f) = body(&view, SIDEBAR, name);
        let Some(call) = f.find(work) else {
            panic!(
                "{SIDEBAR}:{} `fn {name}` が `{work}` を呼んでいない（形を変えたならこの番犬も直す）",
                line_at(&view, at)
            );
        };
        let call_line = line_at(&view, at + call);
        // 塗りを包む background の closure の頭（塗りより前で最後に現れる spawn）
        let Some(spawn) = f[..call].rfind(".spawn(async move {") else {
            panic!(
                "{SIDEBAR}:{call_line} `fn {name}` の `{work}` が background の closure \
                 （`.spawn(async move {{`）の中に無い（#1916。形を変えたならこの番犬も直す）"
            );
        };
        let closure = &f[spawn..call];
        let held = closure.find("UserWork::begin()").is_some_and(|begin| {
            let head = closure[..begin].rsplit('\n').next().unwrap_or("");
            head.contains("let _") && !head.contains("let _ =")
        });
        assert!(
            held,
            "{SIDEBAR}:{call_line} `fn {name}` の `{work}` が `UserWork` を握らずに塗る（#1916）。\
             塗る前に `let _work = UserWork::begin();` で握る（`let _ =` はその場で落ちる）。\
             握らないと App Nap に間引かれた GUI で塗りが E コアへ寄せられ、10 MB で \
             12.0 秒 → 26.9〜33.4 秒になる"
        );
    }
}

// --- 2) UserWork の実装 -----------------------------------------------------------

/// 実装の要: macOS は `NSProcessInfo` の activity を UserInitiated（アイドルスリープは妨げない）で
/// 始め、最後の 1 つを落としたときに終える。A/B の腕は `TAKO_1916_LEGACY` だけ
#[test]
fn user_workはactivityを始めて最後の1つで終える() {
    let view = production_view(USER_WORK);
    for (needle, why) in [
        (
            "c\"beginActivityWithOptions:reason:\"",
            "App Nap を止める口は NSProcessInfo の activity（Info.plist の NSAppSleepDisabled は \
             バンドルの外の起動に効かず、`defaults write /proc/…` は macOS では書けない）",
        ),
        (
            "c\"endActivity:\"",
            "始めた activity を終えないと、塗りが終わってもアプリが間引かれなくなる（電力）",
        ),
    ] {
        assert!(
            view.contains(needle),
            "{USER_WORK}:1 `{needle}` が無い（#1916）。{why}"
        );
    }
    // 依頼の種類: NSActivityUserInitiatedAllowingIdleSystemSleep（利用者が待っている処理。
    // アイドルスリープは妨げない）
    const OPTIONS_HEAD: &str = "const OPTIONS: u64 =";
    let Some(options_at) = view.find(OPTIONS_HEAD) else {
        panic!("{USER_WORK}:1 `{OPTIONS_HEAD}` が無い（#1916。形を変えたならこの番犬も直す）");
    };
    let options = view[options_at + OPTIONS_HEAD.len()..]
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    assert_eq!(
        options,
        "0x00FF_FFFF & !(1 << 20)",
        "{USER_WORK}:{} 依頼の種類が NSActivityUserInitiatedAllowingIdleSystemSleep \
         （= UserInitiated からアイドルスリープの禁止を外したもの）でない（#1916）",
        line_at(&view, options_at)
    );
    // A/B の腕（実経路テストの旧挙動）の名前
    let (legacy_at, legacy) = body(&view, USER_WORK, "legacy");
    assert!(
        legacy.contains("\"TAKO_1916_LEGACY\""),
        "{USER_WORK}:{} 旧挙動へ戻す腕が `TAKO_1916_LEGACY` を読んでいない（#1916。\
         実経路テストの A/B が効かなくなる）",
        line_at(&view, legacy_at)
    );
    // 始める / 終える番は数で決める（重なった処理で依頼を二重に始めない・先に終えない）
    let (begin_at, begin) = body(&view, USER_WORK, "begin");
    for needle in ["legacy()", "holders.acquire()", "sys::begin()"] {
        assert!(
            begin.contains(needle),
            "{USER_WORK}:{} `UserWork::begin` が `{needle}` を通っていない（#1916）",
            line_at(&view, begin_at)
        );
    }
    let (drop_at, drop) = body(&view, USER_WORK, "drop");
    for needle in ["holders.release()", "sys::end("] {
        assert!(
            drop.contains(needle),
            "{USER_WORK}:{} `UserWork` の `drop` が `{needle}` を通っていない（#1916）。\
             落としても activity を終えないと、塗りが終わってもアプリが間引かれなくなる",
            line_at(&view, drop_at)
        );
    }
}

// --- 3) 実経路テストの A/B --------------------------------------------------------

/// 実経路テストは同じ名前の腕で旧挙動へ戻し、「P コアで走ったか」で判定する
/// （所要の実時間は判定に使わない = `.agent/conventions.md`）。しきい値は 1 か所の値で、
/// ③（間引かれた後の 1 本）と④（重なった 2 本）の判定が両方それを通る
#[test]
fn 実経路のabの腕はコードと同じ名前で戻す() {
    let script = read(SCRIPT);
    let line_of = |needle: &str| script.find(needle).map_or(1, |at| line_at(&script, at));
    for (needle, why) in [
        (
            "TAKO_1916_LEGACY=$TAKO_1916_LEGACY",
            "A/B の腕を GUI へ渡す（コードの読む名前と同じ）",
        ),
        ("wait_napped", "間引かれたのを状態で待ってから測る"),
        ("hw.nperflevels", "E コアの無い機は未実測で終える"),
    ] {
        assert!(
            script.contains(needle),
            "{SCRIPT}:{} `{needle}` が無い（#1916）。{why}",
            line_of(needle)
        );
    }
    // 「P コアで走った」の下限（実測は修正後 0.986〜1.000・旧挙動 0.000〜0.089）
    assert!(
        script.contains("P_CORE_MIN=0.5\n"),
        "{SCRIPT}:{} P コアの比率の下限が 0.5 でない（#1916。修正後 0.986〜1.000・旧挙動で \
         間引かれた後 0.000〜0.089 の間で判定する）",
        line_of("P_CORE_MIN=")
    );
    let judged = script.matches("on_p_cores \"$").count();
    assert!(
        judged >= 2,
        "{SCRIPT}:{} P コアの比率で判定している箇所が {judged} か所しか無い（#1916。③ の 1 本と \
         ④ の重なった 2 本の両方を `on_p_cores` で判定する）",
        line_of("on_p_cores() {")
    );
    let helper = script
        .find("on_p_cores() {")
        .map(|at| script[at..].lines().next().unwrap_or(""))
        .unwrap_or("");
    assert!(
        helper.contains(">= $P_CORE_MIN"),
        "{SCRIPT}:{} `on_p_cores` が下限 `P_CORE_MIN` と比べていない（#1916）",
        line_of("on_p_cores() {")
    );
}
