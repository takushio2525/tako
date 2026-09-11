//! #1313 の再現 + 回帰: `config_io::atomic_write` の同一プロセス内の並行書き込み。
//!
//! tmp 名がプロセス単位までしか分かれていないと、A が `rename(tmp → 本番)` したあとに
//! B が**本番ファイルになった同じ inode** へ書き込む（B のハンドルは rename に付いていく）。
//! 読み手は「空」または「どちらの本文でもない中身」を観測し、B の rename も失敗する。
//! #638（`shell_integration::write_state_file`）/ #625（`tmux-backend.conf`）と同型。
//!
//! **単体テストではなく統合テスト（別バイナリ）に置く**。8 本のファイルを同時に開くので、
//! 同じプロセスで走る `ipc::tests::連続接続でfdが漏れない`（プロセス全体の fd の
//! 6 秒間最小値を見る）を押し上げうるため。
//!
//! A/B: `TAKO_1313_LEGACY=1` で修正前の tmp 名へ戻り、このテストは FAILED になる。

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tako_control::config_io::atomic_write;

const THREADS: usize = 8;
const WRITES: usize = 100;

/// テスト用一時ディレクトリの後始末。**一時ディレクトリ配下であることを検証してから**
/// 消す（変数名の取り違えで実ファイルを消す事故を構造的に防ぐ）
fn remove_temp_dir(dir: &Path) {
    assert!(
        dir.starts_with(std::env::temp_dir()),
        "一時ディレクトリ以外を削除しようとしている: {}",
        dir.display()
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// 置き場に残っている tmp ファイルの一覧（書き終えたあとは 0 件が正しい）
fn tmp_leftovers(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut left: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp."))
        .collect();
    left.sort();
    left
}

/// **待ちは状態待ち**（書き手の完了フラグ）で、実時間の比較はしない
#[test]
fn 同一プロセス内の並行書き込みでも途中状態が読まれない() {
    let dir = std::env::temp_dir().join(format!("tako-1313-race-{}", std::process::id()));
    remove_temp_dir(&dir);
    std::fs::create_dir_all(&dir).expect("置き場を作る");
    let path = dir.join("projects.yaml");

    // 長短 2 種類の本文。**長さが違う**ことが要点で、短い側が長い側の上に書かれると
    // 末尾が残って「どちらの本文でもない中身」になる = 途中状態を観測できる
    let long = format!("projects:\n{}", "  - key: long\n".repeat(400));
    let short = "projects: []\n".to_string();
    let valid = [long.clone(), short.clone()];

    let done = Arc::new(AtomicBool::new(false));
    let torn = Arc::new(Mutex::new(Vec::<String>::new()));
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let reads = Arc::new(AtomicUsize::new(0));

    let reader = {
        let path = path.clone();
        let done = Arc::clone(&done);
        let torn = Arc::clone(&torn);
        let reads = Arc::clone(&reads);
        let valid = valid.clone();
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                // 不在（1 回目の rename より前）は異常ではないので読めた回だけ数える
                if let Ok(seen) = std::fs::read_to_string(&path) {
                    reads.fetch_add(1, Ordering::Relaxed);
                    if !valid.contains(&seen) {
                        let mut g = torn.lock().expect("torn");
                        if g.len() < 8 {
                            // 全文は出さない。長さと先頭・末尾だけで形が分かる
                            let head: String = seen.chars().take(16).collect();
                            let tail: String = seen.chars().rev().take(16).collect::<String>();
                            g.push(format!(
                                "len={} head={head:?} tail(逆順)={tail:?}",
                                seen.len()
                            ));
                        }
                    }
                }
            }
        })
    };

    let writers: Vec<_> = (0..THREADS)
        .map(|i| {
            let path = path.clone();
            // 半々で長短を書く。どちらの上書き方向も起こす必要がある
            let content = if i % 2 == 0 {
                long.clone()
            } else {
                short.clone()
            };
            let errors = Arc::clone(&errors);
            std::thread::spawn(move || {
                for _ in 0..WRITES {
                    if let Err(e) = atomic_write(&path, &content) {
                        let mut g = errors.lock().expect("errors");
                        if g.len() < 8 {
                            g.push(e);
                        }
                    }
                }
            })
        })
        .collect();

    for w in writers {
        w.join().expect("書き手の合流");
    }
    done.store(true, Ordering::Relaxed);
    reader.join().expect("読み手の合流");

    let torn = torn.lock().expect("torn").clone();
    let errors = errors.lock().expect("errors").clone();
    let reads = reads.load(Ordering::Relaxed);

    // 読み手が 1 度も読めていないと検査が空回りする（検出力の確認）
    assert!(reads > 0, "読み手が本番ファイルを 1 度も読めていない");
    // 2 つの症状（途中状態の観測 / rename の失敗）は同じ競合の裏表なので一緒に報告する
    assert!(
        torn.is_empty() && errors.is_empty(),
        "並行書き込みが競合した（読み手は {reads} 回読んだ）: \
         読み手が観測した途中状態={torn:?} / 書き込みの失敗={errors:?}"
    );
    assert!(
        tmp_leftovers(&dir).is_empty(),
        "tmp が置き場に残っている: {:?}",
        tmp_leftovers(&dir)
    );
    let last = std::fs::read_to_string(&path).expect("最終状態を読む");
    assert!(valid.contains(&last), "最終内容がどちらの本文でもない");

    remove_temp_dir(&dir);
}
