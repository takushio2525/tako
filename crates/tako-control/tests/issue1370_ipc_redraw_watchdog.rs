//! **#1370 の番犬**: IPC 由来のレイアウト変更のあと 1 フレーム描く。
//!
//! ## なぜ止めるのか
//!
//! ペインの cols / rows を PTY へ渡す唯一の書き手は**描画の中**（`render_pane` /
//! `sync_offscreen_pane_sizes`）にある。ところが macOS の gpui では `cx.notify()` は
//! dirty を立てるだけで、フレームを作る `on_request_frame` は CVDisplayLink /
//! `displayLayer:` / `windowDidBecomeKey` からしか呼ばれず、display link は窓が
//! `NSWindowOcclusionStateVisible` でないと起動しない。
//!
//! = **隠れた窓・最小化した窓・仮想ディスプレイ上の窓ではフレームが 1 枚も来ない**。
//! その状態で MCP / CLI から `resize` / `split` / `equalize` を撃つと、ツリーの取り分
//! （share）は即座に変わるのに、ペインの中のシェル・TUI は古い winsize のまま残る
//! （#1370 の実測: 30 秒 × 5 回で cols 不変・ペインの `stty size` が `21 21`）。
//!
//! ## 何を固定するか
//!
//! 挙動そのもの（本当に届くか）は隔離セルフテスト項目 22b が実測する。ここが止めるのは
//! **構造**で、1 行で戻せてしまう形がこれだけある:
//!
//! 1. [`ipcループはchanges_layoutで判定して1フレーム描く`] — 強制描画が外れる
//! 2. [`描画は応答を返す前に行う`] — 順序が入れ替わると `tako resize` が返った時点では
//!    まだ古く、呼び出し側が別途待つ羽目になる
//! 3. [`描く相手は全ビューポート`] — 2 枚目以降のウィンドウのペインだけ取り残される
//! 4. [`強制描画にabの口がある`] — 回帰を隠さない注入（`TAKO_1370_LEGACY`）が消える
//! 5. [`changes_layoutにワイルドカードが無い`] /
//!    [`changes_layoutは全バリアントをちょうど1回ずつ分類している`] —
//!    `_ => false` を置くと、増えた Request が黙って「描かない」側へ落ちる
//! 6. [`changes_layoutは読み取り系を真にしていない`] — ポーリングに #786 の描画固定費
//!    （実測 5.1M instr/frame）が乗る
//! 7. [`セルフテスト項目22bは描かずに待つ`] — 「待つ前に汚して描く」という他項目の
//!    作法をこの項目へも当てると、製品側の強制描画が無くても通る = 検出力が消える

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

const APP: &str = "crates/tako-app/src/main.rs";
const PROTOCOL: &str = "crates/tako-control/src/protocol.rs";

/// 1-origin の行番号（file:line で名指しするため）
fn line_of(src: &str, idx: usize) -> usize {
    src[..idx].matches('\n').count() + 1
}

/// 目印から目印までを切り出す。**見つからないことも FAILED**（走査範囲が空になると
/// どんな回帰でも通る番犬になってしまう）
fn region<'a>(src: &'a str, rel: &str, from: &str, to: &str) -> (&'a str, usize) {
    let start = src
        .find(from)
        .unwrap_or_else(|| panic!("{rel}: 目印 {from:?} が消えている（走査範囲を作れない）"));
    let rest = &src[start..];
    let end = rest
        .find(to)
        .unwrap_or_else(|| panic!("{rel}: 目印 {to:?} が消えている（走査範囲を作れない）"));
    (&rest[..end + to.len()], line_of(src, start))
}

/// 行コメントを落とす。番犬の doc / 注記は「呼んでいない」と書くために名前を含むので、
/// **実行されるコードだけ**を走査対象にする
fn code_only(src: &str) -> String {
    src.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 本番コード（最初の `#[cfg(test)]` より前）だけを返す
fn production(rel: &str) -> String {
    let src = read(rel);
    let cut = src.find("\n#[cfg(test)]").unwrap_or(src.len());
    src[..cut].to_string()
}

/// IPC 受信ループの「dispatch → 応答」区間
fn ipc_loop() -> (String, usize) {
    let src = read(APP);
    let (body, line) = region(
        &src,
        APP,
        "let needs_frame = !TakoApp::ipc_redraw_legacy()",
        "Err(_) => break, // View",
    );
    (code_only(body), line)
}

/// `changes_layout` の本体
fn changes_layout_body() -> (String, usize) {
    let src = production(PROTOCOL);
    let (body, line) = region(
        &src,
        PROTOCOL,
        "pub fn changes_layout(request: &Request) -> bool {",
        "\n}\n",
    );
    (code_only(body), line)
}

#[test]
fn ipcループはchanges_layoutで判定して1フレーム描く() {
    let (body, line) = ipc_loop();
    assert!(
        body.contains("tako_control::protocol::changes_layout(&incoming.request)"),
        "{APP}:{line} IPC ループが changes_layout で判定していない\
         （全 dispatch が通る 1 箇所でレイアウト変更だけを選り分ける形が崩れた）"
    );
    assert!(
        body.contains("window.draw(cx).clear()"),
        "{APP}:{line} IPC ループが 1 フレームを描いていない\
         （notify だけでは macOS でフレームが作られない = #1370 が戻る）"
    );
}

#[test]
fn 描画は応答を返す前に行う() {
    let (body, line) = ipc_loop();
    let draw = body
        .find("window.draw(cx).clear()")
        .unwrap_or_else(|| panic!("{APP}:{line} 強制描画が消えている"));
    let reply = body
        .find("incoming.reply.send(result)")
        .unwrap_or_else(|| panic!("{APP}:{line} 応答の送出が見つからない"));
    assert!(
        draw < reply,
        "{APP}:{line} 応答を返してから描いている。\
         `tako resize` が返った時点で PTY のサイズが新しいことが #1370 の受け入れ条件なので、\
         描画は必ず reply の前に置く"
    );
}

#[test]
fn 描く相手は全ビューポート() {
    let (body, line) = ipc_loop();
    assert!(
        body.contains("app.viewports.iter().map(|(_, h)| *h).collect()"),
        "{APP}:{line} 描く相手をアクティブウィンドウ等に絞っている。\
         同一 entity を root view にした全ビューポート（#339）を描かないと、\
         2 枚目以降のウィンドウのペインだけ古い winsize で取り残される"
    );
}

#[test]
fn 強制描画にabの口がある() {
    let src = read(APP);
    assert!(
        src.contains("fn ipc_redraw_legacy() -> bool {") && src.contains("TAKO_1370_LEGACY"),
        "{APP}: A/B の口（TAKO_1370_LEGACY）が消えている。\
         同一バイナリで旧挙動を再現できないと、セルフテスト項目 22b の検出力を実証できない"
    );
    let (body, line) = ipc_loop();
    assert!(
        body.contains("!TakoApp::ipc_redraw_legacy()"),
        "{APP}:{line} 強制描画が A/B の口を通っていない"
    );
}

#[test]
fn changes_layoutにワイルドカードが無い() {
    let (body, line) = changes_layout_body();
    for bad in ["_ =>", "_ if", "Request::_"] {
        assert!(
            !body.contains(bad),
            "{PROTOCOL}:{line} changes_layout に {bad:?} がある。\
             ワイルドカードを置くと、増えた Request が黙って片側へ落ちて \
             #1370 がその操作で再発する（exhaustive match が唯一の拘束）"
        );
    }
}

/// enum の宣言から拾ったバリアント名。`match` の分類と突き合わせる
fn request_variants() -> BTreeSet<String> {
    let src = read(PROTOCOL);
    let (body, _) = region(&src, PROTOCOL, "pub enum Request {", "\n}\n");
    let mut out = BTreeSet::new();
    for line in body.lines() {
        let t = line.trim_start();
        // インデント 4 = enum 直下のバリアント（ネストした構造体フィールドは 8 以上）
        if line.len() - t.len() != 4 {
            continue;
        }
        let name: String = t.chars().take_while(|c| c.is_alphanumeric()).collect();
        if name.chars().next().is_some_and(|c| c.is_uppercase()) {
            out.insert(name);
        }
    }
    assert!(
        out.len() > 100,
        "{PROTOCOL}: Request のバリアントを拾えていない（{} 件）。走査が壊れている",
        out.len()
    );
    out
}

#[test]
fn changes_layoutは全バリアントをちょうど1回ずつ分類している() {
    let (body, line) = changes_layout_body();
    let mut seen: Vec<String> = Vec::new();
    for (i, _) in body.match_indices("Request::") {
        let rest = &body[i + "Request::".len()..];
        let name: String = rest.chars().take_while(|c| c.is_alphanumeric()).collect();
        if !name.is_empty() {
            seen.push(name);
        }
    }
    let declared = request_variants();
    let classified: BTreeSet<String> = seen.iter().cloned().collect();
    let missing: Vec<&String> = declared.difference(&classified).collect();
    assert!(
        missing.is_empty(),
        "{PROTOCOL}:{line} changes_layout が分類していない Request がある: {missing:?}"
    );
    let unknown: Vec<&String> = classified.difference(&declared).collect();
    assert!(
        unknown.is_empty(),
        "{PROTOCOL}:{line} changes_layout に存在しない Request 名がある: {unknown:?}"
    );
    let mut dup: Vec<&String> = Vec::new();
    for name in &declared {
        if seen.iter().filter(|s| *s == name).count() > 1 {
            dup.push(name);
        }
    }
    assert!(
        dup.is_empty(),
        "{PROTOCOL}:{line} changes_layout で 2 回以上出てくる Request がある\
         （真と偽の両方に書いた可能性）: {dup:?}"
    );
}

#[test]
fn changes_layoutは読み取り系を真にしていない() {
    let (body, line) = changes_layout_body();
    let split = body
        .find("=> true,")
        .unwrap_or_else(|| panic!("{PROTOCOL}:{line} 真の腕が見つからない"));
    let true_arm = &body[..split];
    // master のポーリングが秒単位で撃つもの。ここが真に倒れると #786 の描画固定費が乗る
    for poll in [
        "Request::List",
        "Request::Read",
        "Request::CheckHealth",
        "Request::OrchestratorWorkerStatus",
        "Request::OrchestratorReport",
        "Request::Send",
        "Request::Scroll",
    ] {
        assert!(
            !true_arm.contains(poll),
            "{PROTOCOL}:{line} {poll} が真の側にある。\
             ポーリングで撃たれる Request を描くと #786 の描画固定費\
             （実測 5.1M instr/frame）が毎回乗る"
        );
    }
}

#[test]
fn セルフテスト項目22bは描かずに待つ() {
    let src = read(APP);
    let (body, line) = region(
        &src,
        APP,
        "// 22b. #1370:",
        "check(pty_ok, \"#1370 レイアウト変更 dispatch 後に PTY サイズが届く\");",
    );
    let code = code_only(body);
    for drawer in [
        "notify_and_draw",
        "wait_for_drawn_state",
        "wait_for_preview_drawn",
    ] {
        assert!(
            !code.contains(drawer),
            "{APP}:{line} 項目 22b が {drawer} を呼んでいる。\
             この項目は**製品側が描くこと**を検査するので、検証側が描くと \
             TAKO_1370_LEGACY=1 でも通ってしまい検出力が丸ごと消える"
        );
    }
    assert!(
        code.contains("wait_for_app_state"),
        "{APP}:{line} 項目 22b が状態待ちを使っていない（固定待ちは #796 の規約違反）"
    );
}
