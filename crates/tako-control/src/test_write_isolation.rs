//! 番犬: **テストプロセスは本番相当の置き場へ 1 バイトも書かない**（Issue #944 / #1030）
//!
//! `TAKO_DATA_DIR` を渡し忘れた `cargo test --workspace` が、ユーザーの
//! `<data_dir>/perf.log` / `persist.log` / `sessions.yaml` / `shell-integration/` と、
//! さらに data dir の**外**（`~/.claude.json` / `~/.claude/backups/` /
//! `~/.codex/config.toml` / `~/.gemini/…/settings.json` / `~/.cache/powershell/`）まで
//! 書き換えていた。診断ログには偽の「メインスレッド専有」が 643 行溜まり、
//! `~/.claude.json` にはテスト用 cwd の事前信頼が数千件積もっていた。
//!
//! 塞ぎ方は 2 系統ある（どちらも「書く側」ではなく**置き場を決める側**を倒す）:
//!
//! - data dir: [`tako_core::paths::data_dir`] がテストプロセスを実行時に見分けて
//!   隔離先を返す（`cfg(test)` はクレートを跨がないので**実行時判定**が要る）
//! - 外部エージェントの設定: `orchestrator::agent_config_home()` を `cfg(test)` で倒す
//!
//! この番犬は**子プロセスを空の `HOME` で起こして**その結果を見る。
//! 「隔離されているはず」をフラグで確かめるのではなく、
//! **本番相当の場所にファイルが 1 つも出来ないこと**を実測で押さえる。
//!
//! 親（[`tests::テストプロセスは本番相当の置き場へ何も書かない`]）が
//! 子（[`tests::子プロセス_本番相当の書き込みを一通り行う`]）を
//! `--exact` で名指しして起こす。子は目印の環境変数が無ければ即座に返るので、
//! 通常の `cargo test` では 1 度も書き込みを試みない。

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use std::path::{Path, PathBuf};

    /// 子プロセスを起こすときの目印（これが無ければ子の本体は走らない）
    const CHILD_ENV: &str = "TAKO_944_WRITE_PROBE";

    /// 子のテスト名（`--exact` で名指しするので、改名したらここも直す）
    const CHILD_TEST: &str =
        "test_write_isolation::tests::子プロセス_本番相当の書き込みを一通り行う";

    /// 子が「エージェント CLI を何個解決できたか」を親へ渡す目印（#1261）。
    /// **パスは載せない**（public リポなので実ホームパスを出さない。#927）
    const RESOLVED_MARKER: &str = "TAKO_1261_RESOLVED=";

    /// 子に**実シェルを起こさせる**ときの目印（#1253）
    const SHELL_PROBE_ENV: &str = "TAKO_1253_SHELL_PROBE";

    /// シェル probe が打つコマンドの目印
    const SHELL_PROBE_MARKER: &str = "TAKO_1253_HISTORY_MARKER";

    /// 空の `HOME`（と必要なら `TAKO_DATA_DIR`）で子テストを 1 回走らせる。
    /// 戻り値は `(子の終了状態が成功か, 子の標準出力, HOME 配下に出来たファイル)`
    fn run_child(
        fake_home: &Path,
        data_dir: Option<&Path>,
        legacy: bool,
    ) -> (bool, String, Vec<String>) {
        run_child_with(fake_home, data_dir, legacy, &[])
    }

    /// [`run_child`] に**追加の環境変数**を渡す版（#1253 のシェル probe / A/B 用）
    fn run_child_with(
        fake_home: &Path,
        data_dir: Option<&Path>,
        legacy: bool,
        extra: &[(&str, &str)],
    ) -> (bool, String, Vec<String>) {
        let exe = std::env::current_exe().expect("テストバイナリのパス");
        let mut cmd = std::process::Command::new(&exe);
        // `--nocapture`: 子が自分の観測（[`RESOLVED_MARKER`]）を親へ渡す唯一の口。
        // libtest は既定で子テストの標準出力を飲み込むので、これが無いと親は
        // 「子が CLI を解決できたのか」を知れない
        cmd.args(["--exact", CHILD_TEST, "--test-threads=1", "--nocapture"])
            .env(CHILD_ENV, "1")
            // 空の HOME。unix / Windows の両方の解決元を差し替える
            .env("HOME", fake_home)
            .env("USERPROFILE", fake_home)
            .env("XDG_DATA_HOME", fake_home.join("xdg-data"))
            .env("APPDATA", fake_home.join("AppData/Roaming"))
            .env("LOCALAPPDATA", fake_home.join("AppData/Local"))
            // 逃げ道を全部塞ぐ: これらが立っていると「隔離できている」ように見えてしまう
            .env_remove("TAKO_ISOLATED")
            .env_remove("TAKO_ORCHESTRATOR_DIR")
            .env_remove("TAKO_PERF_LOG")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME");
        match data_dir {
            Some(dir) => cmd.env("TAKO_DATA_DIR", dir),
            None => cmd.env_remove("TAKO_DATA_DIR"),
        };
        if legacy {
            cmd.env("TAKO_944_LEGACY", "1");
        } else {
            cmd.env_remove("TAKO_944_LEGACY");
        }
        for (key, val) in extra {
            cmd.env(key, val);
        }
        let out = cmd.output().expect("子テストプロセスを起こせる");
        let mut stdout = String::from_utf8_lossy(&out.stdout).to_string();
        stdout.push_str(&String::from_utf8_lossy(&out.stderr));
        (out.status.success(), stdout, files_under(fake_home))
    }

    /// 使い捨ての空ディレクトリ（テスト間で衝突しない名前）
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tako-944-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
        dir
    }

    /// 「本番相当の場所」= 空の `HOME` の下に出来たファイル全部。
    /// 何が出来たかを人が読める形で返す（0 件なら空 Vec）
    fn files_under(root: &Path) -> Vec<String> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && !path.is_symlink() {
                    walk(&path, root, out);
                } else {
                    out.push(
                        path.strip_prefix(root)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }

    /// 本番なら `HOME` 配下へ書きに行く経路を一通り叩く（子プロセス側の本体）。
    ///
    /// 目印の環境変数が無いときは**何もしない**。親から
    /// `--exact <CHILD_TEST>` で起こされたときだけ書き込みを試みる
    #[test]
    fn 子プロセス_本番相当の書き込みを一通り行う() {
        if std::env::var_os(CHILD_ENV).is_none() {
            return;
        }

        // 1. 診断ログ（<data_dir>/persist.log / perf.log）
        crate::diag::persist_log("#944 番犬: 診断ログの書き先を確かめる");
        crate::diag::perf_log("#944 番犬: perf ログの書き先を確かめる");

        // 2. シェル統合のスクリプト書き出し（<data_dir>/shell-integration/**）。
        //    tako-core 側の書き込みで、`cfg(test)` はここへ届かない
        //    = 実行時判定でしか塞げない経路。`install()` ではなく `env()` を叩くのは、
        //    実際のテストがそちらを通るから（`install()` は Windows で
        //    `$PROFILE` = ユーザーの PowerShell プロファイルまで書きに行く別経路）
        let _ = tako_core::shell_integration::env();

        // 3. セッションカタログ（<data_dir>/sessions.yaml と .lock / .bak.N）
        let _ = crate::sessions::record_spawn(crate::sessions::PendingSpawn {
            pane: Some(944),
            kind: "worker".into(),
            project: Some("tako-944-probe".into()),
            cwd: Some("/tmp/tako-944-probe".into()),
            prompt_head: Some("probe".into()),
            agent: Some("claude".into()),
            ..Default::default()
        });

        // 4. タスクチェックポイント（<data_dir>/task_checkpoints.yaml と .lock）
        let _ = crate::task_checkpoints::suspend_by_pane(944, "#944 番犬");

        // 5. 外部エージェントの事前信頼（~/.claude.json・~/.codex/config.toml・
        //    ~/.gemini/antigravity-cli/settings.json）
        let _ = crate::claude_tui::ensure_trusted("/tmp/tako-944-probe");
        for agent in [
            crate::orchestrator::WorkerAgent::Codex,
            crate::orchestrator::WorkerAgent::Agy,
        ] {
            let _ = crate::orchestrator::agent::ensure_trusted(agent, "/tmp/tako-944-probe");
        }

        // 6. worker の状態問い合わせ（本番は `claude agents --json` を起こし、
        //    claude 本体が ~/.claude.json を書き戻して ~/.claude/backups/ を積む）
        let _ = crate::agents::list_agents();

        // 7. エージェント CLI への問い合わせ（#1261）。本番なら**実 CLI が起動**し、
        //    tako が 1 バイトも書かなくても CLI 自身が自分のホームを作る（実測:
        //    agy = `~/.gemini` 32 ファイル / codex = `~/.codex/tmp/arg0/…/.lock` /
        //    claude = `~/.claude.json` と `~/.claude/backups/`）
        let states = crate::setup_bootstrap::status_all();
        // 解決できた数 = `is_authenticated_for` が実 CLI を起こしうる系統の数。
        // 親の PATH で数えると食い違う（`exe::find` は unix ではログインシェル経由なので、
        // **子の空 HOME では rc が無く `~/.local/bin` が PATH に入らない**）
        let resolved = states
            .iter()
            .filter(|(_, state)| state.as_ref().is_ok_and(|s| s.binary.is_some()))
            .count();
        println!("{RESOLVED_MARKER}{resolved}");
        let _ = crate::stale_binary::check_stale(&crate::stale_binary::PaneClaudeInfo {
            spawned_binary: PathBuf::from("/fake/old/claude-cli-2.1.218/claude"),
            pid: None,
            dismissed: false,
        });
        // モデル一覧は `agent_cli::locate` が cfg(test) で存在しないパスへ倒すので
        // 現状ここから実 CLI へは届かないが、その stub が外れたときに素通りしないよう通す
        let _ = crate::agent_models::catalog_all();

        // 8. ペインのシェル（#1253）。**名指しされたときだけ**動かす:
        //    実シェルを起こすので、0 ファイルを見る他の親テストの前提を壊さない
        #[cfg(unix)]
        if std::env::var_os(SHELL_PROBE_ENV).is_some() {
            shell_probe();
        }
    }

    /// 既定シェルを PTY で起こして 1 行打つ（#1253 の子 probe 本体）。
    ///
    /// 本番ならここで**対話ログインシェル**がユーザーの `~/.zsh_history` へ積む。
    /// 待ちは固定時間ではなく**画面の状態**で切る（`conventions.md` の待ち条件）
    #[cfg(unix)]
    fn shell_probe() {
        use tako_core::terminal::{SpawnOptions, TerminalSession};

        let Ok((session, _rx)) = TerminalSession::spawn(80, 24, SpawnOptions::default()) else {
            eprintln!("skip: 既定シェルの PTY を張れない");
            return;
        };
        let poll = std::time::Duration::from_millis(50);
        // プロンプトが出てから打つ（出る前に打つと行が食われる）
        let ready = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while session.visible_lines().iter().all(|l| l.trim().is_empty())
            && std::time::Instant::now() < ready
        {
            std::thread::sleep(poll);
        }
        session.write(format!("echo {SHELL_PROBE_MARKER}\r").into_bytes());
        // 打った行のエコーと出力の 2 本が見えたら、履歴も書かれている
        // （`INC_APPEND_HISTORY` はコマンド確定時に追記する）
        let done = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < done {
            let hits = session
                .visible_lines()
                .iter()
                .filter(|l| l.contains(SHELL_PROBE_MARKER))
                .count();
            if hits >= 2 {
                return;
            }
            std::thread::sleep(poll);
        }
        eprintln!("warn: シェル probe のマーカーが画面に出そろわなかった");
    }

    #[test]
    fn テストプロセスは本番相当の置き場へ何も書かない() {
        let fake_home = scratch("fakehome");
        let (ok, stdout, created) = run_child(&fake_home, None, false);
        let _ = std::fs::remove_dir_all(&fake_home);

        assert!(ok, "子テストが失敗した\n{stdout}");
        assert!(
            stdout.contains("1 passed"),
            "子テストが実行されていない（名前を変えたら CHILD_TEST も直す）\n{stdout}"
        );
        assert!(
            created.is_empty(),
            "テストプロセスが本番相当の HOME へ書いた（#944）: {created:#?}"
        );
    }

    /// A/B: 隔離を切る（`TAKO_944_LEGACY=1`）と**同じ子が本番相当の場所へ書く**。
    ///
    /// 上の番犬が「たまたま何も書かない子」を見ているだけではないこと
    /// = 検査に検出力があることを、実測で固定する
    #[test]
    fn legacyへ倒すと本番相当の置き場へ書いてしまう() {
        let fake_home = scratch("legacy");
        let (ok, stdout, created) = run_child(&fake_home, None, true);
        let _ = std::fs::remove_dir_all(&fake_home);

        assert!(ok, "子テストが失敗した\n{stdout}");
        let has = |needle: &str| created.iter().any(|f| f.contains(needle));
        // data dir 側（tako-core の実行時判定を切ったぶん）
        assert!(
            has("persist.log"),
            "旧挙動で persist.log が出ない: {created:#?}"
        );
        assert!(
            has("shell-integration/"),
            "旧挙動で shell-integration が出ない: {created:#?}"
        );
        // data dir の外（tako-control の cfg(test) 隔離を切ったぶん）
        assert!(
            has(".claude"),
            "旧挙動で claude の設定が出ない: {created:#?}"
        );
        assert!(
            has(".codex/config.toml"),
            "旧挙動で codex の設定が出ない: {created:#?}"
        );
        assert!(has(".gemini/"), "旧挙動で agy の設定が出ない: {created:#?}");
    }

    /// #1030: **claude の設定エントリ数がテスト実行で変わらない**。
    ///
    /// 「何も作られない」（上の番犬）とは別に、**既にある設定ファイルを書き換えない**ことを見る。
    /// 実害はこちらで、ユーザーの生きた `~/.claude.json` に一時ディレクトリの事前信頼が
    /// 数千件積もっていた（実測: `~/.claude.json` 2,573 件中 2,216 件がテスト由来）
    #[test]
    fn claudeの設定エントリはテスト実行で増えない() {
        let seed = r#"{"installMethod":"brew","projects":{"/work/real-project":{"hasTrustDialogAccepted":true}}}"#;

        let prepare = |tag: &str| -> PathBuf {
            let home = scratch(tag);
            std::fs::create_dir_all(home.join(".claude")).expect(".claude を作れる");
            for name in [".claude.json", ".claude/.claude.json"] {
                std::fs::write(home.join(name), seed).expect("種ファイルを書ける");
            }
            home
        };
        let projects = |path: &std::path::Path| -> Vec<String> {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("projects").and_then(|p| p.as_object()).cloned())
                .map(|m| {
                    let mut keys: Vec<String> = m.keys().cloned().collect();
                    keys.sort();
                    keys
                })
                .unwrap_or_default()
        };

        // 修正後: 2 ファイルとも 1 件のまま
        let home = prepare("cfgcount");
        let (ok, stdout, _) = run_child(&home, None, false);
        assert!(ok, "子テストが失敗した\n{stdout}");
        for name in [".claude.json", ".claude/.claude.json"] {
            assert_eq!(
                projects(&home.join(name)),
                vec!["/work/real-project".to_string()],
                "{name} のエントリが変わった（#1030）"
            );
        }
        let _ = std::fs::remove_dir_all(&home);

        // A/B: 隔離を切ると同じ子がエントリを足す
        let home = prepare("cfgcount-legacy");
        let (ok, stdout, _) = run_child(&home, None, true);
        assert!(ok, "子テストが失敗した\n{stdout}");
        let after = projects(&home.join(".claude/.claude.json"));
        assert!(
            after.len() > 1,
            "旧挙動でエントリが増えていない（検出力が無い）: {after:?}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    /// #1253: **検証プロセスはユーザーのシェル履歴へ書かない**。
    ///
    /// tako が開くペインのシェルは対話ログインシェルなので、テストやセルフテストが
    /// 打ち込んだコマンドがそのまま `~/.zsh_history` に積もっていた（実測: `cargo test
    /// --workspace` 1 回で 1 行）。`HISTFILE` を渡すだけでは足りない —— macOS の
    /// `/etc/zshrc` が `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history` を**無条件で代入**する
    /// ので、その形の rc を空の HOME へ置いて**同じ条件**を作ってから測る
    #[test]
    #[cfg(unix)]
    fn 検証プロセスはユーザーのシェル履歴へ書かない() {
        // `/etc/zshrc`（macOS）と同じ形。CI の機械に何が入っていても同じ条件になる
        let rc = "HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history\n\
                  HISTSIZE=2000\n\
                  SAVEHIST=1000\n\
                  setopt INC_APPEND_HISTORY\n";
        let prepare = |tag: &str| -> PathBuf {
            let home = scratch(tag);
            std::fs::write(home.join(".zshrc"), rc).expect("rc を置ける");
            std::fs::write(home.join(".zshenv"), "").expect("rc を置ける");
            home
        };
        let history_files = |created: &[String]| -> Vec<String> {
            created
                .iter()
                .filter(|f| f.contains("history"))
                .cloned()
                .collect()
        };

        let home = prepare("histfix");
        let (ok, stdout, created) = run_child_with(&home, None, false, &[(SHELL_PROBE_ENV, "1")]);
        let _ = std::fs::remove_dir_all(&home);
        assert!(ok, "子テストが失敗した\n{stdout}");
        assert!(
            history_files(&created).is_empty(),
            "検証プロセスがユーザーのシェル履歴へ書いた（#1253）: {:#?}",
            history_files(&created)
        );

        // A/B: 隔離を切る（`TAKO_1253_LEGACY=1`）と同じ子が履歴を残す。
        // 既定シェルが履歴を持たない機械では実証できないので、そのときは明示して落とさない
        let home = prepare("histlegacy");
        let (ok, stdout, created) = run_child_with(
            &home,
            None,
            false,
            &[(SHELL_PROBE_ENV, "1"), ("TAKO_1253_LEGACY", "1")],
        );
        let _ = std::fs::remove_dir_all(&home);
        assert!(ok, "子テストが失敗した\n{stdout}");
        if history_files(&created).is_empty() {
            eprintln!(
                "skip(A/B のみ): 既定シェル（{}）が履歴ファイルを持たないので旧挙動を実証できない",
                std::env::var("SHELL").unwrap_or_default()
            );
        }
    }

    /// #1261: **テストプロセスは実 claude / codex / agy を起こさない**。
    ///
    /// tako 側が 1 バイトも書かなくても、実 CLI は起動しただけで自分のホームを作る
    /// （実測: 空 HOME で `cargo test --workspace` → `~/.gemini` 32 ファイル /
    /// `~/.codex/tmp/arg0/…/.lock` / `~/.claude/backups/`）。上の「何も書かない」番犬でも
    /// 0 件で落ちるが、こちらは**CLI だけが作るファイル**を名指しで押さえるので、
    /// 落ちたときに「tako が書いた」のか「CLI を起こした」のかが分かる
    #[test]
    fn テストプロセスは実エージェントcliを起こさない() {
        // tako 自身の `ensure_trusted` が書く設定ファイル
        // （`.gemini/antigravity-cli/settings.json` / `.codex/config.toml` / `.claude.json`）
        // とは別物 —— これらは**CLI 本体しか作らない**
        let cli_made = |created: &[String]| -> Vec<String> {
            created
                .iter()
                .filter(|f| {
                    (f.starts_with(".gemini/") && !f.ends_with("antigravity-cli/settings.json"))
                        || f.starts_with(".codex/tmp/")
                        || f.starts_with(".claude/backups/")
                })
                .cloned()
                .collect()
        };

        let home = scratch("agentcli");
        let (ok, stdout, created) = run_child(&home, None, false);
        let _ = std::fs::remove_dir_all(&home);
        assert!(ok, "子テストが失敗した\n{stdout}");
        assert!(
            cli_made(&created).is_empty(),
            "テストプロセスが実エージェント CLI を起こした（#1261）: {:#?}",
            cli_made(&created)
        );

        // A/B: 隔離を切ると同じ子が実 CLI を起こし、CLI が自分のホームを作る。
        // CLI が入っていない機械（CI）では実証できないので、そのときは明示して落とさない。
        // 判断材料は**子自身の報告**（親の PATH で数えると、子の空 HOME では
        // ログインシェルが `~/.local/bin` を PATH へ入れないので食い違う）
        let home = scratch("agentcli-legacy");
        let (ok, stdout, created) = run_child(&home, None, true);
        let _ = std::fs::remove_dir_all(&home);
        assert!(ok, "子テストが失敗した\n{stdout}");
        // `--nocapture` の libtest は `test <名前> ... ` を**改行せずに**出すので、
        // 子の 1 行目はその後ろへ連結される。行頭ではなく**部分一致**で拾う
        let resolved: usize = stdout
            .split(RESOLVED_MARKER)
            .nth(1)
            .map(|rest| {
                rest.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
            })
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("子が解決数を報告していない\n{stdout}"));
        if resolved == 0 {
            eprintln!("skip(A/B のみ): 子から claude / codex / agy をどれも解決できない");
            return;
        }
        assert!(
            !cli_made(&created).is_empty(),
            "旧挙動でも実 CLI の痕跡が出ない（検出力が無い）。\
             子が解決できた系統: {resolved} / 出来たもの: {created:#?}"
        );
    }

    /// data dir の隔離が**明示の `TAKO_DATA_DIR` を上書きしない**こと（受け入れ条件のエッジ）。
    /// 子を `TAKO_DATA_DIR` つきで起こし、書き込みがそこへ入ることを実測する
    #[test]
    fn 明示のtako_data_dirはテストプロセスでも尊重される() {
        let base = scratch("explicit");
        let fake_home = base.join("home");
        let data_dir = base.join("data");
        std::fs::create_dir_all(&fake_home).expect("空の HOME を作れる");

        let (ok, stdout, in_home) = run_child(&fake_home, Some(&data_dir), false);
        let in_data = files_under(&data_dir);
        let _ = std::fs::remove_dir_all(&base);

        assert!(ok, "子テストが失敗した\n{stdout}");
        assert!(
            in_data.iter().any(|f| f == "persist.log"),
            "明示した TAKO_DATA_DIR へ書かれていない: {in_data:#?}"
        );
        assert!(
            in_home.is_empty(),
            "TAKO_DATA_DIR を渡しても HOME へ書いている: {in_home:#?}"
        );
    }
}
