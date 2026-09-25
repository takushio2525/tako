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
//! 6. [`ヘルパは配線済みの機でだけ面の指定を明示する`] — 明示の `TAKO_DISPLAY` を見失った
//!    検証用 GUI は窓を開かずに終わる（#1697）ので、配線済みの機では明示し、未配線の機では
//!    渡さない（渡すと CI・他人の機で検証が回らなくなる）

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
    "\n    launch_isolated_gui \"$TMP/app.log\"",
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
    // 配線が無い環境（CI・他人の機・Windows）では素通しする。ここで止めると検証が回らない
    assert!(
        ens.contains("|| \\") || ens.contains("|| echo") || ens.contains("|| true"),
        "{HELPER_REL}:{ens_at}: 面を用意できないときに素通ししていない \
         = tako-vd を配線していない環境で検証そのものが回らなくなる"
    );

    // pid は変数へ置く（`$( )` で受けると副シェルの子になり呼び出し側の wait が壊れる）
    assert!(
        find(&body, "ISOLATED_GUI_PID=$!").is_some(),
        "{HELPER_REL}:{at}: 起こした pid を ISOLATED_GUI_PID へ置いていない（#1490）"
    );
}

/// 面の指定は**配線済みの機でだけ明示する**（#1697）。
///
/// tako は明示の `TAKO_DISPLAY` を見失うと窓を開かずに終わる（名前だけが読めない瞬間に
/// ユーザーの画面へ落ちないため）。なので「配線済み」（今回 ensure が通った / この機で
/// uuid を記録したことがある）ときは明示し、そうでない機では渡さない。
/// 常に明示へ戻すと未配線の機で検証が回らず、常に渡さないへ戻すと配線済みの機で
/// 「見失ったらユーザーの画面へ落ちる」側（暗黙の既定）へ倒れる
#[test]
fn ヘルパは配線済みの機でだけ面の指定を明示する() {
    let src = read(HELPER_REL);
    let (body, at) = fn_body(HELPER_REL, &src, "launch_isolated_gui()");
    let wired = find(&body, "iso_display_recorded").unwrap_or_else(|| {
        panic!(
            "{HELPER_REL}:{at}: launch_isolated_gui が「この機で uuid を記録したことがあるか」を \
             見ていない（#1697）= ensure が今回だけ失敗した配線済みの機を未配線と読み違える"
        )
    });
    let explicit = find(&body, "TAKO_DISPLAY=$ISOLATED_GUI_DISPLAY").unwrap_or_else(|| {
        panic!(
            "{HELPER_REL}:{at}: launch_isolated_gui が配線済みの機で面の指定を明示していない \
             （#1697）= 見失ったときユーザーの画面へ落ちる暗黙の既定に倒れる"
        )
    });
    assert!(
        wired < explicit,
        "{HELPER_REL}:{explicit}: 面の指定を明示する行が配線の判定（{HELPER_REL}:{wired}）より前に在る（#1697）"
    );
    // 無条件の明示（#1697 前の形）は未配線の機で検証を止める
    if let Some(n) = find(&body, "TAKO_DISPLAY=${TAKO_DISPLAY:-$ISOLATED_GUI_DISPLAY}") {
        panic!(
            "{HELPER_REL}:{n}: 面の指定を無条件に明示している（#1697 前の形）。\n\
             → 未配線の機（CI・他人の機）では tako が窓を開かずに終わり検証が回らない"
        );
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
