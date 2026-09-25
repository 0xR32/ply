import { describe, expect, test } from 'bun:test';
import { createTestRoot, hasNativeTestRenderer } from '@gpuix/react/testing';
import { App } from './App';

describe.if(hasNativeTestRenderer)('App', () => {
  test('paints the empty shell with the wordmark', () => {
    const { render, renderer, unmount } = createTestRoot({ width: 1280, height: 800 });
    try {
      render(<App />);
      renderer.flush();
      expect(renderer.getAllText()).toContain('ply');
      const shot = process.env.PLY_SCREENSHOT;
      if (shot) renderer.captureScreenshot(shot);
    } finally {
      unmount();
    }
  });
});
