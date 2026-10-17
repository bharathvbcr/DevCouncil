import { describe, it, vi, expect } from 'vitest';

const fetchThing = vi.fn();

describe('fetchThing', () => {
  it('is the mock declared at module scope', () => {
    fetchThing();
    expect(fetchThing).toHaveBeenCalled();
  });
});
