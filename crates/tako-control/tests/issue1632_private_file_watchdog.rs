//! **#1632 の番犬**: remote の秘密（トークン・QR・状態ファイル）の保護が Windows で no-op へ
//! 戻っていない。
//!
//! ## なぜ止めるのか
//!
//! `remote.rs` の `restrict_permissions` / `write_secret_file` / `ensure_state_dir` は unix だけ
//! 0600 / 0700 にし、Windows は `#[cfg(not(unix))] { let _ = path; }` と素の `std::fs::write` で
//! **何もしていなかった**（置き場も中身も `%APPDATA%\tako` の継承まかせ。縮退の宣言も無い）。
//! #1632 で境界 `tako_core::platform::private_file`（B32）へ寄せ、Windows は継承を切った
//! 本人 + SYSTEM の DACL を当てるようにした。
//!
//! 実際に効いているかは Windows の CI が実ファイルの ACL を読み戻して確かめる
//! （`remote::tests::issue1632_*` / `platform::private_file::tests`）が、それは Windows のジョブでしか
//! 走らない。**macOS の手元でも 1 行の戻しを落とす**ために、構造をここで固定する:
//!
//! 1. [`境界のwindowsの腕がdaclを当てる`] — Windows の腕の 3 つの口（置き場 / 作る / 絞る）が
//!    DACL を当てる 1 実装（`apply`）を通り、その実装が継承を切る形（`PROTECTED_DACL_…`）で
//!    `SetNamedSecurityInfoW` を呼ぶ。no-op（`let _ = path; Ok(())`）へ戻すと落ちる
//! 2. [`remoteの秘密の書き手が境界を通る`] — `remote.rs` の 3 つの書き手が境界を呼び、
//!    `#[cfg]` の腕（片方の OS にしか効かない形）・素の `std::fs::write` へ戻っていない。
//!    QR の保存が絞りを通ることも見る
//!
//! 組む DACL の中身（本人 + SYSTEM だけ・`D:P`）は tako-core の単体
//! `platform::private_file::tests::本人とsystemだけの継承を切ったsddlを組む` が両 OS で固定する。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）
#[path = "common/production_range.rs"]
mod production_range;

const BOUNDARY: &str = "crates/tako-core/src/platform/private_file.rs";
const REMOTE: &str = "crates/tako-control/src/remote.rs";

/// Windows の腕の目印（綴りはこの 1 か所に持つ）
const WINDOWS_ARM: &str = "#[cfg(windows)]\nmod imp {";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番コードの「コードだけの眺め」（テストの item・コメント・文字列を空白へ潰す。
/// バイト長と行番号は保たれるので `file:line` の名指しがそのまま使える）
fn code(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    production_range::code_view_of(&production_range::production(&src, rel))
}

fn line_of(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())].matches('\n').count() + 1
}

/// `from` 以降で最初に現れる関数 1 本の本文（シグネチャから同インデントの `}` まで）。
/// 戻り値は `(シグネチャの行番号, 本文)`
fn fn_body(source: &str, rel: &str, from: usize, signature: &str) -> (usize, String) {
    let at = source[from..]
        .find(signature)
        .map(|i| from + i)
        .unwrap_or_else(|| {
            panic!(
                "{rel}:{} に `{}` が見つからない（#1632。改名したら番犬も直す）",
                line_of(source, from),
                signature.trim()
            )
        });
    let sig = signature.trim_start_matches('\n');
    let start = at + (signature.len() - sig.len());
    let indent = " ".repeat(sig.len() - sig.trim_start().len());
    let close = format!("\n{indent}}}");
    let end = source[start..]
        .find(&close)
        .map(|i| start + i)
        .unwrap_or(source.len());
    (line_of(source, start), source[start..end].to_string())
}

// --------------------------------------------------------------- 検査本体
// 注入テストからも呼ぶので、読むソースは引数で受ける

fn check_boundary_windows_arm(source: &str, rel: &str) {
    let arm = source.find(WINDOWS_ARM).unwrap_or_else(|| {
        panic!(
            "{rel}:1 Windows の腕（`#[cfg(windows)] mod imp`）が無い（#1632）。\n\
             `#[cfg(not(unix))]` の素通りへ戻すと、Windows では秘密のファイルが親の ACL を受け継ぐだけになる"
        )
    });
    for (signature, kind) in [
        ("\n    pub fn restrict_dir(", "置き場"),
        ("\n    pub fn create(", "作る"),
        ("\n    pub fn restrict(", "絞る"),
    ] {
        let (line, body) = fn_body(source, rel, arm, signature);
        assert!(
            body.contains("apply("),
            "{rel}:{line} Windows の腕の「{kind}」口が DACL を当てていない（#1632）。\n\
             `apply(…)` を通らない形（`let _ = path; Ok(())` など）は修正前の no-op と同じ。本文:\n{body}"
        );
    }
    let (line, apply) = fn_body(source, rel, arm, "\n    fn apply(");
    assert!(
        apply.contains("owner_only_sddl("),
        "{rel}:{line} `apply` が本人 + SYSTEM だけの DACL（`owner_only_sddl`）を組んでいない（#1632）。本文:\n{apply}"
    );
    let (line, set) = fn_body(source, rel, arm, "\n    fn set_dacl_of(");
    for needle in [
        "SetNamedSecurityInfoW(",
        "DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION",
    ] {
        assert!(
            set.contains(needle),
            "{rel}:{line} DACL の書き込みが `{needle}` を通っていない（#1632）。\n\
             `PROTECTED_DACL_SECURITY_INFORMATION` が無いと親（%APPDATA%）の ACE が受け継がれたまま残る。本文:\n{set}"
        );
    }
    assert!(
        apply.contains("set_dacl_of("),
        "{rel}:{line} `apply` が DACL の書き込み（`set_dacl_of`）を呼んでいない（#1632）。本文:\n{apply}"
    );
}

fn check_remote_writers(source: &str, rel: &str) {
    for (signature, call) in [
        ("\nfn ensure_state_dir(", "private_file::restrict_dir("),
        ("\nfn write_secret_file(", "private_file::create("),
        ("\nfn restrict_permissions(", "private_file::restrict("),
    ] {
        let (line, body) = fn_body(source, rel, 0, signature);
        assert!(
            body.contains(call),
            "{rel}:{line} `{}` が境界の `{call}…)` を通っていない（#1632）。\n\
             秘密の書き手は `tako_core::platform::private_file` の 1 実装で絞る（Windows は継承を切った DACL）。本文:\n{body}",
            signature.trim()
        );
        if let Some(at) = body.find("#[cfg(") {
            panic!(
                "{rel}:{} `{}` に `#[cfg]` の腕が戻っている（#1632）。\n\
                 OS ごとの差は境界 `platform::private_file` の中に閉じる（remote.rs に cfg を撒かない = #1616 の作法）。本文:\n{body}",
                line + body[..at].matches('\n').count(),
                signature.trim()
            );
        }
        for raw in ["std::fs::write(", "OpenOptions", "set_permissions("] {
            if let Some(at) = body.find(raw) {
                panic!(
                    "{rel}:{} `{}` が境界を通らずに `{raw}` で書いている（#1632）。\n\
                     素の書き込みは Windows で親の ACL を受け継ぐだけになる。本文:\n{body}",
                    line + body[..at].matches('\n').count(),
                    signature.trim()
                );
            }
        }
    }
    let (line, body) = fn_body(source, rel, 0, "\npub fn generate_qr_png(");
    let saved = body.find(".save(").unwrap_or_else(|| {
        panic!("{rel}:{line} `generate_qr_png` の保存（`.save(`）が見つからない（番犬も直す）")
    });
    let restricted = body.find("restrict_permissions(").unwrap_or_else(|| {
        panic!(
            "{rel}:{line} QR（トークン入り URL）の保存の後で絞っていない（#1632 / #104）。本文:\n{body}"
        )
    });
    assert!(
        saved < restricted,
        "{rel}:{line} QR を絞るのが保存より前にある（保存が新しく作ったファイルが絞られない）"
    );
}

// --------------------------------------------------------------- テスト

#[test]
fn 境界のwindowsの腕がdaclを当てる() {
    check_boundary_windows_arm(&code(BOUNDARY), BOUNDARY);
}

#[test]
fn remoteの秘密の書き手が境界を通る() {
    check_remote_writers(&code(REMOTE), REMOTE);
}

/// 番犬に検出力があること: 直前の 2 本が**#1632 以前の形を実際に落とす**。
///
/// 製品ソースを書き換える代わりに、戻した形の断片を同じ検査へ通す
/// （注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）
#[test]
fn 修正前の形を実際に落とす() {
    let boundary = code(BOUNDARY);
    let remote = code(REMOTE);
    let fails = |f: &dyn Fn()| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err();

    // ① 境界の Windows の腕の「絞る」口を no-op へ戻す
    let noop = boundary.replacen(
        "    pub fn restrict(path: &Path) -> io::Result<()> {\n        apply(path, Kind::File)\n    }",
        "    pub fn restrict(path: &Path) -> io::Result<()> {\n        let _ = path;\n        Ok(())\n    }",
        1,
    );
    assert_ne!(
        noop, boundary,
        "① の足場（Windows の restrict）が見つからない"
    );
    assert!(
        fails(&|| check_boundary_windows_arm(&noop, "注入")),
        "① Windows の腕の no-op を見逃した"
    );

    // ② 継承を切らずに DACL を書く（親の ACE が残る）
    let inherit = boundary.replacen(
        "DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION",
        "DACL_SECURITY_INFORMATION",
        1,
    );
    assert_ne!(inherit, boundary, "② の足場（保護の印）が見つからない");
    assert!(
        fails(&|| check_boundary_windows_arm(&inherit, "注入")),
        "② 継承を切らない形を見逃した"
    );

    // ③ Windows の腕ごと消して `#[cfg(not(unix))]` の素通りにする
    let gone = boundary.replacen(WINDOWS_ARM, "#[cfg(not(unix))]\nmod imp {", 1);
    assert!(
        fails(&|| check_boundary_windows_arm(&gone, "注入")),
        "③ Windows の腕の消失を見逃した"
    );

    // ④ #1632 以前の remote.rs そのもの（`#[cfg(not(unix))] { let _ = path; }`）
    let legacy = "
fn ensure_state_dir() -> io::Result<std::path::PathBuf> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    tako_core::platform::private_file::restrict_dir(&dir);
    Ok(dir)
}

fn write_secret_file(path: &std::path::Path, content: &str) -> io::Result<()> {
    let tmp = tako_core::platform::private_file::create(&tmp)?;
    Ok(())
}

fn restrict_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}
";
    assert!(
        fails(&|| check_remote_writers(legacy, "注入")),
        "④ #1632 以前の restrict_permissions を見逃した"
    );

    // ⑤ 書き手が素の std::fs::write へ戻る（境界の呼び出しを残したまま足しても落ちる）
    let raw = remote.replacen(
        "        f.sync_all()?;\n",
        "        f.sync_all()?;\n        std::fs::write(&tmp, content)?;\n",
        1,
    );
    assert_ne!(
        raw, remote,
        "⑤ の足場（write_secret_file の sync_all）が見つからない"
    );
    assert!(
        fails(&|| check_remote_writers(&raw, "注入")),
        "⑤ 素の std::fs::write を見逃した"
    );

    // ⑥ 現行の形は通る（常に落ちる無意味な番犬になっていないこと）
    check_boundary_windows_arm(&boundary, BOUNDARY);
    check_remote_writers(&remote, REMOTE);
}
