import type { ReactNode } from 'react';
import { useChrome } from '../theme/chrome';
import { tokens } from '../theme/tokens';

/** The dimmed full-window layer an overlay card sits on; a click on the dimmed part calls `onClose`. */
export function Backdrop({ onClose, children }: { onClose: () => void; children: ReactNode }) {
  const { z } = useChrome();
  return (
    <div
      testId="overlay"
      style={{
        position: 'absolute',
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        display: 'flex',
        justifyContent: 'center',
        alignItems: 'flex-start',
        paddingTop: z(tokens.layout.overlayTop),
        pointerEvents: 'auto',
      }}
    >
      <div
        testId="overlay-backdrop"
        onClick={onClose}
        style={{
          position: 'absolute',
          top: 0,
          left: 0,
          right: 0,
          bottom: 0,
          backgroundColor: tokens.backdrop,
        }}
      />
      {children}
    </div>
  );
}

/** Props of `Card`; `width` and `height` are unscaled canvas pixels, `height` omitted means content height. */
export interface CardProps {
  width: number;
  height?: number;
  children: ReactNode;
  testId?: string;
}

/** An overlay surface (palette, forms): rounded, hairline ring, deep drop shadow, column layout. */
export function Card({ width, height, children, testId }: CardProps) {
  const { z } = useChrome();
  return (
    <div
      testId={testId}
      style={{
        position: 'relative',
        width: z(width),
        ...(height !== undefined ? { height: z(height) } : {}),
        display: 'flex',
        flexDirection: 'column',
        overflow: 'hidden',
        borderRadius: z(tokens.radius.overlay),
        backgroundColor: tokens.overlay,
        borderWidth: 1,
        borderColor: tokens.white[9],
        boxShadow: {
          offsetX: 0,
          offsetY: 30,
          blurRadius: 80,
          spreadRadius: -20,
          color: tokens.overlayShadow,
        },
      }}
    >
      {children}
    </div>
  );
}

/** A card's top or bottom bar, separated from the body by a hairline. */
export function CardBar({
  height,
  edge,
  children,
  paddingLeft,
  paddingRight,
  gap = 12,
  justify = 'flex-start',
}: {
  height: number;
  edge: 'top' | 'bottom';
  children: ReactNode;
  paddingLeft: number;
  paddingRight: number;
  gap?: number;
  justify?: 'flex-start' | 'space-between';
}) {
  const { z } = useChrome();
  return (
    <div
      style={{
        height: z(height),
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: justify,
        gap: z(gap),
        paddingLeft: z(paddingLeft),
        paddingRight: z(paddingRight),
        ...(edge === 'top'
          ? { borderBottomWidth: 1, borderColor: tokens.white[7] }
          : { borderTopWidth: 1, borderColor: tokens.white[7] }),
      }}
    >
      {children}
    </div>
  );
}
