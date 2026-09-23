//! Code Runner の拡張子既定表（OS 別。Issue #1655）
//!
//! **何のためにあるか**: 組み込み既定は 1 枚の表しか無く、中身が丸ごと POSIX の形
//! （`python3` / `cc` / `c++` / `./<出力>` / `bash`）だった。Windows では
//! **21 種のうち 10 種がそのまま不成立**で、再生ボタンを押すと
//! 「そんなコマンドは無い」で終わる。`.ps1` / `.bat` のように Windows でこそ
//! 走らせたい拡張子には、そもそも既定が無かった。
//!
//! ## 分岐は `cfg!`・表は両 OS ぶんここに置く（#1616 の作法）
//!
//! 属性の `#[cfg(windows)]` で出し分けると、**macOS のビルドに Windows の表が
//! 存在しない**ので、macOS の単体テストから「Windows で何に解決されるか」を
//! 1 マスも検査できない。ここでは [`Platform`] を**引数で受ける純粋関数**にして、
//! 両腕をどちらの OS でもコンパイルする。実行中の OS が要るのは
//! [`Platform::current`] を呼ぶ最後の 1 行だけで、その中身が唯一の `cfg!` になる。
//!
//! ## Windows 側は PowerShell 5.1 でも通る形だけを書く
//!
//! 実行ペインを起こすのは [`super::shell::run_pane_command`] で、Windows では
//! **PowerShell**（既定シェルが pwsh 7 ならそれ、でなければ同梱の
//! `powershell.exe` = 5.1）へスクリプトとして渡る。したがって:
//!
//! - **`&&` を書かない**。pwsh 7 専用の構文で、5.1 では構文エラーになる
//!   （[`super::shell_dialect::ShellDialect::PowerShell`] の規約と同じ）。
//!   「コンパイルしてから走らせる」は `;` と `if ($?) { … }` で書く
//! - **`./` を書かない**。PowerShell のカレントディレクトリ指定は `.\`
//! - コンパイル系の**出力名は `.exe`** にする。付けないと生成物が拡張子なしで
//!   落ち、続く行から起動できない
//!
//! `if ($?) { … }` は実行ペインの終了コード判定
//! （`platform::shell` の `powershell_exit_code_script`）とも噛み合う。
//! コンパイルが失敗すれば本体は走らず `$?` は偽のまま、`$LASTEXITCODE` に
//! コンパイラの終了コードが残るので、ペインのマーカー行はその値を出す。
//!
//! ## 既定を置かない拡張子は「消さず、理由を持たせる」
//!
//! 対応マトリクス（[`super::support`]）と同じ思想で、**表から消さない**。
//! 消すと「その拡張子は tako が知らない」と「その OS では既定を置かないと決めた」の
//! 区別が付かず、理由もユーザーへ届かない。[`Builtin::Absent`] は理由を
//! [`Note`]（日英）で持ち、`tako run` が実行コマンドを見つけられなかったときの
//! 案内へそのまま載る。

use super::support::{Note, Platform};

/// ある OS でのその拡張子の既定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    /// 実行コマンドのテンプレート。`${fileBase}` 等は `runner::expand_variables` が展開する
    Command(&'static str),
    /// この OS では既定を置かない。値は**なぜ置かないか**（ユーザーへ出す案内）
    Absent(Note),
}

impl Builtin {
    pub fn command(self) -> Option<&'static str> {
        match self {
            Self::Command(c) => Some(c),
            Self::Absent(_) => None,
        }
    }

    pub fn absent_reason(self) -> Option<Note> {
        match self {
            Self::Command(_) => None,
            Self::Absent(n) => Some(n),
        }
    }
}

/// 拡張子 1 つぶんの行（両 OS を並べて持つ）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// 小文字の拡張子（`resolve` 側も小文字化して引く）
    pub ext: &'static str,
    pub macos: Builtin,
    pub windows: Builtin,
}

impl Entry {
    pub fn get(&self, platform: Platform) -> Builtin {
        match platform {
            Platform::MacOs => self.macos,
            Platform::Windows => self.windows,
        }
    }
}

/// 両 OS で同じコマンド（OS 差が無い行。表を読むとき**差がある行だけが目に入る**ようにする）
const fn same(ext: &'static str, command: &'static str) -> Entry {
    Entry {
        ext,
        macos: Builtin::Command(command),
        windows: Builtin::Command(command),
    }
}

/// OS で異なる行
const fn split(ext: &'static str, macos: Builtin, windows: Builtin) -> Entry {
    Entry {
        ext,
        macos,
        windows,
    }
}

const fn run(command: &'static str) -> Builtin {
    Builtin::Command(command)
}

const fn absent(ja: &'static str, en: &'static str) -> Builtin {
    Builtin::Absent(Note::new(ja, en))
}

/// どちらの OS でも既定を置かない行（理由も同じ）。**理由を 2 回書かない**
const fn same_absent(ext: &'static str, ja: &'static str, en: &'static str) -> Entry {
    Entry {
        ext,
        macos: absent(ja, en),
        windows: absent(ja, en),
    }
}

/// 組み込み既定の正本（設計 §2.2 の表の後継）。
///
/// 並びは「もとからあった 21 行」→「#1655 で足した 20 行」。
/// `tako run-default` の一覧はキー順に並べ替えて出すので、ここの並びは読みやすさ優先。
pub const TABLE: &[Entry] = &[
    // ─── シェルスクリプト ───────────────────────────────────────────
    // `.command` は Finder でダブルクリックして走らせる macOS 発祥の拡張子だが、
    // **中身はただのシェルスクリプト**なので Windows でも解釈系は bash しかない。
    // 既定を置かない基準は「その OS に解釈系が無い / 決まらない」であって
    // 「その拡張子の慣習が無い」ではない（後者で切ると、同じシェルスクリプトなのに
    // `.sh` は走って `.command` は走らない、という筋の通らない表になる）
    same("command", "bash ${fileBase}"),
    // sh / bash / command は Windows でも `bash` のまま据え置く。`.sh` に対して PowerShell を
    // 当てるのは**常に間違い**で、Git for Windows（Unix ツールを PATH へ通した構成）と
    // WSL の `bash.exe` はどちらもカレントディレクトリを引き継いで相対パスの
    // スクリプトを走らせられる。どちらも無い環境でも「bash が見つからない」という
    // 次の一手が分かる 1 行で落ちるので、既定を置かないより情報が多い
    same("sh", "bash ${fileBase}"),
    same("bash", "bash ${fileBase}"),
    split(
        "zsh",
        run("zsh ${fileBase}"),
        absent(
            "Windows に zsh の標準的な配布が無い（MSYS2 等で入れている場合は `tako run-default zsh \"zsh ${fileBase}\"` で設定できる）",
            "Windows has no standard zsh distribution (if you installed one via MSYS2 or similar, set it with `tako run-default zsh \"zsh ${fileBase}\"`).",
        ),
    ),
    // ─── スクリプト言語 ─────────────────────────────────────────────
    // Windows に `python3` は無い。素の `python3` は Microsoft Store の
    // エイリアススタブ（起動すると Store が開くだけ）へ化けることがあり、
    // 一番たちの悪い失敗になる。`py`（python.org の Launcher）は入っていない
    // 配布（Store 版 / conda / uv）があるので既定にしない（#322 = 最簡形）
    split("py", run("python3 ${fileBase}"), run("python ${fileBase}")),
    same("js", "node ${fileBase}"),
    same("mjs", "node ${fileBase}"),
    same("ts", "npx tsx ${fileBase}"),
    same("rb", "ruby ${fileBase}"),
    same("pl", "perl ${fileBase}"),
    same("php", "php ${fileBase}"),
    same("lua", "lua ${fileBase}"),
    // ─── コンパイル系（Windows は出力を `.exe`・`&&` を使わない） ──────
    // `cc` / `c++` は POSIX の総称名で Windows には無い。MinGW-w64 / MSYS2 /
    // w64devkit のどれで入れても `gcc` / `g++` は付いてくるので、そちらを名指す
    // （MSVC の `cl` は開発者コマンドプロンプトの環境変数が要るので既定にしない）
    split(
        "c",
        run("cc ${fileBase} -o ${fileNoExt} && ./${fileNoExt}"),
        run("gcc ${fileBase} -o ${fileNoExt}.exe; if ($?) { .\\${fileNoExt}.exe }"),
    ),
    split(
        "cpp",
        run("c++ ${fileBase} -o ${fileNoExt} && ./${fileNoExt}"),
        run("g++ ${fileBase} -o ${fileNoExt}.exe; if ($?) { .\\${fileNoExt}.exe }"),
    ),
    split(
        "cc",
        run("c++ ${fileBase} -o ${fileNoExt} && ./${fileNoExt}"),
        run("g++ ${fileBase} -o ${fileNoExt}.exe; if ($?) { .\\${fileNoExt}.exe }"),
    ),
    split(
        "cxx",
        run("c++ ${fileBase} -o ${fileNoExt} && ./${fileNoExt}"),
        run("g++ ${fileBase} -o ${fileNoExt}.exe; if ($?) { .\\${fileNoExt}.exe }"),
    ),
    split(
        "rs",
        run("rustc ${fileBase} -o ${fileNoExt} && ./${fileNoExt}"),
        run("rustc ${fileBase} -o ${fileNoExt}.exe; if ($?) { .\\${fileNoExt}.exe }"),
    ),
    // ランタイムが走らせる形なので OS 差が無い（`go run` / JEP 330 の単一ファイル実行）
    same("go", "go run ${fileBase}"),
    same("java", "java ${fileBase}"),
    same("swift", "swift ${fileBase}"),
    same("tex", "latexmk -pdf -interaction=nonstopmode ${fileBase}"),
    // ─── ここから #1655 で追加 ──────────────────────────────────────
    // `.ps1` は Windows でこそ走らせたいのに既定が無く、再生ボタンが
    // 「実行コマンドが見つからない」で終わっていた（#1655 の代表例）。
    // 実行ポリシーは**既定では回さない**（`-ExecutionPolicy Bypass` を黙って
    // 付けると、ユーザーが設定した安全側の設定を tako が無断で越えることになる）。
    // pwsh 7 が無い Windows では `tako run-default ps1 "powershell -NoLogo -NoProfile -File ${fileBase}"`
    same("ps1", "pwsh -NoLogo -NoProfile -File ${fileBase}"),
    split(
        "bat",
        absent(
            "`.bat` は Windows のバッチファイルで、macOS には解釈する `cmd.exe` が無い",
            "`.bat` is a Windows batch file; macOS has no `cmd.exe` to interpret it.",
        ),
        run("cmd /c ${fileBase}"),
    ),
    split(
        "cmd",
        absent(
            "`.cmd` は Windows のバッチファイルで、macOS には解釈する `cmd.exe` が無い",
            "`.cmd` is a Windows batch file; macOS has no `cmd.exe` to interpret it.",
        ),
        run("cmd /c ${fileBase}"),
    ),
    // `.jsx` を素の `node` へ渡すと JSX の構文で落ちる（node は JSX を解釈しない）。
    // `tsx` は esbuild 経由で `.jsx` / `.tsx` の両方をそのまま走らせられる
    same("tsx", "npx tsx ${fileBase}"),
    same("jsx", "npx tsx ${fileBase}"),
    // Kotlin の単体ファイルは「コンパイルしてから JVM で走らせる」以外の道が無い
    // （`kotlin` コマンドは compiled jar と `.main.kts` 用で `.kt` のソースは走らせない）。
    // 重いが、既定が無いより「時間はかかるが動く」ほうが次の一手に繋がる
    split(
        "kt",
        run("kotlinc ${fileBase} -include-runtime -d ${fileNoExt}.jar && java -jar ${fileNoExt}.jar"),
        run("kotlinc ${fileBase} -include-runtime -d ${fileNoExt}.jar; if ($?) { java -jar ${fileNoExt}.jar }"),
    ),
    // .NET 10 の file-based apps（`dotnet run app.cs`）。プロジェクトとしての実行は #1656
    same("cs", "dotnet run ${fileBase}"),
    // F# は FSI がソースをそのまま走らせる（`.fsx` でなくてもよい）
    same("fs", "dotnet fsi ${fileBase}"),
    same_absent(
        "vb",
        ".NET の単体ファイル実行（file-based apps）は C# だけで、VB は `.vbproj` を持つプロジェクトが要る（プロジェクトとしての実行は #1656）",
        "The .NET file-based app runner supports C# only; VB requires a project with a `.vbproj` (project-aware running is tracked in #1656).",
    ),
    same("dart", "dart run ${fileBase}"),
    same("r", "Rscript ${fileBase}"),
    same("jl", "julia ${fileBase}"),
    same("scala", "scala-cli run ${fileBase}"),
    same("hs", "runghc ${fileBase}"),
    same("clj", "clj -M ${fileBase}"),
    same("ex", "elixir ${fileBase}"),
    same("zig", "zig run ${fileBase}"),
    same("nim", "nim r ${fileBase}"),
    same_absent(
        "sql",
        "接続先の DB が決まらない（`sqlite3` / `psql` / `mysql` で必要な引数も違う）。`tako run-default sql \"sqlite3 app.db < ${fileBase}\"` のように環境に合わせて設定する",
        "There is no way to know which database to connect to (`sqlite3` / `psql` / `mysql` each need different arguments). Set one for your environment, e.g. `tako run-default sql \"sqlite3 app.db < ${fileBase}\"`.",
    ),
    same_absent(
        "ipynb",
        "ノートブックは端末で走らせるより開いて編集するものなので、既定の実行形が一意に決まらない（走らせたい場合は `tako run-default ipynb \"jupyter execute ${fileBase}\"`）",
        "A notebook is meant to be opened and edited rather than run in a terminal, so there is no single obvious default (to run one, use `tako run-default ipynb \"jupyter execute ${fileBase}\"`).",
    ),
];

/// その OS の組み込み既定（`Absent` の行は落ちる）。
///
/// **`Platform` を引数で受ける**ので、macOS の単体から Windows の表を丸ごと検査できる
pub fn builtin_defaults(platform: Platform) -> Vec<(&'static str, &'static str)> {
    TABLE
        .iter()
        .filter_map(|e| e.get(platform).command().map(|c| (e.ext, c)))
        .collect()
}

/// その OS で既定を置いていない理由（置いてあるなら `None`）。
///
/// 表に無い拡張子も `None`。「知らない拡張子」と「置かないと決めた拡張子」は
/// 案内の出し方が違うので、呼び出し側は `Some` のときだけ理由を足す
pub fn absent_reason(platform: Platform, ext: &str) -> Option<Note> {
    entry(ext).and_then(|e| e.get(platform).absent_reason())
}

/// 拡張子の行を引く（小文字で比較する）
pub fn entry(ext: &str) -> Option<&'static Entry> {
    let lower = ext.to_ascii_lowercase();
    TABLE.iter().find(|e| e.ext == lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows 側に POSIX の形が混ざっていないことを**表のデータそのもの**で見る番犬。
    ///
    /// ソースの綴りではなく解決結果を見るので、表の書き方（`same` / `split`）を
    /// 変えても検出力が落ちない
    #[test]
    fn windowsの既定にposixの形が混ざっていない() {
        for (ext, cmd) in builtin_defaults(Platform::Windows) {
            for bad in ["./", "python3", "&&"] {
                assert!(
                    !cmd.contains(bad),
                    "ext '{ext}' の Windows 既定に POSIX / pwsh7 専用の形 '{bad}' が混ざっている: {cmd}\n\
                     （`./` は `.\\`、`python3` は `python`、`&&` は `; if ($?) {{ … }}` へ。\
                     実行ペインの PowerShell は 5.1 へ落ちうる = Issue #1655）"
                );
            }
        }
    }

    /// macOS 側に Windows の形が混ざっていないこと（逆向きも同じ 1 本で見張る）
    #[test]
    fn macosの既定にwindowsの形が混ざっていない() {
        for (ext, cmd) in builtin_defaults(Platform::MacOs) {
            for bad in [".exe", "cmd /c", ".\\"] {
                assert!(
                    !cmd.contains(bad),
                    "ext '{ext}' の macOS 既定に Windows の形 '{bad}' が混ざっている: {cmd}"
                );
            }
        }
    }

    /// コンパイルしてから走らせる行は、Windows では出力も起動も `.exe` になっている
    #[test]
    fn windowsのコンパイル系は出力もexe() {
        for ext in ["c", "cpp", "cc", "cxx", "rs"] {
            let cmd = entry(ext)
                .and_then(|e| e.windows.command())
                .unwrap_or_else(|| panic!("{ext} の Windows 既定が無い"));
            assert!(
                cmd.contains("-o ${fileNoExt}.exe"),
                "{ext}: 出力名が `.exe` でない: {cmd}"
            );
            assert!(
                cmd.contains(".\\${fileNoExt}.exe"),
                "{ext}: 生成物を `.\\<名前>.exe` で起動していない: {cmd}"
            );
            assert!(
                cmd.contains("if ($?)"),
                "{ext}: 失敗したコンパイルの後でも走ってしまう（`if ($?)` が無い）: {cmd}"
            );
        }
    }

    /// シェルスクリプトの拡張子は**両 OS とも既定を持つ**。
    ///
    /// `.command` / `.sh` / `.bash` は中身がただのシェルスクリプトで、Windows でも
    /// 解釈系は bash しかない（PowerShell を当てるのは常に誤り）。「macOS 発祥の
    /// 拡張子だから Windows では置かない」と切ると、**同じシェルスクリプトなのに
    /// `.sh` は走って `.command` は走らない**という筋の通らない表になり、
    /// `Run` を通る経路（dispatch の実行テスト）が Windows でだけ落ちる。
    /// これは実際に #1655 の作業中に CI の Windows で踏んだ（`.command` が Err）
    #[test]
    fn シェルスクリプトの拡張子は両osで既定を持つ() {
        for ext in ["command", "sh", "bash"] {
            let e = entry(ext).unwrap_or_else(|| panic!("{ext} が表に無い"));
            for (label, platform) in [("macos", Platform::MacOs), ("windows", Platform::Windows)] {
                let cmd = e
                    .get(platform)
                    .command()
                    .unwrap_or_else(|| panic!(".{ext} の {label} 既定が無い"));
                assert!(
                    cmd.starts_with("bash "),
                    ".{ext} の {label} 既定が bash でない: {cmd}"
                );
            }
        }
    }

    #[test]
    fn 拡張子の重複が無い() {
        let mut seen = std::collections::HashSet::new();
        for e in TABLE {
            assert!(seen.insert(e.ext), "拡張子 '{}' が重複", e.ext);
            assert_eq!(
                e.ext.to_ascii_lowercase(),
                e.ext,
                "拡張子 '{}' が小文字でない（引く側は小文字化して比較する）",
                e.ext
            );
        }
    }

    /// 既定を置かない行は**必ず理由を持つ**（日英とも空でない）
    #[test]
    fn 既定を置かない行には理由がある() {
        let mut absent_count = 0usize;
        for e in TABLE {
            for (label, b) in [("macos", e.macos), ("windows", e.windows)] {
                if let Some(note) = b.absent_reason() {
                    absent_count += 1;
                    assert!(
                        !note.ja().trim().is_empty() && !note.en().trim().is_empty(),
                        "{}({label}) の理由が空",
                        e.ext
                    );
                }
            }
        }
        assert!(
            absent_count >= 8,
            "理由つきで既定を置かない行が減りすぎている（{absent_count} 件）"
        );
    }

    /// 表の行数と、OS ごとに実際に既定が引ける件数
    #[test]
    fn 表の規模() {
        assert_eq!(TABLE.len(), 41, "拡張子の行数");
        assert_eq!(builtin_defaults(Platform::MacOs).len(), 36);
        assert_eq!(builtin_defaults(Platform::Windows).len(), 37);
    }

    #[test]
    fn 理由は置いていない拡張子にだけ付く() {
        assert!(absent_reason(Platform::Windows, "zsh").is_some());
        // `.command` は両 OS とも既定を持つ（理由は要らない）
        assert!(absent_reason(Platform::Windows, "command").is_none());
        assert!(absent_reason(Platform::MacOs, "zsh").is_none());
        assert!(absent_reason(Platform::MacOs, "bat").is_some());
        assert!(absent_reason(Platform::Windows, "bat").is_none());
        // 表に無い拡張子は「置かないと決めた」ではないので理由を持たない
        assert!(absent_reason(Platform::Windows, "xyz").is_none());
        // 大文字で来ても引ける
        assert!(entry("PS1").is_some());
    }
}
