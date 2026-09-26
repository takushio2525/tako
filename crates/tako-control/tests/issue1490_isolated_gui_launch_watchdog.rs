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
    let explicit = find(
        &body,
        "\"TAKO_DISPLAY=${TAKO_DISPLAY:-$ISOLATED_GUI_DISPLAY}\"",
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

/// 面を用意する係の偽物。`STUB_ENSURE_RC` / `STUB_ENSURE_ERR` / `STUB_BOUNDS_RC` で振る舞いを決める
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
esac
exit 9
"#;

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

/// ヘルパを source して `launch_isolated_gui` を 1 回呼ぶ（macOS 同梱の bash 3.2 で走らせる）
#[cfg(unix)]
fn run_helper(scratch: &Scratch, vd: &Path, env: &[(&str, &str)]) -> Run {
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
launch_isolated_gui "$STUB_DIR/app.log" 2>"$STUB_ERR"
rc=$?
[ -n "$ISOLATED_GUI_PID" ] && wait "$ISOLATED_GUI_PID"
exit "$rc"
"#;
    let mut cmd = std::process::Command::new("/bin/bash");
    cmd.arg("-c")
        .arg(script)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        // ヘルパが差し替え口を無視して本物の virtual-display.sh を呼ぶ形へ後退しても、
        // 実機の面・器・記録に触れずに即失敗するよう閉じる（実在しない面 / 器なし / 一時 dir）
        .env("TAKO_VD_NAME", "tako-vd-watchdog-1744-absent")
        .env("TAKO_VD_BETTERDISPLAY_APP", dir.join("NoBetterDisplay.app"))
        .env("TAKO_VD_RECORD_DIR", dir.join("record"))
        .env("ISOLATED_GUI_DISPLAY", "tako-vd")
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
        let r = run_helper(&scratch, vd_path, env);
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

    // 対照: 面を用意できれば起動し、面の指定を明示して渡す（呼び出し側の明示はそのまま）
    for (name, env, want) in [
        ("通常経路", &[][..], "TAKO_DISPLAY=tako-vd"),
        (
            "呼び出し側の明示",
            &[("TAKO_DISPLAY", "0")][..],
            "TAKO_DISPLAY=0",
        ),
    ] {
        let r = run_helper(&scratch, &vd, env);
        assert_eq!(r.rc, 0, "{name}: 面を用意できたのに失敗した: {}", r.stderr);
        assert_eq!(
            r.started.as_deref().map(str::trim),
            Some(want),
            "{HELPER_REL}:{launch}: {name} で GUI へ渡る面の指定が違う（#1697 / #1744）"
        );
        assert!(
            r.stderr.trim().is_empty(),
            "{name}: stderr が空ではない: {}",
            r.stderr
        );
    }
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
