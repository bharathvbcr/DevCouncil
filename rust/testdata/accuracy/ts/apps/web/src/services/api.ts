export async function getPapers(q: string): Promise<unknown> {
  const res = await fetch(`/api/papers?q=${q}`);
  return res.json();
}
export function postNote(body: string): Promise<Response> {
  return fetch('/api/notes', { method: 'POST', body });
}
export function parseBody(r: unknown): unknown { return r; }
export function loadAll(): Promise<unknown> {
  return getPapers('x').then(parseBody).then((r) => logIt(r));
}
function logIt(r: unknown): unknown { return r; }
