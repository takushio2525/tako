//! 基板とポートの読み書き（#1952）の単体テスト。ファイルシステムは本物の一時 dir、arduino-cli は
//! 偽の口（[`FakeCli`]）= 実物は起こさない（実物の経路は `scripts/test-run-board-1952.sh`）。
//!
//! 偽の `board attach` は `sketch.yaml` の鍵を書き換えるだけの模型（**本番のコードは書式を真似しない**。
//! 書くのは常に arduino-cli = 番犬 `issue1952_sketch_board_watchdog`）

use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Mutex;
use std::time::Duration;

use tako_core::platform::exe::BoundedFind;
use tako_core::probe::Outcome;
use tako_core::test_residue::ScratchDir;

use super::*;

const BUDGET: Duration = Duration::from_secs(1);

/// 実測 1.2.0 の形（Windows の機の形 = **実機は未確認**。COM ポートと、コアの入っていない基板）
const LIST: &str = r#"{"detected_ports":[
    {"matching_boards":[{"name":"Arduino Uno","fqbn":"arduino:avr:uno"}],
     "port":{"address":"COM3","label":"COM3","protocol":"serial","protocol_label":"Serial Port (USB)"}},
    {"matching_boards":[{"name":"Arduino UNO R4 WiFi","fqbn":"arduino:renesas_uno:unor4wifi"}],
     "port":{"address":"COM7","label":"COM7","protocol":"serial","protocol_label":"Serial Port (USB)"}},
    {"port":{"address":"COM1","label":"COM1","protocol":"serial","protocol_label":"Serial Port"}}]}"#;

/// 実測 1.2.0 の `board listall --format jsonmini` を 1 コア 3 基板へ縮めたもの
const LISTALL: &str = r#"{"boards":[
    {"name":"Arduino UNO","fqbn":"arduino:avr:uno","platform":{"metadata":{"id":"arduino:avr"},"release":{"boards":[{"name":"Arduino UNO","fqbn":"arduino:avr:uno"}]}}},
    {"name":"Arduino Nano","fqbn":"arduino:avr:nano","platform":{"metadata":{"id":"arduino:avr"}}},
    {"name":"Arduino Mega or Mega 2560","fqbn":"arduino:avr:mega","platform":{"metadata":{"id":"arduino:avr"}}}]}"#;

fn ok_status() -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }
}

/// 偽の arduino-cli（呼ばれた引数を全部覚える）
#[derive(Default)]
struct FakeCli {
    list: Option<String>,
    listall: Option<String>,
    calls: Mutex<Vec<String>>,
}

impl FakeCli {
    fn real() -> Self {
        Self {
            list: Some(LIST.into()),
            listall: Some(LISTALL.into()),
            ..Self::default()
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// `board attach` の模型: 渡された鍵だけを書き換え、他の行は残す（実測 1.2.0 の振る舞い）
    fn attach(args: &[&str]) {
        let sketch = PathBuf::from(args.last().unwrap());
        let yaml = sketch.join("sketch.yaml");
        let mut lines: Vec<String> = std::fs::read_to_string(&yaml)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        for (flag, key) in [("--fqbn=", "default_fqbn"), ("--port=", "default_port")] {
            if let Some(v) = args.iter().find_map(|a| a.strip_prefix(flag)) {
                let line = format!("{key}: {v}");
                match lines.iter_mut().find(|l| l.starts_with(&format!("{key}:"))) {
                    Some(l) => *l = line,
                    None => lines.push(line),
                }
            }
        }
        std::fs::write(&yaml, lines.join("\n") + "\n").unwrap();
    }
}

impl Prober for FakeCli {
    fn find(&self, _: &str, _: Duration) -> BoundedFind {
        BoundedFind {
            path: None,
            timeout: None,
        }
    }

    fn output(&self, _program: &str, args: &[&str], _: Option<&Path>, _: Duration) -> Outcome {
        self.calls.lock().unwrap().push(args.join(" "));
        let stdout = match args {
            ["board", "list", ..] => self.list.clone(),
            ["board", "listall", ..] => self.listall.clone(),
            ["board", "attach", ..] => {
                Self::attach(args);
                Some(String::new())
            }
            _ => None,
        };
        match stdout {
            Some(out) => Outcome::Done {
                status: ok_status(),
                stdout: out.into_bytes(),
                stderr: Vec::new(),
            },
            None => Outcome::Failed {
                label: args.join(" "),
                reason: "fake: no answer".into(),
            },
        }
    }
}

struct Sketch {
    _scratch: ScratchDir,
    root: PathBuf,
}

impl Sketch {
    /// `<箱>/Blink/Blink.ino`（`.git` を置いて上への探索を止める）
    fn new(tag: &str) -> Self {
        let scratch = ScratchDir::new(tag);
        let root = tako_core::platform::path::canonicalize(scratch.path()).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("Blink")).unwrap();
        std::fs::write(
            root.join("Blink").join("Blink.ino"),
            "void setup() {}\nvoid loop() {}\n",
        )
        .unwrap();
        // 偽の arduino-cli の実体（見つかった扱いにする）
        std::fs::write(root.join("arduino-cli"), "").unwrap();
        Self {
            _scratch: scratch,
            root,
        }
    }

    fn dir(&self) -> PathBuf {
        self.root.join("Blink")
    }

    fn ino(&self) -> PathBuf {
        self.dir().join("Blink.ino")
    }

    fn cli(&self) -> PathBuf {
        self.root.join("arduino-cli")
    }

    fn yaml(&self) -> Option<String> {
        std::fs::read_to_string(self.dir().join("sketch.yaml")).ok()
    }

    fn call(
        &self,
        file: &Path,
        edit: BoardEdit<'_>,
        probe: bool,
        cli: &FakeCli,
        found: bool,
    ) -> Result<Value, DispatchError> {
        let path = self.cli();
        let find = move |_: &Path| found.then(|| path.clone());
        handle_path_with(
            crate::run_configs::PathArgs::empty(file),
            edit,
            probe,
            None,
            cli,
            BUDGET,
            &find,
        )
    }
}

fn board(b: &str) -> BoardEdit<'_> {
    BoardEdit {
        board: Some(b),
        port: None,
    }
}

/// 子プロセスに `core install` が 1 度も渡っていない（受け入れ条件 3）
fn no_core_install(cli: &FakeCli) {
    assert!(
        cli.calls().iter().all(|c| !c.contains("core install")),
        "{:?}",
        cli.calls()
    );
}

#[test]
fn 入っているコアの基板とポートを書き言語サーバの欄を返す() {
    let s = Sketch::new("sb-1952-write");
    let cli = FakeCli::real();
    let v = s
        .call(
            &s.ino(),
            BoardEdit {
                board: Some("arduino:avr:uno"),
                port: Some("COM3"),
            },
            true,
            &cli,
            true,
        )
        .unwrap();
    let sketch = s.dir().to_string_lossy().into_owned();
    // 先に listall で検めてから書く。書いた後は board list を聞き直さない（候補は次の読みが聞く）
    assert_eq!(
        cli.calls(),
        vec![
            "board listall --format jsonmini".to_string(),
            format!("board attach --fqbn=arduino:avr:uno --port=COM3 {sketch}"),
        ]
    );
    no_core_install(&cli);
    assert_eq!(
        s.yaml().as_deref(),
        Some("default_fqbn: arduino:avr:uno\ndefault_port: COM3\n")
    );
    let b = &v["board"];
    assert_eq!(b["fqbn"], "arduino:avr:uno");
    assert_eq!(b["name"], "Arduino UNO");
    assert_eq!(b["port"], "COM3");
    assert_eq!(b["core_installed"], true);
    assert_eq!(b["installed_boards"], 3);
    assert!(b["ports"].is_null(), "書いた後は聞いていない = null");
    assert_eq!(v["attached"]["board"], "arduino:avr:uno");
    assert_eq!(v["attached"]["port"], "COM3");

    // 次の読み（background の腕）が繋がっている基板とポートを聞く。コアの入っていない R4 も候補に出る
    let cli = FakeCli::real();
    let v = s
        .call(&s.ino(), BoardEdit::default(), true, &cli, true)
        .unwrap();
    let b = &v["board"];
    let c = b["candidates"].as_array().unwrap();
    assert_eq!(c.len(), 2, "{c:?}");
    assert_eq!(
        (&c[1]["fqbn"], &c[1]["installed"], &c[1]["port"]),
        (
            &json!("arduino:renesas_uno:unor4wifi"),
            &json!(false),
            &json!("COM7")
        )
    );
    assert_eq!(
        b["ports"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["address"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["COM3", "COM7", "COM1"]
    );
}

#[test]
fn コアが入っていない基板は書かず導入のコマンドを示すだけ() {
    let s = Sketch::new("sb-1952-core");
    let cli = FakeCli::real();
    let e = s
        .call(
            &s.ino(),
            board("arduino:renesas_uno:unor4wifi"),
            true,
            &cli,
            true,
        )
        .unwrap_err();
    let msg = e.to_string();
    assert!(msg.contains("core install arduino:renesas_uno"), "{msg}");
    assert_eq!(
        cli.calls(),
        vec!["board listall --format jsonmini".to_string()]
    );
    no_core_install(&cli);
    assert_eq!(s.yaml(), None, "sketch.yaml を作らない");

    // 既にある sketch.yaml も 1 バイトも変えない
    std::fs::write(
        s.dir().join("sketch.yaml"),
        "default_fqbn: arduino:avr:nano\n",
    )
    .unwrap();
    let cli = FakeCli::real();
    assert!(s
        .call(
            &s.ino(),
            board("arduino:renesas_uno:unor4wifi"),
            false,
            &cli,
            true
        )
        .is_err());
    assert_eq!(
        s.yaml().as_deref(),
        Some("default_fqbn: arduino:avr:nano\n")
    );
    no_core_install(&cli);
}

#[test]
fn 書く前に断る境界() {
    let s = Sketch::new("sb-1952-reject");
    // 入っているコアに無い基板 = 近い候補
    let cli = FakeCli::real();
    let e = s
        .call(&s.ino(), board("arduino:avr:megaa"), false, &cli, true)
        .unwrap_err()
        .to_string();
    assert!(e.contains("arduino:avr:mega"), "{e}");
    // 形の崩れた FQBN・空のポート
    for edit in [
        board("foo"),
        BoardEdit {
            board: None,
            port: Some("  "),
        },
    ] {
        let cli = FakeCli::real();
        assert!(matches!(
            s.call(&s.ino(), edit, false, &cli, true),
            Err(DispatchError::InvalidParams(_))
        ));
        assert!(cli.calls().iter().all(|c| !c.starts_with("board attach")));
    }
    // arduino-cli が見つからない
    let cli = FakeCli::real();
    let e = s
        .call(&s.ino(), board("arduino:avr:uno"), false, &cli, false)
        .unwrap_err();
    assert!(matches!(e, DispatchError::Operation(_)), "{e}");
    assert!(cli.calls().is_empty());
    // 一覧を取れない = 確かめずには書かない
    let broken = FakeCli {
        listall: None,
        ..FakeCli::real()
    };
    assert!(s
        .call(&s.ino(), board("arduino:avr:uno"), false, &broken, true)
        .is_err());
    assert!(broken
        .calls()
        .iter()
        .all(|c| !c.starts_with("board attach")));
    // スケッチでないファイル
    let py = s.root.join("a.py");
    std::fs::write(&py, "print(1)\n").unwrap();
    let cli = FakeCli::real();
    assert!(matches!(
        s.call(&py, board("arduino:avr:uno"), false, &cli, true),
        Err(DispatchError::InvalidParams(_))
    ));
    assert!(cli.calls().is_empty());
    assert_eq!(s.yaml(), None);
}

#[test]
fn ポートだけなら一覧を聞かずに書き基板は残す() {
    let s = Sketch::new("sb-1952-port");
    std::fs::write(
        s.dir().join("sketch.yaml"),
        "# 授業用\ndefault_fqbn: arduino:avr:uno\n",
    )
    .unwrap();
    let cli = FakeCli::real();
    let v = s
        .call(
            &s.ino(),
            BoardEdit {
                board: None,
                port: Some("-weird"),
            },
            false,
            &cli,
            true,
        )
        .unwrap();
    assert_eq!(
        cli.calls(),
        vec![format!(
            "board attach --port=-weird {}",
            s.dir().to_string_lossy()
        )]
    );
    assert_eq!(
        s.yaml().as_deref(),
        Some("# 授業用\ndefault_fqbn: arduino:avr:uno\ndefault_port: -weird\n")
    );
    // 同期の腕（probe = false）は候補を聞かない = ports / installed_boards は null
    assert!(v["board"]["ports"].is_null());
    assert!(v["board"]["installed_boards"].is_null());
    assert_eq!(v["board"]["port"], "-weird");
}

#[test]
fn 読むだけの同期の腕は子プロセスを起こさない() {
    let s = Sketch::new("sb-1952-read");
    std::fs::write(
        s.dir().join("sketch.yaml"),
        "default_fqbn: arduino:avr:uno\ndefault_profile: uno\n",
    )
    .unwrap();
    let cli = FakeCli::real();
    let v = s
        .call(&s.ino(), BoardEdit::default(), false, &cli, true)
        .unwrap();
    assert!(cli.calls().is_empty());
    let b = &v["board"];
    assert_eq!(b["fqbn"], "arduino:avr:uno");
    assert_eq!(b["profile"], "uno");
    assert_eq!(b["candidates"].as_array().unwrap().len(), 1);
    // default_profile の注意・探していないので「見つからない」とは言わない
    assert_eq!(b["notes"].as_array().unwrap().len(), 1, "{b}");
    assert!(b["errors"].as_array().unwrap().is_empty(), "{b}");
    assert!(v.get("attached").is_none());

    // background の腕（probe = true）は list と listall を聞く
    let cli = FakeCli::real();
    let v = s
        .call(&s.ino(), BoardEdit::default(), true, &cli, true)
        .unwrap();
    let mut calls = cli.calls();
    calls.sort();
    assert_eq!(
        calls,
        vec![
            "board list --format json".to_string(),
            "board listall --format jsonmini".to_string()
        ]
    );
    assert_eq!(v["board"]["installed_boards"], 3);
    assert_eq!(v["board"]["name"], "Arduino UNO");

    // スケッチでないファイルは board 欄を付けない（#1731 の応答のまま）
    let py = s.root.join("a.py");
    std::fs::write(&py, "print(1)\n").unwrap();
    let v = s
        .call(&py, BoardEdit::default(), true, &FakeCli::real(), true)
        .unwrap();
    assert!(v.get("board").is_none());
}

#[test]
fn 一覧の欄は案内と同じboard_listを1回で共有する() {
    let s = Sketch::new("sb-1952-resolve");
    let bounds = tako_core::project_root::SearchBounds::with_ceilings([s.root.clone()]);
    let cli_path = s.cli().to_string_lossy().into_owned();
    let words: tako_core::runner::RuntimeWords =
        [("arduino_cli".to_string(), vec![cli_path])].into();
    let mut project =
        tako_core::runner_project::detect(Platform::current(), &s.ino(), "", &bounds, &words)
            .expect("スケッチとして検出");
    assert!(project.followup().is_some(), "基板が未選択の案内がある");
    let cli = FakeCli::real();
    let v = resolve_block_with(
        &mut project,
        &s.dir(),
        Some(&s.cli()),
        Some("nano"),
        &cli,
        BUDGET,
        &|_| panic!("board list の案内は共有の経路で詰める"),
    );
    let mut calls = cli.calls();
    calls.sort();
    assert_eq!(
        calls,
        vec![
            "board list --format json".to_string(),
            "board listall --format jsonmini".to_string()
        ],
        "board list は 1 回だけ"
    );
    // 繋がった基板が 2 台なので案内のコマンドは変えず候補を本文へ（#1950 の規則のまま）
    let note = project
        .notes
        .iter()
        .find(|n| n.id == "board_not_selected")
        .unwrap();
    assert!(note.message.contains("arduino:avr:uno"), "{}", note.message);
    assert_eq!(
        v["matches"],
        json!([{ "fqbn": "arduino:avr:nano", "name": "Arduino Nano" }])
    );
    assert!(v["fqbn"].is_null());
    assert!(v["core_installed"].is_null());

    // 見つからなかった（探して無い）ときは理由を返し、何も起こさない
    let quiet = FakeCli::real();
    let none = resolve_block_with(
        &mut project,
        &s.dir(),
        None,
        Some(""),
        &quiet,
        BUDGET,
        &|_| {},
    );
    assert!(quiet.calls().is_empty());
    assert_eq!(none["errors"].as_array().unwrap().len(), 1);
    assert!(none["matches"].is_null());
    assert!(none["ports"].is_null());
}

#[test]
fn 今の基板のコアが無ければ注意に導入のコマンド() {
    let s = Sketch::new("sb-1952-current");
    std::fs::write(
        s.dir().join("sketch.yaml"),
        "default_fqbn: arduino:renesas_uno:unor4wifi\n",
    )
    .unwrap();
    let cli = FakeCli::real();
    let v = s
        .call(&s.ino(), BoardEdit::default(), true, &cli, true)
        .unwrap();
    let b = &v["board"];
    assert_eq!(b["core_installed"], false);
    let notes = b["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n
            .as_str()
            .unwrap()
            .contains("core install arduino:renesas_uno")),
        "{notes:?}"
    );
    no_core_install(&cli);
}

#[test]
fn 前後の空白は落としてから検めて書く() {
    let s = Sketch::new("sb-1952-trim");
    let cli = FakeCli::real();
    s.call(
        &s.ino(),
        BoardEdit {
            board: Some(" arduino:avr:nano "),
            port: Some(" COM3 "),
        },
        false,
        &cli,
        true,
    )
    .unwrap();
    assert_eq!(
        s.yaml().as_deref(),
        Some("default_fqbn: arduino:avr:nano\ndefault_port: COM3\n")
    );
    no_core_install(&cli);
}
