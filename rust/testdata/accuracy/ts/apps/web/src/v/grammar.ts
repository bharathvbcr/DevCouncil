export interface Paper {
  title: string;
  abstract?: string;
  year: number;
}
function beforeHelper(): number { return 1; }
export function afterAbstract(p: Paper): number { return beforeHelper() + afterHelper(p); }
function afterHelper(p: Paper): number { return p.year; }
