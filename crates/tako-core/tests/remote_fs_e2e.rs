//! #919 / #65 の実機 e2e: 実 SSH 先に対して ControlMaster を張り、SFTP で
//! ディレクトリを一覧し、ファイルを取得できること。**失敗も分類できること**。
//!
//! 実ホストが要るので `--ignored` で明示実行する:
//!
//! ```text
//! TAKO_REMOTE_E2E_HOST=win \
//!   cargo test -p tako-core --test remote_fs_e2e -- --ignored --nocapture --test-threads=1
//! ```
//!
//! `TAKO_REMOTE_E2E_HOST` は `~/.ssh/config` の Host 名。鍵認証で入れる相手を指定する
//! （BatchMode で張るので、パスワードしか無い相手はここでは検証できない）。
//! `TAKO_REMOTE_E2E_DIR` で一覧するディレクトリを、`TAKO_REMOTE_E2E_FILE` で
//! 取得するファイルを上書きできる（既定はリモートのホームと、その中の適当な 1 ファイル）。

use tako_core::remote_fs::{self, RemoteErrorKind, RemoteKind};

fn host() -> Option<String> {
    std::env::var("TAKO_REMOTE_E2E_HOST")
        .ok()
        .filter(|h| !h.trim().is_empty())
}

#[test]
#[ignore = "実 SSH 先が要る（TAKO_REMOTE_E2E_HOST を指定して実行）"]
fn 実ホストへ接続して一覧と取得ができる() {
    let Some(host) = host() else {
        panic!("TAKO_REMOTE_E2E_HOST が未設定（例: TAKO_REMOTE_E2E_HOST=win）");
    };

    // 1) 接続（ControlMaster 確立 + リモート cwd の取得）
    let home = remote_fs::connect(&host).unwrap_or_else(|e| {
        panic!("接続できない: {}\n{}", e.summary(), e.next_step());
    });
    println!("接続 OK: home={home}");
    assert!(!home.is_empty());

    // 2) ControlMaster が生きている = 以後の操作は追加認証なし（#919 要件 6）
    assert!(
        remote_fs::master_alive(&host),
        "ControlMaster が生きていない（追加認証なしの共有が成立していない）"
    );

    // 3) 一覧
    let dir = std::env::var("TAKO_REMOTE_E2E_DIR").unwrap_or_else(|_| home.clone());
    let entries = remote_fs::list_dir(&host, &dir).unwrap_or_else(|e| {
        panic!("一覧できない: {}\n{}", e.summary(), e.next_step());
    });
    println!("{dir} → {} 件", entries.len());
    for e in entries.iter().take(10) {
        println!("  {:?} {} ({} バイト)", e.kind, e.name, e.size);
    }
    assert!(
        !entries.is_empty(),
        "ホームが空に見える（解析が壊れている疑い）"
    );
    // `.` / `..` は落ちている
    assert!(
        !entries.iter().any(|e| e.name == "." || e.name == ".."),
        "`.` / `..` が混ざっている"
    );
    // 種別が全部 Unknown なら longname の解析が効いていない
    assert!(
        entries.iter().any(|e| e.kind != RemoteKind::Unknown),
        "全エントリが Unknown = mode の解析が効いていない"
    );
    // パスは一覧したディレクトリ配下になる
    for e in &entries {
        assert_eq!(e.path, remote_fs::join_remote(&dir, &e.name));
    }

    // 4) ファイル取得（プレビューが開くのと同じ経路）
    let file = match std::env::var("TAKO_REMOTE_E2E_FILE") {
        Ok(f) if !f.trim().is_empty() => f,
        _ => entries
            .iter()
            .find(|e| e.kind == RemoteKind::File && e.size > 0 && e.size < 64 * 1024)
            .map(|e| e.path.clone())
            .unwrap_or_else(|| panic!("取得に使える小さいファイルが {dir} に無い")),
    };
    let (local, stat) = remote_fs::fetch_file(&host, &file, remote_fs::MAX_PREVIEW_BYTES)
        .unwrap_or_else(|e| panic!("取得できない: {}\n{}", e.summary(), e.next_step()));
    // #966: 素性（mode / サイズ / 日時）も返る = 書けるかの見立てが立つ
    let stat = stat.expect("素性を解析できる");
    println!(
        "  素性: mode={} size={} mtime={} writable_hint={:?}",
        stat.mode,
        stat.size,
        stat.mtime,
        stat.writable_hint()
    );
    let bytes = std::fs::metadata(&local)
        .expect("落ちたファイルを stat できる")
        .len();
    println!("取得 OK: {file} → {} ({bytes} バイト)", local.display());
    assert!(bytes > 0, "落ちたファイルが空");
    // プレビューの編集を止める判定（キャッシュ配下と分かること）
    assert!(
        remote_fs::is_cached_remote(&local),
        "キャッシュ配下と判定できない = 読み取り専用にできない"
    );
    // #966: 取得と同時に「開いた時点の内容」が記録される（競合検知の基準）
    let baseline = remote_fs::read_baseline(&host, &file).expect("開いた時点の記録がある");
    assert_eq!(
        baseline,
        std::fs::read(&local).expect("写しを読める"),
        "開いた時点の記録が写しと一致しない = 競合検知の基準がずれる"
    );
    // 拡張子が保たれている（プレビューの種別判定が拡張子を見る）
    let remote_base = remote_fs::base_name(&file);
    assert!(
        local
            .file_name()
            .map(|n| n.to_string_lossy().ends_with(&remote_base))
            .unwrap_or(false),
        "キャッシュ名がリモートの名前で終わっていない: {}",
        local.display()
    );
}

#[test]
#[ignore = "実 SSH 先が要る（TAKO_REMOTE_E2E_HOST を指定して実行）"]
fn 存在しないパスは_not_found_として返る() {
    let Some(host) = host() else {
        panic!("TAKO_REMOTE_E2E_HOST が未設定");
    };
    let err = remote_fs::list_dir(&host, "/tako919/no/such/dir")
        .expect_err("存在しないパスが成功してしまった");
    println!("{} / {}", err.summary(), err.detail);
    assert_eq!(err.kind, RemoteErrorKind::NotFound, "detail={}", err.detail);
    // 静かな失敗にならない
    assert!(err.report().lines().count() >= 3);
}

/// 到達不能・名前解決不能は**ネットワーク非依存で決定的**なので ignore を付けない。
/// `.invalid` は RFC 2606 で予約されており、必ず解決に失敗する
#[test]
fn 解決できないホストは接続前に分類される() {
    if remote_fs::ssh_bin().is_none() {
        eprintln!("ssh が無い環境なのでスキップ");
        return;
    }
    let err = remote_fs::connect("tako919-no-such-host.invalid")
        .expect_err("解決できないホストへ接続できてしまった");
    println!("{} / {}", err.summary(), err.detail);
    assert!(
        matches!(
            err.kind,
            RemoteErrorKind::HostUnresolved | RemoteErrorKind::Unreachable
        ),
        "分類が想定外: {:?} detail={}",
        err.kind,
        err.detail
    );
    // #919 の要点: 理由と次の一手が必ず付く
    let report = err.report();
    assert!(report.contains("tako919-no-such-host.invalid"), "{report}");
    assert!(report.lines().count() >= 3, "{report}");
}

// --- 書き戻し（#966。段階 2） ----------------------------------------------

/// 検証に使う一時ディレクトリ（**相手のリポジトリ・作業物には触らない**）。
/// `TAKO_REMOTE_E2E_WRITE_DIR` で明示できる（既定はホーム配下の `tmp-966`）
fn write_dir(home: &str) -> String {
    std::env::var("TAKO_REMOTE_E2E_WRITE_DIR")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| remote_fs::join_remote(home, "tmp-966"))
}

/// **tako の実装を通さない**素の `sftp -b -`（検証の独立性のため）。
///
/// 競合を作る側・実体を読み戻す側が tako のコードを共有していると、
/// 「tako から見て一致している」だけで「リモートが本当に変わった」が測れない
fn raw_sftp(host: &str, commands: &[String]) -> (bool, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut script = commands.join("\n");
    script.push_str("\nbye\n");
    let mut child = Command::new("sftp")
        .args(["-o", "BatchMode=yes", "-b", "-", host])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sftp を起動できる");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(script.as_bytes())
        .expect("バッチを渡せる");
    let out = child.wait_with_output().expect("sftp の終了を待てる");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// リモートの 1 ファイルを消す（後始末。消えていても打ち切らないよう `-` を前置）
fn remote_rm(host: &str, path: &str) {
    let _ = raw_sftp(host, &[format!("-rm {}", remote_fs::quote_sftp_arg(path))]);
}

fn remote_mkdir(host: &str, dir: &str) {
    let _ = raw_sftp(
        host,
        &[format!("-mkdir {}", remote_fs::quote_sftp_arg(dir))],
    );
}

/// リモートへ内容を置く（**tako の書き戻し経路を通さない**別経路 = 競合の作り方）
fn remote_seed(host: &str, path: &str, body: &str) {
    let tmp = std::env::temp_dir().join(format!("tako966-seed-{}", std::process::id()));
    std::fs::write(&tmp, body).expect("種を書ける");
    let (ok, text) = raw_sftp(
        host,
        &[format!(
            "put {} {}",
            remote_fs::quote_sftp_arg(&tmp.to_string_lossy()),
            remote_fs::quote_sftp_arg(path)
        )],
    );
    let _ = std::fs::remove_file(&tmp);
    assert!(ok, "種を置けなかった {host}:{path}\n{text}");
}

/// リモートの内容を読み戻す（**キャッシュではなく実体**を確かめる）
fn remote_read(host: &str, path: &str) -> Vec<u8> {
    let local = std::env::temp_dir().join(format!("tako966-read-{}", std::process::id()));
    let _ = std::fs::remove_file(&local);
    let (ok, text) = raw_sftp(
        host,
        &[format!(
            "get {} {}",
            remote_fs::quote_sftp_arg(path),
            remote_fs::quote_sftp_arg(&local.to_string_lossy())
        )],
    );
    assert!(ok, "読み戻せなかった {host}:{path}\n{text}");
    let bytes = std::fs::read(&local).expect("落ちたファイルを読める");
    let _ = std::fs::remove_file(&local);
    bytes
}

/// リモートの mode を変える（実行権の検証用。tako の経路を通さない）
fn remote_chmod(host: &str, path: &str, octal: &str) {
    let (ok, text) = raw_sftp(
        host,
        &[format!("chmod {octal} {}", remote_fs::quote_sftp_arg(path))],
    );
    assert!(ok, "chmod できなかった {host}:{path}\n{text}");
}

#[test]
#[ignore = "実 SSH 先が要る（TAKO_REMOTE_E2E_HOST を指定して実行）"]
fn 実ホストへ書き戻せて実体が変わる() {
    let Some(host) = host() else {
        panic!("TAKO_REMOTE_E2E_HOST が未設定");
    };
    let home = remote_fs::connect(&host).expect("接続できる");
    let dir = write_dir(&home);
    let path = remote_fs::join_remote(&dir, "edit.txt");
    remote_mkdir(&host, &dir);
    remote_seed(&host, &path, "original line 1\noriginal line 2\n");

    // 開く（= 取得 + 開いた時点の記録）
    let (local, stat) =
        remote_fs::fetch_file(&host, &path, remote_fs::MAX_PREVIEW_BYTES).expect("取得できる");
    assert_eq!(
        std::fs::read(&local).unwrap(),
        b"original line 1\noriginal line 2\n"
    );
    println!(
        "開いた: mode={} size={}",
        stat.as_ref().unwrap().mode,
        stat.as_ref().unwrap().size
    );

    // 編集して保存
    let edited = "edited line 1\noriginal line 2\nadded line 3\n";
    let report = remote_fs::save_file(&host, &path, edited.as_bytes(), false).unwrap_or_else(|e| {
        panic!(
            "書き戻せない: {}\n{}\n{}",
            e.summary(),
            e.next_step(),
            e.detail
        )
    });
    println!(
        "書き戻し OK: {} バイト atomic={} verified={} mode_restored={:?}",
        report.bytes, report.atomic, report.verified, report.mode_restored
    );
    assert!(report.atomic, "アトミックな経路を通っていない");
    assert!(
        report.verified,
        "競合検知でリモートの内容を突き合わせていない"
    );

    // **リモートの実体**が変わっている（キャッシュではなく読み戻して照合）
    let actual = remote_read(&host, &path);
    assert_eq!(
        String::from_utf8_lossy(&actual),
        edited,
        "リモートの実体が変わっていない"
    );
    // 一時ファイルが残っていない（アトミック経路の後始末）
    let tmp_name = remote_fs::base_name(&remote_fs::temp_remote_path(&path));
    let listed = remote_fs::list_dir(&host, &dir).expect("一覧できる");
    assert!(
        !listed.iter().any(|e| e.name == tmp_name),
        "書き戻しの一時ファイルが残っている: {tmp_name}"
    );
    // 開いた時点の記録が「いま書いた内容」へ進んでいる = 続けて保存できる
    assert_eq!(
        remote_fs::read_baseline(&host, &path).as_deref(),
        Some(edited.as_bytes()),
        "基準が進んでいない = 次の保存が必ず競合になる"
    );
    let again = "edited twice\n";
    remote_fs::save_file(&host, &path, again.as_bytes(), false).expect("続けて保存できる");
    assert_eq!(String::from_utf8_lossy(&remote_read(&host, &path)), again);

    remote_rm(&host, &path);
}

#[test]
#[ignore = "実 SSH 先が要る（TAKO_REMOTE_E2E_HOST を指定して実行）"]
fn 開いた後にリモートが変わっていたら上書きしない() {
    let Some(host) = host() else {
        panic!("TAKO_REMOTE_E2E_HOST が未設定");
    };
    let home = remote_fs::connect(&host).expect("接続できる");
    let dir = write_dir(&home);
    let path = remote_fs::join_remote(&dir, "conflict.txt");
    remote_mkdir(&host, &dir);
    remote_seed(&host, &path, "AAAA\n");
    let _ = remote_fs::fetch_file(&host, &path, remote_fs::MAX_PREVIEW_BYTES).expect("取得できる");

    // 別経路でリモートを書き換える（**同じサイズ** = mtime とサイズだけの比較では
    // 見逃す形。#966 が内容そのものを突き合わせている根拠になる）
    remote_seed(&host, &path, "BBBB\n");

    let err = remote_fs::save_file(&host, &path, b"CCCC\n", false)
        .expect_err("競合しているのに上書きしてしまった");
    println!(
        "競合: {} / {} / {}",
        err.summary(),
        err.next_step(),
        err.detail
    );
    assert_eq!(err.kind, RemoteErrorKind::Conflict);
    assert!(
        err.detail.contains("サイズは同じ"),
        "同サイズの書き換えを内容で見抜いていない: {}",
        err.detail
    );
    // **上書きしていない**（相手の変更が生きている）
    assert_eq!(
        String::from_utf8_lossy(&remote_read(&host, &path)),
        "BBBB\n"
    );
    assert!(err.report().lines().count() >= 3, "理由と次の一手がある");

    // force なら上書きできる（ユーザーが選んだとき）
    remote_fs::save_file(&host, &path, b"CCCC\n", true).expect("force なら書ける");
    assert_eq!(
        String::from_utf8_lossy(&remote_read(&host, &path)),
        "CCCC\n"
    );

    // 消えている相手も競合として止まる（黙って作り直さない）
    remote_rm(&host, &path);
    let err = remote_fs::save_file(&host, &path, b"DDDD\n", false)
        .expect_err("消えているのに書いてしまった");
    println!("消失: {} / {}", err.summary(), err.detail);
    assert_eq!(err.kind, RemoteErrorKind::NotFound);
}

#[test]
#[ignore = "実 SSH 先が要る（TAKO_REMOTE_E2E_HOST を指定して実行）"]
fn 押し出せなかった保存は退避され再接続後に完遂できる() {
    let Some(host) = host() else {
        panic!("TAKO_REMOTE_E2E_HOST が未設定");
    };
    let home = remote_fs::connect(&host).expect("接続できる");
    let dir = write_dir(&home);
    let path = remote_fs::join_remote(&dir, "pending.txt");
    remote_mkdir(&host, &dir);
    remote_seed(&host, &path, "before\n");
    let _ = remote_fs::fetch_file(&host, &path, remote_fs::MAX_PREVIEW_BYTES).expect("取得できる");
    remote_fs::clear_pending(&host, &path);

    // 到達できない状態を作る: ControlMaster を落として **BatchMode で入れない**
    // ホスト名へ向ける（実際の切断と同じ「押し出せない」状態）
    let unreachable = "tako966-down.invalid";
    let body = b"written while offline\n";
    let err = remote_fs::save_file(unreachable, &path, body, false)
        .expect_err("到達できないのに成功してしまった");
    remote_fs::record_pending(unreachable, &path, body, &err).expect("退避できる");
    let pending = remote_fs::list_pending();
    let entry = pending
        .iter()
        .find(|e| e.host == unreachable && e.path == path)
        .expect("退避が残っている");
    println!(
        "退避: {} attempts={} kind={} error={}",
        entry.label(),
        entry.attempts,
        entry.kind,
        entry.error
    );
    assert_eq!(entry.size, body.len() as u64, "内容の大きさが残っていない");
    assert!(!entry.error.is_empty(), "理由が残っていない");
    assert_eq!(
        remote_fs::pending_body(unreachable, &path).as_deref(),
        Some(&body[..]),
        "**書きたかった内容**が残っていない = 無言で消えている"
    );

    // 再試行はやはり失敗する（相手が居ないまま）= 退避は消えない
    let err = remote_fs::push_pending(unreachable, &path, false).expect_err("届くはずがない");
    println!("再試行 1: {} / {}", err.summary(), err.detail);
    assert!(remote_fs::has_pending(unreachable, &path), "退避が消えた");
    let attempts = remote_fs::list_pending()
        .iter()
        .find(|e| e.host == unreachable && e.path == path)
        .map(|e| e.attempts)
        .unwrap_or(0);
    assert!(attempts >= 2, "試行回数が進んでいない: {attempts}");

    // **繋がる相手へ同じ内容を退避し直して押し出す**（= 再接続後に完遂する）
    remote_fs::clear_pending(unreachable, &path);
    remote_fs::record_pending(&host, &path, body, &err).expect("退避できる");
    assert!(remote_fs::has_pending(&host, &path));
    let report = remote_fs::push_pending(&host, &path, false)
        .unwrap_or_else(|e| panic!("再接続後に押し出せない: {} / {}", e.summary(), e.detail));
    println!("再試行 2: OK {} バイト", report.bytes);
    assert_eq!(
        String::from_utf8_lossy(&remote_read(&host, &path)),
        String::from_utf8_lossy(body),
        "再試行でリモートの実体が変わっていない"
    );
    assert!(
        !remote_fs::has_pending(&host, &path),
        "押し出せたのに退避が残っている"
    );

    remote_rm(&host, &path);
}

#[test]
#[ignore = "実 SSH 先が要る（TAKO_REMOTE_E2E_HOST を指定して実行）"]
fn 実行権のあるファイルは書き戻しても実行権が残る() {
    let Some(host) = host() else {
        panic!("TAKO_REMOTE_E2E_HOST が未設定");
    };
    let home = remote_fs::connect(&host).expect("接続できる");
    let dir = write_dir(&home);
    let path = remote_fs::join_remote(&dir, "run.sh");
    remote_mkdir(&host, &dir);
    remote_seed(&host, &path, "#!/bin/sh\necho before\n");
    remote_chmod(&host, &path, "755");
    let (_, stat) =
        remote_fs::fetch_file(&host, &path, remote_fs::MAX_PREVIEW_BYTES).expect("取得できる");
    let before = stat.expect("素性が読める").mode;
    if remote_fs::octal_mode(&before).is_none() {
        // Windows の sftp-server は権限欄が `*` 埋め = 戻す対象が無い（実測）
        println!("mode を持たない相手なのでスキップ: {before}");
        remote_rm(&host, &path);
        return;
    }
    remote_fs::save_file(&host, &path, b"#!/bin/sh\necho after\n", false).expect("書き戻せる");
    let after = remote_fs::stat_file(&host, &path)
        .expect("素性が読める")
        .mode;
    println!("mode: {before} → {after}");
    assert_eq!(
        before, after,
        "書き戻しで mode が変わった（put は元の mode を引き継がないので chmod で戻す）"
    );
    remote_rm(&host, &path);
}
