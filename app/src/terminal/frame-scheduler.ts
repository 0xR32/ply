/** What the scheduler flushes: a terminal session, notified once its changes are due. */
export interface Flushable {
  readonly isFocused: boolean;
  notify(): void;
}

/** The clock and timer the scheduler runs on; tests pass a fake. */
export interface SchedulerClock {
  now(): number;
  setTimer(run: () => void, ms: number): unknown;
  clearTimer(timer: unknown): void;
}

const FRAME_MS = 16;
const BACKGROUND_MS = 50;
/** The longest the scheduler waits between two flushes of a pane, after backing off from a busy main thread (ms). */
export const MAX_FLUSH_MS = 100;
const LATE_MS = 8;
const RECOVER_MS = 4;

/**
 * One timer for every pane (P1, Ruling R30; plyd sends up to 120 Hz): the focused pane at most every 16 ms, the others every 50 ms, all due panes in one React batch; a timer that fires 8 ms late or more, a main thread busy drawing, doubles the interval up to 100 ms, and each one on time takes 4 ms back off.
 */
export class FlushScheduler {
  private readonly pending = new Set<Flushable>();
  private readonly lastFlush = new WeakMap<Flushable, number>();
  private timer: unknown = null;
  private timerAt = 0;
  private interval = FRAME_MS;

  constructor(private readonly clock: SchedulerClock) {}

  /** Marks `item` changed; it is notified at its next due flush, at once after an idle stretch. */
  schedule(item: Flushable): void {
    this.pending.add(item);
    this.arm();
  }

  /** Drops `item` from the next flush (a disposed session). */
  cancel(item: Flushable): void {
    this.pending.delete(item);
  }

  private due(item: Flushable): number {
    const last = this.lastFlush.get(item) ?? Number.NEGATIVE_INFINITY;
    return last + (item.isFocused ? this.interval : Math.max(BACKGROUND_MS, this.interval));
  }

  private arm(): void {
    if (this.pending.size === 0) return;
    const now = this.clock.now();
    let at = Number.POSITIVE_INFINITY;
    for (const item of this.pending) at = Math.min(at, this.due(item));
    at = Math.max(now, at);
    if (this.timer !== null) {
      if (this.timerAt <= at) return;
      this.clock.clearTimer(this.timer);
    }
    this.timerAt = at;
    this.timer = this.clock.setTimer(() => this.flush(), at - now);
  }

  private flush(): void {
    this.timer = null;
    const now = this.clock.now();
    // A pane without focus goes up to a frame early rather than alone, so it never costs a redraw of its own.
    const due = [...this.pending].filter(
      (item) => this.due(item) <= (item.isFocused ? now : now + this.interval),
    );
    this.interval =
      now - this.timerAt >= LATE_MS
        ? Math.min(MAX_FLUSH_MS, this.interval * 2)
        : Math.max(FRAME_MS, this.interval - RECOVER_MS);
    for (const item of due) {
      this.pending.delete(item);
      this.lastFlush.set(item, now);
    }
    for (const item of due) item.notify();
    this.arm();
  }
}
