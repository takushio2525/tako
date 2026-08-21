//! launch_cmd — エージェント起動コマンドの env 前置きとクォートをシェル方言へ寄せる（#867）
//!
//! # なぜ要るのか
//!
//! tako は「ペインで素のシェルを起こし、そこへ起動コマンドを打ち込む」形でエージェントを
//! 立てる。この打ち込む 1 行が POSIX 構文の直書きだと、**Windows では必ず起動できない**。
//! 実測（#867。psmux + pwsh 7.6.5。送達自体は #640 で解決済み）:
//!
//! ```text
//! PS C:\...\proj> TAKO_ORCHESTRATOR_ROLE='worker:p7b' claude --effort max
//! TAKO_ORCHESTRATOR_ROLE=worker:p7b: The term '...' is not recognized as a name of a
//! cmdlet, function, script file, or executable program.
//! ```
//!
//! PowerShell に `VAR=value cmd` というインライン前置きの構文は無い（`$env:VAR='v'; cmd`）。
//! `export` / `unset` も同様に無い。
//!
//! # 対象（#640 の 4 経路 + master / solo が通る 3 関数）
//!
//! | 関数 | 通る経路 |
//! |---|---|
//! | `orchestrator::agent::build_worker_cmd` | orchestrator spawn / git resolve のエージェント |
//! | `orchestrator::build_master_cmd` | `tako master` / `tako solo` / handoff の後任 master |
//! | `transcript::resume_env_prefix_for` | `sessions resume` / worker レジストリの resume_command |
//!
//! # 設計
//!
//! 方言は [`ShellDialect`]（`platform::shell_dialect`）が正。ここは**組み立て**だけを持ち、
//! 方言を引数で受け取る純粋関数なので **macOS 上から PowerShell 側の出力を全分岐テスト
//! できる**。環境から方言を引く薄い入口が [`launch_dialect`]。
//!
//! ## `platform::shell_dialect` との関係（#873 で一本化した）
//!
//! **方言の判定は `ShellDialect` の 1 本だけ**。#867 の着手時点では #865 が未マージ・
//! 活発に変更中で、あちらのファイルへ同時に触るとコンフリクトが避けられなかったため
//! 一時的に独自の enum を持っていたが、#865 のマージ後に #873 で寄せた。
//! ここが持つのは**組み立て**と、起動コマンド専用のクォート規則だけ。
//!
//! ### クォートは統合しない（実測に基づく判断）
//!
//! `ShellDialect::quote_arg`（セルフテスト向け）は `tako_core::shell::quote_for_shell`
//! 経由で、[`quote`] が使う `sh_quote` とは**安全文字の集合が違う**。実測で 10 入力中
//! 7 件が相違し、実運用の値も含む:
//!
//! | 入力 | `quote`（起動コマンド） | `quote_arg`（セルフテスト） |
//! |---|---|---|
//! | `worker:p867`（role） | `'worker:p867'` | `worker:p867` |
//! | `検証`（日本語ラベル） | `検証`（Unicode 英数を素通し） | `'検証'` |
//! | `a,b` / `x@y` / `50%` / `a+b` | 引用する | 素通し |
//!
//! 起動コマンドの文字列は spawn 応答の `command` やレジストリの `resume_command` として
//! **ユーザーと AI に見える**うえ、既存のスナップショットが「#120 以前と同一文字列」を
//! 固定している。寄せると**見える文字列が変わる**ので、リファクタで倒してよい話ではない。
//! 取り違えを止めるため、両者が違うことを `クォート規則は2本あることを固定する` で固定した。
//!
//! なお `shell_dialect` 側の `with_env` / `without_env` は
//! 「退避 → 設定 → 実行 → 復帰」でセルフテストの 1 行用、こちらは
//! **起動したプロセスへ引き継がせたい**ので復帰しない = 意味論そのものが別。
//!
//! # POSIX 側は 1 バイトも変えない
//!
//! 既存の出力（スナップショットテストが固定している）を保つため、クォートは 2 系統ある:
//!
//! - [`quote`] = 必要なときだけ引用（`sh_quote` と同一。`--model` 等）
//! - [`quote_always`] = 常に引用（元コードが `'{x}'` と直書きしていた箇所）
//!
//! この区別が無いと `--append-system-prompt-file '/tmp/p.md'` が
//! `--append-system-prompt-file /tmp/p.md` に変わってしまう。

pub use tako_core::platform::shell_dialect::ShellDialect;

/// 起動コマンドを打ち込む先のシェルの方言。
///
/// 判定は `ShellDialect::from_program` が正で、打ち込む先は
/// `platform::shell::default_shell()`（プロセスの `$SHELL` ではなく**実際にペインで動くもの**を
/// 見ないと、器や設定でシェルが変わったときに食い違う）。
///
/// **知らないシェル（`cmd.exe` / fish）では POSIX に倒す**。`from_program` が `None` を
/// 返すのはセルフテスト側が「対象外」を明示できるようにするためだが、起動コマンドで
/// 組み立てを止めると**これまで動いていた環境でエージェントが起動しなくなる**。
/// fish は元から `export` を持たないので env つきプロファイルは以前から効いていない
/// （#867 で変わる話ではない）
pub fn launch_dialect() -> ShellDialect {
    tako_core::platform::shell_dialect::for_default_shell().unwrap_or(ShellDialect::Posix)
}

/// 値を「必要なときだけ」引用する（POSIX は `sh_quote` と同一の出力）。
///
/// PowerShell は裸のトークンで `,` が配列区切りになる・`;` `(` `$` `@` が構文になる等、
/// 素で通せる文字の判断が難しいので**常に単引用符で包む**（単引用符の中は完全にリテラル）
pub fn quote(dialect: ShellDialect, s: &str) -> String {
    match dialect {
        ShellDialect::Posix => crate::orchestrator::agent::sh_quote(s),
        ShellDialect::PowerShell => ps_quote(s),
    }
}

/// 値を常に引用する（元コードが `'{x}'` と直書きしていた箇所の置き換え）
pub fn quote_always(dialect: ShellDialect, s: &str) -> String {
    match dialect {
        ShellDialect::Posix => format!("'{}'", s.replace('\'', "'\\''")),
        ShellDialect::PowerShell => ps_quote(s),
    }
}

/// PowerShell の単引用符リテラル。中の `'` は `''` で表す（バックスラッシュは効かない）
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 環境変数を**解除**する前置き（末尾に `; ` を含む）。
///
/// PowerShell は `Remove-Item` で `Env:` プロバイダから消す。設定されていないときに
/// エラーを出さないよう `-ErrorAction SilentlyContinue` を付ける（`unset` は POSIX でも
/// 未設定でエラーにならないので、挙動を揃えるために必要）
pub fn unset_prefix(dialect: ShellDialect, key: &str) -> String {
    match dialect {
        ShellDialect::Posix => format!("unset {}; ", quote(dialect, key)),
        ShellDialect::PowerShell => format!(
            "Remove-Item -LiteralPath {} -ErrorAction SilentlyContinue; ",
            ps_quote(&format!("Env:{key}"))
        ),
    }
}

/// 環境変数を**設定**する前置き（末尾に `; ` を含む）。
///
/// どちらの方言でも「この行より後に走るものすべて」に効く。ログインシェルの rc
/// （direnv 等）はコマンド行より先に走るので、後から実行されるこちらが勝つ（#500 / #512）
pub fn export_prefix(dialect: ShellDialect, key: &str, value: &str) -> String {
    match dialect {
        ShellDialect::Posix => {
            format!("export {}={}; ", quote(dialect, key), quote(dialect, value))
        }
        ShellDialect::PowerShell => format!("$env:{key}={}; ", ps_quote(value)),
    }
}

/// 続くコマンド 1 つへ引き継ぐ環境変数の前置き。
///
/// POSIX は**インライン前置き**（`VAR=v cmd`）でそのコマンドだけに効かせる。
/// PowerShell にインライン前置きは無いので代入 + `;` になり、**シェル自身にも残る**。
/// ペインはそのエージェント専用なので実害は無く、むしろ同じペインで撃ち直したときに
/// role が残るぶん都合がよい（`SpawnOptions.env` のプロファイル env も同じくシェルに入る）
pub fn inline_env_prefix(dialect: ShellDialect, key: &str, value: &str) -> String {
    match dialect {
        // 元コードは `sh_quote(role)` と `'{role_env}'` の 2 通りがあったので、
        // 呼び出し側が使い分けられるよう値のクォートは渡す前に済ませてもらう
        ShellDialect::Posix => format!("{key}={value} "),
        ShellDialect::PowerShell => format!("$env:{key}={value}; "),
    }
}

/// ファイルの中身を 1 引数として埋め込む式（`"$(cat p.md)"` 相当）。
///
/// codex は system prompt を `-c developer_instructions=<中身>` で渡すので、
/// コマンド行の中でファイルを読む必要がある。二重引用符で包むのは中身の
/// `$` / `"` / `'` をシェルに再解釈させないため（POSIX / PowerShell とも
/// 部分式展開は `"$( )"` の形で通る）
pub fn file_contents_expr(dialect: ShellDialect, path: &str) -> String {
    match dialect {
        ShellDialect::Posix => format!("\"$(cat {})\"", quote(dialect, path)),
        // Get-Content -Raw は改行を保ったまま 1 文字列で返す（cat 相当）
        ShellDialect::PowerShell => {
            format!("\"$(Get-Content -Raw -LiteralPath {})\"", ps_quote(path))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: ShellDialect = ShellDialect::PowerShell;
    const SH: ShellDialect = ShellDialect::Posix;

    #[test]
    fn posix側の出力は従来と1バイトも変わらない() {
        // sh_quote 相当（必要なときだけ引用）
        assert_eq!(quote(SH, "claude-opus-5"), "claude-opus-5");
        assert_eq!(quote(SH, "worker:p7b"), "'worker:p7b'");
        assert_eq!(quote(SH, "it's"), "'it'\\''s'");
        // 直書き `'{x}'` 相当（常に引用）
        assert_eq!(quote_always(SH, "/tmp/p.md"), "'/tmp/p.md'");
        assert_eq!(quote_always(SH, "master"), "'master'");
        // 前置き
        assert_eq!(
            unset_prefix(SH, "CLAUDE_CONFIG_DIR"),
            "unset CLAUDE_CONFIG_DIR; "
        );
        assert_eq!(export_prefix(SH, "K", "v"), "export K=v; ");
        assert_eq!(export_prefix(SH, "K", "a b"), "export K='a b'; ");
        assert_eq!(
            inline_env_prefix(SH, "TAKO_ORCHESTRATOR_ROLE", "'worker:x'"),
            "TAKO_ORCHESTRATOR_ROLE='worker:x' "
        );
    }

    #[test]
    fn powershellはインライン前置きを代入へ置き換える() {
        // ここが #867 の本体。`VAR=v cmd` は PowerShell で「コマンド名 VAR=v」に化ける
        assert_eq!(
            inline_env_prefix(PS, "TAKO_ORCHESTRATOR_ROLE", "'worker:x'"),
            "$env:TAKO_ORCHESTRATOR_ROLE='worker:x'; "
        );
        assert!(!inline_env_prefix(PS, "K", "'v'").contains("K='v' claude"));
    }

    #[test]
    fn powershellのexportとunset() {
        assert_eq!(export_prefix(PS, "K", "v"), "$env:K='v'; ");
        assert_eq!(
            unset_prefix(PS, "CLAUDE_CONFIG_DIR"),
            "Remove-Item -LiteralPath 'Env:CLAUDE_CONFIG_DIR' -ErrorAction SilentlyContinue; "
        );
        // POSIX の `export` / `unset` が漏れていない
        for s in [export_prefix(PS, "K", "v"), unset_prefix(PS, "K")] {
            assert!(!s.starts_with("export "), "{s}");
            assert!(!s.starts_with("unset "), "{s}");
        }
    }

    #[test]
    fn powershellは常に引用して裸トークンの構文事故を避ける() {
        // 裸だと `,` が配列区切り・`;` が文区切りになる
        assert_eq!(quote(PS, "a,b"), "'a,b'");
        assert_eq!(quote(PS, "x;y"), "'x;y'");
        // 素の値でも引用する（判断を増やさない）
        assert_eq!(quote(PS, "claude-opus-5"), "'claude-opus-5'");
    }

    #[test]
    fn powershellの単引用符は二重化で表す() {
        // POSIX の `'\''` は PowerShell では**リテラルのバックスラッシュ**になる
        assert_eq!(quote(PS, "it's"), "'it''s'");
        assert_eq!(export_prefix(PS, "K", "it's"), "$env:K='it''s'; ");
        assert!(!quote(PS, "it's").contains('\\'));
    }

    #[test]
    fn 日本語ラベルのroleでも壊れない() {
        // worker の label は TAKO_ORCHESTRATOR_ROLE にそのまま入る（#640 の実測）
        let role = "worker:検証:日本語ラベル";
        assert_eq!(
            inline_env_prefix(PS, "TAKO_ORCHESTRATOR_ROLE", &quote(PS, role)),
            "$env:TAKO_ORCHESTRATOR_ROLE='worker:検証:日本語ラベル'; "
        );
        assert_eq!(
            inline_env_prefix(SH, "TAKO_ORCHESTRATOR_ROLE", &quote(SH, role)),
            "TAKO_ORCHESTRATOR_ROLE='worker:検証:日本語ラベル' "
        );
    }

    #[test]
    fn ファイル読み込み式も方言で切り替わる() {
        assert_eq!(
            file_contents_expr(SH, "/tmp/p.md"),
            "\"$(cat /tmp/p.md)\"",
            "POSIX 側は従来の cat のまま"
        );
        let ps = file_contents_expr(PS, "C:\\Users\\x\\p.md");
        assert_eq!(
            ps,
            "\"$(Get-Content -Raw -LiteralPath 'C:\\Users\\x\\p.md')\""
        );
        assert!(
            !ps.contains("cat "),
            "PowerShell に cat 相当の外部依存を残さない: {ps}"
        );
    }

    /// 起動コマンドでは「知らないシェルを POSIX へ倒す」という**方針**を固定する。
    ///
    /// 判定そのものは `shell_dialect` の単体テストが網羅しているので、ここは
    /// 呼び出し側の方針だけを見る（`None` をどう扱うかは用途で変わる）
    #[test]
    fn 知らないシェルは起動コマンドではposixへ倒す() {
        for p in ["/usr/bin/fish", "cmd.exe", "C:\\Windows\\system32\\cmd.exe"] {
            assert!(
                ShellDialect::from_program(p).is_none(),
                "{p} は方言を決められない（セルフテストは対象外にできる）"
            );
            assert_eq!(
                ShellDialect::from_program(p).unwrap_or(ShellDialect::Posix),
                SH,
                "起動コマンドは止めずに POSIX で組む: {p}"
            );
        }
        assert_eq!(ShellDialect::from_program("/bin/zsh"), Some(SH));
        assert_eq!(ShellDialect::from_program("pwsh.exe"), Some(PS));
    }

    /// 方言を表す enum が**ワークスペースで 1 つだけ**であることを固定する（#873 の番犬）。
    ///
    /// #867 では #865 の未マージを避けるため一時的に 2 本あった。同じことが起きたときに
    /// 「片方だけ直して片方が古いまま」になるのを止めるため、定義の数をソースで数える
    #[test]
    fn 方言のenum定義はワークスペースに1つだけ() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .to_path_buf();
        let mut found: Vec<String> = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let path = e.path();
                if path.is_dir() {
                    // target/ は生成物なので見ない
                    if path.file_name().is_some_and(|n| n == "target") {
                        continue;
                    }
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    for line in text.lines() {
                        let t = line.trim_start();
                        // 「シェルの方言」を表す enum の定義だけを数える
                        if t.starts_with("pub enum ") || t.starts_with("enum ") {
                            let name = t
                                .trim_start_matches("pub ")
                                .trim_start_matches("enum ")
                                .split(|c: char| !c.is_alphanumeric() && c != '_')
                                .next()
                                .unwrap_or("");
                            if name.contains("Dialect") || name.contains("LaunchSyntax") {
                                found.push(format!("{}: {name}", path.display()));
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(
            found.len(),
            1,
            "シェル方言の enum は 1 つだけ（#873）。見つかったもの: {found:#?}"
        );
        assert!(
            found[0].contains("shell_dialect.rs"),
            "定義は platform::shell_dialect が正: {found:?}"
        );
    }

    /// クォート規則が 2 本あることを**意図として固定する**（#873）。
    ///
    /// `quote_arg`（セルフテスト向け）へ寄せると起動コマンドの**見える文字列**が変わる
    /// （spawn 応答の `command` / レジストリの `resume_command`）。実測で相違が出る入力を
    /// 並べて、うっかり統合されたらここが落ちるようにしてある
    #[test]
    fn クォート規則は2本あることを固定する() {
        for (input, launch, selftest) in [
            ("worker:p867", "'worker:p867'", "worker:p867"),
            ("a,b", "'a,b'", "a,b"),
            ("x@y", "'x@y'", "x@y"),
        ] {
            assert_eq!(quote(SH, input), launch, "起動コマンド側（sh_quote）");
            assert_eq!(
                SH.quote_arg(input),
                selftest,
                "セルフテスト側（quote_for_shell）"
            );
            assert_ne!(
                quote(SH, input),
                SH.quote_arg(input),
                "2 本あることが前提の設計（統合するなら見える文字列の変更として別途判断）"
            );
        }
        // Unicode の扱いは逆（sh_quote は素通し / quote_for_shell は引用）
        assert_eq!(quote(SH, "検証"), "検証");
        assert_eq!(SH.quote_arg("検証"), "'検証'");
    }
}
