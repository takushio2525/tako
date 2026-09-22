# Progress Log

> AI が作業完了時に**末尾へ追記**する時系列ログ。新しいものほど下。
> **1 エントリは 1〜3 行**（何を / どこを / 結果）。詳細は git log・Issue・PR・`.agent/plans/` に委ねる。

このファイルは `AGENTS.md` から `@import` されるので**毎ターン全文が読み込まれる**。
予算（直近 5 作業日 / 20 エントリ / 12 KB）を超えたぶんは `progress-archive.md` へ
1 行で移る。移送は `tako context-budget fix` が行う（冪等・本文は改変しない・
全文は git 履歴に残る）。規約の全文は `AGENTS.md`「起動時ロードの予算」節。

## 追記フォーマット

```markdown

## YYYY-MM-DD（#Issue 一言）
- {何を / どこを / 結果}
- 関連コミット: `{shortsha}` `[種別] 概要`
- 次: {次にやることがあれば 1 行}
```

---

## 2026-09-22（#1524: 標準 tako setup の依存 [y/N] を setup_deps::offer_and_install の 1 呼び出しへ寄せた）
- #1499 は判断 / 実行 / 再検出を寄せたが**表示と入力は呼び手に残し**、#1509 が束ねた `offer_and_install` も #1501 と同時進行だったため標準 setup は差分ゼロのままだった（体験を組む層が 2 か所）。`setup.rs` の `offer_dep_install` / `print_dep_install_plan` / `print_dep_manual_hint` を消し、`DepPromptIo`（stderr + stdin + 字下げ 6 マス）を渡す 1 呼び出しへ。再検出も中で済むので `resolve` の 2 度引き（ログインシェル起動）と `find_command("brew")` の先引きが各 1 回消えた
- 同時に判明 = `[警告] … インストール後も検出できません` は**到達しない分岐**だった（導入器が成功して引けないときは `install` が `Err` を返し `[警告] {e}` 側へ落ちる）。実測でも before / after とも 0 件なので削除した
- 実測: 隔離実走 13 本の出力が**バイト単位で一致**（文面の差分 0）。`test-setup-deps-prompt-1499.sh` 52 PASS / `test-remote-setup-deps-1509.sh` 49 PASS / `test-setup-continue-1501.sh` 81 PASS / `test-tako-cli-path-1502.sh` 49 OK。番犬 `issue1524_setup_prompt_single_impl_watchdog` 4 本・注入 10 通りすべて FAILED → 戻して緑。workspace 5107 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-22（#1525: AGENTS.md の予算の余地を作り、activeContext を現在状態へ戻した）
- 予算の主因はコマンド表ではなく**リリース運用の本文**（71 行 6121 バイト）だった。両 OS 同時 / 夜間リリース / 版数の予約を `.agent/release.md`（新設）へ移し、AGENTS.md には不変条件 2 行 + バックティック参照だけを残した（`@import` にはしない）。表からは実測値（45c / 95c）と診断オプション列挙の 2 行ぶんを `commands.md` の同じ行へ寄せた
- 実測: **30657 → 25309 バイト**（上限 30720 の 99.8% → 82.4%）。消えた実質 65 行のうち 63 行は release.md に全文一致で残り、残る 2 行は commands.md 側にセル単位で全文あり = **消えた情報 0**。`context_budget` 11 passed / `no_personal_data` 6 passed / fmt 差分なし / docs 生成 2 本とも同期
- `.agent/activeContext.md` は 9/22 の状態（main = `7bfeb6c` / 着地 8 件 / #1500 のレーン A・C / 判断待ち 4 件）へ 77 行で書き直した

## 2026-09-22（#1503: agent CLI の probe に待ち時間の上限を付けた）
- 真因は setup の probe が全部 `Command::output()` で上限を持たないこと。とくに `claude mcp list` は登録済み MCP サーバへ 1 台ずつ繋ぐので 1 台無応答だと返らない（#1500 の R4 = 無言で 6 分ハング）。待ちの 1 実装を `tako_core::probe` へ置き、probe（既定 15 秒 = 実測 `mcp list` 4.57〜5.26 秒の約 3 倍）と dispatch `SetupRun`（既定 600 秒）を両方そこへ通した。超過は「確認できません（N 秒応答なし）」を出してその段だけ諦め、setup は完走する（#1501 の契約は維持）。上限を外す指定は作らない（env は値を変えるだけ・0 / 不正は既定へ）。読み切りにも予算を掛け（孫がパイプを持つと `output()` は返らない）、`Command` の組み立ても境界の中へ入れた（`platform_parity` の baseline は不変）
- 途中で既存の番犬 2 本が自分の変更を捕まえた: `platform_parity`（#628。素の `Command::new` を境界の外に残していた）と `shell_scripts`（#837。`（${LEFTOVERS}）` の波括弧漏れ）。並列負荷下で単体テストが予算 30 秒を丸ごと使う回があったので、読み切りの猶予は 2 秒で頭打ちにした
- 実測: `scripts/test-setup-probe-timeout-1503.sh` **28 PASS 0 FAIL**（CI 登録。修正後 7 秒で完走 / A/B `TAKO_1503_LEGACY=1` は 40 秒の締め切りまで無言で固まる）。番犬 6 本 + 注入 11 通りすべて file:line 名指しで FAILED → 戻して緑。回帰 5 本（#1499 / #1501 / #1502 / #1509 / multiagent）全緑・workspace 5127 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。[提案] 5 件は #1531〜#1535 へ

## 2026-09-22（#1548: docs 生成が古い tako バイナリを黙って選ばないようにした）
- `takoBin()` は debug → release の順に**存在する方**を返すだけで版を見ず、2 スクリプトに複製されていた。開発ツリーで debug だけ古いと（報告時の実測: debug v0.8.13 / release v0.8.17）`--check` が「同期していません」の**偽の赤**を出し、`--check` 無しでは docs が 8 日前へ静かに巻き戻る（CI はフレッシュビルドなので緑のまま手元だけが嘘をつく）。選択を `scripts/lib/tako-bin.mjs` の 1 実装へ寄せ、選んだバイナリの `--version` と `Cargo.toml` の `[workspace.package] version` が違えば生成も検査もせずに 1 行の理由で落とす
- 倒した判断: **debug が古いとき release へ黙って逃げない**（手元の他スクリプトも既定は debug。直すのは 1 コマンドなので最簡形を出す = #322）。明示指定は新オプションではなく既存の env 名 `TAKO_BIN`（相対はリポジトリルート基準）。失敗はスタックトレースではなくメッセージ 1 本（`main()` + try/catch）
- 実測: `scripts/test-gen-docs-bin-1548.sh` **46 PASS 0 FAIL**（CI 登録。テンポラリの偽リポジトリ + 版を埋め込んだスタブ。古い版には別内容の JSON を返させて巻き戻しを実際に観測）。注入 11 通りすべて FAILED → 戻して緑。生成内容・md・Rust 側は不変（生成物が本物と 1 バイト同じ）

## 2026-09-22（#760: 自動命名がシェルの実行ファイルパスを掴まないようにした）
- Windows のコンソールタイトルはシェル / psmux **自身**のフルパスなので、`heuristic_plan()` が OSC タイトルを最優先すると 16 文字で切った `C:\Program Files` / `C:\Users\<user>\A` が**全タブ同じ名前**になっていた（9/9 実機レビュー = 初回起動の第一印象）。`shell_exe_material()` の関門で捨てて cwd へ落とす。cwd の末尾要素は `Path::file_name` をやめ `last_segment()` へ（Unix は `\` を区切りにしないので Windows 形の素材が丸ごと 1 要素になる。Windows 形と判定したときだけ `\` も見る）
- 指紋（cwd / OSC タイトル / 実行状態）はシェル統合の無い Windows で 3 つとも不変 = 命名がペインを開いた直後の 1 回で終わる件は、`tick()` に「同じ指紋のままやり直した回数」を持たせ `STALE_RETRY_DELAYS`（5 分 → 20 分 → 60 分）で再発火する形にした（使い切れば静まる）。画面末尾を指紋へ混ぜる本命は `main.rs` 側なので #1568 へ
- 実測: 実 GUI の A/B（隔離・tako-vd。関門を外すと `C:\Program Files`、入れると cwd 由来 + 診断 1 行）・注入 7 通りすべて file:line 名指しで FAILED → 戻して緑・workspace 5113 passed 0 failed・clippy 3 宇宙 0・check-windows error 0

## 2026-09-23（#1539: MCP ツールカタログを起動時ロードの予算対象にした）
- 予算表（progress 12 KB / AGENTS 30 KB / import 40 KB / global 24 KB / system prompt 24 KB ≒ 130 KB）に MCP カタログの項目が無く、**それより大きい 201,535 バイト / 152 本が誰にも測られていなかった**。`ItemKind::McpCatalog` と `MCP_CATALOG_MAX_BYTES`（210 KB）を足し、採取を `tako_control::context_budget::mcp_catalog` の 1 実装へ寄せて `inventory` に載せた（CLI 表示 / `--json` / MCP `tako_context_budget` の 3 経路が同じ 1 件を見る）。測るのは snapshot ではなく実行時に組み立てた `mcp::tools()`。1 本の JSON なので行数は測らない（`lines: 0`）
- 上限をバイトで置いた根拠 = 実トークナイザ tiktoken `o200k_base` で **52,028 トークン**（3.87 B/tok。参考 `cl100k_base` 62,997）に対し、日本語主体で較正した既存 `estimate_tokens` は 94,934 と**約 1.8 倍**に出る。210 KB は現状 +6.7% で、#1540 の圧縮後に締め直す前提をコードのコメントへ明記。`catalog.rs` は 1 行も触らない（#1540 と並走）
- 実測: 出荷版との A/B（出荷版は `items` にも `budget` にも `mcp_catalog` のキーが無い）・注入 6 通りすべて FAILED → 戻して緑・棚卸しの上乗せは同一 debug ビルドの A/B で 0.01→0.02 秒・AGENTS.md 25309→25638 バイト（上限 30720）・workspace 5136 passed 0 failed・clippy 3 宇宙 0・check-windows error 0。[提案] 3 件は #1567 へ

## 2026-09-23（#1545: getting-started を 9/22 の setup 着地へ追従させた）
- 旧ページは **#1502 が明示的に否定した PATH 手順**（`.app` の実行ファイル置き場を `~/.zshrc` の `export PATH` へ）を読者に指示し続け、同じページ内で「CLI は導入しない」（トラブルシューティング）と「導入 → PATH → ログインの 3 段を案内する」（tip）が矛盾していた。FR-2.14.5 の現行仕様（`$HOME/.local/bin/tako` の symlink + `~/.zprofile`）へ差し替え、旧手順を踏んだ読者向けに「その行は消してよい」の移行案内を足した
- 任意依存が `[y/N]` で導入まで通ること（#1499 / #1509 / #1524）と、詰まった段があっても止まらず「残り N 件」で終わること（FR-2.14.12 = #1501）を追記。`--version` / `--changes` / `--check` の例は現行ビルド（v0.8.17 / rev 19）の実出力へ。「質問ゼロ」3 行を言い直し、次のステップに `/features/remote/` と `/guides/keyboard-shortcuts/` を足した
- 実測: 貼った `--check` は隔離 HOME の再実行と**マスク以外バイト単位で一致**（49 行）・`Contents/MacOS` は docs 全体で 0 hit・`npm run build` 32 ページ警告 0・`verify-og` 31 ページ OK
