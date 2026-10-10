import { describe, it, vi, expect } from 'vitest';
vi.mock('../services/api', () => ({ getPapers: vi.fn() }));
import { loadAll } from '../services/api';
describe('loadAll', () => { it('runs', async () => { expect(await loadAll()).toBeUndefined(); }); });
