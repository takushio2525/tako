// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// Google アナリティクス 4 の測定 ID。takushio2525.com とその全サブドメインで 1 つの
// プロパティを共有している（gtag は既定で cookie_domain: 'auto' なので `_ga` は書ける
// 一番上のドメイン = takushio2525.com に置かれ、サブドメインをまたいでも同じ訪問者として
// 数えられる）。サイト別に見たいときはレポートの「ホスト名」ディメンションで分ける。
const GA_MEASUREMENT_ID = 'G-30GVXMB0GH';

// AdSense のサイト所有権確認に使うパブリッシャー ID。ここでは所有権を示すだけで広告は
// 出さないので adsbygoogle.js は読み込まない。ads.txt はルートドメイン takushio2525.com に
// 1 つ置けば全サブドメインへ効くため、このサイトには置かない。
const ADSENSE_PUBLISHER_ID = 'ca-pub-3136871606832456';

export default defineConfig({
	// 公開 URL。canonical / og:url / sitemap / OG 画像の絶対 URL の基点になる
	// （crates/tako-app/src/about_window.rs の DOCUMENTATION_URL と同じ URL）
	site: 'https://tako.takushio2525.com',
	integrations: [
		starlight({
			title: 'tako',
			description: 'AI エージェントのための次世代ターミナル',
			head: [
				// Pages Functions のミドルウェア（docs/functions/_middleware.js）が届かない配備でも
				// 旧ドメインの閲覧者を新ドメインへ送るための保険。転送の本体は 301 を返す
				// ミドルウェア側で、こちらは静的 HTML だけが配られたときにだけ効く。
				// **アクセス解析より前に置く**（旧ドメインでのページビューを数えないため）。
				{
					tag: 'script',
					content:
						"if (location.hostname === 'tako-docs.pages.dev') {" +
						"location.replace('https://tako.takushio2525.com' + location.pathname + location.search + location.hash);" +
						"}",
				},
				// Cookie 同意（Consent Mode v2）。takushio2525.com 一族で共用のバナー本体を読む。
				// **Google タグより前に `async` を付けずに**置くのが条件（`gtag('consent',
				// 'default', …)` が `gtag('config', …)` より前に dataLayer へ入っていないと
				// 既定値が効かない）。旧ドメインからの転送より後ろなのは、転送で捨てる表示の
				// ために同期の取得を待たせないため。バナー本体・国判定・ポリシーの正本は
				// ハブ（takushio2525.com）側にあり、ここは読み込むだけ。
				{
					tag: 'script',
					attrs: { src: 'https://takushio2525.com/consent/consent.js' },
				},
				// Google アナリティクス 4（gtag.js）。Google タグは 1 ページに 1 つだけ置く。
				{
					tag: 'script',
					attrs: {
						async: true,
						src: `https://www.googletagmanager.com/gtag/js?id=${GA_MEASUREMENT_ID}`,
					},
				},
				{
					tag: 'script',
					content:
						'window.dataLayer = window.dataLayer || [];' +
						'function gtag(){dataLayer.push(arguments);}' +
						// consent.js を読めなかった（通信失敗・ブロック）ときの保険。既定値が
						// 1 つも宣言されていないと gtag は全部同意済みとして動くので、止める側へ
						// 倒す。**省略しない。**
						'if (!window.tkConsent) ' +
						"gtag('consent', 'default', {ad_storage: 'denied', ad_user_data: 'denied', " +
						"ad_personalization: 'denied', analytics_storage: 'denied'});" +
						"gtag('js', new Date());" +
						`gtag('config', '${GA_MEASUREMENT_ID}');`,
				},
				// AdSense のサイト所有権確認（メタタグ方式）。広告ユニットは出さない。
				{
					tag: 'meta',
					attrs: { name: 'google-adsense-account', content: ADSENSE_PUBLISHER_ID },
				},
			],
			// ページごとの OG 画像を head に足す（画像は docs/scripts/generate-og.mjs が生成）
			routeMiddleware: './src/starlightRouteData.ts',
			defaultLocale: 'root',
			locales: {
				root: { label: '日本語', lang: 'ja' },
			},
			customCss: ['./src/styles/tako-theme.css'],
			logo: {
				src: './src/assets/tako-icon.svg',
			},
			social: [
				{ icon: 'github', label: 'GitHub', href: 'https://github.com/takushio2525/tako' },
			],
			components: {
				Sidebar: './src/components/SidebarHelp.astro',
				Footer: './src/components/FooterLegal.astro',
			},
			sidebar: [
				{
					label: 'はじめに',
					items: [
						{ label: 'tako とは', slug: 'index' },
						{ label: 'セットアップ', slug: 'getting-started' },
						{ label: 'クイックスタート', slug: 'getting-started/quickstart' },
						{ label: 'リリースノート', slug: 'releases' },
						{ label: 'Windows 対応状況', slug: 'windows-support' },
					],
				},
				{
					label: 'エージェント CLI',
					items: [
						{ label: 'エージェントの選び方', slug: 'agents' },
						{ label: 'Claude Code', slug: 'agents/claude' },
						{ label: 'OpenAI Codex CLI', slug: 'agents/codex' },
						{ label: 'Antigravity CLI', slug: 'agents/agy' },
						{ label: 'ローカル LLM', slug: 'agents/local-llm' },
						{ label: 'エージェント別の対応状況', slug: 'agent-support' },
					],
				},
				{
					label: 'AI と使う',
					items: [
						{ label: 'オーケストレーションとは', slug: 'features/orchestration' },
						{ label: 'tako master 実践ガイド', slug: 'features/orchestrator' },
						{ label: '内蔵 MCP サーバー', slug: 'features/mcp-server' },
						{ label: '人がやること（ユーザータスク）', slug: 'features/user-tasks' },
					],
				},
				{
					label: '機能紹介',
					items: [
						{ label: 'かんたん表示（GUI モード）', slug: 'features/gui-mode' },
						{ label: 'タブ＆ペイン管理', slug: 'features/tabs-and-panes' },
						{ label: 'ファイルツリー＆プレビュー', slug: 'features/file-preview' },
						{ label: 'git 連携', slug: 'features/git-integration' },
						{ label: 'リモートアクセス', slug: 'features/remote' },
						{ label: 'tmux バックエンド', slug: 'features/tmux-backend' },
						{ label: 'たまり場', slug: 'features/shelving' },
						{ label: 'ポート検知', slug: 'features/port-detection' },
						{ label: 'エラーテレメトリ', slug: 'features/telemetry' },
					],
				},
				{
					label: '使い方ガイド',
					items: [
						{ label: 'CLI リファレンス', slug: 'guides/cli-reference' },
						{ label: 'MCP ツール一覧', slug: 'guides/mcp-tools' },
						{ label: '設定とカスタマイズ', slug: 'guides/settings' },
						{ label: 'キーボードショートカット', slug: 'guides/keyboard-shortcuts' },
						{ label: 'リモート接続の移行ガイド', slug: 'guides/remote-migration' },
					],
				},
				{
					label: '開発者向け',
					items: [
						{ label: 'ビルド方法', slug: 'development/building' },
						{ label: 'アーキテクチャ', slug: 'development/architecture' },
					],
				},
			],
		}),
	],
});
