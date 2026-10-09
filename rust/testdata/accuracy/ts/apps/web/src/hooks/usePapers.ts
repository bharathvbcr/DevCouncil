import { useCallback, useMemo, useState } from 'react';
import { getPapers } from '../services/api';
import { identity } from '@lib/format';
export function computeRank(n: number): number { return n + 1; }
export function usePapers(q: string) {
  const [items, setItems] = useState<unknown[]>([]);
  const reload = useCallback(async () => { const r = await getPapers(q); setItems([r]); }, [q]);
  const rank = useMemo(() => computeRank(items.length), [items]);
  return { items, reload, rank: identity<number>(rank) };
}
