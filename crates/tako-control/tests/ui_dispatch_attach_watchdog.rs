//! UI から dispatch を直接呼ぶ経路が PTY 起動を消化しているかの番犬（#1023）
//!
//! `tako_control::dispatch` はペインを作るところまでしかやらない。PTY の起動には
//! GPUI の `Context` が要るので `SessionHost::attach_session` は `pending_attach` へ
//! 積むだけで、**呼び出し側が消化しないとターミナルが永久に立たない**。
//! IPC / MCP のリクエストループは毎回消化しているが、UI 経路は呼び出しごとに
//! 手で書く形だったため `open_ssh_host` で抜け、「ファイル→リモート接続で出した
//! ペインはターミナルが出るまでめっちゃ待つ」（#1023）になっていた。
//!
//! 見た目には「ペインは出ている」ので壊れて見えず、しかも **CLI / MCP で覗くと
//! その観測自身が消化してしまう**（IPC ループが消化する）ので、手で試すと直って
//! 見える。だから機械検査で止める。
//!
//! 対象は tako-app の UI モジュール（イベントハンドラだけが入っているファイル）。
//! `main.rs` は production と隔離セルフテスト / visual-test が同居しており、
//! 後者は意図的に消化しない（コマンドの中身だけを見る項目がある）ため、
//! ソース走査では両者を区別できない。main.rs 側の同じ不変条件は
//! **隔離セルフテスト項目 132**（`open_ssh_host` の直後に `pending_attach` が空）が守る。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// PTY 起動を伴う（= `attach_session` を呼ぶ）Request の名前。
/// `dispatch.rs` 側で増えたらこの表も足すこと（増減は下のテストが検知する）
const ATTACHING: &[&str] = &[
    "OpenDir",
    "OpenRemote",
    "Split",
    "TabNew",
    "TmuxOpen",
    "Welcome",
    "WindowNew",
];

/// `RemoteFolder` の action のうち、内部で `OpenRemote` へ委譲するもの。
/// #1041 で `open` も（フォルダを開いたらターミナルも繋ぐので）委譲するようになった
const ATTACHING_REMOTE_ACTIONS: &[&str] = &["ssh-pane", "open"];

/// 消化したと認めるしるし
const DRAIN_MARKS: &[&str] = &["pending_attach", "attach_pending_sessions"];

fn ui_modules(root: &Path) -> Vec<PathBuf> {
    let dir = root.join("crates/tako-app/src");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .expect("tako-app/src が読める")
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // main.rs は自己検証コードが同居するので除外（理由はモジュール doc）
        if name == "main.rs" {
            continue;
        }
        out.push(path);
    }
    out.sort();
    out
}

/// dispatch 呼び出しの位置から「近傍」（前 30 行 / 後 80 行）を切り出す。
///
/// **コメント行は落とす**のが要点: 近くに「pending_attach の後処理は不要」のような
/// 説明があるだけで消化したと認めてしまうと番犬が空振りする
/// （実際に `sidebar.rs` でこの空振りを踏んだ）
fn neighborhood(lines: &[&str], at: usize) -> String {
    let from = at.saturating_sub(30);
    let to = (at + 80).min(lines.len());
    lines[from..to]
        .iter()
        .filter(|l| !l.trim_start().starts_with("//"))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ui経路のdispatchはpty起動を消化している() {
    let root = workspace_root();
    let mut offenders = Vec::new();
    for path in ui_modules(&root) {
        let src = std::fs::read_to_string(&path).expect("UI モジュールが読める");
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.contains("tako_control::dispatch(") {
                continue;
            }
            // 呼び出しの直後にある Request 名を拾う
            let to = (i + 35).min(lines.len());
            let call = lines[i..to].join("\n");
            let Some(variant) = call
                .split("Request::")
                .nth(1)
                .map(|rest| {
                    rest.chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect::<String>()
                })
                .filter(|v| !v.is_empty())
            else {
                continue;
            };
            let attaching = ATTACHING.contains(&variant.as_str())
                || (variant == "RemoteFolder"
                    && ATTACHING_REMOTE_ACTIONS
                        .iter()
                        .any(|a| call.contains(&format!("action: \"{a}\""))));
            if !attaching {
                continue;
            }
            let around = neighborhood(&lines, i);
            if !DRAIN_MARKS.iter().any(|m| around.contains(m)) {
                let rel = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                offenders.push(format!("{rel}:{} Request::{variant}", i + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "UI から dispatch を直接呼ぶとき、PTY 起動（pending_attach）を消化していない \
         箇所がある（#1023。`self.attach_pending_sessions(cx)` を呼ぶこと）:\n  {}",
        offenders.join("\n  ")
    );
}

/// PTY 起動を伴う Request の表（[`ATTACHING`]）が古びていないか。
///
/// 「どの Request が `attach_session` を呼ぶか」をソースから機械的に割り出すのは
/// できない: `Request::X { .. }` は **match のアーム**と**呼び出しのための組み立て**で
/// 見分けが付かず、しかもアームが**補助関数へ委譲**してその中で起動を頼む形もある
/// （`Welcome` がそれ）。誤検知で番犬が落ちると本来の検査まで無視されるので、
/// ここは 2 つの弱い（が確実な）不変条件で守る。
#[test]
fn pty起動を伴うrequestの表が古びていない() {
    let root = workspace_root();
    let src = std::fs::read_to_string(root.join("crates/tako-control/src/dispatch.rs"))
        .expect("dispatch.rs が読める");

    // ① 起動を頼む箇所の数を固定する。増減したら表を見直すきっかけになる
    //    （新しい Request が PTY を作るようになったのに表へ足し忘れる、を止める）
    let sites = src.matches("attach_session(").count();
    assert_eq!(
        sites, 12,
        "`attach_session(` の箇所が変わった（#1023）。増えたのが新しい Request なら \
         番犬の ATTACHING 表へ足し、UI 経路が `attach_pending_sessions` を呼んでいるか \
         確かめること"
    );

    // ② 表に載っている名前が実在のアームであること（消えた Request が残らない）
    for name in ATTACHING {
        assert!(
            src.contains(&format!("        Request::{name} "))
                || src.contains(&format!("        Request::{name} {{"))
                || src.contains(&format!("        Request::{name} =>")),
            "ATTACHING に載っている Request::{name} が dispatch に無い（#1023）"
        );
    }
}
