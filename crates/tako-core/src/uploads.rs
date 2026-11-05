//! 添付の置き場（#285 / #2743）。
//!
//! スマホの添付（remote daemon の `POST /api/upload`）と、`tako_send_input` の `attachments`
//! （CLI は `tako send --attach`）は**同じ置き場**へ写す: 対象ペインの作業ディレクトリの
//! [`DIR_NAME`]。置き場の名前と 1 ファイルの大きさの上限の正本はここ 1 か所。
//!
//! 写したファイルは本人だけが読み書きできる形で作る（境界 `platform::private_file`。#287 P2-1 /
//! #2581）。`attachments` の写しは**上書きしない**（同じ名前が在れば `名前-2.拡張子` へずらす =
//! 先に送った添付をエージェントが読む前に差し替えない）。置き場を初めて作ったときは
//! 中身を git へ載せない `.gitignore`（`*`）を置く（作業ディレクトリはたいていリポジトリの中で、
//! `git add -A` に添付の画像が混ざるのを防ぐ）。

use std::io;
use std::path::{Path, PathBuf};

use crate::i18n::Lang;

/// 置き場のディレクトリ名（ペインの作業ディレクトリの直下）
pub const DIR_NAME: &str = ".tako-remote-uploads";

/// 1 ファイルの大きさの上限（20 MB。スマホの添付と同じ）
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;

/// 同じ名前が在るときにずらす番号の上限（`名前-2` 〜 `名前-99`）
const MAX_NUMBER: u32 = 99;

/// 写し終えた 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    /// 渡されたファイル
    pub source: PathBuf,
    /// 写した先（絶対パス）
    pub path: PathBuf,
    pub bytes: u64,
}

impl Staged {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "source": self.source.display().to_string(),
            "path": self.path.display().to_string(),
            "bytes": self.bytes,
        })
    }
}

/// 写せない理由。表示（`Display`）は表示言語に従う（#2365。日英の正本は [`Self::text_in`]）。
/// パスと OS のエラー文は言語に依らずそのまま載せる
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageRefusal {
    /// 相対パス（GUI の作業ディレクトリは呼び手と違うので解釈しない）
    NotAbsolute(PathBuf),
    /// 渡されたファイルが無い
    SourceMissing(PathBuf),
    /// ファイルではない（フォルダ・FIFO・デバイス。FIFO は読むと止まるので開かない）
    NotFile(PathBuf),
    /// [`MAX_BYTES`] を超える
    TooLarge { path: PathBuf, limit_mb: u64 },
    /// 名前の無いパス（`/` やドライブの根）
    NoName(PathBuf),
    /// 写し先の作業ディレクトリが無い
    CwdMissing(PathBuf),
    /// 置き場がシンボリックリンク（リンク先への意図しない書き込みを防ぐ。#287 P2-4）
    DirSymlink(PathBuf),
    /// 別名を [`MAX_NUMBER`] まで試しても空かない
    NamesExhausted(PathBuf),
    /// 読み書きを OS が断った（中身は OS のエラー文）
    Io { path: PathBuf, error: String },
}

impl StageRefusal {
    /// 表示言語を明示しての文言（(バリアント, 言語) の網羅 match = 片方だけ書くと通らない）。
    /// 「見つからない」系の英語は ` not found` を含める（`DispatchError::says_not_found` の当たり方）
    pub fn text_in(&self, lang: Lang) -> String {
        use Lang::{En, Ja};
        match (self, lang) {
            (Self::NotAbsolute(p), Ja) => {
                format!("添付は絶対パスで指定する: {}", p.display())
            }
            (Self::NotAbsolute(p), En) => {
                format!("give each attachment as an absolute path: {}", p.display())
            }
            (Self::SourceMissing(p), Ja) => format!("添付が見つからない: {}", p.display()),
            (Self::SourceMissing(p), En) => format!("attachment not found: {}", p.display()),
            (Self::NotFile(p), Ja) => {
                format!(
                    "添付はファイルで指定する（フォルダ等は送れない）: {}",
                    p.display()
                )
            }
            (Self::NotFile(p), En) => {
                format!(
                    "an attachment must be a file (not a folder or the like): {}",
                    p.display()
                )
            }
            (Self::TooLarge { path, limit_mb }, Ja) => {
                format!("添付が大きすぎる（上限 {limit_mb}MB）: {}", path.display())
            }
            (Self::TooLarge { path, limit_mb }, En) => {
                format!(
                    "attachment is too large (limit {limit_mb}MB): {}",
                    path.display()
                )
            }
            (Self::NoName(p), Ja) => format!("添付に名前が無い: {}", p.display()),
            (Self::NoName(p), En) => format!("the attachment has no file name: {}", p.display()),
            (Self::CwdMissing(p), Ja) => {
                format!("ペインの作業ディレクトリが見つからない: {}", p.display())
            }
            (Self::CwdMissing(p), En) => {
                format!("the pane's working directory not found: {}", p.display())
            }
            (Self::DirSymlink(p), Ja) => {
                format!(
                    "添付の置き場がシンボリックリンクなので書かない: {}",
                    p.display()
                )
            }
            (Self::DirSymlink(p), En) => {
                format!(
                    "the attachment folder is a symbolic link, so nothing is written: {}",
                    p.display()
                )
            }
            (Self::NamesExhausted(p), Ja) => {
                format!("添付の置き場に同じ名前が多すぎる: {}", p.display())
            }
            (Self::NamesExhausted(p), En) => {
                format!(
                    "too many files with the same name in the attachment folder: {}",
                    p.display()
                )
            }
            (Self::Io { path, error }, Ja) => {
                format!("添付を写せない: {}: {error}", path.display())
            }
            (Self::Io { path, error }, En) => {
                format!("cannot copy the attachment: {}: {error}", path.display())
            }
        }
    }
}

impl std::fmt::Display for StageRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(crate::i18n::lang()))
    }
}

/// 置き場のパス（`<cwd>/.tako-remote-uploads`）
pub fn dir_in(cwd: &Path) -> PathBuf {
    cwd.join(DIR_NAME)
}

/// 送る前の確かめ（写す前に全件を見る = 2 件目が駄目なら 1 件目も写さない）。
/// 返すのは大きさ
fn check_source(source: &Path) -> Result<u64, StageRefusal> {
    if !source.is_absolute() {
        return Err(StageRefusal::NotAbsolute(source.to_path_buf()));
    }
    // リンクは辿る（リンク先のファイルを写す）。FIFO・デバイスは開くと止まりうるので開かない
    let meta = match std::fs::metadata(source) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(StageRefusal::SourceMissing(source.to_path_buf()));
        }
        Err(e) => {
            return Err(StageRefusal::Io {
                path: source.to_path_buf(),
                error: e.to_string(),
            });
        }
    };
    if !meta.is_file() {
        return Err(StageRefusal::NotFile(source.to_path_buf()));
    }
    if meta.len() > MAX_BYTES {
        return Err(StageRefusal::TooLarge {
            path: source.to_path_buf(),
            limit_mb: MAX_BYTES / (1024 * 1024),
        });
    }
    if source.file_name().is_none() {
        return Err(StageRefusal::NoName(source.to_path_buf()));
    }
    Ok(meta.len())
}

/// 置き場を用意する（無ければ作り、初めて作ったときは `.gitignore` を置く）
fn ensure_dir(cwd: &Path) -> Result<PathBuf, StageRefusal> {
    if !cwd.is_dir() {
        return Err(StageRefusal::CwdMissing(cwd.to_path_buf()));
    }
    let dir = dir_in(cwd);
    if dir
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(StageRefusal::DirSymlink(dir));
    }
    let created = !dir.exists();
    std::fs::create_dir_all(&dir).map_err(|e| StageRefusal::Io {
        path: dir.clone(),
        error: e.to_string(),
    })?;
    if created {
        // 置けなくても添付は写す（git に載るかどうかは添付の成否と別の話）
        let _ = std::fs::write(dir.join(".gitignore"), "*\n");
    }
    Ok(dir)
}

/// 写し先の候補名（`n` = 1 はそのまま、2 以降は `名前-n.拡張子`）
fn candidate_name(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_string();
    }
    // 先頭のドットはドットファイルの名前の一部（`.env` → `.env-2`）
    let split = name
        .char_indices()
        .rev()
        .find(|&(i, c)| c == '.' && i > 0)
        .map(|(i, _)| i);
    match split {
        Some(i) => format!("{}-{n}{}", &name[..i], &name[i..]),
        None => format!("{name}-{n}"),
    }
}

/// 本人だけが読み書きできる新しいファイルへ写す（既に在る名前は使わない）
fn copy_new(source: &Path, dir: &Path, name: &str) -> Result<(PathBuf, u64), StageRefusal> {
    use std::io::Read as _;
    let io_err = |path: &Path, e: io::Error| StageRefusal::Io {
        path: path.to_path_buf(),
        error: e.to_string(),
    };
    let mut reader = std::fs::File::open(source).map_err(|e| io_err(source, e))?;
    for n in 1..=MAX_NUMBER {
        let target = dir.join(candidate_name(name, n));
        let mut file = match crate::platform::private_file::create_new(&target) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_err(&target, e)),
        };
        // 確かめた後に伸びたファイルも上限で切る（上限を 1 バイトでも超えたら消して断る）
        let copied = io::copy(&mut (&mut reader).take(MAX_BYTES + 1), &mut file);
        let copied = match copied {
            Ok(n) if n <= MAX_BYTES => n,
            Ok(_) => {
                drop(file);
                let _ = std::fs::remove_file(&target);
                return Err(StageRefusal::TooLarge {
                    path: source.to_path_buf(),
                    limit_mb: MAX_BYTES / (1024 * 1024),
                });
            }
            Err(e) => {
                drop(file);
                let _ = std::fs::remove_file(&target);
                return Err(io_err(source, e));
            }
        };
        return Ok((target, copied));
    }
    Err(StageRefusal::NamesExhausted(dir.join(name)))
}

/// `sources` を `cwd` の置き場へ写す（#2743）。
///
/// 写す前に全件を確かめ、途中で失敗したらこの呼び出しで写した分を消してから断る
/// （「1 件目だけ写って本文は送られない」を残さない）
pub fn stage_all(cwd: &Path, sources: &[PathBuf]) -> Result<Vec<Staged>, StageRefusal> {
    for source in sources {
        check_source(source)?;
    }
    let dir = ensure_dir(cwd)?;
    let mut staged: Vec<Staged> = Vec::with_capacity(sources.len());
    for source in sources {
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| StageRefusal::NoName(source.clone()))?;
        match copy_new(source, &dir, &name) {
            Ok((path, bytes)) => staged.push(Staged {
                source: source.clone(),
                path,
                bytes,
            }),
            Err(refusal) => {
                for done in &staged {
                    let _ = std::fs::remove_file(&done.path);
                }
                return Err(refusal);
            }
        }
    }
    Ok(staged)
}

/// 送る本文（`text` の後ろに写した先のパスを空白区切りで足す。空白を含むパスは `"…"` で囲む）。
/// `text` が空ならパスだけ
pub fn compose_text(text: &str, staged: &[Staged]) -> String {
    let paths: Vec<String> = staged
        .iter()
        .map(|s| {
            let p = s.path.display().to_string();
            if p.chars().any(char::is_whitespace) {
                format!("\"{p}\"")
            } else {
                p
            }
        })
        .collect();
    let joined = paths.join(" ");
    match (text.is_empty(), joined.is_empty()) {
        (_, true) => text.to_string(),
        (true, false) => joined,
        (false, false) => format!("{text} {joined}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_residue::ScratchDir;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).expect("write");
    }

    #[test]
    fn 置き場へ写して本文にパスを足す() {
        let tmp = ScratchDir::new("uploads-stage");
        let cwd = tmp.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let src = tmp.path().join("shot.png");
        write(&src, b"\x89PNG data");

        let staged = stage_all(&cwd, std::slice::from_ref(&src)).expect("stage");
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].path, cwd.join(DIR_NAME).join("shot.png"));
        assert_eq!(staged[0].bytes, 9);
        assert_eq!(std::fs::read(&staged[0].path).unwrap(), b"\x89PNG data");
        // 初めて作った置き場には git へ載せない印を置く
        assert_eq!(
            std::fs::read_to_string(cwd.join(DIR_NAME).join(".gitignore")).unwrap(),
            "*\n"
        );
        let text = compose_text("これを見て", &staged);
        assert_eq!(
            text,
            format!(
                "これを見て {}",
                cwd.join(DIR_NAME).join("shot.png").display()
            )
        );
        assert_eq!(
            compose_text("", &staged),
            staged[0].path.display().to_string()
        );
    }

    #[cfg(unix)]
    #[test]
    fn 写したファイルは本人だけが読める() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = ScratchDir::new("uploads-mode");
        let src = tmp.path().join("a.txt");
        write(&src, b"x");
        let staged = stage_all(tmp.path(), &[src]).unwrap();
        let mode = std::fs::metadata(&staged[0].path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn 同じ名前は上書きせずにずらす() {
        let tmp = ScratchDir::new("uploads-dup");
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        write(&a.join("img.png"), b"first");
        write(&b.join("img.png"), b"second");
        let staged = stage_all(tmp.path(), &[a.join("img.png"), b.join("img.png")]).unwrap();
        let dir = tmp.path().join(DIR_NAME);
        assert_eq!(staged[0].path, dir.join("img.png"));
        assert_eq!(staged[1].path, dir.join("img-2.png"));
        assert_eq!(std::fs::read(dir.join("img.png")).unwrap(), b"first");
        assert_eq!(std::fs::read(dir.join("img-2.png")).unwrap(), b"second");
        assert_eq!(candidate_name(".env", 3), ".env-3");
        assert_eq!(candidate_name("a.tar.gz", 2), "a.tar-2.gz");
        assert_eq!(candidate_name("README", 2), "README-2");
    }

    #[test]
    fn 写せないものは何も写さずに断る() {
        let tmp = ScratchDir::new("uploads-refuse");
        let ok = tmp.path().join("ok.txt");
        write(&ok, b"ok");
        // 相対パス
        assert!(matches!(
            stage_all(tmp.path(), &[PathBuf::from("rel.txt")]),
            Err(StageRefusal::NotAbsolute(_))
        ));
        // 無いファイル（1 件目が正しくても 1 件も写さない）
        let missing = tmp.path().join("missing.png");
        assert_eq!(
            stage_all(tmp.path(), &[ok.clone(), missing.clone()]),
            Err(StageRefusal::SourceMissing(missing))
        );
        assert!(!tmp.path().join(DIR_NAME).exists());
        // フォルダ
        assert_eq!(
            stage_all(tmp.path(), &[tmp.path().to_path_buf()]),
            Err(StageRefusal::NotFile(tmp.path().to_path_buf()))
        );
        // 上限ちょうどは通り、1 バイト超えは断る（中身を書かずに大きさだけ持つ疎なファイル）
        let big = tmp.path().join("big.bin");
        let f = std::fs::File::create(&big).unwrap();
        f.set_len(MAX_BYTES + 1).unwrap();
        drop(f);
        assert!(matches!(
            stage_all(tmp.path(), std::slice::from_ref(&big)),
            Err(StageRefusal::TooLarge { limit_mb: 20, .. })
        ));
        // 作業ディレクトリが無い
        let gone = tmp.path().join("gone");
        assert_eq!(
            stage_all(&gone, std::slice::from_ref(&ok)),
            Err(StageRefusal::CwdMissing(gone.clone()))
        );
        assert!(!gone.exists(), "無い作業ディレクトリを作らない");
    }

    #[cfg(unix)]
    #[test]
    fn 置き場がリンクなら書かない() {
        let tmp = ScratchDir::new("uploads-link");
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let cwd = tmp.path().join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();
        std::os::unix::fs::symlink(&elsewhere, cwd.join(DIR_NAME)).unwrap();
        let src = tmp.path().join("a.txt");
        write(&src, b"x");
        assert!(matches!(
            stage_all(&cwd, &[src]),
            Err(StageRefusal::DirSymlink(_))
        ));
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[test]
    fn 空白を含むパスは囲む() {
        let staged = [Staged {
            source: PathBuf::from("/x/a b.png"),
            path: PathBuf::from("/w/.tako-remote-uploads/a b.png"),
            bytes: 1,
        }];
        assert_eq!(
            compose_text("見て", &staged),
            "見て \"/w/.tako-remote-uploads/a b.png\""
        );
        assert_eq!(compose_text("そのまま", &[]), "そのまま");
    }

    #[test]
    fn 断りは日英で書く() {
        let r = StageRefusal::SourceMissing(PathBuf::from("/x/a.png"));
        assert_eq!(r.text_in(Lang::Ja), "添付が見つからない: /x/a.png");
        assert_eq!(r.text_in(Lang::En), "attachment not found: /x/a.png");
    }
}
