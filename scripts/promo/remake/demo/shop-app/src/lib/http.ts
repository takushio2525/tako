export interface Response {
  ok: boolean;
  status: number;
}

export async function post(path: string, body: unknown): Promise<Response> {
  const res = await fetch(path, { method: 'POST', body: JSON.stringify(body) });
  return { ok: res.ok, status: res.status };
}
