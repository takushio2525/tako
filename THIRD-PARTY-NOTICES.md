# サードパーティ告知 / Third-Party Notices

tako 本体は [GPL-3.0-or-later](LICENSE) だが、配布物（`tako.app` / リリース zip）には
別ライセンスの第三者成果物を**改変せず**同梱している。ここはその一覧と告知。

tako itself is licensed under [GPL-3.0-or-later](LICENSE), but the distributed
artifacts bundle third-party works, unmodified, under their own licenses.
This file is the required notice for those works.

Rust クレートの依存関係はここには列挙しない（`Cargo.lock` と `cargo license` が正本）。
ここに書くのは**ソースツリーへ直接コピーして配布している**もの。

Rust crate dependencies are not listed here (`Cargo.lock` plus `cargo license` is the
source of truth). This file covers works **copied into this source tree** verbatim.

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
