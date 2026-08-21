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
//! [`LaunchSyntax`] は「どちらの構文で書くか」だけを表す。判定は
//! [`LaunchSyntax::for_program`] の純粋関数なので、**macOS 上から PowerShell 側の
//! 出力を全分岐テストできる**。組み立ても全部純粋関数。
//!
//! ## `platform::shell_dialect`（#865）との関係
//!
//! #865 が**セルフテストが打ち込む文字列**用に同じ判定を持つ境界を作っている。
//! 本来は判定を 1 本にすべきだが、#867 の着手時点で #865 は未マージ・活発に変更中で、
//! あちらのファイルへ同時に触ると必ずコンフリクトする。**#867 は Windows で
//! エージェントが起動できるかどうかというミッションのコア**なので、依存を切って先に出した。
//! 判定の一本化は #865 マージ後の後続作業（担当者と合意済み）。
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

/// 起動コマンドをどちらの構文で書くか。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchSyntax {
    /// sh / bash / zsh（`export K=V; ` / `VAR=v cmd` / `"$(cat p)"`）
    Posix,
    /// PowerShell 5.1 と 7 の**両方**で通る書き方だけを出す
    PowerShell,
}

impl LaunchSyntax {
    /// シェルの実行ファイルパスから構文を選ぶ（純粋関数）。
    ///
    /// 判定は実行ファイル名だけを見る（`/bin/zsh` /
    /// `C:\Program Files\PowerShell\7\pwsh.exe` のどちらでも同じ答えになる）。
    ///
    /// **知らないシェルは POSIX にする**。`cmd.exe` / fish には変換先が無いが、
    /// ここで「対象外」にして組み立てを止めると、これまで動いていた環境で
    /// エージェントが起動しなくなる。fish は元から `export` を持たないので
    /// env つきプロファイルは以前から効いていない（#867 で変わる話ではない）
    pub fn for_program(program: &str) -> Self {
        let file = program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(program)
            .to_ascii_lowercase();
        match file.strip_suffix(".exe").unwrap_or(&file) {
            "pwsh" | "powershell" => Self::PowerShell,
            _ => Self::Posix,
        }
    }
}

/// 起動コマンドを打ち込む先のシェルから構文を引く。
///
/// ペインで起こすシェルが正（`platform::shell::default_shell`）。プロセスの `$SHELL` では
/// なく**実際にペインで動くもの**を見ないと、器や設定でシェルが変わったときに食い違う
pub fn launch_syntax() -> LaunchSyntax {
    match tako_core::platform::shell::default_shell() {
        Some(s) => LaunchSyntax::for_program(&s.program),
        None => LaunchSyntax::Posix,
    }
}

/// 値を「必要なときだけ」引用する（POSIX は `sh_quote` と同一の出力）。
///
/// PowerShell は裸のトークンで `,` が配列区切りになる・`;` `(` `$` `@` が構文になる等、
/// 素で通せる文字の判断が難しいので**常に単引用符で包む**（単引用符の中は完全にリテラル）
pub fn quote(dialect: LaunchSyntax, s: &str) -> String {
    match dialect {
        LaunchSyntax::Posix => crate::orchestrator::agent::sh_quote(s),
        LaunchSyntax::PowerShell => ps_quote(s),
    }
}

/// 値を常に引用する（元コードが `'{x}'` と直書きしていた箇所の置き換え）
pub fn quote_always(dialect: LaunchSyntax, s: &str) -> String {
    match dialect {
        LaunchSyntax::Posix => format!("'{}'", s.replace('\'', "'\\''")),
        LaunchSyntax::PowerShell => ps_quote(s),
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
pub fn unset_prefix(dialect: LaunchSyntax, key: &str) -> String {
    match dialect {
        LaunchSyntax::Posix => format!("unset {}; ", quote(dialect, key)),
        LaunchSyntax::PowerShell => format!(
            "Remove-Item -LiteralPath {} -ErrorAction SilentlyContinue; ",
            ps_quote(&format!("Env:{key}"))
        ),
    }
}

/// 環境変数を**設定**する前置き（末尾に `; ` を含む）。
///
/// どちらの方言でも「この行より後に走るものすべて」に効く。ログインシェルの rc
/// （direnv 等）はコマンド行より先に走るので、後から実行されるこちらが勝つ（#500 / #512）
pub fn export_prefix(dialect: LaunchSyntax, key: &str, value: &str) -> String {
    match dialect {
        LaunchSyntax::Posix => {
            format!("export {}={}; ", quote(dialect, key), quote(dialect, value))
        }
        LaunchSyntax::PowerShell => format!("$env:{key}={}; ", ps_quote(value)),
    }
}

/// 続くコマンド 1 つへ引き継ぐ環境変数の前置き。
///
/// POSIX は**インライン前置き**（`VAR=v cmd`）でそのコマンドだけに効かせる。
/// PowerShell にインライン前置きは無いので代入 + `;` になり、**シェル自身にも残る**。
/// ペインはそのエージェント専用なので実害は無く、むしろ同じペインで撃ち直したときに
/// role が残るぶん都合がよい（`SpawnOptions.env` のプロファイル env も同じくシェルに入る）
pub fn inline_env_prefix(dialect: LaunchSyntax, key: &str, value: &str) -> String {
    match dialect {
        // 元コードは `sh_quote(role)` と `'{role_env}'` の 2 通りがあったので、
        // 呼び出し側が使い分けられるよう値のクォートは渡す前に済ませてもらう
        LaunchSyntax::Posix => format!("{key}={value} "),
        LaunchSyntax::PowerShell => format!("$env:{key}={value}; "),
    }
}

/// ファイルの中身を 1 引数として埋め込む式（`"$(cat p.md)"` 相当）。
///
/// codex は system prompt を `-c developer_instructions=<中身>` で渡すので、
/// コマンド行の中でファイルを読む必要がある。二重引用符で包むのは中身の
/// `$` / `"` / `'` をシェルに再解釈させないため（POSIX / PowerShell とも
/// 部分式展開は `"$( )"` の形で通る）
pub fn file_contents_expr(dialect: LaunchSyntax, path: &str) -> String {
    match dialect {
        LaunchSyntax::Posix => format!("\"$(cat {})\"", quote(dialect, path)),
        // Get-Content -Raw は改行を保ったまま 1 文字列で返す（cat 相当）
        LaunchSyntax::PowerShell => {
            format!("\"$(Get-Content -Raw -LiteralPath {})\"", ps_quote(path))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: LaunchSyntax = LaunchSyntax::PowerShell;
    const SH: LaunchSyntax = LaunchSyntax::Posix;

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

    #[test]
    fn シェル名から構文を選ぶ() {
        for p in [
            "/bin/zsh",
            "/bin/bash",
            "/bin/sh",
            "/usr/bin/fish",
            "cmd.exe",
            "C:\\Windows\\system32\\cmd.exe",
        ] {
            assert_eq!(LaunchSyntax::for_program(p), SH, "{p}");
        }
        for p in [
            "pwsh",
            "pwsh.exe",
            "PowerShell.exe",
            "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
            "C:/Program Files/PowerShell/7/pwsh.exe",
        ] {
            assert_eq!(LaunchSyntax::for_program(p), PS, "{p}");
        }
    }
}
