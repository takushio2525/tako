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
    let local = remote_fs::fetch_file(&host, &file, remote_fs::MAX_PREVIEW_BYTES)
        .unwrap_or_else(|e| panic!("取得できない: {}\n{}", e.summary(), e.next_step()));
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
