/**
 * スクロール連動のモーション解説（#2053）。依存ライブラリは使わない。
 *
 * 仕組みは 1 つだけ: 「いまの手順番号 N」をステージ（再現した画面）へ写す。
 *   - ステージに `data-step="N"` と CSS 変数 `--step` を置く（場面ごとの CSS が使える）
 *   - ステージの中の `[data-at]` は、指定が N に当たれば `.is-on` が付く（見え方は motion.css）
 *   - `[data-pos]`（ポインター）は N に応じた座標へ動く
 *
 * N の決め方は 2 通り:
 *   - `[data-scrolly]`: 手順（`.tk-step`）のうち「読む位置」を越えた最後の 1 つ（スクロール連動）。
 *     まだどれも越えていなければ手順 1。`[data-scrolly-intro]` の場面だけは 0（手順 1 の前の姿）にして、
 *     読者が場面に着いてから手順 1 を当てる（手順 1 の打鍵や「押す前 → 押した後」を見せたい場面）
 *   - `[data-autoplay]`: 画面に入ったら時間で 1 → 最後まで 1 回だけ進める（ヒーロー用）
 *
 * `prefers-reduced-motion: reduce` のときは動かさない。スクロール連動の場面は、手順ごとに
 * ステージの写しをその手順の中へ置いて**静止した図の並び**にし、自動再生は最後の状態で止める。
 * JS が動かない環境では `data-motion` が付かないので、`[data-at]` は全部見えたままになる。
 *
 * 書き方の手引きは `.agent/plans/2026-10-docs-motion.md`。
 */

type Match = (n: number) => boolean;

const reduce = window.matchMedia('(prefers-reduced-motion: reduce)');
const matchCache = new WeakMap<Element, Match>();

/**
 * `data-at` の書き方: 空白区切りで `3`（3 のときだけ）/ `3+`（3 から後ずっと）/ `2-4`（2〜4）。
 * 先頭の `!` は全体の否定（`!3+` = 3 より前だけ）。手順番号は 1 始まり（読者に見える番号と同じ）。
 */
export function parseAt(spec: string): Match {
	const trimmed = spec.trim();
	if (trimmed.startsWith('!')) {
		const inner = parseAt(trimmed.slice(1));
		return (n) => !inner(n);
	}
	const parts: Match[] = [];
	for (const tok of trimmed.split(/[\s,]+/).filter(Boolean)) {
		let m: RegExpMatchArray | null;
		if ((m = tok.match(/^(\d+)\+$/))) {
			const a = Number(m[1]);
			parts.push((n) => n >= a);
		} else if ((m = tok.match(/^(\d+)-(\d+)$/))) {
			const a = Number(m[1]);
			const b = Number(m[2]);
			parts.push((n) => n >= a && n <= b);
		} else if ((m = tok.match(/^(\d+)$/))) {
			const a = Number(m[1]);
			parts.push((n) => n === a);
		} else {
			// 書き間違いは「見えない」へ倒し、書いた人がコンソールで気づけるようにする
			console.warn(`[tako motion] data-at="${spec}" の "${tok}" を読めません（例: 2 / 2+ / 2-4）`);
		}
	}
	return (n) => parts.some((p) => p(n));
}

/**
 * `data-pos="1:30,40 3:70,80 5:-"` → 手順 N で使う座標（N 以下で最も新しいもの）。
 * `5:-` は「手順 5 から隠す」（その前の位置のまま消える）。指定より前の手順と隠す手順は null
 */
function posAt(spec: string, n: number): [string, string] | null {
	let best: [number, string, string] | [number, null, null] | null = null;
	for (const tok of spec.trim().split(/\s+/)) {
		const m = tok.match(/^(\d+):(?:(-?[\d.]+),(-?[\d.]+)|(-))$/);
		if (!m) continue;
		const k = Number(m[1]);
		if (k <= n && (!best || k > best[0])) best = m[4] ? [k, null, null] : [k, m[2], m[3]];
	}
	return best && best[1] !== null ? [`${best[1]}%`, `${best[2]}%`] : null;
}

/** ステージ（またはその写し）を手順 n の状態にする */
export function applyStep(root: HTMLElement, n: number): void {
	root.dataset.step = String(n);
	root.style.setProperty('--step', String(n));
	for (const el of root.querySelectorAll<HTMLElement>('[data-at]')) {
		let m = matchCache.get(el);
		if (!m) {
			m = parseAt(el.dataset.at ?? '');
			matchCache.set(el, m);
		}
		el.classList.toggle('is-on', m(n));
	}
	for (const el of root.querySelectorAll<HTMLElement>('[data-pos]')) {
		const p = posAt(el.dataset.pos ?? '', n);
		el.classList.toggle('is-on', p !== null);
		if (p) {
			el.style.left = p[0];
			el.style.top = p[1];
		}
		const clicks = el.dataset.click ? parseAt(el.dataset.click) : null;
		el.classList.toggle('is-click', !!clicks && clicks(n));
	}
}

/** 打鍵の演出（`data-fx~="type"`）は文字数ぶんの段で開くので、文字数を CSS へ渡す */
function measureTyping(root: HTMLElement): void {
	for (const el of root.querySelectorAll<HTMLElement>('[data-fx~="type"]')) {
		el.style.setProperty('--n', String(Math.max(1, [...(el.textContent ?? '')].length)));
	}
}

// ---------------------------------------------------------------------------
// スクロール連動
// ---------------------------------------------------------------------------

class Scrolly {
	readonly root: HTMLElement;
	readonly stage: HTMLElement;
	readonly wrap: HTMLElement;
	readonly steps: HTMLElement[];
	/** 手順 1 の前の姿（手順 0）を持つ場面か。持たない場面は、どの手順も読む位置を越える前から手順 1 */
	private readonly intro: boolean;
	private active = -1;
	private figures: HTMLElement[] = [];

	constructor(root: HTMLElement) {
		this.root = root;
		this.stage = root.querySelector<HTMLElement>('[data-stage]')!;
		this.wrap = root.querySelector<HTMLElement>('.tk-scrolly-stage')!;
		this.steps = [...root.querySelectorAll<HTMLElement>(':scope > .tk-scrolly-steps > .tk-step')];
		this.intro = root.hasAttribute('data-scrolly-intro');
		measureTyping(this.stage);
		root.dataset.motion = '';
		this.setMode();
	}

	setMode(): void {
		if (reduce.matches) this.toStatic();
		else this.toMotion();
	}

	/** 動かない図の並びにする: 手順ごとにステージを写して、その手順の状態で止める */
	private toStatic(): void {
		this.root.classList.add('is-static');
		if (this.figures.length) return;
		this.steps.forEach((step, i) => {
			const fig = this.stage.cloneNode(true) as HTMLElement;
			fig.removeAttribute('data-stage');
			fig.classList.add('tk-step-figure');
			applyStep(fig, i + 1);
			step.append(fig);
			this.figures.push(fig);
		});
	}

	private toMotion(): void {
		this.root.classList.remove('is-static');
		for (const fig of this.figures) fig.remove();
		this.figures = [];
		this.active = -1;
		this.update();
	}

	/** 「読む位置」を越えた最後の手順を選ぶ */
	update(): void {
		if (this.root.classList.contains('is-static')) return;
		const vh = window.innerHeight;
		// 縦積み（スマホ幅など）では画面上部にステージが貼り付くので、その下の読む領域で決める
		const stacked = getComputedStyle(this.root).getPropertyValue('--tk-stacked').trim() === '1';
		let probe = vh * 0.5;
		if (stacked) {
			const bottom = this.wrap.getBoundingClientRect().bottom;
			probe = Math.min(vh * 0.85, bottom + (vh - bottom) * 0.4);
		}
		let n = this.intro ? 0 : 1;
		this.steps.forEach((step, i) => {
			if (step.getBoundingClientRect().top <= probe) n = i + 1;
		});
		this.set(n);
	}

	private set(n: number): void {
		if (n === this.active) return;
		this.active = n;
		applyStep(this.stage, n);
		this.steps.forEach((step, i) => {
			const on = i + 1 === n;
			step.classList.toggle('is-active', on);
			if (on) step.setAttribute('aria-current', 'step');
			else step.removeAttribute('aria-current');
		});
	}
}

// ---------------------------------------------------------------------------
// 自動再生（ヒーロー）
// ---------------------------------------------------------------------------

class Autoplay {
	readonly root: HTMLElement;
	private readonly last: number;
	private readonly interval: number;
	private timer = 0;
	private played = false;

	constructor(root: HTMLElement) {
		this.root = root;
		this.last = Math.max(1, Number(root.dataset.autoplay) || 1);
		this.interval = Math.max(400, Number(root.dataset.interval) || 1800);
		measureTyping(root);
		root.dataset.motion = '';
		applyStep(root, reduce.matches ? this.last : 1);
	}

	play(): void {
		if (this.played) return;
		this.played = true;
		if (reduce.matches) {
			applyStep(this.root, this.last);
			return;
		}
		let n = 1;
		const tick = () => {
			n += 1;
			applyStep(this.root, n);
			if (n < this.last) this.timer = window.setTimeout(tick, this.interval);
		};
		this.timer = window.setTimeout(tick, this.interval);
	}

	stopAtEnd(): void {
		window.clearTimeout(this.timer);
		applyStep(this.root, this.last);
	}
}

// ---------------------------------------------------------------------------
// 起動
// ---------------------------------------------------------------------------

function init(): void {
	const scrollies = [...document.querySelectorAll<HTMLElement>('[data-scrolly]')].map((el) => new Scrolly(el));
	const autos = [...document.querySelectorAll<HTMLElement>('[data-autoplay]')].map((el) => new Autoplay(el));

	// 画面の近くにある場面だけを毎フレーム見る（遠い場面の矩形は読まない）
	const near = new Set<Scrolly>();
	let queued = false;
	const tick = () => {
		queued = false;
		for (const s of near) s.update();
	};
	const schedule = () => {
		if (!queued) {
			queued = true;
			window.requestAnimationFrame(tick);
		}
	};

	if ('IntersectionObserver' in window) {
		const io = new IntersectionObserver(
			(entries) => {
				for (const e of entries) {
					const s = scrollies.find((x) => x.root === e.target);
					if (!s) continue;
					if (e.isIntersecting) near.add(s);
					else near.delete(s);
				}
				schedule();
			},
			{ rootMargin: '50% 0px 50% 0px' }
		);
		for (const s of scrollies) io.observe(s.root);

		const autoIo = new IntersectionObserver(
			(entries) => {
				for (const e of entries) {
					if (!e.isIntersecting) continue;
					autos.find((a) => a.root === e.target)?.play();
					autoIo.unobserve(e.target);
				}
			},
			{ threshold: 0.35 }
		);
		for (const a of autos) autoIo.observe(a.root);
	} else {
		for (const s of scrollies) near.add(s);
		for (const a of autos) a.stopAtEnd();
	}

	window.addEventListener('scroll', schedule, { passive: true });
	window.addEventListener('resize', schedule, { passive: true });
	reduce.addEventListener('change', () => {
		for (const s of scrollies) s.setMode();
		if (reduce.matches) for (const a of autos) a.stopAtEnd();
		schedule();
	});
	schedule();
}

if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init, { once: true });
else init();
