import { useGpuix } from '@gpuix/react';
import { useEffect } from 'react';
import { log } from '../ipc/log';
import { terminalStats } from '../terminal/session';

const WINDOW_MS = 1_000;
const FRAME_60HZ_MS = 1000 / 60;

function quantile(sorted: number[], q: number): number {
  return sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] ?? 0;
}

/** Bench probe of the full app (`PLY_TERMINAL_STATS=1`, docs/perf.md): logs GPUI draw time and main-thread stalls every second. */
export function FrameStats() {
  const { renderer } = useGpuix();
  useEffect(() => {
    if (!renderer) return;
    const gaps: number[] = [];
    let last = performance.now();
    // A 1 ms timer only fires between tasks, so each gap is how long the main thread was busy with a frame or JS.
    const probe = setInterval(() => {
      const now = performance.now();
      gaps.push(now - last);
      last = now;
    }, 1);
    let frames = renderer.getDebugFrameOverlayStats?.().frames ?? 0;
    renderer.resetDebugFrameOverlayStats?.();
    const report = setInterval(() => {
      const draw = renderer.getDebugFrameOverlayStats?.();
      renderer.resetDebugFrameOverlayStats?.();
      const sorted = gaps.splice(0).sort((a, b) => a - b);
      log('info', 'frame stats', {
        draw_p90_ms: draw?.p90Ms,
        draw_p99_ms: draw?.p99Ms,
        draw_max_ms: draw?.maxMs,
        frames: (draw?.frames ?? frames) - frames,
        stall_p99_ms: quantile(sorted, 0.99),
        stall_max_ms: sorted.at(-1) ?? 0,
        stalls_over_16ms: sorted.filter((g) => g > FRAME_60HZ_MS).length,
        stalls_over_33ms: sorted.filter((g) => g > 2 * FRAME_60HZ_MS).length,
        panes: terminalStats().length,
      });
      frames = draw?.frames ?? frames;
    }, WINDOW_MS);
    return () => {
      clearInterval(probe);
      clearInterval(report);
    };
  }, [renderer]);
  return null;
}
