#!/usr/bin/env node
// 収録用の「テストの見張りらしい出力」。回数が進むので、再起動の前後で続いていることが分かる
const c = { dim: '\x1b[38;2;128;134;146m', ok: '\x1b[38;2;98;196;140m', r: '\x1b[0m' };
const stamp = () => new Date().toTimeString().slice(0, 8);
console.log(`\n  ${c.ok}test --watch${c.r}  変更を待っています\n`);
let run = 0;
setInterval(() => {
	run += 1;
	console.log(`${c.dim}${stamp()}${c.r} run #${String(run).padStart(2)}  ${c.ok}21 passed${c.r} ${c.dim}(${(1.6 + (run % 5) * 0.1).toFixed(1)}s)${c.r}`);
}, Number(process.env.DEMO_TICK_MS || 2000));
