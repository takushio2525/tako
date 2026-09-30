# セキュリティポリシー / Security Policy

tako の脆弱性を見つけたら、**公開の Issue や Pull Request には書かず**、下の非公開の窓口から知らせてください。
修正の前に内容が公開されると、使っている人が危険にさらされます。

If you find a vulnerability in tako, please **do not report it in a public issue or pull request**.
Use one of the private channels below instead, so that users are not put at risk before a fix is available.

## 報告の窓口 / How to report

次のどちらかで送ってください。

1. **GitHub の非公開の脆弱性報告（推奨）**: [Security タブ](https://github.com/takushio2525/tako/security)の
   「Report a vulnerability」から送ってください（[報告の画面を直接開く](https://github.com/takushio2525/tako/security/advisories/new)）。
   報告は運営者とあなたにだけ見え、修正と公開までのやり取りもその場で続けられます。GitHub のアカウントが必要です。
2. **メール**: GitHub のアカウントが無い・使いたくない場合は、[contact@takushio2525.com](mailto:contact@takushio2525.com)
   へ送ってください。件名の先頭に `[tako security]` と付けてもらえると見落としません。

Please use either of the following:

1. **GitHub private vulnerability reporting (preferred)**: open the [Security tab](https://github.com/takushio2525/tako/security)
   and click "Report a vulnerability" ([open the report form directly](https://github.com/takushio2525/tako/security/advisories/new)).
   Only the maintainer and you can see the report, and you can discuss the fix and its disclosure with the maintainer there. A GitHub account is required.
2. **Email**: if you do not have or prefer not to use a GitHub account, write to
   [contact@takushio2525.com](mailto:contact@takushio2525.com). Starting the subject with `[tako security]` helps it not get missed.

脆弱性ではない不具合の報告や機能の要望は、[GitHub の Issue](https://github.com/takushio2525/tako/issues) へお願いします。
Issue は誰でも読める状態で公開されるので、個人情報やトークン、画面のログをそのまま貼らないでください。
脆弱性以外でも、個人情報に関わること・公開したくない問い合わせは上のメールへ送ってください。

For bugs that are not vulnerabilities and for feature requests, please use [GitHub Issues](https://github.com/takushio2525/tako/issues).
Issues are public, so do not paste personal information, tokens, or terminal logs as they are.
Other questions that involve personal information or that you do not want to make public can go to the email address above.

## 対象の版 / Supported versions

tako は [GitHub Releases](https://github.com/takushio2525/tako/releases) で、安定版とテスト版（pre-release）の 2 系統を配布しています。
修正は新しい版として出し、古い版へは戻しません（バックポートしません）。

tako ships two channels on [GitHub Releases](https://github.com/takushio2525/tako/releases): stable releases and test releases (pre-releases).
Fixes are shipped as a new release and are not backported to older versions.

| 版 / Version | 修正の対象 / Supported |
|---|---|
| 最新の安定版 / Latest stable release | ✅ |
| 最新のテスト版 / Latest test release (pre-release) | ✅ |
| それより前の版 / Older versions | ❌ |

報告の前に、できれば最新の版で再現するかを確かめてください。確かめられない場合も、そのまま送ってもらってかまいません。

If you can, please check whether the issue still reproduces on the latest release. If you cannot, please send the report anyway.

## 対象になるもの / Scope

このリポジトリにあるもの（アプリ本体・`tako` CLI・MCP サーバー・リモートアクセスとスマホ用の画面・インストーラー・
ドキュメントサイト）が対象です。たとえば、ほかの利用者やほかの端末から tako を操作できてしまう、許可していない端末が
リモートアクセスでファイルやペインに触れられる、トークンや画面の内容が意図せず外へ出る、といった問題です。

次のものは対象外です。

- tako が起動するほかのソフトウェア（Claude Code・Codex・tmux など）そのものの脆弱性。それぞれの開発元へ報告してください
- あなたが確認を外すなどして許可した範囲で、AI エージェントが行った操作

Everything in this repository is in scope: the app, the `tako` CLI, the MCP server, remote access and the phone UI,
the installers, and the documentation site. Examples include letting other users or other machines control tako,
letting an unauthorized device reach files or panes through remote access, or leaking tokens or terminal contents unintentionally.

The following are out of scope:

- Vulnerabilities in other software that tako launches (Claude Code, Codex, tmux, and so on). Please report them to their developers.
- Actions an AI agent takes within permissions you granted, for example after turning off its confirmation prompts.

## 報告に書いてほしいこと / What to include

- tako の版（`tako --version`）と、OS とその版 / The tako version (`tako --version`) and your OS and its version
- 入れ方（Homebrew・zip・インストーラー・ソースからのビルド） / How you installed it (Homebrew, zip, installer, or built from source)
- 再現の手順（できれば最小の手順） / Steps to reproduce, ideally minimal
- 何ができてしまうか（影響） / What an attacker can do (impact)
- あれば、確かめに使ったコードやログ / Proof-of-concept code or logs, if you have them

ログや画面の写しを添えるときは、トークン・パスワード・実名などを伏せてから送ってください。

Before attaching logs or screenshots, please mask tokens, passwords, real names, and similar information.

## 返答と公開の流れ / Response and disclosure

tako は個人で開発しているため、決まった期限での対応は約束できません。そのうえで、次の流れで進めます。

1. 受け取ったことを、目安として 7 日以内に返します。返事が無いときは、もう一度知らせてください
2. 再現を確かめ、影響の大きさと直し方をあなたと相談します
3. 修正した版を出したあと、GitHub Security Advisory として内容を公開します。希望があれば、報告者としてお名前を載せます

修正した版を出すまでは、内容の公開を控えてください。時間がかかりそうなときは、公開の時期を相談して決めます。

tako is developed by an individual, so no fixed response deadlines can be promised. With that in mind, reports are handled as follows:

1. You will get an acknowledgement, usually within 7 days. If you hear nothing, please send a reminder.
2. The maintainer will confirm the issue and discuss its impact and the fix with you.
3. After a fixed release is out, the issue is published as a GitHub Security Advisory. If you wish, you will be credited as the reporter.

Please do not disclose the issue publicly until a fixed release is available. If a fix looks like it will take a while, the maintainer will agree on a disclosure date with you.
