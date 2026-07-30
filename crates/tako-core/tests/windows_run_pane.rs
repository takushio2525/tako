//! 実行ペイン（Code Runner）の Windows 経路を**実 PowerShell で**検証する（#525）。
//!
//! 単体テストは組み立てた文字列しか見られないので、
//! 「PowerShell が実際にその引数を受け取って終了コードを正しく報告するか」はここで確かめる。
//!
//! ## なぜ PTY 無しで検証できるか
//!
//! 実行ペインのスクリプトは最後に `[Console]::ReadLine()` で入力待ちに入る。
//! stdin を `null`（NUL デバイス）にすると即 EOF で `null` が返り、スクリプトは終了する。
//! つまり**待ちの構造を保ったまま**プロセスとして完走させられる。
//!
//! `#[cfg(windows)]` なので macOS では 0 件になる（`cargo test` は緑のまま）。
#![cfg(windows)]

use std::process::{Command, Stdio};

const MARKER: &str = "__TAKO_EXIT=";

/// 実行ペインと同じコマンドを組み立てて走らせ、標準出力を返す
fn run(command: &str) -> String {
    let spec = tako_core::platform::shell::run_pane_command(command, MARKER);
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        // ここで即 EOF になり `[Console]::ReadLine()` を抜ける
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // #628: テストでもコンソール窓を出さない
    tako_core::platform::process::no_console_window(&mut cmd);
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("実行ペインのシェルを起動できない（{}: {e}）", spec.program));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// 画面に出たマーカーから終了コードを読む（`dispatch::find_exit_marker` と同じ読み方）
fn exit_code(output: &str) -> Option<i32> {
    output.lines().rev().find_map(|line| {
        line.find(MARKER)
            .and_then(|pos| line[pos + MARKER.len()..].trim().parse::<i32>().ok())
    })
}

#[test]
fn 実行ペインはネイティブexeの終了コードをそのまま報告する() {
    assert_eq!(exit_code(&run("cmd.exe /c exit 0")), Some(0));
    assert_eq!(exit_code(&run("cmd.exe /c exit 7")), Some(7));
}

#[test]
fn 実行ペインはcmdletの成否も終了コードに落とす() {
    // cmdlet は $LASTEXITCODE を設定しない。$? を見ていないと失敗が 0 に化ける
    assert_eq!(exit_code(&run("Get-Date | Out-Null")), Some(0));
    assert_eq!(
        exit_code(&run("Get-Item 'C:\\no-such-path-for-tako-test'")),
        Some(1)
    );
}

#[test]
fn 存在しないコマンドは非ゼロで返る() {
    let code = exit_code(&run("tako-no-such-command-xyz")).expect("マーカーが出る");
    assert_ne!(code, 0);
}

#[test]
fn 複合コマンドはposixのセミコロンと同じ結果になる() {
    // `sh -c 'false; true'` = 0 / `sh -c 'true; false'` = 1 と揃っていること
    assert_eq!(
        exit_code(&run("cmd.exe /c exit 5; Get-Date | Out-Null")),
        Some(0)
    );
    assert_eq!(
        exit_code(&run("cmd.exe /c exit 0; Get-Item 'C:\\nope-xyz'")),
        Some(1)
    );
}

#[test]
fn 空白と日本語と引用符を含むコマンドが壊れずに届く() {
    // -EncodedCommand を使う理由そのものの検証。
    // psmux / cmd.exe / ConPTY のどの層でも引用符が解釈されないことを担保する。
    //
    // 判定に**文字コードを出力する**のは、このテストが stdout をパイプへ落としているから。
    // PowerShell はリダイレクト時 `[Console]::OutputEncoding`（この環境では CP932）で
    // 書くので、生の日本語をバイト比較すると**製品ではなくテストの都合で**落ちる
    //（実ペインは ConPTY で UTF-8 が出るので別問題）。コードポイント列なら ASCII だけで
    // 「PowerShell の中に文字列が正しく届いたか」を検査できる
    let text = "日本語 'と' 引用符 のテスト";
    let out = run(&format!(
        "$s = \"{text}\"; Write-Host ('CP=' + (([int[]][char[]]$s) -join ','))"
    ));
    let expected: Vec<String> = text.encode_utf16().map(|u| u.to_string()).collect();
    assert!(
        out.contains(&format!("CP={}", expected.join(","))),
        "コマンド中の文字列が壊れて届いた: {out}"
    );
    assert_eq!(exit_code(&out), Some(0));
}

#[test]
fn 引数に空白を含むパスを渡しても1語に潰れない() {
    let out = run("Write-Host 'C:\\Program Files\\x y.txt'");
    assert!(out.contains("C:\\Program Files\\x y.txt"), "{out}");
    assert_eq!(exit_code(&out), Some(0));
}

#[test]
fn 長時間実行でもマーカーは完了後にだけ出る() {
    // status ポーリングは「マーカーがまだ無い = running」で判断する（`find_exit_marker`）。
    // コマンドの完了より先にマーカーが出ると、走っている最中に完了扱いされてしまう
    let started = std::time::Instant::now();
    let out = run("Start-Sleep -Milliseconds 1500; Write-Host DONE");
    let elapsed = started.elapsed();
    assert!(
        elapsed >= std::time::Duration::from_millis(1400),
        "コマンドの完了を待たずに抜けた: {elapsed:?}"
    );
    assert!(out.contains("DONE"), "{out}");
    assert_eq!(exit_code(&out), Some(0));
    let marker_pos = out.rfind(MARKER).expect("マーカーがある");
    assert!(
        marker_pos > out.rfind("DONE").expect("DONE がある"),
        "マーカーがコマンド出力より前に出ている: {out}"
    );
}

#[test]
fn 出力が長くてもマーカーは最後に出る() {
    // マーカーは画面の末尾から探されるので、出力に紛れて先に出ないこと
    let out = run("1..50 | ForEach-Object { Write-Host \"line $_\" }");
    assert!(out.contains("line 50"), "{out}");
    assert_eq!(exit_code(&out), Some(0));
    let marker_pos = out.rfind(MARKER).expect("マーカーがある");
    let last_line_pos = out.rfind("line 50").expect("最終行がある");
    assert!(
        marker_pos > last_line_pos,
        "マーカーが出力より前に出ている: {out}"
    );
}
