//! remote_isolated_serve — 隔離中（`TAKO_ISOLATED`）は本物の tailscale の serve を張らない（#2619）
//!
//! tailscale の serve の設定は**この機械のノード単位の状態**で、隔離の砂場（data dir・発見の置き場・
//! tmux のソケット）の外にある。砂場の CLI / 隔離 GUI から `tako remote start`（CLI / 起動ボタン /
//! MCP `tako_remote_start`）を打つと、daemon は既定の tailscale へ serve を張っていた
//! （11/01 の観察: 砂場から既定のノード = GUI 版の Tailscale へ張った）。
//! 砂場の向くノードが本番の公開と同じなら、起動時の張り替えの判定（`tailscale::is_reclaimable_target`）が
//! 本番の待ち受け（`http://127.0.0.1:<port>`）を「前回のポートの残骸」と読んで張り替える = 本番の公開を奪う。
//!
//! 門は [`allows`]（純関数）の 1 実装。**隔離でない（本番の）経路は常に通す**（挙動を変えない）。
//! 隔離中に serve へ書いてよいのは次のどれかのときだけ:
//!
//! - `TAKO_REMOTE_TEST_MODE=1` — serve を張らない検証モード（daemon はそもそも serve へ書かない）
//! - `TAKO_TAILSCALE_BIN` が偽物を指す — 検証用の tailscale の差し替え（`scripts/test-remote-*.sh` の
//!   偽 tailscale）。serve は偽物の中の状態なので機械に触らない。**素の名前・既定の候補と同じファイル
//!   （= 本物）を指す値は差し替えと見なさない**（[`overrides_to_fake`]）
//! - `TAKO_REMOTE_ISOLATED_SERVE=1` — 本物の serve まで確かめる明示の opt-in（[`OPT_IN_ENV`]）
//!
//! 呼ぶのは serve を書く入口: 起動の 1 実装 `remote::spawn_daemon`（子を起こす前に断る = 呼び手の
//! 表示言語）・serve を張る関数 `remote::establish_tailscale_serve`（daemon の中の最後の守り =
//! `tako remote serve` を直に起こした場合）・UDS の `remote_setup::configure_serve`。
//! 稼働中の張り直し（#1049 の `TailscaleServeOps::assert_target`）は起動時の門を通った daemon の中でしか
//! 回らない（env は起動時のまま）。番犬は `tests/issue2619_isolated_remote_serve_watchdog.rs`。
//!
//! A/B は `TAKO_2619_LEGACY=1`（[`LEGACY_ENV`]。隔離中でも従来どおり既定の tailscale へ張る）。

use std::path::{Path, PathBuf};

/// 隔離中でも本物の tailscale の serve を張る明示の opt-in
pub const OPT_IN_ENV: &str = "TAKO_REMOTE_ISOLATED_SERVE";

/// A/B: #2619 前へ戻す（隔離中でも既定の tailscale へ serve を張る）
pub const LEGACY_ENV: &str = "TAKO_2619_LEGACY";

/// 門の入力（env から読んだもの。判定は [`allows`] が持つ）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inputs {
    /// `TAKO_ISOLATED` が真
    pub isolated: bool,
    /// `TAKO_REMOTE_TEST_MODE=1`（serve を張らない検証モード）
    pub test_mode: bool,
    /// `TAKO_TAILSCALE_BIN` が偽物を指す（[`overrides_to_fake`]）
    pub fake_tailscale: bool,
    /// [`OPT_IN_ENV`] が真
    pub opt_in: bool,
    /// [`LEGACY_ENV`] が真
    pub legacy: bool,
}

/// serve へ書いてよいか（純関数）。隔離でなければ常に真 = 本番の経路は変えない
pub fn allows(i: Inputs) -> bool {
    !i.isolated || i.test_mode || i.fake_tailscale || i.opt_in || i.legacy
}

/// `TAKO_TAILSCALE_BIN` の値が本物以外（検証用の偽物）を指しているか（純関数）。
///
/// - 未設定・空は差し替えていない
/// - 素の名前（`tailscale` など区切りの無い値）は PATH で引く = 本物を起こしうるので差し替えと見なさない
/// - パスは `canon` で解決し、本物の候補（`real` = 既定の候補を同じ `canon` で解決したもの）と
///   同じファイルなら差し替えと見なさない（`/opt/homebrew/bin/tailscale` を名指しても本物のまま）
pub fn overrides_to_fake(
    bin: Option<&str>,
    canon: impl Fn(&Path) -> PathBuf,
    real: &[PathBuf],
) -> bool {
    let Some(bin) = bin.map(str::trim).filter(|b| !b.is_empty()) else {
        return false;
    };
    let path = Path::new(bin);
    if !path.is_absolute() && path.components().count() == 1 {
        return false;
    }
    let resolved = canon(path);
    !real.contains(&resolved)
}

/// パスを解決する（シンボリックリンクを辿る。解決できないパス = 存在しない偽物はそのまま）
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1" | "true" | "on")
    )
}

/// このプロセスの env から門の入力を読む
pub fn current_inputs() -> Inputs {
    let isolated =
        tako_core::tmux_cleanup::is_isolated_value(std::env::var("TAKO_ISOLATED").ok().as_deref());
    // 隔離でなければ残りは答えを変えない（本番の経路でファイルを解決しに行かない）
    if !isolated {
        return Inputs {
            isolated,
            test_mode: false,
            fake_tailscale: false,
            opt_in: false,
            legacy: false,
        };
    }
    let real: Vec<PathBuf> = crate::tailscale::real_candidate_paths()
        .iter()
        .map(|p| canonical(p))
        .collect();
    Inputs {
        isolated,
        test_mode: std::env::var("TAKO_REMOTE_TEST_MODE").is_ok_and(|v| v == "1"),
        fake_tailscale: overrides_to_fake(
            std::env::var("TAKO_TAILSCALE_BIN").ok().as_deref(),
            canonical,
            &real,
        ),
        opt_in: truthy(OPT_IN_ENV),
        legacy: truthy(LEGACY_ENV),
    }
}

/// serve を書く入口が最初に通す門。隔離中で通す理由が無ければ日英の断り
pub fn check() -> Result<(), crate::RemoteRefusal> {
    if allows(current_inputs()) {
        Ok(())
    } else {
        Err(crate::RemoteRefusal::IsolatedServe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(isolated: bool) -> Inputs {
        Inputs {
            isolated,
            test_mode: false,
            fake_tailscale: false,
            opt_in: false,
            legacy: false,
        }
    }

    /// 本番（隔離でない）の経路は、ほかの入力がどうであれ常に通す = 挙動を変えない
    #[test]
    fn 隔離でなければ常に通す() {
        for bits in 0u8..16 {
            let i = Inputs {
                isolated: false,
                test_mode: bits & 1 != 0,
                fake_tailscale: bits & 2 != 0,
                opt_in: bits & 4 != 0,
                legacy: bits & 8 != 0,
            };
            assert!(allows(i), "{i:?}");
        }
    }

    /// 隔離中は理由が 1 つも無ければ断り、どれか 1 つで通す
    #[test]
    fn 隔離中は理由が無ければ断る() {
        assert!(!allows(inputs(true)));
        let each: [fn(&mut Inputs); 4] = [
            |i| i.test_mode = true,
            |i| i.fake_tailscale = true,
            |i| i.opt_in = true,
            |i| i.legacy = true,
        ];
        for (n, set) in each.iter().enumerate() {
            let mut i = inputs(true);
            set(&mut i);
            assert!(allows(i), "{n}: {i:?}");
        }
    }

    #[test]
    fn 偽物の差し替えだけを差し替えと読む() {
        let real = vec![
            PathBuf::from("/opt/homebrew/Cellar/tailscale/1/bin/tailscale"),
            PathBuf::from("/usr/local/bin/tailscale"),
        ];
        // 名前 → 解決先の表（シンボリックリンクの代わり）
        let canon = |p: &Path| -> PathBuf {
            match p.to_str() {
                Some("/opt/homebrew/bin/tailscale") => {
                    PathBuf::from("/opt/homebrew/Cellar/tailscale/1/bin/tailscale")
                }
                _ => p.to_path_buf(),
            }
        };
        // 差し替えていない
        assert!(!overrides_to_fake(None, canon, &real));
        assert!(!overrides_to_fake(Some(""), canon, &real));
        assert!(!overrides_to_fake(Some("  "), canon, &real));
        // 素の名前は PATH で本物を引きうる
        assert!(!overrides_to_fake(Some("tailscale"), canon, &real));
        assert!(!overrides_to_fake(Some("fake-tailscale"), canon, &real));
        // 本物の候補・本物へ解決されるリンク
        assert!(!overrides_to_fake(
            Some("/usr/local/bin/tailscale"),
            canon,
            &real
        ));
        assert!(!overrides_to_fake(
            Some("/opt/homebrew/bin/tailscale"),
            canon,
            &real
        ));
        // 偽物（存在しないパスも = 「未導入」を装う検証）
        assert!(overrides_to_fake(
            Some("/tmp/x/fake-tailscale"),
            canon,
            &real
        ));
        assert!(overrides_to_fake(
            Some("/tmp/x/no-such-tailscale"),
            canon,
            &real
        ));
        assert!(overrides_to_fake(Some("./fake-tailscale"), canon, &real));
    }

    /// 断りは日英で、英語に日本語が混ざらない・通す手段（3 つの env）を名指す
    #[test]
    fn 断りは日英で通す手段を名指す() {
        use tako_core::i18n::Lang;
        let r = crate::RemoteRefusal::IsolatedServe;
        let ja = r.text_in(Lang::Ja);
        let en = r.text_in(Lang::En);
        for text in [&ja, &en] {
            for want in [
                "TAKO_ISOLATED",
                "TAKO_REMOTE_TEST_MODE=1",
                "TAKO_TAILSCALE_BIN",
                OPT_IN_ENV,
            ] {
                assert!(text.contains(want), "{want}: {text}");
            }
        }
        assert!(
            !en.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)
                || ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "{en}"
        );
    }
}
