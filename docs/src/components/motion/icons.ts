/**
 * 画面の再現（モーション解説。#2053）で使う線画アイコン。
 *
 * tako 本体と同じく**絵文字は使わない**（印は描画プリミティブで描く。
 * `.agent/conventions.md`「UI に絵文字を使わない」）。形は本体の
 * `crates/tako-app/src/file_icons.rs` の ui_icon と同じ系統（24px グリッド・線幅 2）。
 * 値は `<svg>` の中身。`Icon.astro` が包む。
 */
export const ICONS = {
	search: '<circle cx="11" cy="11" r="7"/><path d="m21 21-4.35-4.35"/>',
	bell: '<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/>',
	terminal: '<path d="m4 17 6-6-6-6"/><path d="M12 19h8"/>',
	chat: '<path d="M21 11.5a8.4 8.4 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.4 8.4 0 0 1-3.8-.9L3 21l1.9-5.7a8.4 8.4 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.4 8.4 0 0 1 3.8-.9h.5a8.5 8.5 0 0 1 8 8z"/>',
	sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M6.3 17.7l-1.4 1.4M19.1 4.9l-1.4 1.4"/>',
	folder: '<path d="M3 6.5A1.5 1.5 0 0 1 4.5 5H9l2 2h8.5A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5z"/>',
	branch: '<path d="M6 3v12"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/>',
	globe: '<circle cx="12" cy="12" r="10"/><path d="M2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z"/>',
	phone: '<rect x="7" y="2" width="10" height="20" rx="2"/><path d="M11 18h2"/>',
	columns: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M12 3v18"/>',
	minus: '<path d="M5 12h14"/>',
	x: '<path d="M18 6 6 18M6 6l12 12"/>',
	square: '<rect x="5" y="5" width="14" height="14" rx="1.5"/>',
	play: '<path d="M7 4.5v15l12-7.5z" fill="currentColor" stroke="none"/>',
	copy: '<rect x="9" y="9" width="12" height="12" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>',
	chevronDown: '<path d="m6 9 6 6 6-6"/>',
	chevronRight: '<path d="m9 6 6 6-6 6"/>',
	check: '<path d="M20 6 9 17l-5-5"/>',
	tasks: '<path d="m3 7 2 2 4-4M3 17l2 2 4-4M13 6h8M13 12h8M13 18h8"/>',
	fleet: '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M9 21V9"/>',
	orch: '<circle cx="12" cy="5" r="2.4"/><circle cx="5" cy="19" r="2.4"/><circle cx="19" cy="19" r="2.4"/><path d="M12 7.4v3.6M12 11H5v5.6M12 11h7v5.6"/>',
	file: '<path d="M14.5 2H7a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V6.5z"/><path d="M14 2v5h5"/>',
	image: '<rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.1-3.1a2 2 0 0 0-2.8 0L6 21"/>',
	arrowUp: '<path d="M12 19V5M5 12l7-7 7 7"/>',
	download: '<path d="M12 3v12M7 10l5 5 5-5M5 21h14"/>',
	shield: '<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>',
	plus: '<path d="M12 5v14M5 12h14"/>',
	laptop: '<path d="M4 6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v9H4z"/><path d="M2 19h20"/>',
	monitor: '<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8M12 17v4"/>',
	refresh: '<path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 4v5h-5"/>',
	lock: '<rect x="4" y="11" width="16" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>',
	package: '<path d="m7.5 4.3 9 5.2M21 16V8a2 2 0 0 0-1-1.7l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.7l7 4a2 2 0 0 0 2 0l7-4a2 2 0 0 0 1-1.7z"/><path d="M3.3 7 12 12l8.7-5M12 22V12"/>',
	bolt: '<path d="M13 2 3 14h9l-1 8 10-12h-9z"/>',
	layers: '<path d="m12 2 10 5-10 5L2 7z"/><path d="m2 17 10 5 10-5M2 12l10 5 10-5"/>',
	pencil: '<path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z"/>',
} as const;

export type IconName = keyof typeof ICONS;
