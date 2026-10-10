function defaultSleep(ms: number): Promise<void> { return new Promise((r) => setTimeout(r, ms)); }
function repoRoot(): string { return '/'; }
function plainDefault(): number { return 1; }
export async function withDeps({ sleep = defaultSleep, cwd = repoRoot() }: { sleep?: (ms: number) => Promise<void>; cwd?: string } = {}): Promise<string> {
  await sleep(1);
  return cwd;
}
export function withPlain(n: number = plainDefault()): number { return n; }
