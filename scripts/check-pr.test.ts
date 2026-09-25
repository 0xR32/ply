import { describe, expect, test } from 'bun:test';
import { checkPrBody } from './check-pr';

describe('check-pr', () => {
  test('accepts a body with Spec and WP lines', () => {
    expect(checkPrBody('Adds the gates.\n\nSpec: 6.0.0\nWP: 1\n')).toEqual([]);
    expect(checkPrBody('Spec: 6.0.0\nWP: 1, 2')).toEqual([]);
  });

  test('names each missing line', () => {
    expect(checkPrBody('')).toHaveLength(2);
    expect(checkPrBody('Spec: 6.0\nWP: 1')).toHaveLength(1);
    expect(checkPrBody('Spec: 6.0.0\nWP: one')).toHaveLength(1);
  });
});
