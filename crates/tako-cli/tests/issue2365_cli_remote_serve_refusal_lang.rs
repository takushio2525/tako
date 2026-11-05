//! 実経路: `tako remote start` が、子の `tako remote serve` の起動前の断り（Tailscale の不足項目・serve の取り合い・
//! 到達手段・待ち受け・serve から待ち受けへ届かない 502）を表示言語で返す（Issue #2365 の続き）。
//!
//! 修正前（実測）: 子が `io::Error` の日本語の固定文を stderr に書き、親（`remote::spawn_daemon`）がそれを手を
//! 加えずに載せていたので、英語表示でも「Tailscale のセットアップが完了していないため、remote サーバーを起動
//! できません。…」「tailscale serve に既存の設定があります…」「永続バックエンド（none）にアウトオブプロセス
//! 到達手段が無い。…」ほかが日本語のまま出た。
//!
//! **ビルドした `tako` をそのまま**叩く（GUI の要らない手元の処理。子の `tako remote serve` も同じバイナリ）。
//! 見ること:
//! - 英語表示: stderr に日本語（かな・漢字・全角の記号）が 1 字も無い
//! - 日本語表示: #2365 より前の出力とバイト一致（数字を `0` に均し、砂場の dir を伏せる）
//! - stdout は日英で同じ（断りは stdout に何も書かない）・終了コードは日英で同じ
//! - 子を手で叩いた（親が頼んでいない = 旧版の親と同じ）ときの文は従来どおり日本語、頼んだときは印つきの 1 行
//!
//! Tailscale は砂場の偽物（`TAKO_TAILSCALE_BIN`。`status` / `serve status` の答えを砂場のファイルで変える）。
//! 隔離中の serve は偽の tailscale なら通る（#2619 の門）ので、本番の tailnet・tailscaled には触れない。
//! 子は Tailscale の段より前に到達手段（tmux）を確かめるので、tmux が無い環境では Tailscale と serve のケースは
//! 走らせない。**走ったかどうかを必ず stderr に 1 行出し**、`I2365S_REQUIRE_TMUX=1` なら走らなかったことを
//! 失敗にする（手元の受け入れで skip でないことを示す）。
//!
//! A/B: `TAKO_2365_REFUSAL_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は英語表示の検査で落ちる）。
//! `I2365S_DUMP=<dir>` を付けると全ケースの出力を書き出す（修正前後の `diff -r` 用）。
//! 本番の data dir・GUI・tailscale へ届かないよう、`TAKO_*` を全部外して一時の置き場だけを渡す。
//!
//! unix だけで走らせる（偽の tailscale が sh のスクリプト・Unix domain socket の長さの上限）。文の日英の対は
//! OS に依らない 1 か所（`ServeRefusal::text_in` / `MissingItem::describe_in`）で、両 OS の番犬
//! （`crates/tako-control/tests/issue1762_dispatch_error_lang.rs`）が表で見る
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2365_REFUSAL_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2365_REFUSAL_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

const NODE: &str = "fake-2365s.tailfake.ts.net";

/// 偽の tailscale。`status` は `status.json`（`status.fail` があれば失敗 = tailscaled が居ない）、`serve status` は
/// `serve.json`、`serve --bg` は張った形を `serve.json` へ書き、`serve --https=443 off` は空に戻す
const FAKE_TAILSCALE: &str = "#!/bin/sh\n\
    D=\"$(dirname \"$0\")\"\n\
    while [ \"$1\" = \"--socket\" ]; do shift 2; done\n\
    case \"$1\" in\n\
    --version) echo 1.0.0-fake; exit 0 ;;\n\
    status) [ -e \"$D/status.fail\" ] && exit 1; cat \"$D/status.json\"; exit 0 ;;\n\
    serve)\n\
    \x20 shift\n\
    \x20 case \"$1\" in\n\
    \x20 status) cat \"$D/serve.json\"; exit 0 ;;\n\
    \x20 --bg) printf '{\"Web\":{\"fake-2365s.tailfake.ts.net:443\":{\"Handlers\":{\"/\":{\"Proxy\":\"%s\"}}}}}\\n' \"$3\" > \"$D/serve.json\"; exit 0 ;;\n\
    \x20 --https=443) [ \"$2\" = off ] && { echo '{}' > \"$D/serve.json\"; exit 0; }; exit 1 ;;\n\
    \x20 esac ;;\n\
    esac\n\
    exit 1\n";

/// `tailscale status --json` の答え（`state` は BackendState・`certs` は CertDomains の JSON）
fn status_json(state: &str, certs: &str) -> String {
    format!(
        "{{\"BackendState\":\"{state}\",\"Version\":\"1.0.0-fake\",\"CertDomains\":{certs},\
         \"Self\":{{\"DNSName\":\"{NODE}.\"}}}}\n"
    )
}

/// tailnet で `/` を `target` へ向けた serve の設定（`path` を変えるとパス分けの形）
fn serve_json(path: &str, target: &str) -> String {
    format!("{{\"Web\":{{\"{NODE}:443\":{{\"Handlers\":{{\"{path}\":{{\"Proxy\":\"{target}\"}}}}}}}}}}\n")
}

/// Unix domain socket の長さの上限（macOS 104・Linux 108 バイト）を確実に越える state の置き場の名前
const LONG_DIR: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// 1 ケース分の砂場: data（表示言語）・発見の置き場・remote の state・ホーム `hm`・偽の tailscale の置き場 `ts`・
/// tmux のソケット名（子が tmux のサーバーを起こしていたら終わるときに畳む）
struct Sandbox {
    dir: ScratchDir,
    real: PathBuf,
    tmux: String,
}

impl Sandbox {
    fn new(lang: &str, name: &str) -> Self {
        let dir = ScratchDir::new("i2365s");
        for d in ["data", "disc", "state", "hm", "ts"] {
            let d = dir.path().join(d);
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        std::fs::write(
            dir.path().join("data/settings.json"),
            format!("{{\"language\":\"{lang}\"}}"),
        )
        .expect("settings.json を書ける");
        let ts = dir.path().join("ts/tailscale");
        std::fs::write(&ts, FAKE_TAILSCALE).expect("偽 tailscale を書ける");
        std::fs::set_permissions(&ts, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let real = std::fs::canonicalize(dir.path()).expect("砂場の実体を引ける");
        // 同じプロセスの 2 本のテストが同じケースを並行で走らせるので、通し番号で器を分ける
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let tmux = format!("t2365s-{}-{seq}-{lang}-{name}", std::process::id());
        let sb = Self { dir, real, tmux };
        sb.put_ts(
            "status.json",
            &status_json("Running", &format!("[\"{NODE}\"]")),
        );
        sb.put_ts("serve.json", "{}\n");
        sb
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// 偽の tailscale の答えのファイルを書く
    fn put_ts(&self, name: &str, text: &str) {
        std::fs::write(self.path().join("ts").join(name), text)
            .unwrap_or_else(|e| panic!("ts/{name} を書ける: {e}"));
    }

    /// `TAKO_*` を外して隔離の置き場だけを渡した `tako <args>`（出力の砂場の dir は `<SB>` に置換）
    fn run(&self, args: &[&str], case: &Case) -> Out {
        let mut command = Command::new(tako());
        command
            .args(args)
            .current_dir(self.path().join("hm"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy().to_string();
            if key.starts_with("TAKO_") || key.starts_with("CLAUDE") || key == "CODEX_HOME" {
                command.env_remove(key);
            }
        }
        let state = match case.state_dir {
            Some(sub) => self.path().join("state").join(sub),
            None => self.path().join("state"),
        };
        std::fs::create_dir_all(&state).expect("state の置き場を作れる");
        command
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.path().join("data"))
            .env("TAKO_DISCOVERY_DIR", self.path().join("disc"))
            .env("TAKO_REMOTE_STATE_DIR", &state)
            .env(
                "TAKO_TAILSCALE_BIN",
                self.path().join("ts").join(case.tailscale),
            )
            .env("TAKO_TMUX_SOCKET", &self.tmux)
            .env("HOME", self.path().join("hm"));
        for (key, value) in case.env {
            command.env(key, value);
        }
        let out = command.output().expect("tako が終わる");
        let real = self.real.display().to_string();
        let root = self.path().display().to_string();
        let norm = |b: &[u8]| {
            String::from_utf8_lossy(b)
                .replace(&real, "<SB>")
                .replace(&root, "<SB>")
        };
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: norm(&out.stdout),
            stderr: norm(&out.stderr),
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // 子が起動の途中まで進んだケース（502）でも tmux のサーバーを残さない（無ければ何もしない）
        tako_core::tmux_backend::kill_server(&self.tmux);
    }
}

#[derive(Clone)]
struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// 1 ケース: 名前・子が tmux の検査を越えるか（越えるなら tmux が要る）・偽の tailscale の名前（無い名前 = 未導入）・
/// state の置き場の下の dir・足す env・偽の tailscale の答えの書き換え
struct Case {
    name: &'static str,
    needs_tmux: bool,
    tailscale: &'static str,
    state_dir: Option<&'static str>,
    env: &'static [(&'static str, &'static str)],
    setup: fn(&Sandbox),
}

const fn case(name: &'static str, setup: fn(&Sandbox)) -> Case {
    Case {
        name,
        needs_tmux: true,
        tailscale: "tailscale",
        state_dir: None,
        env: &[],
        setup,
    }
}

fn no_setup(_: &Sandbox) {}

const CASES: &[Case] = &[
    Case {
        tailscale: "no-such-tailscale",
        ..case("ts-cli-missing", no_setup)
    },
    case("ts-daemon-down", |sb| sb.put_ts("status.fail", "")),
    case("ts-not-logged-in", |sb| {
        sb.put_ts("status.json", &status_json("NeedsLogin", "[]"))
    }),
    case("ts-stopped", |sb| {
        sb.put_ts(
            "status.json",
            &status_json("Stopped", &format!("[\"{NODE}\"]")),
        )
    }),
    case("ts-https-off", |sb| {
        sb.put_ts("status.json", &status_json("Running", "[]"))
    }),
    case("serve-occupied", |sb| {
        sb.put_ts("serve.json", &serve_json("/", "http://localhost:3000"))
    }),
    case("serve-foreign", |sb| {
        sb.put_ts("serve.json", &serve_json("/api", "http://localhost:3000"))
    }),
    Case {
        env: &[("TAKO_1038_INJECT_UNREACHABLE", "1")],
        ..case("backend-unreachable", no_setup)
    },
    Case {
        needs_tmux: false,
        env: &[("TAKO_BACKEND", "none")],
        ..case("no-tmux", no_setup)
    },
    Case {
        needs_tmux: false,
        env: &[("TAKO_REMOTE_ENDPOINT", "bogus")],
        ..case("endpoint-invalid", no_setup)
    },
    Case {
        needs_tmux: false,
        state_dir: Some(LONG_DIR),
        env: &[("TAKO_REMOTE_ENDPOINT", "unix")],
        ..case("socket-too-long", no_setup)
    },
];

/// 1 言語分の全ケースの出力（名前, 出力）と、tmux が要るケースを走らせたか
fn run_lang(lang: &str) -> (Vec<(&'static str, Out)>, bool) {
    let tmux = tako_core::backend::capabilities().detached_access;
    let mut outs = Vec::new();
    for case in CASES.iter().filter(|c| tmux || !c.needs_tmux) {
        let sb = Sandbox::new(lang, case.name);
        (case.setup)(&sb);
        outs.push((case.name, sb.run(&["remote", "start"], case)));
    }
    if let Some(dir) = std::env::var_os("I2365S_DUMP").map(PathBuf::from) {
        let _ = std::fs::create_dir_all(&dir);
        for (name, out) in &outs {
            let _ = std::fs::write(
                dir.join(format!("{lang}-{name}.txt")),
                format!(
                    "exit={}\n--- stdout\n{}--- stderr\n{}",
                    out.code, out.stdout, out.stderr
                ),
            );
        }
    }
    (outs, tmux)
}

/// tmux が要るケースを走らせたかを 1 行で残す（`I2365S_REQUIRE_TMUX=1` なら走らないことを失敗にする）
fn report_tmux_cases(ran: bool) {
    let n = CASES.iter().filter(|c| c.needs_tmux).count();
    if ran {
        eprintln!("[issue2365 remote serve refusal] tmux が要るケース: 走った（{n} 件）");
    } else {
        eprintln!(
            "[issue2365 remote serve refusal] tmux が要るケース: tmux が無い（子は Tailscale の段の前で断る）ので走らせていない（{n} 件）"
        );
        assert!(
            std::env::var("I2365S_REQUIRE_TMUX").as_deref() != Ok("1"),
            "I2365S_REQUIRE_TMUX=1 なのに tmux が要るケースを走らせていない（tmux が無い）"
        );
    }
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 数字の並びを `0` 1 字に均す（ポート・socket の長さは実行ごとに桁数も変わる）
fn norm(s: &str) -> String {
    let mut folded = String::with_capacity(s.len());
    let mut in_digits = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                folded.push('0');
            }
            in_digits = true;
        } else {
            folded.push(c);
            in_digits = false;
        }
    }
    folded
}

/// #2365 より前の日本語表示の stderr（`norm` 済み。`<SB>` は砂場）。修正前のバイナリで採った出力と同じ
fn old_ja(name: &str) -> String {
    let setup = |item: &str| {
        format!(
            "error: Tailscale のセットアップが完了していないため、remote サーバーを起動できません。\n\
             不足項目:\n\
             \x20 - {item}\n\
             `tako remote setup` を実行してください。\n"
        )
    };
    match name {
        "ts-cli-missing" => setup("Tailscale が未導入（App Store 版アプリ または brew install tailscale）"),
        "ts-daemon-down" => {
            setup("Tailscale デーモンが起動していない（アプリを起動するか tailscaled を起動）")
        }
        "ts-not-logged-in" => setup("Tailscale にログインしていない（tailscale up でブラウザ認証）"),
        "ts-stopped" => setup("Tailscale の接続が有効でない（状態: Stopped。tailscale up で再接続）"),
        "ts-https-off" => setup(
            "tailnet の HTTPS 証明書が未有効\
             （https://login.tailscale.com/admin/dns で MagicDNS と HTTPS Certificates を有効化）",
        ),
        "serve-occupied" => "error: tailscale serve に既存の設定があります（HTTPS:0 → http://localhost:0）。\
                             ユーザー設定を壊さないため上書きしません。tako の以前の設定の残骸で\
                             あれば `tailscale serve --https=0 off` で解除してください\n"
            .into(),
        "serve-foreign" => "error: tailscale serve に tako 管理外の設定があります（パス分けハンドラ等）。\
                            上書きを避けるため起動を中止しました。`tailscale serve status` で確認してください\n"
            .into(),
        "backend-unreachable" => "error: tailscale serve は HTTPS:0 → http://0.0.0.0:0 を向いていますが、\
                                  tailscaled がその待ち受け先へ接続できていません（HTTP 0）。\n\
                                  次の一手: 0.0.0.0:0 に別のプロセスが割り込んでいないか\
                                  （`tako remote status` の endpoint と `tailscale serve status` の向き先が一致するか）、\
                                  `tailscale serve --https=0 off` で設定を消してから `tako remote start` をやり直してください。\n"
            .into(),
        "no-tmux" => "error: 永続バックエンド（none）にアウトオブプロセス到達手段が無い。\
                      remote サーバーは tako-app の外からペインを操作するため到達手段が必須です\
                      （macOS / Linux では tmux をインストールすると有効になります）\n"
            .into(),
        "endpoint-invalid" => "error: TAKO_REMOTE_ENDPOINT の値が不正です: bogus（tcp | unix）\n".into(),
        "socket-too-long" => format!(
            "error: socket パスが長すぎます（0 バイト、上限 0）。\
             TAKO_REMOTE_STATE_DIR でより短いパスを指定してください: <SB>/state/{LONG_DIR}/tako-remote.sock\n"
        ),
        other => panic!("見本の無いケース: {other}"),
    }
}

#[test]
fn remoteのstartが返す子の起動前の断りは表示言語に従い日本語は2365より前と同じ() {
    let (ja, ran_ja) = run_lang("ja");
    let (en, ran_en) = run_lang("en");
    report_tmux_cases(ran_ja && ran_en);
    assert_eq!(ja.len(), en.len(), "日英でケースの数が違う");
    let mut failures = Vec::new();
    for ((name, j), (_, e)) in ja.iter().zip(&en) {
        // どのケースも断る（終了コード 1・stdout は空）
        if j.code == 0 || e.code != j.code {
            failures.push(format!(
                "{name}: 終了コード（ja {} / en {}）が断りの形でない・日英で違う\n  ja: {}\n  en: {}",
                j.code, e.code, j.stderr, e.stderr
            ));
        }
        if j.stdout != e.stdout || !j.stdout.is_empty() {
            failures.push(format!(
                "{name}: stdout が日英で違う・断りなのに何か書いた\n  ja: {}\n  en: {}",
                j.stdout, e.stdout
            ));
        }
        for line in e.stderr.lines() {
            if let Some(c) = has_japanese(line) {
                failures.push(format!("{name}: 英語表示に日本語（{c}）: {line}"));
            }
        }
        let (want, got) = (old_ja(name), norm(&j.stderr));
        if got != want {
            failures.push(format!(
                "{name}: 日本語表示が #2365 より前と違う\n  want: {want:?}\n  got:  {got:?}"
            ));
        }
        if j.stderr.lines().count() != e.stderr.lines().count() {
            failures.push(format!(
                "{name}: 行の数が日英で違う（ja {} / en {}）\n  en: {}",
                j.stderr.lines().count(),
                e.stderr.lines().count(),
                e.stderr
            ));
        }
        // 子と親のあいだの印（JSON の 1 行）は利用者へ出さない
        if j.stderr.contains("tako-remote-serve-refusal")
            || e.stderr.contains("tako-remote-serve-refusal")
        {
            failures.push(format!(
                "{name}: 子が渡した印つきの行がそのまま出ている\n  en: {}",
                e.stderr
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn 子の起動前の断りの英語の見本() {
    // 英語の文の要点（日英の対の取り違え・言い換えの書き忘れを拾う。全文の一致は番犬の表が見る）
    let want: &[(&str, &[&str])] = &[
        (
            "ts-cli-missing",
            &["error: Tailscale is not fully set up, so the remote server cannot start.\n\
               Missing:\n\
               \x20 - Tailscale is not installed (the App Store app, or brew install tailscale)\n\
               Run `tako remote setup`.\n"],
        ),
        ("ts-daemon-down", &["  - the Tailscale daemon is not running (open the app, or start tailscaled)"]),
        ("ts-not-logged-in", &["  - not logged in to Tailscale (tailscale up signs in through the browser)"]),
        (
            "ts-stopped",
            &["  - the Tailscale connection is not up (state: Stopped; reconnect with tailscale up)"],
        ),
        ("ts-https-off", &["  - HTTPS certificates are not enabled for the tailnet"]),
        (
            "serve-occupied",
            &["error: tailscale serve already has a setting (HTTPS:0 → http://localhost:0). It is not overwritten"],
        ),
        (
            "serve-foreign",
            &["error: tailscale serve has a setting that tako does not manage (path handlers or similar)."],
        ),
        (
            "backend-unreachable",
            &[
                "error: tailscale serve points HTTPS:0 to http://0.0.0.0:0, but tailscaled cannot connect",
                "\nNext step: check that no other process took 0.0.0.0:0",
            ],
        ),
        (
            "no-tmux",
            &["error: the persistent backend (none) cannot be reached from outside the app."],
        ),
        (
            "endpoint-invalid",
            &["error: invalid value for TAKO_REMOTE_ENDPOINT: bogus (tcp | unix)\n"],
        ),
        (
            "socket-too-long",
            &["error: the socket path is too long (0 bytes, limit 0). Set a shorter path with TAKO_REMOTE_STATE_DIR: <SB>/state/"],
        ),
    ];
    let (en, ran) = run_lang("en");
    report_tmux_cases(ran);
    let mut failures = Vec::new();
    for (name, needles) in want {
        let Some((_, out)) = en.iter().find(|(n, _)| n == name) else {
            let needs_tmux = CASES.iter().any(|c| c.name == *name && c.needs_tmux);
            if ran || !needs_tmux {
                failures.push(format!("{name}: ケースが無い"));
            }
            continue;
        };
        let got = norm(&out.stderr);
        for needle in *needles {
            if !got.contains(needle) {
                failures.push(format!("{name}: `{needle}` が無い\n  got: {got}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// 子を手で叩いた（親が頼んでいない = 旧版の親が起こしたのと同じ）ときの文は、英語表示でも従来どおり日本語
/// （旧版の親の自動復帰の記録を日本語のまま保つ）。親が頼んだときは印つきの JSON 1 行（型と日本語の文）
#[test]
fn 子は親が頼んだときだけ断りを型で渡す() {
    let probe = Case {
        needs_tmux: false,
        env: &[("TAKO_REMOTE_ENDPOINT", "bogus")],
        ..case("serve-direct", no_setup)
    };
    let sb = Sandbox::new("en", "serve-direct");
    let plain = sb.run(&["remote", "serve"], &probe);
    assert_eq!(plain.code, 1, "{}", plain.stderr);
    assert_eq!(
        plain.stderr, "error: TAKO_REMOTE_ENDPOINT の値が不正です: bogus（tcp | unix）\n",
        "頼まれていない子の文が従来と違う"
    );
    let asked = Case {
        env: &[
            ("TAKO_REMOTE_ENDPOINT", "bogus"),
            ("TAKO_REMOTE_SERVE_REFUSAL", "json"),
        ],
        ..probe
    };
    let typed = sb.run(&["remote", "serve"], &asked);
    assert_eq!(typed.code, 1, "{}", typed.stderr);
    let line = typed
        .stderr
        .trim_end()
        .strip_prefix("error: tako-remote-serve-refusal: ")
        .unwrap_or_else(|| panic!("印つきの 1 行で渡していない: {}", typed.stderr));
    let envelope: serde_json::Value = serde_json::from_str(line).expect("印の後ろは JSON");
    assert_eq!(
        envelope["refusal"]["kind"], "endpoint_invalid",
        "{envelope}"
    );
    assert_eq!(envelope["refusal"]["value"], "bogus", "{envelope}");
    assert_eq!(
        envelope["ja"], "TAKO_REMOTE_ENDPOINT の値が不正です: bogus（tcp | unix）",
        "{envelope}"
    );
}
