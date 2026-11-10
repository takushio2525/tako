// 失敗したら間隔を倍にしながらやり直す
export interface RetryOptions {
  retries: number;
  baseMs?: number;
}

export async function withRetry<T>(run: () => Promise<T>, opts: RetryOptions): Promise<T> {
  const base = opts.baseMs ?? 200;
  let lastError: unknown;
  for (let attempt = 0; attempt <= opts.retries; attempt += 1) {
    try {
      return await run();
    } catch (error) {
      lastError = error;
      if (attempt === opts.retries) break;
      await sleep(base * 2 ** attempt);
    }
  }
  throw lastError;
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
