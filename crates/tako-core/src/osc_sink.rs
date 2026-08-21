//! 器が OSC を素通ししない環境向けの、シェル統合の側路（#766）
//!
//! ## なぜ要るか
//!
//! シェル統合（OSC 7 / 133。FR-2.4.1）は「ペインのシェルが出したバイト列を tako が
//! PTY 読み取りで拾う」形で成立している。macOS の tmux は `allow-passthrough on` +
//! DCS 包み（`ESC P tmux; … ESC \`）でこれを通すので、器があっても届く。
//!
//! **psmux は通さない**。実測（#766 起票時）で素の OSC・DCS（ESC 二重化あり / なし）の
//! 3 形すべてが外へ出ず、同時に流した平文だけが届いた。upstream のソースを見ると理由が
//! はっきりする（2026-08-21 時点の master / v3.3.8）:
//!
//! - `allow-passthrough` は**選択肢として存在するだけ**。`src/server/options.rs` の
//!   get / set と config パースにしか現れず、**値を読んで素通しする側が無い**
//! - `Ptmux`（DCS の tmux 形式）の実装は**リポジトリに 1 箇所も無い**
//! - psmux は「パースして画面モデルへ落とし、クライアントへ描き直す」多重化器で、
//!   OSC 7 / 133 / 633 / 1337 は**自分で消費する**（`#{pane_current_path}` /
//!   `#{pane_current_command}` の材料にしている）。画面モデルに置き場の無いバイト列は
//!   原理的にクライアントへ出ない = **私用 OSC を使う抜け道も無い**
//!
//! つまり器の側で直る話ではない（upstream の新機能が要る）。tako が今できるのは
//! **同じバイト列を別の経路で運ぶ**ことだけ。
//!
//! ## 運ぶのは「解釈済みの状態」ではなく OSC バイト列そのまま
//!
//! ファイルへ書くのは統合スクリプトが出すはずだった **OSC のバイト列そのもの**で、
//! 解釈は PTY 経路と同じ [`crate::osc_tap`] に通す。状態機械が 1 本のままになるので
//! 「macOS では Failed(3) だが Windows では Idle」のような分岐が構造的に起きない。
//!
//! ## 書き込みと読み取りの取り決め
//!
//! - 書き手はペインの中のシェル 1 個だけ。1 回のプロンプト（= `D` + `A` + cwd の束）を
//!   **まとめて 1 回で上書き**する（`.new` へ書いて rename = 差し替えは原子的）
//! - 読み手は tako の定期更新。**中身が前回と変わっていたら**そのまま
//!   [`crate::osc_tap`] へ通す。追記ではなく上書きなので、読み取りと書き込みが
//!   競合してもバイト列が混ざらない（ファイルは常に完全な 1 束を持つ）
//! - 同じ束が連続したときは 1 回しか通らない（例: `ls` を 2 回）。状態は同じ値へ
//!   遷移するだけなので実害が無く、追記方式の「読んで truncate する隙に書かれた分を
//!   落とす」窓を作らないほうを採った

use std::path::{Path, PathBuf};

/// ペインの中のシェルへ側路の書き先を教える環境変数。
///
/// これが設定されているときだけ統合スクリプトは側路へ書く（未設定なら従来どおり
/// コンソールへ OSC を出す）。器へは**ペイン固有の値**として渡す必要がある
/// （[`crate::backend::PANE_SCOPED_ENV`]）
pub const SINK_ENV: &str = "TAKO_OSC_SINK";

/// 側路のファイルを置くディレクトリ名（`<data_dir>/<この名前>/`）
const DIR: &str = "osc";

/// ペインの側路ファイルのパス（純粋関数）
pub fn sink_path(data_dir: &Path, pane_id: u64) -> PathBuf {
    data_dir.join(DIR).join(format!("{pane_id}.osc"))
}

/// 側路を使う準備をして書き先を返す。作れなければ `None`（統合が従来どおり
/// コンソールへ出すだけになる = 器の中では効かないが、壊れはしない）。
///
/// 前のペインの残骸は消す。ペイン ID は再起動をまたいで再利用されるので（#210）、
/// 残骸を残すと**前回の最後の状態**を今回の起動直後に食わせてしまう
pub fn prepare(data_dir: &Path, pane_id: u64) -> Option<PathBuf> {
    let path = sink_path(data_dir, pane_id);
    std::fs::create_dir_all(path.parent()?).ok()?;
    let _ = std::fs::remove_file(&path);
    Some(path)
}

/// ペインを閉じたときの後始末（残骸を残さない）
pub fn discard(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// 側路 1 本の読み取り状態。前回通したバイト列を覚えておき、変化したぶんだけ通す
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SinkCursor {
    last: Vec<u8>,
}

impl SinkCursor {
    /// ファイルを読み、前回と違えばそのバイト列を返す（同じ / 読めない場合は `None`）。
    ///
    /// 1 束は 100 バイト程度なので、ペインごと 1 tick に 1 回読んでも実質ゼロコスト。
    /// 上限を設けているのは、想定外に育ったファイル（tako が居ない間に書かれ続けた等）で
    /// メモリと解析時間を食わないため
    pub fn take_new(&mut self, path: &Path) -> Option<Vec<u8>> {
        const MAX: u64 = 64 * 1024;
        let len = std::fs::metadata(path).ok()?.len();
        if len == 0 || len > MAX {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        if bytes == self.last {
            return None;
        }
        self.last = bytes.clone();
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tako-osc-sink-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn 側路のパスはペインごとに分かれる() {
        let base = Path::new("/data");
        assert_eq!(
            sink_path(base, 3),
            Path::new("/data").join("osc").join("3.osc")
        );
        assert_ne!(sink_path(base, 3), sink_path(base, 4));
    }

    #[test]
    fn prepareはディレクトリを作り前のペインの残骸を消す() {
        let dir = temp_dir("prepare");
        let path = sink_path(&dir, 7);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"\x1b]133;D;9\x07").unwrap();

        let got = prepare(&dir, 7).expect("準備できる");
        assert_eq!(got, path);
        assert!(
            !path.exists(),
            "残骸が消えていない（前回の最後の状態を食わせてしまう）"
        );
    }

    #[test]
    fn 変化したときだけ通す() {
        let dir = temp_dir("cursor");
        let path = prepare(&dir, 1).unwrap();
        let mut cursor = SinkCursor::default();

        assert_eq!(
            cursor.take_new(&path),
            None,
            "ファイルが無ければ何も通さない"
        );

        std::fs::write(&path, b"\x1b]133;A\x07").unwrap();
        assert_eq!(
            cursor.take_new(&path).as_deref(),
            Some(&b"\x1b]133;A\x07"[..])
        );
        assert_eq!(cursor.take_new(&path), None, "同じ内容は 2 回通さない");

        std::fs::write(&path, b"\x1b]133;D;3\x07").unwrap();
        assert_eq!(
            cursor.take_new(&path).as_deref(),
            Some(&b"\x1b]133;D;3\x07"[..])
        );
    }

    #[test]
    fn 空と大きすぎるファイルは通さない() {
        let dir = temp_dir("guard");
        let path = prepare(&dir, 2).unwrap();
        let mut cursor = SinkCursor::default();

        std::fs::write(&path, b"").unwrap();
        assert_eq!(cursor.take_new(&path), None);

        std::fs::write(&path, vec![b'x'; 64 * 1024 + 1]).unwrap();
        assert_eq!(cursor.take_new(&path), None);
    }

    #[test]
    fn discardで残骸が消える() {
        let dir = temp_dir("discard");
        let path = prepare(&dir, 5).unwrap();
        std::fs::write(&path, b"\x1b]133;A\x07").unwrap();
        discard(&path);
        assert!(!path.exists());
    }
}
