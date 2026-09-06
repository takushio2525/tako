//! ペインのシェルへ**打ち込むコマンド文字列**の方言差を閉じ込める（抽象境界 B1 の一部）
//!
//! ## なぜ境界が要るか（#865）
//!
//! セルフテストは「ペイン内のシェルへ実際にキーを打ち、画面に出た結果を読む」形で
//! 入力経路・環境変数注入・CLI / IPC の往復を検証する。この打ち込む文字列が
//! POSIX 構文の直書きだと、**機能が正常でも Windows では必ず失敗する**:
//!
//! ```text
//! echo TERMCHK=$TERM,$COLORTERM     → PowerShell では TERMCHK=,
//! ```
//!
//! `$TERM` は PowerShell では（未定義の）PowerShell 変数で、環境変数は `$env:TERM`。
//! 実測（#865）では TERM / COLORTERM の注入自体は Windows でも効いているのに
//! この 1 行で FAILED になり、**以降の項目が一切走らない**状態だった。
//!
//! ## 方言は OS ではなく「シェル」で決まる
//!
//! [`for_default_shell`] は [`super::shell::default_shell`] が選んだプログラムから
//! 方言を引く。`cfg(windows)` で分けないのは、Windows でも PowerShell が無ければ
//! `cmd.exe` へ落ちる（= PowerShell 構文も通らない）ためで、判定を
//! **純粋関数 [`ShellDialect::from_program`]** にしてあるので macOS からも全分岐を
//! テストできる。判定できないシェルでは `None` を返し、呼び出し側は
//! 「このシェルはセルフテストの対象外」と明示的に扱う（黙って POSIX を打たない）。

use std::path::Path;

/// ペインのシェルの方言。
///
/// `cmd.exe` / fish のような「どちらでもないシェル」は変換先を持たないため
/// この enum には**入れない**（[`ShellDialect::from_program`] が `None` を返す）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellDialect {
    /// sh / bash / zsh（`$VAR` / `$(( ))` / `&&` / `>/dev/null` が通る）
    Posix,
    /// PowerShell 5.1 と 7 の**両方**で通る書き方だけを出す。
    /// pwsh 7 専用の `&&` 等は使わない（`default_shell()` は 5.1 へ落ちうる）
    PowerShell,
}

/// 既定シェル（ペインで起動するシェル）の方言。判定できなければ `None`
pub fn for_default_shell() -> Option<ShellDialect> {
    let shell = super::shell::default_shell()?;
    ShellDialect::from_program(&shell.program)
}

impl ShellDialect {
    /// シェルのプログラムパスから方言を判定する（純粋関数。**macOS 上でも全分岐テスト可**）。
    ///
    /// 判定は実行ファイル名だけを見る（`/bin/zsh` / `C:\Program Files\PowerShell\7\pwsh.exe`
    /// のどちらでも同じ答えになる）。**未知のシェルは `None`**。
    /// 「たぶん POSIX」で倒すと、fish や cmd.exe で `$((40+2))` を打って
    /// 原因の分かりにくい失敗になる
    pub fn from_program(program: &str) -> Option<Self> {
        let file = program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(program)
            .to_ascii_lowercase();
        let stem = file.strip_suffix(".exe").unwrap_or(&file);
        match stem {
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" => Some(Self::Posix),
            "pwsh" | "powershell" => Some(Self::PowerShell),
            // cmd.exe（`$` も `[` も無い）と fish（`$(( ))` を持たない）は変換先が無い
            _ => None,
        }
    }

    pub fn is_posix(self) -> bool {
        matches!(self, Self::Posix)
    }

    /// 診断・SKIP 理由に出す名前
    pub fn label(self) -> &'static str {
        match self {
            Self::Posix => "posix",
            Self::PowerShell => "powershell",
        }
    }

    /// 入力行をまるごと消すキー（行編集はシェルの行エディタが持つ機能なので方言差がある）。
    ///
    /// **PSReadLine（Windows モード）に Ctrl+U は無い**（実測: pwsh 7 / 5.1 とも
    /// `Get-PSReadLineKeyHandler -Bound` に現れない）。Escape が `RevertLine` で
    /// 行を消すので、PowerShell ではそれを使う。POSIX 側で Escape を使うと
    /// メタプレフィックスになって次の文字が編集コマンドに化けるため、Ctrl+U のまま
    pub fn clear_line_key(self) -> &'static str {
        match self {
            Self::Posix => "ctrl-u",
            Self::PowerShell => "escape",
        }
    }

    /// 環境変数の参照式（二重引用符の中に埋めて使う）
    pub fn env_ref(self, name: &str) -> String {
        match self {
            Self::Posix => format!("${{{name}}}"),
            Self::PowerShell => format!("$env:{name}"),
        }
    }

    /// 1 行出力する。`template` の `${NAME}` は環境変数参照へ展開される。
    ///
    /// **常に二重引用符で包む**。PowerShell の裸の引数はカンマが配列区切りになり
    /// `echo TERMCHK=$env:TERM,$env:COLORTERM` が 2 行に割れる（実測。#865）
    pub fn echo(self, template: &str) -> String {
        format!("echo \"{}\"", self.expand_env_refs(template))
    }

    /// `${NAME}` を方言の環境変数参照へ置き換える（`echo` の外でも使えるように公開）
    pub fn expand_env_refs(self, template: &str) -> String {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(start) = rest.find("${") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            match after.find('}') {
                Some(end) => {
                    out.push_str(&self.env_ref(&after[..end]));
                    rest = &after[end + 1..];
                }
                None => {
                    // 閉じていない `${` はそのまま通す（テンプレートの書き間違いを隠さない）
                    out.push_str("${");
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// 算術式（`$((40+2))` / `$(40+2)`）。
    ///
    /// セルフテストのマーカーは**打ち込んだ行そのものに答えが現れない**必要がある
    /// （画面検索はコマンドのエコー行も読むため）。だから `42` と書かずに足し算にする
    pub fn arith(self, left: i64, right: i64) -> String {
        match self {
            Self::Posix => format!("$(({left}+{right}))"),
            Self::PowerShell => format!("$({left}+{right})"),
        }
    }

    /// `<prefix><left+right>` を出すマーカー式（`echo` へ埋める）
    pub fn marker(self, prefix: &str, left: i64, right: i64) -> String {
        format!("{prefix}{}", self.arith(left, right))
    }

    /// ANSI エスケープをそのまま端末へ書く（改行は付けない）。
    /// `payload` の `\x1b` が ESC になる。
    ///
    /// **payload に生の改行を入れてはいけない**。POSIX 側は単引用符で包むので
    /// 生の改行がそのまま「打ち込むキー列の改行」= 途中実行になる。
    /// 改行が要るときは [`Self::emit_ansi_line`] を使う
    pub fn emit_ansi(self, payload: &str) -> String {
        debug_assert!(
            !payload.contains('\n'),
            "emit_ansi の payload に生の改行を入れないこと（emit_ansi_line を使う）"
        );
        match self {
            Self::Posix => {
                // printf の書式解釈を避けるため `%` は打たない前提（呼び出し側は制御列のみ）
                format!("printf '{}'", payload.replace('\u{1b}', "\\e"))
            }
            Self::PowerShell => {
                // `` `e `` は pwsh 7 専用なので 5.1 でも通る [char]27 を使う
                format!(
                    "Write-Host -NoNewline \"{}\"",
                    payload.replace('\u{1b}', "$([char]27)")
                )
            }
        }
    }

    /// [`Self::emit_ansi`] + 改行 1 個
    pub fn emit_ansi_line(self, payload: &str) -> String {
        match self {
            // 改行は printf の書式（`\n`）として渡す。生の改行を単引用符へ入れない
            Self::Posix => format!("printf '{}\\n'", payload.replace('\u{1b}', "\\e")),
            // Write-Host は既定で改行を付ける
            Self::PowerShell => format!(
                "Write-Host \"{}\"",
                payload.replace('\u{1b}', "$([char]27)")
            ),
        }
    }

    /// 渡した語を 1 行ずつそのまま出力する（展開しない）。
    ///
    /// PowerShell の `echo`（`Write-Output`）は複数引数を 1 行ずつ出すのでそのまま使える。
    /// POSIX は `printf '%s\n'` に並べる（`echo` は引数を空白で 1 行に並べてしまう）
    pub fn print_lines(self, words: &[String]) -> String {
        let quoted: Vec<String> = words.iter().map(|w| self.quote_arg(w)).collect();
        match self {
            Self::Posix => format!("printf '%s\\n' {}", quoted.join(" ")),
            Self::PowerShell => format!("echo {}", quoted.join(" ")),
        }
    }

    /// ファイルの中身をそのまま 1 行ずつ出す（**打ち込んだ行に本文が現れない**）。
    ///
    /// [`print_lines`] は本文を引数に持つので、**シェルがエコーしたコマンド行にも
    /// 本文が載る**。画面を検索する検査ではこれが致命的になる: 項目 137（#1040）は
    /// 「失敗マーカーの 1 つ上の行が理由」という契約を見るのに、エコー行が
    /// **マーカーの最初の出現**になってしまい、理由が直前のプロンプトへずれていた
    /// （#1127 の実測: `reason=Some("TAKO_1040_RETRY")` → 「待てば直る」と誤判定）。
    /// 数値マーカーで同じ罠を避けている [`arith`](Self::arith) の文字列版。
    ///
    /// 中身は**呼び出し側が Rust で書く**（シェルを通さないのでバイトがそのまま届く）。
    /// PowerShell 5.1 の `Get-Content` は既定でその機のコードページ（日本語環境は
    /// CP932）で読むので、**`-Encoding UTF8` を必ず付ける**（実測: 付けないと
    /// `への接続に失敗しました` が `縺ｸ縺ｮ謗･邯壹↓…` に化ける）
    pub fn print_file(self, path: &Path) -> String {
        let quoted = self.quote_arg(&path.to_string_lossy());
        match self {
            Self::Posix => format!("cat {quoted}"),
            Self::PowerShell => format!("Get-Content -Encoding UTF8 -LiteralPath {quoted}"),
        }
    }

    /// `<prefix><0..count-1>` を 1 行ずつ、行間に待ちを入れて出す。
    /// 取り込み経路（PTY → 画面）を「少しずつ流れてくる出力」で試すのに使う
    pub fn emit_numbered_lines(self, prefix: &str, count: u32, delay_ms: u32) -> String {
        match self {
            Self::Posix => format!(
                "i=0; while [ $i -lt {count} ]; do printf '{prefix}%d\\n' $i; \
                 sleep {}.{:03}; i=$((i+1)); done",
                delay_ms / 1000,
                delay_ms % 1000
            ),
            Self::PowerShell => format!(
                "0..{} | ForEach-Object {{ Write-Output \"{prefix}$_\"; \
                 Start-Sleep -Milliseconds {delay_ms} }}",
                count.saturating_sub(1)
            ),
        }
    }

    /// 連番を 1 行 1 個で出す
    pub fn seq(self, from: i64, to: i64) -> String {
        match self {
            Self::Posix => format!("seq {from} {to}"),
            Self::PowerShell => format!("{from}..{to}"),
        }
    }

    /// 指定秒だけ止まる（コマンド実行中状態の検証用）
    pub fn sleep(self, secs: u32) -> String {
        match self {
            Self::Posix => format!("sleep {secs}"),
            Self::PowerShell => format!("Start-Sleep {secs}"),
        }
    }

    /// 指定の終了コードで終わるコマンド（`true` / `false` 相当）。
    /// シェル統合が出す OSC 133 の exit code 検証に使う
    pub fn exit_status(self, code: u8) -> String {
        match self {
            Self::Posix if code == 0 => "true".to_string(),
            Self::Posix => format!("sh -c 'exit {code}'"),
            // PowerShell の `exit` はシェル自体を終わらせるので外部プロセスで返す
            Self::PowerShell => format!("cmd /c exit {code}"),
        }
    }

    /// 現在のディレクトリを**そのまま 1 行**で出力する（#935）。
    ///
    /// PowerShell 側で `pwd` を使わない理由が 2 つある（どちらも実機実測）:
    ///
    /// - `pwd` は `Get-Location` のエイリアスで、返るのは文字列ではなく `PathInfo`
    ///   オブジェクトなので**表として整形され、パスがコンソール幅で切られる**
    ///   （幅の狭い環境で 51 バイトのパスが `C:\Users\...` の 14 文字で途切れた）
    /// - `Write-Host` ではなく `Write-Output` を使うのは、**stderr がリダイレクト
    ///   されていると `Write-Host` の情報レコードが CLIXML で stderr へ出る**ため
    ///   （実機実測: `Write-Output` は stderr 0 バイト / `Write-Host` は 1078 バイト）。
    ///   `$PWD.Path` は `String` なので `Write-Output` でも素の 1 行になる
    pub fn print_cwd(self) -> String {
        match self {
            Self::Posix => "pwd".to_string(),
            Self::PowerShell => "Write-Output $PWD.Path".to_string(),
        }
    }

    /// ディレクトリ移動（両方 `cd` で通るが、意図を境界に持たせておく）
    pub fn cd(self, path: &str) -> String {
        match self {
            Self::Posix => format!("cd {}", crate::shell::quote_for_shell(path)),
            Self::PowerShell => format!("Set-Location {}", ps_quote(path)),
        }
    }

    /// ディレクトリを作ってそこへ移動する（無ければ作る）
    pub fn mkdir_and_cd(self, path: &str) -> String {
        match self {
            Self::Posix => {
                let q = crate::shell::quote_for_shell(path);
                format!("mkdir -p {q} && cd {q}")
            }
            Self::PowerShell => {
                let q = ps_quote(path);
                format!(
                    "New-Item -ItemType Directory -Force -Path {q} | Out-Null; Set-Location {q}"
                )
            }
        }
    }

    /// 一時ディレクトリ（`cd` の行き先に使う。macOS の `/private/tmp` 直書きを避ける）
    pub fn temp_dir_ref(self) -> String {
        match self {
            Self::Posix => "\"${TMPDIR:-/tmp}\"".to_string(),
            Self::PowerShell => "$env:TEMP".to_string(),
        }
    }

    /// フルパスの実行ファイルを起動する形。
    ///
    /// PowerShell は先頭の引用符付き文字列を**式**として評価してしまうため
    /// 呼び出し演算子 `&` が必須（付けないとパスがそのまま表示されるだけで実行されない）
    pub fn program(self, path: &Path) -> String {
        let quoted = format!("\"{}\"", path.display());
        match self {
            Self::Posix => quoted,
            Self::PowerShell => format!("& {quoted}"),
        }
    }

    /// **コマンド位置**に置く 1 語（実行ファイルのパス）。#322 の最簡形に従い
    /// **囲む必要が無いときは囲まない**。
    ///
    /// `program` との違いは「必要なときだけ囲む」ことと、PowerShell 側で
    /// **単引用符（リテラル）**を使うこと。二重引用符だと `$` が展開されるので、
    /// `C:\Users\a$b\tako.exe` のようなパスが壊れる。
    ///
    /// #899: 旧実装は POSIX 前提の 1 本しかなく、安全文字が `[A-Za-z0-9._-/]` だったため
    /// **Windows の絶対パスは `:` と `\` で「安全でない」判定**になり `'C:\…\tako.exe'` と
    /// なっていた。PowerShell は引用符付き文字列を**式として評価する**ので、これは
    /// 実行されずそのまま表示される。`:` と `\` は PowerShell では素で通るので、
    /// 素で通る形を第一候補にし、囲むときだけ呼び出し演算子 `&` を付ける。
    pub fn command_word(self, path: &str) -> String {
        let plain = |extra: &[u8]| {
            !path.is_empty()
                && path.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || b"._-/".contains(&b) || extra.contains(&b)
                })
        };
        match self {
            // POSIX は従来と 1 バイトも変えない（macOS の見た目を動かさない）。
            // `shell::quote_for_shell` へ委譲しないのは安全文字の集合が違うため
            // （あちらは `:` `@` `%` `+` `,` `=` も素で通す）。#873 で「クォートは
            // 統合しない」と決めた理由と同じで、ユーザーと AI に見える文字列なので
            // リファクタで形を変えない
            Self::Posix => {
                if plain(b"") {
                    path.to_string()
                } else {
                    format!("'{}'", path.replace('\'', r"'\''"))
                }
            }
            Self::PowerShell => {
                if plain(br"\:") {
                    path.to_string()
                } else {
                    format!("& {}", ps_quote(path))
                }
            }
        }
    }

    /// 1 個の引数としてそのまま渡す形にクオートする（中身は展開されない）。
    /// `tako send '<コマンド>'` のように**別のシェルで評価される文字列**を運ぶのに使う
    pub fn quote_arg(self, word: &str) -> String {
        match self {
            Self::Posix => crate::shell::quote_for_shell(word),
            Self::PowerShell => ps_quote(word),
        }
    }

    /// シェル片を走らせる **argv**（`Split { command }` / `spawn_session` へ渡す形）。
    ///
    /// 検証用の疑似 TUI をペインで走らせるのに使う。`/bin/sh` は Windows に無いので
    /// ここで振り替える（`powershell` は 5.1 でも 7 でも同じ名前で起動できる）。
    ///
    /// **PowerShell 側は `-EncodedCommand`（base64 / UTF-16LE）で渡す**（#903）。
    /// 素の `-Command "…"` にしないのは、器（psmux）が内側コマンドを
    /// **自分で単語分割する**ため（#875 が実行ペインで踏んだのと同じ 3 層問題）。
    /// 実機の A/B: 引用符入りのシェル片を `-Command` で渡すと**セッションが即死**し
    /// （`no server running on session …`）、同じ片を `-EncodedCommand` で渡すと
    /// 生き続けて画面を描いた。base64 の出力文字は `A-Za-z0-9+/=` だけなので、
    /// 単語分割・引用符の解釈・コマンドライン組み立てのどの層も通過する。
    /// 非 ASCII（罫線・`❯`）も UTF-16 のまま運べる = 器越しでも落ちない
    pub fn shell_snippet_command(self, snippet: &str) -> Vec<String> {
        match self {
            Self::Posix => vec!["/bin/sh".into(), "-c".into(), snippet.to_string()],
            Self::PowerShell => vec![
                "powershell".into(),
                "-NoProfile".into(),
                "-EncodedCommand".into(),
                crate::platform::shell::encode_powershell_command(snippet),
            ],
        }
    }

    /// 打ち込まれた行をそのまま画面へ返す前面プロセスの argv（POSIX の `cat` 相当）。
    ///
    /// セルフテストが「ペインへ文字列が届いたか」を見る道具。前面がシェルではないので
    /// 届いた行は**実行されず**、それでも画面に出る（本物のエージェントを起動せずに
    /// 送達だけを検証できる）。
    ///
    /// **Windows の `cat` は `Get-Content` のエイリアスで実体が無い**。tako の split は
    /// argv をそのまま `CreateProcess` へ渡すので（`platform::shell::login_shell_command`
    /// は Windows では包まない）ペインが即死し、送達の検証先が消えていた（#889）。
    /// PowerShell では標準入力を 1 行ずつ読んで書き戻すループへ振り替える
    pub fn echo_stdin_command(self) -> Vec<String> {
        match self {
            // 従来どおり 1 語（`login_shell_command` が `$SHELL -l -c cat` へ包む）
            Self::Posix => vec!["cat".to_string()],
            Self::PowerShell => self.shell_snippet_command(ECHO_STDIN_LOOP),
        }
    }

    /// シェル統合（OSC 7 / 133）を**ユーザーのファイルに依存せず**読み込ませた
    /// 対話シェルの argv。`None` = この方言は spawn 時の env 注入だけで統合が効くので
    /// 既定シェルをそのまま起こせばよい（POSIX）。
    ///
    /// なぜ要るか（#889）: PowerShell の統合は `$PROFILE` へ書いたブロック経由なので、
    /// `TAKO_ISOLATED=1` で data_dir を隔離するセルフテストからは配置が見えない
    /// （`status()` が見る `<隔離 data_dir>/shell-integration/tako.ps1` と `$PROFILE` が
    /// 指す本番のパスが別物になる）。**実機の `$PROFILE` の状態でテストの結果が変わる**
    /// ので、統合を自分で読ませた対話シェルを起こして前提を閉じる。
    ///
    /// **`.` とスクリプトパスは別々の語で渡す**: 実機の Rust argv → ConPTY 経路で
    /// 緑になっていることが分かっているのはこの形だけ
    /// （`tako-core/tests/shell_integration_powershell.rs` と同じ。5.1 は 1 語にすると
    /// 引用符を取りこぼしてドットソースごと落ちるという実測がそちらに残っている）。
    ///
    /// 既知の制限: **パスに空白があると効かない**（PowerShell は `-Command` の後ろの
    /// 語を引用符を落として空白で連結する。`Start-Process` 経由の実測で
    /// separate-words=False / single-word=True。#889）。統合スクリプトは data_dir 配下
    /// （隔離時は `%TEMP%`）なので現状は踏まないが、空白入りのユーザー名で踏んだら
    /// `-EncodedCommand`（#875 の実行ペインと同じ手）へ替えるのが筋
    pub fn integration_shell_command(self, program: &str, script: &Path) -> Option<Vec<String>> {
        match self {
            Self::Posix => None,
            Self::PowerShell => Some(vec![
                program.to_string(),
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-NoExit".to_string(),
                "-Command".to_string(),
                ".".to_string(),
                script.display().to_string(),
            ]),
        }
    }

    /// 画面を 1 枚描いて保持する（検証用の疑似 TUI）。
    ///
    /// `body` は **Rust のエスケープ**（`\n` / `\u{1b}`）でそのまま書く。
    /// POSIX 側は `printf '%b'` の書式へ、PowerShell 側は二重引用符の中の
    /// `` `n `` / `$([char]27)` へ翻訳する。**生の改行を出さない**のが肝で、
    /// そのまま打ち込む文字列としても使えるようにしてある。
    ///
    /// 保持するのは、描いた直後にシェルのプロンプトが戻ると最下部の行が
    /// プロンプトになって画面状態が変わってしまうため
    pub fn paint_and_hold(self, body: &str, seconds: u32) -> String {
        match self {
            Self::Posix => {
                let escaped = body
                    .replace('\\', "\\\\")
                    .replace('\u{1b}', "\\033")
                    .replace('\n', "\\n");
                format!("clear; printf '%b' '{escaped}'; sleep {seconds}")
            }
            Self::PowerShell => {
                let escaped = body
                    .replace('`', "``")
                    .replace('"', "`\"")
                    .replace('$', "`$")
                    .replace('\u{1b}', "$([char]27)")
                    .replace('\n', "`n");
                format!("Clear-Host; Write-Host -NoNewline \"{escaped}\"; Start-Sleep {seconds}")
            }
        }
    }

    /// ファイルの中身を画面へ描き直し続ける（検証用の疑似 TUI。#903）。
    ///
    /// `paint_and_hold` は「描いて sleep で保持」なので、状態を切り替えるには
    /// **Ctrl+C で sleep を止めて次のコマンドを打ち込む**必要がある。ところが
    /// Windows では両方が壊れる:
    ///
    /// - 器（psmux）の client 自身が PowerShell スクリプトなので **Ctrl+C で終了**し、
    ///   外側 PTY ごと死んでペインが閉じる（実測: 送った直後に `session=false`）
    /// - **器越しの打鍵から非 ASCII が落ちる**（実測: `─` と `❯` が消えて ASCII の
    ///   本文だけが画面に残る。器の中のシェル自身が印字する経路は無傷なので
    ///   出力側ではなく打鍵側）
    ///
    /// ファイル経由なら打鍵も割り込みも要らず、**書き換えた瞬間に描き替わる**。
    /// 中身は**生バイトのまま**出す（`printf '%b'` のような書式を通さない）ので
    /// ESC 列も日本語もそのまま置ける。変化が無ければ描き直さない = ちらつかない
    pub fn repaint_file_loop(self, path: &Path) -> String {
        let shown = path.display().to_string();
        match self {
            Self::Posix => {
                let quoted = crate::shell::quote_for_shell(&shown);
                format!(
                    "last=''; while :; do b=\"$(cat {quoted} 2>/dev/null)\"; \
                     if [ \"$b\" != \"$last\" ]; then clear; printf '%s' \"$b\"; last=\"$b\"; fi; \
                     sleep 0.3; done"
                )
            }
            Self::PowerShell => {
                let quoted = ps_quote(&shown);
                format!(
                    "$last = ''; while ($true) {{ \
                     $b = Get-Content -Raw -Encoding UTF8 -ErrorAction SilentlyContinue {quoted}; \
                     if ($null -ne $b -and $b -ne $last) {{ Clear-Host; \
                     Write-Host -NoNewline $b; $last = $b }}; \
                     Start-Sleep -Milliseconds 300 }}"
                )
            }
        }
    }

    /// 明示コマンド（`tako split -- <argv>`）として渡す「シェル片を走らせる argv」。
    ///
    /// 引数リストで来る経路なので、シェル片は 1 個の引数として包む
    pub fn shell_snippet_argv(self, snippet: &str) -> String {
        match self {
            Self::Posix => format!("sh -c {}", crate::shell::quote_for_shell(snippet)),
            // 5.1 でも通る名前で呼ぶ（pwsh が無い環境でも動く）
            Self::PowerShell => format!("powershell -NoProfile -Command {}", ps_quote(snippet)),
        }
    }

    /// 標準出力を捨てる
    pub fn discard_output(self, command: &str) -> String {
        match self {
            Self::Posix => format!("{command} >/dev/null"),
            Self::PowerShell => format!("{command} > $null"),
        }
    }

    /// `command` が成功したら `then` を実行する（`&&` 相当）。
    ///
    /// PowerShell 側は `$LASTEXITCODE` で判定するので**外部プロセスにだけ**使える
    /// （cmdlet は `$LASTEXITCODE` を更新しない）。セルフテストの対象は
    /// いずれも実バイナリ（`tako` CLI 等）なのでこれで足りる。
    /// pwsh 7 の `&&` を使わないのは 5.1 でも同じ文字列を通すため
    pub fn on_success(self, command: &str, then: &str) -> String {
        match self {
            Self::Posix => format!("{command} && {then}"),
            Self::PowerShell => format!("{command}; if ($LASTEXITCODE -eq 0) {{ {then} }}"),
        }
    }

    /// `command` が成功したらマーカーを出す
    pub fn on_success_echo(self, command: &str, marker: &str) -> String {
        self.on_success(command, &self.echo(marker))
    }

    /// コマンドの標準出力を変数へ入れる（`p=$(cmd)` 相当）
    pub fn assign_output(self, name: &str, command: &str) -> String {
        match self {
            Self::Posix => format!("{name}=$({command})"),
            Self::PowerShell => format!("${name} = {command}"),
        }
    }

    /// 変数参照（`$p`。どちらの方言も同じだが、意図を境界に持たせておく）
    pub fn var(self, name: &str) -> String {
        format!("${name}")
    }

    /// 同じ手順を `times` 回繰り返す
    pub fn repeat(self, times: u32, body: &str) -> String {
        match self {
            Self::Posix => format!("for i in $(seq 1 {times}); do {body}; done"),
            Self::PowerShell => format!("1..{times} | ForEach-Object {{ {body} }}"),
        }
    }

    /// 文（複数コマンド）を順に並べる。区切りはどちらの方言も `;`
    pub fn sequence(self, parts: &[String]) -> String {
        parts.join("; ")
    }

    /// `command` の出力に `needle` が含まれていたらマーカーを出す（`grep -q` 相当）
    pub fn on_output_contains_echo(self, command: &str, needle: &str, marker: &str) -> String {
        match self {
            Self::Posix => format!(
                "{command} | grep -q {} && echo \"{marker}\"",
                crate::shell::quote_for_shell(needle)
            ),
            Self::PowerShell => format!(
                "if ({command} | Select-String -Quiet -SimpleMatch {}) {{ echo \"{marker}\" }}",
                ps_quote(needle)
            ),
        }
    }

    /// 環境変数が空でなければマーカーを出す（`[ -n "$VAR" ]` 相当）
    pub fn on_env_set_echo(self, name: &str, marker: &str) -> String {
        match self {
            Self::Posix => format!("[ -n \"${{{name}}}\" ] && echo \"{marker}\""),
            Self::PowerShell => format!("if ($env:{name}) {{ echo \"{marker}\" }}"),
        }
    }

    /// IPC の受け口（unix domain socket / named pipe）が実在し、トークンも入っていれば
    /// マーカーを出す。
    ///
    /// unix は `test -S`（ソケット種別まで見る）、Windows は `Test-Path`
    /// （`\\.\pipe\…` に対して実在で true / 不在で false を実測。#865）
    pub fn on_ipc_endpoint_ready_echo(
        self,
        endpoint_env: &str,
        token_env: &str,
        marker: &str,
    ) -> String {
        match self {
            Self::Posix => format!(
                "test -S \"${{{endpoint_env}}}\" && [ -n \"${{{token_env}}}\" ] \
                 && echo \"{marker}\""
            ),
            Self::PowerShell => format!(
                "if ((Test-Path $env:{endpoint_env}) -and $env:{token_env}) \
                 {{ echo \"{marker}\" }}"
            ),
        }
    }

    /// 現在の cwd がホームならマーカーを出す
    pub fn on_cwd_is_home_echo(self, marker: &str) -> String {
        match self {
            Self::Posix => format!("[ \"$PWD\" = \"$HOME\" ] && echo \"{marker}\""),
            Self::PowerShell => format!("if ($PWD.Path -eq $HOME) {{ echo \"{marker}\" }}"),
        }
    }

    /// 環境変数を**一時的に**差し替えてコマンドを走らせる。
    ///
    /// PowerShell には `VAR=v cmd` に当たる構文が無いので、退避 → 設定 → 実行 → 復帰
    /// を 1 行で組む（代入は `$LASTEXITCODE` を変えないので、後段の成功判定に影響しない）
    pub fn with_env(self, vars: &[(&str, &str)], command: &str) -> String {
        match self {
            Self::Posix => {
                let mut out = String::new();
                for (name, value) in vars {
                    out.push_str(name);
                    out.push('=');
                    out.push_str(&crate::shell::quote_for_shell(value));
                    out.push(' ');
                }
                out.push_str(command);
                out
            }
            Self::PowerShell => {
                let mut save = String::new();
                let mut restore = String::new();
                for (index, (name, value)) in vars.iter().enumerate() {
                    let tmp = format!("$__tako{index}");
                    save.push_str(&format!(
                        "{tmp}=$env:{name}; $env:{name}={}; ",
                        ps_quote(value)
                    ));
                    restore.push_str(&format!("; {}", ps_restore_env(name, &tmp)));
                }
                format!("{save}{command}{restore}")
            }
        }
    }

    /// 環境変数を**外して**コマンドを走らせる（`env -u VAR cmd` 相当）
    pub fn without_env(self, names: &[&str], command: &str) -> String {
        match self {
            Self::Posix => {
                let unsets: Vec<String> = names.iter().map(|n| format!("-u {n}")).collect();
                format!("env {} {command}", unsets.join(" "))
            }
            Self::PowerShell => {
                let mut save = String::new();
                let mut restore = String::new();
                for (index, name) in names.iter().enumerate() {
                    let tmp = format!("$__tako{index}");
                    save.push_str(&format!(
                        "{tmp}=$env:{name}; Remove-Item Env:{name} -ErrorAction SilentlyContinue; "
                    ));
                    restore.push_str(&format!("; {}", ps_restore_env(name, &tmp)));
                }
                format!("{save}{command}{restore}")
            }
        }
    }
}

/// 標準入力を 1 行ずつ読んで書き戻す PowerShell 片（`cat` 相当。#889）。
///
/// **引用符を 1 個も含めない**: この文字列は `-Command` の 1 語として届くまでに
/// 複数の層（Rust の argv 組み立て → PowerShell の引数解釈）を通るので、引用符を
/// 入れると層ごとに解釈が変わる（同じ理由で実行ペインは `-EncodedCommand` を使う。#875）。
/// `[Console]::In.ReadLine()` を直に呼ぶのは PSReadLine の行編集を挟まないため
/// （届いたバイトの見え方が素直になる）。`$null` で抜けるのは stdin が閉じたときに
/// 例外ループへ落ちないようにする保険
const ECHO_STDIN_LOOP: &str = "while ($true) { $line = [Console]::In.ReadLine(); \
     if ($null -eq $line) { break }; [Console]::Out.WriteLine($line) }";

/// PowerShell の単引用符クオート（中の `'` は `''`）
fn ps_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "''"))
}

/// 退避した値を戻す。**元から無かったなら消す**（`$null` を代入すると
/// PowerShell は空文字列の環境変数を残すので、明示的に取り除く）
fn ps_restore_env(name: &str, tmp: &str) -> String {
    format!(
        "if ($null -ne {tmp}) {{ $env:{name}={tmp} }} \
         else {{ Remove-Item Env:{name} -ErrorAction SilentlyContinue }}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1127: 打ち込んだ行に本文が現れない出力手段。
    /// 両方言ぶんを macOS からも Windows からも検証する
    #[test]
    fn print_fileは本文をコマンド行へ載せない() {
        let path = Path::new("/tmp/tako-1127.txt");
        for dialect in [ShellDialect::Posix, ShellDialect::PowerShell] {
            let line = dialect.print_file(path);
            assert!(
                line.contains("tako-1127.txt"),
                "{dialect:?}: パスが入っていない: {line}"
            );
            // 本文（= 中身）は 1 文字も現れない
            assert!(
                !line.contains("ssh exit "),
                "{dialect:?}: 本文がコマンド行に載っている: {line}"
            );
        }
        // PowerShell 5.1 は既定でコードページ依存に読むので UTF-8 を明示する
        assert!(
            ShellDialect::PowerShell
                .print_file(path)
                .contains("-Encoding UTF8"),
            "PowerShell の読み出しに -Encoding UTF8 が無い（日本語が化ける）"
        );
        assert!(ShellDialect::Posix.print_file(path).starts_with("cat "));
    }

    /// 対照: [`ShellDialect::print_lines`] は**本文を引数に持つ**ので
    /// エコー行に載る。だから項目 137 の fixture では使えない（#1127）
    #[test]
    fn print_linesは本文がコマンド行に載る() {
        let body = "tako: h への接続に失敗しました（ssh exit ）。理由は上の行です".to_string();
        for dialect in [ShellDialect::Posix, ShellDialect::PowerShell] {
            assert!(
                dialect
                    .print_lines(std::slice::from_ref(&body))
                    .contains("ssh exit "),
                "{dialect:?}: 前提が変わった（print_lines が本文を隠すようになった）"
            );
        }
    }

    const POSIX: ShellDialect = ShellDialect::Posix;
    const PS: ShellDialect = ShellDialect::PowerShell;

    #[test]
    fn シェル名から方言を引く() {
        assert_eq!(ShellDialect::from_program("/bin/zsh"), Some(POSIX));
        assert_eq!(ShellDialect::from_program("/bin/sh"), Some(POSIX));
        assert_eq!(
            ShellDialect::from_program("/usr/local/bin/bash"),
            Some(POSIX)
        );
        assert_eq!(
            ShellDialect::from_program("C:\\Program Files\\PowerShell\\7\\pwsh.exe"),
            Some(PS)
        );
        assert_eq!(
            ShellDialect::from_program(
                "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
            ),
            Some(PS)
        );
        // 大文字小文字は Windows のパス表記に合わせて無視する
        assert_eq!(
            ShellDialect::from_program("C:\\WINDOWS\\PWSH.EXE"),
            Some(PS)
        );
    }

    #[test]
    fn 変換先を持たないシェルはnone() {
        // cmd.exe は `$` も `[` も無く、fish は `$(( ))` を持たない。
        // 「たぶん POSIX」で倒すと原因の分かりにくい失敗になるので判定を拒否する
        assert_eq!(
            ShellDialect::from_program("C:\\Windows\\system32\\cmd.exe"),
            None
        );
        assert_eq!(ShellDialect::from_program("/opt/homebrew/bin/fish"), None);
    }

    /// #865 で実機が教えた差: PSReadLine の Windows モードに Ctrl+U は無い
    #[test]
    fn 入力行を消すキーは方言で変わる() {
        assert_eq!(POSIX.clear_line_key(), "ctrl-u");
        assert_eq!(PS.clear_line_key(), "escape");
    }

    #[test]
    fn 環境変数参照は方言ごとに変わる() {
        assert_eq!(POSIX.env_ref("TERM"), "${TERM}");
        assert_eq!(PS.env_ref("TERM"), "$env:TERM");
    }

    /// #865 の本体。PowerShell では**必ず二重引用符で包む**
    /// （裸の引数はカンマが配列区切りになり 2 行へ割れる = 実測）
    #[test]
    fn echoは環境変数を展開して1行で出す() {
        assert_eq!(
            POSIX.echo("TERMCHK=${TERM},${COLORTERM}"),
            "echo \"TERMCHK=${TERM},${COLORTERM}\""
        );
        assert_eq!(
            PS.echo("TERMCHK=${TERM},${COLORTERM}"),
            "echo \"TERMCHK=$env:TERM,$env:COLORTERM\""
        );
    }

    #[test]
    fn 閉じていないテンプレートは黙って直さない() {
        assert_eq!(PS.expand_env_refs("a${TERM"), "a${TERM");
    }

    #[test]
    fn マーカーは打った行に答えが出ない形で組む() {
        assert_eq!(POSIX.marker("CWDCHK-", 40, 2), "CWDCHK-$((40+2))");
        assert_eq!(PS.marker("CWDCHK-", 40, 2), "CWDCHK-$(40+2)");
        for dialect in [POSIX, PS] {
            assert!(
                !dialect.marker("CWDCHK-", 40, 2).contains("42"),
                "マーカー式に答えが現れてはいけない（画面検索がエコー行を拾う）"
            );
        }
    }

    #[test]
    fn ansiはescをそのまま端末へ書く() {
        assert_eq!(POSIX.emit_ansi("\u{1b}[?1049h"), "printf '\\e[?1049h'");
        assert_eq!(
            PS.emit_ansi("\u{1b}[31mRED\u{1b}[0m"),
            "Write-Host -NoNewline \"$([char]27)[31mRED$([char]27)[0m\""
        );
    }

    /// 生の改行が単引用符の中へ入ると「打ち込むキー列の改行」= 途中実行になる。
    /// 改行は書式として渡す
    #[test]
    fn ansi行は改行を書式として渡す() {
        let posix = POSIX.emit_ansi_line("\u{1b}[31mRED\u{1b}[0m");
        assert_eq!(posix, "printf '\\e[31mRED\\e[0m\\n'");
        assert!(!posix.contains('\n'), "生の改行が混ざっている: {posix}");
        assert_eq!(
            PS.emit_ansi_line("\u{1b}[31mRED\u{1b}[0m"),
            "Write-Host \"$([char]27)[31mRED$([char]27)[0m\""
        );
    }

    #[test]
    fn ディレクトリを作って移動する() {
        assert_eq!(
            POSIX.mkdir_and_cd("/tmp/tako-osc-e2e"),
            "mkdir -p /tmp/tako-osc-e2e && cd /tmp/tako-osc-e2e"
        );
        assert_eq!(
            PS.mkdir_and_cd("C:\\Temp\\tako-osc-e2e"),
            "New-Item -ItemType Directory -Force -Path 'C:\\Temp\\tako-osc-e2e' | Out-Null; \
             Set-Location 'C:\\Temp\\tako-osc-e2e'"
        );
    }

    /// 現在のディレクトリの出力（#935）。PowerShell は `pwd` のエイリアスを使わない
    /// （`Get-Location` は表として整形され、パスがコンソール幅で切られる = 実機実測）
    #[test]
    fn 現在のディレクトリを1行で出す() {
        assert_eq!(POSIX.print_cwd(), "pwd");
        assert_eq!(PS.print_cwd(), "Write-Output $PWD.Path");
        assert!(
            !PS.print_cwd().split_whitespace().any(|w| w == "pwd"),
            "PowerShell 側が pwd エイリアスへ戻っている"
        );
        // `Write-Host` は情報レコードが CLIXML で stderr へ出る（実機実測 1078 バイト）
        assert!(
            !PS.print_cwd().contains("Write-Host"),
            "Write-Host へ戻っている（stderr が CLIXML で汚れる）"
        );
    }

    /// `tako split -- <argv>` へ渡す「シェル片を走らせる argv」
    #[test]
    fn シェル片を走らせるargv() {
        assert_eq!(
            POSIX.shell_snippet_argv("echo X; sleep 15"),
            "sh -c 'echo X; sleep 15'"
        );
        assert_eq!(
            PS.shell_snippet_argv("echo X; Start-Sleep 15"),
            "powershell -NoProfile -Command 'echo X; Start-Sleep 15'"
        );
    }

    /// ファイルの中身を描き直し続けるループ（#903）。
    ///
    /// 疑似 TUI の状態を**打鍵ではなくファイルの書き換え**で切り替えるための形。
    /// 不変条件は 4 つ: 生の改行を出さない（1 行のシェル片として渡せる）/
    /// 中身を書式解釈せずそのまま出す / 変化が無ければ描き直さない（ちらつき防止）/
    /// パスを引用する（空白入りのパスで割れない）
    #[test]
    fn ファイルの中身を描き直し続ける() {
        let posix = POSIX.repaint_file_loop(Path::new("/tmp/tako 903/body.txt"));
        assert!(
            posix.contains("cat '/tmp/tako 903/body.txt'"),
            "パスが引用されていない: {posix}"
        );
        assert!(posix.contains("printf '%s'"), "書式解釈している: {posix}");
        assert!(
            posix.contains(r#""$b" != "$last""#),
            "変化検出が無い: {posix}"
        );
        let ps = PS.repaint_file_loop(Path::new("C:\\Temp\\tako 903\\body.txt"));
        assert!(
            ps.contains(
                "Get-Content -Raw -Encoding UTF8 -ErrorAction SilentlyContinue \
                         'C:\\Temp\\tako 903\\body.txt'"
            ),
            "読み方 / 引用が想定と違う: {ps}"
        );
        assert!(
            ps.contains("Write-Host -NoNewline $b"),
            "生バイトのまま出していない: {ps}"
        );
        assert!(ps.contains("$b -ne $last"), "変化検出が無い: {ps}");
        for rendered in [posix, ps] {
            assert!(
                !rendered.contains('\n'),
                "生の改行が混ざっている: {rendered}"
            );
        }
    }

    /// 疑似 TUI の 1 枚絵。**生の改行を出さない**（打ち込む文字列としても使える）
    #[test]
    fn 画面を描いて保持する() {
        let body = "\u{1b}[2J行1\n\u{1b}[31m行2\n";
        let posix = POSIX.paint_and_hold(body, 30);
        assert_eq!(
            posix,
            "clear; printf '%b' '\\033[2J行1\\n\\033[31m行2\\n'; sleep 30"
        );
        let ps = PS.paint_and_hold(body, 30);
        assert_eq!(
            ps,
            "Clear-Host; Write-Host -NoNewline \
             \"$([char]27)[2J行1`n$([char]27)[31m行2`n\"; Start-Sleep 30"
        );
        for rendered in [posix, ps] {
            assert!(
                !rendered.contains('\n'),
                "生の改行が混ざっている: {rendered}"
            );
        }
    }

    /// PowerShell の二重引用符の中で意味を持つ文字を素で通さない
    #[test]
    fn 画面本文の特殊文字を潰す() {
        let ps = PS.paint_and_hold("a\"b`c$d", 1);
        assert!(ps.contains("a`\"b``c`$d"), "{ps}");
    }

    #[test]
    fn シェル片のargvは方言で変わる() {
        assert_eq!(
            POSIX.shell_snippet_command("echo x"),
            vec!["/bin/sh", "-c", "echo x"]
        );
        // PowerShell 側は `-EncodedCommand`（#903）。器（psmux）が内側コマンドを
        // 単語分割するので、引用符・空白・非 ASCII を含む片は符号化しないと死ぬ
        let snippet = "$last = ''; Write-Host -NoNewline '❯ 箱'; Start-Sleep 30";
        let got = PS.shell_snippet_command(snippet);
        assert_eq!(got[..3], ["powershell", "-NoProfile", "-EncodedCommand"]);
        assert!(
            got[3]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'='),
            "base64 以外の文字が混ざっている: {}",
            got[3]
        );
        assert_eq!(
            crate::platform::shell::decode_powershell_command(&got[3]),
            snippet,
            "符号化した片が元に戻らない"
        );
    }

    #[test]
    fn 連番行をゆっくり出す() {
        assert_eq!(
            POSIX.emit_numbered_lines("l", 150, 20),
            "i=0; while [ $i -lt 150 ]; do printf 'l%d\\n' $i; sleep 0.020; i=$((i+1)); done"
        );
        assert_eq!(
            PS.emit_numbered_lines("l", 150, 20),
            "0..149 | ForEach-Object { Write-Output \"l$_\"; Start-Sleep -Milliseconds 20 }"
        );
    }

    #[test]
    fn 語を1行ずつ出す() {
        let words = vec!["A".to_string(), "~/".to_string(), "x y".to_string()];
        assert_eq!(POSIX.print_lines(&words), "printf '%s\\n' A '~/' 'x y'");
        assert_eq!(PS.print_lines(&words), "echo 'A' '~/' 'x y'");
    }

    #[test]
    fn 連番と待機と終了コード() {
        assert_eq!(POSIX.seq(1, 200), "seq 1 200");
        assert_eq!(PS.seq(1, 200), "1..200");
        assert_eq!(POSIX.sleep(5), "sleep 5");
        assert_eq!(PS.sleep(5), "Start-Sleep 5");
        assert_eq!(POSIX.exit_status(0), "true");
        assert_eq!(POSIX.exit_status(1), "sh -c 'exit 1'");
        // PowerShell の `exit` はシェルごと終わるので外部プロセスで返す
        assert_eq!(PS.exit_status(1), "cmd /c exit 1");
    }

    #[test]
    fn フルパス起動はpowershellで呼び出し演算子が付く() {
        let path = Path::new("C:\\t a\\tako.exe");
        assert_eq!(POSIX.program(Path::new("/x/tako")), "\"/x/tako\"");
        assert_eq!(PS.program(path), "& \"C:\\t a\\tako.exe\"");
    }

    #[test]
    fn 成功時マーカーと出力一致マーカー() {
        assert_eq!(
            POSIX.on_success_echo(&POSIX.discard_output("tako list"), "M-$((40+2))"),
            "tako list >/dev/null && echo \"M-$((40+2))\""
        );
        assert_eq!(
            PS.on_success_echo(&PS.discard_output("tako list"), "M-$(40+2)"),
            "tako list > $null; if ($LASTEXITCODE -eq 0) { echo \"M-$(40+2)\" }"
        );
        assert_eq!(
            POSIX.on_output_contains_echo("tako persist", "\"enabled\":true", "M"),
            "tako persist | grep -q '\"enabled\":true' && echo \"M\""
        );
        assert_eq!(
            PS.on_output_contains_echo("tako persist", "\"enabled\":true", "M"),
            "if (tako persist | Select-String -Quiet -SimpleMatch '\"enabled\":true') \
             { echo \"M\" }"
        );
    }

    #[test]
    fn 引数クオートは中身を展開させない() {
        // 別ペインのシェルで評価させる文字列を運ぶ（`tako send`）
        assert_eq!(
            POSIX.quote_arg("echo \"X-$((40+2))\""),
            "'echo \"X-$((40+2))\"'"
        );
        assert_eq!(PS.quote_arg("echo \"X-$(40+2)\""), "'echo \"X-$(40+2)\"'");
        // 単引用符を含む語も壊れない
        assert_eq!(PS.quote_arg("it's"), "'it''s'");
        assert_eq!(POSIX.quote_arg("it's"), "'it'\\''s'");
    }

    #[test]
    fn 環境変数の実在判定() {
        assert_eq!(
            POSIX.on_env_set_echo("TAKO_MCP_URL", "M"),
            "[ -n \"${TAKO_MCP_URL}\" ] && echo \"M\""
        );
        assert_eq!(
            PS.on_env_set_echo("TAKO_MCP_URL", "M"),
            "if ($env:TAKO_MCP_URL) { echo \"M\" }"
        );
        assert_eq!(
            POSIX.on_ipc_endpoint_ready_echo("TAKO_SOCKET", "TAKO_TOKEN", "M"),
            "test -S \"${TAKO_SOCKET}\" && [ -n \"${TAKO_TOKEN}\" ] && echo \"M\""
        );
        assert_eq!(
            PS.on_ipc_endpoint_ready_echo("TAKO_SOCKET", "TAKO_TOKEN", "M"),
            "if ((Test-Path $env:TAKO_SOCKET) -and $env:TAKO_TOKEN) { echo \"M\" }"
        );
    }

    #[test]
    fn cwdがホームかの判定() {
        assert_eq!(
            POSIX.on_cwd_is_home_echo("M"),
            "[ \"$PWD\" = \"$HOME\" ] && echo \"M\""
        );
        assert_eq!(
            PS.on_cwd_is_home_echo("M"),
            "if ($PWD.Path -eq $HOME) { echo \"M\" }"
        );
    }

    /// ストレス検査（項目 40b）の 1 行が両方言で組めること
    #[test]
    fn 繰り返しと出力の変数化() {
        let body_posix = POSIX.on_success(
            &POSIX.assign_output("p", "tako split --right"),
            &format!("tako close --pane {}", POSIX.var("p")),
        );
        assert_eq!(
            POSIX.repeat(10, &body_posix),
            "for i in $(seq 1 10); do p=$(tako split --right) && tako close --pane $p; done"
        );
        let body_ps = PS.on_success(
            &PS.assign_output("p", "tako split --right"),
            &format!("tako close --pane {}", PS.var("p")),
        );
        assert_eq!(
            PS.repeat(10, &body_ps),
            "1..10 | ForEach-Object { $p = tako split --right; \
             if ($LASTEXITCODE -eq 0) { tako close --pane $p } }"
        );
    }

    #[test]
    fn 文の並びは方言に依らず同じ区切り() {
        for dialect in [POSIX, PS] {
            assert_eq!(
                dialect.sequence(&["a".to_string(), "b".to_string()]),
                "a; b"
            );
        }
    }

    #[test]
    fn 環境変数の一時差し替えと除去() {
        assert_eq!(
            POSIX.with_env(&[("TAKO_SOCKET", "/nonexistent.sock")], "tako list"),
            "TAKO_SOCKET=/nonexistent.sock tako list"
        );
        assert_eq!(
            PS.with_env(&[("TAKO_SOCKET", "/nonexistent.sock")], "tako list"),
            "$__tako0=$env:TAKO_SOCKET; $env:TAKO_SOCKET='/nonexistent.sock'; tako list\
             ; if ($null -ne $__tako0) { $env:TAKO_SOCKET=$__tako0 } \
             else { Remove-Item Env:TAKO_SOCKET -ErrorAction SilentlyContinue }"
        );
        assert_eq!(
            POSIX.without_env(&["TAKO_SOCKET", "TAKO_TOKEN"], "tako list"),
            "env -u TAKO_SOCKET -u TAKO_TOKEN tako list"
        );
        assert_eq!(
            PS.without_env(&["TAKO_TOKEN"], "tako list"),
            "$__tako0=$env:TAKO_TOKEN; Remove-Item Env:TAKO_TOKEN -ErrorAction SilentlyContinue; \
             tako list; if ($null -ne $__tako0) { $env:TAKO_TOKEN=$__tako0 } \
             else { Remove-Item Env:TAKO_TOKEN -ErrorAction SilentlyContinue }"
        );
    }

    /// #889: `cat` は Windows に実体が無い（`Get-Content` のエイリアス）。
    /// POSIX 側は**従来と 1 バイトも変えない**（macOS の項目 93 / 97 が同じものを測る）
    #[test]
    fn 打鍵をそのまま返すペインのargvは方言で変わる() {
        assert_eq!(POSIX.echo_stdin_command(), vec!["cat".to_string()]);
        let ps = PS.echo_stdin_command();
        // 渡し方は `shell_snippet_command` と同じ = `-EncodedCommand`（#903）
        assert_eq!(ps[..3], ["powershell", "-NoProfile", "-EncodedCommand"]);
        let snippet = crate::platform::shell::decode_powershell_command(&ps[3]);
        let snippet = snippet.as_str();
        // 標準入力を読んで書き戻す = `cat` と同じ役（届いた行が実行されない）
        assert!(
            snippet.contains("ReadLine"),
            "stdin を読んでいない: {snippet}"
        );
        assert!(snippet.contains("WriteLine"), "書き戻していない: {snippet}");
        // Rust の行継続でつないでいるので、空白が潰れていないことも固定する
        assert!(
            !snippet.contains("  ") && !snippet.contains(");if"),
            "行継続で空白が壊れている: {snippet}"
        );
    }

    /// #889: PowerShell の統合は `$PROFILE` 経由なので、隔離した data_dir で走る
    /// セルフテストからは配置が見えない。自分でドットソースして前提を閉じる
    #[test]
    fn 統合を読み込ませた対話シェルのargv() {
        let script = Path::new("C:\\iso\\shell-integration\\tako.ps1");
        // POSIX は env 注入（ZDOTDIR 等）で完結するので明示コマンドは要らない
        assert_eq!(POSIX.integration_shell_command("/bin/zsh", script), None);
        let ps = PS
            .integration_shell_command("C:\\Program Files\\PowerShell\\7\\pwsh.exe", script)
            .expect("PowerShell では明示コマンドが要る");
        assert_eq!(ps[0], "C:\\Program Files\\PowerShell\\7\\pwsh.exe");
        // ユーザーのファイルを読ませない + コマンド実行後も対話を続ける
        assert!(ps.contains(&"-NoProfile".to_string()));
        assert!(ps.contains(&"-NoExit".to_string()));
        // `.` とパスは**別の語**（1 語にすると 5.1 が引用符を落として落ちる。実測）
        assert_eq!(ps[ps.len() - 2], ".");
        assert_eq!(ps[ps.len() - 1], script.display().to_string());
    }

    /// 実環境の既定シェルから方言が引けること（両プラットフォームの経路が動く証明）
    #[test]
    fn 実環境の既定シェルの方言を引ける() {
        let dialect = for_default_shell();
        assert!(
            dialect.is_some(),
            "既定シェルの方言が判定できない: {:?}",
            super::super::shell::default_shell().map(|s| s.program)
        );
        if cfg!(windows) {
            assert_eq!(dialect, Some(PS));
        } else {
            assert_eq!(dialect, Some(POSIX));
        }
    }
}
