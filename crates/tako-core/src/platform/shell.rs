//! 既定シェルの解決（抽象境界 B1）
//!
//! 「ペインで何を起動するか」「明示コマンドをどう包むか」のプラットフォーム差を閉じ込める。
//! PTY そのもの（unix pty / ConPTY）は `alacritty_terminal::tty` が吸収するため、
//! ここでは扱わない（設計 §2.2）。

use crate::terminal::SpawnCommand;

/// 既定シェル。`None` を返すとペインを spawn できない
pub(crate) fn default_shell() -> Option<SpawnCommand> {
    imp::default_shell()
}

/// 明示コマンド（`tako split -- <command>` 等）を、ユーザーの環境で実行される形に包む
pub fn login_shell_command(command: SpawnCommand) -> SpawnCommand {
    imp::login_shell_command(command)
}

/// 実行ペイン（Code Runner `tako run` / `tako run-interactive`）を起こすコマンド。
///
/// 組み立てる形は「コマンド本体 → 終了コードを `<marker_prefix><code>` の 1 行で出力 →
/// 入力待ちで停止」。入力待ちで止めるのは、即終了するコマンドでも出力を読めるように
/// ペインを残すため。`RunInteractiveStatus` はこのマーカー行を画面から拾って
/// 終了コードと auto_close を決めるので、**マーカーの出力が唯一の契約**（`find_exit_marker`）
pub fn run_pane_command(command: &str, marker_prefix: &str) -> SpawnCommand {
    imp::run_pane_command(command, marker_prefix)
}

/// `tako:shell` 宣言で指定されたシェルへコマンドを包む（`tako run` の解決結果に使う）
pub fn declared_shell_command(shell: &str, command: &str) -> String {
    imp::declared_shell_command(shell, command)
}

#[cfg(unix)]
mod imp {
    use super::*;

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
        super::build_posix_run_pane_command(command, marker_prefix)
    }

    pub(crate) fn declared_shell_command(shell: &str, command: &str) -> String {
        super::declared_shell_command_for_posix(shell, command)
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
    /// ここではなく `run_pane_command` が PowerShell へ包む（#525）
    pub(crate) fn login_shell_command(command: SpawnCommand) -> SpawnCommand {
        command
    }

    pub(crate) fn run_pane_command(command: &str, marker_prefix: &str) -> SpawnCommand {
        super::build_windows_run_pane_command(&current_windows_shell(), command, marker_prefix)
    }

    pub(crate) fn declared_shell_command(shell: &str, command: &str) -> String {
        super::declared_shell_command_for_windows(shell, command)
    }

    fn current_windows_shell() -> super::WindowsShell {
        super::resolve_windows_shell_kind(
            env_string("ProgramFiles"),
            env_string("SystemRoot"),
            env_string("ComSpec"),
            &|p| std::path::Path::new(p).exists(),
        )
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
/// **cfg の外に出してあるのは Windows 上でも文字列をテストできるようにするため**
/// （Windows 実機で開発しているときに POSIX 側を壊していないことを機械で確かめられる）
#[cfg_attr(windows, allow(dead_code))]
fn build_posix_run_pane_command(command: &str, marker_prefix: &str) -> SpawnCommand {
    let wrapped = format!(
        "{command}; echo \"{marker_prefix}$?\"; read -r __TAKO_DUMMY__ 2>/dev/null || true"
    );
    // 複合シェルコード（`;` / `||` 入り）は program 1 語に詰めず /bin/sh -c の引数で渡す。
    // program に詰めると login_shell_command の shell_quoted が全文を 1 語にクォートし、
    // シェルが「セミコロン込みの 1 コマンド名」として探して 127 で即死する（#453）
    SpawnCommand {
        program: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), wrapped],
    }
}

/// `tako:shell` 宣言の包み方（POSIX）。単引用符で包み、中の `'` は `'\''` で閉じ直す
fn declared_shell_command_for_posix(shell: &str, command: &str) -> String {
    let escaped = command.replace('\'', "'\\''");
    format!("{shell} -c '{escaped}'")
}

/// 解決した Windows の既定シェル。**種類まで持つ**のは、実行ペインの組み立て
/// （終了コードの拾い方・入力待ちの書き方）がシェルの種類ごとに全く違うため
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum WindowsShell {
    /// `pwsh.exe`（PowerShell 7）または `powershell.exe`（Windows PowerShell 5.1）
    PowerShell(String),
    /// `%ComSpec%` / `cmd.exe`。PowerShell がどちらも見つからなかった最後の砦
    Cmd(String),
}

#[cfg_attr(not(windows), allow(dead_code))]
impl WindowsShell {
    fn path(&self) -> &str {
        match self {
            Self::PowerShell(p) | Self::Cmd(p) => p,
        }
    }
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
    resolve_windows_shell_kind(program_files, system_root, com_spec, exists)
        .path()
        .to_string()
}

/// `resolve_windows_shell` と同じ探索を行い、**見つかったシェルの種類まで**返す
#[cfg_attr(not(windows), allow(dead_code))]
fn resolve_windows_shell_kind(
    program_files: Option<String>,
    system_root: Option<String>,
    com_spec: Option<String>,
    exists: &dyn Fn(&str) -> bool,
) -> WindowsShell {
    // PowerShell 7 以降（別途インストール）
    if let Some(pf) = program_files.as_deref().filter(|s| !s.is_empty()) {
        let pwsh = format!("{pf}\\PowerShell\\7\\pwsh.exe");
        if exists(&pwsh) {
            return WindowsShell::PowerShell(pwsh);
        }
    }
    // Windows 同梱の Windows PowerShell 5.1
    if let Some(root) = system_root.as_deref().filter(|s| !s.is_empty()) {
        let ps = format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        if exists(&ps) {
            return WindowsShell::PowerShell(ps);
        }
    }
    // 最後の砦。%ComSpec% は通常 cmd.exe を指す
    WindowsShell::Cmd(
        com_spec
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "cmd.exe".into()),
    )
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
/// **どの層でも一切書き換えられずに通過する**。スペース・日本語・引用符入りの
/// コマンドが壊れないことを、場当たりのエスケープではなく符号化で担保する
#[cfg_attr(not(windows), allow(dead_code))]
fn build_windows_run_pane_command(
    shell: &WindowsShell,
    command: &str,
    marker_prefix: &str,
) -> SpawnCommand {
    match shell {
        WindowsShell::PowerShell(path) => SpawnCommand {
            program: path.clone(),
            args: vec![
                "-NoLogo".to_string(),
                "-EncodedCommand".to_string(),
                encode_powershell_command(&powershell_run_script(command, marker_prefix)),
            ],
        },
        // PowerShell がどちらも無い環境の最後の砦。`/v:on` は遅延展開（`!ERRORLEVEL!`）のため。
        // `%ERRORLEVEL%` だとコマンドライン解析時に展開されてしまい常に 0 になる。
        // **引用符を含むコマンドは cmd の /c 解析を通せない**が、Windows PowerShell 5.1 は
        // OS 同梱なのでこの経路には実質到達しない
        WindowsShell::Cmd(path) => SpawnCommand {
            program: path.clone(),
            args: vec![
                "/v:on".to_string(),
                "/c".to_string(),
                format!("{command}& echo {marker_prefix}!ERRORLEVEL!& pause>nul"),
            ],
        },
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
/// そこで `$?` を先に見て（false のときだけ）`$LASTEXITCODE` を採る。
/// この順なら「ネイティブ exe が失敗 → 実コード」「cmdlet が失敗 → 1」
/// 「exe 成功のあと cmdlet 成功 → 0」がすべて POSIX の `;` と同じ結果になる
#[cfg_attr(not(windows), allow(dead_code))]
fn powershell_run_script(command: &str, marker_prefix: &str) -> String {
    let marker = powershell_single_quoted(marker_prefix);
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

/// PowerShell の単引用符文字列。中の `'` は `''` で表す（POSIX の `'\''` とは別物）
#[cfg_attr(not(windows), allow(dead_code))]
fn powershell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// `-EncodedCommand` が要求する UTF-16LE + base64。
///
/// base64 は 20 行で書けるうえ、この 1 箇所でしか使わない。
/// 依存を増やさない判断（グローバル規約「新しいライブラリを無条件で追加しない」）
#[cfg_attr(not(windows), allow(dead_code))]
fn encode_powershell_command(script: &str) -> String {
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    base64_encode(&bytes)
}

#[cfg_attr(not(windows), allow(dead_code))]
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

/// `tako:shell` 宣言の包み方（Windows）。
///
/// 宣言されたシェルが PowerShell 系かどうかで引用符の書き方が違う。
/// PowerShell / cmd 以外（Git Bash の `bash` 等）は POSIX 形のままにする —
/// Windows でも POSIX シェルを入れて使う人はいるので、**知らないシェルは POSIX 扱い**が既定
#[cfg_attr(not(windows), allow(dead_code))]
fn declared_shell_command_for_windows(shell: &str, command: &str) -> String {
    match declared_shell_family(shell) {
        DeclaredShell::PowerShell => {
            format!("{shell} -Command {}", powershell_single_quoted(command))
        }
        DeclaredShell::Cmd => format!("{shell} /c \"{command}\""),
        DeclaredShell::Posix => declared_shell_command_for_posix(shell, command),
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
enum DeclaredShell {
    PowerShell,
    Cmd,
    Posix,
}

/// 宣言文字列（`pwsh` / `C:\…\powershell.exe` 等）からシェルの系統を見分ける
#[cfg_attr(not(windows), allow(dead_code))]
fn declared_shell_family(shell: &str) -> DeclaredShell {
    // `.EXE` のような大文字表記も拾えるよう、小文字化してから拡張子を落とす
    let base = shell.rsplit(['\\', '/']).next().unwrap_or(shell);
    let base = base.to_ascii_lowercase();
    match base.strip_suffix(".exe").unwrap_or(&base) {
        "pwsh" | "powershell" => DeclaredShell::PowerShell,
        "cmd" => DeclaredShell::Cmd,
        _ => DeclaredShell::Posix,
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

    #[test]
    fn 解決したシェルの種類も返る() {
        assert!(matches!(
            resolve_windows_shell_kind(pf(), sr(), None, &|_p| true),
            WindowsShell::PowerShell(_)
        ));
        assert!(matches!(
            resolve_windows_shell_kind(None, None, None, &|_p| false),
            WindowsShell::Cmd(_)
        ));
    }

    // --- POSIX 側の不変（#525 で Windows 対応を入れても 1 バイトも変えない） ---

    /// #525 以前に `dispatch::spawn_command_pane` が組み立てていた文字列そのもの。
    /// **これが変わると macOS の実行ペインの挙動が変わる**ので、リテラルで固定する
    const POSIX_EXPECTED: &str =
        "npm test; echo \"__TAKO_EXIT=$?\"; read -r __TAKO_DUMMY__ 2>/dev/null || true";

    #[test]
    fn posixの実行ペインは従来の組み立てとバイト一致する() {
        let got = build_posix_run_pane_command("npm test", "__TAKO_EXIT=");
        assert_eq!(got.program, "/bin/sh");
        assert_eq!(got.args, vec!["-c".to_string(), POSIX_EXPECTED.to_string()]);
    }

    #[test]
    fn posixのtako_shell宣言は従来どおり単引用符で包む() {
        assert_eq!(
            declared_shell_command_for_posix("bash", "echo hi"),
            "bash -c 'echo hi'"
        );
        // 単引用符は `'\''` で閉じ直す（POSIX 形）
        assert_eq!(
            declared_shell_command_for_posix("zsh", "echo it's"),
            r"zsh -c 'echo it'\''s'"
        );
    }

    // --- Windows の実行ペイン ---

    fn ps() -> WindowsShell {
        WindowsShell::PowerShell("C:\\Program Files\\PowerShell\\7\\pwsh.exe".into())
    }

    /// `-EncodedCommand` の引数を元のスクリプトへ戻す（テスト用の検算）
    fn decode(arg: &str) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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
        let got = build_windows_run_pane_command(&ps(), "cargo test", "__TAKO_EXIT=");
        assert_eq!(got.program, "C:\\Program Files\\PowerShell\\7\\pwsh.exe");
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
        let got = build_windows_run_pane_command(&ps(), "conda run python x.py", "__TAKO_EXIT=");
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
            let got = build_windows_run_pane_command(&ps(), command, "__TAKO_EXIT=");
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
        assert_eq!(powershell_single_quoted("a'b"), "'a''b'");
        let got = build_windows_run_pane_command(&ps(), "x", "IT'S=");
        assert!(
            decode(&got.args[2]).contains("Write-Host ('IT''S=' + $__tako_code)"),
            "{}",
            decode(&got.args[2])
        );
    }

    #[test]
    fn powershellが無ければcmdの遅延展開で終了コードを拾う() {
        let got = build_windows_run_pane_command(
            &WindowsShell::Cmd("cmd.exe".into()),
            "dir",
            "__TAKO_EXIT=",
        );
        assert_eq!(got.program, "cmd.exe");
        // `%ERRORLEVEL%` は解析時に展開されて常に 0 になるため `!…!` + /v:on が要る
        assert_eq!(got.args[0], "/v:on");
        assert_eq!(got.args[1], "/c");
        assert!(got.args[2].contains("!ERRORLEVEL!"), "{}", got.args[2]);
        assert!(got.args[2].contains("pause>nul"), "{}", got.args[2]);
    }

    // --- tako:shell 宣言（Windows） ---

    #[test]
    fn 宣言シェルの系統をパス表記からも見分ける() {
        assert_eq!(declared_shell_family("pwsh"), DeclaredShell::PowerShell);
        assert_eq!(
            declared_shell_family("C:\\Program Files\\PowerShell\\7\\pwsh.exe"),
            DeclaredShell::PowerShell
        );
        assert_eq!(
            declared_shell_family("PowerShell.EXE"),
            DeclaredShell::PowerShell
        );
        assert_eq!(declared_shell_family("cmd.exe"), DeclaredShell::Cmd);
        // 知らないシェルは POSIX 扱い（Git Bash 等を Windows で使う人がいる）
        assert_eq!(declared_shell_family("bash"), DeclaredShell::Posix);
        assert_eq!(declared_shell_family("/usr/bin/zsh"), DeclaredShell::Posix);
    }

    #[test]
    fn windowsの宣言シェルはpowershell流に包む() {
        assert_eq!(
            declared_shell_command_for_windows("pwsh", "echo it's"),
            "pwsh -Command 'echo it''s'"
        );
        // POSIX シェルの宣言は macOS と同じ組み立てのまま
        assert_eq!(
            declared_shell_command_for_windows("bash", "echo it's"),
            r"bash -c 'echo it'\''s'"
        );
    }
}
