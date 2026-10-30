//! 実経路: OS のシェルに委ねるファイル操作（Finder で表示・既定アプリ / 指定のアプリで開く・ゴミ箱へ
//! 移す・まとめてゴミ箱へ）と System Settings の Battery を開く口の断りが、枠と中身ごと表示言語に従う
//! （Issue #2365 の続き）。
//!
//! 修正前（実測。CLI / MCP と同じ dispatch を GUI の代わりのホストへ通して）: 英語表示でも
//! 「デフォルトアプリで開けない: …」「ゴミ箱への移動に失敗: …」「記録を書けない（…）: …」
//! 「System Settings を開けません: URL を開けない: …」のように日本語のまま = dispatch が
//! `platform::os_integration` / `platform::finder_reveal` / `sleep_guard` の `Err(String)`
//! （日本語の固定文）を素通ししていた。
//!
//! どのケースも**注入した失敗**で断られて何も起こさない:
//!
//! - `PATH` を空の dir へ向ける = `open`（既定アプリ / 指定のアプリ / URL）を起こせない
//!   （最初の 1 件で差し替えが効いたことを確かめてから先へ進む。効いていなければ何も叩かずに止まる）
//! - `TAKO_REVEAL_RECORD` を無い dir の下へ向ける = Finder へ頼む直前の記録を書けない
//!   （Finder は開かない = #2247 の検証の口）
//! - `TAKO_TRASH_DIR` を無い dir へ向ける = 本物のゴミ箱の代わりの置き場へ移せない
//!   （ユーザーのゴミ箱へ入れない = #1867 の検証の口）
//!
//! 蓋閉じ継続の解除（`remove-lid-sleep`）は叩かない: macOS では sudoers が登録済みなら管理者権限の
//! 確認を出す経路で、結果がこの機械の状態で変わる（文の日英は番犬 `issue1762_dispatch_error_lang.rs` の表）。
//!
//! 見るもの:
//!
//! - 英語表示: 断りに日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//!
//! 表示言語と env はプロセス全体の状態なので、**1 本のテスト関数を `run_alone!` の子**で走らせる
//! （#2400）。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）。macOS だけ: 失敗を注入する口（`open` を PATH で引く・Finder の記録）が macOS の実装
//! （Windows は explorer.exe / ShellExecuteW を PATH に依らず起こすので、同じ注入で止められない）
#![cfg(target_os = "macos")]

use std::path::Path;

use serde_json::{json, Value};
use tako_control::protocol::Request;
use tako_control::{
    DispatchError, PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost,
    WebViewHost, WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木だけを持つ（どの要求もペインを作らない）
struct Host {
    workspace: Workspace,
}

impl WorkspaceHost for Host {
    fn workspace(&self) -> &Workspace {
        &self.workspace
    }
    fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }
}

impl SessionHost for Host {
    fn session(&self, _pane: PaneId) -> Option<&TerminalSession> {
        None
    }
    fn attach_session(&mut self, _pane: PaneId, _options: SpawnOptions) {}
    fn detach_session(
        &mut self,
        _pane: PaneId,
        _origin: tako_core::pane_log::CloseOrigin,
        _caller: Option<&str>,
    ) {
    }
}

impl UiStateHost for Host {}
impl PreviewHost for Host {}
impl TmuxHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}
impl SystemHost for Host {}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// `open` を起こせないときの OS のエラー文（ENOENT）
const ENOENT: &str = "No such file or directory (os error 2)";

struct Case {
    name: &'static str,
    request: Value,
    /// #2365 より前の日本語の断り（`<root>` は一時の置き場）
    old_ja: String,
}

fn case(name: &'static str, request: Value, old_ja: impl Into<String>) -> Case {
    Case {
        name,
        request,
        old_ja: old_ja.into(),
    }
}

/// 一時の置き場の中身: 開く・移す対象のファイル 2 つと、空の PATH の dir
fn make_fixture(root: &Path) {
    std::fs::create_dir_all(root.join("empty-path")).expect("空の PATH の dir");
    std::fs::create_dir_all(root.join("home")).expect("HOME");
    std::fs::create_dir_all(root.join("data")).expect("data dir");
    std::fs::write(root.join("a.txt"), "a").expect("fixture a");
    std::fs::write(root.join("b.txt"), "b").expect("fixture b");
}

fn cases(root: &Path) -> Vec<Case> {
    let a = root.join("a.txt").display().to_string();
    let b = root.join("b.txt").display().to_string();
    let record = root.join("no-dir").join("reveal.log").display().to_string();
    vec![
        case(
            "file reveal（Finder へ頼む直前の記録を書けない）",
            json!({ "method": "file_op", "params": { "op": "reveal", "path": a } }),
            format!("記録を書けない（{record}）: {ENOENT}"),
        ),
        case(
            "file open_default（open を起こせない）",
            json!({ "method": "file_op", "params": { "op": "open_default", "path": a } }),
            format!("デフォルトアプリで開けない: {ENOENT}"),
        ),
        case(
            "file open_with（open を起こせない）",
            json!({ "method": "file_op", "params": { "op": "open_with", "path": a, "name": "Preview" } }),
            format!("アプリ 'Preview' で開けない: {ENOENT}"),
        ),
        case(
            "file trash（置き場へ移せない）",
            json!({ "method": "file_op", "params": { "op": "trash", "path": a } }),
            format!("ゴミ箱への移動に失敗: {ENOENT}"),
        ),
        case(
            "file trash まとめて 2 件（全件移せない = 包みと中身）",
            json!({ "method": "file_op_many", "params": { "op": "trash", "paths": [a, b] } }),
            format!("2 件すべてできなかった: ゴミ箱への移動に失敗: {ENOENT} / ゴミ箱への移動に失敗: {ENOENT}"),
        ),
        case(
            "sleep_guard open-battery-settings（open を起こせない）",
            json!({ "method": "sleep_guard", "params": { "action": "open-battery-settings" } }),
            format!("System Settings を開けません: URL を開けない: {ENOENT}"),
        ),
        // 境界: 中身へ届く前に dispatch が断る（存在しないパス）。修正前から日英済み = 変わらない
        case(
            "file reveal（存在しないパス）",
            json!({ "method": "file_op", "params": { "op": "reveal", "path": root.join("nope.txt").display().to_string() } }),
            format!("パスが存在しない: {}", root.join("nope.txt").display()),
        ),
    ]
}

fn host() -> Host {
    Host {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
    }
}

fn run(request: &Value) -> String {
    let mut host = host();
    let request: Request = serde_json::from_value(request.clone()).expect("要求の形");
    let result: Result<Value, DispatchError> =
        tako_control::dispatch(&mut host, request, PaneOrigin::Cli);
    match result {
        Err(e) => e.to_string(),
        Ok(v) => format!("<断られずに通った: {v}>"),
    }
}

#[test]
fn osのシェルに委ねるファイル操作とbatteryの断りは表示言語に従う() {
    if tako_core::run_alone!(osのシェルに委ねるファイル操作とbatteryの断りは表示言語に従う)
    {
        return;
    }
    let root = std::env::temp_dir().join(format!("tako-2365o-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("一時の置き場");
    let root = root.canonicalize().expect("正規化");
    make_fixture(&root);
    // 本番の設定・データを読み書きしない（どの置き場も一時の場所の下）
    std::env::set_var("HOME", root.join("home"));
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    // 失敗の注入（上の doc）
    std::env::set_var("PATH", root.join("empty-path"));
    std::env::set_var("TAKO_REVEAL_RECORD", root.join("no-dir").join("reveal.log"));
    std::env::set_var("TAKO_TRASH_DIR", root.join("no-trash"));

    // 先に差し替えが効いたことを確かめる: 無いアプリ名で開く = `open` が見つかっても何も開かない形。
    // `open` を起こせた（= PATH の差し替えが効いていない）なら、本物の Finder / アプリ / システム設定を
    // 開きうるので先へ進まない
    let probe = run(&json!({ "method": "file_op", "params": {
        "op": "open_with",
        "path": root.join("a.txt").display().to_string(),
        "name": "tako-no-such-app-2365",
    } }));
    assert!(
        probe.contains(ENOENT),
        "PATH の差し替えが効いていない（`open` を起こせた）ので先へ進まない: {probe}"
    );

    let all = cases(&root);
    let original = i18n::lang();
    let mut by_lang = Vec::new();
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        by_lang.push((
            lang,
            all.iter().map(|c| run(&c.request)).collect::<Vec<_>>(),
        ));
    }
    i18n::set_lang(original);

    let mut failures = Vec::new();
    let mut en_japanese = 0;
    for (lang, seen) in &by_lang {
        for (text, c) in seen.iter().zip(&all) {
            println!("[{lang:?}] {}: {text}", c.name);
            match lang {
                Lang::Ja => {
                    if *text != c.old_ja {
                        failures.push(format!(
                            "[Ja] {}: #2365 より前の文と違う\n  want: {}\n  got:  {text}",
                            c.name, c.old_ja
                        ));
                    }
                }
                Lang::En => {
                    if let Some(ch) = has_japanese(text) {
                        en_japanese += 1;
                        failures.push(format!(
                            "[En] {}: 英語表示に日本語（{ch}）が混ざる: {text}",
                            c.name
                        ));
                    }
                }
            }
        }
    }
    // 何も動かしていない（移す・消すの注入が効いている）
    assert!(root.join("a.txt").exists() && root.join("b.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
    println!(
        "英語表示で日本語入り: {en_japanese} / {}（ケース数）",
        all.len()
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
