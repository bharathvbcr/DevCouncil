import { useEffect } from 'react';
import { usePapers } from '../hooks/usePapers';
import CardDefault, { Card } from './widgets';
function effectLoad(): void {}
export function EffectUser() {
  const { reload } = usePapers('q');
  useEffect(() => { effectLoad(); reload(); }, [reload]);
  return <><Card t="a" /><CardDefault t="b" /></>;
}
