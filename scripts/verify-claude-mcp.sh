#!/usr/bin/env bash
# Claude Code 実機検証: tako の内蔵 MCP サーバーへ「設定ゼロ」で接続できることを確認する。
#
# GUI を起動せず、tako-control の example ホスト（mcp_host）が IPC + MCP + dispatch を
# 立ち上げて TAKO_* 環境変数を注入した状態でこのスクリプト自身を再実行し、その中で
# 実物の `claude -p` を 2 経路で走らせる:
#   1/2 stdio ブリッジ（`tako mcp serve`）— Claude Code への登録形態と同じ
#       （`TAKO_VERIFY_BIN=<…/tako.app/Contents/MacOS/tako>` で配布物の形 = `tako-mcp` への引き渡しを通す。#2021）
#   2/2 Streamable HTTP（TAKO_MCP_URL + Bearer トークン）
#
# ユーザーのグローバル claude 設定は変更しない（--mcp-config + --strict-mcp-config を使用）。
# 実運用の登録コマンド（1 回だけ）: claude mcp add --scope user tako -- <tako のパス> mcp serve
#
# 注意: claude の実行には認証済みの Claude Code とネットワークが必要。CI では実行しない。
#       トークンは検証プロセス内の一時値で、終了とともに無効になる。
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/lib/queue.sh # run_heavy = 重い処理を順番待ちへ（#1994）

# 外側か内側（example ホストの中で再実行された側）かは専用の印で見分ける（#1970）。
# TAKO_SOCKET は本番 tako のペインにもあるので印にならない（それで見分けていた頃は、本番のペインから
# 回すと外側を飛ばし、本物の claude が本番 tako の MCP でペインを分割しうる形だった）
if [ -z "${TAKO_VERIFY_CLAUDE_MCP_INNER:-}" ]; then
  # 外側: ビルドし、本番 tako の env（ホストが注入しない mod の env も）を落としてから
  # example ホスト内で自分を再実行する
  run_heavy cargo build -p tako-cli --quiet
  unset TAKO_SOCKET TAKO_TOKEN TAKO_PANE_ID TAKO_TAB_ID TAKO_CLI TAKO_MCP_URL CLAUDE_CODE_PLUGIN_DIRS \
    TAKO_ORCHESTRATOR_ROLE
  export TAKO_VERIFY_CLAUDE_MCP_INNER=1
  exec cargo run -p tako-control --example mcp_host --quiet -- bash "$0"
fi

# 叩くブリッジ（既定は dev の置き場 = 同じプロセスで続ける形）。.app の中の `Contents/MacOS/tako` を
# 渡すと、隣の `tako-mcp` へ exec で引き渡される配布物の形を通す（#2021）
TAKO="${TAKO_VERIFY_BIN:-$PWD/target/debug/tako}"
fail() { echo "NG: $1" >&2; exit 1; }

echo "== 1/2 stdio ブリッジ（tako mcp serve）経由で tako_list_panes =="
out=$(claude -p "tako の MCP ツール tako_list_panes を呼び、返ってきた JSON をそのまま出力して" \
  --strict-mcp-config \
  --mcp-config "{\"mcpServers\":{\"tako\":{\"command\":\"$TAKO\",\"args\":[\"mcp\",\"serve\"]}}}" \
  --allowedTools "mcp__tako__tako_list_panes" 2>&1) || fail "claude の実行（stdio）: $out"
grep -q '"tabs"' <<<"$out" || fail "stdio 経由で tabs JSON が返らない: $out"
echo "OK: stdio ブリッジで list が通った"

echo "== 2/2 Streamable HTTP（TAKO_MCP_URL）経由でペイン分割 =="
out=$(claude -p "tako の MCP ツール tako_split_pane で右に新しいペインを作り、返ってきた新ペイン ID を出力して" \
  --strict-mcp-config \
  --mcp-config "{\"mcpServers\":{\"tako\":{\"type\":\"http\",\"url\":\"$TAKO_MCP_URL\",\"headers\":{\"Authorization\":\"Bearer $TAKO_TOKEN\",\"X-Tako-Pane\":\"$TAKO_PANE_ID\"}}}}" \
  --allowedTools "mcp__tako__tako_split_pane" 2>&1) || fail "claude の実行（http）: $out"
# 分割が実際に起きたかをホスト側の状態（tako list のツリー）で確認する
tree="$("$TAKO" list)" && grep -q '"type": "split"' <<<"$tree" || fail "HTTP 経由の split がツリーに反映されていない"
echo "OK: Streamable HTTP で split が通った（claude の応答: $(head -1 <<<"$out")）"

echo "VERIFY_CLAUDE_MCP_OK"
