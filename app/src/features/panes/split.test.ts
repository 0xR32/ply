import { describe, expect, test } from 'bun:test';
import { moveBoundary, tracks } from './split';

describe('tracks', () => {
  test('splits a length into whole-pixel tracks with the gaps between and nothing lost', () => {
    expect(tracks([1 / 3, 1 / 3, 1 / 3], 6, 986, 10)).toEqual([
      { start: 6, size: 322 },
      { start: 338, size: 322 },
      { start: 670, size: 322 },
    ]);
    const uneven = tracks([0.25, 0.75], 0, 1001, 10);
    expect(uneven[0]?.size).toBe(248);
    expect((uneven[1]?.start ?? 0) + (uneven[1]?.size ?? 0)).toBe(1001);
  });
});

describe('moveBoundary', () => {
  test('puts the boundary under the pointer and keeps both tracks at least the minimum', () => {
    const half = [0.5, 0.5];
    const moved = moveBoundary(half, 0, 605, 1010, 10, 200);
    expect(moved[0]).toBeCloseTo(0.6, 6);
    expect(moved[1]).toBeCloseTo(0.4, 6);
    expect(moveBoundary(half, 0, 5_000, 1010, 10, 200)).toEqual([0.8, 0.19999999999999996]);
    expect(moveBoundary(half, 0, -50, 1010, 10, 200)[0]).toBeCloseTo(0.2, 6);
    expect(
      moveBoundary([0.2, 0.3, 0.5], 1, 505, 1020, 10, 100).reduce((a, b) => a + b),
    ).toBeCloseTo(1, 9);
    expect(moveBoundary(half, 0, 400, 300, 10, 200), 'too little room for two minimums').toEqual(
      half,
    );
  });
});
