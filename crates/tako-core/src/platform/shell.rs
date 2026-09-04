//! 既定シェルの解決（抽象境界 B1）
//!
//! 「ペインで何を起動するか」「明示コマンドをどう包むか」のプラットフォーム差を閉じ込める。
//! PTY そのもの（unix pty / ConPTY）は `alacritty_terminal::tty` が吸収するため、
//! ここでは扱わない（設計 §2.2）。

use crate::platform::shell_dialect::ShellDialect;
use crate::terminal::SpawnCommand;

/// 既定シェル。`None` を返すとペインを spawn できない。
///
/// **ペインへ打ち込むコマンドの構文はこのシェルで決まる**（#867）。同じ OS でも
/// PowerShell か POSIX かで `export` / インライン env 前置きの書き方が変わるので、
/// 起動コマンドを組み立てる側（`tako-control::launch_cmd`）がここを見る
pub fn default_shell() -> Option<SpawnCommand> {
    imp::default_shell()
}

/// 明示コマンド（`tako split -- <command>` 等）を、ユーザーの環境で実行される形に包む。
///
/// **「ユーザーの環境」= 対話ペインと同じ環境**（#1031）。素のペインは
/// [`default_shell`] が返す `$SHELL -l` を **tty 上で**起こすので、zsh から見ると
/// 「対話 + ログイン」シェル = `.zprofile` も `.zshrc` も読まれる。ところが明示コマンドは
/// `-c` を付けるぶん**非対話**になり、`.zshrc` だけが読まれない。nodebrew / fnm /
/// Homebrew の PATH を `.zshrc` で通している人は多いので、同じ tako の中なのに
/// 「素のペインでは `npm` が引けるのに、カードの実行ペインでは command not found」に
/// なっていた（#1031 実発）。`-i` を足して**素のペインと同じ読み込み順**へ揃える。
///
/// `TAKO_1031_LEGACY=1` で #1031 前（`-l -c` = 非対話）へ戻せる = 同一バイナリで A/B が取れる
pub fn login_shell_command(command: SpawnCommand) -> SpawnCommand {
    imp::login_shell_command(command)
}

/// コマンドが**失敗したときだけ**ペインを残す形に包む（#1031）。
///
/// `tako split --command` のペインは「コマンドの終了 = ペイン close」なので、
/// 起動に失敗すると数秒で消え、理由が画面にもログにも残らなかった（#1031 実発）。
/// 成功したときは従来どおり即 close（`exit 0` するだけ）、非 0 のときだけ
/// 終了コードのマーカー行 + 案内を出して入力待ちで止める。
///
/// マーカーの形は実行ペイン（[`run_pane_command`]）と同じ `<marker_prefix><code>` で、
/// 読む側（`tako_control::dispatch::find_exit_marker`）も 1 つ。**接頭辞は呼び出し側が渡す**
/// （契約の持ち主を増やさない）。
///
/// `TAKO_1031_LEGACY=1` では**包まない** = #1031 前の「失敗しても黙って消える」へ戻る
pub fn hold_on_failure_command(command: SpawnCommand, marker_prefix: &str) -> SpawnCommand {
    if legacy_1031() {
        return command;
    }
    imp::hold_on_failure_command(command, marker_prefix, hold_hint(crate::i18n::lang()))
}

/// 失敗したペインに出す案内（日英）。
///
/// シェルへ**そのまま埋め込む**ので `"` / `$` / バッククォート / `\` を含めない
/// （POSIX は二重引用符の中、PowerShell は単引用符の中に置く）。
/// 純粋関数にしてあるので macOS 上から両言語を検査できる
pub fn hold_hint(lang: crate::i18n::Lang) -> &'static str {
    match lang {
        crate::i18n::Lang::Ja => {
            "[tako] コマンドが失敗しました。上の出力を確認できます。Enter でこのペインを閉じます。"
        }
        crate::i18n::Lang::En => {
            "[tako] The command failed. The output above is kept. Press Enter to close this pane."
        }
    }
}

/// #1031 の A/B ゲート（対話シェル化と失敗時の保持を両方 #1031 前へ戻す）
fn legacy_1031() -> bool {
    std::env::var_os("TAKO_1031_LEGACY").is_some()
}

/// 実行ペイン（#453 Code Runner / #666 コマンド提案カード / `tako run-interactive`）を
/// 起こすコマンド。
///
/// 組み立てる形は「コマンド本体 → 終了コードを `<marker_prefix><code>` の 1 行で出力 →
/// 入力待ちで停止」。入力待ちで止めるのは、即終了するコマンドでも出力を読めるように
/// ペインを残すため。`RunInteractiveStatus` はこのマーカー行を画面から拾って
/// 終了コードと auto_close を決めるので、**マーカーの出力が唯一の契約**
/// （`tako_control::dispatch::find_exit_marker`）。
///
/// 呼び出し側（`dispatch`）が `/bin/sh -c` を直書きしていたため Windows では
/// `CreateProcess` が失敗し、ペインだけ生えて PTY が立たなかった（#875）
pub fn run_pane_command(command: &str, marker_prefix: &str) -> SpawnCommand {
    imp::run_pane_command(command, marker_prefix)
}

/// スクリプト本文をそのままシェルへ渡すペイン用コマンド（境界 B1。#919）。
///
/// [`run_pane_command`] との違いは**終了マーカーと入力待ちを付けない**こと。
/// 「どこで待つか」「失敗したときだけ画面を残すか」をスクリプト側が決めたい用途
/// （SSH ペインの接続前バナー + 失敗時の理由表示）で使う。
///
/// スクリプトの方言（POSIX / PowerShell）は呼び出し側が
/// [`script_dialect`] を見て組む
pub fn script_pane_command(script: &str) -> SpawnCommand {
    imp::script_pane_command(script)
}

/// このプラットフォームで tako がシェルへ流すスクリプトの方言（#919 / #935）。
///
/// 実行するシェルは [`script_pane_command`]（ペイン）と [`output_command`]
/// （PTY 無し）が決めるので、**OS ではなくそのシェル**に合わせた文法で組む必要がある。
/// どちらも同じシェル系統（unix = POSIX / Windows = PowerShell）を起こすので
/// 方言の判定はこの 1 本で足りる。enum は #873 で一本化した
/// [`ShellDialect`] を使う（新しい判定を作らない）
pub fn script_dialect() -> ShellDialect {
    imp::script_dialect()
}

/// 「1 本の文字列としてのシェル片」を **PTY 無し**で走らせる `Command`（境界 B1。#935）。
///
/// [`run_pane_command`] / [`script_pane_command`] との違いは**ペインを持たない**こと。
/// 出力をその場で読み取って判定に使う用途（受け入れゲートのコマンド型述語 =
/// `tako task gate check`）向けで、返した `Command` に呼び出し側が
/// `current_dir` 等を足して `output()` する。
///
/// - **unix は `sh -c <片>`**（#935 以前の `acceptance_gates` と 1 バイトも変えない）
/// - **Windows は PowerShell へ `-EncodedCommand`**。`sh` は無いので
///   `CreateProcess` が失敗し、コマンド型ゲートが一切判定できなかった（#935）
///
/// コンソールウィンドウの抑止（#586）は**ここで済ませる**。GUI プロセスから到達する
/// 経路なので呼び出し側に思い出させない。
///
/// `TAKO_935_LEGACY=1` で #935 前の挙動（**プラットフォームに依らず POSIX シェルを
/// 直起動する**）へ戻せる = 同一バイナリで A/B が取れる。Windows では `sh` が
/// 無いので `CreateProcess` が失敗し、当時の症状（どの述語も「コマンド実行に失敗」）が
/// そのまま再現する。macOS では新旧が同じ argv になるので挙動は変わらない
pub fn output_command(script: &str) -> std::process::Command {
    if std::env::var_os("TAKO_935_LEGACY").is_some() {
        return build_output_command(&posix_output_argv(script));
    }
    imp::output_command(script)
}

/// `tako:shell` 宣言（`# tako:shell: pwsh`）で指定されたシェルへコマンドを包む。
///
/// **OS ではなく宣言されたシェルで決まる**ので `cfg` を持たない。判定は
/// [`ShellDialect::from_program`] 1 本に任せ、判定できないシェル（fish / cmd.exe /
/// Git Bash の `bash` 等）は POSIX 形のままにする — Windows でも POSIX シェルを
/// 入れて使う人はいるので「知らないシェルは POSIX 扱い」が既定
pub fn declared_shell_command(shell: &str, command: &str) -> String {
    match ShellDialect::from_program(shell) {
        Some(ShellDialect::PowerShell) => {
            format!(
                "{shell} -Command {}",
                ShellDialect::PowerShell.quote_arg(command)
            )
        }
        // 単引用符で包み、中の `'` は `'\''` で閉じ直す（POSIX 形）。
        // `quote_arg`（= `quote_for_shell`）は「必要なときだけ」引用するので使わない:
        // `bash -c ls` へ変わって macOS の出力がバイト等価でなくなる
        _ => format!("{shell} -c '{}'", command.replace('\'', "'\\''")),
    }
}

/// PTY の子へ argv を「1 語 = 1 引数」で届ける（#884）。
///
/// unix は `execvp` へ argv がそのまま渡るので**何もしない**。Windows には argv という
/// 概念が無く、`CreateProcessW` へ渡すのは 1 本のコマンドライン文字列なので、
/// alacritty が `program` と `args` を空白で連結する。その既定（`escape_args = false`）は
/// **各語を素のまま**つなぐため、空白を含む語が子側の CRT パーサで複数語へ割れる。
///
/// 実害（psmux 3.3.7 / Windows 11 で実測）: [`crate::tmux_backend::wrap_options`] が積む
/// `-c <cwd>` の cwd が `C:\Users\...\dir with space` のとき、器へは
/// `-c C:\Users\...\dir` `with` `space` として届く。`new-session` は余った語を
/// **shell-command** と解釈して実行するので `with: The term 'with' is not recognized`
/// でペインが即死する。tako の器設定は `remain-on-exit` が off なので
/// **画面には何も出ないまま**ペインごと消える。`-e KEY=<空白入りの値>` も同じ機序で壊れる。
///
/// 語ごとの引用を自前で組まずここで alacritty へ委ねるのは、CRT 規則
/// （引用符の前の連続バックスラッシュを倍にする等）を二重実装しないため
pub fn apply_arg_escaping(options: &mut alacritty_terminal::tty::Options) {
    imp::apply_arg_escaping(options);
}

#[cfg(unix)]
mod imp {
    use super::*;

    /// unix の `execvp` は argv をそのまま受け取るので、組み直す余地が無い
    pub(crate) fn apply_arg_escaping(_options: &mut alacritty_terminal::tty::Options) {}

    /// unix では alacritty に `None` を渡さず**ここで明示解決する**。
    ///
    /// alacritty の既定（None）は macOS で setuid root の `login` ラッパ経由になり、
    /// ペイン close 時の `Pty::drop` が `kill(login, SIGHUP)` を権限エラーで失敗（返り値無視）
    /// → `child.wait()` が永久ブロック → master fd・signal fd・IO スレッド・login プロセスが
    /// **close のたびにリーク**する。fd 枯渇で PTY 生成が失敗し日常使用でアプリが死ぬ
    /// （2026-06-11 常用報告の根本原因）。本家 alacritty はウィンドウ close = プロセス終了の
    /// ため顕在化しないが、tako はペイン単位でセッションを破棄するので直撃する。
    /// ユーザー権限のシェルを直接 spawn すれば SIGHUP が届き wait も即返る。
    /// `-l` でログインシェル動作（profile 読み込み）は維持する
    pub(crate) fn default_shell() -> Option<SpawnCommand> {
        Some(SpawnCommand {
            program: user_shell(),
            args: vec!["-l".into()],
        })
    }

    /// .app（Dock 起動）のプロセス環境は PATH が最小構成（/usr/bin:/bin:…）のため、
    /// コマンドを直接 exec すると Homebrew の `tmux` や `npm` が見つからず PTY 生成に
    /// 失敗する（2026-06-12 実機リグレッション）。`$SHELL -l -c "<コマンド>"` にして
    /// ユーザーの PATH・環境変数で実行する
    pub(crate) fn login_shell_command(command: SpawnCommand) -> SpawnCommand {
        super::posix_login_shell_command(
            &user_shell(),
            &crate::tmux_backend::shell_quoted(&command),
            // #1031: 素のペイン（`$SHELL -l` を tty で起こす = 対話）と読み込み順を揃える
            !super::legacy_1031(),
        )
    }

    pub(crate) fn hold_on_failure_command(
        command: SpawnCommand,
        marker_prefix: &str,
        hint: &str,
    ) -> SpawnCommand {
        super::posix_hold_on_failure_command(&command, marker_prefix, hint)
    }

    pub(crate) fn run_pane_command(command: &str, marker_prefix: &str) -> SpawnCommand {
        super::posix_run_pane_command(command, marker_prefix)
    }

    pub(crate) fn script_pane_command(script: &str) -> SpawnCommand {
        // `posix_run_pane_command` と同じ理由で `/bin/sh` 決め打ち
        // （fish のような POSIX でないログインシェルでも成立させる）
        SpawnCommand {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
        }
    }

    pub(crate) fn script_dialect() -> super::ShellDialect {
        super::ShellDialect::Posix
    }

    pub(crate) fn output_command(script: &str) -> std::process::Command {
        super::build_output_command(&super::posix_output_argv(script))
    }

    fn user_shell() -> String {
        std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    /// 各語を CRT 規則でエスケープさせる（#884）。既定の `false` は素の連結なので、
    /// 空白を含む語が子側で複数語へ割れる
    pub(crate) fn apply_arg_escaping(options: &mut alacritty_terminal::tty::Options) {
        options.escape_args = true;
    }

    pub(crate) fn default_shell() -> Option<SpawnCommand> {
        Some(SpawnCommand {
            program: super::resolve_windows_shell(
                env_string("ProgramFiles"),
                env_string("SystemRoot"),
                env_string("ComSpec"),
                &|p| std::path::Path::new(p).exists(),
            ),
            // 引数は付けない。実機で banner の見え方を確認してから調整する（#517）
            args: Vec::new(),
        })
    }

    /// Windows の `CreateProcess` は PATH 探索を行うため、単一コマンドはそのまま起動できる。
    ///
    /// `tako split -- <command>` 等の**語のリスト**で来る経路はこれで足りる。
    /// 複合シェル構文を含む「1 本の文字列」を走らせたい経路（実行ペイン）は
    /// ここではなく `run_pane_command` が PowerShell へ包む（#875）
    pub(crate) fn login_shell_command(command: SpawnCommand) -> SpawnCommand {
        command
    }

    /// argv を PowerShell の呼び出し演算子（`&`）で起こし、失敗したときだけ止める。
    ///
    /// POSIX と違ってここが**唯一のシェル層**（[`login_shell_command`] は素通し）なので、
    /// `$PROFILE` を読む形（`-NoProfile` を付けない）にして
    /// [`run_pane_command`] と環境を揃える
    pub(crate) fn hold_on_failure_command(
        command: SpawnCommand,
        marker_prefix: &str,
        hint: &str,
    ) -> SpawnCommand {
        super::powershell_hold_on_failure_command(&run_pane_shell(), &command, marker_prefix, hint)
    }

    pub(crate) fn run_pane_command(command: &str, marker_prefix: &str) -> SpawnCommand {
        super::powershell_run_pane_command(&run_pane_shell(), command, marker_prefix)
    }

    pub(crate) fn script_pane_command(script: &str) -> SpawnCommand {
        // `-EncodedCommand`（base64 / UTF-16LE）で渡す理由は
        // `powershell_run_pane_command` の doc と同じ（引用符を解釈する層が 3 つある）
        SpawnCommand {
            program: run_pane_shell(),
            args: vec![
                "-NoLogo".to_string(),
                "-EncodedCommand".to_string(),
                super::encode_powershell_command(script),
            ],
        }
    }

    pub(crate) fn script_dialect() -> super::ShellDialect {
        // 起こすのは必ず PowerShell（`run_pane_shell` は cmd.exe へ倒さない）
        super::ShellDialect::PowerShell
    }

    pub(crate) fn output_command(script: &str) -> std::process::Command {
        // ペインと同じシェルを起こす（`script_dialect` の答えと食い違わせない）
        super::build_output_command(&super::powershell_output_argv(&run_pane_shell(), script))
    }

    /// 実行ペインを起こす PowerShell の実行ファイル。
    ///
    /// 既定シェル（[`default_shell`]）が PowerShell ならその解決結果を使う
    /// （pwsh 7 が入っていればそれが選ばれる = ユーザーが日常使うシェルと揃う）。
    /// **`cmd.exe` へは倒さない**: 実行ペインの契約は「終了コードを 1 行のマーカーで
    /// 出す」ことで、cmd は `%ERRORLEVEL%` の展開時期も引用符の扱いも別物になる。
    /// `powershell.exe`（5.1）は Windows 同梱で System32 に必ず居るので、
    /// 既定シェルが PowerShell でないときはそれを名前で起こす（PATH 探索が効く）。
    /// 方言の判定は [`ShellDialect::from_program`] に任せる（判定を増やさない。#873）。
    ///
    /// **解決したフルパスをそのまま返す**。器（psmux）が空白入りのパスを運べない件は
    /// #875 でここが実行ファイル名へ落として回避していたが、#881 で器側
    /// （`platform::program_path` 経由の `backend::psmux::inner_command`）が
    /// 面倒を見るようになったので、取り違えの余地が無い正確な方を渡す
    fn run_pane_shell() -> String {
        default_shell()
            .map(|s| s.program)
            .filter(|p| ShellDialect::from_program(p) == Some(ShellDialect::PowerShell))
            .unwrap_or_else(|| "powershell.exe".to_string())
    }

    /// 非 UTF-8 の環境変数値は解決に使えないため落とす（後段のフォールバックへ回す）
    fn env_string(key: &str) -> Option<String> {
        std::env::var_os(key).and_then(|v| v.into_string().ok())
    }
}

/// POSIX の「ユーザーの環境で 1 本のシェル片を走らせる」argv（純粋関数。#1031）。
///
/// `interactive`（= `-i`）が付くと zsh は `.zshrc` も読む。**素のペインと同じ読み込み順**に
/// するのが目的で、`-l`（`.zprofile`）は従来どおり残す。bash は「対話ログイン」でも
/// `.bashrc` ではなく `.bash_profile` を読む（= `-i` の有無で読むファイルが変わらない）が、
/// これも素のペイン（`bash -l` を tty で起こす）と同じなので**揃っている**のが正しい。
///
/// フラグの並びを `-l -i -c` に固定するのは、`-c` の直後がスクリプト本体である必要があるため
#[cfg_attr(windows, allow(dead_code))]
fn posix_login_shell_command(shell: &str, script: &str, interactive: bool) -> SpawnCommand {
    let mut args = vec!["-l".to_string()];
    if interactive {
        args.push("-i".to_string());
    }
    args.push("-c".to_string());
    args.push(script.to_string());
    SpawnCommand {
        program: shell.to_string(),
        args,
    }
}

/// POSIX の「失敗したときだけ止める」argv（純粋関数。#1031）。
///
/// 成功時は `exit 0` で即終了する = ペインは従来どおり閉じる。非 0 のときだけ
/// マーカー行 + 案内を出して `read` で待つ。最後に元の終了コードで `exit` するので、
/// 呼び出し元（`login_shell_command` のラッパーシェル）から見た終了コードも保たれる。
///
/// `/bin/sh` 決め打ちの理由は [`posix_run_pane_command`] と同じ
/// （fish のような POSIX でないログインシェルでも成立させる）。この 1 段は
/// 実行ペインが既に通っている形と同じで、プロセスの深さも同じ
#[cfg_attr(windows, allow(dead_code))]
fn posix_hold_on_failure_command(
    command: &SpawnCommand,
    marker_prefix: &str,
    hint: &str,
) -> SpawnCommand {
    let inner = crate::tmux_backend::shell_quoted(command);
    let script = format!(
        "{inner}; __tako_code=$?; \
         if [ \"$__tako_code\" -ne 0 ]; then \
         echo \"{marker_prefix}$__tako_code\"; \
         echo \"{hint}\"; \
         read -r __TAKO_HOLD__ 2>/dev/null || true; \
         fi; \
         exit \"$__tako_code\""
    );
    SpawnCommand {
        program: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), script],
    }
}

/// Windows の「失敗したときだけ止める」argv（純粋関数。**macOS 上でもテストできる**。#1031）。
///
/// 終了コードの決め方は [`powershell_exit_code_script`] に任せる（規則を 2 つ持たない）。
/// argv は呼び出し演算子（`&`）で起こす: `Invoke-Expression` と違って**語のリストのまま**
/// 渡せるので、空白や日本語を含む引数が割れない
#[cfg_attr(not(windows), allow(dead_code))]
fn powershell_hold_on_failure_command(
    program: &str,
    command: &SpawnCommand,
    marker_prefix: &str,
    hint: &str,
) -> SpawnCommand {
    let invocation = std::iter::once(&command.program)
        .chain(command.args.iter())
        .map(|w| ShellDialect::PowerShell.quote_arg(w))
        .collect::<Vec<_>>()
        .join(" ");
    let marker = ShellDialect::PowerShell.quote_arg(marker_prefix);
    let hint_lit = ShellDialect::PowerShell.quote_arg(hint);
    let script = format!(
        "{}if ($__tako_code -ne 0) {{\n\
         Write-Host ({marker} + $__tako_code)\n\
         Write-Host {hint_lit}\n\
         try {{ $null = [Console]::ReadLine() }} catch {{ }}\n\
         }}\nexit $__tako_code\n",
        powershell_exit_code_script(&format!("& {invocation}"))
    );
    SpawnCommand {
        program: program.to_string(),
        args: vec![
            "-NoLogo".to_string(),
            "-EncodedCommand".to_string(),
            encode_powershell_command(&script),
        ],
    }
}

/// POSIX の実行ペイン用コマンド（純粋関数）。
///
/// `read` で入力待ちにするのは、コマンドが即終了しても出力を読めるようにするため。
/// `2>/dev/null || true` は stdin が閉じている場合に非ゼロで落ちないための保険。
///
/// 複合シェルコード（`;` / `||` 入り）は program 1 語に詰めず `/bin/sh -c` の引数で渡す。
/// program に詰めると `login_shell_command` の `shell_quoted` が全文を 1 語にクォートし、
/// シェルが「セミコロン込みの 1 コマンド名」として探して 127 で即死する（#453）。
///
/// ユーザーの `$SHELL` ではなく `/bin/sh` 決め打ちなのは従来どおり
/// （fish のような POSIX でないログインシェルでも実行ペインが成立する）
#[cfg_attr(windows, allow(dead_code))]
fn posix_run_pane_command(command: &str, marker_prefix: &str) -> SpawnCommand {
    let wrapped = format!(
        "{command}; echo \"{marker_prefix}$?\"; read -r __TAKO_DUMMY__ 2>/dev/null || true"
    );
    SpawnCommand {
        program: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), wrapped],
    }
}

/// Windows の実行ペイン用コマンド（純粋関数。**macOS 上でもテストできる**）。
///
/// PowerShell へは `-EncodedCommand`（base64 / UTF-16LE）でスクリプトを渡す。
/// 素の `-Command "…"` にしないのは、コマンド文字列が届くまでに
/// **引用符を解釈する層が 3 つ**あるため:
///
/// 1. psmux の `new-session` 引数（`backend::psmux::inner_command` が POSIX 風に単語分割する）
/// 2. `cmd.exe /c`（プログラムパスに空白があるとき psmux 側がこの形に包む）
/// 3. ConPTY へ渡す Windows のコマンドライン組み立て
///
/// base64 の出力文字は `A-Za-z0-9+/=` だけで空白も引用符も含まないので、
/// **どの層でも一切書き換えられずに通過する**（`psmux::inner_command` 側にも
/// この前提を固定する回帰テストがある）。スペース・日本語・引用符入りのコマンドが
/// 壊れないことを、場当たりのエスケープではなく符号化で担保する
#[cfg_attr(not(windows), allow(dead_code))]
fn powershell_run_pane_command(program: &str, command: &str, marker_prefix: &str) -> SpawnCommand {
    SpawnCommand {
        program: program.to_string(),
        args: vec![
            "-NoLogo".to_string(),
            "-EncodedCommand".to_string(),
            encode_powershell_command(&powershell_run_script(command, marker_prefix)),
        ],
    }
}

/// コマンド本体を走らせて**終了コードを `$__tako_code` へ確定させる**までの片（純粋関数）。
///
/// 終了コードの決め方が POSIX の `$?` そのままにならないのが肝:
///
/// - PowerShell の cmdlet は `$LASTEXITCODE` を**設定しない**（ネイティブ exe だけが設定する）。
///   `$LASTEXITCODE` だけを見ると cmdlet の失敗が常に成功扱いになる
/// - 逆に `$?` は「直前が成功したか」の真偽値しか持たないので、
///   ネイティブ exe の**実際の終了コード**が取れない
///
/// そこで `$?` を先に見て（false のときだけ）`$LASTEXITCODE` を採る。この順なら
/// 混在した場合まで POSIX の `;` と一致する:
///
/// | コマンド | POSIX 相当 | この式 |
/// |---|---|---|
/// | ネイティブ exe が 7 で失敗 | `sh -c 'exit 7'` = 7 | 7 |
/// | cmdlet が失敗（`$LASTEXITCODE` は付かない） | — | 1 |
/// | exe 失敗 → cmdlet 成功 | `sh -c 'false; true'` = 0 | 0 |
/// | exe 成功 → cmdlet 失敗 | `sh -c 'true; false'` = 1 | 1 |
///
/// **確定した後どう伝えるかは呼び出し側**が決める（実行ペインはマーカー行として画面へ
/// 出し、PTY 無しの実行は `exit` で親プロセスへ返す）。規則を 1 実装に閉じてあるので、
/// 「ペインでは正しく判定できるのにゲートでは失敗が成功に見える」形の食い違いが起きない
#[cfg_attr(not(windows), allow(dead_code))]
fn powershell_exit_code_script(command: &str) -> String {
    format!(
        // プロファイルが走らせたネイティブ exe の値が残っていると誤検知するので先に消す
        "$global:LASTEXITCODE = $null\n\
         {command}\n\
         $__tako_ok = $?\n\
         if ($__tako_ok) {{ $__tako_code = 0 }}\n\
         elseif ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {{ $__tako_code = $LASTEXITCODE }}\n\
         else {{ $__tako_code = 1 }}\n"
    )
}

/// 実行ペインで走らせる PowerShell スクリプト（純粋関数）。
///
/// 終了コードの決め方は [`powershell_exit_code_script`] の doc を参照。ここは
/// 確定した値を**マーカー行として画面へ出し、入力待ちで止める**部分だけを足す。
///
/// `-NoProfile` を付けないのは、POSIX 側が `login_shell_command` の `-l` で
/// プロファイルを読んでいるのと揃えるため。conda / nvm が `$PROFILE` で通す PATH が
/// 効かないと「コマンドが見つからない」になる
/// （**PTY 無しの [`powershell_output_script`] は逆に `-NoProfile`**。あちらの
/// POSIX 側は `sh -c` = プロファイルを読まないので、揃える先が違う）
#[cfg_attr(not(windows), allow(dead_code))]
fn powershell_run_script(command: &str, marker_prefix: &str) -> String {
    // マーカーの引用は PowerShell 方言の `quote_arg`（単引用符 + `''` 二重化）へ委ねる
    let marker = ShellDialect::PowerShell.quote_arg(marker_prefix);
    format!(
        "{}Write-Host ({marker} + $__tako_code)\n\
         try {{ $null = [Console]::ReadLine() }} catch {{ }}\n",
        powershell_exit_code_script(command)
    )
}

/// POSIX の「シェル片を PTY 無しで走らせる」argv（純粋関数）。
///
/// **`sh` を素の名前で起こす形は #935 以前の `acceptance_gates` から 1 バイトも変えない**
/// （境界へ寄せたことで macOS の挙動が動いていないことをスナップショットで固定する）。
/// `run_pane_command` 側の `/bin/sh` と違って素の名前なのは従来どおりで、
/// PATH 上の `sh` を使う = ユーザーが差し替えていればそれに従う。
///
/// **Windows でも使う**（`TAKO_935_LEGACY=1` の A/B が #935 前の形を再現するため）
fn posix_output_argv(script: &str) -> SpawnCommand {
    SpawnCommand {
        program: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
    }
}

/// Windows の「シェル片を PTY 無しで走らせる」argv（純粋関数。**macOS 上でもテストできる**）。
///
/// `-EncodedCommand` を使う理由は [`powershell_run_pane_command`] と同じ
/// （引用符を解釈する層をどれも書き換えなしに通す）。器を経由しない経路でも
/// 符号化の実装を 2 つ持たないため同じ出口を通す。
///
/// **`-NoProfile` を付ける**のが実行ペインとの違い。この経路の POSIX 側は
/// `sh -c`（ログインシェルではない = プロファイルを読まない）なので、揃える先が
/// 「ユーザーの対話環境」ではなく「素の非対話シェル」になる。判定用のコマンドを
/// 走らせる経路なので、プロファイルの副作用が混ざらないほうが再現性も高い
/// （実機実測: `cargo` は `-NoProfile` でも `%USERPROFILE%\.cargo\bin` から解決できた）
#[cfg_attr(not(windows), allow(dead_code))]
fn powershell_output_argv(program: &str, script: &str) -> SpawnCommand {
    SpawnCommand {
        program: program.to_string(),
        args: vec![
            "-NoLogo".to_string(),
            "-NoProfile".to_string(),
            "-EncodedCommand".to_string(),
            encode_powershell_command(&powershell_output_script(script)),
        ],
    }
}

/// PTY 無しで走らせる PowerShell スクリプト（純粋関数）。
///
/// 実行ペイン（[`powershell_run_script`]）との違いは 2 つ:
///
/// 1. **確定した終了コードを `exit` で親へ返す**。`-EncodedCommand` の終了コードは
///    `$LASTEXITCODE` を素通しせず、実機実測では `cmd /c exit 7` が親から見て
///    **1** に化けた（`exit 7` を明示すれば 7 で届く）。`exit` を省くと
///    「7 で落ちたコマンド」と「1 で落ちたコマンド」を区別できないうえ、
///    非終端エラーの後に成功コマンドが続くと**失敗が 0 に見える**
/// 2. **出力を UTF-8 で書かせる**。パイプ相手の PowerShell 5.1 は
///    `[Console]::OutputEncoding` の既定（日本語 Windows では CP932）で書くため、
///    ゲートの evidence が `from_utf8_lossy` で置換文字だらけになる
///    （実機実測: `日本語` が `93fa 967b 8cea` = UTF-8 として不正）。
///    設定できない環境（コンソールを持たない等）では黙って既定のままにする。
///    **stdout / stderr の両方**に効き、`cmd` / `cargo` のような**ネイティブの子**の
///    出力も UTF-8 になる（実機実測: 前置きなしの stderr は `8c9f 8fd8` = CP932、
///    前置きありは `e6a49c e8a8bc` = 正しい UTF-8）
/// 3. **進捗レコードを黙らせる**。stderr がリダイレクトされていると PowerShell 5.1 は
///    進捗・情報・エラーの各レコードを **CLIXML でシリアライズして stderr へ書く**。
///    既定では成功しただけで「モジュールを初めて使用するための準備」の進捗レコードが
///    出るので、**どのゲートの evidence にも数百バイトの XML が混ざる**
///    （実機実測: `$ProgressPreference` 未指定で stderr 400 バイト → 指定すると **0 バイト**）
///
/// **既知の限界**: 上の 3 でも消えないのは **cmdlet のエラーレコード**（実機実測:
/// `Get-Item` の失敗が CLIXML 632 バイト）と `Write-Host` の情報レコード（1078 バイト）。
/// 5.1 にこの直列化を止める手段は無い。判定そのものは終了コードで決まるので**合否は
/// 正しい**が、evidence は読みにくくなる。ネイティブコマンド（`cargo` / `git` / `gh`）は
/// 失敗時も stderr が素のテキストなので（実機実測: `cmd /c exit 7` で stderr 0 バイト・
/// exit 7）、ゲートの述語は**ネイティブコマンドで書くのが望ましい**
#[cfg_attr(not(windows), allow(dead_code))]
fn powershell_output_script(command: &str) -> String {
    format!(
        "try {{ [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 }} catch {{ }}\n\
         $ProgressPreference = 'SilentlyContinue'\n\
         {}exit $__tako_code\n",
        powershell_exit_code_script(command)
    )
}

/// [`output_command`] の共通部分（argv を `Command` へ移して副作用の抑止を掛ける）
fn build_output_command(spawn: &SpawnCommand) -> std::process::Command {
    let mut command = std::process::Command::new(&spawn.program);
    command.args(&spawn.args);
    // #586: GUI プロセス（release は GUI サブシステム）から到達するので
    // コンソールウィンドウを出させない。呼び出し側に思い出させず境界で済ませる
    crate::platform::process::no_console_window(&mut command);
    command
}

/// `-EncodedCommand` が要求する UTF-16LE + base64。
///
/// base64 は 20 行で書けるうえ、依存を増やさない判断
/// （グローバル規約「新しいライブラリを無条件で追加しない」）。
///
/// **符号化はここ 1 箇所**。実行ペイン（#875）とセルフテストのシェル片
/// （`ShellDialect::shell_snippet_command`。#903）が同じ実装を通る
///
/// 出力は必ず [`container_safe_script`] を通してから符号化する（#906）。
/// `TAKO_906_NO_PAD=1` で修正前（素の符号化）へ戻せる = 同一バイナリで A/B が取れる
///
/// **`pub`**（#1057）: ユーザー PATH 境界（[`super::user_path`]）が
/// レジストリ操作の PowerShell 片を同じ符号化で渡す。符号化の実装を
/// 2 つ持たないためにここを公開している（新しい判定は増やさない）
pub fn encode_powershell_command(script: &str) -> String {
    let script = if std::env::var_os("TAKO_906_NO_PAD").is_some() {
        std::borrow::Cow::Borrowed(script)
    } else {
        container_safe_script(script)
    };
    base64_encode(&utf16le_bytes(&script))
}

/// UTF-16LE のバイト列（`-EncodedCommand` が要求する形）
fn utf16le_bytes(script: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// 器（psmux）が拒否する形の符号化ペイロードを避ける（純粋関数。#906）。
///
/// **なぜ要るか**: psmux は `-EncodedCommand` のペイロードが **base64 の `==`
/// パディングで終わる**とき、内側コマンドを起こす段で `new-session` ごと
/// 失敗する（実機実測: `psmux: アクセスが拒否されました。(os error 5)` / exit 5）。
/// tako 側には失敗が返らず、器の client が終了して外側 PTY が死ぬだけなので
/// **ペインが無音で消える**（セルフテスト項目 101 が `session=false size=None
/// backend=None` で止まっていた症状そのもの）。
///
/// 実測の要点（`.agent/plans/2026-08-windows-main-merge-wip.md` の #906 の記録）:
///
/// - **同一長で padding だけを変えると判別できる**: base64 長 448 / 544 / 576 の
///   それぞれで `==` は落ち、`=` 1 個・パディング無しは通る
/// - コマンドライン側は無関係（`==` の後ろに別の引数を足しても落ちる）ので、
///   トークンの位置ではなく**ペイロードの内容**が条件
/// - 落ちるのは長さの帯（実測 448〜576）の中だけだが、帯の上端は測り切れていない。
///   `==` を出さない側は 164〜752 の全実測で通ったので**そちらへ寄せる**
///
/// **直し方**: UTF-16 の要素数が 3 の倍数になるよう末尾へ空白を 1 個足す
/// （バイト数 = 要素数 × 2 なので、要素数 ≡ 2 (mod 3) のときだけ足せば
/// バイト数が 3 の倍数 = パディング無しになる）。PowerShell から見て末尾の
/// 空白は何もしないので、スクリプトの意味は変わらない
pub(crate) fn container_safe_script(script: &str) -> std::borrow::Cow<'_, str> {
    // base64 のパディングはバイト数 % 3 で決まる。UTF-16LE はバイト数が要素数の
    // 2 倍なので、要素数 ≡ 2 (mod 3) ⟺ バイト数 ≡ 1 (mod 3) ⟺ `==` の 2 個
    if script.encode_utf16().count() % 3 == 2 {
        std::borrow::Cow::Owned(format!("{script} "))
    } else {
        std::borrow::Cow::Borrowed(script)
    }
}

/// [`encode_powershell_command`] の逆（**テストの検算用**）。
///
/// 符号化した文字列が元のスクリプトへ戻ることを、実行ペイン（#875）と
/// セルフテストのシェル片（#903）の両方のテストから同じ 1 実装で確かめる
#[cfg(test)]
pub(crate) fn decode_powershell_command(arg: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = Vec::new();
    for c in arg.bytes().filter(|&b| b != b'=') {
        let v = TABLE.iter().position(|&t| t == c).expect("base64 の文字") as u32;
        bits.push(v);
    }
    let mut bytes = Vec::new();
    for chunk in bits.chunks(4) {
        let mut n = 0u32;
        for (i, v) in chunk.iter().enumerate() {
            n |= v << (18 - 6 * i);
        }
        bytes.push((n >> 16) as u8);
        if chunk.len() > 2 {
            bytes.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            bytes.push(n as u8);
        }
    }
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
        .collect();
    String::from_utf16(&units).expect("UTF-16LE")
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18 & 0x3f) as usize] as char);
        out.push(TABLE[(n >> 12 & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6 & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Windows の既定シェル解決（純粋関数。**macOS 上でもテストできる**ようにしてある）。
///
/// 優先順は「新しい PowerShell → 同梱の Windows PowerShell → `%ComSpec%` → `cmd.exe`」。
/// PowerShell を優先するのは、Windows の既定ターミナル体験に合わせるため
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_windows_shell(
    program_files: Option<String>,
    system_root: Option<String>,
    com_spec: Option<String>,
    exists: &dyn Fn(&str) -> bool,
) -> String {
    // PowerShell 7 以降（別途インストール）
    if let Some(pf) = program_files.as_deref().filter(|s| !s.is_empty()) {
        let pwsh = format!("{pf}\\PowerShell\\7\\pwsh.exe");
        if exists(&pwsh) {
            return pwsh;
        }
    }
    // Windows 同梱の Windows PowerShell 5.1
    if let Some(root) = system_root.as_deref().filter(|s| !s.is_empty()) {
        let ps = format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        if exists(&ps) {
            return ps;
        }
    }
    // 最後の砦。%ComSpec% は通常 cmd.exe を指す
    com_spec
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "cmd.exe".into())
}

#[cfg(test)]
mod tests_1031 {
    use super::*;
    use crate::i18n::Lang;

    fn cmd(words: &[&str]) -> SpawnCommand {
        let mut it = words.iter();
        SpawnCommand {
            program: it.next().expect("program").to_string(),
            args: it.map(|w| w.to_string()).collect(),
        }
    }

    /// #1031 の中身: ラッパーが `-i` を持つと zsh は `.zshrc` も読む。
    /// **フラグの並びも固定する**（`-c` の直後がスクリプト本体でないと動かない）
    #[test]
    fn ログインシェルのラッパーは対話フラグを持つ() {
        let got = posix_login_shell_command("/bin/zsh", "npm --version", true);
        assert_eq!(got.program, "/bin/zsh");
        assert_eq!(got.args, vec!["-l", "-i", "-c", "npm --version"]);
    }

    /// **配線**の検査（純粋関数だけを見ていると `imp` 側で `-i` を落としても気づけない）。
    /// `login_shell_command` は `spawn_session` が実際に通す 1 実装なので、ここが
    /// #1031 前へ戻ると実行ペイン 3 経路の環境が丸ごと変わる
    #[cfg(unix)]
    #[test]
    fn 配線されたログインシェルのラッパーが対話フラグを持つ() {
        assert!(
            std::env::var_os("TAKO_1031_LEGACY").is_none(),
            "A/B の env が立ったままではこの検査に意味が無い"
        );
        let got = login_shell_command(cmd(&["/bin/echo", "hi"]));
        assert_eq!(
            got.args
                .iter()
                .take(3)
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["-l", "-i", "-c"],
            "args={:?}",
            got.args
        );
    }

    /// **配線**の検査（`hold_on_failure_command` の既定は「包む」）
    #[test]
    fn 配線された失敗時の保持が既定で有効() {
        assert!(std::env::var_os("TAKO_1031_LEGACY").is_none());
        let got = hold_on_failure_command(cmd(&["npm", "run", "dev"]), "__TAKO_EXIT=");
        assert_ne!(got.program, "npm", "包まれていない: {got:?}");
        let script = got.args.last().expect("スクリプトが要る");
        assert!(script.contains("__TAKO_EXIT="), "script={script}");
    }

    /// `TAKO_1031_LEGACY=1` 相当。#1031 前は `-l -c`（非対話）= `.zshrc` を読まない
    #[test]
    fn legacy指定ではラッパーが非対話へ戻る() {
        let got = posix_login_shell_command("/bin/zsh", "npm --version", false);
        assert_eq!(got.args, vec!["-l", "-c", "npm --version"]);
        assert!(
            !got.args.iter().any(|a| a == "-i"),
            "legacy に -i が混ざっている: {:?}",
            got.args
        );
    }

    /// 成功時は従来どおり即終了（ペインが閉じる）、失敗時だけマーカー + 案内 + 待ち
    #[test]
    fn 失敗時だけ止めるposix片の構造() {
        let got = posix_hold_on_failure_command(
            &cmd(&["npm", "run", "dev"]),
            "__TAKO_EXIT=",
            "[tako] failed",
        );
        assert_eq!(got.program, "/bin/sh");
        assert_eq!(got.args[0], "-c");
        let script = &got.args[1];
        // 本体はそのまま先頭に居る（余計な包みを増やさない）
        assert!(script.starts_with("npm run dev; "), "script={script}");
        // 判定は非 0 のときだけ
        assert!(
            script.contains(r#"if [ "$__tako_code" -ne 0 ]; then"#),
            "script={script}"
        );
        // 実行ペインと同じマーカー行
        assert!(
            script.contains(r#"echo "__TAKO_EXIT=$__tako_code""#),
            "script={script}"
        );
        assert!(
            script.contains(r#"echo "[tako] failed""#),
            "script={script}"
        );
        // 待つのは失敗したときだけ（`fi` の前）
        let read_at = script.find("read -r __TAKO_HOLD__").expect("待ちが要る");
        let fi_at = script.rfind("fi;").expect("fi が要る");
        assert!(read_at < fi_at, "待ちが if の外にある: {script}");
        // 終了コードは元のまま返す（呼び出し元のラッパーから見た値を変えない）
        assert!(
            script.trim_end().ends_with(r#"exit "$__tako_code""#),
            "script={script}"
        );
    }

    /// 空白・日本語・引用符を含む語が 1 語のまま届く（#884 と同じ不変条件）
    #[test]
    fn 失敗時に止める包みは語を割らない() {
        let got = posix_hold_on_failure_command(
            &cmd(&["/bin/echo", "a b", "検証", "it's"]),
            "__TAKO_EXIT=",
            "[tako] failed",
        );
        let script = &got.args[1];
        assert!(
            script.starts_with("/bin/echo 'a b' '検証' 'it'"),
            "script={script}"
        );
        // 実際に sh で走らせて語の数を確かめる（POSIX 上でだけ）
        #[cfg(unix)]
        {
            let out = std::process::Command::new(&got.program)
                .args(&got.args)
                .output()
                .expect("sh が走る");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim(),
                "a b 検証 it's",
                "stderr={}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    /// 成功したコマンドは待たずに終わる（= ペインが従来どおり閉じる）
    #[cfg(unix)]
    #[test]
    fn 成功したコマンドは待たずに終わる() {
        let got = posix_hold_on_failure_command(&cmd(&["true"]), "__TAKO_EXIT=", "[tako] failed");
        let out = std::process::Command::new(&got.program)
            .args(&got.args)
            .output()
            .expect("sh が走る");
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");
    }

    /// 失敗したコマンドはマーカーと案内を出し、終了コードを保つ
    /// （stdin が閉じているので `read` は即戻る = テストがハングしない）
    #[cfg(unix)]
    #[test]
    fn 失敗したコマンドは終了コードと案内を出す() {
        let got = posix_hold_on_failure_command(
            &cmd(&["sh", "-c", "exit 7"]),
            "__TAKO_EXIT=",
            "[tako] failed",
        );
        let out = std::process::Command::new(&got.program)
            .args(&got.args)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("sh が走る");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("__TAKO_EXIT=7"), "stdout={stdout}");
        assert!(stdout.contains("[tako] failed"), "stdout={stdout}");
        assert_eq!(out.status.code(), Some(7));
    }

    /// Windows 側（**macOS 上で検査する**）: `&` で argv を起こし、
    /// 失敗したときだけマーカー + 案内 + 待ちを出す
    #[test]
    fn 失敗時だけ止めるpowershell片の構造() {
        let got = powershell_hold_on_failure_command(
            "pwsh.exe",
            &cmd(&["npm", "run dev", "it's"]),
            "__TAKO_EXIT=",
            "[tako] failed",
        );
        assert_eq!(got.program, "pwsh.exe");
        assert_eq!(got.args[0], "-NoLogo");
        assert_eq!(got.args[1], "-EncodedCommand");
        let script = decode_powershell_command(&got.args[2]);
        // 呼び出し演算子 + 語ごとの引用（空白と `'` が壊れない）
        assert!(
            script.contains("& 'npm' 'run dev' 'it''s'"),
            "script={script}"
        );
        // 終了コードの決め方は 1 実装（`powershell_exit_code_script`）に任せている
        assert!(script.contains("$__tako_code"), "script={script}");
        assert!(
            script.contains("if ($__tako_code -ne 0) {"),
            "script={script}"
        );
        assert!(
            script.contains("Write-Host ('__TAKO_EXIT=' + $__tako_code)"),
            "script={script}"
        );
        assert!(
            script.contains("Write-Host '[tako] failed'"),
            "script={script}"
        );
        assert!(script.contains("[Console]::ReadLine()"), "script={script}");
        // 親へ返す終了コードは元のまま（`-EncodedCommand` は素通ししない。#935 の実測）
        assert!(
            script.trim_end().ends_with("exit $__tako_code"),
            "script={script}"
        );
        // プロファイルは読む（実行ペイン `run_pane_command` と環境を揃える）
        assert!(
            !got.args.iter().any(|a| a == "-NoProfile"),
            "args={:?}",
            got.args
        );
    }

    /// 案内は日英とも用意し、**シェルへ直に埋め込める文字だけ**で書く
    #[test]
    fn 失敗時の案内は日英ともシェル安全() {
        let ja = hold_hint(Lang::Ja);
        let en = hold_hint(Lang::En);
        assert_ne!(ja, en);
        for (name, hint) in [("ja", ja), ("en", en)] {
            assert!(hint.contains("tako"), "{name}: 発信元が分からない");
            for bad in ['"', '$', '`', '\\'] {
                assert!(
                    !hint.contains(bad),
                    "{name}: シェルが解釈する文字が入っている: {bad:?}"
                );
            }
        }
        // Enter で閉じられることを両言語で伝える
        assert!(ja.contains("Enter"), "ja={ja}");
        assert!(en.contains("Enter"), "en={en}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pf() -> Option<String> {
        Some("C:\\Program Files".into())
    }
    fn sr() -> Option<String> {
        Some("C:\\Windows".into())
    }

    #[test]
    fn pwsh7があればそれを最優先する() {
        let got = resolve_windows_shell(
            pf(),
            sr(),
            Some("C:\\Windows\\system32\\cmd.exe".into()),
            &|_p| true,
        );
        assert_eq!(got, "C:\\Program Files\\PowerShell\\7\\pwsh.exe");
    }

    #[test]
    fn pwsh7が無ければ同梱のpowershellへ落ちる() {
        let got = resolve_windows_shell(pf(), sr(), None, &|p| !p.contains("PowerShell\\7"));
        assert_eq!(
            got,
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        );
    }

    #[test]
    fn powershellが無ければcomspecを使う() {
        let got = resolve_windows_shell(
            pf(),
            sr(),
            Some("C:\\Windows\\system32\\cmd.exe".into()),
            &|_p| false,
        );
        assert_eq!(got, "C:\\Windows\\system32\\cmd.exe");
    }

    #[test]
    fn 環境変数が空でもcmdへ落ちる() {
        // サービス起動などで環境が痩せていても spawn 不能（None）にはしない
        let got =
            resolve_windows_shell(Some(String::new()), None, Some(String::new()), &|_p| false);
        assert_eq!(got, "cmd.exe");
    }

    // --- 実行ペイン: POSIX 側の不変（#875 で Windows 対応を入れても 1 バイトも変えない） ---

    /// #875 以前に `dispatch::spawn_command_pane` が直書きしていた文字列そのもの。
    /// **これが変わると macOS の実行ペインの挙動が変わる**ので、リテラルで固定する
    const POSIX_EXPECTED: &str =
        "npm test; echo \"__TAKO_EXIT=$?\"; read -r __TAKO_DUMMY__ 2>/dev/null || true";

    #[test]
    fn posixの実行ペインは従来の直書きとバイト一致する() {
        let got = posix_run_pane_command("npm test", "__TAKO_EXIT=");
        assert_eq!(got.program, "/bin/sh");
        assert_eq!(got.args, vec!["-c".to_string(), POSIX_EXPECTED.to_string()]);
    }

    #[test]
    fn posixのtako_shell宣言は従来どおり単引用符で包む() {
        assert_eq!(
            declared_shell_command("bash", "echo hi"),
            "bash -c 'echo hi'"
        );
        // 単引用符は `'\''` で閉じ直す（POSIX 形）
        assert_eq!(
            declared_shell_command("zsh", "echo it's"),
            r"zsh -c 'echo it'\''s'"
        );
        // 判定できないシェル（fish / cmd.exe）も POSIX 形のまま = 従来どおり
        assert_eq!(
            declared_shell_command("fish", "echo hi"),
            "fish -c 'echo hi'"
        );
        assert_eq!(
            declared_shell_command("cmd.exe", "echo hi"),
            "cmd.exe -c 'echo hi'"
        );
    }

    // --- 実行ペイン: Windows 側 ---

    const PWSH: &str = "C:\\Program Files\\PowerShell\\7\\pwsh.exe";

    use super::decode_powershell_command as decode;

    #[test]
    fn base64は既知のベクタと一致する() {
        // RFC 4648 のテストベクタ（パディングの 3 分岐をすべて通す）
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn windowsの実行ペインはpowershellをencodedcommandで起こす() {
        let got = powershell_run_pane_command(PWSH, "cargo test", "__TAKO_EXIT=");
        assert_eq!(got.program, PWSH);
        assert_eq!(got.args[0], "-NoLogo");
        assert_eq!(got.args[1], "-EncodedCommand");

        let script = decode(&got.args[2]);
        assert!(script.contains("cargo test"), "{script}");
        // 終了コードの解決とマーカー出力・入力待ちが揃っている
        assert!(script.contains("$__tako_ok = $?"), "{script}");
        assert!(script.contains("$LASTEXITCODE"), "{script}");
        assert!(
            script.contains("Write-Host ('__TAKO_EXIT=' + $__tako_code)"),
            "{script}"
        );
        assert!(script.contains("[Console]::ReadLine()"), "{script}");
    }

    #[test]
    fn windowsの実行ペインはプロファイルを読ませる() {
        // POSIX 側は `login_shell_command` の `-l` でログインプロファイルを読む。
        // Windows で `-NoProfile` を付けると conda / nvm 等の PATH 設定が効かなくなり
        // 「コマンドが見つからない」になるので、既定シェルの起動（引数なし）と揃える
        let got = powershell_run_pane_command(PWSH, "conda run python x.py", "__TAKO_EXIT=");
        assert!(
            !got.args
                .iter()
                .any(|a| a.eq_ignore_ascii_case("-noprofile")),
            "{:?}",
            got.args
        );
    }

    // --- PTY 無しの実行（受け入れゲートのコマンド型述語。#935）---

    /// **macOS の挙動を 1 バイトも変えていない**ことの固定（#935 の受け入れ条件）。
    ///
    /// 境界へ寄せる前の `acceptance_gates::execute_command` は
    /// `Command::new("sh").args(["-c", cmd])` だった。ここが変わると
    /// 「Windows を直したついでに macOS のゲートの意味が変わった」ことになる
    #[test]
    fn posixのpty無し実行はshマイナスcのまま() {
        let got = posix_output_argv("cargo test --workspace && git diff --quiet");
        assert_eq!(got.program, "sh");
        assert_eq!(
            got.args,
            vec!["-c", "cargo test --workspace && git diff --quiet"]
        );

        // シェル構文・引用符・非 ASCII をそのまま 1 引数で渡す（加工しない）
        for script in ["echo it's", "echo \"a b\"; false", "echo 検証", ""] {
            let got = posix_output_argv(script);
            assert_eq!(got.program, "sh");
            assert_eq!(got.args, vec!["-c", script], "script={script:?}");
        }
    }

    #[test]
    fn windowsのpty無し実行はpowershellをencodedcommandで起こす() {
        let got = powershell_output_argv(PWSH, "cargo test --workspace");
        assert_eq!(got.program, PWSH);
        assert_eq!(got.args[0], "-NoLogo");
        assert_eq!(got.args[1], "-NoProfile");
        assert_eq!(got.args[2], "-EncodedCommand");

        let script = decode(&got.args[3]);
        assert!(script.contains("cargo test --workspace"), "{script}");
        // 終了コードの確定と、それを親へ返す `exit` が揃っている
        assert!(script.contains("$__tako_ok = $?"), "{script}");
        assert!(script.contains("$LASTEXITCODE"), "{script}");
        assert!(script.contains("exit $__tako_code"), "{script}");
        // 出力を UTF-8 で書かせる（CP932 だと evidence が置換文字へ潰れる）
        assert!(
            script.contains("[Console]::OutputEncoding = [System.Text.Encoding]::UTF8"),
            "{script}"
        );
        // 進捗レコードを黙らせる（既定では成功しただけで stderr へ CLIXML が出る）
        assert!(
            script.contains("$ProgressPreference = 'SilentlyContinue'"),
            "{script}"
        );
        // ペイン用の要素（マーカー行・入力待ち）は入らない
        assert!(!script.contains("ReadLine"), "{script}");
        assert!(!script.contains("Write-Host ("), "{script}");
    }

    /// **実行ペインとは逆に `-NoProfile` を付ける**（揃える先が違う）。
    ///
    /// あちらの POSIX 側は `login_shell_command` の `-l`（ログインプロファイルを読む）だが、
    /// こちらの POSIX 側は `sh -c`（読まない）。判定用のコマンドを走らせる経路なので
    /// プロファイルの副作用を混ぜないほうが再現性も高い
    #[test]
    fn windowsのpty無し実行はプロファイルを読まない() {
        let got = powershell_output_argv(PWSH, "cargo test");
        assert!(
            got.args
                .iter()
                .any(|a| a.eq_ignore_ascii_case("-noprofile")),
            "{:?}",
            got.args
        );
        // POSIX 側もログインシェルではない（対称性の明示）
        assert!(!posix_output_argv("cargo test").args.contains(&"-l".into()));
    }

    /// 終了コードの決め方は**実行ペインと同じ 1 実装**（#935）。
    ///
    /// 別々に書くと「ペインでは失敗が見えるのにゲートでは成功に見える」形の
    /// 食い違いが起きる。片方だけ直す変更をここで落とす
    #[test]
    fn 終了コードの決め方はペインとpty無しで共有する() {
        let shared = powershell_exit_code_script("cargo test");
        assert!(
            powershell_run_script("cargo test", "__TAKO_EXIT=").starts_with(&shared),
            "実行ペインが共有の片から始まっていない"
        );
        assert!(
            powershell_output_script("cargo test").contains(&shared),
            "PTY 無しの実行が共有の片を含んでいない"
        );
    }

    #[test]
    fn pty無し実行のencodedcommandも空白も引用符も含まない() {
        for command in [
            "echo \"hello world\"",
            "cargo test --workspace && git diff --quiet",
            "echo 検証テスト",
            "echo it's",
        ] {
            let encoded = &powershell_output_argv(PWSH, command).args[3];
            assert!(
                encoded
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)),
                "base64 の外の文字が混ざった: {encoded}"
            );
            // #906: 器を経由しない経路でも符号化の出口は 1 つなので `==` は出ない
            assert!(!encoded.ends_with("=="), "{encoded}");
            assert!(decode(encoded).contains(command), "{command}");
        }
    }

    /// 起こすシェルと [`script_dialect`] の答えが食い違わない（**両プラットフォームで走る**）。
    ///
    /// 述語を書く側は `script_dialect()` を見て方言を決めるので、ここがずれると
    /// 「PowerShell 用に書いた述語が POSIX シェルへ渡る」形の取り違えになる
    #[test]
    fn pty無し実行のシェルは方言の申告と一致する() {
        let command = output_command("echo probe");
        let program = command.get_program().to_string_lossy().to_string();
        assert_eq!(
            ShellDialect::from_program(&program),
            Some(script_dialect()),
            "program={program}"
        );
    }

    #[test]
    fn encodedcommandは空白も引用符も含まない() {
        // psmux の `inner_command` → `quote_for_shell` → ConPTY のコマンドライン組み立ての
        // 3 層をどれも書き換えなしに通ることの担保。ここが崩れると引用符入りコマンドが壊れる
        for command in [
            "echo \"hello world\"",
            "python 'my script.py'",
            "echo 日本語のテスト",
            "C:\\Program Files\\x\\y.exe --flag=\"a b\"",
            "echo it's",
            "a && b || c; d | e > f",
        ] {
            let got = powershell_run_pane_command(PWSH, command, "__TAKO_EXIT=");
            let encoded = &got.args[2];
            assert!(
                encoded
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)),
                "base64 の外の文字が混ざった: {encoded}"
            );
            // 復号したら元のコマンドがそのまま入っている（エスケープで壊れていない）
            assert!(decode(encoded).contains(command), "{command}");
        }
    }

    /// #906: 器（psmux）は `==` で終わる符号化ペイロードを拒否する（実機実測）。
    /// **符号化の出口でそれを作らない**ことを、長さを 1 文字ずつ動かして総当たりで固定する
    #[test]
    fn encodedcommandは末尾が二重パディングにならない() {
        // 素の符号化なら 3 文字ごとに `==` が現れる長さの帯を必ず含む範囲
        for n in 0..120 {
            let script = format!("Write-Host {}", "x".repeat(n));
            let encoded = encode_powershell_command(&script);
            assert!(
                !encoded.ends_with("=="),
                "n={n} で `==` パディングが出た: {encoded}"
            );
            // 意味は変わらない（末尾に空白が 1 個増えるだけ）
            let back = decode(&encoded);
            assert!(
                back == script || back == format!("{script} "),
                "n={n} で元へ戻らない: {back:?}"
            );
        }
    }

    /// 修正前は 3 文字ごとに `==` が出ていた = 直したことの検出力（同じ入力で before/after が違う）
    #[test]
    fn 素の符号化では二重パディングが出る長さがある() {
        let mut padded = 0;
        for n in 0..120 {
            let script = format!("Write-Host {}", "x".repeat(n));
            if base64_encode(&utf16le_bytes(&script)).ends_with("==") {
                padded += 1;
            }
        }
        assert_eq!(padded, 40, "3 文字ごとに `==` になる前提が崩れた");
    }

    /// 足すのは「要素数 ≡ 2 (mod 3)」のときだけ（余計な空白を付けない）
    #[test]
    fn container_safe_scriptは必要なときだけ空白を足す() {
        assert_eq!(container_safe_script("ab"), "ab ", "2 要素 = 足す");
        assert_eq!(container_safe_script("abc"), "abc", "3 要素 = 足さない");
        assert_eq!(container_safe_script("abcd"), "abcd", "4 要素 = 足さない");
        assert_eq!(container_safe_script("abcde"), "abcde ", "5 要素 = 足す");
        // 非 ASCII は UTF-16 の**要素数**で数える（バイト数ではない）
        assert_eq!(container_safe_script("箱箱"), "箱箱 ");
        assert_eq!(container_safe_script("箱箱箱"), "箱箱箱");
    }

    /// #906 の実機で落ちていたペイロードそのもの（セルフテスト項目 101 の疑似 TUI）。
    /// **この文字列は実機で `new-session` が exit 5 で拒否した形**なので、
    /// 二重パディングにならないことをここで固定しておく
    #[test]
    fn セルフテスト項目101の疑似tuiは拒否される形にならない() {
        let body = format!(
            "{} Auto  5h 12%   ctx 55% ....  110K/200K\n",
            "\n".repeat(60)
        );
        let script = ShellDialect::PowerShell.paint_and_hold(&body, 3600);
        // 修正前は `==` だった（帯 448〜576 の内側 = 実機で拒否された）
        let bare = base64_encode(&utf16le_bytes(&script));
        assert!(
            bare.ends_with("=="),
            "前提が変わった: {}",
            &bare[bare.len() - 8..]
        );
        assert!(
            (448..=576).contains(&bare.len()),
            "帯の外へ出た: {}",
            bare.len()
        );
        // 修正後は `==` で終わらない
        let fixed = encode_powershell_command(&script);
        assert!(!fixed.ends_with("=="), "{}", &fixed[fixed.len() - 8..]);
    }

    #[test]
    fn マーカーの単引用符はpowershell流に二重化する() {
        // マーカーは定数だが、変えたときに壊れない書き方であることを固定しておく
        let got = powershell_run_pane_command(PWSH, "x", "IT'S=");
        let script = decode(&got.args[2]);
        assert!(
            script.contains("Write-Host ('IT''S=' + $__tako_code)"),
            "{script}"
        );
    }

    #[test]
    fn windowsの宣言シェルはpowershell流に包む() {
        assert_eq!(
            declared_shell_command("pwsh", "echo it's"),
            "pwsh -Command 'echo it''s'"
        );
        // パス表記・大文字表記でも PowerShell と判定される（`ShellDialect` の判定を共有）
        assert_eq!(
            declared_shell_command(PWSH, "echo hi"),
            format!("{PWSH} -Command 'echo hi'")
        );
        assert_eq!(
            declared_shell_command("PowerShell.EXE", "echo hi"),
            "PowerShell.EXE -Command 'echo hi'"
        );
    }

    /// `TerminalSession::spawn` が argv の組み直しを境界へ委ねていること（番犬。#884）。
    ///
    /// `escape_args` は `#[cfg(target_os = "windows")]` なので **macOS からは
    /// フィールドごと見えない** = 分岐の中身をユニットテストで踏めない。
    /// せめて「境界を通っていること」だけは macOS の `cargo test` で固定しておく
    /// （実挙動の網は Windows 専用の `tests/spawn_arg_quoting.rs`）
    #[test]
    fn spawnはargvの組み直しを境界へ委ねる() {
        let src = include_str!("../terminal.rs");
        let body = src
            .split("let mut tty_options = tty::Options {")
            .nth(1)
            .expect("spawn の tty::Options 組み立て");
        let body = &body[..body.find("tty::new(").expect("tty::new の呼び出し")];
        assert!(
            body.contains("apply_arg_escaping(&mut tty_options)"),
            "tty::new へ渡す前に apply_arg_escaping を通していない\
             （Windows で空白入りの語が割れてペインが即死する。#884）"
        );
    }
}
