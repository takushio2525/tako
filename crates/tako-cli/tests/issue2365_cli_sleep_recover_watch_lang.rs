//! 実経路: `tako sleep-guard`・`tako recover` 一式・`tako orchestrator watch` の人向けの表示が表示言語に
//! 従う（Issue #2365 の続き。前のスライスは `issue2365_cli_todo_sessions_lang.rs`）。
//!
//! 修正前（実測）: 英語表示でも sleep-guard は状態の行（「取得元:」「モード:」「蓋:」ほか）・設定の変更の行・
//! 蓋閉じ防止の登録 / 解除の行と `set` の断り、recover は世代の一覧（「（無し）」「N 日前」「読めない:」・
//! 「復元の直後の全滅」）・復元の案内・復元と合流の結果と断り、watch は切り詰めの印・`note:` の説明・
//! `respond:` のプレースホルダ・`WORKER_DEAD` の説明が、日本語の固定文のまま出た（`main.rs` の直書き）。
//!
//! **ビルドした `tako` をそのまま**叩く。sleep-guard の状態と watch は GUI へ問い合わせるので、決まった応答を
//! 返す受け口（偽の GUI）を立てる（実機の電池・蓋・温度に左右されない）。蓋閉じ防止の登録 / 解除は
//! **偽の `osascript`**（PATH の先頭）を通す = 管理者権限のスクリプトは走らず、sudoers にも pmset にも
//! 触れない。recover は一時の data dir に世代のファイルを置き、tmux は誰も居ない名前のソケットを見る。見ること:
//! - 英語表示: 出力に日本語（かな・漢字）が 1 字も無い（応答の値は ASCII だけ）。例外は 2 つで、
//!   watch の現在の選択の印（master の guide が字面で引く印 = `orchestrator::wait::CHOICE_HIGHLIGHT_MARK`）と、
//!   sleep-guard の状態の JSON（stdout。応答の値の `description` / `lid_skip_reason.text` は日本語のまま）
//! - 日本語表示: #2365 より前の出力とバイト一致（stdout / stderr とも。砂場の置き場は `<SB>` へ置き換える）
//! - 終了コードは日英で同じ・状態の JSON は日英で同じ
//! - 英語表示でも一覧のラベルの桁（20 桁）・字下げ・`WORKER_*` と `detail:` ほかのラベルが変わらない
//!
//! A/B: `TAKO_2365_HUMAN_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は英語表示の検査で落ちる）。
//! 本番の data dir・GUI へ届かないよう、`TAKO_*` を全部外して一時の置き場と偽の GUI の口だけを渡す。
//!
//! unix だけで走らせる（偽の GUI が Unix domain socket・偽の osascript がシェルスクリプト）。sleep-guard の
//! 状態と登録 / 解除の行は macOS の出し分け（蓋・sudoers・温度）を前提にした見本なので macOS だけで見る。
//! 文の日英の対は OS に依らない 1 か所（`ReplyNote` / `Reason` と中身の型）で、両 OS の番犬
//! （`crates/tako-control/tests/issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use tako_control::protocol::ResponseEnvelope;
use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2365_HUMAN_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2365_HUMAN_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// 偽の osascript: 呼ばれた記録を残し、管理者権限のスクリプトが `echo` する `OK:` の行だけを返す。
/// `I2365_OSASCRIPT=cancel` なら利用者が取り消したときの出力（`User canceled. (-128)`）で落ちる
const FAKE_OSASCRIPT: &str = r#"#!/bin/sh
echo called >> "$I2365_OSA_LOG"
if [ "${I2365_OSASCRIPT:-}" = cancel ]; then
  echo "execution error: User canceled. (-128)" >&2
  exit 1
fi
printf '%s\n' "$2" | sed -n "s/^echo '\(OK:.*\)'\$/\1/p"
"#;

/// 1 回の実行の結果（砂場の置き場は `<SB>` へ置き換え済み）
#[derive(Debug, Clone, PartialEq)]
struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// 1 言語分の砂場（表示言語の設定・偽の GUI の受け口・偽の osascript・世代のファイルの置き場）
struct Sandbox {
    dir: ScratchDir,
}

impl Sandbox {
    fn new(lang: &str) -> Self {
        let sb = Self {
            // 偽の GUI の Unix domain socket が macOS の上限（104 バイト）に収まるよう短い名前にする
            dir: ScratchDir::new("i2365g"),
        };
        for d in ["data", "disc", "orch", "home", "tmp", "bin"] {
            let d = sb.path().join(d);
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        sb.write(
            "data/settings.json",
            &format!("{{\"language\":\"{lang}\"}}"),
        );
        sb.write("bin/osascript", FAKE_OSASCRIPT);
        let osa = sb.path().join("bin/osascript");
        std::fs::set_permissions(&osa, std::fs::Permissions::from_mode(0o755))
            .unwrap_or_else(|e| panic!("{} に実行の権限を付けられる: {e}", osa.display()));
        sb
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.path().join(rel);
        std::fs::write(&p, text).unwrap_or_else(|e| panic!("{} を書ける: {e}", p.display()));
    }

    /// 更新時刻を 3 日と 10 分前へ（一覧の経過が「3 日前」で決まる）
    fn age(&self, rel: &str) {
        let p = self.path().join(rel);
        let at = SystemTime::now() - Duration::from_secs(3 * 86400 + 600);
        std::fs::File::options()
            .write(true)
            .open(&p)
            .and_then(|f| f.set_modified(at))
            .unwrap_or_else(|e| panic!("{} の更新時刻を変えられる: {e}", p.display()));
    }

    /// 偽の osascript が呼ばれた回数
    fn osascript_calls(&self) -> usize {
        std::fs::read_to_string(self.path().join("osa.log"))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    /// `TAKO_*` を外して隔離の置き場（と偽の GUI の口）だけを渡した `tako <args>` の結果。
    /// `gui` が無ければ誰も居ない口を渡す（= GUI が動いていない手元の処理）
    fn run(&self, args: &[&str], gui: Option<&str>, extra: &[(&str, &str)]) -> Out {
        let mut command = Command::new(tako());
        command
            .args(args)
            .current_dir(self.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy().to_string();
            if key.starts_with("TAKO_") || key.starts_with("CLAUDE") || key == "CODEX_HOME" {
                command.env_remove(key);
            }
        }
        // 偽の osascript を PATH の先頭へ（本物の osascript = 管理者権限のスクリプトへ届かない）
        let path = format!(
            "{}:{}",
            self.path().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let none = self.path().join("none.sock");
        command
            .env("PATH", path)
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.path().join("data"))
            .env("TAKO_DISCOVERY_DIR", self.path().join("disc"))
            .env("TAKO_ORCHESTRATOR_DIR", self.path().join("orch"))
            .env("HOME", self.path().join("home"))
            .env("TMPDIR", self.path().join("tmp"))
            // 誰も居ない名前（`-L`）= 本番の器のサーバー（`tako`）へ問い合わせない
            .env("TAKO_TMUX_SOCKET", format!("i2365g{}", std::process::id()))
            .env(
                "TAKO_SOCKET",
                gui.map_or_else(|| none.display().to_string(), String::from),
            )
            .env("TAKO_TOKEN", "t")
            .env("I2365_OSA_LOG", self.path().join("osa.log"));
        for (k, v) in extra {
            command.env(k, v);
        }
        let out = command.output().expect("tako が終わる");
        let root = self.path().display().to_string();
        let clean = |b: &[u8]| String::from_utf8_lossy(b).replace(&root, "<SB>");
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: clean(&out.stdout),
            stderr: clean(&out.stderr),
        }
    }

    /// 決まった `result` を返す偽の GUI（受けた接続すべてへ同じ応答を返す）
    fn fake_gui(&self, tag: &str, result: Value) -> String {
        let path = self.path().join(format!("{tag}.sock"));
        let listener = UnixListener::bind(&path).expect("受け口を作れる");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut line = String::new();
                let _ =
                    BufReader::new(stream.try_clone().expect("複製できる")).read_line(&mut line);
                let reply = ResponseEnvelope::ok(1, result.clone());
                let mut stream = stream;
                let _ = writeln!(
                    stream,
                    "{}",
                    serde_json::to_string(&reply).expect("応答を組める")
                );
            }
        });
        path.display().to_string()
    }
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

// ---------------------------------------------------------------------------
// 見本の応答と世代のファイル
// ---------------------------------------------------------------------------

/// watch: 番号の効かない選択肢ダイアログ（カーソルは画面外・ラベルの切り詰めあり・1 つ目が今の選択）
fn dialog_status() -> Value {
    json!({
        "status": "waiting",
        "status_source": "agents",
        "recent_output": "",
        "events": [],
        "choice_dialog": {
            "kind": "select",
            "title": "Pick a model",
            "numbered": false,
            "cursor_visible": false,
            "labels_truncated": true,
            "recommended_action": "respond",
            "options": [
                { "number": 1, "label": "Default", "highlighted": true },
                { "number": 2, "label": "Opus long lab", "label_truncated": true }
            ]
        }
    })
}

/// watch: エージェント CLI の突然死（session ID の記録なし = resume のコマンドを組めない）。
/// 2 回続けて見えたら確定するので、5 秒の間隔を 1 回待つ
fn dead_status() -> Value {
    json!({
        "status": "busy",
        "status_source": "agents",
        "recent_output": "",
        "events": [{ "kind": "agent_dead" }]
    })
}

/// sleep-guard status: GUI が持つ状態（効かない理由あり・電池・高温・ディスプレイ消灯）
fn guard_state_busy() -> Value {
    json!({
        "assertion_held": true,
        "mode": "while-agents-running",
        "power_condition": "ac-only",
        "on_ac_power": false,
        "busy_agents": 2,
        "platform_supported": true,
        "lid_closed": true,
        "lid_sleep_disabled": false,
        "lid_sleep_mode": "while-agents-running",
        "sudoers_installed": true,
        "lid_setup_required": false,
        "thermal_state": "serious",
        "display_sleep_forced": true,
        "lid_power_condition": "ac-only",
        "lid_battery_floor": 20,
        "battery_percent": 57,
        "lid_skip_reason": { "reason": "no-ac-power" }
    })
}

/// sleep-guard status: GUI が持つ状態（未保持・電池を読めない・蓋閉じ継続を適用中）
fn guard_state_idle() -> Value {
    json!({
        "assertion_held": false,
        "mode": "off",
        "power_condition": "always",
        "on_ac_power": true,
        "busy_agents": 0,
        "platform_supported": true,
        "lid_closed": false,
        "lid_sleep_disabled": true,
        "lid_sleep_mode": "off",
        "sudoers_installed": false,
        "lid_setup_required": true,
        "thermal_state": "nominal",
        "display_sleep_forced": false,
        "lid_power_condition": "always",
        "lid_battery_floor": 20,
        "battery_percent": null,
        "lid_skip_reason": null
    })
}

/// layout.json の 1 ペイン（`id` と同じ番号の器の名前）
fn pane(id: u64) -> Value {
    json!({
        "type": "pane", "id": id, "session": format!("tako-{id}"), "title": null,
        "title_source": "default", "role": null, "origin": "user", "cwd": null
    })
}

fn tab(id: u64, title: &str, focused: u64, tree: Value) -> Value {
    json!({ "id": id, "title": title, "title_source": "manual", "focused": focused, "tree": tree })
}

fn layout(tabs: Vec<Value>) -> String {
    json!({ "version": 1, "active_tab": 1, "tabs": tabs }).to_string()
}

/// recover の世代: 現行 = 2 タブ（2 枚目はバックアップより後）・bak.1 = 1 タブ・bak.2 = 途中で切れた
/// JSON・bak.3 = 無し・good = 形の違う JSON（タブが無い）・rescued = 2 ペイン・復元の直後の全滅の記録 1 回
fn seed_generations(sb: &Sandbox) {
    let split = json!({
        "type": "split", "axis": "x", "ratio": 0.5, "first": pane(1), "second": pane(3)
    });
    sb.write(
        "data/layout.json",
        &layout(vec![tab(1, "main", 1, pane(1)), tab(2, "work", 2, pane(2))]),
    );
    sb.write(
        "data/layout.json.bak.1",
        &layout(vec![tab(1, "main", 1, pane(1))]),
    );
    sb.write("data/layout.json.bak.2", "{");
    sb.write("data/layout.json.good", r#"{"version":1,"active_tab":1}"#);
    sb.write(
        "data/layout.json.rescued",
        &layout(vec![tab(1, "main", 1, split)]),
    );
    sb.write(
        "data/restore-guard.json",
        r#"{"reason":"all_exited","count":1,"at_ms":0}"#,
    );
    for rel in [
        "data/layout.json",
        "data/layout.json.bak.1",
        "data/layout.json.bak.2",
        "data/layout.json.good",
        "data/layout.json.rescued",
    ] {
        sb.age(rel);
    }
}

// ---------------------------------------------------------------------------
// 手順と #2365 より前の日本語表示
// ---------------------------------------------------------------------------

/// 1 言語分を叩いた結果（名前 → 結果）と、偽の osascript が呼ばれた回数
struct Run {
    outs: Vec<(&'static str, Out)>,
    osascript_calls: usize,
}

impl Run {
    fn get(&self, name: &str) -> &Out {
        self.outs
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, o)| o)
            .unwrap_or_else(|| panic!("{name} を叩いていない"))
    }
}

/// sleep-guard の登録 / 解除の行は macOS の出し分けに従うので、macOS の手順だけ叩く
fn on_macos() -> bool {
    cfg!(target_os = "macos")
}

fn run_lang(lang: &str) -> Run {
    let sb = Sandbox::new(lang);
    let mut outs: Vec<(&'static str, Out)> = Vec::new();
    // ---- watch（偽の GUI の応答で即時のダイアログと 2 回で確定する突然死）----
    let gui = sb.fake_gui("w1", dialog_status());
    outs.push((
        "watch_dialog",
        sb.run(&["orchestrator", "watch", "--pane", "7"], Some(&gui), &[]),
    ));
    let gui = sb.fake_gui("w2", dead_status());
    outs.push((
        "watch_dead",
        sb.run(&["orchestrator", "watch", "--pane", "7"], Some(&gui), &[]),
    ));
    // ---- sleep-guard の set（OS に依らない）----
    let set_cases: [(&'static str, &[&str]); 6] = [
        ("set_ok", &["--mode", "on", "--lid-battery-floor", "30"]),
        ("set_bad_mode", &["--mode", "bogus"]),
        ("set_bad_power", &["--power-condition", "bogus"]),
        ("set_bad_lid_mode", &["--lid-sleep-mode", "bogus"]),
        ("set_bad_lid_power", &["--lid-power-condition", "bogus"]),
        ("set_bad_floor", &["--lid-battery-floor", "3"]),
    ];
    for (name, args) in set_cases {
        let mut all = vec!["sleep-guard", "set"];
        all.extend_from_slice(args);
        outs.push((name, sb.run(&all, None, &[])));
    }
    // ---- sleep-guard の状態・登録 / 解除（macOS の出し分け）----
    if on_macos() {
        let gui = sb.fake_gui("g1", guard_state_busy());
        outs.push((
            "status_busy",
            sb.run(&["sleep-guard", "status"], Some(&gui), &[]),
        ));
        let gui = sb.fake_gui("g2", guard_state_idle());
        outs.push((
            "status_idle",
            sb.run(&["sleep-guard", "status"], Some(&gui), &[]),
        ));
        outs.push((
            "status_local",
            sb.run(&["sleep-guard", "status"], None, &[]),
        ));
        let cancel = [("I2365_OSASCRIPT", "cancel")];
        outs.push((
            "install",
            sb.run(&["sleep-guard", "install-lid-sleep"], None, &[]),
        ));
        outs.push((
            "install_cancel",
            sb.run(&["sleep-guard", "install-lid-sleep"], None, &cancel),
        ));
        outs.push((
            "remove",
            sb.run(&["sleep-guard", "remove-lid-sleep"], None, &[]),
        ));
    }
    // ---- recover（世代の一覧 → 断り → 合流つきの復元 → 現行が読めない → 現行が無い）----
    seed_generations(&sb);
    outs.push(("recover_list", sb.run(&["recover"], None, &[])));
    outs.push((
        "recover_bad_generation",
        sb.run(&["recover", "--apply", "9"], None, &[]),
    ));
    outs.push((
        "recover_corrupt",
        sb.run(&["recover", "--apply", "2", "--force"], None, &[]),
    ));
    outs.push((
        "recover_shape",
        sb.run(&["recover", "--apply", "good", "--force"], None, &[]),
    ));
    outs.push((
        "recover_merge",
        sb.run(&["recover", "--apply", "1", "--force"], None, &[]),
    ));
    sb.write("data/layout.json", "{");
    outs.push((
        "recover_current_corrupt",
        sb.run(&["recover", "--apply", "rescued", "--force"], None, &[]),
    ));
    std::fs::remove_file(sb.path().join("data/layout.json")).expect("layout.json を消せる");
    outs.push((
        "recover_no_current",
        sb.run(&["recover", "--apply", "1", "--force"], None, &[]),
    ));
    Run {
        outs,
        osascript_calls: sb.osascript_calls(),
    }
}

/// 日英を並べて 1 回だけ叩く（watch の突然死が 5 秒待つので、2 つのテストで共有する）
fn runs() -> &'static (Run, Run) {
    static RUNS: OnceLock<(Run, Run)> = OnceLock::new();
    RUNS.get_or_init(|| {
        std::thread::scope(|s| {
            let ja = s.spawn(|| run_lang("ja"));
            let en = s.spawn(|| run_lang("en"));
            (
                ja.join().expect("日本語の手順が終わる"),
                en.join().expect("英語の手順が終わる"),
            )
        })
    })
}

/// 終了の打鍵の表記（Windows は Ctrl+Shift+Q。この CLI と同じ正本から引く）
fn quit_key() -> String {
    tako_core::platform::keys::quit(tako_core::platform::support::Platform::current()).to_string()
}

/// #2365 より前の日本語表示（stdout / stderr / 終了コード）。`None` の stdout は比べない
/// （sleep-guard の `set` / 手元の `status` の JSON は実機の電源・電池・温度を読む）
fn expected_ja(name: &str) -> Option<(Option<String>, String, i32)> {
    let q = quit_key();
    let bad = |text: &str| Some((Some(String::new()), format!("error: {text}\n"), 1));
    let sudoers_installed = tako_control::sleep_guard::is_sudoers_installed();
    Some(match name {
        "watch_dialog" => (
            Some(
                "WORKER_DIALOG: tako:7 (select)\n  title: Pick a model\n  1. Default ← 現在の選択\n  \
                 2. Opus long lab [切り詰め]\n  note: 番号キーは無反応なダイアログ（tako が矢印移動で応答する）\n  \
                 note: ダイアログがペインより高く選択カーソルが画面外（番号で応答する）\n  \
                 note: ラベルが切り詰められている（respond はラベルではなく番号で）\n  action: respond\n  \
                 respond: tako orchestrator respond --pane 7 --choice <番号|ラベル>\n"
                    .into(),
            ),
            String::new(),
            0,
        ),
        "watch_dead" => (
            Some(
                "WORKER_DEAD: tako:7\n  detail: エージェント CLI プロセスが終了している（SIGSEGV 等の突然死の疑い）\n  \
                 resume: (session ID 未記録のため resume コマンドを組み立てられない)\n  action: resume_session\n"
                    .into(),
            ),
            String::new(),
            0,
        ),
        "set_ok" => (
            None,
            "  設定を変更しました: mode=on, power=ac-only, lid-sleep=off, lid-power=ac-only, \
             lid-battery-floor=30%\n"
                .into(),
            0,
        ),
        "set_bad_mode" => {
            return bad("不明な mode: \"bogus\"（off / on / while-agents-running のいずれか）")
        }
        "set_bad_power" => return bad("不明な power: \"bogus\"（ac-only / always のいずれか）"),
        "set_bad_lid_mode" => {
            return bad("不明な lid-sleep-mode: \"bogus\"（off / while-agents-running のいずれか）")
        }
        "set_bad_lid_power" => {
            return bad(
                "不明な lid-power-condition: \"bogus\"（ac-only = AC 電源に接続しているときだけ / \
                 always = バッテリー駆動でも）",
            )
        }
        "set_bad_floor" => {
            return bad(&tako_control::OsRefusal::BatteryFloorOutOfRange(3).text_in(
                tako_core::i18n::Lang::Ja,
            ))
        }
        "status_busy" => (
            None,
            "  取得元: tako アプリ（IPC）\n  idle-sleep: アサーション保持中\n  モード: while-agents-running\n  \
             電源条件: ac-only\n  AC 電源: 未接続\n  蓋: 閉\n  蓋閉じ防止: while-agents-running (sudoers: 登録済み)\n  \
             蓋閉じ継続の適用: 無効\n  蓋閉じ継続の電源条件: ac-only\n  バッテリー残量: 57%（下限 20%）\n  \
             蓋閉じ継続が効かない理由: AC 未接続のため無効（tako sleep-guard set --lid-power-condition always \
             でバッテリーでも継続）\n  thermal: serious\n  ディスプレイ: 消灯済み（蓋閉じ中）\n"
                .into(),
            0,
        ),
        "status_idle" => (
            None,
            "  取得元: tako アプリ（IPC）\n  idle-sleep: アサーション未保持\n  モード: off\n  電源条件: always\n  \
             AC 電源: 接続中\n  蓋: 開\n  蓋閉じ防止: off (sudoers: 未登録)\n  蓋閉じ継続の適用: 有効\n  \
             蓋閉じ継続の電源条件: always\n  バッテリー残量: 不明（この機械では取得できません）\n  thermal: nominal\n"
                .into(),
            0,
        ),
        // 手元の状態は実機を読むので、取得元の行（1 行目）だけを比べる（下の検査）
        "status_local" => return None,
        "install" => {
            let head = if sudoers_installed {
                "蓋閉じ防止を有効にします...\n"
            } else {
                "蓋閉じ防止の sudoers 登録を行います...\n  登録内容: pmset -a disablesleep 0/1 のみ NOPASSWD\n  \
                 管理者パスワードの入力ダイアログが表示されます。\n"
            };
            (
                Some(String::new()),
                format!(
                    "{head}  OK: sudoers 登録完了\n  lid-sleep-mode を while-agents-running に設定しました。\n  \
                     解除: tako sleep-guard remove-lid-sleep\n"
                ),
                0,
            )
        }
        "install_cancel" => {
            let head = if sudoers_installed {
                "蓋閉じ防止を有効にします...\n"
            } else {
                "蓋閉じ防止の sudoers 登録を行います...\n  登録内容: pmset -a disablesleep 0/1 のみ NOPASSWD\n  \
                 管理者パスワードの入力ダイアログが表示されます。\n"
            };
            let refusal =
                tako_control::OsRefusal::UserCanceled.text_in(tako_core::i18n::Lang::Ja);
            (
                Some(String::new()),
                format!("{head}error: {refusal}\n"),
                1,
            )
        }
        "remove" => {
            let done = if sudoers_installed {
                "OK: sudoers 削除完了・disablesleep 解除"
            } else {
                "sudoers は未登録です（削除不要）"
            };
            (
                Some(String::new()),
                format!("  {done}\n  lid-sleep-mode を off に設定しました。\n"),
                0,
            )
        }
        "recover_list" => (
            Some(
                "layout.json         : 2 タブ / 2 ペイン（3 日前 更新）\n\
                 layout.json.bak.1   : 1 タブ / 1 ペイン（3 日前 更新）\n\
                 layout.json.bak.2   : 読めない: レイアウトファイルを解釈できない（破損）: \
                 JSON が途中で切れている（1 行 1 列）（3 日前 更新）\n\
                 layout.json.bak.3   : （無し）\n\
                 layout.json.good    : 読めない: レイアウトファイルを解釈できない（破損）: \
                 JSON の形が期待と違う（1 行 28 列）（3 日前 更新）\n\
                 layout.json.rescued : 1 タブ / 2 ペイン（3 日前 更新）\n\
                 復元の直後の全滅    : 連続 1 回（2 回続くと次の起動は復元を見送る）\n"
                    .into(),
            ),
            format!(
                "\n復元するには: tako を終了（{q}）してから `tako recover --apply <世代>` →\n\
                 tako を再起動すると復元されたレイアウトで立ち上がります。\n\
                 実体の tmux セッションが生きていれば、実行中プロセスごと画面に戻ります。\n\
                 （good = 最後に復元へ成功した良品。`tako recover --apply good` で戻せます）\n\
                 （rescued = 復元の直後に全ペインが終わったので退避した構成。`tako recover --apply rescued` で戻せます）\n"
            ),
            0,
        ),
        "recover_bad_generation" => {
            return bad("世代は 1〜3・good・rescued のどれかで指定してください（指定: 9）")
        }
        "recover_corrupt" => {
            return bad(
                "バックアップ <SB>/data/layout.json.bak.2 を読めない: レイアウトファイルを解釈できない（破損）: \
                 JSON が途中で切れている（1 行 1 列）",
            )
        }
        "recover_shape" => {
            return bad(
                "バックアップ <SB>/data/layout.json.good を読めない: レイアウトファイルを解釈できない（破損）: \
                 JSON の形が期待と違う（1 行 28 列）",
            )
        }
        "recover_merge" => (
            Some(String::new()),
            "現在の layout.json → <SB>/data/layout.json.pre-recover へ退避\n\
             layout.json.bak.1（1 タブ / 1 ペイン）→ layout.json へ復元しました。\n\
             バックアップより後に作られたタブを 1 枚合流しました:\n  \
             タブ 2「work」: 1 ペイン（tmux セッションあり 0）\n\
             （tmux に問い合わせられない（サーバー未起動・ソケットが無い等）ので、tmux セッションの生死は確かめずに合流しました）\n\
             合流後: 2 タブ / 2 ペイン\n\
             tako を起動すると、このレイアウトで復元されます。\n"
                .into(),
            0,
        ),
        "recover_current_corrupt" => (
            Some(String::new()),
            "現在の layout.json を読めないので、バックアップより後のタブは合流しない: \
             レイアウトファイルを解釈できない（破損）: JSON が途中で切れている（1 行 1 列）\n\
             現在の layout.json → <SB>/data/layout.json.pre-recover へ退避\n\
             layout.json.rescued（1 タブ / 2 ペイン）→ layout.json へ復元しました。\n\
             tako を起動すると、このレイアウトで復元されます。\n"
                .into(),
            0,
        ),
        "recover_no_current" => (
            Some(String::new()),
            "layout.json.bak.1（1 タブ / 1 ペイン）→ layout.json へ復元しました。\n\
             tako を起動すると、このレイアウトで復元されます。\n"
                .into(),
            0,
        ),
        other => panic!("{other} の見本が無い"),
    })
}

/// 英語表示で日本語のまま残してよいもの（master が読む印）を外した文
fn without_marks(s: &str) -> String {
    s.replace(tako_control::orchestrator::wait::CHOICE_HIGHLIGHT_MARK, "")
}

#[test]
fn sleep_guardとrecoverとwatchの人向けの表示は英語表示に日本語を含まず日本語表示は2365より前とバイト一致(
) {
    let (ja, en) = runs();
    let mut failures = Vec::new();
    for (name, o) in &en.outs {
        // sleep-guard の状態の JSON（stdout）は応答の値（`description` ほか）なので見ない
        let stdout = if name.starts_with("status_") || *name == "set_ok" {
            String::new()
        } else {
            without_marks(&o.stdout)
        };
        if let Some(c) = has_japanese(&stdout).or_else(|| has_japanese(&o.stderr)) {
            failures.push(format!(
                "{name}: 英語表示に日本語（{c}）が混ざる:\n{}{}",
                o.stdout, o.stderr
            ));
        }
    }
    for (name, o) in &ja.outs {
        let e = en.get(name);
        if o.code != e.code {
            failures.push(format!(
                "{name}: 終了コードが日英で違う（ja {} / en {}）",
                o.code, e.code
            ));
        }
        if name.starts_with("status_") && o.stdout != e.stdout {
            failures.push(format!(
                "{name}: 状態の JSON が日英で違う\n  ja: {}\n  en: {}",
                o.stdout, e.stdout
            ));
        }
        if *name == "status_local" {
            let first = o.stderr.lines().next().unwrap_or("");
            if first != "  取得元: この CLI プロセス（tako アプリは未起動）" {
                failures.push(format!(
                    "{name}: 取得元の行が #2365 より前と違う: {first:?}"
                ));
            }
            continue;
        }
        let Some((stdout, stderr, code)) = expected_ja(name) else {
            continue;
        };
        if o.code != code {
            failures.push(format!(
                "{name}: 日本語表示の終了コードが #2365 より前と違う（want {code} / got {}）",
                o.code
            ));
        }
        let mut pairs = vec![("stderr", &o.stderr, stderr)];
        if let Some(stdout) = stdout {
            pairs.push(("stdout", &o.stdout, stdout));
        }
        for (stream, got, want) in pairs {
            if *got != want {
                failures.push(format!(
                    "{name}: 日本語表示の {stream} が #2365 より前と違う\n  want: {want:?}\n  got:  {got:?}"
                ));
            }
        }
    }
    // 蓋閉じ防止の登録 / 解除は偽の osascript だけを通った（日英 × 登録 2 回 + 解除 1 回。未登録の機械の
    // 解除は osascript を呼ばない）
    if on_macos() {
        let per_lang = if tako_control::sleep_guard::is_sudoers_installed() {
            3
        } else {
            2
        };
        for (lang, run) in [("ja", ja), ("en", en)] {
            if run.osascript_calls != per_lang {
                failures.push(format!(
                    "{lang}: 偽の osascript が {per_lang} 回呼ばれるはずが {} 回",
                    run.osascript_calls
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} 件で落ちた:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// 英語表示の枠の見本（ラベル・字下げ・一覧の 20 桁・master が読む印は日本語と同じ）
#[test]
fn 英語表示の枠はラベルと字下げと印を保つ() {
    let (_, en) = runs();
    assert_eq!(
        en.get("watch_dialog").stdout,
        "WORKER_DIALOG: tako:7 (select)\n  title: Pick a model\n  1. Default ← 現在の選択\n  \
         2. Opus long lab [truncated]\n  note: a dialog that ignores number keys (tako answers by moving \
         with the arrow keys)\n  note: the dialog is taller than the pane and the selection cursor is off \
         screen (answer by number)\n  note: labels are truncated (respond by number, not by label)\n  \
         action: respond\n  respond: tako orchestrator respond --pane 7 --choice <number|label>\n"
    );
    assert_eq!(
        en.get("watch_dead").stdout,
        "WORKER_DEAD: tako:7\n  detail: the agent CLI process has exited (suspected sudden death such \
         as SIGSEGV)\n  resume: (no session ID recorded, so the resume command cannot be built)\n  \
         action: resume_session\n"
    );
    let list = &en.get("recover_list").stdout;
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines[3], "layout.json.bak.3   : (none)", "{list}");
    assert_eq!(
        lines[5], "layout.json.rescued : 1 tab(s) / 2 pane(s) (updated 3 d ago)",
        "{list}"
    );
    assert_eq!(
        lines[6],
        "post-restore wipeout: 1 in a row (after 2 in a row the next launch skips restoring)",
        "ラベルは日本語と同じ 20 桁: {list}"
    );
    let merge = &en.get("recover_merge").stderr;
    assert!(
        merge.contains("\n  tab 2 \"work\": 1 pane(s) (tmux sessions alive 0)\n"),
        "合流したタブの行は日本語と同じ 2 桁の字下げ: {merge}"
    );
    assert_eq!(
        en.get("set_bad_mode").stderr,
        "error: unknown mode: \"bogus\" (one of off / on / while-agents-running)\n"
    );
    if on_macos() {
        let status = &en.get("status_busy").stderr;
        assert!(
            status.starts_with("  source: the tako app (IPC)\n  idle-sleep: assertion held\n"),
            "{status}"
        );
        assert!(status.contains("\n  thermal: serious\n"), "{status}");
    }
}
