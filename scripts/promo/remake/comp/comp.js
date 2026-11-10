// tako プロモ動画（リメイク版）の合成エンジン。
//
// `window.__seek(秒)` を呼ぶと、その時刻の 1 枚を決定的に描く（同じ時刻なら必ず同じ絵）。
// 壁時計で進む CSS の transition / animation は使わない。render.mjs が 1/30 秒ずつ進めて撮る。
//
// 入力（render.mjs がページへ流し込む）:
//   window.TL      … timeline.json（場面の時刻・字幕・実画面の切り出し・カメラ）
//   window.FOOTAGE … 収録した場面ごとの { dir, frames: [[番号, ms]…], w, h }
//   window.ASSETS  … { icon: アプリのアイコンの URL }
(() => {
	const TL = window.TL;
	const FOOTAGE = window.FOOTAGE || {};
	const ASSETS = window.ASSETS || {};
	const VW = 1504;
	const VH = 846;

	const $ = (sel, root = document) => root.querySelector(sel);
	const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
	const clamp = (v, a = 0, b = 1) => Math.min(b, Math.max(a, v));
	const lerp = (a, b, p) => a + (b - a) * p;
	const easeOut = (p) => 1 - (1 - p) ** 3;
	const easeInOut = (p) => (p < 0.5 ? 4 * p * p * p : 1 - (-2 * p + 2) ** 3 / 2);
	const prog = (t, start, dur) => clamp((t - start) / dur);
	const pending = [];

	function setSrc(img, url) {
		if (img.dataset.url === url) return;
		img.dataset.url = url;
		img.src = url;
		pending.push(img.decode().catch(() => {}));
	}

	// ---- 出し入れ（data-a="in:開始,長さ,種類; out:開始,長さ,種類"。時刻は場面の頭から） ----

	function parseTween(el) {
		const spec = {};
		for (const part of el.dataset.a.split(';')) {
			const [key, value] = part.trim().split(':');
			if (!key || !value) continue;
			const [s, d, kind] = value.split(',');
			spec[key.trim()] = { s: Number(s), d: Number(d), kind: (kind || 'fade').trim() };
		}
		return spec;
	}

	function applyTween(el, spec, lt) {
		let opacity = 1;
		let y = 0;
		let scale = 1;
		if (spec.in) {
			const p = easeOut(prog(lt, spec.in.s, spec.in.d));
			opacity = p;
			if (spec.in.kind === 'up') y = (1 - p) * 38;
			if (spec.in.kind === 'pop') scale = 0.84 + 0.16 * p;
		}
		if (spec.out) {
			const q = easeInOut(prog(lt, spec.out.s, spec.out.d));
			opacity *= 1 - q;
			y -= q * 20;
		}
		el.style.opacity = opacity.toFixed(4);
		el.style.setProperty('--ay', `${y.toFixed(2)}px`);
		el.style.setProperty('--as', scale.toFixed(4));
	}

	// ---- 0–20 秒: 散らばる窓 → 1 つのタブ ----

	const TILES = [
		{ label: 'エージェント', state: '実行中', rows: ['hot', '', '', 'ok'], pile: [300, 560, -6], scatter: [70, 440, -9], slot: [0, 0, 0.42, 1] },
		{ label: '子エージェント', state: '実行中', rows: ['', 'hot', ''], pile: [565, 612, 4], scatter: [455, 96, 6], slot: [0.42, 0, 0.29, 0.5] },
		{ label: '子エージェント', state: '入力待ち', rows: ['', '', 'hot'], pile: [825, 556, -3], scatter: [830, 500, -4], slot: [0.71, 0, 0.29, 0.5] },
		{ label: '子エージェント', state: '完了', rows: ['', 'ok', ''], pile: [1085, 620, 5], scatter: [1500, 120, 8], slot: [0.42, 0.5, 0.29, 0.5] },
		{ label: 'dev サーバー', state: '', rows: ['ok', '', ''], pile: [1340, 566, -4], scatter: [1490, 690, -7], slot: [0.71, 0.5, 0.29, 0.25] },
		{ label: 'テスト', state: '', rows: ['', 'ok'], pile: [700, 764, 3], scatter: [96, 742, 5], slot: [0.71, 0.75, 0.145, 0.25] },
		{ label: 'ログ', state: '', rows: ['', ''], pile: [1160, 782, -5], scatter: [1040, 84, -6], slot: [0.855, 0.75, 0.145, 0.25] },
	];
	const TILE_W = 330;
	const TILE_H = 212;
	const ONETAB_A = [1010, 236, 800, 580]; // 題字の右に置く「1 つのタブ」
	const ONETAB_B = [208, 212, VW, VH]; // 次の場面の実画面と同じ矩形
	let tileEls = [];

	function buildAct1() {
		const host = $('#tiles');
		tileEls = TILES.map((t) => {
			const el = document.createElement('div');
			el.className = 'tile';
			el.innerHTML = `<div class="bar"><i></i><i></i><i></i><span>${t.label}</span></div><div class="rows">${t.rows
				.map((r, i) => `<u class="${r}" style="width:${[78, 52, 66, 40][i % 4]}%"></u>`)
				.join('')}</div><div class="state">${t.state}</div>`;
			host.appendChild(el);
			return el;
		});
	}

	function drawAct1(lt) {
		// 1 つのタブ（窓の枠）
		const morph = easeInOut(prog(lt, 18.8, 1.1));
		const rect = ONETAB_A.map((v, i) => lerp(v, ONETAB_B[i], morph));
		const one = $('#onetab');
		one.style.left = `${rect[0]}px`;
		one.style.top = `${rect[1]}px`;
		one.style.width = `${rect[2]}px`;
		one.style.height = `${rect[3]}px`;
		one.style.opacity = easeOut(prog(lt, 12.5, 0.7)).toFixed(4);
		$('.onetab-bar', one).style.opacity = (1 - prog(lt, 19.3, 0.5)).toFixed(4);
		const barH = 46;
		const pad = 8;

		tileEls.forEach((el, i) => {
			const t = TILES[i];
			const born = 2.2 + i * 0.33;
			const pop = easeOut(prog(lt, born, 0.45));
			const toScatter = easeInOut(prog(lt, 5.0, 1.7));
			const toSlot = easeInOut(prog(lt, 12.5 + i * 0.07, 1.5));
			// ゆっくり漂う（散らばっている間だけ）
			const drift = toScatter * (1 - toSlot);
			const wobX = Math.sin(lt * 0.7 + i * 1.7) * 14 * drift;
			const wobY = Math.cos(lt * 0.55 + i * 2.3) * 10 * drift;
			let x = lerp(t.pile[0], t.scatter[0], toScatter) + wobX;
			let y = lerp(t.pile[1], t.scatter[1], toScatter) + wobY;
			let rot = lerp(t.pile[2], t.scatter[2], toScatter);
			let w = TILE_W;
			let h = TILE_H;
			if (toSlot > 0) {
				const sx = rect[0] + pad + t.slot[0] * (rect[2] - pad * 2);
				const sy = rect[1] + barH + t.slot[1] * (rect[3] - barH - pad);
				const sw = t.slot[2] * (rect[2] - pad * 2) - 6;
				const sh = t.slot[3] * (rect[3] - barH - pad) - 6;
				x = lerp(x, sx, toSlot);
				y = lerp(y, sy, toSlot);
				w = lerp(TILE_W, sw, toSlot);
				h = lerp(TILE_H, sh, toSlot);
				rot = lerp(rot, 0, toSlot);
			}
			el.style.width = `${w.toFixed(1)}px`;
			el.style.height = `${h.toFixed(1)}px`;
			el.style.transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px) rotate(${rot.toFixed(2)}deg) scale(${(0.7 + 0.3 * pop).toFixed(3)})`;
			el.style.opacity = (pop * (1 - prog(lt, 19.35, 0.55))).toFixed(4);
			el.style.borderRadius = `${lerp(14, 8, toSlot)}px`;
		});
	}

	// ---- 実画面の場面 ----

	function buildFootage(sec, def) {
		const subs = (def.subs || []).map((s, i) => `<p class="sub" data-i="${i}">${s.html}</p>`).join('');
		const callouts = (def.callouts || [])
			.map((c, i) => `<div class="callout ${c.kind === 'box' ? 'box' : ''}" data-i="${i}">${c.kind === 'box' ? '' : `<span>${c.text}</span>`}</div>`)
			.join('');
		const gap = def.gap ? `<div class="gap"><p>${def.gap.title}</p><div class="dots"><i></i><i></i><i></i></div><small>${def.gap.note}</small></div>` : '';
		sec.innerHTML = `<p class="demo-note">画面は実際の tako。エージェントの出力はデモ用の台本です</p>
			<div class="cap"><span class="chip">${def.chip}</span><p class="head">${def.head}</p>${subs}</div>
			${gap}
			<div class="viewport"><div class="content"><img class="frame" alt=""><div class="lights"><i></i><i></i><i></i></div>${callouts}</div></div>`;
	}

	// 折れ線の補間（[{t, …値}] を時刻でたどる）
	function track(keys, lt, fields) {
		if (!keys || !keys.length) return null;
		if (lt <= keys[0].t) return keys[0];
		for (let i = 1; i < keys.length; i += 1) {
			if (lt <= keys[i].t) {
				const a = keys[i - 1];
				const b = keys[i];
				const p = (b.ease === 'linear' ? (v) => v : easeInOut)(prog(lt, a.t, b.t - a.t));
				const out = {};
				for (const f of fields) out[f] = lerp(a[f], b[f], p);
				return out;
			}
		}
		return keys[keys.length - 1];
	}

	function frameAt(footage, ms) {
		const frames = footage.frames;
		let lo = 0;
		let hi = frames.length - 1;
		while (lo < hi) {
			const mid = (lo + hi + 1) >> 1;
			if (frames[mid][1] <= ms) lo = mid;
			else hi = mid - 1;
		}
		return frames[lo][0];
	}

	function drawFootage(sec, def, lt, dur) {
		const enter = easeOut(prog(lt, def.enter ?? 0, 0.6));
		const leave = easeInOut(prog(lt, dur - 0.45, 0.45));
		const cap = $('.cap', sec);
		cap.style.opacity = (enter * (1 - leave)).toFixed(4);
		cap.style.transform = `translateY(${((1 - enter) * 26).toFixed(2)}px)`;
		$('.demo-note', sec).style.opacity = (enter * (1 - leave)).toFixed(4);

		$$('.sub', sec).forEach((el) => {
			const s = def.subs[Number(el.dataset.i)];
			const a = easeOut(prog(lt, s.t0, 0.4));
			const b = easeInOut(prog(lt, s.t1 - 0.3, 0.3));
			el.style.opacity = (a * (1 - b)).toFixed(4);
			el.style.transform = `translateY(${((1 - a) * 18 - b * 10).toFixed(2)}px)`;
		});

		// 窓の出し入れ
		const vis = track(def.vis, lt, ['o']) || { o: 1 };
		const vp = $('.viewport', sec);
		const vpIn = def.viewportIn === false ? 1 : easeOut(prog(lt, 0, 0.7));
		vp.style.opacity = (vis.o * vpIn * (1 - leave)).toFixed(4);
		vp.style.transform = `translateY(${((1 - vpIn) * 60 + (1 - vis.o) * 26).toFixed(2)}px) scale(${(0.985 + 0.015 * vis.o).toFixed(4)})`;
		const gap = $('.gap', sec);
		if (gap) gap.style.opacity = clamp(1 - vis.o * 1.6).toFixed(4);

		// 実画面のどの瞬間を出すか（clips: 映像の区間 at → 収録の区間 src）
		let clip = def.clips[0];
		for (const c of def.clips) if (lt >= c.at[0]) clip = c;
		const footage = FOOTAGE[clip.footage || def.footage];
		if (footage) {
			const p = prog(lt, clip.at[0], clip.at[1] - clip.at[0]);
			const ms = lerp(clip.src[0], clip.src[1], p) * 1000;
			const index = frameAt(footage, ms);
			setSrc($('img.frame', sec), `${footage.dir}/${String(index).padStart(6, '0')}.jpg`);
		}

		// カメラ（z = 倍率、x / y = 窓の中で真ん中へ寄せたい点。窓の枠からはみ出さない範囲で動く）
		const cam = track(def.cam, lt, ['z', 'x', 'y']) || { z: 1, x: 0.5, y: 0.5 };
		const tx = clamp(VW / 2 - cam.x * VW * cam.z, VW - VW * cam.z, 0);
		const ty = clamp(VH / 2 - cam.y * VH * cam.z, VH - VH * cam.z, 0);
		$('.content', sec).style.transform = `translate(${tx.toFixed(2)}px, ${ty.toFixed(2)}px) scale(${cam.z.toFixed(4)})`;

		$$('.callout', sec).forEach((el) => {
			const c = def.callouts[Number(el.dataset.i)];
			const a = easeOut(prog(lt, c.t0, 0.35));
			const b = easeInOut(prog(lt, c.t1 - 0.3, 0.3));
			el.style.opacity = (a * (1 - b)).toFixed(4);
			el.style.left = `${c.x * VW}px`;
			el.style.top = `${c.y * VH}px`;
			if (c.kind === 'box') {
				el.style.width = `${c.w * VW}px`;
				el.style.height = `${c.h * VH}px`;
			} else {
				el.style.transform = `scale(${((0.9 + 0.1 * a) / cam.z).toFixed(4)})`;
			}
		});
	}

	// ---- 全体 ----

	const scenes = TL.scenes.map((def) => {
		const sec = document.getElementById(def.id);
		if (sec.classList.contains('footage')) buildFootage(sec, def);
		return { def, sec, tweens: $$('[data-a]', sec).map((el) => [el, parseTween(el)]) };
	});
	buildAct1();
	for (const img of $$('img[data-asset]')) setSrc(img, ASSETS[img.dataset.asset] || '');
	// 静止画のカード: 収録の最後の 1 枚から、data-crop="左,上,幅"（窓に対する割合）の範囲を切り出して見せる
	for (const img of $$('img[data-still]')) {
		const f = FOOTAGE[img.dataset.still];
		if (!f) continue;
		setSrc(img, `${f.dir}/${String(f.frames[f.frames.length - 1][0]).padStart(6, '0')}.jpg`);
		const [x, y, w] = (img.dataset.crop || '0,0,1').split(',').map(Number);
		const width = 520 / w; // カードの絵の幅（comp.css の .card .shot と同じ）
		img.style.width = `${width}px`;
		img.style.height = `${(width * 9) / 16}px`;
		img.style.left = `${-x * width}px`;
		img.style.top = `${(-y * width * 9) / 16}px`;
	}

	window.__seek = async (t) => {
		for (const { def, sec, tweens } of scenes) {
			const on = t >= def.start && t < def.end;
			sec.style.display = on ? 'block' : 'none';
			if (!on) continue;
			const lt = t - def.start;
			for (const [el, spec] of tweens) applyTween(el, spec, lt);
			if (def.id === 'act1') drawAct1(lt);
			if (sec.classList.contains('footage')) drawFootage(sec, def, lt, def.end - def.start);
		}
		await Promise.all(pending.splice(0));
	};

	window.__ready = (async () => {
		await document.fonts.ready;
		await Promise.all(pending.splice(0));
	})();
})();
