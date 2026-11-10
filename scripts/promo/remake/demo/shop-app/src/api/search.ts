import { query } from '../lib/db';

export interface Product {
  id: string;
  name: string;
  price: number;
}

export interface Page<T> {
  items: T[];
  next: number | null;
}

// 名前の部分一致で商品を探す
export async function search(q: string, offset = 0, limit = 20): Promise<Page<Product>> {
  const rows = await query<Product>(
    'SELECT id, name, price FROM products WHERE name LIKE ? ORDER BY name LIMIT ? OFFSET ?',
    [`%${q}%`, limit + 1, offset],
  );
  const items = rows.slice(0, limit);
  return { items, next: rows.length > limit ? offset + limit : null };
}
