//! 番犬: tako CLI の PATH 設置（FR-2.14.5 / #1502）が外れたまま緑にならないようにする
//!
//! ## 何が起きていたのか
//!
//! `tako setup` は**エージェント CLI の PATH** しか通しておらず、tako 自身の CLI を
//! PATH へ置くコードが 0 件だった。`check-health` が「このディレクトリを PATH に
//! 追加すること」と案内するだけで、`tako setup` / `--check` は 1 行も出さない。
//! 外部ターミナル（Terminal.app / iTerm2）で `tako` を打つには人が自分で
//! `~/.zprofile` を編集する必要があり、FR-2.14.5 が未実装のまま残っていた。
//!
//! ## ここで止める 4 つ
//!
//! 1. **段の呼び忘れ**。設置は `tako setup` の 1 行と `tako setup --check` の 1 行で
//!    正本（tako-control）へ入る。**この 2 行は消えやすい**（`setup.rs` は
//!    #1499 / #1501 / #1503… が同じ関数を順番に触る）ので、呼んでいることを縛る
//! 2. **設置先の退行**。PATH へ通すのは**安定した symlink の置き場所**であって、
//!    実体のディレクトリ（`.app` の `Contents/MacOS` や dev の `target/debug`）ではない。
//!    直接通すと同居物ごと PATH へ出るうえ `.app` を動かすと黙って切れる
//! 3. **ブロックの増殖**。`~/.zprofile` のマーカーは**1 組**のままで、中に
//!    ディレクトリを並べる（2 組目を足すと `undo-path` が片方しか外せない）
//! 4. **判定がプロセスの PATH へ戻ること**。「もう通っているか」は
//!    **新しいターミナルが見る PATH** で測る。自分のプロセスの PATH で測ると、
//!    tako のペインは #601 で CLI ディレクトリを注入済みなので必ず「通っている」に
//!    見えて、設置が丸ごと飛ぶ（#1502 の実測で踏んだ）
//!
//! 落ちるときは **file:line で名指し**する（直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! 実経路（隔離 HOME で `tako setup` → `zsh -l -c 'command -v tako'`）は
//! `scripts/test-tako-cli-path-1502.sh`。設置の判断そのもの（symlink の状態遷移・
//! Windows 分岐）は `tako_core::tako_cli_path` の単体テスト。ここは
//! **配線が外れていないこと**だけを見る。

use std::path::{Path, PathBuf};

use tako_core::shell_profile::{self, ShellKind};
use tako_core::source_scan::{fn_head_name, is_top_level_fn_head};
use tako_core::tako_cli_path;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリのルートを解決できる")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// `needle` を含む**コメントでない**行を、それを囲むトップレベル関数名つきで拾う。
/// 関数名の追跡は `source_scan` の 1 実装（#1496。`pub fn` / `async fn` を取りこぼさない）
fn hits_in_fn(text: &str, needle: &str) -> Vec<(usize, String)> {
    let mut current = String::new();
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if is_top_level_fn_head(line) {
            current = fn_head_name(line).unwrap_or("").to_string();
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("///") {
            continue;
        }
        if line.contains(needle) {
            out.push((i + 1, current.clone()));
        }
    }
    out
}

/// 1: `tako setup` の本体が設置の段を呼んでいる（#1502 の配線）
#[test]
fn setupが_tako_cliのpath設置を呼んでいる() {
    let rel = "crates/tako-cli/src/setup.rs";
    let text = read(rel);
    let hits = hits_in_fn(&text, "run_tako_cli_path_stage");
    assert!(
        !hits.is_empty(),
        "{rel}: `tako setup` が tako CLI の PATH 設置を呼んでいない（FR-2.14.5 / #1502）。\n\
         `run_setup` の中で \
         `eprintln!(\"{{}}\", setup_bootstrap::run_tako_cli_path_stage());` を呼ぶこと。\n\
         これが無いと外部ターミナルから `tako` が打てないまま setup が完了する"
    );
    let in_run_setup = hits.iter().any(|(_, f)| f == "run_setup");
    assert!(
        in_run_setup,
        "{rel}: tako CLI の PATH 設置の呼び出しが `run_setup` の外に居る（見つかった場所: {}）。\n\
         `tako setup` の本流から必ず通る位置へ置くこと（#1502）",
        hits.iter()
            .map(|(line, f)| format!("{rel}:{line}（{f}）"))
            .collect::<Vec<_>>()
            .join(" / ")
    );
}

/// 1': `tako setup --check` が設置状況を報告している
#[test]
fn setup_checkが_tako_cliのpathを報告している() {
    let rel = "crates/tako-cli/src/setup.rs";
    let text = read(rel);
    let hits = hits_in_fn(&text, "tako_cli_path_check_line");
    assert!(
        !hits.is_empty(),
        "{rel}: `tako setup --check` が tako CLI の PATH を報告していない（#1502 / Z19）。\n\
         `run_check` の中で \
         `eprintln!(\"{{}}\", setup_bootstrap::tako_cli_path_check_line());` を呼ぶこと"
    );
    assert!(
        hits.iter().any(|(_, f)| f == "run_check"),
        "{rel}: 報告の呼び出しが `run_check` の外に居る（見つかった場所: {}）",
        hits.iter()
            .map(|(line, f)| format!("{rel}:{line}（{f}）"))
            .collect::<Vec<_>>()
            .join(" / ")
    );
}

/// 2: PATH へ通すのは symlink の置き場所であって、実体のディレクトリではない
#[test]
fn pathへ通すのは安定した置き場所で実体のディレクトリではない() {
    let home = Path::new("/home/testuser");
    // `.app` 同梱 / dev ビルド / zip 展開 — どれも「同居物ごと PATH へ出す」形にしない
    for cli in [
        PathBuf::from("/Applications/tako.app/Contents/MacOS/tako"),
        home.join("dev/tako/target/debug/tako"),
        home.join("Downloads/tako.app/Contents/MacOS/tako"),
    ] {
        let plan = tako_cli_path::plan_for(home, &cli, false);
        let parent = cli.parent().expect("親ディレクトリ");
        assert_ne!(
            plan.dir,
            parent,
            "crates/tako-core/src/tako_cli_path.rs: {} の実体のディレクトリを PATH へ入れようとしている。\n\
             同居物（tako-app / ビルド生成物）ごとユーザーの PATH へ出るうえ、\n\
             `.app` を動かすと黙って切れる（#1502 の設計の芯）",
            cli.display()
        );
        assert_eq!(
            plan.dir,
            home.join(tako_cli_path::LINK_DIR_REL),
            "crates/tako-core/src/tako_cli_path.rs: 置き場所が $HOME/{} から動いている",
            tako_cli_path::LINK_DIR_REL
        );
        // 名前の正本は `shell_integration::cli_file_name`（Windows は `tako.exe`）。
        // `"tako"` 直書きは Windows で必ず外れる（#1278）
        assert_eq!(
            plan.link.as_deref(),
            Some(
                home.join(tako_cli_path::LINK_DIR_REL)
                    .join(tako_core::shell_integration::cli_file_name())
                    .as_path()
            ),
            "crates/tako-core/src/tako_cli_path.rs: symlink を張らなくなっている"
        );
    }
}

/// 3: マーカーブロックは 1 組のまま（中にディレクトリを並べる）
#[test]
fn ブロックは何ディレクトリでも一組のまま() {
    let home = std::env::temp_dir().join(format!(
        "tako-1502-watchdog-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("一時ディレクトリを作れる");

    let dirs = vec![
        home.join(".local/bin"),
        PathBuf::from("/opt/tako/bin"),
        home.join(".local/bin"), // 重複（macOS の 3 系統 + tako が同じ置き場所）
    ];
    let out = shell_profile::ensure_dirs_on_path_in(&home, ShellKind::Zsh, &dirs, Some("/usr/bin"))
        .expect("書ける");
    let text = std::fs::read_to_string(&out.profile).expect("読める");
    assert_eq!(
        text.matches("# >>> tako PATH >>>").count(),
        1,
        "crates/tako-core/src/shell_profile.rs: マーカーが 1 組でない。\n\
         2 組目ができると `undo-path` が片方しか外せない（#1502）:\n{text}"
    );
    assert_eq!(text.matches("# <<< tako PATH <<<").count(), 1);
    assert_eq!(out.dirs.len(), 2, "重複が落ちていない: {:?}", out.dirs);

    assert!(
        home.starts_with(std::env::temp_dir()),
        "一時ディレクトリ以外を消そうとした: {}",
        home.display()
    );
    let _ = std::fs::remove_dir_all(&home);
}

/// 4: 「もう通っているか」を**新しいターミナルが見る PATH**で測っている
#[test]
fn 通っているかの判定がプロセスのpathへ戻っていない() {
    let rel = "crates/tako-control/src/setup_bootstrap.rs";
    let text = read(rel);

    // ログインシェルの PATH を読む口が在ること（#1502 で分けた 1 実装）
    assert!(
        text.contains("fn login_shell_path()"),
        "{rel}: `login_shell_path()` が消えている。\n\
         「新しいターミナルが見る PATH」を読む口が無いと、判定が自分のプロセスの \
         PATH へ戻る（#601 の注入で必ず「通っている」に見える = #1502 の症状）"
    );
    // その口が PATH を launchd の既定へ戻してから起こすこと（継承したままだと
    // macOS の path_helper が親の PATH を引き継いで答えが濁る）
    let starts = hits_in_fn(&text, ".env(\"PATH\", DEFAULT_LOGIN_PATH)");
    assert!(
        starts.iter().any(|(_, f)| f == "login_shell_path"),
        "{rel}: `login_shell_path()` がログインシェルへ既定の PATH を渡していない。\n\
         継承したまま起こすと `/etc/zprofile` の path_helper が親の PATH を引き継ぐので、\n\
         答えが「新しいターミナルの PATH」ではなく「この親の PATH」になる（#1502 の実測）"
    );
    // 書くかどうかの判定が `login_shell_path()` を第一候補にしていること
    let decides = hits_in_fn(&text, "login_shell_path()");
    assert!(
        decides.iter().any(|(_, f)| f == "ensure_dirs_on_path"),
        "{rel}: `ensure_dirs_on_path` が「通っているか」をプロセスの PATH で決めている。\n\
         見つかった `login_shell_path()` の呼び出し: {}",
        decides
            .iter()
            .map(|(line, f)| format!("{rel}:{line}（{f}）"))
            .collect::<Vec<_>>()
            .join(" / ")
    );
    assert!(
        decides.iter().any(|(_, f)| f == "dir_on_path"),
        "{rel}: `dir_on_path`（--check / check-health の表示）がプロセスの PATH で答えている"
    );
}

/// 4': `undo-path` は書いたもの（ブロック + symlink）を両方外す
#[test]
fn undo_pathがsymlinkも外している() {
    let rel = "crates/tako-control/src/setup_bootstrap.rs";
    let text = read(rel);
    let hits = hits_in_fn(&text, "tako_cli_path::remove_link");
    assert!(
        hits.iter().any(|(_, f)| f == "undo_path_for"),
        "{rel}: `undo_path_for` が tako CLI の symlink を外していない（#1502）。\n\
         ブロックだけ消すと `~/.local/bin/tako` が残り、\n\
         「元へ戻せる」という `undo-path` の約束が破れる。\n\
         見つかった呼び出し: {}",
        if hits.is_empty() {
            "（無し）".to_string()
        } else {
            hits.iter()
                .map(|(line, f)| format!("{rel}:{line}（{f}）"))
                .collect::<Vec<_>>()
                .join(" / ")
        }
    );
}
