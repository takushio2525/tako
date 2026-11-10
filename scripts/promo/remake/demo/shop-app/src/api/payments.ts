import { withRetry } from '../lib/retry';
import { post } from '../lib/http';

export interface Order {
  id: string;
  total: number;
}

export interface Receipt {
  orderId: string;
  paidAt: string;
}

// 決済する。通信に失敗したら最大 retries 回までやり直す
export async function pay(order: Order, opts = { retries: 3 }): Promise<Receipt> {
  return withRetry(async () => {
    const res = await post('/gateway/charge', { orderId: order.id, amount: order.total });
    if (!res.ok) throw new Error(`charge failed: ${res.status}`);
    return { orderId: order.id, paidAt: new Date().toISOString() };
  }, opts);
}
