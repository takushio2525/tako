//! 実経路: エージェント CLI の導入の実行（`tako_setup_bootstrap` の install = インストーラの取得と実行）と
//! PATH 通し（path / undo-path）の断りが、枠と中身（tako-core の profile の PATH ブロック）ごと表示言語に
//! 従う（Issue #2365 の続き）。
//!
//! 修正前（実測。CLI / MCP と同じ dispatch を GUI の代わりのホストへ通して）: 英語表示でも
//! 「curl も wget も見つかりません …」「インストーラを取得できませんでした …」「取得した内容がインストーラでは
//! ありません …」「… のインストーラが失敗しました …」「… を読めません: …」「ホームディレクトリを特定できません …」
//! のように日本語のまま = dispatch が日本語の口（`install_for` / `ensure_path_for` / `undo_path_for`）の
//! `Err(String)` を素通ししていた。
//!
//! 「使っているシェル（…）の設定ファイルが分かりません」の 2 文はここでは叩かず、
//! `issue1762_dispatch_error_lang.rs` の表で日英を見る。#2550 より前は unix で届かなかった（種別の分からない
//! `$SHELL` も zsh と見なしていた）。届くようになった実経路（未知のシェルで書かずに断る）は
//! `issue2550_unknown_shell_path.rs` が見る。
//!
//! **本物の環境を 1 つも書き換えない・本物のインストーラを取得も実行もしない**（bootstrap はネットから
//! インストーラを取って走らせ、シェルの profile とユーザーの PATH を書く経路）:
//!
//! - `HOME` / `TAKO_DATA_DIR` / `TAKO_ORCHESTRATOR_DIR` / `CLAUDE_CONFIG_DIR` / `TMPDIR`（インストーラの
//!   一時ファイルの置き場）は一時の置き場の下
//! - `PATH` は空の dir、`SHELL` は偽のログインシェル（`-l -c "command -v <名前>"` には `answers/<名前>` に
//!   書いた**砂場の偽物のパス**だけを答え、PATH の読み取りほかには何も答えない）= 本物の curl / wget /
//!   bash / claude / codex を引けない（最初に引けないことを確かめてから先へ進む。引けたら何も叩かずに止まる）
//! - インストーラの取得は偽の curl（URL を見ずに、名乗った置き場へ HTML / 偽のインストーラを書くか、
//!   失敗して終わる）、実行は偽の bash（取得物を読まずに終了コードだけ返す）。本物の取得元へは出ない
//! - 失敗は注入で起こす: 取得手段が無い・名前解決の失敗・黙った失敗・HTML・何も書かない・壊れた
//!   インタプリタ・インストーラの終了コード（137 / 143 / 1 / 0 = 入ったのに引けない）・セルフテスト中・
//!   フォルダの `.zprofile`・書けないホーム・`HOME` を外す
//!
//! 見るもの:
//!
//! - 英語表示: 断りに日本語（かな・漢字）が 1 字も無い
//! - 日本語表示: #2365 より前の文とバイト一致
//!
//! 表示言語と env はプロセス全体の状態なので、**1 本のテスト関数を `run_alone!` の子**で走らせる
//! （#2400）。このファイルは修正前の木でもコンパイルできる API だけを使う（A/B: 修正前は英語表示の
//! 検査で落ちる）。unix だけ: 引けなくする注入（`SHELL` のログインシェル）が unix の探索の経路
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_control::protocol::Request;
use tako_control::setup_bootstrap;
use tako_control::{
    DispatchError, PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost,
    WebViewHost, WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::platform::agent_install::AgentKind;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木だけを持つ（どの要求もペインを作らない）
struct Host {
    workspace: Workspace,
}

impl WorkspaceHost for Host {
    fn workspace(&self) -> &Workspace {
        &self.workspace
    }
    fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }
}

impl SessionHost for Host {
    fn session(&self, _pane: PaneId) -> Option<&TerminalSession> {
        None
    }
    fn attach_session(&mut self, _pane: PaneId, _options: SpawnOptions) {}
    fn detach_session(
        &mut self,
        _pane: PaneId,
        _origin: tako_core::pane_log::CloseOrigin,
        _caller: Option<&str>,
    ) {
    }
}

impl UiStateHost for Host {}
impl PreviewHost for Host {}
impl TmuxHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}
impl SystemHost for Host {}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 偽のログインシェル。`__ANSWERS__` は砂場の答えの置き場へ差し替える
const FAKE_SHELL: &str = r#"#!/bin/sh
# 偽のログインシェル（#2365 の砂場）: command -v には answers/<名前> に書いた偽物だけを答える
case "$3" in
  "command -v "*)
    name=${3#command -v }
    if [ -f "__ANSWERS__/$name" ]; then /bin/cat "__ANSWERS__/$name"; exit 0; fi
    exit 1 ;;
esac
exit 1
"#;

/// 偽の curl の本体の頭（`-o` の次の引数を保存先として `$dest` に取る）
const FAKE_CURL_HEAD: &str = r#"#!/bin/sh
dest=""
while [ $# -gt 0 ]; do
  if [ "$1" = "-o" ]; then dest=$2; fi
  shift
done
"#;

/// 起動できない実行ファイル（実行ビットはあるがインタプリタが無い = ENOENT）
const BROKEN_INTERPRETER: &str = "#!/nonexistent/tako-2365/interp\n";

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    fn bin(&self, name: &str) -> PathBuf {
        self.root.join("bin").join(name)
    }
    fn write_exec(&self, path: &Path, body: &str) {
        std::fs::write(path, body).expect("偽物を書く");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("実行ビット");
    }
    /// 偽のログインシェルに `name` の答え（砂場の偽物）を持たせる / 外す
    fn answer(&self, name: &str, on: bool) {
        let at = self.root.join("answers").join(name);
        if on {
            std::fs::write(&at, self.bin(name).display().to_string()).expect("答え");
        } else {
            let _ = std::fs::remove_file(&at);
        }
    }
    fn curl(&self, body: &str) {
        self.write_exec(&self.bin("curl"), &format!("{FAKE_CURL_HEAD}{body}"));
    }
    fn bash(&self, body: &str) {
        self.write_exec(&self.bin("bash"), &format!("#!/bin/sh\n{body}"));
    }
    /// 名前が `name` の偽のログインシェル（答え方は同じ。名前で profile の種別が決まる）
    fn shell(&self, name: &str) -> PathBuf {
        let at = self.root.join("fakesh").join(name);
        let answers = self.root.join("answers").display().to_string();
        self.write_exec(&at, &FAKE_SHELL.replace("__ANSWERS__", &answers));
        at
    }
}

fn host() -> Host {
    Host {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
    }
}

fn run(request: &Value) -> String {
    let mut host = host();
    let request: Request = serde_json::from_value(request.clone()).expect("要求の形");
    let result: Result<Value, DispatchError> =
        tako_control::dispatch(&mut host, request, PaneOrigin::Cli);
    match result {
        Err(e) => e.to_string(),
        Ok(v) => format!("<断られずに通った: {v}>"),
    }
}

fn bootstrap(action: &str, agent: &str) -> Value {
    json!({ "method": "setup_bootstrap", "params": { "action": action, "agent": agent } })
}

/// 1 ケース = 注入（前段）と要求と #2365 より前の日本語の断り
struct Seen {
    lang: Lang,
    name: String,
    text: String,
    old: String,
}

/// 日英の 2 回ぶん走らせる（表示言語を切り替えた直後の 1 件目も数える）
fn run_both(seen: &mut Vec<Seen>, name: &str, request: &Value, old: &str) {
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        seen.push(Seen {
            lang,
            name: name.to_string(),
            text: run(request),
            old: old.to_string(),
        });
    }
}

/// #2365 より前の `fetch_installer` の取得失敗の文
fn old_fetch_failed(url: &str, detail: &str) -> String {
    format!(
        "インストーラを取得できませんでした: {}\n\
         {}\n\
         ネットワーク接続とプロキシ設定を確認してから再実行してください。\n\
         社内プロキシ環境では {} へのアクセス許可が要ることがあります",
        url, detail, url,
    )
}

#[test]
fn bootstrapの導入とpath通しの断りは表示言語に従う() {
    if tako_core::run_alone!(bootstrapの導入とpath通しの断りは表示言語に従う) {
        return;
    }
    let root = std::env::temp_dir().join(format!("tako-2365bs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("一時の置き場");
    let root = root.canonicalize().expect("正規化");
    for dir in [
        "home",
        "data",
        "orch",
        "ccd",
        "tmp",
        "empty-path",
        "bin",
        "answers",
        "fakesh",
    ] {
        std::fs::create_dir_all(root.join(dir)).expect("一時の dir");
    }
    let sb = Sandbox { root: root.clone() };
    // 本番の設定・データを読み書きしない（どの置き場も一時の場所の下）
    std::env::set_var("HOME", sb.home());
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    std::env::set_var("TAKO_ORCHESTRATOR_DIR", root.join("orch"));
    std::env::set_var("CLAUDE_CONFIG_DIR", root.join("ccd"));
    std::env::set_var("TMPDIR", root.join("tmp"));
    std::env::remove_var("CODEX_HOME");
    std::env::remove_var("TAKO_SELF_TEST");
    std::env::set_var("PATH", root.join("empty-path"));
    let zsh = sb.shell("zsh");
    std::env::set_var("SHELL", &zsh);
    // 本物の CLI・取得手段・インタプリタを引けない（上の doc）
    for name in [
        "claude", "codex", "agy", "brew", "curl", "wget", "bash", "sh",
    ] {
        assert!(
            tako_core::platform::exe::find(name).is_none(),
            "{name} を引けてしまう（注入が効いていない）ので本物を起こしうる。先へ進まない"
        );
    }
    assert_eq!(
        std::env::temp_dir(),
        root.join("tmp"),
        "インストーラの一時ファイルの置き場が砂場にない"
    );

    let recipe = setup_bootstrap::recipe_for(AgentKind::Claude);
    let url = recipe.source.url.to_string();
    let plan = setup_bootstrap::install_plan_for(AgentKind::Claude).expect("計画");
    assert!(
        plan.can_run,
        "この環境では代行しない（取得の経路へ届かない）"
    );
    let launcher = tako_core::paths::shorten_home(&plan.launcher.to_string_lossy());
    let codex = setup_bootstrap::recipe_for(AgentKind::Codex);
    let install = bootstrap("install", "claude");

    let original = i18n::lang();
    let mut seen = Vec::new();

    // ---- install ----
    std::env::set_var("TAKO_SELF_TEST", "1");
    run_both(
        &mut seen,
        "install（セルフテスト中）",
        &install,
        "セルフテスト中は実インストールを行いません（dry_run でのみ呼べます）",
    );
    std::env::remove_var("TAKO_SELF_TEST");
    run_both(
        &mut seen,
        "install（curl も wget も無い）",
        &install,
        "curl も wget も見つかりません。どちらかを導入してから再実行してください\n\
         （macOS なら通常 curl が標準で入っています）",
    );

    sb.answer("curl", true);
    let curl = sb.bin("curl").display().to_string();
    sb.write_exec(&sb.bin("curl"), BROKEN_INTERPRETER);
    let enoent = std::process::Command::new(sb.bin("curl"))
        .output()
        .expect_err("起動できない")
        .to_string();
    run_both(
        &mut seen,
        "install（取得手段を起動できない）",
        &install,
        &format!("{curl} を起動できません: {enoent}"),
    );
    sb.curl("echo 'curl: (6) Could not resolve host: claude.ai' >&2\nexit 6\n");
    run_both(
        &mut seen,
        "install（名前を引けない = 英語の stderr を包む）",
        &install,
        &old_fetch_failed(&url, "（curl: (6) Could not resolve host: claude.ai）"),
    );
    sb.curl("exit 22\n");
    run_both(
        &mut seen,
        "install（取得手段が黙って失敗）",
        &install,
        &old_fetch_failed(&url, &format!("（{curl} が exit 22）")),
    );
    sb.curl("printf '<html><body>proxy error</body></html>\\n' > \"$dest\"\n");
    run_both(
        &mut seen,
        "install（取れたのが HTML）",
        &install,
        &format!(
            "取得した内容がインストーラではありません（{} が HTML かエラーページを返しています）。\n\
             社内プロキシやネットワーク制限が疑われます。ブラウザで {} を開いて中身を確認するか、\n\
             ネットワークを変えて再実行してください",
            url, url
        ),
    );
    sb.curl("exit 0\n");
    run_both(
        &mut seen,
        "install（取れたと言って何も書かない）",
        &install,
        &format!("取得したインストーラを読めません: {enoent}"),
    );

    // ここから先は偽のインストーラ（中身は exit 0 だけ）を取り、偽の bash が中身を読まずに終わる
    sb.curl("printf '#!/bin/sh\\n# 砂場の偽のインストーラ\\nexit 0\\n' > \"$dest\"\n");
    sb.answer("bash", true);
    let bash = sb.bin("bash").display().to_string();
    sb.write_exec(&sb.bin("bash"), BROKEN_INTERPRETER);
    run_both(
        &mut seen,
        "install（インタプリタを起動できない）",
        &install,
        &format!("{bash} を起動できません: {enoent}"),
    );
    sb.bash("echo Killed >&2\nexit 137\n");
    run_both(
        &mut seen,
        "install（メモリ不足 = exit 137）",
        &install,
        "インストールがメモリ不足で中断されました（exit 137）。\
         空きメモリを増やしてから再実行してください\nKilled",
    );
    sb.bash("exit 143\n");
    run_both(
        &mut seen,
        "install（強制終了 = exit 143・出力なし）",
        &install,
        "インストールが途中で強制終了しました（exit 143）。\
         もう一度実行するか、上に出ているメッセージを確認してください",
    );
    sb.bash("echo 'error: Permission denied' >&2\nexit 1\n");
    run_both(
        &mut seen,
        "install（インストーラが失敗 = 英語の出力を包む）",
        &install,
        &format!(
            "{} のインストーラが失敗しました（exit 1）。上に出ているエラーを確認してください。\
             解決しない場合は {} を参照してください\nerror: Permission denied",
            AgentKind::Claude.product(),
            recipe.troubleshoot_url,
        ),
    );
    // codex のインストーラは sh で走る（手順の側のデータ）ので、同じ偽物を sh としても答える
    std::fs::copy(sb.bin("bash"), sb.bin("sh")).expect("偽の sh");
    sb.answer("sh", true);
    run_both(
        &mut seen,
        "install --agent codex（その系統の案内先）",
        &bootstrap("install", "codex"),
        &format!(
            "{} のインストーラが失敗しました（exit 1）。上に出ているエラーを確認してください。\
             解決しない場合は {} を参照してください\nerror: Permission denied",
            AgentKind::Codex.product(),
            codex.troubleshoot_url,
        ),
    );
    sb.bash("exit 0\n");
    run_both(
        &mut seen,
        "install（入ったと言うのに引けない）",
        &install,
        &format!(
            "インストーラは正常終了しましたが {} が見つかりません。\n\
             `{}` を手で実行して、出力に出るエラーを確認してください",
            launcher, recipe.source.official_command
        ),
    );
    sb.answer("curl", false);
    sb.answer("bash", false);
    sb.answer("sh", false);

    // ---- path / undo-path ----
    let zprofile = sb.home().join(".zprofile");
    std::fs::create_dir_all(&zprofile).expect("フォルダの .zprofile");
    let eisdir = std::fs::read(&zprofile).expect_err("フォルダ").to_string();
    for action in ["path", "undo-path"] {
        run_both(
            &mut seen,
            &format!("{action}（.zprofile がフォルダ = 読めない）"),
            &bootstrap(action, "claude"),
            &format!("{} を読めません: {eisdir}", zprofile.display()),
        );
    }
    std::fs::remove_dir(&zprofile).expect("戻す");
    std::fs::set_permissions(sb.home(), std::fs::Permissions::from_mode(0o555)).expect("書けない");
    let eacces = std::fs::write(&zprofile, b"")
        .expect_err("書けない")
        .to_string();
    run_both(
        &mut seen,
        "path（ホームが書けない）",
        &bootstrap("path", "claude"),
        &format!("{} へ書けません: {eacces}", zprofile.display()),
    );
    std::fs::set_permissions(sb.home(), std::fs::Permissions::from_mode(0o755)).expect("戻す");

    // ---- HOME を外す（何も書かない） ----
    std::env::remove_var("HOME");
    for action in ["install", "path", "undo-path"] {
        run_both(
            &mut seen,
            &format!("{action}（HOME が無い）"),
            &bootstrap(action, "claude"),
            "ホームディレクトリを特定できません（HOME が未設定）",
        );
    }
    std::env::set_var("HOME", sb.home());
    i18n::set_lang(original);

    let mut failures = Vec::new();
    let mut en_japanese = 0;
    let mut ja_same = 0;
    for s in &seen {
        println!("[{:?}] {}: {}", s.lang, s.name, s.text);
        match s.lang {
            Lang::Ja => {
                if s.text == s.old {
                    ja_same += 1;
                } else {
                    failures.push(format!(
                        "[Ja] {}: #2365 より前の文と違う\n  want: {}\n  got:  {}",
                        s.name, s.old, s.text
                    ));
                }
            }
            Lang::En => {
                if let Some(ch) = has_japanese(&s.text) {
                    en_japanese += 1;
                    failures.push(format!(
                        "[En] {}: 英語表示に日本語（{ch}）が混ざる: {}",
                        s.name, s.text
                    ));
                }
                if s.text.starts_with("<断られずに通った") {
                    failures.push(format!("[En] {}: 断られなかった: {}", s.name, s.text));
                }
            }
        }
    }
    // 何も書いていない: profile もランチャーも無い・インストーラの一時ファイルは片付いている
    assert!(!zprofile.exists(), ".zprofile を書いた");
    assert!(
        !sb.home().join(".local").exists(),
        "ランチャーの置き場を作った"
    );
    let leftovers: Vec<_> = std::fs::read_dir(root.join("tmp"))
        .expect("一時の置き場")
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftovers.is_empty(),
        "インストーラの一時ファイルが残った: {leftovers:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
    let total = seen.len() / 2;
    println!("英語表示で日本語入り: {en_japanese} / {total}（ケース数）");
    println!("日本語表示で #2365 より前とバイト一致: {ja_same} / {total}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
