//! visual-test の項目 109（#496 の「解消エージェントを起動」の 3 択）が押すエージェントの偽物（#2726）。
//!
//! ## 何が起きていたか
//!
//! 項目 109 は claude / codex / agy の各ボタンを実クリックし、立ったペインのシェルは起動コマンド
//! （`TAKO_ORCHESTRATOR_ROLE=… codex -c …`）の先頭の素の名前を PATH から引いて**本物の CLI** を起こして
//! いた。隔離 GUI のペインは検証用の設定置き場で動くので codex は未ログインとして立ち、ログインの流れが
//! ブラウザ（tako-vd ではなく利用者の本物の画面）に出ていた。「立ったペインは毎回すぐ閉じる」作りでも、
//! 閉じる前にログインの画面が開いている。
//!
//! ## 直し方
//!
//! - 3 択を押す間だけ、偽の claude / codex / agy（[`stub_text`]。`fake_exec::write_script` で空打ち済み）を
//!   置いた dir を GUI の PATH の先頭へ置く。立つペインは GUI の env を継ぐので、起動コマンドの素の名前は
//!   偽物に当たる。偽物は呼ばれたことを記録し（[`parse_calls`]）、端末から起きたなら目印の 1 行を出して
//!   入力を読み捨てる（後から流し込まれる初回のプロンプトをシェルへ渡さない）
//! - **押す前に確かめる**: 解消エージェントのペインと同じ形（分割 + GUI の env のログインシェル）で立てた
//!   確かめ用のペインで `command -v` を引き（[`which_command`] / [`parse_which`]）、偽物に当たらない名前は
//!   押さない（利用者の rc が PATH を並べ替えた・alias が被さった・器のサーバーが古い env を持つ、等。
//!   [`resolution`]）。押さなかった名前は `TAKO_SELF_TEST_SKIPPED` で名指しする
//! - 判定は「押すとそのエージェントのペインが立ち、そのペインで偽物が端末つきで起きた」
//!
//! 修正前の形へ戻す A/B の env は置かない（戻すと本物のエージェントが起き、利用者の画面にログインが出る）。
//! 検出力はこのファイルの番犬のテストが修正前の断片で示す。
//!
//! ## 番犬（このファイルのテスト）
//!
//! セルフテスト本体（`main.rs` のセルフテストのモジュールと `self_test/*.rs`）の中で、エージェントの CLI を
//! 起こす入口を名指しする。入口は次の 3 つの形。
//!
//! 1. 解消エージェントの 3 択（`GitResolveAgent {`・`git_do_resolve_agent(`・`git_agent_menu_open = true`）。
//!    直前 12 行以内に偽物の確かめ（`.resolves_to_fake(`）が要る
//! 2. エージェントの名前をペインへ打ち込む（`type_text(` / `pty_line(` の引数に `"claude"` 等）。実 claude の
//!    e2e の明示の opt-in（`claude_e2e…` / `TAKO_VISUAL_CLAUDE`）の内側か、偽物の確かめの後に限る
//! 3. スターターの起動導線（`starter_action(` / `starter_action_with_profile(` の Master / Solo / Setup）。
//!    積んだ `tako master` 等が走ると、インストール済みの tako が本物の claude を exec する（セルフテストの
//!    項目 97(e) がこの形で本物の claude を起こしていた。項目 739 も `tako master -<名前>` をペインへ送って
//!    走らせていた）。opt-in の内側か、積んだ直後の取り下げが要る
//!
//! どれも、積んだ起動をその場で取り下げる（#2446 の `withdraw_launch(`）か、入口の行か直前の行に
//! `tako:launch-ok <理由>` があれば通す。worker の spawn・パレットの起動導線（#2446 の入口）は
//! `selftest_launch_withdraw_watchdog` が見る。
//!
//! 端末の無い問い合わせ（`claude auth status` / `codex login status` / `agy models` / `claude agents` /
//! `mcp list`）はこの番犬の範囲外（`tako_control::agent_probe` が隔離 GUI では意図して塞がない = #1261 の設計）。

use std::ffi::{OsStr, OsString};
use std::path::Path;
#[cfg(feature = "visual-test")]
use std::path::PathBuf;

/// 偽物を置くエージェントの名前（`WorkerAgent::ALL` の順。系統が増えれば偽物も番犬の名前も増える）
pub(crate) fn agent_names() -> Vec<&'static str> {
    tako_control::orchestrator::agent::WorkerAgent::ALL
        .iter()
        .map(|a| a.as_str())
        .collect()
}

/// 偽物が端末から起きたときにペインへ出す目印
pub(crate) const MARK: &str = "TAKO_FAKE_AGENT_2726";

/// 偽のエージェントの本文（`/bin/sh`）。呼ばれたこと（名前・端末か・最初の引数）を `log` へ 1 行足す。
///
/// - `--version` / `-v` / `-V` には偽の版を答えて終わる
/// - 端末でない呼び出し（背景の問い合わせ）はそのまま 0 で終わる
/// - 端末から起きたら目印の 1 行を出し、入力を読み捨てて待つ（ペインを閉じた SIGHUP で終わる）。
///   すぐ終わると、続けて流し込まれる初回のプロンプトがシェルにコマンドとして読まれる
pub(crate) fn stub_text(agent: &str, log: &Path) -> String {
    let log = tako_core::shell::quote_for_shell(&log.display().to_string());
    format!(
        "#!/bin/sh\n\
         # tako の visual-test が PATH の先頭へ置く偽の {agent}（#2726）。本物のエージェントの CLI を起こさない\n\
         if [ -t 0 ]; then tty=tty; else tty=pipe; fi\n\
         printf '%s\\t%s\\t%s\\n' {agent} \"$tty\" \"${{1-}}\" >> {log}\n\
         case \"${{1-}}\" in --version|-v|-V) echo '{agent} 0.0.0 (tako fake #2726)'; exit 0 ;; esac\n\
         [ \"$tty\" = tty ] || exit 0\n\
         echo '{MARK} {agent}'\n\
         exec cat > /dev/null\n"
    )
}

/// 偽物の 1 回の呼び出し
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Call {
    pub(crate) agent: String,
    /// 標準入力が端末だった = ペインのシェルから起きた
    pub(crate) tty: bool,
    pub(crate) first_arg: String,
}

/// 呼び出しの記録（[`stub_text`] が書く `名前\t端末か\t最初の引数` の行）を読む。形の崩れた行は飛ばす
pub(crate) fn parse_calls(text: &str) -> Vec<Call> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let agent = parts.next().filter(|a| !a.is_empty())?;
            let tty = match parts.next()? {
                "tty" => true,
                "pipe" => false,
                _ => return None,
            };
            Some(Call {
                agent: agent.to_string(),
                tty,
                first_arg: parts.next().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// 確かめ用のペインへ打つ 1 行。名前ごとに `名前=command -v の答え` を `out` へ書く。
/// 書き終わってから置き場へ移す（読む側が書きかけを読まない）
pub(crate) fn which_command(agents: &[&str], out: &Path) -> String {
    let quote = |p: &Path| tako_core::shell::quote_for_shell(&p.display().to_string());
    let tmp = out.with_extension("tmp");
    format!(
        "for a in {}; do printf '%s=%s\\n' \"$a\" \"$(command -v \"$a\")\"; done > {} && command mv -f {} {}",
        agents.join(" "),
        quote(&tmp),
        quote(&tmp),
        quote(out)
    )
}

/// [`which_command`] の答えを `(名前, command -v の答え)` に読む（見つからない名前は答えが空）
pub(crate) fn parse_which(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (name, found) = line.split_once('=')?;
            (!name.is_empty()).then(|| (name.to_string(), found.trim().to_string()))
        })
        .collect()
}

/// その名前がペインのシェルで何に当たるか
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// 偽物の置き場の実行ファイル
    Fake,
    /// 偽物以外（本物の CLI・alias・関数）。押すと本物が起きうる
    Other(String),
    /// 見つからない（確かめの答えが無い・空）
    Missing,
}

/// `command -v` の答えを分類する（純関数）。置き場の比べは symlink を解いた形でも見る
/// （macOS の一時 dir は `/var` → `/private/var`）
pub(crate) fn resolution(found: Option<&str>, bin: &Path) -> Resolution {
    let Some(found) = found.map(str::trim).filter(|f| !f.is_empty()) else {
        return Resolution::Missing;
    };
    let path = Path::new(found);
    let in_bin = |dir: &Path| path.parent() == Some(dir);
    let canonical = |p: &Path| tako_core::platform::path::canonicalize(p).ok();
    let same_dir = in_bin(bin)
        || match (path.parent().and_then(canonical), canonical(bin)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        };
    if path.is_absolute() && same_dir {
        Resolution::Fake
    } else {
        Resolution::Other(found.to_string())
    }
}

/// `bin` を先頭に置いた PATH（元の並びは保つ。元から入っていれば重ねない）
pub(crate) fn prepend_path(bin: &Path, old: Option<&OsStr>) -> Option<OsString> {
    let rest = old
        .map(|o| std::env::split_paths(o).collect::<Vec<_>>())
        .unwrap_or_default();
    std::env::join_paths(
        std::iter::once(bin.to_path_buf()).chain(rest.into_iter().filter(|p| p.as_path() != bin)),
    )
    .ok()
}

/// 項目 109 が押す間だけ PATH の先頭に置く偽物の置き場（visual-test 専用）
#[cfg(feature = "visual-test")]
pub(crate) struct FakeAgents {
    pub(crate) bin: PathBuf,
    pub(crate) log: PathBuf,
    /// 確かめ用のペインが [`which_command`] で書く答え
    pub(crate) which: PathBuf,
    /// 確かめの答え（[`Self::set_which`]）
    resolved: Vec<(String, String)>,
    /// PATH を書き換える前の値（外側の `Some` = 書き換え中）
    saved_path: Option<Option<OsString>>,
}

#[cfg(feature = "visual-test")]
impl FakeAgents {
    /// 偽物を書いて空打ちする。**background で呼ぶ**（初回の exec の検査を UI スレッドで待たない）
    pub(crate) fn write(root: &Path) -> std::io::Result<Self> {
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin)?;
        let log = root.join("calls.log");
        std::fs::write(&log, "")?;
        for agent in agent_names() {
            let path = bin.join(agent);
            let text = stub_text(agent, &log);
            // 書けないときの write_script は panic する（テストの組み立ての誤り）。節の失敗として返す
            std::panic::catch_unwind(|| tako_core::fake_exec::write_script(&path, &text)).map_err(
                |_| std::io::Error::other(format!("偽の {agent} を書けない: {}", path.display())),
            )?;
        }
        Ok(Self {
            bin,
            log,
            which: root.join("which.txt"),
            resolved: Vec::new(),
            saved_path: None,
        })
    }

    /// GUI の PATH の先頭へ置く（以後に立つペインが継ぐ）。2 回目は何もしない
    // tako:env-write-ok テストからは呼ばない（visual-test の GUI 本体が、立つペインに継がせるため自分の PATH を書き換える）
    pub(crate) fn activate(&mut self) {
        if self.saved_path.is_some() {
            return;
        }
        let before = std::env::var_os("PATH");
        if let Some(path) = prepend_path(&self.bin, before.as_deref()) {
            std::env::set_var("PATH", path);
            self.saved_path = Some(before);
        }
    }

    /// PATH を置く前の値へ戻す
    // tako:env-write-ok テストからは呼ばない（visual-test の GUI 本体が、押し終えた後に自分の PATH を戻す）
    pub(crate) fn restore(&mut self) {
        match self.saved_path.take() {
            Some(Some(path)) => std::env::set_var("PATH", path),
            Some(None) => std::env::remove_var("PATH"),
            None => {}
        }
    }

    /// 確かめ用のペインの答え（[`which_command`] の出力）を覚える
    pub(crate) fn set_which(&mut self, text: &str) {
        self.resolved = parse_which(text);
    }

    /// その名前がペインのシェルで何に当たったか
    pub(crate) fn resolved(&self, agent: &str) -> Resolution {
        let found = self
            .resolved
            .iter()
            .find(|(name, _)| name == agent)
            .map(|(_, found)| found.as_str());
        resolution(found, &self.bin)
    }

    /// 押してよい = ペインのシェルがその名前を偽物に引く
    pub(crate) fn resolves_to_fake(&self, agent: &str) -> bool {
        self.resolved(agent) == Resolution::Fake
    }

    /// これまでの呼び出し
    pub(crate) fn calls(&self) -> Vec<Call> {
        parse_calls(&std::fs::read_to_string(&self.log).unwrap_or_default())
    }

    /// その名前の偽物が端末から（= ペインのシェルの起動コマンドとして）起きたか
    pub(crate) fn started_in_pane(&self, agent: &str) -> bool {
        self.calls().iter().any(|c| c.agent == agent && c.tty)
    }

    /// 押し終えた: PATH を戻し、呼び出しを読んでから置き場を消す（失敗の出口では `remove_on_fail` が消す）
    pub(crate) fn finish(mut self) -> Vec<Call> {
        self.restore();
        let calls = self.calls();
        if let Some(root) = self.bin.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
        calls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 偽物の本文は呼ばれたことを記録し端末なら目印を出して待つ() {
        // 記録の置き場はシェルへ引用して渡す（空白を含む一時 dir でも 1 語）
        let log = Path::new("/tmp/tako st2726-agents-1/calls.log");
        let text = stub_text("codex", log);
        assert!(
            text.starts_with("#!/bin/sh\n"),
            "shebang が無いと exec できない: {text}"
        );
        assert!(text.contains(">> '/tmp/tako st2726-agents-1/calls.log'\n"));
        assert!(text.contains("printf '%s\\t%s\\t%s\\n' codex \"$tty\" \"${1-}\""));
        assert!(text.contains("echo 'codex 0.0.0 (tako fake #2726)'"));
        assert!(text.contains(&format!("echo '{MARK} codex'")));
        // 端末でない呼び出しは待たずに終わる・端末なら入力を読み捨てて待つ
        assert!(text.contains("[ \"$tty\" = tty ] || exit 0"));
        assert!(text.trim_end().ends_with("exec cat > /dev/null"));
        // fake_exec が空打ちの行を差し込める形（shebang の 1 行目）
        assert!(tako_core::fake_exec::script_text(&text, None).is_some());
    }

    #[test]
    fn 偽物は3系統すべてに置く() {
        assert_eq!(agent_names(), vec!["claude", "codex", "agy"]);
    }

    #[test]
    fn 呼び出しの記録を読む() {
        let calls =
            parse_calls("codex\ttty\t-c\nclaude\tpipe\t--version\n壊れた行\nagy\t??\tx\n\ttty\t\n");
        assert_eq!(
            calls,
            vec![
                Call {
                    agent: "codex".into(),
                    tty: true,
                    first_arg: "-c".into()
                },
                Call {
                    agent: "claude".into(),
                    tty: false,
                    first_arg: "--version".into()
                },
            ]
        );
        assert!(parse_calls("").is_empty());
    }

    #[test]
    fn 確かめの1行は名前ごとに答えを書き終えてから置き場へ移す() {
        let line = which_command(&["claude", "codex"], Path::new("/tmp/x/which.txt"));
        assert_eq!(
            line,
            "for a in claude codex; do printf '%s=%s\\n' \"$a\" \"$(command -v \"$a\")\"; done \
             > /tmp/x/which.tmp && command mv -f /tmp/x/which.tmp /tmp/x/which.txt"
        );
        // 空白を含む置き場は引用して 1 語で渡す
        assert!(
            which_command(&["agy"], Path::new("/tmp/a b/which.txt")).ends_with(
                "> '/tmp/a b/which.tmp' && command mv -f '/tmp/a b/which.tmp' '/tmp/a b/which.txt'"
            )
        );
        assert_eq!(
            parse_which("claude=/a/bin/claude\ncodex=\nagy=alias agy='x'\n=空\n"),
            vec![
                ("claude".to_string(), "/a/bin/claude".to_string()),
                ("codex".to_string(), String::new()),
                ("agy".to_string(), "alias agy='x'".to_string()),
            ]
        );
    }

    #[test]
    fn 偽物の置き場の実行ファイルだけを偽物と読む() {
        // 置き場はその OS の絶対パスで組む（`/tmp/…` は Windows では絶対パスにならない）
        let dir = tako_core::test_residue::ScratchDir::new("st2726-fake-bin");
        let bin = dir.path().join("bin");
        let fake = bin.join("codex").display().to_string();
        assert_eq!(resolution(Some(&fake), &bin), Resolution::Fake);
        // 本物の CLI（置き場の外）・alias・相対の答え・見つからないは押さない
        let elsewhere = dir.path().join("real").join("codex").display().to_string();
        for other in [
            elsewhere.as_str(),
            "/home/testuser/.local/bin/codex",
            "alias codex='codex --x'",
            "bin/codex",
        ] {
            assert_eq!(
                resolution(Some(other), &bin),
                Resolution::Other(other.to_string()),
                "{other}"
            );
        }
        assert_eq!(resolution(Some("  "), &bin), Resolution::Missing);
        assert_eq!(resolution(None, &bin), Resolution::Missing);
    }

    #[test]
    fn 置き場の比べはsymlinkを解いた形でも見る() {
        let dir = tako_core::test_residue::ScratchDir::new("st2726-resolution");
        let real = dir.path().join("real-bin");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("link-bin");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let found = real.join("agy").display().to_string();
            assert_eq!(resolution(Some(&found), &link), Resolution::Fake);
        }
        let _ = link;
    }

    #[test]
    fn pathの先頭へ置き元の並びは保つ() {
        // 区切り（unix は `:`・Windows は `;`）は OS のものを使う
        let path_of = |dirs: &[&str]| std::env::join_paths(dirs).unwrap();
        let bin = Path::new("/tmp/fake/bin");
        let joined = prepend_path(bin, Some(&path_of(&["/usr/bin", "/tmp/fake/bin", "/bin"])));
        assert_eq!(
            joined.unwrap(),
            path_of(&["/tmp/fake/bin", "/usr/bin", "/bin"])
        );
        assert_eq!(
            prepend_path(bin, None).unwrap(),
            OsString::from("/tmp/fake/bin")
        );
    }

    /// 番犬: 検証の中でエージェントの CLI を起こす入口（#2726）
    mod watchdog {
        /// 解消エージェントの 3 択（dispatch・ハンドラの直呼び・3 択を開いて押す）
        const MENU_ENTRIES: &[&str] = &[
            "GitResolveAgent {",
            "git_do_resolve_agent(",
            "git_agent_menu_open = true",
        ];
        /// ペインへ打ち込む呼び出し（引数にエージェントの名前があれば入口）
        const TYPERS: &[&str] = &["type_text(", "pty_line(", "type_until_focused_text("];
        /// スターターの起動導線（ペインのシェルへ `tako master` / `solo` / `setup` を積む。積んだ行が
        /// 走ると、インストール済みの tako がエージェントの CLI を起こす）
        const STARTERS: &[&str] = &["starter_action(", "starter_action_with_profile("];
        /// スターターのうち起動を積む操作（`UseTerminal` はターミナル表示へ戻すだけ）
        const STARTER_LAUNCHES: &[&str] = &[
            "StarterAction::Master",
            "StarterAction::Solo",
            "StarterAction::Setup",
        ];
        /// 呼び出しの後ろで引数を探す行数（rustfmt が引数を行へ割った形）
        const TYPER_ARGS: usize = 4;
        /// 偽物の確かめ（`FakeAgents::resolves_to_fake` の呼び出し）
        const FAKE_GUARD: &str = ".resolves_to_fake(";
        const GUARD_ABOVE: usize = 12;
        /// 実 claude の e2e の明示の opt-in（`claude_e2e_enabled(` / `claude_e2e` / `TAKO_VISUAL_CLAUDE`）
        const OPT_IN: &[&str] = &["claude_e2e", "TAKO_VISUAL_CLAUDE"];
        const OPT_IN_ABOVE: usize = 30;
        /// 積んだ起動の取り下げ（#2446）
        const WITHDRAW: &str = "withdraw_launch(";
        const WITHDRAW_BELOW: usize = 40;
        const ALLOW_MARKER: &str = "tako:launch-ok";
        const ALLOW_ABOVE: usize = 6;

        fn code(line: &str) -> bool {
            !line.trim_start().starts_with("//")
        }

        fn names_agent(line: &str) -> bool {
            super::agent_names().iter().any(|name| {
                line.contains(&format!("\"{name}\"")) || line.contains(&format!("\"{name} "))
            })
        }

        /// 呼び出し（`calls` のどれか）の引数に `hit` が当たるか。引数は呼び出しの行から、文が閉じる
        /// （`;`）か次の呼び出しが始まるまでの行で探す
        fn args_hit(
            lines: &[(usize, &str)],
            i: usize,
            calls: &[&str],
            hit: impl Fn(&str) -> bool,
        ) -> bool {
            let args = &lines[i..(i + TYPER_ARGS).min(lines.len())];
            for (k, (_, l)) in args.iter().enumerate() {
                if k > 0 && calls.iter().any(|p| l.contains(p)) {
                    break;
                }
                if code(l) && hit(l) {
                    return true;
                }
                if l.trim_end().ends_with(';') {
                    break;
                }
            }
            false
        }

        /// 入口の種類（入口でなければ `None`）
        fn entry_kind(lines: &[(usize, &str)], i: usize) -> Option<&'static str> {
            let line = lines[i].1;
            if !code(line) {
                return None;
            }
            if MENU_ENTRIES.iter().any(|p| line.contains(p)) {
                return Some("解消エージェントの 3 択");
            }
            if STARTERS.iter().any(|p| line.contains(p)) {
                let launches = |l: &str| STARTER_LAUNCHES.iter().any(|a| l.contains(a));
                return args_hit(lines, i, STARTERS, launches).then_some("スターターの起動");
            }
            if TYPERS.iter().any(|p| line.contains(p)) {
                return args_hit(lines, i, TYPERS, names_agent)
                    .then_some("エージェントの名前の打ち込み");
            }
            None
        }

        fn any_in(lines: &[(usize, &str)], tokens: &[&str]) -> bool {
            lines
                .iter()
                .any(|(_, l)| code(l) && tokens.iter().any(|t| l.contains(t)))
        }

        /// 守りの無い入口（`file:line 種類: 行`）
        fn findings(file: &str, lines: &[(usize, &str)]) -> Vec<String> {
            let mut hits = Vec::new();
            for i in 0..lines.len() {
                let Some(kind) = entry_kind(lines, i) else {
                    continue;
                };
                let above = |n: usize| &lines[i.saturating_sub(n)..=i];
                let allowed = above(ALLOW_ABOVE)
                    .iter()
                    .any(|(_, l)| l.contains(ALLOW_MARKER));
                let faked = any_in(above(GUARD_ABOVE), &[FAKE_GUARD]);
                // 3 択は opt-in でも通さない（偽物で確かめる形がある）
                let opted_in =
                    kind != "解消エージェントの 3 択" && any_in(above(OPT_IN_ABOVE), OPT_IN);
                let withdrawn = any_in(
                    &lines[i..(i + WITHDRAW_BELOW).min(lines.len())],
                    &[WITHDRAW],
                );
                if !(allowed || faked || opted_in || withdrawn) {
                    let (number, line) = lines[i];
                    hits.push(format!("{file}:{number} {kind}: {}", line.trim()));
                }
            }
            hits
        }

        /// `self_test/*.rs` の製品側（最初の `#[cfg(test)]` より前）の行
        fn product_lines(src: &str) -> Vec<(usize, &str)> {
            src.lines()
                .enumerate()
                .take_while(|(_, l)| l.trim() != "#[cfg(test)]")
                .map(|(i, l)| (i + 1, l))
                .collect()
        }

        /// 走査する範囲（main.rs のセルフテストのモジュール + self_test/*.rs）の守りの無い入口
        fn scan() -> (Vec<String>, usize) {
            let main = include_str!("../main.rs");
            let region = crate::selftest_launch_withdraw_watchdog::self_test_region(main);
            assert!(
                region.len() > 10_000,
                "main.rs のセルフテストのモジュールが見つからない = この番犬は何も見ていない"
            );
            let mut seen = region.len();
            let mut hits = findings("src/main.rs", &region);
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/self_test");
            let mut files: Vec<_> = std::fs::read_dir(&dir)
                .expect("src/self_test を読めない")
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "rs"))
                .collect();
            files.sort();
            assert!(
                files.len() > 10,
                "src/self_test の .rs が見つからない: {files:?}"
            );
            for path in files {
                let src = std::fs::read_to_string(&path).unwrap_or_default();
                let lines = product_lines(&src);
                seen += lines.len();
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                hits.extend(findings(&format!("src/self_test/{name}"), &lines));
            }
            (hits, seen)
        }

        #[test]
        fn 検証の中でエージェントのcliを起こす入口は偽物か明示のopt_inを通る() {
            let (hits, seen) = scan();
            assert!(seen > 10_000, "走査した行が少なすぎる: {seen}");
            assert!(
                hits.is_empty(),
                "セルフテスト / visual-test の中で、本物のエージェントの CLI（claude / codex / agy）を起こしうる \
                 入口に守りが無い（#2726）:\n{}\n\
                 立ったペインのシェルは起動コマンドの素の名前を PATH から引くので、本物が起きる（未ログインの \
                 codex はログインの画面を利用者の本物の画面のブラウザへ出した）。押す・打つ前に、偽物の置き場 \
                 （`fake_agents_2726::FakeAgents`）を PATH の先頭へ置いて `.resolves_to_fake(名前)` で確かめるか、\
                 実 claude の e2e の明示の opt-in（`claude_e2e_enabled(…)` / `TAKO_VISUAL_CLAUDE`）の内側に置くか、\
                 積んだ起動を `withdraw_launch` で取り下げること。走らせる理由がある入口だけ、入口の行か直前 6 行に \
                 `// tako:launch-ok <理由>` を書く",
                hits.join("\n")
            );
        }

        /// 走査の範囲に今ある入口を数える（入口を見失ったら番犬は何も見ていない）
        #[test]
        fn 番犬は今ある入口を見ている() {
            let main = include_str!("../main.rs");
            let region = crate::selftest_launch_withdraw_watchdog::self_test_region(main);
            let kinds: Vec<_> = (0..region.len())
                .filter_map(|i| entry_kind(&region, i))
                .collect();
            let count = |kind: &str| kinds.iter().filter(|k| **k == kind).count();
            let menu = count("解消エージェントの 3 択");
            let starters = count("スターターの起動");
            let typed = count("エージェントの名前の打ち込み");
            assert!(menu >= 1, "項目 109 の 3 択の入口が見つからない: {kinds:?}");
            // 項目 93(d1)・97(d) の setup・97(e)・97c の solo・739 の ▾
            assert!(
                starters >= 5,
                "スターターの起動の入口が見つからない: {kinds:?}"
            );
            // 実 claude の e2e の打ち込み（`TAKO_VISUAL_CLAUDE` / 項目 45c / 95c）
            assert!(
                typed >= 3,
                "エージェントの名前の打ち込みが見つからない: {kinds:?}"
            );
        }

        /// 検出力: 修正前の項目 109 の形と、opt-in の無い打ち込みを名指しし、守りのある形は通す
        #[test]
        fn 番犬は守りの無い入口を名指しする() {
            let lines = |src: &'static str| -> Vec<(usize, &'static str)> {
                src.lines().enumerate().map(|(i, l)| (i + 1, l)).collect()
            };
            // 修正前の項目 109（main.rs の 3 択のループの頭）
            let old109 = "for agent in [\"claude\", \"codex\", \"agy\"] {\n\
                          \x20   let _ = window496.update(cx, |app, _, cx| {\n\
                          \x20       app.git_feedback = None;\n\
                          \x20       app.git_agent_menu_open = true;\n\
                          \x20       cx.notify();\n\
                          \x20   });\n\
                          \x20   click_at(any496, cx, choice.center());\n\
                          }\n";
            assert_eq!(
                findings("x.rs", &lines(old109)),
                vec!["x.rs:4 解消エージェントの 3 択: app.git_agent_menu_open = true;".to_string()]
            );
            // 偽物の確かめの後なら通す。確かめがコメントの中だけなら通さない
            let guarded = "for agent in agents {\n\
                           \x20   if !fakes.resolves_to_fake(agent) {\n\
                           \x20       continue;\n\
                           \x20   }\n\
                           \x20   app.git_agent_menu_open = true;\n\
                           }\n";
            assert!(findings("x.rs", &lines(guarded)).is_empty());
            let commented =
                "// if !fakes.resolves_to_fake(agent) {\napp.git_agent_menu_open = true;\n";
            assert_eq!(findings("x.rs", &lines(commented)).len(), 1);
            // dispatch の形・ハンドラの直呼び
            assert_eq!(
                findings(
                    "x.rs",
                    &lines("Request::GitResolveAgent {\n    pane: None,\n}\n")
                )
                .len(),
                1
            );
            assert_eq!(
                findings("x.rs", &lines("app.git_do_resolve_agent(\"codex\", cx);\n")).len(),
                1
            );
            // 打ち込み: opt-in の外は名指し・内は通す・引数が次の行でも見る
            assert_eq!(
                findings("x.rs", &lines("type_text(any, cx, \"codex\", true);\n")),
                vec![
                    "x.rs:1 エージェントの名前の打ち込み: type_text(any, cx, \"codex\", true);"
                        .to_string()
                ]
            );
            assert_eq!(
                findings(
                    "x.rs",
                    &lines("session.write(pty_line(\"claude --resume\"));\n")
                )
                .len(),
                1
            );
            assert_eq!(
                findings(
                    "x.rs",
                    &lines("type_text(\n    any,\n    cx,\n    \"agy\",\n);\n")
                )
                .len(),
                1
            );
            assert!(findings(
                "x.rs",
                &lines("if claude_e2e_enabled(\"28\") {\n    type_text(any, cx, \"claude\", true);\n}\n")
            )
            .is_empty());
            // エージェントの名前でない打ち込み（`claude-code` の語・別の単語）は入口にしない
            assert!(findings(
                "x.rs",
                &lines("type_text(any, cx, \"echo claude\", true);\n")
            )
            .is_empty());
            // 取り下げ・明示の逃げ道
            assert!(findings(
                "x.rs",
                &lines("app.git_do_resolve_agent(\"codex\", cx);\nwithdraw_launch(app, p);\n")
            )
            .is_empty());
            assert!(findings(
                "x.rs",
                &lines("// tako:launch-ok 偽の CLI だけの PATH で起こす\ntype_text(any, cx, \"codex\", true);\n")
            )
            .is_empty());
            // コメントの中の入口は見ない
            assert!(findings("x.rs", &lines("// app.git_agent_menu_open = true;\n")).is_empty());

            // スターターの起動: 修正前の項目 739（選んだ行が走るのを画面で待つ）と 97(e) を名指しする
            let old739 = "let selected = window\n\
                          \x20   .update(cx, |app, _, cx| {\n\
                          \x20       app.starter_action_with_profile(\n\
                          \x20           starter_pane,\n\
                          \x20           StarterAction::Master,\n\
                          \x20           Some(probe),\n\
                          \x20           cx,\n\
                          \x20       )\n\
                          \x20   })\n\
                          \x20   .unwrap_or(false);\n";
            assert_eq!(
                findings("x.rs", &lines(old739)),
                vec!["x.rs:3 スターターの起動: app.starter_action_with_profile(".to_string()]
            );
            assert_eq!(
                findings(
                    "x.rs",
                    &lines(
                        "let done = app.starter_action(master_pane, StarterAction::Master, cx);\n"
                    )
                )
                .len(),
                1
            );
            assert_eq!(
                findings(
                    "x.rs",
                    &lines("let done = app.starter_action(fresh, StarterAction::Setup, cx);\n")
                )
                .len(),
                1
            );
            // 積んだ直後に取り下げれば通す。ターミナル表示へ戻すだけの操作は入口にしない
            assert!(findings(
                "x.rs",
                &lines(
                    "let done = app.starter_action(p, StarterAction::Solo, cx);\n\
                     let queued = withdraw_launch(app, p);\n"
                )
            )
            .is_empty());
            assert!(findings(
                "x.rs",
                &lines("app.starter_action(base, StarterAction::UseTerminal, cx)\n")
            )
            .is_empty());
            // 実 claude の e2e の opt-in の内側は通す
            assert!(findings(
                "x.rs",
                &lines(
                    "if claude_e2e_enabled(\"720\") {\n    app.starter_action(p, StarterAction::Solo, cx)\n}\n"
                )
            )
            .is_empty());
        }
    }
}
