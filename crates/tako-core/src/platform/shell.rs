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

/// 明示コマンド（`tako split -- <command>` 等）を、ユーザーの環境で実行される形に包む
pub fn login_shell_command(command: SpawnCommand) -> SpawnCommand {
    imp::login_shell_command(command)
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
        SpawnCommand {
            program: user_shell(),
            args: vec![
                "-l".into(),
                "-c".into(),
                crate::tmux_backend::shell_quoted(&command),
            ],
        }
    }

    pub(crate) fn run_pane_command(command: &str, marker_prefix: &str) -> SpawnCommand {
        super::posix_run_pane_command(command, marker_prefix)
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

    pub(crate) fn run_pane_command(command: &str, marker_prefix: &str) -> SpawnCommand {
        super::powershell_run_pane_command(&run_pane_shell(), command, marker_prefix)
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

/// 実行ペインで走らせる PowerShell スクリプト（純粋関数）。
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
/// `-NoProfile` を付けないのは、POSIX 側が `login_shell_command` の `-l` で
/// プロファイルを読んでいるのと揃えるため。conda / nvm が `$PROFILE` で通す PATH が
/// 効かないと「コマンドが見つからない」になる
#[cfg_attr(not(windows), allow(dead_code))]
fn powershell_run_script(command: &str, marker_prefix: &str) -> String {
    // マーカーの引用は PowerShell 方言の `quote_arg`（単引用符 + `''` 二重化）へ委ねる
    let marker = ShellDialect::PowerShell.quote_arg(marker_prefix);
    format!(
        // プロファイルが走らせたネイティブ exe の値が残っていると誤検知するので先に消す
        "$global:LASTEXITCODE = $null\n\
         {command}\n\
         $__tako_ok = $?\n\
         if ($__tako_ok) {{ $__tako_code = 0 }}\n\
         elseif ($null -ne $LASTEXITCODE -and $LASTEXITCODE -ne 0) {{ $__tako_code = $LASTEXITCODE }}\n\
         else {{ $__tako_code = 1 }}\n\
         Write-Host ({marker} + $__tako_code)\n\
         try {{ $null = [Console]::ReadLine() }} catch {{ }}\n"
    )
}

/// `-EncodedCommand` が要求する UTF-16LE + base64。
///
/// base64 は 20 行で書けるうえ、依存を増やさない判断
/// （グローバル規約「新しいライブラリを無条件で追加しない」）。
///
/// **符号化はここ 1 箇所**。実行ペイン（#875）とセルフテストのシェル片
/// （`ShellDialect::shell_snippet_command`。#903）が同じ実装を通る
pub(crate) fn encode_powershell_command(script: &str) -> String {
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    base64_encode(&bytes)
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
