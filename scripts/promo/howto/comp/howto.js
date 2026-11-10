// 入れ方・使い方動画（#2866）が、#2864 の合成エンジン（comp.js）に足す 3 つ。comp.js の後ろで読む。
//
//   1. 実画面の場面の右上の注記を、場面ごとに差し替える（timeline.json の note。無ければ comp.js の文のまま）
//   2. 図解の場面の終わりを、場面ごとまとめて消す（部品 1 つずつに out を書かない）
//   3. ターミナルの再現のカットで、コマンドを 1 文字ずつ打つ（data-type="開始,長さ"。時刻は場面の頭から）
//
// comp.js と同じく壁時計は使わない。`window.__seek(秒)` に同じ時刻を渡せば、必ず同じ絵になる。
(() => {
	const TL = window.TL;
	const clamp = (v) => Math.min(1, Math.max(0, v));
	const easeInOut = (p) => (p < 0.5 ? 4 * p * p * p : 1 - (-2 * p + 2) ** 3 / 2);
	const FADE = 0.4;

	const scenes = TL.scenes.map((def) => ({ def, sec: document.getElementById(def.id) }));

	for (const { def, sec } of scenes) {
		const note = sec.querySelector('.demo-note');
		if (note && def.note) note.textContent = def.note;
	}

	// 打つ文字: 元の文を覚えておき、時刻に応じて先頭から見せる。打っている間だけ後ろにカーソルを置く
	const typed = [];
	for (const { def, sec } of scenes) {
		for (const el of sec.querySelectorAll('[data-type]')) {
			const [start, dur] = el.dataset.type.split(',').map(Number);
			const caret = document.createElement('i');
			caret.className = 'caret';
			el.after(caret);
			typed.push({ def, el, caret, start, dur, text: el.textContent });
		}
	}

	const base = window.__seek;
	window.__seek = async (t) => {
		await base(t);
		for (const { def, sec } of scenes) {
			if (t < def.start || t >= def.end || sec.classList.contains('footage')) continue;
			const left = def.end - t;
			sec.style.opacity = (1 - easeInOut(clamp((FADE - left) / FADE))).toFixed(4);
		}
		for (const ty of typed) {
			if (t < ty.def.start || t >= ty.def.end) continue;
			const lt = t - ty.def.start;
			const p = clamp((lt - ty.start) / ty.dur);
			ty.el.textContent = ty.text.slice(0, Math.round(ty.text.length * p));
			// 打ち始める少し前から、打ち終えて結果が出るまでの間だけ見せる
			ty.caret.style.display = lt >= ty.start - 0.5 && lt < ty.start + ty.dur + 0.5 ? 'inline-block' : 'none';
		}
	};
})();
