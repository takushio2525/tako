/**
 * Windows 対応状況を、生成ページ `src/content/docs/windows-support.md` から**ビルド時に**読む（#2053）。
 *
 * そのページは tako 本体の対応マトリクス（crates/tako-core/src/platform/support.rs）から
 * `scripts/gen-windows-support-docs.mjs` が作り、CI が同期を検査している。数字をここで手書きしないので、
 * マトリクスが変われば トップ / インストールページの「Windows でどこまで使えるか」も一緒に変わる。
 * 生成ページの書式が変わって読めなくなったら、黙って 0 を出さずにビルドを落とす。
 */
import raw from '../../content/docs/windows-support.md?raw';

export type Status = '対応' | '一部対応' | '未対応 / 未実測' | '対象外';

export interface Area {
	/** 節の見出し（ターミナルの基本 など） */
	name: string;
	counts: Record<Status, number>;
	total: number;
}

export interface WindowsSupport {
	supported: number;
	degraded: number;
	unmeasured: number;
	unsupported: number;
	notApplicable: number;
	total: number;
	percent: number;
	areas: Area[];
	/** 機能（MCP ツール名）→ 状態 */
	rows: Map<string, Status>;
}

const STATUSES: Status[] = ['対応', '一部対応', '未対応 / 未実測', '対象外'];

function fail(what: string): never {
	throw new Error(
		`windowsSupport.ts: windows-support.md から ${what} を読めません。` +
			'生成ページの書式（scripts/gen-windows-support-docs.mjs）が変わったら、この読み取りも直してください'
	);
}

function overall(label: string): RegExpMatchArray {
	const m = raw.match(new RegExp(`^\\| ${label} \\| (\\d+)(?: / (\\d+)（(\\d+)%）)? \\|`, 'm'));
	return m ?? fail(`「全体」の表の「${label}」の行`);
}

function parse(): WindowsSupport {
	const sup = overall('対応');
	const supported = Number(sup[1]);
	const total = Number(sup[2] ?? fail('対応の分母'));
	const percent = Number(sup[3]);

	const areas: Area[] = [];
	const sectionRe = /^## (.+)\n\n((?:対応|一部対応|未対応 \/ 未実測|対象外) \d+(?:・(?:対応|一部対応|未対応 \/ 未実測|対象外) \d+)*)\n/gm;
	for (const m of raw.matchAll(sectionRe)) {
		const counts = Object.fromEntries(STATUSES.map((s) => [s, 0])) as Record<Status, number>;
		for (const part of m[2].split('・')) {
			const pm = part.match(/^(.+) (\d+)$/);
			if (!pm || !STATUSES.includes(pm[1] as Status)) fail(`節「${m[1]}」の件数（${part}）`);
			counts[pm[1] as Status] = Number(pm[2]);
		}
		areas.push({ name: m[1], counts, total: STATUSES.reduce((a, s) => a + counts[s], 0) });
	}
	if (areas.length < 5) fail(`節ごとの件数（${areas.length} 節しか読めない）`);

	const rows = new Map<string, Status>();
	for (const m of raw.matchAll(/^\| `(tako_[a-z0-9_]+)` \| (対応|一部対応|未対応 \/ 未実測|対象外) \|/gm)) {
		rows.set(m[1], m[2] as Status);
	}
	if (rows.size !== total) fail(`機能の行（${rows.size} 行。全体の表は ${total}）`);
	const sum = areas.reduce((a, x) => a + x.total, 0);
	if (sum !== total) fail(`節の合計（${sum}。全体の表は ${total}）`);

	return {
		supported,
		degraded: Number(overall('一部対応')[1]),
		unmeasured: Number(overall('未実測')[1]),
		unsupported: Number(overall('未対応')[1]),
		notApplicable: Number(overall('対象外')[1]),
		total,
		percent,
		areas,
		rows,
	};
}

export const windowsSupport: WindowsSupport = parse();

/**
 * 「Windows でまだのもの」に挙げた機能が、本当にまだ「対応」になっていないかを確かめる。
 * マトリクスで対応へ上がったのにページが「まだ使えない」と言い続けるのを、ビルドで止める。
 */
export function assertNotSupported(tools: string[], where: string): void {
	for (const tool of tools) {
		const status = windowsSupport.rows.get(tool);
		if (!status) throw new Error(`${where}: ${tool} は Windows 対応表に無い機能名です`);
		if (status === '対応') {
			throw new Error(
				`${where}: ${tool} は Windows で「対応」になりました。「まだのもの」の一覧から外してください`
			);
		}
	}
}
