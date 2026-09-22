//! 外部ターミナルから `tako` を打てるようにする設置（FR-2.14.5 / #1502）
//!
//! tako 内側のシェルは [`crate::shell_integration`] が PATH を注入するので
//! `tako` が打てる（FR-2.4.6 / #601）。**外側のターミナル**（Terminal.app / iTerm2 /
//! VS Code の統合ターミナル）には届かないので、ここがその 1 点を引き受ける。
//!
//! ## なぜ「CLI の実体があるディレクトリ」を直接 PATH へ通さないか（unix）
//!
//! tako CLI の実体は配布形態ごとに別の場所に居る。
//!
//! | 形態 | 実体 | そのディレクトリを PATH へ入れると |
//! |---|---|---|
//! | `.app` | `/Applications/tako.app/Contents/MacOS/tako` | `tako-app` も一緒に PATH へ出る。**`.app` を動かすと黙って切れる** |
//! | dev ビルド | `<repo>/target/debug/tako` | ビルド生成物と依存クレートの実行ファイルが**丸ごと** PATH へ出る |
//! | zip 展開 | 展開先のどこか | 展開先を片付けると切れる |
//!
//! どれも「ユーザーの PATH へ入れてよいディレクトリ」ではない。なので
//! **安定した 1 点（`$HOME/.local/bin/tako`）に symlink を張り、PATH へ通すのは
//! その置き場所だけ**にする。利点が 3 つある。
//!
//! 1. PATH へ出るのは `tako` **1 個**だけ（同居物を巻き込まない）
//! 2. `$HOME/.local/bin` は **claude / codex / agy のランチャーと同じ置き場所**
//!    （[`crate::platform::agent_install`] の macOS の 3 系統とも `.local/bin`）なので、
//!    `~/.zprofile` のマーカーブロックは**今までどおり 1 個・中身も 1 ディレクトリ**のまま
//! 3. `.app` を**その場で更新**しても（`tako update apply` は同じパスへ入れ替える）
//!    symlink の指す先は変わらないので切れない。**移動・削除**されたときだけ
//!    [`LinkState::Dangling`] になり、次の起動（tako-app）か `tako setup` /
//!    `tako setup bootstrap path` が張り直す
//!
//! ## Windows は symlink を使わない（境界 B23 側へ寄せる）
//!
//! Windows の symlink は管理者権限か開発者モードを要求するので、セットアップが
//! 黙って張れない。そのかわり Windows の tako は**インストーラが専用ディレクトリ**
//! （`%LOCALAPPDATA%\Programs\tako`）へ入れるので、そのディレクトリを
//! ユーザー環境変数 `Path`（`HKCU\Environment`）へ足せばよい。同居するのは
//! tako 自身の実行ファイルだけなので unix の 1. の問題も起きない。
//!
//! **Windows 実機では未検証**（コード分岐のみ。#1502）。
//!
//! ## 判断は純粋関数、副作用は薄く
//!
//! 「どこへ何を置くか」は [`plan_for`]、「いま何が置かれているか」は
//! [`classify_link`] が引数だけで決める。実ファイルを触るのは [`ensure_link`] /
//! [`remove_link`] / [`repair_dangling`] の 3 本だけで、どれも純粋関数の判断を実行するだけ

use std::path::{Path, PathBuf};

/// symlink を置くディレクトリ（`$HOME` からの相対）。
///
/// **`/usr/local/bin` は選ばない**: 書き込みに sudo が要る環境があり（Apple Silicon の
/// 素の macOS は `/usr/local` 自体が無い）、セットアップが権限昇格を求めない
/// という #868 の方針に反する。`$HOME/.local/bin` はエージェント CLI の公式
/// インストーラ 3 種がそろって使う置き場所でもある
pub const LINK_DIR_REL: &str = ".local/bin";

/// PATH から引かせたいコマンド名。正本は [`crate::shell_integration::cli_file_name`]
/// （tako 内側の注入と外側の設置で名前がずれない）
fn cli_file_name() -> String {
    crate::shell_integration::cli_file_name()
}

/// 設置の計画（「どのディレクトリを PATH へ通すか」と「symlink を張るか」）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// PATH へ通すディレクトリ
    pub dir: PathBuf,
    /// 張る symlink の位置（Windows / 既に実体がその場所に居る場合は `None`）
    pub link: Option<PathBuf>,
    /// symlink の指す先（= いま動いている tako CLI の実体）
    pub source: PathBuf,
}

/// 設置計画を組む純粋関数。
///
/// `home` = ホームディレクトリ / `cli` = いま動いている tako CLI の実体 /
/// `windows` = Windows か（`cfg!(windows)` を引数で受けるので、macOS の
/// `cargo test` から Windows 側の形を検証できる）
pub fn plan_for(home: &Path, cli: &Path, windows: bool) -> Placement {
    if windows {
        // 実体のディレクトリ（インストーラ管理の専用ディレクトリ）をそのまま通す
        return Placement {
            dir: cli.parent().unwrap_or(cli).to_path_buf(),
            link: None,
            source: cli.to_path_buf(),
        };
    }
    let dir = rel_join(home, LINK_DIR_REL);
    let link = dir.join(cli_file_name());
    Placement {
        // 実体が既に置き場所そのものに居るなら symlink は要らない
        // （パッケージマネージャ等がそこへ入れた場合。自分自身を指す symlink を作らない）
        link: (link != cli).then(|| link.clone()),
        dir,
        source: cli.to_path_buf(),
    }
}

/// `/` 区切りの相対パスを結合する（Windows でも `\` に解決される）
fn rel_join(home: &Path, rel: &str) -> PathBuf {
    let mut path = home.to_path_buf();
    for part in rel.split('/') {
        path.push(part);
    }
    path
}

/// symlink の置き場所にいま何があるか
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    /// 何も無い
    Missing,
    /// tako が置いた symlink が狙いどおりの先を指している
    Correct,
    /// symlink はあるが**指す先が消えている**（`.app` を移動・削除した）
    Dangling { target: PathBuf },
    /// symlink はあるが別の場所を指している（別の tako から設置した）
    Other { target: PathBuf },
    /// symlink ではない実体が置かれている。**触らない**
    /// （ユーザーかパッケージマネージャが自分で置いたもの）
    Occupied,
}

impl LinkState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Correct => "correct",
            Self::Dangling { .. } => "dangling",
            Self::Other { .. } => "other",
            Self::Occupied => "occupied",
        }
    }

    /// その場所から `tako` を実行できるか（= PATH に入っていれば打てるか）
    pub fn usable(&self) -> bool {
        matches!(self, Self::Correct | Self::Other { .. } | Self::Occupied)
    }
}

/// [`LinkState`] を引数だけで決める純粋関数。
///
/// `existing` = その場所の symlink が指す先（symlink でなければ `None`）/
/// `present` = その場所に何かあるか / `target_exists` = `existing` の指す先が実在するか
pub fn classify_link(
    want: &Path,
    present: bool,
    existing: Option<&Path>,
    target_exists: bool,
) -> LinkState {
    match (present, existing) {
        (false, _) => LinkState::Missing,
        (true, None) => LinkState::Occupied,
        (true, Some(target)) if target == want => {
            // 狙いどおりの先を指していても、その先が消えていれば使えない
            if target_exists {
                LinkState::Correct
            } else {
                LinkState::Dangling {
                    target: target.to_path_buf(),
                }
            }
        }
        (true, Some(target)) if target_exists => LinkState::Other {
            target: target.to_path_buf(),
        },
        (true, Some(target)) => LinkState::Dangling {
            target: target.to_path_buf(),
        },
    }
}

/// 実ファイルを見て [`LinkState`] を返す
pub fn link_state(link: &Path, want: &Path) -> LinkState {
    let present = std::fs::symlink_metadata(link).is_ok();
    let existing = std::fs::read_link(link).ok();
    let target_exists = existing.as_deref().is_some_and(|t| {
        let absolute = if t.is_absolute() {
            t.to_path_buf()
        } else {
            link.parent().unwrap_or(Path::new(".")).join(t)
        };
        absolute.exists()
    });
    classify_link(want, present, existing.as_deref(), target_exists)
}

/// symlink に対して行った（行わなかった）こと
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkChange {
    /// 新しく張った
    Created,
    /// 指す先を張り替えた
    Updated,
    /// 既に正しいので触っていない
    Unchanged,
    /// symlink ではない実体が居るので触らなかった
    Skipped,
    /// 取り除いた
    Removed,
    /// もともと無かった
    Absent,
}

impl LinkChange {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
            Self::Skipped => "skipped",
            Self::Removed => "removed",
            Self::Absent => "absent",
        }
    }

    pub fn wrote(self) -> bool {
        matches!(self, Self::Created | Self::Updated | Self::Removed)
    }
}

/// `link` が `want` を指すようにする（冪等）。
///
/// **symlink ではない実体が居る場所は触らない**（[`LinkChange::Skipped`]）。
/// ユーザーが自分で置いた `tako` を消す権利はセットアップに無い
pub fn ensure_link(link: &Path, want: &Path) -> Result<LinkChange, String> {
    match link_state(link, want) {
        LinkState::Correct => Ok(LinkChange::Unchanged),
        LinkState::Occupied => Ok(LinkChange::Skipped),
        LinkState::Missing => {
            create_parent(link)?;
            create_symlink(want, link)
                .map_err(|e| format!("{} を作成できません: {e}", link.display()))?;
            Ok(LinkChange::Created)
        }
        LinkState::Dangling { .. } | LinkState::Other { .. } => {
            std::fs::remove_file(link)
                .map_err(|e| format!("{} を置き換えられません: {e}", link.display()))?;
            create_symlink(want, link)
                .map_err(|e| format!("{} を作成できません: {e}", link.display()))?;
            Ok(LinkChange::Updated)
        }
    }
}

/// 指す先が消えている symlink **だけ**を張り直す（起動時に呼ぶ）。
///
/// 新規に張ることはしない = 一度も設置していない人の `$HOME` を起動のたびに
/// 触らない。`.app` を移動した人の切れたリンクを黙って直すためだけの経路
pub fn repair_dangling(link: &Path, want: &Path) -> Result<LinkChange, String> {
    match link_state(link, want) {
        LinkState::Dangling { .. } => ensure_link(link, want),
        _ => Ok(LinkChange::Unchanged),
    }
}

/// 置いた symlink を取り除く。**symlink 以外は消さない**
pub fn remove_link(link: &Path) -> Result<LinkChange, String> {
    if std::fs::symlink_metadata(link).is_err() {
        return Ok(LinkChange::Absent);
    }
    if std::fs::read_link(link).is_err() {
        // ユーザーが置いた実体。消さない
        return Ok(LinkChange::Skipped);
    }
    std::fs::remove_file(link).map_err(|e| format!("{} を削除できません: {e}", link.display()))?;
    Ok(LinkChange::Removed)
}

fn create_parent(link: &Path) -> Result<(), String> {
    let Some(parent) = link.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("{} を作成できません: {e}", parent.display()))
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Windows では [`plan_for`] が `link: None` を返すのでここへは来ない。
/// 万一呼ばれたら**黙って成功させない**（権限が要ることを理由つきで返す）
#[cfg(not(unix))]
fn create_symlink(_target: &Path, link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!(
            "{} への symlink はこの OS では張れません（ユーザー環境変数 Path を使います）",
            link.display()
        ),
    ))
}

/// いま動いている tako CLI の実体（解決できなければ `None`）。
///
/// 実体の在り処の正本は [`crate::shell_integration::cli_dir`]
/// （実行中バイナリの隣を見る = `.app` / dev ビルド / zip 展開のどれでも当たる）。
/// ここで別の解決を書くと 2 本目の正本ができる
pub fn current_cli() -> Option<PathBuf> {
    Some(crate::shell_integration::cli_dir()?.join(cli_file_name()))
}

/// この環境の設置計画（解決できなければ `None`）
pub fn plan_here(home: &Path) -> Option<Placement> {
    Some(plan_for(home, &current_cli()?, cfg!(windows)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tako-cli-path-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// **一時ディレクトリ配下であることを確認してから消す**
    fn cleanup(dir: &Path) {
        assert!(
            dir.starts_with(std::env::temp_dir()),
            "一時ディレクトリ以外を消そうとした: {}",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unixは安定したsymlink先を選び実体のディレクトリを直接通さない() {
        let home = Path::new("/home/testuser");
        let app = Path::new("/Applications/tako.app/Contents/MacOS/tako");
        let plan = plan_for(home, app, false);
        assert_eq!(plan.dir, home.join(".local/bin"));
        assert_eq!(plan.link, Some(home.join(".local/bin/tako")));
        assert_eq!(plan.source, app);
        // 実体のディレクトリ（同居物ごと）を PATH へ出さない = #1502 の設計の芯
        assert_ne!(plan.dir, app.parent().unwrap());
    }

    /// dev ビルドの `target/debug` を PATH へ出すと依存クレートの実行ファイルまで
    /// ユーザーの PATH に載る。ここが崩れたら設計の前提が壊れている
    #[test]
    fn devビルドでもpathへ通すのはlocalbinだけ() {
        let home = Path::new("/home/testuser");
        let dev = home.join("dev/tako/target/debug/tako");
        let plan = plan_for(home, &dev, false);
        assert_eq!(plan.dir, home.join(".local/bin"));
        assert_eq!(plan.source, dev);
    }

    #[test]
    fn windowsはsymlinkを張らず実体のディレクトリを通す() {
        let home = Path::new(r"C:\Users\winuser");
        let exe = Path::new(r"C:\Users\winuser\AppData\Local\Programs\tako\tako.exe");
        let plan = plan_for(home, exe, true);
        assert_eq!(plan.link, None, "Windows で symlink を張ろうとしている");
        assert_eq!(plan.dir, exe.parent().unwrap());
    }

    /// 実体が既に置き場所そのものに居るなら symlink は要らない（自分自身を指さない）
    #[test]
    fn 実体がlocalbinに居るならsymlinkを張らない() {
        let home = Path::new("/home/testuser");
        let cli = home.join(".local/bin/tako");
        let plan = plan_for(home, &cli, false);
        assert_eq!(plan.link, None);
        assert_eq!(plan.dir, home.join(".local/bin"));
    }

    #[test]
    fn リンクの状態を引数だけで判定する() {
        let want = Path::new("/Applications/tako.app/Contents/MacOS/tako");
        let other = Path::new("/opt/tako/tako");
        assert_eq!(classify_link(want, false, None, false), LinkState::Missing);
        assert_eq!(classify_link(want, true, None, false), LinkState::Occupied);
        assert_eq!(
            classify_link(want, true, Some(want), true),
            LinkState::Correct
        );
        // 狙いどおりの先でも消えていれば張り直しの対象（`.app` を消した）
        assert_eq!(
            classify_link(want, true, Some(want), false),
            LinkState::Dangling {
                target: want.to_path_buf()
            }
        );
        assert_eq!(
            classify_link(want, true, Some(other), true),
            LinkState::Other {
                target: other.to_path_buf()
            }
        );
        assert!(!LinkState::Missing.usable());
        assert!(!classify_link(want, true, Some(want), false).usable());
        assert!(LinkState::Occupied.usable());
    }

    #[cfg(unix)]
    #[test]
    fn 張って二回目は無変更で削除すると元へ戻る() {
        let home = temp_home("roundtrip");
        let app = home.join("Applications/tako.app/Contents/MacOS");
        std::fs::create_dir_all(&app).unwrap();
        let real = app.join("tako");
        std::fs::write(&real, "#!/bin/sh\n").unwrap();

        let plan = plan_for(&home, &real, false);
        let link = plan.link.clone().expect("unix は symlink を張る");
        assert_eq!(
            ensure_link(&link, &plan.source).unwrap(),
            LinkChange::Created
        );
        assert_eq!(
            ensure_link(&link, &plan.source).unwrap(),
            LinkChange::Unchanged,
            "2 回目で張り直している"
        );
        assert_eq!(link_state(&link, &plan.source), LinkState::Correct);

        assert_eq!(remove_link(&link).unwrap(), LinkChange::Removed);
        assert_eq!(remove_link(&link).unwrap(), LinkChange::Absent);
        assert!(!link.exists());
        cleanup(&home);
    }

    /// `.app` を移動した（= 指す先が消えた）ときだけ起動時に張り直す
    #[cfg(unix)]
    #[test]
    fn 切れたリンクだけを起動時に張り直す() {
        let home = temp_home("repair");
        let old = home.join("Downloads/tako.app/Contents/MacOS");
        let new = home.join("Applications/tako.app/Contents/MacOS");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("tako"), "#!/bin/sh\n").unwrap();
        std::fs::write(new.join("tako"), "#!/bin/sh\n").unwrap();

        let link = home.join(".local/bin/tako");
        ensure_link(&link, &old.join("tako")).unwrap();
        // まだ切れていないので張り替えない（ユーザーの選択を尊重する）
        assert_eq!(
            repair_dangling(&link, &new.join("tako")).unwrap(),
            LinkChange::Unchanged
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), old.join("tako"));

        // `.app` を移動した = 指す先が消えた
        std::fs::remove_dir_all(home.join("Downloads")).unwrap();
        assert!(matches!(
            link_state(&link, &new.join("tako")),
            LinkState::Dangling { .. }
        ));
        assert_eq!(
            repair_dangling(&link, &new.join("tako")).unwrap(),
            LinkChange::Updated
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), new.join("tako"));
        cleanup(&home);
    }

    /// ユーザーが自分で置いた実体（symlink でないファイル）は消さない・上書きしない
    #[cfg(unix)]
    #[test]
    fn symlinkでない実体は触らない() {
        let home = temp_home("occupied");
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let link = bin.join("tako");
        std::fs::write(&link, "user's own tako\n").unwrap();
        let want = home.join("Applications/tako.app/Contents/MacOS/tako");

        assert_eq!(ensure_link(&link, &want).unwrap(), LinkChange::Skipped);
        assert_eq!(remove_link(&link).unwrap(), LinkChange::Skipped);
        assert_eq!(
            std::fs::read_to_string(&link).unwrap(),
            "user's own tako\n",
            "ユーザーのファイルを壊した"
        );
        cleanup(&home);
    }
}
