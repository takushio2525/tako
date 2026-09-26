//! **#1490 の番犬**: 隔離 GUI の起動を `scripts/lib/isolated-gui.sh` の 1 実装に保つ。
//!
//! ## なぜ止めるのか
//!
//! 蓋閉じ運用の `tako-vd` は**アイドルで眠り、眠った面は CoreGraphics の active 一覧から
//! 落ちる**。その状態の検証用 GUI はユーザーの画面へ窓を出さずに終了コード 4 で終わる
//! （#1160 の設計。これは正しい）ので、`virtual-display.sh ensure` を**起動の直前に毎回**
//! 通していない検証スクリプトは「GUI が立たない」で止まる。#1487 の worker は同じ検証に
//! 3 回失敗し、直したのはその 1 本だけだった。
//!
//! 実測（2026-09-21・差し替え前の `scripts/test-*.sh` 12 本）:
//!
//! | ensure の呼び方 | 本数 |
//! |---|---|
//! | 起動の直前に毎回 | 4（1485 / 1487 / 1466 / user-task-delivery） |
//! | 冒頭で 1 回だけ（起動まで数十行ある） | 6 |
//! | 呼んでいない | 2（master-launch / worker-min-width） |
//!
//! 起動の手順が 14 か所へ手書きで散っているのが原因なので、**散らせないこと**を
//! ここで拘束する。`ensure` は冪等（常設の面を作り直さない）なので毎回通して困らない。
//!
//! ## 何を固定するか
//!
//! 1. [`ヘルパが起動の直前に面を起こす`] — `launch_isolated_gui` が起動行より前に ensure を通す
//! 2. [`ヘルパは名前一致でプロセスを落とさない`] — `pkill -f tako` は本番 GUI に当たる
//! 3. [`テストスクリプトは隔離起動を直書きしない`] — バイナリのパス / 面の名前の直書き
//! 4. [`テストスクリプトはヘルパ経由で起動する`] — 手書きの背景起動と ensure を残さない
//! 5. [`検査は実際の直書きを検出する`] — 検査そのものの検出力
//! 6. [`ヘルパは面の指定を常に明示する`] — 明示の `TAKO_DISPLAY` を見失った検証用 GUI は
//!    窓を開かずに終わる（#1697）。渡さない起動は tako の暗黙の既定（見えている面へ落ちる）に
//!    乗るので、ヘルパからは作らない（#1744）
//!
//! ## #1744: 面を用意できなければ起動しない
//!
//! 以前のヘルパは `ensure` が失敗しても「起動は続ける」で素通しし（CI・他人の機でも検証を
//! 回すため）、uuid の記録も無い機では面の指定を持たない起動が tako の暗黙の既定に乗って
//! **ユーザーの画面へ窓が出うる**潜在経路があった（この道で窓が出た実例は確認されていない）。
//! 蓋閉じ + ディスプレイスリープでは `ensure` が面を起こせないのが日常なので、
//! 「開かずに終了コード 4 で返す → 呼び出し側が未実測と明記する」を固定する。
//!
//! 7. [`ヘルパは面を用意できなければ起動しない`] — 起動行より前に、`ensure` の失敗で
//!    `return "$ISOLATED_GUI_RC_NO_DISPLAY"` する分岐が在る（file:line で名指す）
//! 8. [`面を用意できないとヘルパは起動せずに理由を1行出す`] — 実際に `/bin/bash` で
//!    ヘルパを走らせ、「用意できない面」を注入して GUI（偽のバイナリ）が起きないことを見る
//! 9. [`テストスクリプトは面を用意できない失敗を拾う`] — 呼び手の起動行が失敗を捨てない
//! 10. [`失敗の拾い方の検査は実際の取りこぼしを検出する`] — 9 の検出力
//!
//! ## #1760: tako-vd 以外の面の明示を通さない
//!
//! #1744 のあとも、面を用意できたうえで呼び出し側が `TAKO_DISPLAY=0`（= メイン画面）のように
//! 別の面を明示すると、その指定がそのまま GUI へ渡っていた。ヘルパは起動の前に値を判定し
//! （`iso_display_allowed`）、通さない値なら窓を開かずに終了コード 2 で返す。通すのは
//! tako-vd の名前 / `ensure` が記録した tako-vd の uuid / 空（tako の定義で未指定）/
//! 実在し得ない index（`index:999` 等）だけ。
//!
//! 11. [`ヘルパはtako_vd以外の面の明示を起動の前に断る`] — 判定と断る行が起動行より前に在り、
//!     GUI へ渡すのは判定した値（呼び出し側の `VAR=VAL` より後ろ）。差し替え口も戻さない
//! 12. [`tako_vd以外の面の明示ではヘルパは起動せずに理由を1行出す`] — 実際に `/bin/bash` で
//!     走らせ、別の面の名前・index・uuid で偽の GUI が起きないこと、tako-vd の名前・uuid・
//!     未指定・空・実在し得ない index では従来どおり起きることを見る
//! 13. [`実在し得ないindexはtakoでも見失いになり窓を開かない`] — ヘルパが通す
//!     「当たらない面」（#1697 の ④ が使う）が tako の当て方でも本当に当たらず、
//!     明示の見失い（窓を開かずに終わる = #1697 の道）になる

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const HELPER_REL: &str = "scripts/lib/isolated-gui.sh";
const VD_REL: &str = "scripts/lib/virtual-display.sh";

/// 落ちたときに出す正しい書き方
const RECIPE: &str = concat!(
    "\n    . \"$REPO_ROOT/scripts/lib/isolated-gui.sh\"",
    "\n    isolated_gui_bins || exit 1",
    "\n    launch_isolated_gui \"$TMP/app.log\" || exit $?   # 面を用意できなければ 4（#1744）",
    "\n    APP_PID=\"$ISOLATED_GUI_PID\"",
    "\n    wait_isolated_gui \"$TMP/app.log\" || exit 1",
);

/// 走査から外すもの。**増やすときは理由を書く**（ここが緩むと番犬が形だけになる）
const EXEMPT: &[(&str, &str)] = &[(
    "test-virtual-display-guard.sh",
    "virtual-display.sh 自身のモックテスト（画面・器をすべてスタブして ensure の中身を検査する）。\
     GUI は 1 つも立てない",
)];

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

/// `scripts/test-*.sh` を集める（除外リストは外してから返す）
fn test_scripts() -> Vec<PathBuf> {
    let dir = repo_root().join("scripts");
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("scripts/ を読む")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                return false;
            };
            name.starts_with("test-")
                && name.ends_with(".sh")
                && !EXEMPT.iter().any(|(ex, _)| *ex == name)
        })
        .collect();
    out.sort();
    // 走査範囲が空だとどんな回帰でも通る
    assert!(
        out.len() >= 10,
        "scripts/test-*.sh が {} 本しか見つからない（走査範囲の取り方が壊れている）",
        out.len()
    );
    out
}

/// 行コメントを外した眺め（規約の説明文そのものが引っかかるのを避ける）
fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// `rel:line` で名指すための相対パス
fn rel_of(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// 関数 1 本の本体を `(行番号, 行)` で返す（署名の行から、字下げなしの `}` まで）。
/// **見つからないことも FAILED**（走査範囲が空だとどんな回帰でも通る）
fn fn_body(rel: &str, src: &str, signature: &str) -> (Vec<(usize, String)>, usize) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.starts_with(signature))
        .unwrap_or_else(|| panic!("{rel}: 目印 {signature:?} が消えている（走査範囲を作れない）"));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == "}")
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

fn find(body: &[(usize, String)], needle: &str) -> Option<usize> {
    body.iter()
        .find(|(_, l)| l.contains(needle))
        .map(|(n, _)| *n)
}

// --------------------------------------------- 1. 起動の直前に起こす

#[test]
fn ヘルパが起動の直前に面を起こす() {
    let src = read(HELPER_REL);
    let (body, at) = fn_body(HELPER_REL, &src, "launch_isolated_gui()");

    let ensure = find(&body, "iso_ensure_display").unwrap_or_else(|| {
        panic!(
            "{HELPER_REL}:{at}: launch_isolated_gui が面を起こしていない \
             = 眠った tako-vd では GUI が立たない（#1490 の症状そのもの）"
        )
    });
    let launch = find(&body, "$APP_BIN").unwrap_or_else(|| {
        panic!("{HELPER_REL}:{at}: launch_isolated_gui が GUI を起こす行が見つからない")
    });
    assert!(
        ensure < launch,
        "{HELPER_REL}:{ensure}: 面を起こすのが起動（{HELPER_REL}:{launch}）より後ろに在る \
         = 眠ったまま起動して窓が開かない（#1490 / #1160）"
    );

    // 起こす実体は virtual-display.sh の ensure（冪等・常設の面を作り直さない）
    let (ens_body, ens_at) = fn_body(HELPER_REL, &src, "iso_ensure_display()");
    let ens = ens_body
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        ens.contains("virtual-display.sh") && ens.contains("ensure"),
        "{HELPER_REL}:{ens_at}: iso_ensure_display が `virtual-display.sh ensure` を通っていない \
         = 眠りを起こす 1 実装（#1160 の vd_ensure_drawable）から外れる"
    );
    // 用意できないときに素通しする形（#1744 前）へ戻っていない。
    // 素通しした起動は面の指定を持たず、tako の暗黙の既定でユーザーの画面へ落ちうる
    if let Some((n, l)) = ens_body.iter().find(|(_, l)| l.contains("|| true")) {
        panic!(
            "{HELPER_REL}:{n}: 面を用意できない失敗を握りつぶしている（#1744）\n    {}",
            l.trim()
        );
    }

    // pid は変数へ置く（`$( )` で受けると副シェルの子になり呼び出し側の wait が壊れる）
    assert!(
        find(&body, "ISOLATED_GUI_PID=$!").is_some(),
        "{HELPER_REL}:{at}: 起こした pid を ISOLATED_GUI_PID へ置いていない（#1490）"
    );
}

/// 面の指定は**常に明示する**（#1697 / #1744）。
///
/// tako は明示の `TAKO_DISPLAY` を見失うと窓を開かずに終わる（名前だけが読めない瞬間に
/// ユーザーの画面へ落ちないため）。#1697 のヘルパは「配線済みの機」でだけ明示し、未配線と
/// 読んだ機では渡さなかったが、その起動は tako の暗黙の既定（見えている面へ落ちる）に乗る。
/// #1744 で「面を用意できなければ起動しない」にしたので、起動に届いた時点で面は在る =
/// 無条件に明示してよい
#[test]
fn ヘルパは面の指定を常に明示する() {
    let src = read(HELPER_REL);
    let (body, at) = fn_body(HELPER_REL, &src, "launch_isolated_gui()");
    // 呼び出し側の指定が無ければ tako-vd（ISOLATED_GUI_DISPLAY）を明示する
    let explicit = find(
        &body,
        "local display=\"${TAKO_DISPLAY:-$ISOLATED_GUI_DISPLAY}\"",
    )
    .unwrap_or_else(|| {
        panic!(
            "{HELPER_REL}:{at}: launch_isolated_gui が面の指定を明示していない（#1697 / #1744）\n\
                 → 見失ったときユーザーの画面へ落ちる暗黙の既定に倒れる"
        )
    });
    let launch = find(&body, "\"$APP_BIN\"").unwrap_or_else(|| {
        panic!("{HELPER_REL}:{at}: launch_isolated_gui が GUI を起こす行が見つからない")
    });
    assert!(
        explicit < launch,
        "{HELPER_REL}:{explicit}: 面の指定が起動（{HELPER_REL}:{launch}）より後ろに在る（#1697）"
    );
    let launch_line = &body.iter().find(|(n, _)| *n == launch).expect("起動行").1;
    assert!(
        launch_line.contains("\"TAKO_DISPLAY=${display}\""),
        "{HELPER_REL}:{launch}: GUI を起こす行が面の指定を渡していない（#1697 / #1744）\n    {}",
        launch_line.trim()
    );
    // 「配線済みのときだけ明示」（#1697 の形）へ戻ると、未配線と読んだ機で暗黙の既定に乗る
    for needle in ["iso_display_recorded", "wired"] {
        if let Some(n) = find(&body, needle) {
            panic!(
                "{HELPER_REL}:{n}: 面の指定を条件つきで明示している（#1744 前の形: {needle}）。\n\
                 → 渡さない起動は tako の暗黙の既定でユーザーの画面へ落ちる"
            );
        }
    }
}

// --------------------------------------------- 2. 名前一致で殺さない

#[test]
fn ヘルパは名前一致でプロセスを落とさない() {
    let src = read(HELPER_REL);
    let strays: Vec<String> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l))
        .filter(|(_, l)| l.contains("pkill") || l.contains("killall"))
        .map(|(i, l)| format!("{HELPER_REL}:{}: {}", i + 1, l.trim()))
        .collect();
    assert!(
        strays.is_empty(),
        "隔離 GUI のヘルパが名前一致でプロセスを落としている \
         = 本番 /Applications/tako.app と他 worker の隔離インスタンスにも当たる \
         （実際にユーザーの GUI を落とした事故がある）:\n{}",
        strays.join("\n")
    );

    let (body, at) = fn_body(HELPER_REL, &src, "stop_isolated_gui()");
    let text = body
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("kill \"$pid\""),
        "{HELPER_REL}:{at}: stop_isolated_gui が明示 pid を落としていない（#1490）"
    );
    assert!(
        text.contains("kill -0 \"$pid\""),
        "{HELPER_REL}:{at}: 素直に終わるのを待たずに次へ進んでいる \
         = 落とし切れていない GUI が次の起動と同居する（#1490）"
    );
}

// --------------------------------------------- 3. 直書きの禁止

/// 隔離起動を手書きする形。`(見つけたら落とす文字列, 代わりに使うもの)`
const DIRECT: &[(&str, &str)] = &[
    (
        "target/debug/tako-app",
        "isolated_gui_bins（ISOLATED_GUI_APP_REL の 1 か所に閉じる）",
    ),
    (
        "target/release/tako-app",
        "isolated_gui_bins（ISOLATED_GUI_APP_REL の 1 か所に閉じる）",
    ),
    (
        "cargo run -p tako-app",
        "isolated_gui_bins + launch_isolated_gui",
    ),
];

/// 面の名前を手で置く形。`TAKO_DISPLAY=tako-vd` と
/// `export TAKO_DISPLAY="${TAKO_DISPLAY:-tako-vd}"` の**どちらも**拾う
/// （差し替え前に実在したのは後者だけだった）
fn names_display(line: &str) -> bool {
    line.contains("TAKO_DISPLAY") && line.contains("tako-vd")
}

/// その行が隔離起動の直書きなら `(見つけた文字列, 代わりに使うもの)`
fn direct_launch(line: &str) -> Option<(&'static str, &'static str)> {
    if is_comment(line) {
        return None;
    }
    if let Some((n, alt)) = DIRECT.iter().find(|(needle, _)| line.contains(*needle)) {
        return Some((*n, *alt));
    }
    if names_display(line) {
        return Some((
            "TAKO_DISPLAY / tako-vd",
            "launch_isolated_gui（ISOLATED_GUI_DISPLAY の既定）",
        ));
    }
    None
}

#[test]
fn テストスクリプトは隔離起動を直書きしない() {
    let mut violations: Vec<String> = Vec::new();
    for path in test_scripts() {
        let src = std::fs::read_to_string(&path).expect("読める .sh");
        for (i, line) in src.lines().enumerate() {
            if let Some((needle, alt)) = direct_launch(line) {
                violations.push(format!(
                    "{}:{}: `{needle}` の直書き（代わりに {alt} を通す）\n    {}",
                    rel_of(&path),
                    i + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "隔離 GUI の起動が `{HELPER_REL}` の外へ手書きで散っている。\n\
         散ると「起動の直前に ensure」を足す場所が増え、蓋閉じで tako-vd が眠ったときに\n\
         その経路だけ GUI が立たない（#1490 / #1160）。ヘルパを source すること:\n\
         {RECIPE}\n\n{}",
        violations.join("\n")
    );
}

// --------------------------------------------- 4. ヘルパ経由で起動する

#[test]
fn テストスクリプトはヘルパ経由で起動する() {
    let mut violations: Vec<String> = Vec::new();
    for path in test_scripts() {
        let src = std::fs::read_to_string(&path).expect("読める .sh");
        let sources_helper = src.contains("lib/isolated-gui.sh");
        for (i, line) in src.lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            let trimmed = line.trim_end();
            // 手書きの背景起動（`"$APP_BIN" > … &`）。起動の手順がここへ戻ると
            // ensure を足す場所が増える
            if trimmed.ends_with('&')
                && !trimmed.ends_with("&&")
                && (line.contains("$APP_BIN") || line.contains("${APP_BIN"))
            {
                violations.push(format!(
                    "{}:{}: GUI を手書きで背景起動している（launch_isolated_gui を通す）\n    {}",
                    rel_of(&path),
                    i + 1,
                    line.trim()
                ));
            }
            // 手書きの ensure。ヘルパが**起動の直前に**通すので呼び直す必要は無く、
            // 残っていると「冒頭で 1 回だけ」の形（#1490 の症状）が戻る
            if line.contains("virtual-display.sh") && line.contains("ensure") {
                violations.push(format!(
                    "{}:{}: `virtual-display.sh ensure` を手書きで呼んでいる\
                     （launch_isolated_gui が起動の直前に通す）\n    {}",
                    rel_of(&path),
                    i + 1,
                    line.trim()
                ));
            }
        }
        // ヘルパを source したなら実際に通す（source だけして手書きに戻る形を止める）
        if sources_helper && !src.contains("launch_isolated_gui") {
            violations.push(format!(
                "{}: {HELPER_REL} を source しているのに launch_isolated_gui を呼んでいない",
                rel_of(&path)
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "隔離 GUI の起動手順がヘルパの外に残っている（#1490）:\n{}",
        violations.join("\n")
    );
}

// --------------------------------------------- 5. 検査の検出力

#[test]
fn 検査は実際の直書きを検出する() {
    // 差し替え前に実在した 14 行の形（そのまま戻ってきたら落ちること）
    for line in [
        r#"APP_BIN="${APP_BIN:-$REPO_ROOT/target/debug/tako-app}""#,
        r#"APP_SRC="$REPO_ROOT/target/debug/tako-app""#,
        r#"export TAKO_DISPLAY="${TAKO_DISPLAY:-tako-vd}""#,
        r#"  TAKO_SELF_TEST=1 TAKO_ISOLATED=1 cargo run -p tako-app"#,
    ] {
        assert!(
            direct_launch(line).is_some(),
            "検査が直書きを見逃している: {line}"
        );
    }
    // 行コメントと、ヘルパ経由の書き方は引っかからない
    for line in [
        r#"# 窓は常設の仮想ディスプレイ（tako-vd）へ出る（#1141）"#,
        r#"  # `TAKO_DISPLAY=tako-vd` は launch_isolated_gui が置く"#,
        r#"isolated_gui_bins"#,
        r#"launch_isolated_gui "$TMP/app.log" TAKO_1487_LEGACY=1"#,
        r#"cp "$APP_BIN" "$BIN_DIR/tako-app""#,
    ] {
        assert!(
            direct_launch(line).is_none(),
            "検査が正しい書き方を誤検出している: {line}"
        );
    }

    // ヘルパ自身と virtual-display.sh は走査対象に入らない（入れると自分で落ちる）
    let scanned = test_scripts();
    for rel in [HELPER_REL, VD_REL] {
        assert!(
            !scanned.iter().any(|p| rel_of(p) == rel),
            "{rel} が走査対象に入っている（ヘルパ自身は直書きの置き場）"
        );
    }
    // 除外リストは実在するファイルだけを指す（消えたファイルの除外が残ると穴になる）
    for (name, why) in EXEMPT {
        assert!(
            repo_root().join("scripts").join(name).exists(),
            "除外リストの {name} が存在しない（除外の理由: {why}）"
        );
    }
}

// --------------------------------------------- 7. 面を用意できなければ起動しない（#1744）

/// `launch_isolated_gui` の中の `(ensure の行, 起動を断る行, GUI を起こす行)`
fn refusal_lines(src: &str) -> (usize, Option<usize>, usize) {
    let (body, at) = fn_body(HELPER_REL, src, "launch_isolated_gui()");
    let ensure = find(&body, "iso_ensure_display").unwrap_or_else(|| {
        panic!("{HELPER_REL}:{at}: launch_isolated_gui が面を起こしていない（#1490）")
    });
    let launch = find(&body, "\"$APP_BIN\"").unwrap_or_else(|| {
        panic!("{HELPER_REL}:{at}: launch_isolated_gui が GUI を起こす行が見つからない")
    });
    let refuse = body
        .iter()
        .find(|(n, l)| *n > ensure && l.contains("return \"$ISOLATED_GUI_RC_NO_DISPLAY\""))
        .map(|(n, _)| *n);
    (ensure, refuse, launch)
}

#[test]
fn ヘルパは面を用意できなければ起動しない() {
    let src = read(HELPER_REL);
    let (ensure, refuse, launch) = refusal_lines(&src);
    let lines: Vec<&str> = src.lines().collect();
    let ensure_line = lines[ensure - 1].trim();
    assert!(
        ensure_line.starts_with("if ! iso_ensure_display"),
        "{HELPER_REL}:{ensure}: 面を用意できたかを起動の条件にしていない（#1744）\n    {ensure_line}\n\
         → `if ! iso_ensure_display; then …; return \"$ISOLATED_GUI_RC_NO_DISPLAY\"; fi` の形にする"
    );
    let refuse = refuse.unwrap_or_else(|| {
        panic!(
            "{HELPER_REL}:{ensure}: 面を用意できないときに起動を断っていない（#1744）。\n\
             → 既定の面（= ユーザーの画面）で続行する道になる。\
             `return \"$ISOLATED_GUI_RC_NO_DISPLAY\"` で返す"
        )
    });
    assert!(
        refuse < launch,
        "{HELPER_REL}:{refuse}: 起動を断る行が GUI を起こす行（{HELPER_REL}:{launch}）より後ろに在る（#1744）"
    );
    // 終了コードは tako 本体の「窓を開かずに終わる」と同じ 4（呼び出し側が未実測と読む番号）
    assert!(
        src.lines()
            .any(|l| l.trim() == "ISOLATED_GUI_RC_NO_DISPLAY=4"),
        "{HELPER_REL}: ISOLATED_GUI_RC_NO_DISPLAY が 4 ではない \
         （tako 本体の REFUSED_EXIT_CODE = 4 とそろえる。#1744）"
    );
}

// --------------------------------------------- 8. 実際に走らせて起動しないことを見る（#1744）

/// 一時 dir（抜けるときに消す。**消す前に一時 dir の下であることを確かめる**）
#[cfg(unix)]
struct Scratch(PathBuf);

#[cfg(unix)]
impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tako-1744-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時 dir を作る");
        Scratch(dir)
    }
}

#[cfg(unix)]
impl Drop for Scratch {
    fn drop(&mut self) {
        assert!(
            self.0.starts_with(std::env::temp_dir()),
            "一時 dir の外を消そうとしている: {}",
            self.0.display()
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 面を用意する係の偽物。`STUB_ENSURE_RC` / `STUB_ENSURE_ERR` / `STUB_BOUNDS_RC` /
/// `STUB_RECORDED_UUID`（空なら記録なし）で振る舞いを決める
#[cfg(unix)]
const STUB_VD: &str = r#"#!/bin/bash
case "$1" in
  ensure)
    echo "   仮想スクリーン tako-vd を接続"
    [ -n "${STUB_ENSURE_ERR:-}" ] && echo "ERROR: $STUB_ENSURE_ERR" >&2
    exit "${STUB_ENSURE_RC:-0}" ;;
  bounds)
    [ "${STUB_BOUNDS_RC:-0}" = 0 ] && echo "0 0 1920 1080"
    exit "${STUB_BOUNDS_RC:-0}" ;;
  recorded-uuid)
    [ -n "${STUB_RECORDED_UUID:-}" ] || exit 1
    echo "$STUB_RECORDED_UUID" ;;
  *) exit 9 ;;
esac
"#;

/// 偽の係の下でヘルパが狙う面の名前（`TAKO_VD_NAME`）。**実在しない名前**にしておくのは、
/// ヘルパが差し替え口を無視して本物の virtual-display.sh を呼ぶ形へ後退しても、
/// 実機の面・器・記録に触れずに即失敗させるため
#[cfg(unix)]
const STUB_VD_NAME: &str = "tako-vd-watchdog-1744-absent";

/// GUI の偽物。起きたら受け取った面の指定を印へ書く（窓は出さない）
#[cfg(unix)]
const STUB_APP: &str = r#"#!/bin/bash
echo "TAKO_DISPLAY=${TAKO_DISPLAY-<unset>}" > "$STUB_MARK"
"#;

#[cfg(unix)]
struct Run {
    rc: i32,
    started: Option<String>,
    stderr: String,
}

/// ヘルパを source して `launch_isolated_gui` を 1 回呼ぶ（macOS 同梱の bash 3.2 で走らせる）。
/// `args` は `launch_isolated_gui` へ並べる `VAR=VAL`
#[cfg(unix)]
fn run_helper(scratch: &Scratch, vd: &Path, env: &[(&str, &str)], args: &[&str]) -> Run {
    use std::os::unix::fs::PermissionsExt;
    let dir = &scratch.0;
    let app = dir.join("app");
    std::fs::write(&app, STUB_APP).expect("偽の GUI を書く");
    std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let mark = dir.join("mark");
    let err = dir.join("err");
    let _ = std::fs::remove_file(&mark);
    let helper = repo_root().join(HELPER_REL);
    let script = r#"
. "$HELPER"
APP_BIN="$STUB_APP_BIN"
launch_isolated_gui "$STUB_DIR/app.log" "$@" 2>"$STUB_ERR"
rc=$?
[ -n "$ISOLATED_GUI_PID" ] && wait "$ISOLATED_GUI_PID"
exit "$rc"
"#;
    let mut cmd = std::process::Command::new("/bin/bash");
    cmd.arg("-c")
        .arg(script)
        .arg("run_helper")
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        // ヘルパが差し替え口を無視して本物の virtual-display.sh を呼ぶ形へ後退しても、
        // 実機の面・器・記録に触れずに即失敗するよう閉じる（実在しない面 / 器なし / 一時 dir）
        .env("TAKO_VD_NAME", STUB_VD_NAME)
        .env("TAKO_VD_BETTERDISPLAY_APP", dir.join("NoBetterDisplay.app"))
        .env("TAKO_VD_RECORD_DIR", dir.join("record"))
        .env("HELPER", &helper)
        .env("ISOLATED_GUI_VD", vd)
        .env("STUB_APP_BIN", &app)
        .env("STUB_DIR", dir)
        .env("STUB_MARK", &mark)
        .env("STUB_ERR", &err);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let status = cmd.status().expect("/bin/bash を起こす");
    Run {
        rc: status.code().unwrap_or(-1),
        started: std::fs::read_to_string(&mark).ok(),
        stderr: std::fs::read_to_string(&err).unwrap_or_default(),
    }
}

#[cfg(unix)]
#[test]
fn 面を用意できないとヘルパは起動せずに理由を1行出す() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("refuse");
    let vd = scratch.0.join("virtual-display.sh");
    std::fs::write(&vd, STUB_VD).expect("偽の係を書く");
    std::fs::set_permissions(&vd, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let missing = scratch.0.join("no-such-virtual-display.sh");

    let src = read(HELPER_REL);
    let (ensure, _, launch) = refusal_lines(&src);

    // 係の振る舞いを決める env（場面ごと）
    type Env<'a> = &'a [(&'a str, &'a str)];
    // (場面, 係, env, stderr の理由に含まれるべき語)
    let cases: &[(&str, &Path, Env, &str)] = &[
        (
            "ensure が非 0（眠った面を起こせない）",
            &vd,
            &[
                ("STUB_ENSURE_RC", "1"),
                (
                    "STUB_ENSURE_ERR",
                    "tako-vd を起こせなかった（描画可能にならない）",
                ),
            ],
            "tako-vd を起こせなかった",
        ),
        (
            "ensure が非 0 で ERROR 行が無い",
            &vd,
            &[("STUB_ENSURE_RC", "3")],
            "終了コード 3",
        ),
        (
            "ensure は 0 だが OS の一覧に居ない",
            &vd,
            &[("STUB_BOUNDS_RC", "1")],
            "OS のディスプレイ一覧に無い",
        ),
        ("面を用意する係が無い", &missing, &[], "見つからない"),
        (
            "呼び出し側が TAKO_DISPLAY を明示していても ensure が非 0",
            &vd,
            &[
                ("TAKO_DISPLAY", "0"),
                ("STUB_ENSURE_RC", "1"),
                (
                    "STUB_ENSURE_ERR",
                    "仮想ディスプレイ tako-vd が OS のディスプレイ一覧に現れない",
                ),
            ],
            "一覧に現れない",
        ),
    ];
    for (name, vd_path, env, why) in cases {
        let r = run_helper(&scratch, vd_path, env, &[]);
        assert!(
            r.started.is_none(),
            "{HELPER_REL}:{launch}: 面を用意できない（{name}）のに GUI を起動した（#1744）。\n\
             面を起こす行は {HELPER_REL}:{ensure}。既定の面（= ユーザーの画面）で続行する道が戻っている\n\
             受け取った指定: {:?}",
            r.started
        );
        assert_eq!(
            r.rc, 4,
            "{HELPER_REL}:{ensure}: 面を用意できない（{name}）ときの終了コードが 4 ではない（#1744）\n\
             stderr: {}",
            r.stderr
        );
        let lines: Vec<&str> = r.stderr.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(
            lines.len(),
            1,
            "{HELPER_REL}:{ensure}: 理由が 1 行ではない（{name}）: {lines:?}"
        );
        assert!(
            lines[0].starts_with("未実測: ") && lines[0].contains(why),
            "{HELPER_REL}:{ensure}: 理由の 1 行が「未実測: …{why}…」になっていない（{name}）: {}",
            lines[0]
        );
    }

    // 対照: 面を用意できれば起動し、面の指定（tako-vd）を明示して渡す。
    // 呼び出し側が別の面を明示したときは 12 の検査（#1760）が見る
    let r = run_helper(&scratch, &vd, &[], &[]);
    assert_eq!(
        r.rc, 0,
        "通常経路: 面を用意できたのに失敗した: {}",
        r.stderr
    );
    assert_eq!(
        r.started.as_deref().map(str::trim),
        Some(format!("TAKO_DISPLAY={STUB_VD_NAME}").as_str()),
        "{HELPER_REL}:{launch}: 通常経路で GUI へ渡る面の指定が違う（#1697 / #1744）"
    );
    assert!(
        r.stderr.trim().is_empty(),
        "通常経路: stderr が空ではない: {}",
        r.stderr
    );
}

// --------------------------------------------- 11. tako-vd 以外の面の明示を断る（#1760）

/// `launch_isolated_gui` の中の `(面の指定を判定する行, 断る行, GUI を起こす行)`
fn foreign_refusal_lines(src: &str) -> (Option<usize>, Option<usize>, usize) {
    let (body, at) = fn_body(HELPER_REL, src, "launch_isolated_gui()");
    let launch = find(&body, "\"$APP_BIN\"").unwrap_or_else(|| {
        panic!("{HELPER_REL}:{at}: launch_isolated_gui が GUI を起こす行が見つからない")
    });
    let judge = body
        .iter()
        .find(|(_, l)| {
            l.trim_start()
                .starts_with("if ! iso_display_allowed \"$display\"")
        })
        .map(|(n, _)| *n);
    let refuse = judge.and_then(|j| {
        body.iter()
            .find(|(n, l)| *n > j && l.contains("return \"$ISOLATED_GUI_RC_FOREIGN_DISPLAY\""))
            .map(|(n, _)| *n)
    });
    (judge, refuse, launch)
}

#[test]
fn ヘルパはtako_vd以外の面の明示を起動の前に断る() {
    let src = read(HELPER_REL);
    let (judge, refuse, launch) = foreign_refusal_lines(&src);
    let lines: Vec<&str> = src.lines().collect();
    let launch_line = lines[launch - 1].trim();
    let judge = judge.unwrap_or_else(|| {
        panic!(
            "{HELPER_REL}:{launch}: 面の指定を判定せずに GUI を起こしている（#1760）\n    {launch_line}\n\
             → 呼び出し側の `TAKO_DISPLAY=0`（= メイン画面）がそのまま GUI へ渡る。\
             起動の前に `if ! iso_display_allowed \"$display\"; then …; \
             return \"$ISOLATED_GUI_RC_FOREIGN_DISPLAY\"; fi` を置く"
        )
    });
    let refuse = refuse.unwrap_or_else(|| {
        panic!(
            "{HELPER_REL}:{judge}: 面の指定を判定しても断っていない（#1760）。\n\
             → `return \"$ISOLATED_GUI_RC_FOREIGN_DISPLAY\"` で返す"
        )
    });
    assert!(
        judge < launch && refuse < launch,
        "{HELPER_REL}:{judge}: 面の指定の判定（断る行 {HELPER_REL}:{refuse}）が \
         GUI を起こす行（{HELPER_REL}:{launch}）より後ろに在る（#1760）"
    );
    // GUI へ渡すのは判定した値。呼び出し側の `VAR=VAL`（"$@"）より後ろに置かないと、
    // 判定と違う値が GUI へ届く道が残る
    let (Some(args_at), Some(display_at)) = (
        launch_line.find("\"$@\""),
        launch_line.find("\"TAKO_DISPLAY=${display}\""),
    ) else {
        panic!(
            "{HELPER_REL}:{launch}: GUI を起こす行が「呼び出し側の VAR=VAL → 判定した面の指定」の\
             順で渡していない（#1760）\n    {launch_line}"
        );
    };
    assert!(
        args_at < display_at,
        "{HELPER_REL}:{launch}: 判定した面の指定が呼び出し側の VAR=VAL より前に在る \
         = 呼び出し側の `TAKO_DISPLAY=` が判定を素通りして勝つ（#1760）\n    {launch_line}"
    );
    // 断るときの終了コードは 2（4 = 未実測と読ませない）
    assert!(
        src.lines()
            .any(|l| l.trim() == "ISOLATED_GUI_RC_FOREIGN_DISPLAY=2"),
        "{HELPER_REL}: ISOLATED_GUI_RC_FOREIGN_DISPLAY が 2 ではない（#1760。\
         4 は「面を用意できない = 未実測」の番号で、書き方の誤りを未実測と読ませない）"
    );
    // 面の名前は面を用意する係（TAKO_VD_NAME）と同じものだけを見る。別に差し替えられると
    // 「tako-vd を用意して別の面へ窓を出す」口になる
    if let Some((i, l)) = lines
        .iter()
        .enumerate()
        .find(|(_, l)| !is_comment(l) && l.trim_start().starts_with("ISOLATED_GUI_DISPLAY="))
    {
        assert_eq!(
            l.trim(),
            "ISOLATED_GUI_DISPLAY=${TAKO_VD_NAME:-tako-vd}",
            "{HELPER_REL}:{}: 面の名前を面を用意する係と別に差し替えられる（#1760）",
            i + 1
        );
    } else {
        panic!("{HELPER_REL}: ISOLATED_GUI_DISPLAY の定義が見つからない");
    }
}

// --------------------------------------------- 12. 実際に走らせて断ることを見る（#1760）

#[cfg(unix)]
#[test]
fn tako_vd以外の面の明示ではヘルパは起動せずに理由を1行出す() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("foreign");
    let vd = scratch.0.join("virtual-display.sh");
    std::fs::write(&vd, STUB_VD).expect("偽の係を書く");
    std::fs::set_permissions(&vd, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let src = read(HELPER_REL);
    let (judge, _, launch) = foreign_refusal_lines(&src);
    // 名指す行: 判定の行（消えていれば起動行）と判定の実体
    let judge_at = judge.unwrap_or(launch);
    let (_, allowed_at) = fn_body(HELPER_REL, &src, "iso_display_allowed()");

    // 例の uuid（実機の値ではない）。VD は ensure が記録した tako-vd の uuid の役
    const VD_UUID: &str = "11111111-2222-4333-8444-555555555555";
    const OTHER_UUID: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let with_record: &[(&str, &str)] = &[("STUB_RECORDED_UUID", VD_UUID)];
    let other_uuid_arg = format!("TAKO_DISPLAY={OTHER_UUID}");
    let vd_uuid_arg = format!("TAKO_DISPLAY={VD_UUID}");
    let vd_name_arg = format!("TAKO_DISPLAY={STUB_VD_NAME}");
    type Env<'a> = &'a [(&'a str, &'a str)];

    // (場面, 係への env, launch_isolated_gui へ並べる VAR=VAL)
    let refused: &[(&str, Env, &[&str])] = &[
        ("引数で index 0（= メイン画面）", &[], &["TAKO_DISPLAY=0"]),
        ("export で index 0", &[("TAKO_DISPLAY", "0")], &[]),
        (
            "index: 付きの実在しうる index",
            &[],
            &["TAKO_DISPLAY=index:1"],
        ),
        ("境界の 1 つ下（index:99）", &[], &["TAKO_DISPLAY=index:99"]),
        (
            "先頭 0 の index（08 を 8 進と読まない）",
            &[],
            &["TAKO_DISPLAY=08"],
        ),
        (
            "別の面の名前",
            &[],
            &["TAKO_DISPLAY=Built-in Retina Display"],
        ),
        (
            "当たらないはずの名前",
            &[],
            &["TAKO_DISPLAY=no-such-display-1697"],
        ),
        ("記録と違う uuid", with_record, &[other_uuid_arg.as_str()]),
        ("記録が無いときの uuid", &[], &[vd_uuid_arg.as_str()]),
        (
            "tako-vd のあとに 0 を並べる（最後が勝つ）",
            &[],
            &[vd_name_arg.as_str(), "TAKO_DISPLAY=0"],
        ),
    ];
    for (name, env, args) in refused {
        let r = run_helper(&scratch, &vd, env, args);
        assert!(
            r.started.is_none(),
            "{HELPER_REL}:{judge_at}: tako-vd 以外の面の明示（{name}）で GUI を起動した（#1760）。\n\
             判定の実体は {HELPER_REL}:{allowed_at}。受け取った指定: {:?}",
            r.started
        );
        assert_eq!(
            r.rc, 2,
            "{HELPER_REL}:{judge_at}: tako-vd 以外の面の明示（{name}）で終了コードが 2 ではない（#1760）\n\
             stderr: {}",
            r.stderr
        );
        let lines: Vec<&str> = r.stderr.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(
            lines.len(),
            1,
            "{HELPER_REL}:{judge_at}: 理由が 1 行ではない（{name}）: {lines:?}"
        );
        assert!(
            lines[0].starts_with("ERROR: TAKO_DISPLAY=") && lines[0].contains("#1760"),
            "{HELPER_REL}:{judge_at}: 理由の 1 行が「ERROR: TAKO_DISPLAY=… #1760」になっていない（{name}）: {}",
            lines[0]
        );
    }

    // 対照: tako-vd を指す値・未指定・実在し得ない index は従来どおり起動する。
    // (場面, 係への env, VAR=VAL, GUI が受け取るべき指定)
    let vd_upper = VD_UUID.to_ascii_uppercase();
    let vd_upper_arg = format!("TAKO_DISPLAY={vd_upper}");
    let default_want = format!("TAKO_DISPLAY={STUB_VD_NAME}");
    let upper_name = format!("TAKO_DISPLAY=  {}  ", STUB_VD_NAME.to_ascii_uppercase());
    let allowed: Vec<(&str, Env, Vec<&str>, String)> = vec![
        ("未指定", &[], vec![], default_want.clone()),
        (
            "export の空（未指定と同じ）",
            &[("TAKO_DISPLAY", "")],
            vec![],
            default_want.clone(),
        ),
        (
            "差し替え口 ISOLATED_GUI_DISPLAY は効かない",
            &[("ISOLATED_GUI_DISPLAY", "0")],
            vec![],
            default_want.clone(),
        ),
        (
            "tako-vd の名前（大文字・前後の空白）",
            &[],
            vec![upper_name.as_str()],
            upper_name.clone(),
        ),
        (
            "記録と一致する uuid（大文字）",
            with_record,
            vec![vd_upper_arg.as_str()],
            vd_upper_arg.clone(),
        ),
        (
            "空（tako の定義で未指定 = 暗黙の既定）",
            &[],
            vec!["TAKO_DISPLAY="],
            "TAKO_DISPLAY=".into(),
        ),
        (
            "実在し得ない index（index:999）",
            &[],
            vec!["TAKO_DISPLAY=index:999"],
            "TAKO_DISPLAY=index:999".into(),
        ),
        (
            "境界ちょうど（index:100）",
            &[],
            vec!["TAKO_DISPLAY=index:100"],
            "TAKO_DISPLAY=index:100".into(),
        ),
        (
            "export の 0 を引数の tako-vd で上書き",
            &[("TAKO_DISPLAY", "0")],
            vec![vd_name_arg.as_str()],
            default_want.clone(),
        ),
    ];
    for (name, env, args, want) in &allowed {
        let r = run_helper(&scratch, &vd, env, args);
        assert_eq!(
            r.rc, 0,
            "{HELPER_REL}:{allowed_at}: tako-vd を指す値（{name}）を断った（#1760）: {}",
            r.stderr
        );
        assert_eq!(
            r.started.as_deref().map(|s| s.trim_end_matches('\n')),
            Some(want.as_str()),
            "{HELPER_REL}:{launch}: {name} で GUI へ渡る面の指定が違う（#1760）"
        );
        assert!(
            r.stderr.trim().is_empty(),
            "{name}: stderr が空ではない: {}",
            r.stderr
        );
    }
}

// --------------------------------------------- 13. 実在し得ない index の前提（#1760）

/// ヘルパが「実在し得ない」とみなす index の下限（`ISOLATED_GUI_ABSENT_INDEX_MIN`）
fn absent_index_min(src: &str) -> usize {
    src.lines()
        .find_map(|l| l.trim().strip_prefix("ISOLATED_GUI_ABSENT_INDEX_MIN="))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            panic!("{HELPER_REL}: ISOLATED_GUI_ABSENT_INDEX_MIN=<数> が見つからない")
        })
}

/// ヘルパが通す「当たらない面」は、tako の当て方でも**本当にどの面にも当たらず**、
/// 検証用 GUI では明示の見失い = 窓を開かずに終わる（#1697 の道）。
/// ここが崩れると、#1697 の検査の ④ が「窓を開かない」を確かめる代わりに
/// どこかの面（= ユーザーの画面）へ窓を開く
#[test]
fn 実在し得ないindexはtakoでも見失いになり窓を開かない() {
    use tako_core::platform::display::{
        explicit_request, miss_for, select, DisplayCandidate, Miss, Selection,
    };
    let src = read(HELPER_REL);
    let min = absent_index_min(&src);
    // 実機に 100 枚の面は無い。下限を小さくすると「実在しうる面」を通す
    assert!(
        min >= 32,
        "{HELPER_REL}: ISOLATED_GUI_ABSENT_INDEX_MIN={min} は実在しうる面の index に届く（#1760）"
    );
    // 下限の 1 つ手前まで面がある最悪の一覧（名前と uuid も持たせる = 名前・uuid の道で当たらないことも見る）
    let candidates: Vec<DisplayCandidate> = (0..min)
        .map(|i| DisplayCandidate {
            index: i,
            id: i as u64 + 1,
            uuid: Some(format!("00000000-0000-4000-8000-{i:012}")),
            name: Some(if i == 1 {
                "tako-vd".to_string()
            } else {
                format!("Display {i}")
            }),
            primary: i == 0,
            rect: None,
        })
        .collect();
    // 下限の手前は index として当たる = 下限が形だけではない（index の読み方が生きている）
    assert!(
        matches!(
            select(&format!("index:{}", min - 1), &candidates),
            Selection::Selected { .. }
        ),
        "index:{} が当たらない = tako の index の読み方が変わり、下限の前提が崩れている",
        min - 1
    );

    // #1697 の検査の ④ が実際に使う値もここへ並べる（書き換えたら一緒に見る）
    let script_rel = "scripts/test-display-uuid-1697.sh";
    let script = read(script_rel);
    let used = script
        .lines()
        .filter(|l| !is_comment(l))
        .find_map(|l| {
            l.split('"')
                .find_map(|w| w.strip_prefix("TAKO_DISPLAY=index:"))
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            panic!(
                "{script_rel}: ④ の「当たらない面」（TAKO_DISPLAY=index:N）が見つからない（#1760）"
            )
        });
    let used_n: usize = used
        .parse()
        .unwrap_or_else(|_| panic!("{script_rel}: index:{used} が数ではない"));
    assert!(
        used_n >= min,
        "{script_rel}: ④ の index:{used_n} はヘルパの下限 {min} より小さい = ヘルパが断る（#1760）"
    );

    for spec in [
        format!("index:{min}"),
        format!("index:{used_n}"),
        used_n.to_string(),
    ] {
        assert!(
            explicit_request(Some(&spec)),
            "{spec} が明示として扱われない = 見失っても暗黙の既定（ユーザーの画面へ開く）に倒れる"
        );
        assert!(
            matches!(select(&spec, &candidates), Selection::NotFound { .. }),
            "{spec} が {min} 枚の面のどれかに当たった = 「当たらない面」になっていない（#1760）"
        );
    }
    // 面が見えていて明示を見失った検証用 GUI は窓を開かない（#1697）。
    // この構えが FallBack へ戻ったときに落ちるのが #1697 の検査の ④
    assert_eq!(
        miss_for(true, false, true),
        Miss::Refuse,
        "明示の見失いで検証用 GUI が既定の面へ開く構えになっている（#1697）"
    );
}

// --------------------------------------------- 9. 呼び手が失敗を拾う（#1744）

/// 起動の直後に置いてよい受け方（`|| exit $?` / `|| RC=$?` / `if` の条件）。
/// `|| return` は許さない: 呼び手が戻り値を捨てると 4 が消える（#1744 の時点で
/// `test-editor-keys-1652.sh` の `run_section … || return 1` が実際にそうなっていた）
fn handles_failure(cmd: &str, conditional: bool) -> bool {
    if conditional {
        return true;
    }
    cmd.match_indices("||").any(|(i, _)| {
        let rest = cmd[i + 2..].trim_start();
        if rest.starts_with("exit") {
            return true;
        }
        // `NAME=$?`（戻り値を受けて自分で未実測を書く形）
        let name_len = rest
            .char_indices()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        name_len > 0 && rest[name_len..].starts_with("=$?")
    })
}

/// `launch_isolated_gui` を呼ぶ行なら `Some(条件として呼んでいるか)`
fn calls_launch(line: &str) -> Option<bool> {
    if is_comment(line) {
        return None;
    }
    let mut rest = line.trim_start();
    let mut conditional = false;
    for prefix in ["if ", "elif ", "! "] {
        if let Some(r) = rest.strip_prefix(prefix) {
            rest = r.trim_start();
            conditional = true;
        }
    }
    if let Some(r) = rest.strip_prefix("! ") {
        rest = r.trim_start();
    }
    // 先頭の `VAR=値` を読み飛ばす（`APP_BIN="$X/tako-app" launch_isolated_gui …`）
    while let Some((word, tail)) = rest.split_once(char::is_whitespace) {
        let is_assign = word.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if !is_assign {
            break;
        }
        rest = tail.trim_start();
    }
    let after = rest.strip_prefix("launch_isolated_gui")?;
    (after.is_empty() || after.starts_with(char::is_whitespace)).then_some(conditional)
}

/// 失敗を拾っていない起動の `(行番号, 呼び出し全体)` を返す（`\` の継続行はつなげて見る）
fn unhandled_launches(src: &str) -> (usize, Vec<(usize, String)>) {
    let lines: Vec<&str> = src.lines().collect();
    let mut found = 0;
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(conditional) = calls_launch(line) else {
            continue;
        };
        found += 1;
        let mut cmd = line.trim_end().to_string();
        let mut j = i;
        while cmd.ends_with('\\') && j + 1 < lines.len() {
            cmd.pop();
            j += 1;
            cmd.push(' ');
            cmd.push_str(lines[j].trim());
        }
        if !handles_failure(&cmd, conditional) {
            out.push((i + 1, cmd));
        }
    }
    (found, out)
}

#[test]
fn テストスクリプトは面を用意できない失敗を拾う() {
    let mut total = 0;
    let mut violations: Vec<String> = Vec::new();
    for path in test_scripts() {
        let src = std::fs::read_to_string(&path).expect("読める .sh");
        let (found, bad) = unhandled_launches(&src);
        total += found;
        for (n, cmd) in bad {
            violations.push(format!("{}:{n}: {}", rel_of(&path), cmd.trim()));
        }
    }
    // 走査が空振りしていない（#1744 の時点で 29 か所）
    assert!(
        total >= 20,
        "launch_isolated_gui の呼び出しが {total} か所しか見つからない（走査が壊れている）"
    );
    assert!(
        violations.is_empty(),
        "面を用意できないとき launch_isolated_gui は起動せずに 4 を返す（#1744）のに、\n\
         その失敗を拾っていない呼び出しがある。拾わないと「GUI が立たない」の FAIL や\n\
         20 秒の空待ちになり、未実測と読めない:\n    \
         launch_isolated_gui \"$TMP/app.log\" || exit $?   # 関数の中でも exit で止める\n\n{}",
        violations.join("\n")
    );
}

// --------------------------------------------- 10. 9 の検出力

#[test]
fn 失敗の拾い方の検査は実際の取りこぼしを検出する() {
    // #1744 の前に実在した形（そのまま戻ってきたら落ちること）
    for src in [
        "launch_isolated_gui \"$TMP/app.log\"\nAPP_PID=\"$ISOLATED_GUI_PID\"",
        "  launch_isolated_gui \"$log\" TAKO_VISUAL_TEST=1 \"$@\" || return 1",
        "launch_isolated_gui \"$TMP/app.log\" || true",
        "launch_isolated_gui \"$TMP/run.log\" \\\n    HOME=\"$TMP/home\" \\\n    TAKO_VISUAL_TEST=1",
        "    APP_BIN=\"$MOVED2/tako-app\" launch_isolated_gui \"$TMP/app2.log\" \\\n        \"HOME=$ISO_HOME\"",
    ] {
        let (found, bad) = unhandled_launches(src);
        assert_eq!(found, 1, "検査が呼び出しを見つけていない: {src}");
        assert_eq!(bad.len(), 1, "検査が取りこぼしを見逃している: {src}");
    }
    // 拾っている形とコメント・関数定義は引っかからない
    for src in [
        "launch_isolated_gui \"$TMP/app.log\" || exit $?",
        "  launch_isolated_gui \"$TMP/app-$1.log\" || exit 1",
        "launch_isolated_gui \"$SANDBOX/app.log\" || GUI_RC=$?",
        "if ! launch_isolated_gui \"$TMP/a.log\"; then",
        "launch_isolated_gui \"$TMP/run.log\" \\\n    TAKO_VISUAL_TEST=1 \"$OUT\" || exit $?",
        "    APP_BIN=\"$MOVED2/tako-app\" launch_isolated_gui \"$TMP/app2.log\" \\\n        \"HOME=$ISO_HOME\" || exit $?",
        "# 面を起こすのは launch_isolated_gui の中（#1490）",
        "launch_isolated_gui() {",
    ] {
        let (_, bad) = unhandled_launches(src);
        assert!(bad.is_empty(), "検査が正しい書き方を誤検出している: {src} → {bad:?}");
    }
}
