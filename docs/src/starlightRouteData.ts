/**
 * ページごとの OG 画像を head に足す Starlight route middleware。
 *
 * Starlight は og:title / og:type / og:url / og:description / og:site_name と
 * twitter:card までは自前で出すが、og:image は出さない（画像が無いと X 等では
 * カードが小さいテキストだけになる）。ここで画像 3 種を足す。
 *
 * 画像の実体は `docs/scripts/generate-og.mjs` が焼いてコミットしてある PNG で、
 * どのパスにどれが対応するかは `src/data/og-manifest.json` が持つ。manifest に
 * 無いページ（画像を焼く前に追加されたページ）はトップの画像へ落とすので、
 * タグが壊れることはない。
 *
 * og:image は絶対 URL でなければクローラが読めない。基点は astro.config.mjs の
 * `site` 一本（ここには URL を書かない）。
 */
import { defineRouteMiddleware } from '@astrojs/starlight/route-data';
import manifest from './data/og-manifest.json';

const FALLBACK_IMAGE = '/og/index.png';
const IMAGE_ALT = 'tako — AI エージェントのための次世代ターミナル';

const images: Record<string, string> = manifest.pages;

/** `/foo/bar` も `/foo/bar/index.html` も `/foo/bar/` に揃える（manifest のキーと同じ形） */
function normalizePath(pathname: string): string {
	let path = pathname.replace(/index\.html$/, '');
	if (!path.startsWith('/')) path = `/${path}`;
	if (!path.endsWith('/')) path = `${path}/`;
	return path;
}

export const onRequest = defineRouteMiddleware((context) => {
	const { head } = context.locals.starlightRoute;
	const path = normalizePath(context.url.pathname);

	// site 未設定だと絶対 URL を作れない = 画像タグを出さない方が無害
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
});
