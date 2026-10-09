import { describe, it, expect } from 'vitest';
import { formatTitle } from './format';
describe('formatTitle', () => {
  it('uppercases', () => { expect(formatTitle('a')).toBe('A'); });
});
