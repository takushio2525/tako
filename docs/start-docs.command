#!/usr/bin/env bash
# このファイルをダブルクリックすると、ドキュメントサイトのローカルサーバーが立ち上がり、
# ブラウザ（Google Chrome があれば Chrome、無ければデフォルトブラウザ）が自動で開く。
# 終了するときはこのターミナルウィンドウで Ctrl+C → 閉じる。

set -e
cd "$(dirname "$0")"

# 初回のみ依存ライブラリをインストール
if [ ! -d node_modules ]; then
    echo "================================================================"
    echo "  初回起動: npm install を実行します（数分かかります）"
    echo "================================================================"
    npm install
    echo ""
fi

# 空きポートを探す（4321 から順に、使用中なら 1 ずつ増やす）
PORT=4321
while lsof -i :$PORT >/dev/null 2>&1; do
    PORT=$((PORT + 1))
done

# サーバー起動後にブラウザを自動で開く（3 秒待ってから）
open_url() {
    sleep 3
    if [ -d "/Applications/Google Chrome.app" ]; then
        open -a "Google Chrome" "http://localhost:$PORT"
    else
        open "http://localhost:$PORT"
    fi
}
open_url &

echo "================================================================"
echo "  tako ドキュメントサイトを起動します"
echo "  URL: http://localhost:$PORT"
echo "  終了するには Ctrl+C を押してください"
echo "================================================================"
echo ""

npm run dev -- --port "$PORT"
