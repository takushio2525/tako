//! **#841 の番犬**: ループバック TCP の `X-Forwarded-For` は、接続元プロセスを
//! 検証してからでないと identity として読めない。
//!
//! ## なぜ止めるのか
//!
//! remote デーモンのローカル待ち受けは既定がループバック TCP（#1038）。TCP には
//! UDS 0600 のような「同一ユーザー限定」がカーネルに無いので、**同一マシンの
//! 別プロセス**が `127.0.0.1:<port>` へ繋いで、ペアリング済み端末の tailnet IP を
//! `X-Forwarded-For` に入れるだけで層①を通れた。role 次第で実質シェルアクセスになる。
//!
//! 実測（修正前 = `TAKO_841_LEGACY=1` で再現できる）: 偽の XFF / XFH を付けた
//! ローカルプロセスからの `/api/me` が **200**、`/api/v2/panes`（role 認可あり）も
//! 通った。検証を入れると同じリクエストが **403** になる
//! （`remote::tests::偽のxffを付けたローカルプロセスは接続元検証で拒否される`）。
//!
//! ## 何を固定するか
//!
//! 規則そのもの（何を通し何を弾くか）は単体テストが持つ。ここが止めるのは**構造**で、
//! 1 行で戻せてしまう形が 5 通りある:
//!
//! 1. [`xffを読む入口は1実装だけ`] — 認可側が header を直読みして検証を迂回する
//! 2. [`forwarded_identityは読む前に接続元を検証する`] — 検証の呼び出しが消える / 順序が入れ替わる
//! 3. [`verify_peerはエンドポイントの形で分岐する`] — UDS にも検証を通す / TCP の検証を素通しにする
//! 4. [`verify_peerは2つのゲートを両方通す`] — 所有者ゲートか実行ファイルゲートが消える
//! 5. [`名前の判定は設置場所に依らない`] — 実行ファイルの**パス**を焼き込む（受け入れ条件 4）
//!
//! 併せて、拒否の記録が理由コードだけであること（[`拒否の記録は理由コードだけを残す`]）と、
//! 状態が `tako remote status` から読めること（[`検証の状態はstatusに出る`]）も見る。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）
#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const ENDPOINT: &str = "crates/tako-control/src/platform/local_endpoint.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn production(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    production_range::production(&src, rel)
}

/// 関数 1 本の本文（シグネチャ行から同インデントの `}` まで）。
/// 戻り値は `(シグネチャ行の行番号, コメントを落とした本文)`
fn fn_body(source: &str, rel: &str, signature: &str) -> (usize, String) {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{rel} に `{signature}` が見つからない（改名したら番犬も直す）"));
    let head_line = source[..start].lines().count() + 1;
    let sig_line = signature.trim_start_matches('\n');
    let indent = " ".repeat(sig_line.len() - sig_line.trim_start().len());
    let close = format!("\n{indent}}}");
    let end = source[start..]
        .find(&close)
        .map(|i| start + i)
        .unwrap_or(source.len());
    let body = source[start..end]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    (head_line, body)
}

/// `X-Forwarded-For` を読んでよい関数（`check_admin` は**値を使わず**
/// 「付いていたら拒否」の判定にだけ使うので別扱い）
const XFF_READERS: [&str; 2] = ["forwarded_identity", "check_admin"];

/// 検査①: XFF を読む場所が増えていないか（読む場所 = 検証を迂回できる場所）
fn check_single_xff_entry(source: &str, rel: &str) {
    let mut current = String::new();
    let mut offenders: Vec<String> = Vec::new();
    let mut entry_found = false;
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("fn ")
            .or_else(|| trimmed.strip_prefix("pub fn "))
            .or_else(|| trimmed.strip_prefix("pub(crate) fn "))
            .or_else(|| trimmed.strip_prefix("pub(super) fn "))
        {
            current = rest
                .split(['(', '<', ' '])
                .next()
                .unwrap_or_default()
                .to_string();
        }
        if !line.contains("\"x-forwarded-for\"") {
            continue;
        }
        if current == "forwarded_identity" {
            entry_found = true;
        }
        if !XFF_READERS.contains(&current.as_str()) {
            offenders.push(format!("{rel}:{} （{current}）", i + 1));
        }
    }
    assert!(
        offenders.is_empty(),
        "{}\n`X-Forwarded-For` を読む場所が増えている（#841）。\
         読める場所 = 接続元検証を迂回できる場所なので、入口は `forwarded_identity` の\n\
         1 実装だけにすること（`check_admin` は値を使わず「付いていたら拒否」の判定のみ）",
        offenders.join("\n")
    );
    assert!(
        entry_found,
        "{rel} の `forwarded_identity` が XFF を読んでいない（入口が別へ移ったら番犬も直す）"
    );
}

/// 検査②: 入口は**読む前に**接続元を検証し、拒否ならヘッダを渡さない
fn check_entry_verifies_first(source: &str, rel: &str) {
    let (line, body) = fn_body(source, rel, "\nfn forwarded_identity(");
    let verify = body
        .find("local_endpoint::verify_peer(")
        .unwrap_or_else(|| {
            panic!(
                "{rel}:{line} `forwarded_identity` が `verify_peer` を呼んでいない（#841）。\
             XFF を identity として読む前に接続元プロセスを検証すること"
            )
        });
    let host = body.find("\"x-forwarded-host\"").unwrap_or_else(|| {
        panic!("{rel}:{line} `forwarded_identity` が `x-forwarded-host` を読んでいない")
    });
    assert!(
        verify < host,
        "{rel}:{line} 接続元の検証より先にヘッダを読んでいる（#841）。\
         検証を通ってから初めてヘッダを identity として渡すこと"
    );
    assert!(
        body.contains(".reject()") && body.contains("return Err(("),
        "{rel}:{line} 拒否を握り潰している（#841）。\
         `verify_peer` が拒否したらヘッダを返さず 403 で落とすこと"
    );
    assert!(
        body.contains("record_peer_reject"),
        "{rel}:{line} 拒否を記録していない（#841）。理由コードを残さないと\
         「無言で 403」になって原因を追えない"
    );
}

/// 検査③: 認可の 2 経路がどちらも入口を通る
fn check_auth_paths_use_entry(source: &str, rel: &str) {
    for sig in ["\nfn authorize_device(", "\nfn identify_tailnet("] {
        let (line, body) = fn_body(source, rel, sig);
        assert!(
            body.contains("forwarded_identity("),
            "{rel}:{line} `{}` が入口を通っていない（#841）",
            sig.trim()
        );
        assert!(
            !body.contains("\"x-forwarded-for\""),
            "{rel}:{line} `{}` がヘッダを直読みしている（#841）。\
             直読みは接続元検証を丸ごと迂回する",
            sig.trim()
        );
    }
}

/// 検査④: 分岐は**プラットフォームではなくエンドポイントの形**（受け入れ条件 4）
fn check_endpoint_shape_branch(source: &str, rel: &str) {
    let (line, body) = fn_body(source, rel, "\npub fn verify_peer(");
    let shape = body.find("let Endpoint::Loopback(").unwrap_or_else(|| {
        panic!(
            "{rel}:{line} `verify_peer` がエンドポイントの形で分岐していない（#841）。\
             `TAKO_REMOTE_ENDPOINT=unix` の UDS 経路は 0600 でカーネルが同一ユーザーに\
             限定するので検証を通さない"
        )
    });
    let not_required = body.find("PeerTrust::NotRequired").unwrap_or_else(|| {
        panic!("{rel}:{line} UDS 経路の `PeerTrust::NotRequired` が消えている（#841）")
    });
    assert!(
        shape < not_required,
        "{rel}:{line} UDS の素通しが形の判定より前にある（#841）。\
         ループバック TCP まで素通しになる"
    );
    assert!(
        !body.contains("cfg!(windows)") && !body.contains("target_os"),
        "{rel}:{line} `verify_peer` がプラットフォームで分岐している（#841）。\
         分岐の軸はエンドポイントの形（能力差は `owner_check_kind` が名乗る）"
    );
}

/// 検査⑤: 2 つのゲートが両方在り、材料が欠けたら拒否側へ倒れる
fn check_two_gates(source: &str, rel: &str) {
    let (line, body) = fn_body(source, rel, "\npub fn verify_peer(");
    assert!(
        body.contains("procinfo::current_uid()") && body.contains("PeerReject::OwnerUntrusted"),
        "{rel}:{line} 所有者ゲートが消えている（#841）。\
         ソケットの所有ユーザーはカーネルが答える事実で、名前と違って詐称できない。\
         これが UDS 0600 の「別 OS ユーザーを排除する」性質を戻している唯一の段"
    );
    assert!(
        body.contains("procinfo::image_path(") && body.contains("looks_like_tailscale_daemon("),
        "{rel}:{line} 実行ファイルゲートが消えている（#841）"
    );
    for reject in [
        "PeerReject::NoPeerAddr",
        "PeerReject::Unresolved",
        "PeerReject::ImageUnknown",
        "PeerReject::NotTailscaleDaemon",
    ] {
        assert!(
            body.contains(reject),
            "{rel}:{line} `{reject}` への分岐が消えている（#841）。\
             材料が 1 つでも欠けたら拒否側へ倒す（接続を即閉じて素通りさせない）"
        );
    }
    for cache in ["OnceLock", "static ", "lazy_static", "thread_local"] {
        assert!(
            !body.contains(cache),
            "{rel}:{line} 判定をキャッシュしている（`{cache}`。#841）。\
             `image_path` は「いまのファイル名」を返す（#936）ので、pid が別の実行ファイルへ\
             置き換わった瞬間から結果が変わる。使い回すとその窓で古い判定が通る"
        );
    }
    assert_eq!(
        body.matches("PeerTrust::Trusted").count(),
        1,
        "{rel}:{line} `PeerTrust::Trusted` を返す場所が 1 つではない（#841）。\
         信頼は 2 つのゲートを両方通った最後の 1 箇所からしか出さない"
    );
}

/// 検査⑥: 名前の判定にパスを焼き込まない（受け入れ条件 4）
fn check_name_rule_is_path_free(source: &str, rel: &str) {
    let (line, body) = fn_body(source, rel, "\npub fn looks_like_tailscale_daemon(");
    for needle in ["/usr", "/opt", "/Applications", "Program Files", "C:\\"] {
        assert!(
            !body.contains(needle),
            "{rel}:{line} 実行ファイルの**設置場所**を判定に使っている（`{needle}`。#841）。\
             版番号・UUID・インストール先はすべて環境依存なので、名前だけで判定すること"
        );
    }
    assert!(
        body.contains("tailscale"),
        "{rel}:{line} 名前の判定が消えている（#841）"
    );
}

#[test]
fn xffを読む入口は1実装だけ() {
    check_single_xff_entry(&production(REMOTE), REMOTE);
}

#[test]
fn forwarded_identityは読む前に接続元を検証する() {
    check_entry_verifies_first(&production(REMOTE), REMOTE);
}

#[test]
fn 認可の2経路はどちらも入口を通る() {
    check_auth_paths_use_entry(&production(REMOTE), REMOTE);
}

#[test]
fn verify_peerはエンドポイントの形で分岐する() {
    check_endpoint_shape_branch(&production(ENDPOINT), ENDPOINT);
}

#[test]
fn verify_peerは2つのゲートを両方通す() {
    check_two_gates(&production(ENDPOINT), ENDPOINT);
}

#[test]
fn 名前の判定は設置場所に依らない() {
    check_name_rule_is_path_free(&production(ENDPOINT), ENDPOINT);
}

#[test]
fn 拒否の記録は理由コードだけを残す() {
    let source = production(REMOTE);
    let (line, body) = fn_body(&source, REMOTE, "\n    fn record_peer_reject(");
    assert!(
        body.contains("persist_log") && body.contains("reject.code()"),
        "{REMOTE}:{line} 拒否の理由コードを診断ログへ残していない（#841）"
    );
    for leak in ["forwarded", "admin_token", "remote_addr", "peer.ip()"] {
        assert!(
            !body.contains(leak),
            "{REMOTE}:{line} 診断ログへ接続元の詳細（`{leak}`）を載せようとしている（#841）。\
             残すのは理由コードと件数だけ（AGENTS.md の絶対ルール）"
        );
    }
}

#[test]
fn 検証の状態はstatusに出る() {
    let source = production(REMOTE);
    assert!(
        source.contains("status[\"peer_verification\"]"),
        "{REMOTE} `tako remote status` に接続元検証の状態が出ていない（#841）。\
         CLI / MCP の両方がこの JSON を読む"
    );
    assert!(
        source.contains("fn read_peer_guard()") && source.contains("fn write_peer_guard("),
        "{REMOTE} 接続元検証の状態ファイルの読み書きが消えている（#841）"
    );
}

/// 検査に検出力があること自体を固定する（#841 で実際に在った形と、
/// 1 行で戻せてしまう形を注入する）
#[test]
fn 検査は修正前の形を検出する() {
    // ① 検証を素通り（修正前の forwarded_identity そのもの）
    let bypass = "\nfn forwarded_identity(\n\
        \x20   ctx: &DaemonCtx,\n\
        \x20   request: &tiny_http::Request,\n\
        ) -> Result<(Option<String>, Option<String>), (u16, String)> {\n\
        \x20   let forwarded = header_value(request, \"x-forwarded-for\");\n\
        \x20   Ok((forwarded, header_value(request, \"x-forwarded-host\")))\n\
        }\n";
    let caught = std::panic::catch_unwind(|| check_entry_verifies_first(bypass, "注入"));
    assert!(caught.is_err(), "① 検証の素通りを検出できていない");

    // ② 検証はするが拒否を握り潰す
    let swallowed = "\nfn forwarded_identity(\n\
        \x20   ctx: &DaemonCtx,\n\
        \x20   request: &tiny_http::Request,\n\
        ) -> Result<(Option<String>, Option<String>), (u16, String)> {\n\
        \x20   let forwarded = header_value(request, \"x-forwarded-for\");\n\
        \x20   let trust = local_endpoint::verify_peer(&ctx.endpoint, None);\n\
        \x20   let _ = trust;\n\
        \x20   Ok((forwarded, header_value(request, \"x-forwarded-host\")))\n\
        }\n";
    let caught = std::panic::catch_unwind(|| check_entry_verifies_first(swallowed, "注入"));
    assert!(caught.is_err(), "② 拒否の握り潰しを検出できていない");

    // ③ 認可側がヘッダを直読みして入口を迂回する（修正前の authorize_device）
    let direct = "\nfn authorize_device(\n\
        \x20   ctx: &DaemonCtx,\n\
        ) -> AuthDecision {\n\
        \x20   let forwarded = header_value(request, \"x-forwarded-for\");\n\
        \x20   AuthDecision::Rejected(401, String::new())\n\
        }\n\
        \nfn identify_tailnet(\n\
        \x20   ctx: &DaemonCtx,\n\
        ) -> Result<WhoisInfo, (u16, String)> {\n\
        \x20   let (forwarded, forwarded_host) = forwarded_identity(ctx, request)?;\n\
        \x20   Err((401, String::new()))\n\
        }\n";
    let caught = std::panic::catch_unwind(|| check_auth_paths_use_entry(direct, "注入"));
    assert!(caught.is_err(), "③ ヘッダの直読みを検出できていない");
    let caught = std::panic::catch_unwind(|| check_single_xff_entry(direct, "注入"));
    assert!(caught.is_err(), "③' 入口が増えたことを検出できていない");

    // ④ UDS にも検証を通す（分岐が消える）
    let no_shape =
        "\npub fn verify_peer(endpoint: &Endpoint, peer: Option<SocketAddr>) -> PeerTrust {\n\
        \x20   let Some(peer) = peer else {\n\
        \x20       return PeerTrust::Rejected(PeerReject::NoPeerAddr);\n\
        \x20   };\n\
        \x20   PeerTrust::Trusted\n\
        }\n";
    let caught = std::panic::catch_unwind(|| check_endpoint_shape_branch(no_shape, "注入"));
    assert!(
        caught.is_err(),
        "④ エンドポイントの形の分岐が消えたのを検出できていない"
    );

    // ⑤ プラットフォームで分岐する（Windows だけ素通しにする形）
    let by_platform =
        "\npub fn verify_peer(endpoint: &Endpoint, peer: Option<SocketAddr>) -> PeerTrust {\n\
        \x20   let Endpoint::Loopback(local_port) = endpoint else {\n\
        \x20       return PeerTrust::NotRequired;\n\
        \x20   };\n\
        \x20   if cfg!(windows) {\n\
        \x20       return PeerTrust::NotRequired;\n\
        \x20   }\n\
        \x20   PeerTrust::Trusted\n\
        }\n";
    let caught = std::panic::catch_unwind(|| check_endpoint_shape_branch(by_platform, "注入"));
    assert!(caught.is_err(), "⑤ プラットフォーム分岐を検出できていない");

    // ⑥ 所有者ゲートを外す（名前だけで信じる = 別ユーザーの偽 tailscaled が通る）
    let name_only =
        "\npub fn verify_peer(endpoint: &Endpoint, peer: Option<SocketAddr>) -> PeerTrust {\n\
        \x20   let Endpoint::Loopback(local_port) = endpoint else {\n\
        \x20       return PeerTrust::NotRequired;\n\
        \x20   };\n\
        \x20   let Some(peer) = peer else {\n\
        \x20       return PeerTrust::Rejected(PeerReject::NoPeerAddr);\n\
        \x20   };\n\
        \x20   let Some(found) = procinfo::loopback_tcp_peer(peer.port(), *local_port) else {\n\
        \x20       return PeerTrust::Rejected(PeerReject::Unresolved);\n\
        \x20   };\n\
        \x20   let Some(path) = procinfo::image_path(found.pid) else {\n\
        \x20       return PeerTrust::Rejected(PeerReject::ImageUnknown);\n\
        \x20   };\n\
        \x20   if !looks_like_tailscale_daemon(&name) {\n\
        \x20       return PeerTrust::Rejected(PeerReject::NotTailscaleDaemon);\n\
        \x20   }\n\
        \x20   PeerTrust::Trusted\n\
        }\n";
    let caught = std::panic::catch_unwind(|| check_two_gates(name_only, "注入"));
    assert!(caught.is_err(), "⑥ 所有者ゲートの欠落を検出できていない");

    // ⑦ 判定をパスへ戻す（受け入れ条件 4 の違反）
    let by_path = "\npub fn looks_like_tailscale_daemon(file_name: &str) -> bool {\n\
        \x20   file_name.starts_with(\"C:\\\\Program Files\\\\Tailscale\\\\\")\n\
        }\n";
    let caught = std::panic::catch_unwind(|| check_name_rule_is_path_free(by_path, "注入"));
    assert!(caught.is_err(), "⑦ パスの焼き込みを検出できていない");
}
