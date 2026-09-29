import { defineCollection } from 'astro:content';
import { z } from 'astro/zod';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';

export const collections = {
	docs: defineCollection({
		loader: docsLoader(),
		schema: docsSchema({
			extend: z.object({
				// 検索結果に出す `<title>`（と og:title）だけを差し替える。見出し（h1）・サイドバー・
				// OG 画像は `title` のまま。「Claude Code」のような短い見出しでは、検索した人に
				// 何のページか伝わらないため（#1843）。末尾の「| tako」は src/starlightRouteData.ts が付ける
				seoTitle: z.string().optional(),
			}),
		}),
	}),
};
