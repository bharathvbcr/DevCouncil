export const exportedArrow = async (n: number): Promise<number> => n + 1;
export enum Mode { A = 'a', B = 'b' }
export function modeName(m: Mode): string { return m; }
