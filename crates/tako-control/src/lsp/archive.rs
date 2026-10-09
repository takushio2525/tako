//! 取得物の展開（#1944）: gzip 1 枚 / tar.gz（丸ごと・1 ファイルだけ）/ zip の 1 ファイル
//!
//! 新しいクレートを足さない（tar / zip のクレートは依存の木に無い）。読むのは配布元の
//! 素直な形だけで、ハッシュは展開の**前に**取得物全体で検証済み（ここは壊れた入力に対して
//! 「止まる・外へ書かない」だけを守る）:
//!
//! - tar: ustar のヘッダ 512 バイト + 中身（512 境界まで詰める）。pax の `path` と GNU の長い名前を読む。
//!   通常ファイルとディレクトリだけを置き、リンク・デバイスは置かない
//! - zip: 末尾の EOCD → 中央ディレクトリ → ローカルヘッダの順に引く（stored / deflate だけ。zip64 は扱わない）
//! - どの形もパスは `tako_core::lsp::fetch::safe_relative` を通す（`..`・絶対パスを拒む）

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use flate2::read::{DeflateDecoder, GzDecoder};
use tako_core::lsp::fetch::safe_relative;

/// 展開の失敗（理由の文は `lsp::text` が組む）
#[derive(Debug)]
pub enum ArchiveError {
    Io(io::Error),
    /// 形が読めない / 欲しいものが入っていない
    Format(String),
}

impl From<io::Error> for ArchiveError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Format(s) => f.write_str(s),
        }
    }
}

fn format_error(detail: impl Into<String>) -> ArchiveError {
    ArchiveError::Format(detail.into())
}

/// 書いたファイルを実行できるようにする（unix だけ。Windows は拡張子で決まる）
pub fn make_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// gzip 1 枚を `dest` へ解く（上限 `limit` バイト。超えたら止める = 壊れた入力で disk を埋めない）
pub fn gunzip_to(archive: &Path, dest: &Path, limit: u64) -> Result<u64, ArchiveError> {
    let mut decoder = GzDecoder::new(BufReader::new(File::open(archive)?));
    copy_limited(&mut decoder, dest, limit)
}

fn copy_limited(reader: &mut dyn Read, dest: &Path, limit: u64) -> Result<u64, ArchiveError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = File::create(dest)?;
    let mut limited = reader.take(limit.saturating_add(1));
    let written = io::copy(&mut limited, &mut out)?;
    if written > limit {
        return Err(format_error(format!(
            "展開した大きさが上限 {limit} バイトを超えた"
        )));
    }
    out.flush()?;
    Ok(written)
}

/// tar の 1 項目
struct TarEntry {
    name: String,
    kind: u8,
    size: u64,
}

/// tar を先頭から読む（ヘッダ → 中身の順に 1 項目ずつ。中身は `visit` が読むか読み飛ばす）
fn walk_tar(
    reader: &mut dyn Read,
    visit: &mut dyn FnMut(&TarEntry, &mut dyn Read) -> Result<bool, ArchiveError>,
) -> Result<(), ArchiveError> {
    let mut long_name: Option<String> = None;
    loop {
        let mut header = [0u8; 512];
        if !read_full(reader, &mut header)? {
            return Ok(()); // 末尾の 0 ブロックを欠いた tar も終わりとして受ける
        }
        if header.iter().all(|&b| b == 0) {
            return Ok(());
        }
        verify_checksum(&header)?;
        let size = parse_size(&header[124..136])?;
        let kind = header[156];
        let padded = size.div_ceil(512) * 512;
        match kind {
            // pax の拡張ヘッダ（次の項目の名前）/ GNU の長い名前
            b'x' | b'L' => {
                if size > 1 << 20 {
                    return Err(format_error("拡張ヘッダが大きすぎる"));
                }
                let mut data = vec![0u8; padded as usize];
                reader.read_exact(&mut data)?;
                data.truncate(size as usize);
                long_name = if kind == b'x' {
                    pax_path(&data)
                } else {
                    Some(c_string(&data))
                };
                continue;
            }
            // pax の全体ヘッダは使わない
            b'g' => {
                skip(reader, padded)?;
                continue;
            }
            _ => {}
        }
        let name = long_name.take().unwrap_or_else(|| ustar_name(&header));
        let entry = TarEntry { name, kind, size };
        let mut body = reader.take(size);
        let stop = visit(&entry, &mut body)?;
        // 読み残しと詰め物を飛ばす
        io::copy(&mut body, &mut io::sink())?;
        skip(reader, padded - size)?;
        if stop {
            return Ok(());
        }
    }
}

/// 全部読めたら true、1 バイトも読めずに終わったら false（途中で切れたら誤り）
fn read_full(reader: &mut dyn Read, buf: &mut [u8]) -> Result<bool, ArchiveError> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = reader.read(&mut buf[filled..])?;
        if n == 0 {
            if filled == 0 {
                return Ok(false);
            }
            return Err(format_error("tar のヘッダの途中で切れている"));
        }
        filled += n;
    }
    Ok(true)
}

fn skip(reader: &mut dyn Read, n: u64) -> Result<(), ArchiveError> {
    let copied = io::copy(&mut reader.take(n), &mut io::sink())?;
    if copied != n {
        return Err(format_error("tar の中身の途中で切れている"));
    }
    Ok(())
}

fn verify_checksum(header: &[u8; 512]) -> Result<(), ArchiveError> {
    let stored = parse_octal(&header[148..156])
        .ok_or_else(|| format_error("tar のヘッダのチェックサムが読めない"))?;
    let sum: u64 = header
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if (148..156).contains(&i) {
                32
            } else {
                u64::from(b)
            }
        })
        .sum();
    if sum != stored {
        return Err(format_error(
            "tar のヘッダが壊れている（チェックサムが合わない）",
        ));
    }
    Ok(())
}

fn parse_octal(field: &[u8]) -> Option<u64> {
    let text: String = field
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    let text = text.trim();
    if text.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(text, 8).ok()
}

/// 大きさ（8 進。最上位ビットが立っていれば base-256 = 8 GiB を超える GNU の形）
fn parse_size(field: &[u8]) -> Result<u64, ArchiveError> {
    if field[0] & 0x80 != 0 {
        let mut value: u64 = 0;
        for (i, &b) in field.iter().enumerate() {
            let b = if i == 0 { b & 0x7f } else { b };
            value = value
                .checked_mul(256)
                .and_then(|v| v.checked_add(u64::from(b)))
                .ok_or_else(|| format_error("tar の大きさが大きすぎる"))?;
        }
        return Ok(value);
    }
    parse_octal(field).ok_or_else(|| format_error("tar の大きさが読めない"))
}

fn c_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn ustar_name(header: &[u8; 512]) -> String {
    let name = c_string(&header[0..100]);
    // ustar は 155 バイトの prefix を持てる
    if &header[257..262] == b"ustar" {
        let prefix = c_string(&header[345..500]);
        if !prefix.is_empty() {
            return format!("{prefix}/{name}");
        }
    }
    name
}

/// pax の拡張ヘッダ（`<長さ> <鍵>=<値>\n` の並び）から `path` を読む
fn pax_path(data: &[u8]) -> Option<String> {
    let mut rest = data;
    while !rest.is_empty() {
        let space = rest.iter().position(|&b| b == b' ')?;
        let len: usize = std::str::from_utf8(&rest[..space]).ok()?.parse().ok()?;
        if len == 0 || len > rest.len() {
            return None;
        }
        let record = &rest[space + 1..len];
        let record = record.strip_suffix(b"\n").unwrap_or(record);
        if let Some(value) = record.strip_prefix(b"path=") {
            return Some(String::from_utf8_lossy(value).into_owned());
        }
        rest = &rest[len..];
    }
    None
}

/// tar.gz の中の `member` 1 つだけを `dest` へ書く
pub fn untar_gz_member(
    archive: &Path,
    member: &str,
    dest: &Path,
    limit: u64,
) -> Result<u64, ArchiveError> {
    let mut decoder = GzDecoder::new(BufReader::new(File::open(archive)?));
    let wanted = safe_relative(member, 0).ok_or_else(|| format_error("取り出す名前が不正"))?;
    let mut written = None;
    walk_tar(&mut decoder, &mut |entry, body| {
        if !is_file(entry.kind) || safe_relative(&entry.name, 0).as_ref() != Some(&wanted) {
            return Ok(false);
        }
        written = Some(copy_limited(body, dest, limit.min(entry.size))?);
        Ok(true)
    })?;
    written.ok_or_else(|| format_error(format!("{member} が入っていない")))
}

fn is_file(kind: u8) -> bool {
    matches!(kind, b'0' | 0 | b'7')
}

/// npm の tarball を丸ごと `dest` へ置く（先頭の 1 段 = `package/` を外す）。
/// 置いたファイルの数と合計の大きさを返す。`limit` は展開後の合計の上限
pub fn untar_gz_all(archive: &Path, dest: &Path, limit: u64) -> Result<(u64, u64), ArchiveError> {
    let mut decoder = GzDecoder::new(BufReader::new(File::open(archive)?));
    let (mut files, mut total) = (0u64, 0u64);
    std::fs::create_dir_all(dest)?;
    walk_tar(&mut decoder, &mut |entry, body| {
        let Some(rel) = safe_relative(&entry.name, 1) else {
            return Ok(false); // `package/` そのもの・外へ出る名前は置かない
        };
        let target = dest.join(&rel);
        match entry.kind {
            b'5' => {
                std::fs::create_dir_all(&target)?;
            }
            kind if is_file(kind) => {
                let remaining = limit.saturating_sub(total);
                if entry.size > remaining {
                    return Err(format_error(format!(
                        "展開した大きさが上限 {limit} バイトを超えた"
                    )));
                }
                total += copy_limited(body, &target, entry.size)?;
                files += 1;
            }
            // シンボリックリンク・ハードリンク・デバイスは置かない（npm の tarball には無い）
            _ => {}
        }
        Ok(false)
    })?;
    Ok((files, total))
}

/// zip の中の `member` 1 つだけを `dest` へ書く（stored / deflate）
pub fn unzip_member(
    archive: &Path,
    member: &str,
    dest: &Path,
    limit: u64,
) -> Result<u64, ArchiveError> {
    let mut file = File::open(archive)?;
    let len = file.metadata()?.len();
    // EOCD（22 バイト + コメント最大 65535）を末尾から探す
    let tail_len = len.min(22 + 65_535);
    file.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail)?;
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or_else(|| format_error("zip の終わりの印が無い"))?;
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_at = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let cd_size = u32_at(&tail, eocd + 12);
    let cd_offset = u32_at(&tail, eocd + 16);
    if cd_size == u32::MAX || cd_offset == u32::MAX {
        return Err(format_error("zip64 は扱わない"));
    }
    if u64::from(cd_offset) + u64::from(cd_size) > len || cd_size > 64 << 20 {
        return Err(format_error("zip の中央ディレクトリが範囲の外"));
    }
    file.seek(SeekFrom::Start(u64::from(cd_offset)))?;
    let mut cd = vec![0u8; cd_size as usize];
    file.read_exact(&mut cd)?;
    let mut at = 0usize;
    while at + 46 <= cd.len() && cd[at..at + 4] == [0x50, 0x4b, 0x01, 0x02] {
        let method = u16_at(&cd, at + 10);
        let compressed = u64::from(u32_at(&cd, at + 20));
        let uncompressed = u64::from(u32_at(&cd, at + 24));
        let name_len = usize::from(u16_at(&cd, at + 28));
        let extra_len = usize::from(u16_at(&cd, at + 30));
        let comment_len = usize::from(u16_at(&cd, at + 32));
        let local = u64::from(u32_at(&cd, at + 42));
        let name_end = at + 46 + name_len;
        if name_end > cd.len() {
            break;
        }
        let name = String::from_utf8_lossy(&cd[at + 46..name_end]).into_owned();
        at = name_end + extra_len + comment_len;
        if name != member {
            continue;
        }
        if uncompressed > limit {
            return Err(format_error(format!(
                "展開した大きさが上限 {limit} バイトを超える"
            )));
        }
        file.seek(SeekFrom::Start(local))?;
        let mut header = [0u8; 30];
        file.read_exact(&mut header)?;
        if header[0..4] != [0x50, 0x4b, 0x03, 0x04] {
            return Err(format_error("zip のローカルヘッダが壊れている"));
        }
        let skip_len = u64::from(u16_at(&header, 26)) + u64::from(u16_at(&header, 28));
        file.seek(SeekFrom::Current(skip_len as i64))?;
        let body = BufReader::new(file).take(compressed);
        let written = match method {
            0 => copy_limited(&mut { body }, dest, uncompressed)?,
            8 => copy_limited(&mut DeflateDecoder::new(body), dest, uncompressed)?,
            other => return Err(format_error(format!("zip の圧縮方式 {other} は扱わない"))),
        };
        if written != uncompressed {
            return Err(format_error("zip の中身の大きさが合わない"));
        }
        return Ok(written);
    }
    Err(format_error(format!("{member} が入っていない")))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use flate2::write::{DeflateEncoder, GzEncoder};
    use flate2::Compression;
    use std::path::PathBuf;

    /// テスト用の一時置き場（固定名を使わない = 並行する cargo test 同士で消し合わない）
    pub(crate) struct Scratch(pub PathBuf);

    impl Scratch {
        pub(crate) fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tako-archive-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// ustar の 1 項目を組む（名前が 100 バイトを超えたら pax の `path` を前に置く）
    pub(crate) fn tar_entry(out: &mut Vec<u8>, name: &str, kind: u8, body: &[u8]) {
        if name.len() >= 100 {
            let record_body = format!("path={name}\n");
            // 長さは自分自身の桁を含む
            let mut len = record_body.len() + 3;
            loop {
                let candidate = format!("{len} {record_body}");
                if candidate.len() == len {
                    break;
                }
                len = candidate.len();
            }
            let record = format!("{len} {record_body}");
            tar_entry(out, "././@PaxHeader", b'x', record.as_bytes());
        }
        let mut header = [0u8; 512];
        let short = &name.as_bytes()[..name.len().min(99)];
        header[..short.len()].copy_from_slice(short);
        header[100..108].copy_from_slice(b"0000644\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", body.len()).as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[156] = kind;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(body);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }

    pub(crate) fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut enc = GzEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(bytes).unwrap();
        enc.finish().unwrap()
    }

    pub(crate) fn tar_gz(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, kind, body) in entries {
            tar_entry(&mut tar, name, *kind, body);
        }
        tar.extend_from_slice(&[0u8; 1024]);
        gzip(&tar)
    }

    /// zip を組む（stored と deflate を 1 つずつ混ぜられる）
    pub(crate) fn zip(entries: &[(&str, bool, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, deflate, body) in entries {
            let data = if *deflate {
                let mut enc = DeflateEncoder::new(Vec::new(), Compression::fast());
                enc.write_all(body).unwrap();
                enc.finish().unwrap()
            } else {
                body.to_vec()
            };
            let offset = out.len() as u32;
            let method: u16 = if *deflate { 8 } else { 0 };
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]); // 時刻 + CRC（読まない）
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&data);
            central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(body.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]); // extra / comment / disk / attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn gzip_1_枚を解く() {
        let s = Scratch::new("gz");
        let archive = write(&s.0, "a.gz", &gzip(b"#!/bin/sh\necho hi\n"));
        let dest = s.0.join("out/bin");
        assert_eq!(gunzip_to(&archive, &dest, 1 << 20).unwrap(), 18);
        assert_eq!(std::fs::read(&dest).unwrap(), b"#!/bin/sh\necho hi\n");
        // 上限を超える中身は止める
        assert!(gunzip_to(&archive, &s.0.join("small"), 4).is_err());
    }

    #[test]
    fn npm_の_tarball_を先頭の段を外して置く() {
        let long = format!("package/{}/index.js", "d".repeat(120));
        let bytes = tar_gz(&[
            ("package/", b'5', b""),
            ("package/package.json", b'0', b"{}"),
            ("package/lib/cli.mjs", b'0', b"export {}"),
            (&long, b'0', b"long"),
            ("package/../../escape.txt", b'0', b"x"),
            ("package/link", b'2', b""),
        ]);
        let s = Scratch::new("npm");
        let archive = write(&s.0, "p.tgz", &bytes);
        let dest = s.0.join("node_modules/pkg");
        let (files, total) = untar_gz_all(&archive, &dest, 1 << 20).unwrap();
        assert_eq!(files, 3, "外へ出る名前とリンクは置かない");
        assert_eq!(total, 2 + 9 + 4);
        assert_eq!(
            std::fs::read(dest.join("lib/cli.mjs")).unwrap(),
            b"export {}"
        );
        assert!(
            dest.join(format!("{}/index.js", "d".repeat(120))).exists(),
            "pax の長い名前"
        );
        assert!(!s.0.join("escape.txt").exists());
        assert!(!s.0.join("node_modules/escape.txt").exists());
        // 合計の上限
        assert!(untar_gz_all(&archive, &s.0.join("again"), 5).is_err());
    }

    #[test]
    fn tar_gz_から_1_つだけ取り出す() {
        let bytes = tar_gz(&[
            ("node-v1/", b'5', b""),
            ("node-v1/README.md", b'0', b"readme"),
            ("node-v1/bin/node", b'0', b"ELF"),
        ]);
        let s = Scratch::new("member");
        let archive = write(&s.0, "n.tgz", &bytes);
        let dest = s.0.join("node");
        assert_eq!(
            untar_gz_member(&archive, "node-v1/bin/node", &dest, 1 << 20).unwrap(),
            3
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"ELF");
        assert!(untar_gz_member(&archive, "node-v1/bin/npm", &s.0.join("x"), 1 << 20).is_err());
    }

    #[test]
    fn 壊れた_tar_は止まる() {
        let mut tar = Vec::new();
        tar_entry(&mut tar, "package/a", b'0', b"abc");
        tar[0] ^= 0xff; // 名前を壊す = チェックサムが合わない
        let s = Scratch::new("broken");
        let archive = write(&s.0, "b.tgz", &gzip(&tar));
        let error = untar_gz_all(&archive, &s.0.join("out"), 1 << 20).unwrap_err();
        assert!(error.to_string().contains("チェックサム"), "{error}");
        // 途中で切れた gzip
        let mut cut = tar_gz(&[("package/a", b'0', &[7u8; 4096])]);
        cut.truncate(cut.len() / 2);
        let archive = write(&s.0, "cut.tgz", &cut);
        assert!(untar_gz_all(&archive, &s.0.join("cut"), 1 << 20).is_err());
    }

    #[test]
    fn zip_から_1_つだけ取り出す() {
        let big = vec![b'z'; 100_000];
        let bytes = zip(&[
            ("tool.pdb", false, b"pdb"),
            ("tool.exe", true, &big),
            ("dir/stored.txt", false, b"plain"),
        ]);
        let s = Scratch::new("zip");
        let archive = write(&s.0, "t.zip", &bytes);
        let dest = s.0.join("tool.exe");
        assert_eq!(
            unzip_member(&archive, "tool.exe", &dest, 1 << 20).unwrap(),
            100_000
        );
        assert_eq!(std::fs::read(&dest).unwrap(), big);
        let stored = s.0.join("stored.txt");
        unzip_member(&archive, "dir/stored.txt", &stored, 1 << 20).unwrap();
        assert_eq!(std::fs::read(&stored).unwrap(), b"plain");
        assert!(unzip_member(&archive, "missing.exe", &s.0.join("m"), 1 << 20).is_err());
        assert!(unzip_member(&archive, "tool.exe", &s.0.join("small"), 10).is_err());
        // zip でないもの
        let not_zip = write(&s.0, "n.zip", b"not a zip at all, definitely not");
        assert!(unzip_member(&not_zip, "tool.exe", &s.0.join("n"), 1 << 20).is_err());
    }
}
