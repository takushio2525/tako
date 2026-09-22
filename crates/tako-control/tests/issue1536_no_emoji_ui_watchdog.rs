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

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;
use production_range::code_view;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

/// 絵文字を残してよい場所。**`why` が空の行は無効**（下の検査が落とす）
struct Allow {
    /// リポジトリルートからの相対パス
    rel: &'static str,
    /// 許す文字
    ch: char,
    /// そのファイルにその文字が出る件数（増減したら落とす = 見直しを強制する）
    count: usize,
    /// なぜ「tako が描く UI の絵文字」ではないのか
    why: &'static str,
}

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
fn app_sources() -> Vec<(String, String)> {
    let root = repo_root().join("crates/tako-app/src");
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{dir:?} を読めない: {e}"));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(repo_root())
                    .expect("リポジトリ内")
                    .to_string_lossy()
                    .replace('\\', "/");
                let src = std::fs::read_to_string(&path).expect("UTF-8 のソース");
                out.push((rel, src));
            }
        }
    }
    out.sort();
    out
}

/// 見つけた 1 件（`file:line` で名指しするための最小の情報）
#[derive(Debug, Clone)]
struct Hit {
    rel: String,
    line: usize,
    ch: char,
    /// `\u{XXXX}` の書き方で見つけたか（報告に出す）
    escaped: bool,
}

impl Hit {
    fn describe(&self) -> String {
        let how = if self.escaped {
            r"（\u{} 表記）"
        } else {
            ""
        };
        format!(
            "{}:{}: {:?} (U+{:04X}){}",
            self.rel, self.line, self.ch, self.ch as u32, how
        )
    }
}

/// `\u{XXXX}` を復号して返す（`}` が無い・16 進でない書き方は無視する）
fn unicode_escapes(line: &str) -> Vec<char> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 3 < b.len() {
        if b[i] == b'\\' && b[i + 1] == b'u' && b[i + 2] == b'{' {
            let start = i + 3;
            let mut j = start;
            // 16 進の続きは ASCII なので、多バイト文字の途中で切れることはない
            while j < b.len() && b[j] != b'}' {
                j += 1;
            }
            if j > start && j < b.len() {
                if let Ok(cp) = u32::from_str_radix(&line[start..j], 16) {
                    if let Some(c) = char::from_u32(cp) {
                        out.push(c);
                    }
                }
            }
            i = j.max(start);
            continue;
        }
        i += 1;
    }
    out
}

/// 1 ファイルぶんの違反（**本番コードの文字列リテラルの中身だけ**を見る）
fn hits_in(rel: &str, src: &str) -> Vec<Hit> {
    // 下限は掛けない: `main.rs` のようにテストの厚いファイルまで舐めるため
    // （黙って縮んでいないことは `走査範囲が黙って消えていない` が見る）
    let production = production_range::scan(src).text;
    let view = code_view::literals_only(&production);
    let mut out = Vec::new();
    for (idx, line) in view.lines().enumerate() {
        let no = idx + 1;
        for ch in line.chars() {
            if tako_core::emoji::is_emoji(ch) {
                out.push(Hit {
                    rel: rel.to_string(),
                    line: no,
                    ch,
                    escaped: false,
                });
            }
        }
        for ch in unicode_escapes(line) {
            if tako_core::emoji::is_emoji(ch) {
                out.push(Hit {
                    rel: rel.to_string(),
                    line: no,
                    ch,
                    escaped: true,
                });
            }
        }
    }
    out
}

/// 走査全体の違反（許可リストを引く前の生の一覧）
fn all_hits() -> Vec<Hit> {
    app_sources()
        .iter()
        .flat_map(|(rel, src)| hits_in(rel, src))
        .collect()
}

#[test]
fn tako_appの画面文字列に絵文字が無い() {
    let hits = all_hits();
    let mut unexpected: Vec<&Hit> = Vec::new();
    let mut counted: Vec<(&Allow, Vec<&Hit>)> =
        ALLOW.iter().map(|a| (a, Vec::new())).collect::<Vec<_>>();
    for hit in &hits {
        match counted
            .iter_mut()
            .find(|(a, _)| a.rel == hit.rel && a.ch == hit.ch)
        {
            Some((_, got)) => got.push(hit),
            None => unexpected.push(hit),
        }
    }

    let mut problems: Vec<String> = Vec::new();
    for hit in &unexpected {
        problems.push(format!(
            "{}  ← UI の文字列に絵文字。`gpui::svg()` + `file_icons::ui_icon` で描くか、\
             ターミナル / claude の画面データなら ALLOW へ理由つきで載せる",
            hit.describe()
        ));
    }
    for (allow, got) in &counted {
        if got.len() != allow.count {
            let where_ = got
                .iter()
                .map(|h| h.describe())
                .collect::<Vec<_>>()
                .join("\n    ");
            problems.push(format!(
                "{} の {:?} が {} 件（許可リストは {} 件）。\
                 増えたなら UI へ混ざっていないか、減ったなら許可の理由がまだ要るかを見直す\n    {}",
                allow.rel,
                allow.ch,
                got.len(),
                allow.count,
                where_
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "UI に絵文字を使わない（#1536 / #217）に反する箇所が {} 件:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn 許可リストは理由を必ず持つ() {
    for a in ALLOW {
        assert!(
            !a.why.trim().is_empty(),
            "{} の {:?} に理由が無い（理由なしの許可は認めない）",
            a.rel,
            a.ch
        );
        assert!(a.count > 0, "{} の {:?} の件数が 0", a.rel, a.ch);
    }
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
            code_view::literals_only(&production_range::scan(src).text)
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
    let hits = hits_in("crates/tako-app/src/injected.rs", injected);
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
        hits_in("crates/tako-app/src/c.rs", only_comments).is_empty(),
        "コメントを違反として拾っている"
    );
}

/// `code_view` を `Keep` で出し分ける形へ替えた（#1536）ので、**コード側の眺めが
/// 1 バイトも変わっていない**ことと、2 つの眺めが噛み合っていることを固定する
#[test]
fn 二つの眺めは噛み合っていて長さを変えない() {
    let src = "// コメント ☕\nfn f() {\n    let s = \"本文 ☕\";\n    let c = 'x';\n    let r = r#\"生 ☕\"#;\n}\n";
    let code = code_view::code_view(src);
    let lits = code_view::literals_only(src);
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
        ui_text.contains("tako_core::emoji::is_emoji"),
        "ui_text のカタログ検査が共有実装を呼んでいない（範囲がずれる）"
    );
    let me = std::fs::read_to_string(
        repo_root().join("crates/tako-control/tests/issue1536_no_emoji_ui_watchdog.rs"),
    )
    .expect("この番犬自身");
    assert!(
        me.contains("tako_core::emoji::is_emoji"),
        "番犬が共有実装を呼んでいない"
    );
}
