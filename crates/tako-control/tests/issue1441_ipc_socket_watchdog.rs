//! **#1441 の番犬**: IPC のソケットが `sun_path` の上限で黙って立たなくならない。
//!
//! ## 何が起きていたか
//!
//! ソケットの実体を `<data_dir>/tako.sock` に置いていたので、**パス長が data dir に
//! 比例していた**。`TAKO_ISOLATED=1 TAKO_DATA_DIR=<深いパス>` の隔離起動では
//! bind が `path must be shorter than SUN_LEN` で落ち、`warning: IPC サーバーを
//! 起動できない` の 1 行だけで GUI は普通に立つ。**CLI / MCP から一切操作できない**
//! のに見た目は正常なので、原因に気づくまで時間を失う（#782 の計測中に実測）。
//!
//! ## 何を固定するか
//!
//! 1. [`深いdatadirでも上限に収まるソケットへ逃げる`] — 決め方の実測（**旧アームは
//!    同じ機で bind に失敗する** = A/B が成立していることも同時に測る）
//! 2. [`浅いdatadirの置き場は変わらない`] — 既存挙動の据え置き
//! 3. [`置き場を決める式が1実装だけ`] — `tako.sock` を自分で組み立てる分岐が
//!    他所へ戻っていない（戻ったら `file:line` で名指して落ちる）
//! 4. [`bindの成否を必ず記録する`] — 成功・失敗のどちらでも `ipc_socket::record` を通る
//! 5. [`checkhealthにipc節がある`] / [`cliとmcpが1対1`] — 機械的に検出できる口
//! 6. [`abの逃げ道は両側で同じenvを読む`] — 置き場（tako-core）と通知（tako-app）の
//!    アームが 1 つの env に閉じている
//! 7. [`通知欄への申告を落としていない`] — 起動経路が 1 実装の出し口を呼び続けている

use std::path::{Path, PathBuf};

use tako_core::ipc_socket::{self, SocketPathKind};

#[path = "common/production_range.rs"]
mod production_range;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// 本番コードだけの眺め（テスト領域は空白へ潰す。行番号は保たれる = #1420）
fn production(root: &Path, rel: &str) -> (String, String) {
    let src = read(root, rel);
    (
        production_range::scan(&normalize_test_attrs(&src)).text,
        src,
    )
}

/// `#[cfg(all(test, unix))]` の形も #1420 の 1 実装へ食わせられるようにする。
///
/// `common/production_range.rs` が探すのは**リテラルの `#[cfg(test)]` だけ**なので、
/// `#[cfg(all(test, unix))]` が付いたテストモジュール（`discovery.rs` がそれ）は
/// 潰されず、番犬の走査に**テストコードが混ざる**。ここでは**長さを保ったまま**
/// `#[cfg(test)]` へ揃える（行番号もバイト長も動かないので `file:line` はそのまま）。
/// 共有部品側を触るのは他の番犬の走査範囲を動かすので、寄せるのは別 Issue にする
fn normalize_test_attrs(src: &str) -> String {
    const NEEDLE: &str = "#[cfg(all(test";
    const CANON: &str = "#[cfg(test)]";
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(at) = rest.find(NEEDLE) {
        let Some(end) = rest[at..].find(")]").map(|i| at + i + 2) else {
            break;
        };
        out.push_str(&rest[..at]);
        out.push_str(CANON);
        out.push_str(&" ".repeat(end - at - CANON.len()));
        rest = &rest[end..];
    }
    out.push_str(rest);
    debug_assert_eq!(out.len(), src.len(), "長さが変わると file:line がずれる");
    out
}

/// `needle` を含む行を `file:line` で名指す
fn hits(rel: &str, text: &str, needle: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, l)| format!("{rel}:{} `{}`", i + 1, l.trim()))
        .collect()
}

/// ちょうど `bytes` バイトの data dir（中身は `d` だけ = 個人情報を含まない）
fn deep_dir(bytes: usize) -> PathBuf {
    let head = "/tmp/tako1441-";
    PathBuf::from(format!("{head}{}", "d".repeat(bytes - head.len())))
}

#[test]
fn 深いdatadirでも上限に収まるソケットへ逃げる() {
    let limit = ipc_socket::max_path_bytes();
    // #782 が踏んだ形（data dir 150 バイト = 固定ソケット 160 バイト）
    let data = deep_dir(150);
    let plan = ipc_socket::plan_with(&data, &std::env::temp_dir(), limit);
    assert_eq!(
        plan.kind,
        SocketPathKind::Shortened,
        "深い data dir で短縮していない（実測 {} バイト / 上限 {limit}）",
        plan.well_known_bytes,
    );
    assert!(
        plan.path_bytes() <= limit,
        "逃がし先も上限を超えている: {} > {limit}",
        plan.path_bytes(),
    );

    // 実際に張るのは unix だけ（Windows は名前付きパイプで `sun_path` の上限が無い）。
    // **決め方の検査は両 OS で走る**（上の `plan_with` は純関数）ので、
    // Windows 側でも「深さで置き場が変わる」規則そのものは固定され続ける
    #[cfg(unix)]
    {
        // 実測: 新アームは**実際に bind できる**
        let _ = std::fs::remove_file(&plan.path);
        let listener = std::os::unix::net::UnixListener::bind(&plan.path).unwrap_or_else(|e| {
            panic!(
                "短縮パスへ bind できない（{} バイト）: {e}",
                plan.path_bytes()
            )
        });
        drop(listener);
        let _ = std::fs::remove_file(&plan.path);

        // 旧アーム（data dir 直置き）は**同じ機で bind に失敗する** = A/B が成立している
        std::fs::create_dir_all(&data).expect("検証用 data dir");
        let legacy = data.join(ipc_socket::WELL_KNOWN_NAME);
        let err = std::os::unix::net::UnixListener::bind(&legacy)
            .err()
            .map(|e| e.to_string());
        std::fs::remove_dir_all(&data).ok();
        assert!(
            err.as_deref().is_some_and(|e| e.contains("SUN_LEN")),
            "旧アームが同じ機で落ちない = この検査に検出力が無い（実測: {err:?}）",
        );
    }
}

#[test]
fn 浅いdatadirの置き場は変わらない() {
    let limit = ipc_socket::max_path_bytes();
    let data = Path::new("/tmp/tako-iso-data-4242");
    let plan = ipc_socket::plan_with(data, &std::env::temp_dir(), limit);
    assert_eq!(plan.kind, SocketPathKind::WellKnown);
    assert_eq!(plan.path, data.join(ipc_socket::WELL_KNOWN_NAME));
    assert_eq!(plan.pointer, None, "浅いときは参照ファイルを作らない");
}

/// 決め方は `tako_core::ipc_socket` の 1 実装だけが持つ。
///
/// 直書きの `join("tako.sock")` が他所へ戻ると、**そこだけ上限を見ない**分岐が復活する
/// （まさに #1441 の形）。繋ぐ側（CLI の診断）と bind 側が別々の規則を持つのも同じ穴
#[test]
fn 置き場を決める式が1実装だけ() {
    let root = workspace_root();
    // 決め方の家（ここだけは組み立ててよい）
    let owner = "crates/tako-core/src/ipc_socket.rs";
    let (owner_src, _) = production(&root, owner);
    assert!(
        owner_src.contains("fn plan_with("),
        "{owner} に決め方（`plan_with`）が無い"
    );

    let mut offenders: Vec<String> = Vec::new();
    for rel in [
        "crates/tako-control/src/ipc.rs",
        "crates/tako-control/src/discovery.rs",
        "crates/tako-cli/src/main.rs",
        "crates/tako-app/src/main.rs",
    ] {
        let (src, _) = production(&root, rel);
        for needle in [
            "join(\"tako.sock\")",
            "join(tako_core::ipc_socket::WELL_KNOWN_NAME)",
        ] {
            offenders.extend(hits(rel, &src, needle));
        }
    }
    assert!(
        offenders.is_empty(),
        "ソケットの置き場を自分で組み立てている箇所がある（#1441）。\
         `tako_core::ipc_socket::plan()` / `resolve_with()` を通すこと（実測: {offenders:?}）",
    );

    // bind 側が 1 実装を通っている（黙って外した変更を落とす）
    let (ipc, _) = production(&root, "crates/tako-control/src/ipc.rs");
    assert!(
        ipc.contains("tako_core::ipc_socket::plan()"),
        "ipc.rs が `ipc_socket::plan()` を通っていない（#1441）"
    );
    // 繋ぐ側（CLI の診断）も同じ 1 実装を通っている
    let (cli, _) = production(&root, "crates/tako-cli/src/main.rs");
    assert!(
        cli.contains("tako_core::ipc_socket::resolve_with("),
        "tako-cli の診断が `ipc_socket::resolve_with()` を通っていない（#1441）"
    );
}

/// bind の成否は**どちらも**記録する。片方だけだと「立たなかった」が
/// `check_health` から消え、#1441 の症状（無言の縮退）がそのまま戻る
#[test]
fn bindの成否を必ず記録する() {
    let root = workspace_root();
    let (src, _) = production(&root, "crates/tako-control/src/ipc.rs");
    // `record(` の呼び出しごとに、その直後の `bound: <真偽>` を対にして拾う。
    // **件数だけ数えると 1 本落としても残りで下限を満たしてしまう**ので、
    // 成功 / 失敗の両方が unix / windows の両方に在ることまで測る
    let lines: Vec<&str> = src.lines().collect();
    let mut recorded: Vec<(String, bool)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("ipc_socket::record(") {
            continue;
        }
        let window = lines[i..lines.len().min(i + 12)].join("\n");
        let bound = if window.contains("bound: true") {
            true
        } else if window.contains("bound: false") {
            false
        } else {
            panic!(
                "ipc.rs:{} の `record(` が bound を記録していない（#1441）",
                i + 1
            );
        };
        recorded.push((format!("ipc.rs:{}", i + 1), bound));
    }
    let ok = recorded.iter().filter(|(_, b)| *b).count();
    let ng = recorded.iter().filter(|(_, b)| !*b).count();
    assert!(
        ok >= 2 && ng >= 2,
        "bind の成否の記録が欠けている（unix / windows の成功 {ok} 件 / 失敗 {ng} 件。\
         どちらも 2 件以上であること。#1441。実測: {recorded:?}）",
    );
}

#[test]
fn checkhealthにipc節がある() {
    let root = workspace_root();
    let (src, _) = production(&root, "crates/tako-control/src/dispatch.rs");
    assert!(
        src.contains("tako_core::ipc_socket::status()"),
        "check_health が IPC の記録を読んでいない（#1441）"
    );
    assert!(
        src.contains("\"ipc\": ipc.as_ref().map(ipc_describe)"),
        "check_health の応答に `ipc` 節が無い（#1441）"
    );
    assert!(
        src.contains("fn ipc_health_issue("),
        "立たなかったときの申告（issues 行）が無い（#1441）"
    );
}

/// 設計原則 5（AI フルコントロール）: 受け口の診断は CLI / MCP の両方から引ける
#[test]
fn cliとmcpが1対1() {
    let root = workspace_root();
    let (cli, _) = production(&root, "crates/tako-cli/src/main.rs");
    assert!(
        cli.contains("#[command(name = \"check-health\")]"),
        "`tako check-health` が CLI に無い（MCP だけ = 1:1 が崩れている。#1441）"
    );
    assert!(
        cli.contains("Command::CheckHealth(ref args) => check_health_cli(args.json)"),
        "`check-health` がローカル診断へ落ちていない（IPC が立たないときに答えが出ない）"
    );
    let (mcp, _) = production(&root, "crates/tako-control/src/mcp/request.rs");
    assert!(
        mcp.contains("\"tako_check_health\" => Request::CheckHealth"),
        "MCP 側の `tako_check_health` が無い（#1441）"
    );
}

/// A/B の逃げ道は 1 つの env に閉じる。置き場（tako-core）と通知（tako-app）が
/// 別々の env を読むと、片方だけ倒したときに**回帰が隠れる**
#[test]
fn abの逃げ道は両側で同じenvを読む() {
    let root = workspace_root();
    let (core, _) = production(&root, "crates/tako-core/src/ipc_socket.rs");
    let (sidebar, _) = production(&root, "crates/tako-app/src/sidebar.rs");
    for (rel, src) in [
        ("crates/tako-core/src/ipc_socket.rs", &core),
        ("crates/tako-app/src/sidebar.rs", &sidebar),
    ] {
        assert!(
            src.contains("fn legacy_1441() -> bool {"),
            "{rel} に #1441 のアームの宣言が無い"
        );
        assert!(
            src.contains("std::env::var(\"TAKO_1441_LEGACY\")"),
            "{rel} のアームが `TAKO_1441_LEGACY` を読んでいない（#1441）"
        );
        // 別 Issue の env を巻き込んでいない
        let strays: Vec<&str> = ["TAKO_1399_LEGACY", "TAKO_1417_LEGACY", "TAKO_1422_LEGACY"]
            .into_iter()
            .filter(|e| {
                src.split_once("fn legacy_1441() -> bool {")
                    .map(|(_, rest)| rest.split("\n    }").next().unwrap_or("").contains(e))
                    .unwrap_or(false)
            })
            .collect();
        assert!(
            strays.is_empty(),
            "{rel} のアームが別 Issue の env を読んでいる: {strays:?}"
        );
    }
    // 旧アームは置き場を data dir 直下へ固定する（= 深い data dir で落ちる形の再現）
    assert!(
        core.contains("if legacy_1441() {"),
        "`plan()` に旧アームが無い = A/B が成立していない（#1441）"
    );
}

/// 起動経路が 1 実装の出し口（`notify_ui_failure`）を呼び続けている。
/// ここを外すと「見た目は正常なのに CLI / MCP が届かない」が無言で戻る
#[test]
fn 通知欄への申告を落としていない() {
    let root = workspace_root();
    let main = read(&root, "crates/tako-app/src/main.rs");
    let calls = hits("main.rs", &main, "app.notify_ipc_unavailable(&status)");
    assert_eq!(
        calls.len(),
        1,
        "起動経路から受け口の申告が {} 件（1 件であること。#1441。実測: {calls:?}）",
        calls.len(),
    );
    let (sidebar, _) = production(&root, "crates/tako-app/src/sidebar.rs");
    let body = sidebar
        .split_once("fn notify_ipc_unavailable(")
        .map(|(_, rest)| rest.split("\n    }").next().unwrap_or("").to_string())
        .expect("`notify_ipc_unavailable` が sidebar.rs に無い（#1441）");
    assert!(
        body.contains("self.notify_ui_failure("),
        "受け口の申告が 1 実装（`notify_ui_failure`）を通っていない（#1441）"
    );
    assert!(
        body.contains("NoticeArm::Issue1441"),
        "受け口の申告が #1441 のアームを通っていない（A/B が効かない）"
    );
    assert!(
        body.contains("if status.bound {"),
        "立っているときも出す形になっている（誤検知。#1441）"
    );
}
