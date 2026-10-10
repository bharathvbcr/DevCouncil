import { vi } from 'vitest';
vi.mock('../services/api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../services/api')>()),
  getPapers: vi.fn(),
}));
function testHelperAfterTypeofImport(): number { return 2; }
export function callsHelper(): number { return testHelperAfterTypeofImport(); }
