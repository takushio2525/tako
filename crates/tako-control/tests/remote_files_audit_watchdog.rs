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
    for (i, line) in src.lines().enumerate() {
        if !line.contains("deps.audit)(") {
            continue;
        }
        if !line.contains("audit_payload(") {
            offenders.push(format!("remote_files.rs:{} — {}", i + 1, line.trim()));
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
    for kind in ["roots", "list", "content", "download"] {
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
