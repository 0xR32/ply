import type { PublicInstance, StyleDesc } from '@gpuix/react';
import type { ReactNode, Ref } from 'react';
import { useChrome } from '../theme/chrome';
import { tokens } from '../theme/tokens';

/** Fill of a button: `primary` is the accent, `amber` answers a prompt, `danger` stops a process, `quiet` is bare. */
export type ButtonVariant = 'primary' | 'secondary' | 'amber' | 'danger' | 'quiet';

/** Props of `Button`; lengths are unscaled canvas pixels. */
export interface ButtonProps {
  children: ReactNode;
  onClick: () => void;
  variant?: ButtonVariant;
  height?: number;
  paddingLeft?: number;
  paddingRight?: number;
  gap?: number;
  radius?: number;
  width?: number;
  focusable?: boolean;
  focused?: boolean;
  testId?: string;
  label?: string;
  innerRef?: Ref<PublicInstance>;
  onFocus?: () => void;
  onBlur?: () => void;
}

/** A clickable box (GPUIX has no `<button>`): a `div` with `onClick`, a pointer cursor and a hover wash. */
export function Button({
  children,
  onClick,
  variant = 'secondary',
  height = 34,
  paddingLeft = 14,
  paddingRight = 14,
  gap = 8,
  radius = tokens.radius.control,
  width,
  focusable = false,
  focused = false,
  testId,
  label,
  innerRef,
  onFocus,
  onBlur,
}: ButtonProps) {
  const { z, accent } = useChrome();
  const look: Record<ButtonVariant, StyleDesc> = {
    primary: {
      backgroundColor: accent.base,
      boxShadow: { offsetX: 0, offsetY: 8, blurRadius: 24, spreadRadius: -10, color: accent.a80 },
      hover: { opacity: 0.92 },
    },
    secondary: {
      backgroundColor: tokens.white[5],
      borderWidth: 1,
      borderColor: tokens.white[10],
      hover: { backgroundColor: tokens.white[8] },
    },
    amber: { backgroundColor: tokens.amber, hover: { opacity: 0.9 } },
    danger: { backgroundColor: tokens.red, hover: { opacity: 0.9 } },
    quiet: { hover: { backgroundColor: tokens.white[4] } },
  };
  return (
    <div
      ref={innerRef}
      testId={testId}
      role="button"
      aria-label={label}
      tabIndex={focusable ? 0 : undefined}
      onClick={onClick}
      onFocus={onFocus}
      onBlur={onBlur}
      style={{
        height: z(height),
        ...(width !== undefined ? { width: z(width) } : {}),
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: width !== undefined ? 'center' : 'flex-start',
        gap: z(gap),
        paddingLeft: z(paddingLeft),
        paddingRight: z(paddingRight),
        borderRadius: z(radius),
        cursor: 'pointer',
        userSelect: 'none',
        ...look[variant],
        ...(focused ? { borderWidth: 1, borderColor: accent.a80 } : {}),
      }}
    >
      {children}
    </div>
  );
}
