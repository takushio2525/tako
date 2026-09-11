//! **#1371 の番犬**: URL を開く経路にシェルを挟まない。
//!
//! 修正前の Windows 実装は `cmd /C start "" <url>` を組んでいた。
//! `std::process::Command` の Windows 実装は空白かタブを含まない引数を引用符で囲まない
//! （`Quote::Auto`）ため、cmd.exe が引用符の外の `&` を**コマンド区切り**として解釈し、
//! クエリ文字列つきの URL はそこで切れ、残りが別コマンドとして実行されていた。
//! tako が開く URL は画面 / PDF / Markdown 由来 = **第三者が書ける**ので、
//! これはクリック 1 回の任意コマンド実行になる。
//!
//! Windows 実機が無くても再発を落とせるように、検査は 2 本立てにしてある:
//!
//! 1. **ソース走査**: 境界 B8 の実装部にシェル起動（`cmd` / `powershell` / `sh` …）が
//!    1 つも無いこと。増えたら file:line で名指しする
//! 2. **純粋関数**: `windows_url_launch` が返す `ShellExecuteW` の引数。
//!    メタ文字を含む URL でも `lpFile` に**丸ごと 1 つの値**として載り、
//!    生のコマンドライン（`lpParameters`）を組まないこと

use std::path::{Path, PathBuf};

use tako_control::platform::os_integration::windows_url_launch;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中のテスト用ヘルパで走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

const SOURCE: &str = "crates/tako-control/src/platform/os_integration.rs";

fn source() -> String {
    std::fs::read_to_string(repo_root().join(SOURCE)).expect("境界 B8 の実装本体を読める")
}

/// 実装部（`#[cfg(test)]` の付いた item を空白へ潰した眺め）だけを返す。
/// 切らない理由と「黙って縮んだ」の検出は `common/production_range.rs`（#1420）
fn production(src: &str) -> String {
    let impl_src = production_range::production(src, SOURCE);
    assert!(
        impl_src.contains("pub fn open_url"),
        "open_url が見つからない: 走査先が間違っている"
    );
    assert!(
        impl_src.contains("pub fn windows_url_launch"),
        "windows_url_launch が見つからない: 走査先が間違っている"
    );
    impl_src
}

/// コメント行（`//` / `///` / `//!`）を除いたコード行。
/// 「`cmd /C start` は使わない」のような**説明文**で落ちないようにするため
fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// `#[cfg(windows)] mod imp { … }` の本体と、**本体先頭のファイル行番号**（1 始まり）。
/// ブロックの終わりは列 0 の `}`（このファイルの mod は全部そう書かれている）。
/// 行番号を返すのは、落ちたときに `file:line` で名指しできるようにするため
fn windows_imp(src: &str) -> (usize, String) {
    let head = "#[cfg(windows)]\nmod imp {";
    let start = src.find(head).expect("Windows 実装の mod が見つからない");
    // 本体の 0 行目は `mod imp {` の行末（= start の行 + 1 行）
    let base = src[..start].matches('\n').count() + 2;
    let body = &src[start + head.len()..];
    let end = body
        .find("\n}\n")
        .expect("Windows 実装の mod の終わりが見つからない");
    (base, body[..end].to_string())
}

/// Windows 実装の本体内オフセットをファイル行番号へ直す
fn line_at(win: &str, base: usize, needle: &str) -> usize {
    match win.find(needle) {
        Some(at) => base + win[..at].matches('\n').count(),
        None => base,
    }
}

/// **受け入れ条件 1**: 境界 B8 の実装部に、URL / ファイルを開くためのシェル起動が残っていない。
///
/// cmd.exe を通した瞬間にコマンドラインの再解釈が起きるので、
/// 「引用符を足したから安全」ではなく**シェルを経路に置かない**ことを固定する
#[test]
fn urlを開く経路にシェルが挟まっていない() {
    // 実際にプロセスを起こす形だけを見る（コメント中の言及は除外済み）
    const SHELLS: &[&str] = &[
        "Command::new(\"cmd\"",
        "Command::new(\"cmd.exe\"",
        "Command::new(\"powershell",
        "Command::new(\"pwsh",
        "Command::new(\"sh\"",
        "Command::new(\"bash\"",
        "Command::new(\"zsh\"",
    ];
    // cmd の内蔵コマンドを組む形（program が cmd でなくても、この並びが出たら同じ穴）
    const CMD_ARGS: &[&str] = &["\"/C\"", "\"/c\"", "\"start\""];

    let src = source();
    let mut offenders = Vec::new();
    for (i, line) in production(&src).lines().enumerate() {
        if is_comment(line) {
            continue;
        }
        if SHELLS.iter().chain(CMD_ARGS).any(|p| line.contains(p)) {
            offenders.push(format!("{SOURCE}:{}: {}", i + 1, line.trim()));
        }
    }
    assert!(
        offenders.is_empty(),
        "URL / ファイルを開く経路にシェルが挟まっている（`&` でコマンドが割り込む）:\n  {}\n\
         → ShellExecuteW（`shell_execute`）へ寄せてください（#1371）",
        offenders.join("\n  ")
    );
}

/// **受け入れ条件 1**: Windows の URL 起動が `ShellExecuteW` の薄いラッパーへ寄っており、
/// 引数の組み方は境界の外の純粋関数（`windows_url_launch`）が正本であること。
///
/// Windows 実装でプロセスを起こしてよいのは `explorer.exe`（`reveal`）だけ。
/// **explorer は `CommandLineToArgvW` を通さない**特殊な事情があってプロセス起動のままで、
/// それ以外が増えたらシェル経由が戻ってきた疑いがある
#[test]
fn windowsのurl起動はshellexecuteへ寄っている() {
    let src = source();
    let (base, win) = windows_imp(&production(&src));

    let open_url_at = line_at(&win, base, "pub fn open_url(url: &str)");
    let open_url = win
        .split_once("pub fn open_url(url: &str)")
        .map(|(_, rest)| rest.split("\n    }").next().unwrap_or(rest).to_string())
        .expect("Windows 実装に open_url がある");
    assert!(
        open_url.contains("windows_url_launch(url)"),
        "{SOURCE}:{open_url_at}: Windows の open_url が引数の正本を通っていない:\n{open_url}"
    );
    assert!(
        open_url.contains("shell_execute("),
        "{SOURCE}:{open_url_at}: Windows の open_url が ShellExecuteW のラッパーを通っていない:\n{open_url}"
    );

    let wait_at = line_at(&win, base, "pub fn open_url_wait(url: &str)");
    let open_url_wait = win
        .split_once("pub fn open_url_wait(url: &str)")
        .map(|(_, rest)| rest.split("\n    }").next().unwrap_or(rest).to_string())
        .expect("Windows 実装に open_url_wait がある");
    assert!(
        open_url_wait.contains("open_url(url)"),
        "{SOURCE}:{wait_at}: Windows の open_url_wait が open_url と同じ経路を通っていない:\n{open_url_wait}"
    );

    let mut offenders = Vec::new();
    for (i, line) in win.lines().enumerate() {
        if is_comment(line) || !line.contains("Command::new(") {
            continue;
        }
        if line.contains("Command::new(\"explorer.exe\")") {
            continue;
        }
        offenders.push(format!("{SOURCE}:{}: {}", base + i, line.trim()));
    }
    assert!(
        offenders.is_empty(),
        "Windows の境界 B8 で explorer.exe 以外のプロセスを起こしている:\n  {}\n\
         → シェル API（ShellExecuteW）で開いてください（#1371）",
        offenders.join("\n  ")
    );
}

/// **受け入れ条件 2**: メタ文字を含む URL でも `lpFile` に**丸ごと 1 つの値**として載り、
/// 生のコマンドライン（`lpParameters`）を組まないこと。
///
/// `cmd /C start` に戻すとここで組まれるのは argv になるので、この形では表現できない。
/// Windows 実機を待たずに macOS からも同じ判定が走る
#[test]
fn メタ文字を含むurlは1つの値のまま渡る() {
    // すべて実際に踏みうる形（`&` は URL の正規の文字で links.rs も通す）
    const URLS: &[&str] = &[
        "https://example.com/search?q=a&lang=ja",
        "https://example.com/?a=1&b=2|c^d%20e",
        "https://example.com/?x=\"y\"&z=<w>",
        "https://example.com/?q=1&&whoami",
        "https://example.com/?p=%TEMP%&r=1",
        "https://example.com/a?b=1&c=2#frag",
        "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
    ];

    for url in URLS {
        let launch = windows_url_launch(url);
        // 加工しない = 切らない・引用符を足さない・エスケープしない
        assert_eq!(
            launch.file, *url,
            "URL が加工されている（`&` の手前で切れていないか）: {url}"
        );
        // ここに入れるとシェルのメタ文字が効く余地ができる
        assert!(
            launch.parameters.is_none(),
            "URL が生のコマンドライン（lpParameters）へ載っている: {launch:?}"
        );
        // 既定の動詞（無ければ open に落ちる）。プロトコルハンドラ側の既定に従う
        assert_eq!(
            launch.verb, None,
            "動詞を固定すると開けない URL が出る: {url}"
        );
    }
}

/// `&` の手前で切れていたときの症状を、症状そのものの形で固定する。
///
/// 修正前は `https://example.com/search?q=a&lang=ja` の `lang=ja` が
/// **別コマンド**として実行されていた。`lpFile` が URL 全体と一致していれば、
/// 切れ目が存在しない = 後続がコマンドとして解釈されない
#[test]
fn クエリ文字列の後半が落ちない() {
    let url = "https://example.com/search?q=a&lang=ja";
    let launch = windows_url_launch(url);
    assert!(
        launch.file.ends_with("&lang=ja"),
        "`&` 以降が落ちている: {:?}",
        launch.file
    );
    assert_eq!(launch.file.matches('&').count(), 1);
}

/// 番犬が名指しする行番号がファイルと一致すること。
///
/// 1 行ずれると直す人が別の行を見に行くので、**物差しそのもの**をここで固定する。
/// 基準には Windows 実装で唯一許したプロセス起動（`reveal` の explorer.exe）を使う
#[test]
fn 名指しする行番号がファイルと一致する() {
    let src = source();
    let (base, win) = windows_imp(&production(&src));
    const NEEDLE: &str = "Command::new(\"explorer.exe\")";
    let reported = line_at(&win, base, NEEDLE);
    let actual = src
        .lines()
        .position(|l| l.contains(NEEDLE))
        .expect("explorer.exe の直起動がある")
        + 1;
    assert_eq!(reported, actual, "番犬が名指しする行番号が 1 行ずれている");
}
