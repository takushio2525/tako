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
						{ label: 'クイックスタート', slug: 'getting-started/quickstart' },
						{ label: 'リリースノート', slug: 'releases' },
					],
				},
				{
					label: 'AI と使う',
					items: [
						{ label: 'オーケストレーションとは', slug: 'features/orchestration' },
						{ label: 'tako master 実践ガイド', slug: 'features/orchestrator' },
						{ label: '内蔵 MCP サーバー', slug: 'features/mcp-server' },
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
