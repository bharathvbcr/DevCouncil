import { MemClient, type Client } from '../services/store';
export function viaParam(m: MemClient): string { return m.get('p'); }
export function viaIface(c: Client): string { return c.get('q'); }
export const viaArrowParam = (m: MemClient): string => m.get('r');
