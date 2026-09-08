//! 側路（シェル統合の OSC 運搬）の待ち合わせ先が「書き手の同一性」を持つ番犬（#1199）
//!
//! 旧実装の待ち合わせは**パスだけ**だった（`<data_dir>/osc/<pane>.osc`）。
//! 器へ渡すペイン固有の env は器の中の全シェルへ配られる（psmux の `-e` は
//! セッションではなく**サーバーのグローバル環境**へ入る。実測: `show-environment -g` に
//! `TAKO_PANE_ID` / `TAKO_OSC_SINK` が並ぶ）ので、psmux が持つ**プリウォーム済みの
//! シェルの一団**もそのペインの側路へ書けてしまう。
//!
//! そのシェルたちは cwd がユーザーのホームなので、tako がペインをリサイズするたびに
//! （psmux はプールもクライアントの寸法へ合わせる）プロンプトを描き直して
//! `OSC 7 <ホーム>` を書き、**ペインの cwd がホームへ巻き戻っていた**（#1199 = 機能不能。
//! ファイルツリー・ステータスバー・`tako list` が誤ったフォルダを指す）。
//!
//! 直し方は「書く側を減らす」ではなく**同じファイルへ書けなくする**こと:
//!   1. 統合スクリプトが待ち合わせ先へ**自分の pid を載せる**（`<pane>@p<pid>.osc`）
//!   2. tako は**そのペインのシェルの pid** ぶんだけを読む（`SinkReader`）
//!   3. 規則の正本は `tako_core::osc_sink::resolve_writer_path` の 1 実装で、
//!      スクリプトはその写し（ここで両方を突き合わせる）
//!
//! ソース走査で見張るのはこの 3 つ。

use std::path::{Path, PathBuf};

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

#[test]
fn 統合スクリプトは側路の待ち合わせ先へ自分のpidを載せる() {
    let root = workspace_root();
    let ps1 = read(&root, "crates/tako-core/shell-integration/tako.ps1");

    assert!(
        ps1.contains("'@p' + $PID + '.osc'"),
        "tako.ps1 が待ち合わせ先へ自分の pid を載せていない（#1199。\
         パスだけの待ち合わせでは器の中の別のシェルが同じファイルへ書ける）"
    );
    assert!(
        ps1.contains("-notmatch '@p[0-9]+\\.osc$'"),
        "tako.ps1 の解決が冪等でない（#1199。ペインの中で起こした子シェルが env を\
         継承して再解決すると、tako が読まない場所へ書く）"
    );
    assert!(
        ps1.contains("$env:TAKO_OSC_SINK = $global:__takoSink"),
        "解決結果を env へ書き戻していない（#1199。入れ子の pwsh / ユーザー自身の tmux が\
         同じファイルを共有できず cwd 追従が死ぬ）"
    );
}

#[test]
fn スクリプトの解決規則はtako_coreの正本と同じ形を作る() {
    // スクリプトは `<...>.osc` -> `<...>@p<pid>.osc` を作る。正本と 1 バイト違えば
    // tako は誰も書かないファイルを読み続ける（cwd 追従が黙って死ぬ）
    let resolved = tako_core::osc_sink::resolve_writer_path("C:\\d\\osc\\1.osc", 4242);
    assert_eq!(resolved, "C:\\d\\osc\\1@p4242.osc");
    assert_eq!(
        tako_core::osc_sink::writer_pid_of(&resolved),
        Some(4242),
        "解決済みパスから pid を読み戻せない"
    );
}

#[test]
fn appは書き手を区別しない読み口を持たない() {
    let root = workspace_root();
    let main = read(&root, "crates/tako-app/src/main.rs");

    assert!(
        main.contains("osc_sinks: HashMap<PaneId, tako_core::osc_sink::SinkReader>"),
        "側路の読み口が `SinkReader` でない（#1199。パスと読み取り位置だけを持つ形へ\
         戻すと、器の中の別のシェルが書いたぶんをそのペインの状態へ入れてしまう）"
    );
    assert!(
        main.contains("fn set_osc_sink_writer"),
        "そのペインのシェルの pid を張る入口が無い（#1199）"
    );
    // 器ありは器へ聞いた `#{pane_pid}`、器なしは PTY 直下の子。どちらも張ること
    assert!(
        main.contains("if let Some(pid) = facts.pid {"),
        "器ありのペインで `#{{pane_pid}}` から書き手を張っていない（#1199）"
    );
    assert!(
        main.contains("if backend_session.is_none() {"),
        "器なしのペインで PTY 直下の子から書き手を張っていない（#1199）"
    );
}
