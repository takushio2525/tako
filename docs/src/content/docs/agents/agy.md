---
title: Antigravity CLI
description: agy を tako の worker として使う手順と、worker 専用であること・利用制限が読めないことなどの差分
---

`agy`（Antigravity CLI）は **worker 専用**の系統です。worker としては claude とほぼ同じように動きますが、**master / solo にはなれません**。master には claude か codex が別に要ります。

## 入れる

```bash
tako setup bootstrap install --agent agy
```

何をどこに入れるかは実行前に表示されます。先に見るだけなら `--dry-run`、いま入っているかどうかだけなら `tako setup bootstrap status --agent agy` です。

公式の手順を直接使う場合は次のとおりです。

```bash
curl -fsSL https://antigravity.google/cli/install.sh | bash
```

Windows の公式手順は `irm https://antigravity.google/cli/install.ps1 | iex` ですが、**Windows で tako が導入を代行できるのは claude だけ**です。agy は状態の確認と手順の案内までなので、表示されたコマンドをご自身で実行してください。

単一バイナリなので本体は macOS では `~/.local/bin/agy` だけです（Windows は `%LOCALAPPDATA%\agy\bin`）。更新は agy が実行のたびに自分で行います。

## ログインする

```bash
agy
```

agy には専用のログインコマンドがありません。**引数なしの起動がサインインの入口**です（有効なセッションが残っていれば何も聞かれずに起動します）。ブラウザでの操作が要るため tako は代行しません。ログインが済んだら `tako setup` をやり直してください。

## tako に繋ぐ（MCP）

```bash
tako setup-mcp --agent agy
```

引数なしの `tako setup-mcp` でも、導入済みなら agy は対象に入ります。書き込み先は `~/.gemini/config/mcp_config.json` です。

:::caution[agy はこの登録が必須です]
codex の master・worker には、tako が起動のたびに MCP 設定をその場で渡す経路もあります。**agy にはその手段が CLI 側にありません**（起動時に MCP を差し込むオプションが無い）。そのため agy の worker から tako を操作できるかどうかは、この恒久登録が済んでいるかどうかだけで決まります（[#986](https://github.com/takushio2525/tako/issues/986)）。

登録さえ済んでいれば、agy は親プロセスの環境変数をそのまま MCP サーバーへ渡すので、ペインを省略した `tako_set_title` などは claude と同じように自分のペインへ当たります。
:::

## worker として使う

```bash
tako orchestrator spawn --agent agy --project app --prompt "テストを通してください"
```

毎回 agy にしたいなら `tako orchestrator profiles set default --worker-agent agy` で既定を変えられます。

完了検知は claude と同じ速さです（[#1033](https://github.com/takushio2525/tako/issues/1033) で画面推定をやめ、agy が書く実況ログを一次シグナルとして読むようになりました）。報告も会話ログから取れます。

:::note[思考の深さも指定できます]
agy にも `--effort low|medium|high` があるので、tako からの effort 指定はそのまま届きます。モデル名に `(High)` のような表記を含むモデルでも同じです。使えるモデルは `tako setup models --agent agy` で実際の一覧を引けます。
:::

## master には使えません

```bash
tako master   # プロファイルの master_agent に agy は指定できません
```

agy を master / solo として起動しようとすると、**起動前にエラー**になります。worker 専用として設計されているためです（[#127](https://github.com/takushio2525/tako/issues/127)）。この前提を実機で見直す作業は [#987](https://github.com/takushio2525/tako/issues/987) で追跡しています。

agy だけが入っている環境でも `tako setup` は完了しますが、`tako master` を使う前に claude か codex を追加してください。

## 利用制限は表示できません

ステータスバーの利用制限表示を agy へ切り替えると、値ではなく「未対応」と出ます。数字が取れないのに「--」（未取得）と出すと「そのうち取れる」と読めてしまうため、取得手段が無いことを明示しています。

調べた結果は次のとおりです（[#357](https://github.com/takushio2525/tako/issues/357) で 1.1.3 / 1.1.4 を調査し、[#985](https://github.com/takushio2525/tako/issues/985) で 1.1.22 を再確認しました）。

- **ローカルに残量のファイルがありません**（`~/.agy/` が存在せず、`~/.antigravity/` はエディタ拡張の残骸だけ）
- **`agy --help` に usage / quota 系のサブコマンドがありません**。バイナリに出てくる `RateLimit` はどれも PR レビュー設定や内部ライブラリの名前で、利用枠とは関係ありません
- **そもそも「5 時間枠 / 週枠とリセット時刻」という形をしていません**。agy の残量は前払いの AI クレジット残高で、時間が経てば戻るものではありません
- **残高は対話中の `/credits` モーダルの中にしか出ません**。読むには worker の画面を操作する必要があり、作業中のペインを乱さずに取る口がありません

同じ理由で、**利用上限で止まったことの検知**と**上限解除後の自動再開**も agy では成立しません。クレジットが尽きた場合は待っても戻らないので、買い足す以外の出口が無いためです。

## Claude Code との差分

現時点で 47 件中 21 件が「対応」です。全件の内訳と理由は [Antigravity CLI を選ぶと落ちるもの](/agent-support/#antigravity-cli-を選ぶと落ちるもの) にあります。手元で最新を引くなら次を実行してください。

```bash
tako agent-support --agent agy
```

master 関連（上記）と利用制限（上記）のほかで効くのは次の 2 点です。

- **会話の復元がまだ配線されていません**。`tako sessions resume` と、PC 再起動後にペインを会話ごと戻す復元は claude 専用です（[#984](https://github.com/takushio2525/tako/issues/984) / [#1238](https://github.com/takushio2525/tako/issues/1238)。手段自体は上流にあり、`agy --conversation` を手で実行すれば戻せます）
- **契約プランを検出できません**。認証済みかどうかは分かりますが、プランの規模が取れないので、`tako setup` の推奨プロファイルは agy 単独では規模を決められません
