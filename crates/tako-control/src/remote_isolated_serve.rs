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
//!
//! # serve の外の、機械全体へ触る操作（#2624）
//!
//! 同じ門（[`allows_op`] の 1 実装）が serve 以外の 3 つも見る。通す理由は操作ごとに違う:
//!
//! - [`Op::ServeCleanup`] — `tako remote stop` の後始末（`remote::cleanup_serve_leftover`）が本物の
//!   tailscale の serve を読み、自分の待ち受けと一致すれば外す。通すのは**この env が serve を張れた**とき
//!   （偽の tailscale・opt-in・#2619 の A/B）だけ。検証モードは serve を張らないので外す
//! - [`Op::Install`] — 対話の `tako remote setup` が未導入の Tailscale を導入器（brew）で入れる。
//!   導入はこの機械全体へのインストールで、**偽の tailscale では偽物にならない**（「未導入を装う」偽の
//!   `TAKO_TAILSCALE_BIN` で門を通すと本物の brew が走る）ので、通すのは opt-in だけ
//! - [`Op::OpenViewer`] — 対話の setup が QR を既定のビューアで開く（隔離中でも利用者の画面に窓が出る）。
//!   通すのは opt-in だけ
//!
//! 3 つの A/B は `TAKO_2624_LEGACY=1`（[`LEGACY_2624_ENV`]。#2624 前 = 隔離中でも従来どおり触る）。

use std::path::{Path, PathBuf};

/// 隔離中でも本物の tailscale の serve を張る明示の opt-in
pub const OPT_IN_ENV: &str = "TAKO_REMOTE_ISOLATED_SERVE";

/// A/B: #2619 前へ戻す（隔離中でも既定の tailscale へ serve を張る）
pub const LEGACY_ENV: &str = "TAKO_2619_LEGACY";

/// A/B: #2624 前へ戻す（隔離中でも stop の後始末が本物の tailscale を読み・外し、対話の setup が
/// Tailscale を導入し、QR を既定のビューアで開く）
pub const LEGACY_2624_ENV: &str = "TAKO_2624_LEGACY";

/// 門の入力（env から読んだもの。判定は [`allows_op`] が持つ）
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
    /// [`LEGACY_2624_ENV`] が真
    pub legacy_2624: bool,
}

/// 隔離中に門を通す操作（#2624。通す理由は操作ごとに違う = モジュールの doc）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// serve を張る（#2619）
    Serve,
    /// stop の後始末で serve を読み・自分の待ち受けなら外す
    ServeCleanup,
    /// Tailscale を導入器（brew）で入れる
    Install,
    /// QR を既定のビューアで開く
    OpenViewer,
}

/// serve へ書いてよいか（純関数）。隔離でなければ常に真 = 本番の経路は変えない
pub fn allows(i: Inputs) -> bool {
    allows_op(Op::Serve, i)
}

/// その操作をしてよいか（純関数。**判定はこの 1 実装**）。隔離でなければ常に真 = 本番の経路は変えない
pub fn allows_op(op: Op, i: Inputs) -> bool {
    if !i.isolated {
        return true;
    }
    match op {
        Op::Serve => i.test_mode || i.fake_tailscale || i.opt_in || i.legacy,
        // 後始末は「この env が serve を張れた」ときだけ（検証モードは張らない）
        Op::ServeCleanup => i.legacy_2624 || i.fake_tailscale || i.opt_in || i.legacy,
        // 導入と画面は tailscale を偽物にしても偽物にならない
        Op::Install | Op::OpenViewer => i.legacy_2624 || i.opt_in,
    }
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

/// パスを解決する（シンボリックリンクを辿る。解決できないパス = 存在しない偽物はそのまま）。
/// 境界 B26（#970）の 1 実装を通す = 両辺とも Windows の verbatim prefix を落とした形で比べる
fn canonical(path: &Path) -> PathBuf {
    tako_core::platform::path::canonicalize_or_self(path)
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
            legacy_2624: false,
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
        legacy_2624: truthy(LEGACY_2624_ENV),
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

/// serve 以外の操作の門（#2624）。断りを返さない操作（後始末・ビューア）は呼び手が黙って見送る
pub fn allowed(op: Op) -> bool {
    allows_op(op, current_inputs())
}

/// 対話の setup が Tailscale を導入する前に通す門（#2624）。隔離中で opt-in が無ければ日英の断り
pub fn check_install() -> Result<(), crate::RemoteRefusal> {
    if allowed(Op::Install) {
        Ok(())
    } else {
        Err(crate::RemoteRefusal::IsolatedInstall)
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
            legacy_2624: false,
        }
    }

    const OPS: [Op; 4] = [Op::Serve, Op::ServeCleanup, Op::Install, Op::OpenViewer];

    /// 本番（隔離でない）の経路は、操作とほかの入力がどうであれ常に通す = 挙動を変えない
    #[test]
    fn 隔離でなければ常に通す() {
        for bits in 0u8..32 {
            let i = Inputs {
                isolated: false,
                test_mode: bits & 1 != 0,
                fake_tailscale: bits & 2 != 0,
                opt_in: bits & 4 != 0,
                legacy: bits & 8 != 0,
                legacy_2624: bits & 16 != 0,
            };
            assert!(allows(i), "{i:?}");
            for op in OPS {
                assert!(allows_op(op, i), "{op:?} {i:?}");
            }
        }
    }

    /// 隔離中は理由が 1 つも無ければ断り、どれか 1 つで通す（serve = #2619 のまま）
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
        // #2624 の A/B は serve の門を開けない（#2619 の判定は変えない）
        let mut i = inputs(true);
        i.legacy_2624 = true;
        assert!(!allows(i));
    }

    /// 隔離中に操作ごとに通す理由の表（#2624）。
    /// 後始末は「serve を張れた env」だけ・導入と画面は tailscale の偽物では開かない
    #[test]
    fn 隔離中の操作ごとの通す理由() {
        // (理由の名前, 入れ方, [Serve, ServeCleanup, Install, OpenViewer] で通すか)
        type Row = (&'static str, fn(&mut Inputs), [bool; 4]);
        let table: [Row; 6] = [
            ("理由なし", |_| {}, [false, false, false, false]),
            (
                "検証モード",
                |i| i.test_mode = true,
                [true, false, false, false],
            ),
            (
                "偽の tailscale",
                |i| i.fake_tailscale = true,
                [true, true, false, false],
            ),
            ("opt-in", |i| i.opt_in = true, [true, true, true, true]),
            (
                "#2619 の A/B",
                |i| i.legacy = true,
                [true, true, false, false],
            ),
            (
                "#2624 の A/B",
                |i| i.legacy_2624 = true,
                [false, true, true, true],
            ),
        ];
        for (name, set, want) in table {
            let mut i = inputs(true);
            set(&mut i);
            for (op, want) in OPS.into_iter().zip(want) {
                assert_eq!(allows_op(op, i), want, "{name}: {op:?}");
            }
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
