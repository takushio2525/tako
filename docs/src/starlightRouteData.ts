/**
 * ページごとの head を仕上げる Starlight route middleware。
 *
 * 1. og:image（#1547 まで）: Starlight は og:title / og:type / og:url / og:description /
 *    og:site_name と twitter:card までは自前で出すが、og:image は出さない（画像が無いと
 *    X 等ではカードが小さいテキストだけになる）。ここで画像 3 種を足す。
 * 2. 検索向けの `<title>`（#1843）: frontmatter の `seoTitle` があれば `<title>` と og:title を
 *    それに差し替える（見出し・サイドバー・OG 画像は `title` のまま）。
 * 3. 構造化データ（#1843）: トップに WebSite、それ以外に BreadcrumbList（「よくある質問」の
 *    節があれば FAQPage も）。組み立ては src/structuredData.ts。
 *
 * 画像の実体は `docs/scripts/generate-og.mjs` が焼いてコミットしてある PNG で、
 * どのパスにどれが対応するかは `src/data/og-manifest.json` が持つ。manifest に
 * 無いページ（画像を焼く前に追加されたページ）はトップの画像へ落とすので、
 * タグが壊れることはない。
 *
 * og:image などの絶対 URL の基点は astro.config.mjs の `site` 一本（ここには URL を書かない）。
 * 検査は scripts/verify-og.mjs（画像）と scripts/verify-seo.mjs（title・構造化データ・sitemap）。
 */
import { defineRouteMiddleware } from '@astrojs/starlight/route-data';
import manifest from './data/og-manifest.json';
import { breadcrumbGraph, faqGraph, jsonLdTag, normalizePath, siteGraph } from './structuredData';

const FALLBACK_IMAGE = '/og/index.png';
const IMAGE_ALT = 'tako — AI エージェントのための次世代ターミナル';

const images: Record<string, string> = manifest.pages;

export const onRequest = defineRouteMiddleware((context) => {
	const { head, entry, sidebar, siteTitle } = context.locals.starlightRoute;
	const path = normalizePath(context.url.pathname);
	const isNotFound = path.startsWith('/404');

	const seoTitle = entry.data.seoTitle;
	if (seoTitle) {
		const title = head.find((tag) => tag.tag === 'title');
		if (title) title.content = `${seoTitle} | ${siteTitle}`;
		const ogTitle = head.find((tag) => tag.tag === 'meta' && tag.attrs?.['property'] === 'og:title');
		if (ogTitle?.attrs) ogTitle.attrs['content'] = seoTitle;
	}

	// site 未設定だと絶対 URL を作れない = 画像タグ・構造化データを出さない方が無害
	if (!context.site) return;
	const imageUrl = new URL(images[path] ?? FALLBACK_IMAGE, context.site).href;

	// トップは記事ではなくサイトそのもの
	if (path === '/') {
		const ogType = head.find((tag) => tag.tag === 'meta' && tag.attrs?.['property'] === 'og:type');
		if (ogType?.attrs) ogType.attrs['content'] = 'website';
	}

	head.push(
		{ tag: 'meta', attrs: { property: 'og:image', content: imageUrl } },
		{ tag: 'meta', attrs: { property: 'og:image:width', content: String(manifest.width) } },
		{ tag: 'meta', attrs: { property: 'og:image:height', content: String(manifest.height) } },
		{ tag: 'meta', attrs: { property: 'og:image:alt', content: IMAGE_ALT } },
		{ tag: 'meta', attrs: { name: 'twitter:image', content: imageUrl } },
		{ tag: 'meta', attrs: { name: 'twitter:image:alt', content: IMAGE_ALT } }
	);

	// 404 は索引されないページなので構造化データを付けない
	if (isNotFound) return;
	head.push(
		jsonLdTag(
			path === '/'
				? siteGraph(context.site, entry.data.description ?? '')
				: [
						...breadcrumbGraph(context.site, path, sidebar, entry.data.title),
						...faqGraph(entry.body),
					]
		)
	);
});
