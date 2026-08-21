# Active Context

> このファイルは AI が毎ターン上書きする現在状態のスナップショット。
> 過去ログは `progress.md` を見ること。

## 現在の対象（2026-08-21、#467 Windows 移植はスライス 9 まで完了）

- **スライス 9 は main へ入った**（PR #863 = `8f3401a`）。境界 **B9（スリープ防止）** と
  **B5 の検査側（ポート検知）** が Windows 実機で動く。macOS は挙動不変
- 残りは**スライス 7（PowerShell シェル統合。別 worker が並行中）とスライス 8（棚卸し）**

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

- **スライス 8（対応マトリクスの棚卸し）**。スライス 9 の実測表（plan のスライス 9 節）を
  `tako_sleep_guard` / ポート検知の Supported / Degraded の判断材料に使う
- スライス 9 が残した宿題: #724 症状②（WebView2 の借用 panic で abort）/
  #727（設定画面のスリープ系が macOS 前提）/ `setup` の対話フロー（L3 の蓋閉じ案内）

## 現フェーズで Read すべき設計書

- `.agent/plans/2026-08-windows-main-merge-wip.md`（スライス 9 節の実測表と申し送り・作法 12 項目）
- `.agent/plans/2026-07-windows-port-architecture.md`（境界 B5 / B9 の定義）
