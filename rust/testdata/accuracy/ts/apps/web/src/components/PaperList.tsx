import { Button, MemoBadge, FancyInput, Parts } from './Button';
import { usePapers } from '@/hooks/usePapers';
import { useAuth } from '../ctx/AuthContext';
import * as api from '../services/api';
import { fmt, namedA, aliasDefaultA, starB } from '@/lib';
import defaultA from '@lib/a';
import { formatTitle as ft } from '@lib/format';
import { jsDouble } from '../legacy/jsutil';
import { caseHelper } from '../lib/CaseFile';
import { sharedFormat } from '@acme/shared';
import type { Client } from '../services/store';
export function savePaper(): void { api.postNote('n'); }
export function PaperList({ client }: { client?: Client }) {
  const { items, reload } = usePapers('q');
  const auth = useAuth();
  const handleSave = () => savePaper();
  const total = namedA() + aliasDefaultA() + starB() + defaultA() + jsDouble(1) + caseHelper();
  return (
    <div>
      <h2>{fmt('t')}{ft('u')}{sharedFormat(' s ')}{total}</h2>
      <Button label="save" onPress={handleSave} />
      <Button label="reload" onPress={reload} />
      <Button label="out" onPress={() => auth.signOut()} />
      <MemoBadge n={items.length} />
      <FancyInput v="x" />
      <Parts.Header />
      <button onClick={() => api.loadAll()}>all</button>
    </div>
  );
}
