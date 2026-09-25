import { useGpuixRequired } from '@gpuix/react';
import { useEffect } from 'react';

declare global {
  var plyWindowZoomed: boolean | undefined;
}

/** Zooms the window once per process so it fills its screen; `PLY_WINDOW_ZOOM=0` keeps the opening size. */
export function StartZoomed(): null {
  const renderer = useGpuixRequired();
  useEffect(() => {
    // The native zoom toggles and bun --hot remounts this component, so only a process's first mount zooms.
    if (globalThis.plyWindowZoomed || process.env.PLY_WINDOW_ZOOM === '0') return;
    globalThis.plyWindowZoomed = true;
    renderer.zoomWindow?.();
  }, [renderer]);
  return null;
}
