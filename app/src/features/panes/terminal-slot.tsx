import { type PublicInstance, useGpuix } from '@gpuix/react';
import { useEffect, useRef } from 'react';
import type { TerminalTheme } from '../../state/actions';
import { tokens } from '../../theme/tokens';

/** The pane body's contract (C6) plus focus reporting; WP5's TerminalView takes these props unchanged. */
export interface TerminalSlotProps {
  paneId: number;
  /** The store's focused pane with no overlay open: the body takes GPUI focus so keys reach its pty. */
  focused: boolean;
  theme: TerminalTheme;
  fontFamily: string;
  fontSize: number;
  onTitle?: (title: string) => void;
  onBell?: () => void;
  onExit?: (code: number) => void;
  /** GPUI focus arrived here (a click), so the store's focus must follow. */
  onFocus?: () => void;
  /** What the placeholder shows until TerminalView mounts here. */
  placeholder: string;
}

/** Where TerminalView (WP5) mounts; until then a focusable placeholder that holds the pane's GPUI focus. */
export function TerminalSlot({
  paneId,
  focused,
  fontFamily,
  fontSize,
  onFocus,
  placeholder,
}: TerminalSlotProps) {
  const { renderer } = useGpuix();
  const ref = useRef<PublicInstance>(null);
  useEffect(() => {
    const id = ref.current?.id;
    if (focused && id !== undefined) renderer?.focusElement?.(id);
  }, [focused, renderer]);
  return (
    <div
      ref={ref}
      testId={`terminal-${paneId}`}
      tabIndex={-1}
      onFocus={onFocus}
      style={{
        flexGrow: 1,
        minHeight: 0,
        display: 'flex',
        flexDirection: 'column',
        justifyContent: 'flex-end',
      }}
    >
      <text
        style={{
          color: tokens.hint,
          fontFamily,
          fontSize,
          lineHeight: Math.round(
            (fontSize * tokens.type.terminal.lineHeight) / tokens.type.terminal.fontSize,
          ),
          whiteSpace: 'nowrap',
          overflow: 'hidden',
          textOverflow: 'ellipsis',
        }}
      >
        {placeholder}
      </text>
    </div>
  );
}
