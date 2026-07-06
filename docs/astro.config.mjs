// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
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
						{ label: 'オーケストレーター', slug: 'features/orchestrator' },
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
