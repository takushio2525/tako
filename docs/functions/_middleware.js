// 旧ドメイン（tako-docs.pages.dev）宛の要求を、同じパスのまま新ドメインへ 301 で転送する。
//
// なぜここに書くか: `*.pages.dev` は利用者のゾーンに属さないホスト名なので、
// ゾーン側の Redirect Rules / Bulk Redirects では転送を書けない。Pages Functions は
// pages.dev 宛のトラフィックにも噛むため、旧ドメインを畳めるのはこの経路だけ。
// `_redirects` はホスト名を条件に書けないので使えない。
//
// 完全一致で見るのは、プレビュー配備（<hash>.tako-docs.pages.dev）を転送しないため。
// 後方一致にすると、本番へ出す前の確認ができなくなる。

const LEGACY_HOST = 'tako-docs.pages.dev';
const CANONICAL_HOST = 'tako.takushio2525.com';

export const onRequest = (context) => {
	const url = new URL(context.request.url);
	if (url.hostname !== LEGACY_HOST) {
		return context.next();
	}
	url.hostname = CANONICAL_HOST;
	return Response.redirect(url.toString(), 301);
};
