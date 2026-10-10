export interface Client { get(k: string): string; }
export class MemClient implements Client {
  get(k: string): string { return this.norm(k); }
  private norm(k: string): string { return k.toLowerCase(); }
  handle = (k: string): string => this.get(k);
  static create(): MemClient { return new MemClient(); }
}
export function useClient(c: Client): string { return c.get('a'); }
export function viaConcrete(): string { const m = new MemClient(); return m.get('b'); }
export function viaStatic(): string { return MemClient.create().handle('z'); }
export const paperService = {
  fetchOne(id: string): string { return svcHelper(id); },
  remove: (id: string): string => svcHelper(id),
};
function svcHelper(id: string): string { return id; }
export function useService(): string { return paperService.fetchOne('1') + paperService.remove('2'); }
