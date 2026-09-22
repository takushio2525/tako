//! 番犬: **tako が描く UI の文字列に絵文字を置かない**（#1536 / #217）
//!
//! ## なぜ止めるのか
//!
//! ユーザーの確定方針。絵文字は UI が安っぽく見えるうえ、**フォント依存で描画が揺れる**
//! （同じ文字が環境によってカラー絵文字・白黒グリフ・豆腐のどれにもなる）。
//! 印が要る場所は `gpui::svg()` に描画プリミティブを渡す
//! （アセット `crates/tako-app/assets/icons/ui/`・定数 `file_icons::ui_icon`）。
//!
//! これまでは `ui_text` のカタログ検査（#217）だけが見ていたが、そこは
//! **`tr!` で登録された文字列しか通らない**。render コードへ直書きした
//! `.child("⬆")` や、言語非依存の `pub const` は素通りする。実際 #1536 の時点で
//! `right_panel.rs` の `⬆`・`preview_render.rs` の `↔`・
//! `ui_text/preview.rs` の `▶`（異体字セレクタでテキスト表示へ倒していた形）が残っていた。
//!
//! ## 何を見るか
//!
//! 走査は `crates/tako-app/src` の全 `.rs`。見るのは
//! **本番コードの文字列 / 文字リテラルの中身だけ**
//! （`common/production_range.rs` でテスト領域を潰してから
//! `common/code_view.rs` の `literals_only` を掛ける）。
//!
//! **走査そのものは `common/emoji_scan.rs` の 1 実装**で、CLI 出力を見張る
//! `issue1578_no_emoji_cli_watchdog` と共有する（#1578）。この番犬が持つのは
//! 「どこを見るか」（[`ROOTS`]）と「何を許すか」（[`ALLOW`]）だけ。
//!
//! - **コメントは対象外**。UI には出ないし、規約や理由の説明文に禁止文字そのものを
//!   書けなくなると「何を禁じているか」をソースに残せない
//! - **`#[cfg(test)]` の中も対象外**。`text_field.rs` / `right_panel.rs` の
//!   `\u{1F600}` は「**ユーザーが打った絵文字**を文字境界で壊さない」ことの検証で、
//!   tako が描く UI ではない。範囲取りは `production_range::scan` の 1 実装を通す
//!   （`src.find("#[cfg(test)]")` で切ると途中のヘルパ以降が視界から消える = #1420）
//! - 生の文字だけでなく **`\u{XXXX}` の書き方も復号して見る**（生の文字を避けて
//!   書けば通る、という抜け道を残さない）
//! - 「絵文字か」の判定は `tako_core::emoji::is_emoji` の **1 実装**を通す。
//!   `ui_text` のカタログ検査も同じ関数を呼ぶので、範囲がずれて
//!   「片方は緑・片方は赤」になることがない
//!
//! ## 例外（[`ALLOW`]）
//!
//! `crates/tako-app/src/main.rs` の**セルフテスト（`TAKO_SELF_TEST=1`）が再現する
//! 画面データ**だけ。セルフテストは env で分岐する**本番コード**なので
//! `#[cfg(test)]` の除外に当たらないが、中身はターミナルと claude の TUI であって
//! tako が描く UI ではなく、**その文字が出ること自体を検証している**ので消せない。
//! 例外は「ファイル × 文字 × 件数 × 理由」で持つので、
//! **同じファイルに別の絵文字が増えても、同じ絵文字が 1 個増えても落ちる**。

#[path = "common/emoji_scan.rs"]
mod emoji_scan;

use emoji_scan::{production_range, repo_root, Allow, Hit};

/// セルフテスト（`TAKO_SELF_TEST=1`）が画面へ流し込む**ターミナル / claude TUI の
/// 中身**。`#[cfg(test)]` ではなく env で分岐する本番コードなので範囲取りでは外れない。
const MAIN: &str = "crates/tako-app/src/main.rs";

const ALLOW: &[Allow] = &[
    Allow {
        rel: MAIN,
        ch: '\u{23FA}', // ⏺
        count: 5,
        why: "claude の応答マーカー。セルフテストがターミナルへ流し込む画面データで、\
              フォールバックフォント（advance がセル幅と合わないグリフ）の描画そのものが検証対象",
    },
    Allow {
        rel: MAIN,
        ch: '\u{276F}', // ❯
        count: 7,
        why: "claude TUI の入力欄・選択肢の行頭。ダイアログ検知（#1293 / #633）と\
              リミット復帰（#813）の検証データで、実採取の形を 1 バイトも変えられない",
    },
    Allow {
        rel: MAIN,
        ch: '\u{23F5}', // ⏵
        count: 6,
        why: "claude TUI のフッタ `⏵⏵ auto mode on`。生成中かどうかの判定（#1067）の\
              検証データで、実採取の形を変えると検知そのものが試せない",
    },
    Allow {
        rel: MAIN,
        ch: '\u{273B}', // ✻
        count: 1,
        why: "claude TUI の思考中スピナー。テーマフォント外グリフの混在行を\
              打鍵経路で再現するための入力文字列",
    },
    Allow {
        rel: MAIN,
        ch: '\u{1F389}', // 🎉
        count: 1,
        why: "ターミナルグリッドの絵文字描画（セル幅 2 のカラーグリフ）の検証入力。\
              ユーザーが打った絵文字を tako が正しく描けることを見ている",
    },
    Allow {
        rel: MAIN,
        ch: '\u{2705}', // ✅
        count: 1,
        why: "同上（🎉 と同じ行の printf）。絵文字混在行の折り返しとグループ化を見る",
    },
];

/// 走査対象（`crates/tako-app/src` の全 `.rs`）
const ROOTS: &[&str] = &["crates/tako-app/src"];

fn app_sources() -> Vec<(String, String)> {
    emoji_scan::sources_under(ROOTS)
}

/// 走査全体の違反（許可リストを引く前の生の一覧）
fn all_hits() -> Vec<Hit> {
    app_sources()
        .iter()
        .flat_map(|(rel, src)| emoji_scan::hits_in(rel, src))
        .collect()
}

#[test]
fn tako_appの画面文字列に絵文字が無い() {
    let problems = emoji_scan::problems(
        &all_hits(),
        ALLOW,
        "UI の文字列に絵文字。`gpui::svg()` + `file_icons::ui_icon` で描くか、\
         ターミナル / claude の画面データなら ALLOW へ理由つきで載せる",
    );
    assert!(
        problems.is_empty(),
        "UI に絵文字を使わない（#1536 / #217）に反する箇所が {} 件:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn 許可リストは理由を必ず持つ() {
    emoji_scan::assert_allow_is_justified(ALLOW);
}

#[test]
fn 走査範囲が黙って消えていない() {
    let sources = app_sources();
    assert!(
        sources.len() > 30,
        "走査できたファイルが {} 件しかない（置き場が変わった?）",
        sources.len()
    );
    // リテラルの眺めが空になっていたら、検出力を失ったまま緑になる
    let literal_bytes: usize = sources
        .iter()
        .map(|(_, src)| {
            production_range::code_view::literals_only(&production_range::scan(src).text)
                .bytes()
                .filter(|b| !b.is_ascii_whitespace())
                .count()
        })
        .sum();
    assert!(
        literal_bytes > 100_000,
        "文字列リテラルの眺めが {literal_bytes} バイトしかない（literals_only が壊れている?）"
    );
}

#[test]
fn 番犬は修正前の形を名指しで落とす() {
    // #1536 で実際に消した 4 つの形を合成ソースへ戻す
    let injected = r#"
fn render() {
    div().child("⬆")                                   // right_panel.rs の復帰ボタン
        .child("↔")                                     // preview_render.rs の置換欄
        .child("\u{25b6}\u{fe0e} 再生")                 // ui_text/preview.rs の再生ラベル
        .child("☕ 休憩");                               // Issue が名指しした形
}
"#;
    let hits = emoji_scan::hits_in("crates/tako-app/src/injected.rs", injected);
    let found: Vec<char> = hits.iter().map(|h| h.ch).collect();
    for expected in ['\u{2B06}', '\u{2194}', '\u{25B6}', '\u{FE0F}', '\u{2615}'] {
        if expected == '\u{FE0F}' {
            continue; // FE0E は許可側なので出ない
        }
        assert!(
            found.contains(&expected),
            "U+{:04X} を検出できていない: {found:?}",
            expected as u32
        );
    }
    // `\u{25b6}` はエスケープ表記から拾えていること
    assert!(
        hits.iter().any(|h| h.ch == '\u{25B6}' && h.escaped),
        "エスケープ表記の絵文字を拾えていない"
    );
    // 名指しが `file:line` であること
    let up = hits.iter().find(|h| h.ch == '\u{2B06}').expect("⬆");
    assert_eq!(up.line, 3, "行番号がずれている: {}", up.describe());
    assert!(up
        .describe()
        .starts_with("crates/tako-app/src/injected.rs:3:"));
}

#[test]
fn コメントの中の絵文字は落とさない() {
    // 規約や理由の説明文には禁止文字そのものを書けること（書けないと何を
    // 禁じているかをソースへ残せない）
    let only_comments = r#"
// #1536: ☕ や 🌐 は UI に置かない
/// `⬆` は `ui_icon::UNSHELVE` で描く
fn f() {}
"#;
    assert!(
        emoji_scan::hits_in("crates/tako-app/src/c.rs", only_comments).is_empty(),
        "コメントを違反として拾っている"
    );
}

/// `code_view` を `Keep` で出し分ける形へ替えた（#1536）ので、**コード側の眺めが
/// 1 バイトも変わっていない**ことと、2 つの眺めが噛み合っていることを固定する
#[test]
fn 二つの眺めは噛み合っていて長さを変えない() {
    let src = "// コメント ☕\nfn f() {\n    let s = \"本文 ☕\";\n    let c = 'x';\n    let r = r#\"生 ☕\"#;\n}\n";
    let code = production_range::code_view::code_view(src);
    let lits = production_range::code_view::literals_only(src);
    assert_eq!(code.len(), src.len(), "コード側でバイト長が変わっている");
    assert_eq!(lits.len(), src.len(), "リテラル側でバイト長が変わっている");
    assert_eq!(
        code.lines().count(),
        src.lines().count(),
        "行番号が保たれていない"
    );
    // コード側: 識別子は残り、コメントと文字列の中身は消える
    assert!(code.contains("fn f()") && code.contains("let s ="));
    assert!(!code.contains("コメント") && !code.contains("本文") && !code.contains("生"));
    // リテラル側: 文字列の中身だけ残り、コードとコメントは消える
    assert!(
        lits.contains("本文 \u{2615}"),
        "通常文字列の中身が残っていない"
    );
    assert!(lits.contains("生 \u{2615}"), "生文字列の中身が残っていない");
    assert!(!lits.contains("fn f()"), "コードが残っている");
    assert!(!lits.contains("コメント"), "コメントが残っている");
    // 同じ位置で両方が原文を残すことはない（片方は必ず空白）
    for (i, ((a, b), o)) in code.bytes().zip(lits.bytes()).zip(src.bytes()).enumerate() {
        if o != b' ' && o != b'\n' {
            assert!(
                !(a == o && b == o),
                "{i} バイト目を両方の眺めが残している: {:?}",
                o as char
            );
        }
    }
}

#[test]
fn 判定の写しを持たない() {
    // `ui_text` 側と番犬側が別々の範囲を持つと、片方だけ広げたときに気づけない
    let ui_text = std::fs::read_to_string(repo_root().join("crates/tako-app/src/ui_text/mod.rs"))
        .expect("ui_text/mod.rs");
    assert!(
        code_view(&ui_text).contains("tako_core::emoji::is_emoji"),
        "ui_text のカタログ検査が共有実装を呼んでいない（範囲がずれる）"
    );
    // 走査は `common/emoji_scan.rs` へ寄せた（#1578）ので、判定を呼んでいるのは
    // そちら。**この番犬自身の説明文で緑になってはいけない**ので実体を見る
    let shared =
        std::fs::read_to_string(repo_root().join("crates/tako-control/tests/common/emoji_scan.rs"))
            .expect("共有の走査");
    assert!(
        code_view(&shared).contains("tako_core::emoji::is_emoji"),
        "共有の走査が判定の 1 実装を呼んでいない（コメントではなくコードで）"
    );
    let me = std::fs::read_to_string(
        repo_root().join("crates/tako-control/tests/issue1536_no_emoji_ui_watchdog.rs"),
    )
    .expect("この番犬自身");
    assert!(
        // `#[path = "…"]` は文字列リテラルなので code_view では消える。
        // モジュール宣言そのもの（コード）を見る
        code_view(&me).contains("mod emoji_scan"),
        "番犬が共有の走査を使っていない（写しを持つと範囲がずれる）"
    );
}

/// 「コメントではなくコードに書いてあるか」を見るための眺め（説明文で緑にしない）
fn code_view(src: &str) -> String {
    production_range::code_view::code_view(src)
}
