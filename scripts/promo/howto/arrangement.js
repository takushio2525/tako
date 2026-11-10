// ---- 構成（入れ方・使い方動画 #2866。v2 = #2886。小節は 1 始まり。bar(b) = その小節の頭の秒） ----
//
// このファイルは単体では動かない断片で、stage.mjs が #2864 の bgm.mjs の「構成」の節と差し替える。
// 使っている楽器・和音・型（pad / drums / bassBar / arp / phrase / pluck / kick / hat / snap / bass / riser /
// CH / PH_A / PH_B / BEAT / BAR）はすべて #2864 の bgm.mjs のもの。ここで決めるのは
// 「どの小節で何を鳴らすか」だけ。96 BPM・1 小節 2.5 秒で、場面の切り替わり（comp/timeline.json）は
// すべて小節の頭にある。全 100 小節 = 250 秒。
//
// 作り分け: 読ませる場面（図解と、対話のセットアップ）は刻みを軽く、動きのある実画面の場面は 16 分の
// 分散和音で前へ進める。章の見出し（1 小節）では一度止めて、上がる分散和音で次の章を開ける。
// 対話のセットアップは 70 秒続き、画面の文字を読ませる場面なので、伴奏を一段下げて旋律を間引いてある。

const bar = (b) => (b - 1) * BAR;
const PROG = [CH.D, CH.Bm, CH.G, CH.A];

function chapterCard(b) {
	const t = bar(b);
	kick(t, 1);
	pad(t, CH.As.notes, BAR, 0.9);
	bass(t, CH.As.root, BEAT * 2.5, 0.9);
	[57, 62, 64, 69, 74, 76, 81].forEach((m, i) => pluck(t + i * (BEAT / 4), m, 0.85, -0.5 + i * 0.16, 4, 0.6));
	for (let k = 0; k < 8; k += 1) hat(t + BEAT * 2 + (k * BEAT) / 4, 0.35 + k * 0.08, false, 0);
}

function calm(b, c, level = 1) {
	const t = bar(b);
	pad(t, c.notes, BAR, 0.9 * level);
	drums(t, 0.75 * level);
	bassBar(t, c, 0.85 * level);
	arp(t, c, false, 0.7 * level);
}

// ドラムは #2864 の本編より少し下げる: 実画面の場面が長く続くので、同じ強さだと全体の音量を -15 LUFS へ
// 上げたときにリミッタへ当たり続け、AAC にした後の true peak が -0.1 dBFS まで上がった（実測）
function drive(b, c, level = 1, ghost = false) {
	const t = bar(b);
	pad(t, c.notes, BAR, 0.9 * level);
	drums(t, 0.8 * level, ghost);
	bassBar(t, c, level);
	arp(t, c, true, 0.62 * level);
}

// 1–3 小節: 題字。開けて始める
kick(bar(1), 1);
[62, 66, 69, 74, 78, 81, 86].forEach((m, i) => pluck(bar(1) + i * (BEAT / 4), m, 0.9, -0.5 + i * 0.16, 4, 0.6));
[CH.D, CH.Bm].forEach((c, i) => {
	const t = bar(1 + i);
	pad(t, c.notes, BAR, 1);
	bass(t, c.root, BEAT * 2.5, 0.9);
	if (i === 1) arp(t, c, false, 0.8);
});
pad(bar(3), CH.G.notes, BEAT * 2, 1);
pad(bar(3) + BEAT * 2, CH.A.notes, BEAT * 2, 1);
bass(bar(3), CH.G.root, BEAT * 1.6, 0.9);
bass(bar(3) + BEAT * 2, CH.A.root, BEAT * 1.6, 0.9);
arp(bar(3), CH.G, false, 0.8);
riser(bar(3) + BEAT * 2, bar(4), 0.7);

// 4 小節: 章 1 の見出し
chapterCard(4);

// 5–17 小節: 入れる（Homebrew 5 小節 / zip 4 小節 / Windows 4 小節。図解なので軽く）
for (let i = 0; i < 13; i += 1) calm(5 + i, PROG[i % 4]);
riser(bar(17) + BEAT * 2, bar(18), 0.8);

// 18 小節: 章 2 の見出し
chapterCard(18);

// 19–23 小節: 初回起動（実画面。案内とボタン）
[CH.Bm, CH.G, CH.D, CH.A, CH.Bm].forEach((c, i) => drive(19 + i, c, 1, i % 2 === 1));
phrase(bar(21), PH_A, 0.9);

// 24–30 小節: ボタン → tako setup（実画面）
[CH.G, CH.D, CH.A, CH.Bm, CH.G, CH.D, CH.A].forEach((c, i) => drive(24 + i, c, 1, i % 2 === 1));
phrase(bar(26), PH_B, 0.9);

// 31–52 小節: 対話のセットアップ（master / worker 10 小節・プロジェクト 4 小節・GitHub 8 小節）。読ませるので軽く
for (let i = 0; i < 22; i += 1) calm(31 + i, PROG[i % 4], 0.92);
phrase(bar(35), PH_A, 0.6);
phrase(bar(47), PH_B, 0.6);

// 53–58 小節: 対話を閉じて「master を起動」へ（実画面。区切りなので少し前へ出す）
[CH.D, CH.Bm, CH.G, CH.A, CH.D, CH.A].forEach((c, i) => drive(53 + i, c, 0.95, i % 2 === 1));
phrase(bar(55), PH_A, 0.8);

// 59–66 小節: Claude Code が無いとき（実画面。分かれ道の説明なので軽く）
for (let i = 0; i < 8; i += 1) calm(59 + i, PROG[(i + 1) % 4]);

// 67–71 小節: アップデートの方法（図解）
[CH.G, CH.A, CH.D, CH.Bm, CH.G].forEach((c, i) => calm(67 + i, c));

// 72–74 小節: 更新の確認（実画面）。章 3 へ向けて上げる
[CH.A, CH.Bm, CH.A].forEach((c, i) => drive(72 + i, c, 0.95, i === 2));
riser(bar(74) + BEAT * 2, bar(75), 0.9);

// 75 小節: 章 3 の見出し
chapterCard(75);

// 76–90 小節: master（実画面。この動画の締めなので旋律を足す）
[CH.D, CH.Bm, CH.G, CH.A, CH.D, CH.Bm, CH.G, CH.A, CH.Bm, CH.G, CH.D, CH.A, CH.Bm, CH.G, CH.A].forEach((c, i) =>
	drive(76 + i, c, 1, i % 4 === 3),
);
phrase(bar(80), PH_A);
phrase(bar(84), PH_B);
phrase(bar(88), PH_A);

// 91–95 小節: 覚えておくと便利なこと（図解）
[CH.D, CH.Bm, CH.G, CH.A, CH.G].forEach((c, i) => calm(91 + i, c));
riser(bar(95) + BEAT * 2, bar(96), 0.8);

// 96–100 小節: まとめ。主和音に着地して、鳴らし切る
kick(bar(96), 1);
[CH.D, CH.G, CH.D].forEach((c, i) => {
	const t = bar(96 + i);
	pad(t, c.notes, BAR, 1);
	bass(t, c.root, BEAT * 3, 0.9);
	arp(t, c, false, 0.7 - i * 0.12);
	snap(t + BEAT, 0.5);
	snap(t + BEAT * 3, 0.5);
});
kick(bar(99), 0.8);
pad(bar(99), [50, 57, 62, 64, 66, 69], BAR * 1.6, 1);
bass(bar(99), 38, BEAT * 6, 0.9);
[62, 66, 69, 76, 81].forEach((m, i) => pluck(bar(99) + i * (BEAT / 4), m, 0.8, -0.4 + i * 0.2, 2.2, 0.8));
