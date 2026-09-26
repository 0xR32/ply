import { describe, expect, test } from 'bun:test';
import { FlushScheduler } from './frame-scheduler';

function fakeClock() {
  let now = 0;
  let timer: { at: number; run: () => void } | null = null;
  return {
    now: () => now,
    setTimer: (run: () => void, ms: number) => {
      timer = { at: now + ms, run };
      return 1;
    },
    clearTimer: () => {
      timer = null;
    },
    /** Moves to `to`, running the timer whenever it is due; `lag` makes each run that many ms late, as a busy main thread does. */
    advance(to: number, lag = 0) {
      while (timer && timer.at + lag <= to) {
        const due = timer;
        timer = null;
        now = due.at + lag;
        due.run();
      }
      now = to;
    },
  };
}

function pane(clock: ReturnType<typeof fakeClock>, isFocused = true) {
  const flushes: number[] = [];
  return { isFocused, flushes, notify: () => flushes.push(clock.now()) };
}

/** Changes `panes` every millisecond from the clock's time until `to`. */
function stream(
  clock: ReturnType<typeof fakeClock>,
  scheduler: FlushScheduler,
  panes: ReturnType<typeof pane>[],
  to: number,
  lag = 0,
) {
  for (let t = clock.now(); t <= to; t++) {
    clock.advance(t, lag);
    for (const p of panes) scheduler.schedule(p);
  }
}

describe('FlushScheduler', () => {
  test('the first change after an idle stretch flushes at once; later ones wait for the next 16 ms frame', () => {
    const clock = fakeClock();
    const s = new FlushScheduler(clock);
    const a = pane(clock);
    s.schedule(a);
    clock.advance(0);
    clock.advance(5);
    s.schedule(a);
    clock.advance(10);
    s.schedule(a);
    clock.advance(40);
    expect(a.flushes).toEqual([0, 16]);
  });

  test('every due pane flushes in the same batch', () => {
    const clock = fakeClock();
    const s = new FlushScheduler(clock);
    const a = pane(clock);
    const b = pane(clock);
    stream(clock, s, [a, b], 50);
    expect(a.flushes).toEqual([0, 16, 32, 48]);
    expect(b.flushes).toEqual(a.flushes);
  });

  test('a pane without focus flushes at most every ~50 ms, riding in a focused pane’s batch', () => {
    const clock = fakeClock();
    const s = new FlushScheduler(clock);
    const focused = pane(clock);
    const other = pane(clock, false);
    stream(clock, s, [focused, other], 100);
    expect(focused.flushes).toEqual([0, 16, 32, 48, 64, 80, 96]);
    expect(other.flushes).toEqual([0, 48, 96]);
  });

  test('panes without focus flush every 50 ms on their own', () => {
    const clock = fakeClock();
    const s = new FlushScheduler(clock);
    const other = pane(clock, false);
    stream(clock, s, [other], 120);
    expect(other.flushes).toEqual([0, 50, 100]);
  });

  test('late timers stretch the interval up to 100 ms; timers on time bring it back to 16 ms', () => {
    const clock = fakeClock();
    const s = new FlushScheduler(clock);
    const a = pane(clock);
    const gaps = () => a.flushes.slice(1).map((t, i) => t - (a.flushes[i] as number));
    stream(clock, s, [a], 450, 30);
    // Each run 30 ms late: intervals of 32, 64, then the 100 ms cap, plus the 30 ms lag.
    expect(gaps()).toEqual([62, 94, 130, 130]);
    a.flushes.length = 0;
    stream(clock, s, [a], 3000);
    expect(gaps().slice(0, 3)).toEqual([96, 92, 88]);
    expect(gaps().slice(-3)).toEqual([16, 16, 16]);
  });

  test('a cancelled pane is not flushed', () => {
    const clock = fakeClock();
    const s = new FlushScheduler(clock);
    const a = pane(clock);
    s.schedule(a);
    s.cancel(a);
    clock.advance(40);
    expect(a.flushes).toEqual([]);
  });
});
