//! ファイル API の監査ログにパスが漏れていないかの番犬（#1079）
//!
//! `/api/files*` は**ユーザーのファイルの中身と名前**を扱う。監査ログ
//! （`<state_dir>/audit.log`）はペイン内容と同基準で「何をどれだけ」しか残さない
//! （#287 P2-2。upload API が確立した規約）。
//!
//! 単体テスト `audit_payloadにパスが混ざらない` は**今の実装**が漏らさないことを見るが、
//! 後から `audit` の呼び出しを増やしたときに `audit_payload` を経由せず
//! 生のパスを載せる書き方は止められない。この番犬はソースを走査して
//! 「`remote_files.rs` の監査呼び出しは `audit_payload` 経由だけ」を機械検証する。
//!
//! 併せて、`audit_payload` に**あらゆる形のパスを食わせても**出力に現れないことを
//! 実際に呼んで確かめる（テキスト検査だけだと呼び出し側の作り込みを見逃す）。

use std::path::{Path, PathBuf};

fn remote_files_source() -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("remote_files.rs");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// コメント行を落とす（doc コメント中の見本で誤検知しないため）
fn without_comments(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 本文（`#[cfg(test)] mod tests` より前）だけを見る
fn production_part(src: &str) -> String {
    match src.find("#[cfg(test)]") {
        Some(i) => src[..i].to_string(),
        None => src.to_string(),
    }
}

#[test]
fn 監査の呼び出しはaudit_payload経由だけ() {
    let src = production_part(&without_comments(&remote_files_source()));
    let mut offenders: Vec<String> = Vec::new();
    // **行ではなく文**を見る: `cargo fmt` が引数を改行へ折ると
    // 「同じ行に audit_payload( があるか」では偽の違反になる（#1023 の番犬の教訓）
    for (pos, _) in src.match_indices("deps.audit)(") {
        let stmt_end = src[pos..].find(';').map(|i| pos + i).unwrap_or(src.len());
        let stmt = &src[pos..stmt_end];
        if !stmt.contains("audit_payload(") {
            let line_no = src[..pos].matches('\n').count() + 1;
            offenders.push(format!(
                "remote_files.rs:{} — {}",
                line_no,
                stmt.replace('\n', " ").trim()
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "監査への追記は audit_payload( を通すこと（パスを載せないため）:\n{}",
        offenders.join("\n")
    );
    // 呼び出しが 1 つも無い = 走査が空振りしている（規約が形骸化する）
    let calls = src.matches("deps.audit)(").count();
    assert!(
        calls >= 3,
        "監査呼び出しが見つからない（走査が空振り）: {calls}"
    );
}

#[test]
fn audit_payloadはどんなパスを渡しても漏らさない() {
    // 実パスに現れうる形を kind へ流し込んでも、パスらしき断片は出力に残らない。
    // kind は呼び出し側の固定文字列だが、将来ここへ変数が入る改変を検出する
    // #1084 / #1085 で増えた種別も含める（新しい呼び出し口が検査から漏れない）
    for kind in [
        "roots",
        "list",
        "content",
        "download",
        "ssh_list",
        "ssh_content",
        "ssh_download",
        "pending",
        "write",
        "write_denied",
        "push",
        "push_failed",
    ] {
        let payload = tako_control::remote_files::audit_payload(kind, 1024, 3);
        let text = payload.to_string();
        for forbidden in ['/', '\\', '~'] {
            assert!(
                !text.contains(forbidden),
                "監査 JSON に {forbidden:?} が出た: {text}"
            );
        }
        let keys: Vec<String> = payload
            .as_object()
            .expect("オブジェクト")
            .keys()
            .cloned()
            .collect();
        for k in &keys {
            assert!(
                tako_control::remote_files::AUDIT_KEYS.contains(&k.as_str()),
                "許可リストに無い監査キー: {k}（AUDIT_KEYS を意図して増やしたなら番犬も直す）"
            );
        }
    }
}

#[test]
fn 監査の種別は固定文字列だけ() {
    // 上のテストのコメントが言っている「将来ここへ変数が入る改変」を**実際に**検出する。
    // `audit_payload(kind, ...)` の第 1 引数が変数になると、パスやファイル名が
    // 種別として監査ログへ流れうる（#287 P2-2 の規約が静かに崩れる）
    let src = production_part(&without_comments(&remote_files_source()));
    let mut offenders: Vec<String> = Vec::new();
    for (pos, _) in src.match_indices("audit_payload(") {
        // 定義そのもの（`pub fn audit_payload(kind: &str, ...)`）は呼び出しではない
        if src[..pos].trim_end().ends_with("fn") {
            continue;
        }
        let rest = &src[pos + "audit_payload(".len()..];
        let arg = rest.split(',').next().unwrap_or("").trim();
        // 文字列リテラル（`"..."`）だけを許す
        if !(arg.starts_with('"') && arg.ends_with('"') && arg.len() >= 2) {
            let line_no = src[..pos].matches('\n').count() + 1;
            offenders.push(format!("remote_files.rs:{line_no} — 第 1 引数が {arg}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "監査の種別は固定文字列で書くこと:\n{}",
        offenders.join("\n")
    );
    // 走査が空振りしていないこと
    assert!(
        src.matches("audit_payload(").count() >= 8,
        "audit_payload の呼び出しが見つからない（走査が空振り）"
    );
}

#[test]
fn ファイルapiの応答はキャッシュさせない() {
    // ファイルの中身は機密。共有プロキシや PWA のキャッシュへ残さない
    let src = production_part(&without_comments(&remote_files_source()));
    let responders = src.matches("request.respond(resp)").count();
    let no_store = src.matches("no-store, private").count();
    assert!(
        responders > 0 && no_store >= responders,
        "応答経路 {responders} 件に対して no-store の付与が {no_store} 件しかない"
    );
}

#[test]
fn ファイルapiは自分でsshやsftpを起こさない() {
    // #1085 受け入れ条件 3「スマホは SSH 鍵に触らない」を**構造で**固定する。
    //
    // 2 ホップの後段（SFTP）は tako app 側が `<data_dir>/ssh/` の ControlMaster で
    // 張るもので、daemon は IPC で proxy するだけ。daemon 側に接続を生やすと
    // ①鍵・known_hosts・2FA の扱いが 2 箇所になる
    // ②#966 のキャッシュ / 退避 / 競合検知の基準が app と食い違う
    // のどちらも起きるので、**呼んでよいのは純粋な文字列関数だけ**に閉じる
    let src = production_part(&without_comments(&remote_files_source()));

    // 接続・取得・書き戻し・器の起動はどれも呼ばない
    for forbidden in [
        "remote_fs::connect",
        "remote_fs::ensure_master",
        "remote_fs::fetch_file",
        "remote_fs::save_file",
        "remote_fs::list_dir",
        "remote_fs::stat_file",
        "remote_fs::push_pending",
        "remote_fs::sftp_bin",
        "remote_fs::ssh_bin",
        "Command::new",
        "process::Command",
    ] {
        assert!(
            !src.contains(forbidden),
            "ファイル API が {forbidden} を直接呼んでいる（SSH は app 側の 1 実装に閉じる）"
        );
    }

    // 使ってよいのは純粋な文字列関数だけ（増やすときは理由をここへ）
    const ALLOWED: &[&str] = &["base_name", "join_remote"];
    let mut used: Vec<String> = Vec::new();
    for (pos, _) in src.match_indices("remote_fs::") {
        let rest = &src[pos + "remote_fs::".len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !ALLOWED.contains(&name.as_str()) {
            let line_no = src[..pos].matches('\n').count() + 1;
            used.push(format!("remote_files.rs:{line_no} — remote_fs::{name}"));
        }
    }
    assert!(
        used.is_empty(),
        "純粋な文字列関数以外の remote_fs を使っている:\n{}",
        used.join("\n")
    );
    // 走査が空振りしていないこと（`remote_fs::` の呼び出しが実際にある）
    assert!(
        src.matches("remote_fs::").count() >= 2,
        "remote_fs の呼び出しが見つからない（走査が空振り）"
    );
}
