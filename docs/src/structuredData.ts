/**
 * 構造化データ（JSON-LD）を組み立てる。呼び出しは src/starlightRouteData.ts の 1 か所だけ（#1843）。
 *
 * 出すのは**実態どおりに書けるものだけ**:
 * - トップ: WebSite（検索結果のサイト名）と、それを載せている人・作品への参照
 * - トップ以外: BreadcrumbList（URL の階層のうち、ページとして実在する段だけ）
 * - 「## よくある質問」の下に「### 質問」を並べたページ: FAQPage（本文から組み立てるので、
 *   画面に出ている Q&A と食い違わない）
 *
 * 出さないもの: SoftwareApplication（Google は評価 `aggregateRating` / `review` が無いと
 * リッチリザルトの誤りにする。偽の評価は書かない）、SearchAction（サイト内検索ボックスの
 * 表示は Google が 2024 年に廃止した）。
 *
 * 人（Person）と作品（SoftwareSourceCode）の `@id` はハブ（takushio2525.com）の JSON-LD と
 * 同じ値にして、別サイトの同じ実体だと読めるようにしている。値を変えるならハブ側と揃える。
 * 公開ハンドル以外の個人情報（実名・メール等）はここに書かない。
 */
import type { StarlightRouteData } from '@astrojs/starlight/route-data';

const HUB_URL = 'https://takushio2525.com/';
const PERSON_ID = 'https://takushio2525.com/#person';
const WORK_ID = 'https://takushio2525.com/works/tako/#work';
const REPOSITORY_URL = 'https://github.com/takushio2525/tako';

type HeadEntry = StarlightRouteData['head'][number];
type SidebarEntry = StarlightRouteData['sidebar'][number];
type Json = Record<string, unknown>;

/** `<script type="application/ld+json">` 1 本にする。`</script>` で本文が切れないよう `<` を逃がす */
export function jsonLdTag(graph: Json[]): HeadEntry {
	const data = { '@context': 'https://schema.org', '@graph': graph };
	return {
		tag: 'script',
		attrs: { type: 'application/ld+json' },
		content: JSON.stringify(data).replace(/</g, '\\u003c'),
	};
}

/** トップページ用: サイト名・運営者・扱っているソフトウェア */
export function siteGraph(site: URL, description: string): Json[] {
	const home = new URL('/', site).href;
	return [
		{
			'@type': 'WebSite',
			'@id': `${home}#website`,
			url: home,
			name: 'tako',
			alternateName: ['tako ドキュメント', 'tako ターミナル'],
			description,
			inLanguage: 'ja',
			publisher: { '@id': PERSON_ID },
			about: { '@id': WORK_ID },
		},
		{
			'@type': 'Person',
			'@id': PERSON_ID,
			name: 'takushio2525',
			url: HUB_URL,
			sameAs: ['https://github.com/takushio2525'],
		},
		{
			'@type': 'SoftwareSourceCode',
			'@id': WORK_ID,
			name: 'tako',
			url: home,
			codeRepository: REPOSITORY_URL,
			programmingLanguage: ['Rust'],
			license: 'https://www.gnu.org/licenses/gpl-3.0.html',
			author: { '@id': PERSON_ID },
		},
	];
}

/** サイドバーのリンクを「正規化したパス → 表示名」に畳む（グループは中身だけ見る） */
function sidebarLabels(entries: SidebarEntry[], acc = new Map<string, string>()) {
	for (const entry of entries) {
		if (entry.type === 'group') sidebarLabels(entry.entries, acc);
		else if (entry.href.startsWith('/')) acc.set(normalizePath(entry.href), entry.label);
	}
	return acc;
}

/** `/foo/bar` も `/foo/bar/index.html` も `/foo/bar/` に揃える（og-manifest のキーと同じ形） */
export function normalizePath(pathname: string): string {
	let path = pathname.replace(/index\.html$/, '');
	if (!path.startsWith('/')) path = `/${path}`;
	if (!path.endsWith('/')) path = `${path}/`;
	return path;
}

/**
 * パンくず: tako（トップ）→ 実在する親ページ → このページ。
 * 例: `/agents/claude/` は「tako → エージェントの選び方 → Claude Code」。
 * `/features/` のようにページが無い段は飛ばす（Google は途中の段にも URL を求めるため、
 * サイドバーのグループ名のような URL の無い段は入れられない）。
 */
export function breadcrumbGraph(
	site: URL,
	path: string,
	sidebar: SidebarEntry[],
	pageTitle: string
): Json[] {
	const labels = sidebarLabels(sidebar);
	const items: { name: string; path: string }[] = [{ name: 'tako', path: '/' }];
	const segments = path.split('/').filter(Boolean);
	for (let i = 1; i < segments.length; i++) {
		const parent = `/${segments.slice(0, i).join('/')}/`;
		const label = labels.get(parent);
		if (label) items.push({ name: label, path: parent });
	}
	items.push({ name: labels.get(path) ?? pageTitle, path });
	return [
		{
			'@type': 'BreadcrumbList',
			itemListElement: items.map((item, index) => ({
				'@type': 'ListItem',
				position: index + 1,
				name: item.name,
				item: new URL(item.path, site).href,
			})),
		},
	];
}

const FAQ_HEADING = '## よくある質問';

/** 回答の Markdown を、画面に出る文字だけの平文にする（リンク・強調・コード・<kbd> を剥がす） */
function markdownToText(markdown: string): string {
	return markdown
		.replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
		.replace(/\*\*([^*]*)\*\*/g, '$1')
		.replace(/`([^`]*)`/g, '$1')
		.replace(/<[^>]+>/g, '')
		.replace(/\s*\n\s*/g, '')
		.trim();
}

/**
 * FAQPage: 本文の「## よくある質問」節にある「### 質問」と、その下の段落（回答）を拾う。
 * 節が無い・回答が空のページは何も返さない。回答にコードブロックや表を入れると平文に
 * 畳めないので、その形の Q&A は「### 質問」ではなく本文の別の節に書く。
 * 検査は scripts/verify-seo.mjs（質問と回答が画面の本文に在ることまで見る）。
 */
export function faqGraph(body: string | undefined): Json[] {
	if (!body) return [];
	const start = body.indexOf(`\n${FAQ_HEADING}\n`);
	if (start < 0) return [];
	const rest = body.slice(start + FAQ_HEADING.length + 2);
	const section = rest.split(/\n## /)[0];
	const questions = section
		.split(/\n(?=### )/)
		.filter((block) => block.startsWith('### '))
		.map((block) => {
			const [heading, ...answer] = block.split('\n');
			return { name: heading.replace(/^### /, '').trim(), text: markdownToText(answer.join('\n')) };
		})
		.filter((qa) => qa.name && qa.text);
	if (questions.length === 0) return [];
	return [
		{
			'@type': 'FAQPage',
			mainEntity: questions.map((qa) => ({
				'@type': 'Question',
				name: qa.name,
				acceptedAnswer: { '@type': 'Answer', text: qa.text },
			})),
		},
	];
}
