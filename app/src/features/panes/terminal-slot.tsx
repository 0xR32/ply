import type { TerminalTheme } from '../../state/actions';
import { TerminalView } from './terminal-view';

/** The pane body's contract (C6) plus focus reporting; TerminalView takes these props unchanged. */
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
  /** What the body shows until the pane's first screen arrives from plyd. */
  placeholder: string;
}

/** Where a pane's terminal mounts: only visible panes render a slot, so only they stay attached (R-R20). */
export function TerminalSlot(props: TerminalSlotProps) {
  return <TerminalView {...props} />;
}
