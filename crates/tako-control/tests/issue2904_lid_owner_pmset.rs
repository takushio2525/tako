//! #2904: macOS の蓋閉じ継続（`pmset disablesleep`）の腕が**倒した主を記録し、自分が倒した上書きだけを
//! 戻す**ことを、偽の `sudo` / `pmset`（`TAKO_2904_FAKE_PMSET`）を差し込んだ実経路で確かめる。
//!
//! 通す経路は GUI と同じ口（周期 = `sleep_guard::update`・倒す = `lid::apply_override`・終了 =
//! `sleep_guard::cleanup_on_exit` = `on_app_quit` が呼ぶもの）。A/B の旧い腕（`TAKO_2904_LEGACY=1`）が
//! 同じ注入で「他の主の上書きを戻してしまう・設定 off で自分の上書きを戻さない」ことも並べて見る
//! （注入が効いていて、修正前の症状を再現できている証拠）。
//!
//! **本物の電源設定には触らない**: 偽の置き場は検証プロセス（テストはそう）でしか読まれず、`sudo` の
//! 代わりに偽の `sudo` が引数を `calls.log` へ残して `SleepDisabled` を書き換えるだけ。env は各テストが
//! `run_alone!` の子の中だけで立てる（#2520 / #2525）
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};

use tako_control::platform::lid::{self, pmset, LidHolder};
use tako_control::sleep_guard::{self, LidSleepMode, SleepGuardConfig, SleepGuardMode};
use tako_core::test_residue::ScratchDir;

/// 偽の `sudo`。本物と同じ引数（`-n /usr/bin/pmset -a disablesleep 0|1`）だけを受け、受けた引数を
/// `calls.log` へ 1 行で残し、値を `SleepDisabled` へ書く
const FAKE_SUDO: &str = r#"#!/bin/sh
d=${0%/*}
printf '%s\n' "$*" >> "$d/calls.log"
if [ "$#" -eq 5 ] && [ "$1" = -n ] && [ "$2" = /usr/bin/pmset ] && [ "$3" = -a ] && [ "$4" = disablesleep ]; then
  printf '%s\n' "$5" > "$d/SleepDisabled"
  exit 0
fi
echo "unexpected: $*" >&2
exit 1
"#;

/// 偽の `sudo` が受けた引数（本物の `pmset` が受けるはずだったもの）
const SET_0: &str = "-n /usr/bin/pmset -a disablesleep 0";
const SET_1: &str = "-n /usr/bin/pmset -a disablesleep 1";

/// 子の中の砂場（data dir と偽の置き場）
struct Sandbox {
    _scratch: ScratchDir,
    data: PathBuf,
    fake: PathBuf,
}

impl Sandbox {
    /// 砂場を作り、env を砂場へ向ける。`disabled` はいまの値（`true` = 誰かが倒している）。
    /// `extra` は先に立てる env（A/B・隔離）
    fn new(disabled: bool, extra: &[(&str, &str)]) -> Self {
        tako_core::test_child::assert_alone("issue2904 の砂場");
        let scratch = ScratchDir::new("tako-2904");
        let data = scratch.path().join("data");
        let fake = scratch.path().join("fake");
        std::fs::create_dir_all(&data).expect("data dir を作れる");
        std::fs::create_dir_all(&fake).expect("偽の置き場を作れる");
        tako_core::fake_exec::write_script(&fake.join("sudo"), FAKE_SUDO);
        std::fs::write(fake.join("sudoers"), "").expect("偽の sudoers を置ける");
        set_value(&fake, disabled);
        // SAFETY: run_alone! の子の中だけ（他のテストは居ない）
        unsafe {
            std::env::set_var("TAKO_DATA_DIR", &data);
            std::env::set_var(pmset::FAKE_ENV, &fake);
            for (key, value) in extra {
                std::env::set_var(key, value);
            }
        }
        Self {
            _scratch: scratch,
            data,
            fake,
        }
    }

    /// 偽の `sudo` が受けた引数の並び
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.fake.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// いまの値（偽の `SleepDisabled`）
    fn disabled(&self) -> bool {
        pmset::fake_sleep_disabled().expect("偽の置き場が効いている")
    }

    /// 倒した主の記録（`lid-guard.json`）
    fn record(&self) -> Option<serde_json::Value> {
        let text = std::fs::read_to_string(self.data.join("lid-guard.json")).ok()?;
        Some(serde_json::from_str(&text).expect("記録は JSON"))
    }

    /// 何が起きたかを 1 行で出す（`--nocapture` で受け入れの証拠として並べる・落ちたときの材料）
    fn report(&self, case: &str) {
        println!(
            "TAKO_2904: {case}: 偽の sudo が受けた引数={:?} SleepDisabled={} 記録={} persist.log={:?}",
            self.calls(),
            u8::from(self.disabled()),
            self.record()
                .map_or_else(|| "なし".to_string(), |r| r["method"].to_string()),
            self.lid_log()
        );
    }

    /// data dir の persist.log の `lid-sleep:` の行
    fn lid_log(&self) -> Vec<String> {
        std::fs::read_to_string(self.data.join("persist.log"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("lid-sleep"))
            .map(str::to_string)
            .collect()
    }
}

/// 別の主（本番の GUI など）が値を変えたことにする（偽の `sudo` を通さない = 呼び手の記録に残らない）
fn set_value(fake: &Path, disabled: bool) {
    std::fs::write(
        fake.join("SleepDisabled"),
        if disabled { "1\n" } else { "0\n" },
    )
    .expect("偽の値を書ける");
}

/// 蓋閉じ継続を off にした設定（アイドルスリープ側も off = アサーションを握らない）
fn lid_off() -> SleepGuardConfig {
    SleepGuardConfig {
        mode: SleepGuardMode::Off,
        lid_sleep_mode: LidSleepMode::Off,
        ..SleepGuardConfig::default()
    }
}

/// 受け入れ条件 2 の 1: **別の主が倒した上書きを、検証起動の tako の正常終了で戻さない**
#[test]
fn 他の主の上書きは終了で戻さない() {
    if tako_core::run_alone!(他の主の上書きは終了で戻さない) {
        return;
    }
    let sb = Sandbox::new(true, &[]);
    assert!(sb.record().is_none(), "前提: この tako は記録を持たない");

    // 周期（設定は既定の off）を 1 回回しても触らない
    let state = sleep_guard::update(lid_off(), 0);
    assert_eq!(state.lid_override_holder, Some(LidHolder::NoRecord));
    // アプリの終了フック（Cmd+Q / SIGTERM の読み替え = 隔離 GUI の stop_isolated_gui）
    sleep_guard::cleanup_on_exit();
    sb.report("他の主の上書き + 検証起動の正常終了（新）");

    assert_eq!(
        sb.calls(),
        Vec::<String>::new(),
        "他の主の上書きへ pmset を打った"
    );
    assert!(sb.disabled(), "他の主の上書きが外れた");
    let log = sb.lid_log();
    assert_eq!(log.len(), 1, "終了時の記録は 1 行だけ: {log:?}");
    assert!(
        log[0].contains("lid-sleep: 終了時は戻さない holder=no-record"),
        "戻さなかったことを persist.log へ残す: {log:?}"
    );
}

/// A/B: 旧い腕（`TAKO_2904_LEGACY=1`）は同じ状況で**他の主の上書きを 0 へ戻してしまう**（#2904 の症状 1）
#[test]
fn 旧い腕は他の主の上書きを終了で戻してしまう() {
    if tako_core::run_alone!(旧い腕は他の主の上書きを終了で戻してしまう) {
        return;
    }
    let sb = Sandbox::new(true, &[(pmset::LEGACY_ENV, "1")]);
    let _ = sleep_guard::update(lid_off(), 0);
    sleep_guard::cleanup_on_exit();
    sb.report("他の主の上書き + 検証起動の正常終了（旧 = TAKO_2904_LEGACY=1）");
    assert_eq!(
        sb.calls(),
        vec![SET_0],
        "旧い腕は終了で disablesleep 0 を打つ"
    );
    assert!(!sb.disabled());
}

/// 受け入れ条件 2 の 2: **自分が倒した上書きは終了で戻す**（記録の持ち主は自分 → 終了で記録も捨てる）
#[test]
fn 自分が倒した上書きは終了で戻す() {
    if tako_core::run_alone!(自分が倒した上書きは終了で戻す) {
        return;
    }
    let sb = Sandbox::new(false, &[]);
    assert_eq!(lid::apply_override(true, false), Ok(true), "倒せる");
    assert_eq!(sb.calls(), vec![SET_1]);
    assert!(sb.disabled());
    let record = sb.record().expect("倒したら記録が残る");
    assert_eq!(record["method"], "pmset");
    assert_eq!(record["owner"]["pid"], std::process::id());
    assert_eq!(lid::override_holder(true), Some(LidHolder::Mine));
    // 同じ要求の繰り返し（2 秒 tick）は何もしない
    assert_eq!(lid::apply_override(true, false), Ok(false));
    assert_eq!(sb.calls(), vec![SET_1]);

    sleep_guard::cleanup_on_exit();
    sb.report("自分が倒した上書き + 正常終了（新）");
    assert_eq!(sb.calls(), vec![SET_1, SET_0], "自分の上書きは終了で戻す");
    assert!(!sb.disabled());
    assert!(sb.record().is_none(), "戻したら記録も捨てる");
    assert!(
        sb.lid_log()
            .iter()
            .any(|l| l.contains("lid-sleep: 蓋閉じ継続を解除 reason=exit")),
        "{:?}",
        sb.lid_log()
    );
}

/// 受け入れ条件 3: **設定を off にしたら自分の上書きだけ戻す**（#2904 の症状 2）。他の主の上書きは戻さない
#[test]
fn 設定offで自分の上書きだけ戻す() {
    if tako_core::run_alone!(設定offで自分の上書きだけ戻す) {
        return;
    }
    let sb = Sandbox::new(false, &[]);
    assert_eq!(lid::apply_override(true, false), Ok(true));
    // GUI の 2 秒 tick と同じ口。設定は off
    let state = sleep_guard::update(lid_off(), 0);
    sb.report("自分が倒した上書き + 設定 off（新）");
    assert_eq!(sb.calls(), vec![SET_1, SET_0], "off で自分の上書きを戻す");
    assert!(!sb.disabled());
    assert!(sb.record().is_none());
    assert!(!state.lid_sleep_disabled);
    assert!(
        sb.lid_log()
            .iter()
            .any(|l| l.contains("lid-sleep: 蓋閉じ継続を解除 reason=mode-off")),
        "{:?}",
        sb.lid_log()
    );

    // 別の主が倒した（記録は無い）→ off のまま何周しても戻さない
    set_value(&sb.fake, true);
    for _ in 0..3 {
        let state = sleep_guard::update(lid_off(), 0);
        assert_eq!(state.lid_override_holder, Some(LidHolder::NoRecord));
    }
    sb.report("続けて他の主が倒した上書き + 設定 off で 3 周（新）");
    assert_eq!(
        sb.calls(),
        vec![SET_1, SET_0],
        "他の主の上書きを off で戻した"
    );
    assert!(sb.disabled());
}

/// A/B: 旧い腕は設定 off の間 `pmset` に触らない = **自分が倒した上書きも戻さない**（#2904 の症状 2）
#[test]
fn 旧い腕は設定offで自分の上書きも戻さない() {
    if tako_core::run_alone!(旧い腕は設定offで自分の上書きも戻さない) {
        return;
    }
    let sb = Sandbox::new(false, &[(pmset::LEGACY_ENV, "1")]);
    assert_eq!(lid::apply_override(true, false), Ok(true));
    let _ = sleep_guard::update(lid_off(), 0);
    sb.report("自分が倒した上書き + 設定 off（旧 = TAKO_2904_LEGACY=1）");
    assert_eq!(
        sb.calls(),
        vec![SET_1],
        "旧い腕は off の間 pmset を打たない"
    );
    assert!(sb.disabled(), "旧い腕では off にしても倒れたまま");
}

/// 終了時の門（#2904 の案 A）: **持ち主の死んだ記録は、隔離中の終了では戻さない**。
///
/// 値は機械全体で共有なので、自分の data dir に前回の tako の記録が残っていても、いまの 1 が別の
/// tako（本番の GUI）のものでないとは言えない。起動時の残留チェックと同じ門（隔離・他の tako の稼働）を
/// 通ったときだけ戻す。門が開いた側は「他の tako が居ない」を作れない（この機械では本番が走っている・
/// CI では走っていない）ので、純粋な判定（`pmset::tests`）で見る
#[test]
fn 持ち主の死んだ記録は隔離中の終了で戻さない() {
    if tako_core::run_alone!(持ち主の死んだ記録は隔離中の終了で戻さない) {
        return;
    }
    let sb = Sandbox::new(true, &[("TAKO_ISOLATED", "1")]);
    // 前回の tako（もう居ない pid）が倒した記録
    let reaped = {
        let mut cmd = std::process::Command::new("/usr/bin/true");
        tako_core::platform::process::ReapedChild::run(&mut cmd).expect("看取れる子を起こせる")
    };
    let dead = reaped.pid();
    assert!(
        tako_core::platform::process::wait_gone(dead, std::time::Duration::from_secs(10)),
        "看取った pid {dead} が居なくならない"
    );
    std::fs::write(
        sb.data.join("lid-guard.json"),
        serde_json::json!({
            "method": "pmset",
            "scheme": pmset::RECORD_SCHEME,
            "ac": null,
            "dc": null,
            "owner": { "pid": dead, "started_unix": 1_700_000_000u64 },
        })
        .to_string(),
    )
    .expect("記録を置ける");
    assert_eq!(lid::override_holder(true), Some(LidHolder::Stale));

    sleep_guard::cleanup_on_exit();
    sb.report("持ち主の死んだ記録 + 隔離中の正常終了（新）");
    assert_eq!(sb.calls(), Vec::<String>::new(), "門が閉じているのに戻した");
    assert!(sb.disabled());
    assert!(sb.record().is_some(), "門が閉じている間は記録も残す");
    drop(reaped);
}
