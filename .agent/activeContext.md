# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21、#467 Windows 移植スライス 9 = スリープ防止 / 蓋閉じ継続 / ポート検知）

- ブランチ `windows/467-slice9-sleep-ports`（worktree `~/dev/tako-wt-s9`）→ PR #863
- 境界 **B9（スリープ防止）** と **B5 の検査側（ポート検知）** を main へ移植。macOS は挙動不変

## 実装（完了）

- `platform::power`（#524）= `PowerCreateRequest` + `PowerSetRequest`。
  `platform::lid`（#697）= 電源プランの `GUID_LIDCLOSE_ACTION` を倒して**必ず戻す**（起動時 + 終了時）
- `sleep_guard` は保持判定 / 蓋閉じ判定を純関数 2 本へ集約し両 OS が同じ 1 本を通る。
  能力は `lid_control_supported` / `lid_state_detectable` / `lid_requires_privileged_setup` /
  `lid_setup_pending` の 4 本で表に出し、「sudoers」という macOS 固有の手段を呼び出し側から隠す
- `ports::pane_key()` にペイン配下の判定材料を閉じ込め（macOS = rdev / Windows = 子 pid）+
  器（psmux）自身の LISTEN を落とす（#724 症状①）
- `agents::process_parent_map` を境界 B5 経由に（`ps` 直叩きだと Windows で常に空 =
  sleep guard の既定モードが死んでいた）

## 次の一手

- PR #863 を squash merge → #467 へ実測を報告 → worktree 掃除（mac / Windows 両方）
- 残りは**スライス 7（PowerShell シェル統合。別 worker が並行中）とスライス 8（棚卸し）**

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（スライス 9 節の実測表と申し送り・作法 12 項目）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界 B5 / B9 の定義）
