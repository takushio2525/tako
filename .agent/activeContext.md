# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-14、Issue #792 handoff の知識 / 実行状態の分離）

- ブランチ `improve/792-handoff-split`（worktree `~/dev/tako-wt-792`）で実装完了 → PR 待ち
- 引き継ぎファイルを **2 節**へ分離: `## 知識（マシン非依存）` / `## 実行状態（このマシン限定）`
  （英語表示なら `## Knowledge (machine-independent)` / `## Runtime state (this machine only)`）
- **旧書式は従来どおり動く**（後方互換が本質）: 書式に関わらず全文を後任へ渡し、
  旧書式なら「番号は実態で確認 + 次の更新で 2 節へ書き直せ」を後任プロンプトに添える
  = 自然な更新で移行する。本番の実 handoff 5 本は読み取りだけで検証（全部 legacy / 全文保持）
- ついで（Issue の 3 項目目）: `_system_prompt_*` を Local(GENERATED) としてカタログ登録し、
  被覆テストの走査を `join(format!(…))` へ拡張（動的名が網に掛かるようにした）

## 書式の正本（触るときはここから）

- `tako_core::handoff`: `HandoffSection` / 見出し 4 定数 / `section_of_line`（寛容な前方一致。
  番号付き・半角括弧・強調・語尾省略を吸収）/ `split_handoff` / `handoff_template`
- 判定できなければ Legacy = 安全側（実態と突き合わせろ）に倒れる設計。誤認識で壊れない
- 応答の `handoff_format`（`sectioned` / `legacy` / 未作成は null）+ `handoff_sections` は
  `tako_orchestrator_self` と `tako_orchestrator_handoff` の両方が返す
- prompt 側の見出し文字列と定数のドリフトは tako-control のテストが落とす

## 不変条件

- 引き継ぎ内容は**節に切って渡さない**（認識漏れが黙って落ちる）。全文 + 節ごとの扱いを添える
- 既存 handoff ファイルの**一括変換はしない**（後方互換で読めるので自然更新に任せる）
- pane / tab 番号は実行状態節にだけ書く（知識に混ざると別デバイスで誤指示の元になる）

## 次の一手（master 判断）

- PR（`Closes #792` / `Refs #513 #749`）→ CI 緑 → squash merge → install
- #513 側の判断: 知識節だけを共有カタログへ Shared 追加するか（本 PR は分離までで止めている）

## 現フェーズで Read すべき設計書

- 引き継ぎ: `.agent/orchestrator.md`「master の自動ハンドオフ」→「引き継ぎファイルの書式」
- 要件: `.agent/requirements.md` FR-2.24.8（書式）/ FR-5.14.12（動的名の分類）
