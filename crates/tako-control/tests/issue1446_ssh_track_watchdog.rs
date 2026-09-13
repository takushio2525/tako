//! SSH ペインの「自動再接続の対象だ」という記憶が消えないことの番犬（#1446）
//!
//! # なぜ要るか
//!
//! #1040 の自動再接続は正しく判断していたが、**判断の入口に立てるペインが限られて
//! いた**。追跡の器 `ssh_connect: HashMap<PaneId, SshConnect>` はメモリだけに在り、
//! 作る口は `begin_ssh_connect` 1 つ、その唯一の呼び出し元は dispatch の
//! `Request::OpenRemote` だった。したがって:
//!
//! - **GUI を再起動したペイン**（器つきなので ssh は生き続ける）は追跡が消え、
//!   その後に切れても誰も見ていない。実測: 再起動後の硬切断で 32 秒 `ssh_connect=null`・
//!   persist.log に検知行 0 行・画面は `ssh exit 255` →「ローカルのシェルに戻ります」
//!   のまま（= 実機報告そのもの）
//! - **ユーザーが手で `ssh <host>` と打ったペイン**は最初から追跡されない。
//!   実測: 接続成立後の硬切断で 24 秒 `null`・無言でローカルへ
//!
//! # 何を縛るか
//!
//! 1. 追跡を作る口が 1 実装（`track_ssh_connect`）で、入口 3 つがそこを通る
//! 2. `layout.json` へ落ちる（`PaneMetaRef.ssh`）・**接続実績のあるペインだけ**
//! 3. 復元が引き継ぐ（`restore_ssh_connect`）・**器が生きていたときだけ**
//! 4. #976 の検知が見つけた未追跡ペインを引き取る（`adopt_ssh_connect`）
//! 5. 復元・引き取りは見張りの起点を取り直す（`needs_rebase`）
//! 6. 切断を拾って撃たないときに**無言で終わらない**（通知 + persist.log）
//! 7. `tako list` / `read` に「再接続の対象か」が載る
//! 8. A/B は `TAKO_1446_LEGACY` の 1 つに閉じ、`TAKO_1040_LEGACY` と独立
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜8 はすべて無意味に緑になるので、[`走査が空振りしていない`]
//! で窓が採れていることを固定し、[`逆戻りを名指しできる`] で**修正前を再現した
//! 6 通りの注入**が file:line で名指しされることを確かめる。範囲取りは #1420 の
//! 1 実装を通す（`main.rs` はテストが厚く、雑に切るとセルフテスト側の文字列を
//! 本番コードと数えてしまう）。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const MAIN: &str = "crates/tako-app/src/main.rs";
const FOLDERS: &str = "crates/tako-app/src/ssh_folders.rs";
const LAYOUT: &str = "crates/tako-control/src/layout.rs";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 1 つの違反（`ファイル:行 — 理由`）
#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。
///
/// 終わりは**宣言行と同じ字下げの `}`**。「宣言行から N 行」で切ると隣の関数の
/// 実装を自分のものと数える（#1417 が踏んだ形）
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 注釈（`//` で始まる行）を落とした窓。検査は**コードだけ**を見る
/// （アンチパターンを説明した注釈で落ちると、理由を書くほど落ちることになる）
fn code_only(window: &str) -> String {
    window
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の**コードの**行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

/// 追跡の作り方（`main.rs`）の検査
fn scan_main(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: MAIN,
            line,
            why,
        })
    };

    // 1. 追跡を作る口は 1 実装
    match fn_window(src, "fn track_ssh_connect(") {
        None => push(
            0,
            "`track_ssh_connect` が無い（追跡を作る 1 実装が消えている。#1446）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("ConnectPhase::Connected") {
                push(
                    at,
                    "復元・引き取りが見張り（`Connected`）から始まっていない。\
                     `Connecting` から入ると相手が黙っている限り 120 秒で畳まれ、\
                     追跡がまた消える（#1446 を作り直す）"
                        .into(),
                );
            }
            if !code.contains("ever_connected: restored_or_adopted") {
                push(
                    at,
                    "復元・引き取りで接続の証拠を引き継いでいない（`should_arm` が \
                     常に false = 繋ぎ直さない。#1446）"
                        .into(),
                );
            }
            if !code.contains("needs_rebase") {
                push(
                    at,
                    "見張りの起点を取り直す印を持っていない（画面に残った古い切断マーカーを \
                     もう一度「いま切れた」と読む。#1040 が実装中に踏んだ形）"
                        .into(),
                );
            }
        }
    }

    // 2. 入口 3 つがその 1 実装を通る
    for (needle, why) in [
        (
            "fn begin_ssh_connect(",
            "dispatch 経由の入口が `track_ssh_connect` を通っていない",
        ),
        (
            "fn restore_ssh_connect(",
            "復元の入口が `track_ssh_connect` を通っていない",
        ),
        (
            "fn adopt_ssh_connect(",
            "引き取りの入口が `track_ssh_connect` を通っていない",
        ),
    ] {
        match fn_window(src, needle) {
            None => push(0, format!("{needle} が無い（#1446 の入口が欠けている）")),
            Some((at, window)) => {
                if !code_only(&window).contains("self.track_ssh_connect(") {
                    push(at, why.into());
                }
            }
        }
    }

    // 2b. 諦めた / 失敗の表示で止まっているペインは引き取り直す。
    //     実測で見つけた穴: 上限まで撃って `gave_up` になったペインを案内どおり手で
    //     繋ぎ直しても、引き取りが「既に追跡している」で降りるため表示が `gave_up` の
    //     ままになり、**もう一度切れても撃たない**ペインが残る
    if let Some((at, window)) = fn_window(src, "fn adopt_ssh_connect(") {
        let code = code_only(&window);
        if !code.contains("ConnectPhase::GaveUp") {
            push(
                at,
                "諦めた表示のペインを引き取り直していない（案内どおり手で繋ぎ直しても                  見張りが再開せず、次の切断で撃たない。#1446）"
                    .into(),
            );
        }
    }

    // 3. 保存されるのは接続実績のあるペインだけ
    match fn_window(src, "fn ssh_layout_in(") {
        None => push(
            0,
            "`ssh_layout_in` が無い（`layout.json` へ落とす材料の 1 実装が消えている）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("ever_connected") {
                push(
                    at,
                    "接続実績を見ずに保存している（届かない相手で開いただけのペインを \
                     再起動後に叩き始める）"
                        .into(),
                );
            }
            if !code.contains("legacy_1446()") {
                push(at, "A/B の腕（`TAKO_1446_LEGACY`）を通っていない".into());
            }
        }
    }
    if !src.contains("ssh: Self::ssh_layout_in(ssh_tracked, pane),") {
        push(
            fn_window(src, "fn save_layout(")
                .map(|(at, _)| at)
                .unwrap_or(0),
            "`save_layout` が SSH の追跡を保存していない（`PaneMetaRef.ssh` が埋まらない = \
             GUI 再起動で追跡が消える。#1446 の真因）"
                .into(),
        );
    }

    // 4. 復元は器が生きていたときだけ引き継ぐ
    match src
        .lines()
        .position(|l| l.contains("app.restore_ssh_connect(pane, ssh);"))
    {
        None => push(
            0,
            "復元ループが `restore_ssh_connect` を呼んでいない（GUI 再起動をまたいだ \
             ペインの追跡が戻らない。#1446 の真因）"
                .into(),
        ),
        Some(i) => {
            let guard = src.lines().nth(i.saturating_sub(1)).unwrap_or("");
            if !guard.contains("backend_alive") {
                push(
                    i + 1,
                    "器の生存を見ずに引き継いでいる（器ごと消えたペインは新しいシェルが \
                     開くので、見張る相手が居ない）"
                        .into(),
                );
            }
        }
    }

    // 5. 見張りの起点を取り直す
    match fn_window(src, "fn drive_ssh_connect(") {
        None => push(
            0,
            "`drive_ssh_connect` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("st.needs_rebase") {
                push(
                    at,
                    "復元・引き取りのエントリで起点を取り直していない（画面に残った \
                     古い切断マーカーを読んで即座に繋ぎ直しへ入る）"
                        .into(),
                );
            }
            // 6. 撃たないときに無言で終わらない
            if !code.contains("notify_ui_failure(") {
                push(
                    at + code_line_with(&window, "ConnectPhase::Failed { reason }").unwrap_or(0),
                    "切断を拾って撃たないときに画面へ何も出していない（#1446 の報告は \
                     まさに「リトライも諦めの案内も出ない」だった）"
                        .into(),
                );
            }
            if !code.contains("reconnect::arm_state(") {
                push(
                    at,
                    "撃つ / 撃たないを `bool` だけで決めている（**なぜ撃たないか**を \
                     画面にも `tako list` にも出せない）"
                        .into(),
                );
            }
        }
    }

    // 7. `tako list` / `read` に対象かどうかが載る
    match fn_window(src, "fn ssh_connect_state(") {
        None => push(
            0,
            "`ssh_connect_state` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("\"reconnect\"") {
                push(
                    at,
                    "応答に再接続の対象かどうかが載っていない（切れる前に \
                     「守られているか」を読めない。#1446 受け入れ条件）"
                        .into(),
                );
            }
            // 繋がっているペインでこそ知りたい値なので、`Connected` で捨てない
            if let Some(i) = code_line_with(&window, "ConnectPhase::Connected) {") {
                let tail = window
                    .lines()
                    .skip(i)
                    .take(8)
                    .collect::<Vec<_>>()
                    .join("\n");
                if !tail.contains("reconnect") {
                    push(
                        at + i,
                        "`Connected` のペインで応答を捨てている（普通に繋がっている間は \
                         対象かどうかが読めない = #1446 の報告が遅れた原因）"
                            .into(),
                    );
                }
            }
        }
    }
    out
}

/// 引き取り（`ssh_folders.rs`）の検査
fn scan_folders(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "pub(crate) fn apply_ssh_scan(") {
        None => out.push(Offender {
            file: FOLDERS,
            line: 0,
            why: "`apply_ssh_scan` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if !code_only(&window).contains("self.adopt_ssh_connect(") {
                out.push(Offender {
                    file: FOLDERS,
                    line: at,
                    why: "#976 の検知が見つけた未追跡ペインを引き取っていない（手で \
                          `ssh <host>` と打ったペインは切断しても無言でローカルへ落ちる。\
                          #1446 の仮説 2 = 実測で再現済み）"
                        .into(),
                });
            }
        }
    }
    out
}

/// 永続（`layout.rs`）の検査
fn scan_layout(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: LAYOUT,
            line,
            why,
        })
    };
    if !src.contains("pub struct SshPaneLayout") {
        push(
            0,
            "`SshPaneLayout` が無い（`layout.json` に SSH の追跡が載らない = #1446 の真因）".into(),
        );
    }
    match src
        .lines()
        .position(|l| l.trim() == "pub ssh: Option<SshPaneLayout>,")
    {
        None => push(0, "`PaneLayout.ssh` が無い".into()),
        Some(i) => {
            // 旧ファイルが読めること（移行 Step 無しで済む根拠）は serde の default
            let attrs = src
                .lines()
                .skip(i.saturating_sub(3))
                .take(3)
                .collect::<Vec<_>>()
                .join("\n");
            if !attrs.contains("serde(default") {
                push(
                    i + 1,
                    "`serde(default)` が無い（`ssh` を知らない旧 layout.json が読めなくなる。\
                     読めないなら移行 Step が要る = #916）"
                        .into(),
                );
            }
        }
    }
    if !src.contains("(\"PaneLayout\", \"ssh\"),") {
        push(
            0,
            "変化検出キーの宣言に `PaneLayout.ssh` が無い（#1425: 追跡が変わっても \
             layout.json が書かれない = 保存されないフィールドになる）"
                .into(),
        );
    }
    out
}

fn all(main: &str, folders: &str, layout: &str) -> Vec<String> {
    scan_main(main)
        .iter()
        .chain(scan_folders(folders).iter())
        .chain(scan_layout(layout).iter())
        .map(Offender::report)
        .collect()
}

/// 本番コードだけの眺め（#1420 の 1 実装）
fn sources(root: &Path) -> (String, String, String) {
    (
        production_range::production(&read(root, MAIN), MAIN),
        production_range::production(&read(root, FOLDERS), FOLDERS),
        production_range::production(&read(root, LAYOUT), LAYOUT),
    )
}

#[test]
fn ssh追跡はプロセスの寿命を越えて残る() {
    let root = workspace_root();
    let (main, folders, layout) = sources(&root);
    let offenders = all(&main, &folders, &layout);
    assert!(
        offenders.is_empty(),
        "SSH ペインの自動再接続が発火しない形へ戻っている（#1446）:\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let (main, folders, layout) = sources(&root);
    for needle in [
        "fn track_ssh_connect(",
        "fn begin_ssh_connect(",
        "fn restore_ssh_connect(",
        "fn adopt_ssh_connect(",
        "fn ssh_layout_in(",
        "fn drive_ssh_connect(",
        "fn ssh_connect_state(",
    ] {
        let (at, window) = fn_window(&main, needle)
            .unwrap_or_else(|| panic!("{MAIN} の {needle} の窓が採れない（走査が空振り）"));
        assert!(at > 0, "{MAIN} の {needle} の行番号が採れていない");
        assert!(
            window.lines().count() > 2,
            "{MAIN} の {needle} の窓が 2 行以下（字下げの規約が変わって窓が切れている）"
        );
    }
    let (at, window) =
        fn_window(&folders, "pub(crate) fn apply_ssh_scan(").expect("apply_ssh_scan の窓");
    assert!(
        at > 0 && window.lines().count() > 10,
        "引き取り側の窓が小さい"
    );
    assert!(
        layout.contains("pub struct SshPaneLayout"),
        "永続構造体が本番コードの眺めに入っていない"
    );
}

/// A/B の腕が #1446 の 1 つに閉じ、#1040 と独立に効く
#[test]
fn abの腕は一つで独立している() {
    let root = workspace_root();
    let sidebar = production_range::production(
        &read(root.as_path(), "crates/tako-app/src/sidebar.rs"),
        "crates/tako-app/src/sidebar.rs",
    );
    let (main, folders, _) = sources(&root);
    assert!(
        sidebar.contains("TAKO_1446_LEGACY"),
        "A/B の腕（`TAKO_1446_LEGACY`）が消えている"
    );
    // env の直読みは 1 箇所（`legacy_1446`）だけ。散らばると腕が揃わない
    // （注釈で名前を挙げるのは自由なので、数えるのは**コードの行だけ**）
    let reads: usize = [&sidebar, &main, &folders]
        .iter()
        .map(|s| code_only(s).matches("TAKO_1446_LEGACY").count())
        .sum();
    assert_eq!(
        reads, 1,
        "`TAKO_1446_LEGACY` を読む場所が 1 箇所でない（腕が揃わなくなる）"
    );
    // #1040 の腕と混ざっていない（「記憶が残るか」と「残った記憶で撃つか」を別に倒せる）
    let ab = fn_window(&sidebar, "fn legacy_1446()")
        .expect("legacy_1446 の窓")
        .1;
    assert!(
        !ab.contains("TAKO_1040_LEGACY"),
        "#1446 の腕が #1040 の腕を巻き込んでいる（片方がもう片方の回帰を隠す）"
    );
    let ab1040 = fn_window(&main, "fn ssh_reconnect_enabled()")
        .expect("ssh_reconnect_enabled の窓")
        .1;
    assert!(
        !ab1040.contains("TAKO_1446_LEGACY"),
        "#1040 の腕が #1446 の腕を巻き込んでいる"
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let (main, folders, layout) = sources(&root);

    // 注入 1: 永続を落とす（#1446 の真因そのもの = メモリだけの追跡へ戻す）
    let no_persist = main.replace(
        "            ssh: Self::ssh_layout_in(ssh_tracked, pane),\n",
        "",
    );
    assert!(no_persist != main, "注入 1 の対象が見つからない");
    let found = all(&no_persist, &folders, &layout);
    assert!(
        found.iter().any(|o| o.contains(MAIN)),
        "永続を落とした形を名指しできていない: {found:?}"
    );

    // 注入 2: 復元時の引き継ぎを落とす（GUI 再起動で追跡が消える）
    let no_restore = main.replace("app.restore_ssh_connect(pane, ssh);", "let _ = ssh;");
    assert!(no_restore != main, "注入 2 の対象が見つからない");
    let found = all(&no_restore, &folders, &layout);
    assert!(
        found.iter().any(|o| o.contains(MAIN)),
        "復元の引き継ぎを落とした形を名指しできていない: {found:?}"
    );

    // 注入 3: 器の生存を見ずに引き継ぐ（消えた器のペインまで見張る）
    let blind_restore = main.replace(
        "                if let (Some(ssh), true) = (&r.ssh, backend_alive) {",
        "                if let Some(ssh) = &r.ssh {",
    );
    assert!(blind_restore != main, "注入 3 の対象が見つからない");
    let want = blind_restore
        .lines()
        .position(|l| l.contains("app.restore_ssh_connect(pane, ssh);"))
        .expect("注入した行がある")
        + 1;
    let found = all(&blind_restore, &folders, &layout);
    assert!(
        found.iter().any(|o| o.contains(&format!("{MAIN}:{want}"))),
        "器の生存を見ない形を名指しできていない: {found:?}"
    );

    // 注入 4: 引き取りを落とす（手打ちの ssh が対象外に戻る）
    let no_adopt = folders.replace(
        "            self.adopt_ssh_connect(PaneId::from_raw(session.pane), &session.destination);",
        "",
    );
    assert!(no_adopt != folders, "注入 4 の対象が見つからない");
    let found = all(&main, &no_adopt, &layout);
    assert!(
        found.iter().any(|o| o.contains(FOLDERS)),
        "引き取りを落とした形を名指しできていない: {found:?}"
    );

    // 注入 5: 撃たないときを無言へ戻す（#1446 の報告の見え方）
    let silent = main.replace(
        "self.notify_ui_failure(\n",
        "#[allow(unused)]\n                        let _silent = (\n",
    );
    assert!(silent != main, "注入 5 の対象が見つからない");
    let found = all(&silent, &folders, &layout);
    assert!(
        found.iter().any(|o| o.contains(MAIN)),
        "無言へ戻した形を名指しできていない: {found:?}"
    );

    // 注入 6: 応答から「対象か」を落とす（切れる前に読めない形へ戻す）
    let no_field = main.replace("\"reconnect\"", "\"_reconnect_dropped\"");
    assert!(no_field != main, "注入 6 の対象が見つからない");
    let found = all(&no_field, &folders, &layout);
    assert!(
        found.iter().any(|o| o.contains(MAIN)),
        "応答から対象かどうかを落とした形を名指しできていない: {found:?}"
    );

    // 注入 7: 諦めたペインの引き取り直しを落とす（手で繋ぎ直しても見張りが再開しない）
    let no_regain = main.replace("ConnectPhase::Failed { .. } | ConnectPhase::GaveUp", "");
    assert!(no_regain != main, "注入 7 の対象が見つからない");
    let found = all(&no_regain, &folders, &layout);
    assert!(
        found.iter().any(|o| o.contains(MAIN)),
        "引き取り直しを落とした形を名指しできていない: {found:?}"
    );

    // 注入 8: 永続を serde default 無しにする（旧ファイルが読めなくなる = 移行漏れ）
    let no_default = layout.replace(
        "    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n    pub ssh: Option<SshPaneLayout>,",
        "    pub ssh: Option<SshPaneLayout>,",
    );
    assert!(no_default != layout, "注入 8 の対象が見つからない");
    let want = no_default
        .lines()
        .position(|l| l.trim() == "pub ssh: Option<SshPaneLayout>,")
        .expect("注入した行がある")
        + 1;
    let found = all(&main, &folders, &no_default);
    assert!(
        found
            .iter()
            .any(|o| o.contains(&format!("{LAYOUT}:{want}"))),
        "旧ファイルが読めなくなる形を名指しできていない: {found:?}"
    );
}
