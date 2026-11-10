#!/usr/bin/env node
// 収録用の「dev サーバーらしい出力」。ポートは開かない（何も待ち受けない）
const paths = ['/api/products', '/api/search?q=tea', '/api/orders', '/api/payments', '/assets/app.js', '/api/health'];
const c = { dim: '\x1b[38;2;128;134;146m', ok: '\x1b[38;2;98;196;140m', info: '\x1b[38;2;118;170;255m', r: '\x1b[0m' };
const stamp = () => new Date().toTimeString().slice(0, 8);
console.log(`\n  ${c.ok}shop-app dev${c.r}  ready in 312 ms\n`);
console.log(`  ${c.dim}Local:${c.r}   ${c.info}http://localhost:5173/${c.r}\n`);
let n = 0;
setInterval(() => {
	const p = paths[n % paths.length];
	const ms = 6 + ((n * 7) % 37);
	const method = p.includes('orders') || p.includes('payments') ? 'POST' : 'GET ';
	console.log(`${c.dim}${stamp()}${c.r} ${method} ${p.padEnd(20)} ${c.ok}200${c.r} ${String(ms).padStart(3)}ms`);
	n += 1;
}, Number(process.env.DEMO_TICK_MS || 900));
