//! 番犬: **tako が端末へ出す文字列に絵文字を置かない**（#1578 / #1536 / #217）
//!
//! ## なぜ止めるのか
//!
//! ユーザーの確定方針。#1536（PR #1573）で GUI 側（`crates/tako-app/src`）は
//! `tako_core::emoji::is_emoji` の 1 実装 + 番犬で固定したが、**CLI の出力には
//! 同じ規律が届いていなかった**。AGENTS.md の「コマンド案内の規約（#322）」が
//! 「CLI 出力・system prompt・docs のすべてに適用」と言っているとおり、
//! 端末へ出る文字列も同じ扱いにする。
//!
//! 理由は GUI と同じ 2 つで、**安っぽく見える**ことと、**フォント依存で描画が揺れる**こと。
//! 端末ではこれに **3 つめ**が乗る: 絵文字は多くが**セル幅 2** で、フォントが持って
//! いなければ豆腐か幅 1 になるため、**桁を揃えた出力が環境ごとに崩れる**。
//! `tako fda status` の `✓` / `△` の列がまさにそれだった。
//!
//! 印が要る場所は**文字ラベル**（`[OK]` / `[情報]` / `[警告]` / `[不足]` / `[任意]` /
//! `[失敗]`）にする。これは `crates/tako-cli/src/setup.rs` が既に使っている作法で、
//! #1578 の時点で `main.rs` だけが取り残されていた（`agents status` の
//! `✓ エージェント共通ルール同期: 最新` に対し、setup.rs は同じ文言を
//! `[OK] エージェント共通ルール同期: 最新` で出していた）。
//!
//! ## 何を見るか
//!
//! 走査は `crates/tako-cli/src` と `crates/tako-control/src` の全 `.rs`。
//! 見るのは**本番コードの文字列 / 文字リテラルの中身だけ**で、
//! 走査そのものは [`emoji_scan`] の 1 実装（#1536 の番犬と共有）。
//!
//! **「`println!` の引数だけ」には絞らない**。出力は `eprintln!` / `format!` /
//! `Err(String)` / 定数 / `serde_json` の値と何段も経由して端末へ出るので、
//! マクロ名で絞ると経路を 1 つ足すたびに穴が空く。**リテラルを全部見て、
//! 端末へ出ないものだけを理由つきで許す**（[`ALLOW`]）ほうが、
//! 「なぜその絵文字が残っているか」がソースに残る。
//!
//! ## 例外（[`ALLOW`]）
//!
//! **AI だけが読む MCP ツールカタログの説明文**。ここの `❯` は claude TUI の
//! 入力行がどの字で始まるかという**データ**であって、tako が人へ見せる装飾ではない。
//! 例外は「ファイル × 文字 × 件数 × 理由」で持つので、
//! **同じファイルに別の絵文字が増えても、同じ絵文字が 1 個増えても落ちる**。
//!
//! なお、claude TUI を**読む側**（`claude_tui.rs` の `ch == '❯'` など）は
//! 多バイトの**文字リテラル**なので `literals_only` の眺めに入らない
//! （`code_view` の既知の穴。`char` は出力文字列にならないので実害が無い）。

#[path = "common/emoji_scan.rs"]
mod emoji_scan;

use emoji_scan::{production_range, Allow, Hit};

/// 走査する置き場（CLI 本体と、CLI / MCP が呼ぶ制御プレーン）
const ROOTS: &[&str] = &["crates/tako-cli/src", "crates/tako-control/src"];

/// MCP の `tools/list` が返すツール説明。**AI だけが読む**ので端末の出力ではない
const MCP_CATALOG: &str = "crates/tako-control/src/mcp/catalog.rs";

const ALLOW: &[Allow] = &[Allow {
    rel: MCP_CATALOG,
    ch: '\u{276F}', // ❯
    // #1711 で tako_send_input の説明の重複（await_prompt の説明を description にも
    // 書いていた 1 文）を外して 4 → 3。残る 3 件は await_prompt と input_status の説明
    count: 3,
    why: "claude TUI の入力行の字（tako_send_input の await_prompt / tako_read_pane の \
          input_status が何を見ているか）。MCP ツールカタログは AI が読む説明文で\
          端末へは出ず、この字そのものが判定対象を指す情報なので置き換えられない",
}];

/// 走査全体の違反（許可リストを引く前の生の一覧）
fn all_hits() -> Vec<Hit> {
    emoji_scan::sources_under(ROOTS)
        .iter()
        .flat_map(|(rel, src)| emoji_scan::hits_in(rel, src))
        .collect()
}

#[test]
fn cliの出力文字列に絵文字が無い() {
    let problems = emoji_scan::problems(
        &all_hits(),
        ALLOW,
        "端末へ出る文字列に絵文字。`[OK]` / `[情報]` / `[警告]` / `[不足]` / `[任意]` / \
         `[失敗]` の文字ラベルにするか（setup.rs の作法）、AI だけが読む文字列なら \
         ALLOW へ理由つきで載せる",
    );
    assert!(
        problems.is_empty(),
        "CLI 出力に絵文字を使わない（#1578 / #1536）に反する箇所が {} 件:\n{}",
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
    let sources = emoji_scan::sources_under(ROOTS);
    assert!(
        // 実測 94 ファイル（#1578 時点）
        sources.len() > 50,
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
        // 実測 426,964 バイト（#1578 時点）。半分を割ったら眺めが壊れている
        literal_bytes > 200_000,
        "文字列リテラルの眺めが {literal_bytes} バイトしかない（literals_only が壊れている?）"
    );
    // 両方のクレートを実際に見ていること（片方の綴りを間違えても緑にならない）
    for root in ROOTS {
        assert!(
            sources.iter().any(|(rel, _)| rel.starts_with(root)),
            "{root} を 1 ファイルも走査していない"
        );
    }
}

#[test]
fn 番犬は修正前の形を名指しで落とす() {
    // #1578 で実際に消した 4 種を合成ソースへ戻す（行番号もそのまま突き合わせる）
    let injected = r#"
fn out() {
    eprintln!("ℹ {notice}");
    eprintln!("✓ フルディスクアクセス: 付与済み");
    let mark = match action { "updated" => "✓", _ => "✗" };
    format!("⚠ {source} のモデル '{model}' は 1M コンテキスト版のため…")
}
"#;
    let hits = emoji_scan::hits_in("crates/tako-cli/src/injected.rs", injected);
    for (ch, line) in [
        ('\u{2139}', 3), // ℹ
        ('\u{2713}', 4), // ✓
        ('\u{2717}', 5), // ✗
        ('\u{26A0}', 6), // ⚠
    ] {
        let hit = hits
            .iter()
            .find(|h| h.ch == ch && h.line == line)
            .unwrap_or_else(|| {
                panic!(
                    "U+{:04X} を {line} 行目で検出できていない: {:?}",
                    ch as u32,
                    hits.iter().map(Hit::describe).collect::<Vec<_>>()
                )
            });
        assert!(
            hit.describe()
                .starts_with(&format!("crates/tako-cli/src/injected.rs:{line}:")),
            "名指しが file:line になっていない: {}",
            hit.describe()
        );
    }
    // 許可リストを引いても落ちること（`injected.rs` は ALLOW に無い）
    assert!(
        !emoji_scan::problems(&hits, ALLOW, "x").is_empty(),
        "許可リストを通すと違反が消えている"
    );
}

#[test]
fn 許可した場所に別の絵文字が増えたら落とす() {
    // 件数だけでなく「その文字か」も見ていること（❯ を許した穴から ✅ が通らない）
    let injected = "fn d() { let _ = \"❯ ok\"; let _ = \"✅ done\"; }\n";
    let problems = emoji_scan::problems(&emoji_scan::hits_in(MCP_CATALOG, injected), ALLOW, "x");
    assert!(
        problems.iter().any(|p| p.contains("2705")),
        "許可ファイルに増えた別の絵文字を見逃している: {problems:?}"
    );
}

#[test]
fn コメントの中の絵文字は落とさない() {
    // 規約や理由の説明文には禁止文字そのものを書けること（書けないと何を
    // 禁じているかをソースへ残せない）
    let only_comments = r#"
// #1578: ℹ や ⚠ は端末へ出さない
/// `✓` は `[OK]` の文字ラベルで出す
fn f() {}
"#;
    assert!(
        emoji_scan::hits_in("crates/tako-cli/src/c.rs", only_comments).is_empty(),
        "コメントを違反として拾っている"
    );
}

#[test]
fn 判定と走査の写しを持たない() {
    // #1536（GUI）と #1578（CLI）が別々の走査を持つと、片方だけ `\u{}` 表記を
    // 取りこぼすような食い違いが起きる
    // **説明文で緑にならない**よう、コードだけの眺めで見る
    let code_of = |rel: &str| {
        let src = std::fs::read_to_string(emoji_scan::repo_root().join(rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}"));
        production_range::code_view::code_view(&src)
    };
    assert!(
        code_of("crates/tako-control/tests/common/emoji_scan.rs")
            .contains("tako_core::emoji::is_emoji"),
        "共有の走査が判定の 1 実装を呼んでいない（コメントではなくコードで）"
    );
    for watchdog in [
        "crates/tako-control/tests/issue1536_no_emoji_ui_watchdog.rs",
        "crates/tako-control/tests/issue1578_no_emoji_cli_watchdog.rs",
    ] {
        // `#[path = "…"]` は文字列リテラルなので code_view では消える。
        // モジュール宣言そのもの（コード）を見る
        assert!(
            code_of(watchdog).contains("mod emoji_scan"),
            "{watchdog} が共有の走査を使っていない（写しを持つと範囲がずれる）"
        );
    }
}

/// 置き換え先の文字ラベルが**その場の思いつき**にならないよう、
/// `setup.rs` が既に使っている語彙と同じであることを固定する（#1578）
#[test]
fn 文字ラベルはsetup_rsの語彙と同じ() {
    let setup =
        std::fs::read_to_string(emoji_scan::repo_root().join("crates/tako-cli/src/setup.rs"))
            .expect("setup.rs");
    let main = std::fs::read_to_string(emoji_scan::repo_root().join("crates/tako-cli/src/main.rs"))
        .expect("main.rs");
    for label in ["[OK]", "[情報]", "[警告]", "[不足]", "[任意]", "[失敗]"] {
        assert!(
            setup.contains(label),
            "{label} が setup.rs に無い（語彙の正本から外れている）"
        );
    }
    // #1578 で置き換えた先が実際に main.rs へ入っていること
    for placed in [
        "[情報] {notice}",
        "[OK] フルディスクアクセス: 付与済み",
        "[任意] フルディスクアクセス: 未付与",
        "[OK] エージェント共通ルール同期: 最新",
        "[不足] 正本ファイルが見つかりません",
    ] {
        assert!(main.contains(placed), "main.rs に {placed:?} が無い");
    }
}
