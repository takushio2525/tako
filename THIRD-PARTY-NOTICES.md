# サードパーティ告知 / Third-Party Notices

tako 本体は [GPL-3.0-or-later](LICENSE) だが、配布物（`tako.app` / リリース zip）には
別ライセンスの第三者成果物を同梱している（そのまま同梱したものと、改変して取り込んだものがある）。
ここはその一覧と告知。

tako itself is licensed under [GPL-3.0-or-later](LICENSE), but the distributed
artifacts bundle third-party works under their own licenses — some verbatim,
some adapted. This file is the required notice for those works.

Rust クレートの依存関係はここには列挙しない（`Cargo.lock` と `cargo license` が正本）。
ここに書くのは**ソースツリーへ直接取り込んで配布している**もの。

Rust crate dependencies are not listed here (`Cargo.lock` plus `cargo license` is the
source of truth). This file covers works **incorporated into this source tree**.

---

## alacritty_terminal（PTY IO ループの移植 / adapted PTY IO loop）

- バージョン / Version: **0.26.0**
- 用途 / Used for: PTY の読み書きと VT パースを回す IO スレッド。
  upstream の `EventLoop::spawn()` は reader スレッドのスタックへ 1 MiB の配列を置き、
  ペイン 1 枚につき約 1 MB が常駐していた（Issue #817）。この定数は `pub(crate)` で
  外から下げられないため、ループを tako 側へ取り込んで**読み取りバッファだけ
  ヒープへ移した**
- 取り込み先 / Incorporated into: `crates/tako-core/src/pty_loop.rs`
- 上流 / Upstream: https://github.com/alacritty/alacritty
  （`alacritty_terminal/src/event_loop.rs`）
- ライセンス / License: Apache-2.0
  （全文: https://www.apache.org/licenses/LICENSE-2.0 。
  クレート同梱の `LICENSE-APACHE` と同一）

### 加えた変更 / Changes made

Apache-2.0 セクション 4(b) が要求する「改変の明示」:

- 読み取りバッファをスタックの `[0u8; READ_BUFFER_SIZE]` から、64 KiB 始まりで
  必要時のみ `READ_BUFFER_SIZE` まで伸びるヒープの `Vec<u8>` へ置き換えた（Issue #817）
- tako が使っていなかった `ref_test`（PTY 出力の記録ファイル書き出し）と
  `drain_on_exit` を削除した
- ログ出力を `log` クレートから tako が使う `tracing` へ差し替え、文言を日本語にした
- IO スレッドで panic していた 2 箇所（送信側が全部落ちたときのチャネル受信と、
  write interest の再登録失敗）を、記録してループを畳む静かな終了へ倒した。
  tako はペイン単位でセッションを捨てるので、IO スレッドの panic は事故になりやすい
- Unix でのみ `pub(crate)` の `PTY_READ_WRITE_TOKEN` / `PTY_CHILD_EVENT_TOKEN` を
  同じ値で再定義した（Windows は upstream が `pub` で出しているのでそれを使う）。
  値のずれは実 PTY を張る単体テストで検出する
- 型名を tako の文脈に合わせて改名した（`EventLoop` → `PtyLoop`、
  `EventLoopSender` → `LoopSender`、`EventLoopSendError` → `SendError`）

The read buffer was moved from a 1 MiB stack array to a heap `Vec<u8>` that starts at
64 KiB and grows on demand; `ref_test` and `drain_on_exit` were dropped; logging was
switched from `log` to `tracing`; the Unix-only `pub(crate)` poller tokens were
re-declared with the same values (guarded by a unit test against a real PTY); and the
types were renamed to fit tako's naming.

### GPL との関係 / Relationship to the GPL

Apache-2.0 は GPL-3.0 と**一方向に互換**（Apache-2.0 のコードを GPL-3.0-or-later の
著作物へ取り込める）であり、取り込んだ結果は tako 全体と同じ GPL-3.0-or-later で配布する。
Apache-2.0 が課す義務 — 出所・ライセンスの表示と改変の明示 — は本節と
`crates/tako-core/src/pty_loop.rs` 冒頭のコメントで満たす。

Apache-2.0 is one-way compatible with GPL-3.0: Apache-2.0 code may be incorporated into a
GPL-3.0-or-later work, and the result is distributed under tako's GPL-3.0-or-later terms.
The obligations — attribution, license notice, and stating the changes — are satisfied by
this section and by the header comment of `crates/tako-core/src/pty_loop.rs`.

---

## zsh-autosuggestions

- バージョン / Version: **v0.7.1**
- 用途 / Used for: tako 内の zsh に履歴ベースの入力予測を出す（Issue #600）。
  `crates/tako-core/shell-integration/zsh-autosuggestions/` に同梱し、
  シェル統合の ZDOTDIR 注入経路から tako 内のシェルにだけ読み込む
- 同梱物 / Bundled files:
  `crates/tako-core/shell-integration/zsh-autosuggestions/zsh-autosuggestions.zsh`
- 上流 / Upstream: https://github.com/zsh-users/zsh-autosuggestions
- 出所の詳細 / Provenance:
  [`crates/tako-core/shell-integration/zsh-autosuggestions/PROVENANCE.md`](crates/tako-core/shell-integration/zsh-autosuggestions/PROVENANCE.md)
- ライセンス / License: MIT
  （全文: [`crates/tako-core/shell-integration/zsh-autosuggestions/LICENSE`](crates/tako-core/shell-integration/zsh-autosuggestions/LICENSE)）

```
Copyright (c) 2013 Thiago de Arruda
Copyright (c) 2016-2021 Eric Freese

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

### GPL との関係 / Relationship to the GPL

zsh-autosuggestions は tako の Rust バイナリへリンクされるライブラリではなく、
**tako が起動した zsh が読み込む独立したシェルスクリプト**として同梱・配置される
（`<data_dir>/shell-integration/zsh-autosuggestions/`）。MIT は GPL-3.0 と両立する
寛容型ライセンスであり、GPL-3.0-or-later の配布物へ同梱するにあたっての義務は
「著作権表示とライセンス全文を添付すること」— それを本ファイルと同梱 `LICENSE` で満たす。

zsh-autosuggestions is not linked into the tako binary; it is bundled and installed as a
standalone shell script that the zsh started by tako sources. MIT is GPL-compatible, and
its only obligation for redistribution — preserving the copyright notice and license text —
is satisfied by this file and the bundled `LICENSE`.
