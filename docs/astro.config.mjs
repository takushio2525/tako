// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
	// Astro 6 では markdown.gfm の既定値が unified() 側へ移ったが、@astrojs/mdx v5 は
	// 未設定（undefined）を「無効」と解釈するため、.mdx ページだけ GFM テーブルが
	// 素通しになる。明示的に true を置いて .md / .mdx の描画を揃える。
	// このオプション自体は deprecated 警告が出る。Astro / Starlight 側が
	// unified() ベースの設定に揃ったら、そちらへ移すこと
	markdown: {
		gfm: true,
	},
	integrations: [
		starlight({
			title: 'tako',
			description: 'AI エージェントのための次世代ターミナル',
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
			},
			sidebar: [
				{
					label: 'はじめに',
					items: [
						{ label: 'tako とは', slug: 'index' },
						{ label: 'セットアップ', slug: 'getting-started' },
						{ label: 'クイックスタート', slug: 'getting-started/quickstart' },
						{ label: 'Windows 対応状況', slug: 'windows-support' },
						{ label: 'リリースノート', slug: 'releases' },
					],
				},
				{
					label: '機能紹介',
					items: [
						{ label: 'タブ＆ペイン管理', slug: 'features/tabs-and-panes' },
						{ label: '内蔵 MCP サーバー', slug: 'features/mcp-server' },
						{ label: 'ファイルプレビュー', slug: 'features/file-preview' },
						{ label: 'tmux バックエンド', slug: 'features/tmux-backend' },
						{ label: 'ポート検知', slug: 'features/port-detection' },
						{ label: 'たまり場', slug: 'features/shelving' },
						{ label: 'オーケストレーションとは', slug: 'features/orchestration' },
						{ label: 'tako master 実践ガイド', slug: 'features/orchestrator' },
						{ label: 'git 連携', slug: 'features/git-integration' },
					],
				},
				{
					label: '使い方ガイド',
					items: [
						{ label: 'CLI リファレンス', slug: 'guides/cli-reference' },
						{ label: 'MCP ツール一覧', slug: 'guides/mcp-tools' },
						{ label: 'キーボードショートカット', slug: 'guides/keyboard-shortcuts' },
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
