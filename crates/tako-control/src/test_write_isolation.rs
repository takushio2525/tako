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
//!
//! **子を `deps/` の外へ置いても同じ**であることも見る（#1811）。テスト判定が
//! 「実行ファイルの置き場」だけだった頃は、計測のために `/tmp` へコピーした
//! テストバイナリが本番の `recent.json` を上書きし、`shell-integration/cli-dir` を空にした。
//! 親はこのテストバイナリを一時 dir へ置き直し（同じ名前 / 改名）、そこから子を起こす。

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
    #[cfg(unix)]
    const SHELL_PROBE_ENV: &str = "TAKO_1253_SHELL_PROBE";

    /// シェル probe が打つコマンドの出力の目印（打った行のエコーには現れない。[`shell_probe_line`]）
    #[cfg(unix)]
    const SHELL_PROBE_MARKER: &str = "TAKO_1253_HISTORY_MARKER";

    /// 子のシェル probe が打った行の実行まで見届けたときに出す目印（#2479）。
    /// 親はこれが無ければ落とす = probe が走らないまま「履歴が無い」で緑になる形を塞ぐ
    #[cfg(unix)]
    const SHELL_PROBE_DONE: &str = "TAKO_2479_PROBE=executed";

    /// シェル probe の待ちの素の上限（混み具合で伸ばす前。#2479）。**旧の固定窓（20 秒）より
    /// 広く採る**（等しいと空いている機で旧と同じ形になる。#771 / #1308 と同じ理由）
    #[cfg(unix)]
    const SHELL_PROBE_BASE: std::time::Duration = std::time::Duration::from_secs(30);

    /// 打った行を送り直す回数の上限（間隔 = 予算 / これ。予算を伸ばしても回数は増えない）
    #[cfg(unix)]
    const SHELL_PROBE_MAX_SENDS: u32 = 8;

    /// 出力ゼロで終わった子を起こし直す上限（初回を含む。[`run_child_at`]。#1832）
    const SILENT_CHILD_ATTEMPTS: usize = 3;

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
        run_child_at(&exe, fake_home, data_dir, legacy, extra)
    }

    /// [`run_child_with`] の子を**指定の実行ファイル**から起こす版（#1811）。
    /// `deps/` の外へ置き直したこのテストバイナリ（[`placed_outside_deps`]）を渡す
    fn run_child_at(
        exe: &Path,
        fake_home: &Path,
        data_dir: Option<&Path>,
        legacy: bool,
        extra: &[(&str, &str)],
    ) -> (bool, String, Vec<String>) {
        let mut cmd = std::process::Command::new(exe);
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
            .env_remove("TAKO_SELF_TEST")
            .env_remove("TAKO_VISUAL_TEST")
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
        // #1811 の A/B は `extra` で名指ししたときだけ立てる（親の env から漏らさない）
        cmd.env_remove("TAKO_1811_LEGACY");
        for (key, val) in extra {
            cmd.env(key, val);
        }
        // 子が **1 バイトも出さずに**失敗した回は、子のテスト本体まで届いていない
        // （libtest は本体より先に `running 1 test` を出す）= 見たい性質を 1 つも検査して
        // いないし、書き込みも起きていない。#1832 で観測した落ち方がこれ（負荷の下の
        // workspace 全体の実行で 1 回。子の stdout / stderr はどちらも空）なので、上限つきで
        // 起こし直す。諦めたときは各回の終了状態（コード / シグナル）を出力の末尾へ残す
        let mut statuses: Vec<String> = Vec::new();
        let out = loop {
            let out = cmd.output().expect("子テストプロセスを起こせる");
            let reached = out.status.success() || !out.stdout.is_empty() || !out.stderr.is_empty();
            if reached || statuses.len() + 1 >= SILENT_CHILD_ATTEMPTS {
                break out;
            }
            statuses.push(out.status.to_string());
        };
        let mut stdout = String::from_utf8_lossy(&out.stdout).to_string();
        stdout.push_str(&String::from_utf8_lossy(&out.stderr));
        if !out.status.success() {
            statuses.push(out.status.to_string());
            stdout.push_str(&format!(
                "\n[#1832] 子の終了状態（起こし直した回を含む）: {}\n",
                statuses.join(" / ")
            ));
        }
        (out.status.success(), stdout, files_under(fake_home))
    }

    /// このテストバイナリを**`deps/` の外**（一時 dir）へ置き直したパス（#1811）。
    ///
    /// `rename` が偽なら同じ名前（`<名前>-<hash>`。#1811 の事故の形）、真なら
    /// ハッシュの付かない名前にする（名前の形で拾う判定では救えない形）。
    /// 置き直しはプロセスで 1 回だけ（テストバイナリは大きいので、テストごとに複製しない）。
    /// 置き場は [`tako_core::test_residue::process_scratch`] = プロセス終了時に消える
    fn placed_outside_deps(rename: bool) -> PathBuf {
        static SAME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        static RENAMED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        let slot = if rename { &RENAMED } else { &SAME };
        slot.get_or_init(|| {
            let exe = std::env::current_exe().expect("テストバイナリのパス");
            let name = if rename {
                format!("tako-1811-renamed{}", std::env::consts::EXE_SUFFIX)
            } else {
                exe.file_name()
                    .expect("テストバイナリのファイル名")
                    .to_string_lossy()
                    .into_owned()
            };
            let dir = tako_core::test_residue::process_scratch(if rename {
                "1811-renamed"
            } else {
                "1811-same"
            });
            let to = dir.join(name);
            // 同じボリュームならハードリンク（一瞬で済む）、跨ぐならコピーへ落とす
            // （Windows の CI は一時 dir と作業ツリーのドライブが違う）
            if std::fs::hard_link(&exe, &to).is_err() {
                std::fs::copy(&exe, &to).expect("テストバイナリを一時 dir へ置き直せる");
            }
            assert!(
                to.parent()
                    .and_then(Path::file_name)
                    .is_none_or(|n| n != "deps"),
                "置き直し先が deps/ の直下になっている: {}",
                to.display()
            );
            to
        })
        .clone()
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

    /// **tako の書き込みではない**ので数から除くもの（#1278）。
    ///
    /// 子は実シェルを起こす（#1253 のシェル probe）。`HOME` / `LOCALAPPDATA` を
    /// 差し替えた隔離では、**そのシェル自身が**自分のキャッシュをこの木の下へ書く。
    /// tako は 1 バイトも関与しないので「本番相当の置き場へ tako が書いた」ではない。
    /// CI の Windows ランナーで実測: PowerShell が
    /// `AppData/Local/Microsoft/Windows/PowerShell/StartupProfileData-NonInteractive`
    /// を書く。**前方一致で最小限だけ**除き、増やすときは「誰が書いたか」を必ず書く
    const NOT_TAKO_WRITES: &[&str] = &[
        // PowerShell 5.1 / 7 の起動プロファイルキャッシュ（pwsh 自身が書く）
        "AppData/Local/Microsoft/Windows/PowerShell/",
    ];

    /// 「本番相当の場所」= 空の `HOME` の下に出来たファイル全部
    /// （[`NOT_TAKO_WRITES`] を除く）。何が出来たかを人が読める形で返す（0 件なら空 Vec）
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
        out.retain(|p| !NOT_TAKO_WRITES.iter().any(|skip| p.starts_with(skip)));
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

        // 4b. 最近開いた項目（<data_dir>/recent.json）。#1811 の実害はここで、
        //     deps/ の外へコピーしたテストバイナリが本番の一覧を 1 件で上書きした
        //     （復元元なし）。tako-core 側の書き込みなので cfg(test) は届かない
        let mut recent = tako_core::recent::RecentList::default();
        recent.push(tako_core::recent::RecentEntry::Ssh {
            host: "tako-1811-probe".into(),
        });
        recent.save();

        // 4c. Web ビューの生成中の印（<data_dir>/webview-guard.json と .lock。#724 の W4）。
        //     足して外す = 平常時と同じくファイルは残らないが、排他の .lock と書き先は見る
        let probe = crate::webview_guard::GuardMark {
            pane: Some(944),
            url: "about:blank".into(),
        };
        let _ = crate::webview_guard::begin(&probe);
        let _ = crate::webview_guard::end(&probe);

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
    /// 待ちは固定時間ではなく**画面の状態**で切り、尽きたら次へ進まずに落とす
    /// （[`wait_for_shell`]。#2479）。**前提（シェルが上がった）と本題（打った行が
    /// 実行された）は別々に待つ**（#1308 と同じ理由: 1 つの窓にまとめると
    /// 「準備ができていない」と「本題が壊れた」が同じ失敗に見える）
    #[cfg(unix)]
    fn shell_probe() {
        use tako_core::terminal::{SpawnOptions, TerminalSession};

        // tako:login-shell-ok 主題が対話ログインシェルの履歴（#1253）なのでログインシェル自体が要る
        let spawned = TerminalSession::spawn(80, 24, SpawnOptions::default());
        let (session, _rx) = spawned.unwrap_or_else(|e| {
            panic!(
                "TAKO_2479_PROBE: 既定シェル（{}）の PTY を張れない = 履歴の番犬が何も見られない: {e}",
                shell_name()
            )
        });
        let screen = || session.visible_lines().join("\n");
        // 前提: ログインシェルが上がる（画面に何か出る）。上がる前に打った行は食われる
        wait_for_shell(
            "ログインシェルのプロンプト",
            None,
            &screen,
            &|s| !s.trim().is_empty(),
        );
        // 本題: 打った行が実行される = **出力にだけ現れる**マーカーが見える
        // （[`shell_probe_line`]）。`INC_APPEND_HISTORY` はコマンドの確定時に追記するので、
        // ここまで来れば履歴も書かれている
        let send = || session.write(shell_probe_line().into_bytes());
        wait_for_shell("打った行の実行", Some(&send), &screen, &probe_executed);
        println!("{SHELL_PROBE_DONE}");
    }

    /// 画面（行を `\n` でつないだもの）から「打った行が実行された」を読む（#2479）。
    /// 目印は出力にだけ現れる（[`shell_probe_line`]）ので、1 本見えれば足りる
    #[cfg(unix)]
    fn probe_executed(screen: &str) -> bool {
        screen.contains(SHELL_PROBE_MARKER)
    }

    /// シェル probe が打つ行（#2479）。マーカーを `""` で割って打つので、**打った行の
    /// エコーにはマーカーが現れず、実行した出力にだけ現れる**（sh / bash / zsh / dash で実測）。
    ///
    /// 旧実装は `echo <マーカー>` を打ち、「マーカーを含む行が 2 本（エコーと出力）」で実行を
    /// 判定していた。子のシェルの cwd は空の HOME（`$TMPDIR` の下 = `/private/var/…` へ
    /// 解決されて `~` に縮まない）なので、プロンプトに長い一時 dir 名が載り、打った行が
    /// 80 桁で折り返すと**エコー側のマーカーが 2 行に割れて**数えられない。ユーザー名と
    /// ホスト名の長さ次第で、毎回 20 秒の窓を使い切って警告だけで通っていた（手元で実測）。
    /// 出力のマーカーは行頭から 24 桁なので折り返さない
    #[cfg(unix)]
    fn shell_probe_line() -> String {
        let (head, tail) = SHELL_PROBE_MARKER.split_at(SHELL_PROBE_MARKER.len() / 2);
        format!("echo {head}\"\"{tail}\r")
    }

    /// 失敗文に載せる既定シェルの名前（**ファイル名だけ**。`$SHELL` がホームの下を
    /// 指していても実ホームパスを出さない = #927）
    #[cfg(unix)]
    fn shell_name() -> String {
        let shell = std::env::var("SHELL").unwrap_or_default();
        Path::new(&shell).file_name().map_or_else(
            || "unknown".to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }

    /// 失敗文に添える機の状態（load の 3 値・メモリ圧・swap。採れない OS では `unknown`）。
    /// 予算は load だけで伸ばすので、load では見えない詰まりも失敗文だけで読めるようにする
    /// （#1308 の 10/25 と同じ理由）
    #[cfg(unix)]
    fn machine_line() -> String {
        let Some(m) = tako_core::platform::resources::machine() else {
            return "load=unknown pressure=unknown swap=unknown".to_string();
        };
        let load = m.load.map_or("unknown".to_string(), |l| {
            format!("{:.1}/{:.1}/{:.1}", l[0], l[1], l[2])
        });
        let pressure = m
            .pressure_level
            .map_or("unknown".to_string(), |p| p.to_string());
        let swap = m.swap.map_or("unknown".to_string(), |(used, total)| {
            format!("{}/{}MB", used >> 20, total >> 20)
        });
        format!("load={load} pressure={pressure} swap={swap}")
    }

    /// シェル probe の状態待ち（#2479）。`ready`（その時点の画面を受ける）が真になるまで待ち、
    /// **尽きたら次の手順へ進まずに理由つきで落とす**。
    ///
    /// 旧実装は固定 20 秒の窓でプロンプトを待ち、尽きても検査せずに打ち、打った行の実行も
    /// 20 秒待って出そろわなければ警告だけで返していた（#1308 と同じ形）。打った行が
    /// 実行されなければ履歴は書かれないので、**番犬が何も見ないまま「履歴へ書かない」で緑**になる。
    ///
    /// - 上限は `wait_budget::state_wait_budget`（混み具合で伸ばすだけ・4 倍で打ち切り）
    /// - `send` を渡すと届くまで上限つきで送り直す（#1165 / #1308 と同じ形。「画面に何か
    ///   出た」は zle が上がった証拠としては弱く、起動途中の PTY は打鍵を落とす = #640）。
    ///   間隔は予算に比例 = 混み具合が変わっても送る回数は変わらない
    /// - 失敗文は見出し `TAKO_2479_WAIT:` の 1 行に値をすべて載せ、続けて画面を出す
    ///   （#1308 の 10/25: 値を 2 行目に置くと `grep` で消える）
    #[cfg(unix)]
    fn wait_for_shell(
        what: &str,
        send: Option<&dyn Fn()>,
        screen: &dyn Fn() -> String,
        ready: &dyn Fn(&str) -> bool,
    ) {
        use std::time::{Duration, Instant};

        let started = Instant::now();
        let busy = tako_core::wait_budget::machine_busy();
        let budget = tako_core::wait_budget::state_wait_budget(SHELL_PROBE_BASE, busy);
        let every = budget / SHELL_PROBE_MAX_SENDS;
        let mut next_send = started;
        let mut sends = 0u32;
        let mut last_screen = String::new();
        let mut changes = 0u32;
        let mut last_change: Option<Duration> = None;
        loop {
            let now = screen();
            if now != last_screen {
                changes += 1;
                last_change = Some(started.elapsed());
                last_screen = now.clone();
            }
            if ready(&now) {
                return;
            }
            if started.elapsed() >= budget {
                let fmt_busy =
                    |b: Option<f64>| b.map_or("unknown".to_string(), |b| format!("{b:.2}"));
                panic!(
                    "TAKO_2479_WAIT: 待っていたもの={what} waited={:.1}s budget={:.1}s \
                     base={:.1}s busy={}→{} {} sends={sends} screen_changes={changes} \
                     last_change={} shell={}\n\
                     固定窓ではなく状態で待っている。尽きたまま次へ進むと、履歴が書かれないまま\
                     「履歴へ書かない」で緑になる（#2479）\n届いたもの=画面:\n{now}",
                    started.elapsed().as_secs_f64(),
                    budget.as_secs_f64(),
                    SHELL_PROBE_BASE.as_secs_f64(),
                    fmt_busy(busy),
                    fmt_busy(tako_core::wait_budget::machine_busy()),
                    machine_line(),
                    last_change.map_or("never".to_string(), |d| format!("{:.1}s", d.as_secs_f64())),
                    shell_name(),
                );
            }
            if Instant::now() >= next_send {
                if let Some(send) = send {
                    send();
                    sends += 1;
                }
                next_send = Instant::now() + every;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
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
        assert!(
            has("recent.json"),
            "旧挙動で recent.json が出ない（子の 4b が書いていない）: {created:#?}"
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

        // 子が打った行の実行まで見届けたか（#2479）。見届けずに返る子だと、下の「履歴が無い」は
        // 何も見ていない緑になる。`--nocapture` の libtest は `test <名前> ... ` の後ろへ
        // 改行せずに続けるので、行頭ではなく部分一致で拾う（#2248）
        let probed = |stdout: &str| {
            assert!(
                stdout.contains(SHELL_PROBE_DONE),
                "子のシェル probe が打った行の実行まで見届けていない（#2479）\n{stdout}"
            );
        };

        let home = prepare("histfix");
        let (ok, stdout, created) = run_child_with(&home, None, false, &[(SHELL_PROBE_ENV, "1")]);
        let _ = std::fs::remove_dir_all(&home);
        assert!(ok, "子テストが失敗した\n{stdout}");
        probed(&stdout);
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
        probed(&stdout);
        if history_files(&created).is_empty() {
            eprintln!(
                "skip(A/B のみ): 既定シェル（{}）が履歴ファイルを持たないので旧挙動を実証できない",
                std::env::var("SHELL").unwrap_or_default()
            );
        }
    }

    /// #2479: シェル probe の「打った行が実行された」は、**打った行のエコーと折り返しに
    /// 左右されない**。
    ///
    /// 旧実装は「目印を含む行が 2 本（エコーと出力）」で判定していて、打った行が 80 桁で
    /// 折り返すとエコー側の目印が 2 行に割れて成り立たなかった（下の `executed` は手元で
    /// 実測した画面の形。ユーザー名・ホスト名・時刻はプレースホルダ）。送り直しを足すと、
    /// 逆にエコー 2 本だけで（実行されていなくても）成り立つ形でもあった
    #[test]
    #[cfg(unix)]
    fn シェルprobeの実行の判定は打った行のエコーと折り返しに左右されない() {
        let typed = shell_probe_line();
        let echo = typed.trim_end_matches('\r');
        assert!(
            !echo.contains(SHELL_PROBE_MARKER),
            "打つ行に目印がそのまま入っている = エコーだけで実行に見える: {echo:?}"
        );
        // 実行されない（tty のエコーだけが送り直しのたびに積もる）画面は、どの桁で
        // 折り返しても実行と見なさない
        let line = format!("testuser@host % {echo}");
        for col in 1..line.len() {
            let (a, b) = line.split_at(col);
            let deaf = format!("{a}\n{b}\n{line}\n{line}");
            assert!(
                !probe_executed(&deaf),
                "エコーだけの画面を実行と見なした（{col} 桁で折り返し）:\n{deaf}"
            );
        }
        // 実行された画面（打った行は折り返している）は 1 本の出力で実行と見なす
        let wrapped = format!("testuser@host tako-944-histfix-12345-1761700000000000000 % {echo}");
        let (a, b) = wrapped.split_at(80);
        let executed = format!("{a}\n{b}\n{SHELL_PROBE_MARKER}\ntestuser@host % ");
        assert!(
            probe_executed(&executed),
            "実行された画面を見落とした:\n{executed}"
        );
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

    /// 安全側へ倒したときに子が stderr へ出す目印（`paths::warn_no_product_entry`）
    const FAILSAFE_MARKER: &str = "Issue #1811";

    /// #1811: **テストバイナリを `deps/` の外へ置いても**本番相当の置き場へ 1 バイトも書かない。
    ///
    /// 修正前は `is_test_process()` が実行ファイルの置き場だけで決まっていたので、
    /// 計測のために `/tmp` へコピーしたテストバイナリが本番の `recent.json` を上書きし、
    /// `shell-integration/cli-dir` を空にし、実 agent CLI を実 HOME で起動した。
    /// 同じ名前のまま置き直した形（事故の形）と、改名した形（名前の形で拾う判定では
    /// 救えない形）の両方を見る。どちらも**安全側へ倒した理由**（製品の入口を通っていない）
    /// を stderr へ出すことまで確かめる（`deps/` の判定で偶然真になったのではない）
    #[test]
    fn depsの外へ置いたテストバイナリも本番相当の置き場へ何も書かない() {
        for rename in [false, true] {
            let exe = placed_outside_deps(rename);
            let home = tako_core::test_residue::ScratchDir::new("1811-home");
            let (ok, stdout, created) = run_child_at(&exe, home.path(), None, false, &[]);

            assert!(ok, "子テストが失敗した（rename={rename}）\n{stdout}");
            assert!(
                stdout.contains("1 passed"),
                "子テストが実行されていない（rename={rename}）\n{stdout}"
            );
            assert!(
                created.is_empty(),
                "deps/ の外のテストバイナリが本番相当の HOME へ書いた（#1811。rename={rename}）: \
                 {created:#?}"
            );
            assert!(
                stdout.contains(FAILSAFE_MARKER),
                "安全側へ倒した知らせが出ていない（rename={rename}）\n{stdout}"
            );
        }
    }

    /// A/B: 修正前の判定（`TAKO_1811_LEGACY=1` = 置き場だけで決める）へ戻すと、
    /// `deps/` の外へ置いた**同じ子**が本番相当の場所へ書く = 上の番犬に検出力がある。
    ///
    /// 事故で観測した書き込み（`recent.json` の上書き・`shell-integration/cli-dir`・
    /// `persist.log`・`~/.claude.json` 系）をここで名指しで再現する
    #[test]
    fn 修正前の判定ではdepsの外のテストバイナリが本番相当の置き場へ書く() {
        let exe = placed_outside_deps(false);
        let home = tako_core::test_residue::ScratchDir::new("1811-legacy");
        let (ok, stdout, created) =
            run_child_at(&exe, home.path(), None, false, &[("TAKO_1811_LEGACY", "1")]);

        assert!(ok, "子テストが失敗した\n{stdout}");
        let has = |needle: &str| created.iter().any(|f| f.contains(needle));
        for needle in [
            "recent.json",
            "shell-integration/cli-dir",
            "persist.log",
            ".claude",
        ] {
            assert!(
                has(needle),
                "修正前の判定で {needle} が出ない（検出力が無い）: {created:#?}"
            );
        }
        assert!(
            !stdout.contains(FAILSAFE_MARKER),
            "修正前の判定なのに安全側へ倒した知らせが出ている\n{stdout}"
        );
    }

    /// #1811 のエッジ: `deps/` の外でも**明示の `TAKO_DATA_DIR` は尊重**し、
    /// ホーム側（外部エージェントの設定）は倒したままにする
    #[test]
    fn depsの外でも明示のtako_data_dirは尊重される() {
        let exe = placed_outside_deps(true);
        let base = tako_core::test_residue::ScratchDir::new("1811-explicit");
        let fake_home = base.path().join("home");
        let data_dir = base.path().join("data");
        std::fs::create_dir_all(&fake_home).expect("空の HOME を作れる");

        let (ok, stdout, in_home) = run_child_at(&exe, &fake_home, Some(&data_dir), false, &[]);
        let in_data = files_under(&data_dir);

        assert!(ok, "子テストが失敗した\n{stdout}");
        for name in ["persist.log", "recent.json"] {
            assert!(
                in_data.iter().any(|f| f == name),
                "明示した TAKO_DATA_DIR へ {name} が書かれていない: {in_data:#?}"
            );
        }
        assert!(
            in_home.is_empty(),
            "deps/ の外で TAKO_DATA_DIR を渡しても HOME へ書いている: {in_home:#?}"
        );
    }
}
