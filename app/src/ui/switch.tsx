import type { PublicInstance } from '@gpuix/react';
import type { Ref } from 'react';
import { useChrome } from '../theme/chrome';
import { tokens } from '../theme/tokens';

/** Props of `Switch`; `focused` draws the keyboard focus ring the owning form tracks. */
export interface SwitchProps {
  on: boolean;
  onToggle: () => void;
  label: string;
  focused?: boolean;
  testId?: string;
  innerRef?: Ref<PublicInstance>;
  onFocus?: () => void;
  onBlur?: () => void;
}

/** An on/off switch: accent track with the knob right when on, a white-wash track with the knob left when off. */
export function Switch({
  on,
  onToggle,
  label,
  focused = false,
  testId,
  innerRef,
  onFocus,
  onBlur,
}: SwitchProps) {
  const { z, accent } = useChrome();
  return (
    <div
      ref={innerRef}
      testId={testId}
      role="button"
      aria-label={label}
      aria-description={on ? 'on' : 'off'}
      tabIndex={0}
      onClick={onToggle}
      onFocus={onFocus}
      onBlur={onBlur}
      style={{
        position: 'relative',
        width: z(34),
        height: z(20),
        flexShrink: 0,
        borderRadius: z(10),
        backgroundColor: on ? accent.base : tokens.white[8],
        borderWidth: 1,
        borderColor: focused ? accent.a80 : on ? accent.base : tokens.white[14],
        cursor: 'pointer',
      }}
    >
      <div
        style={{
          position: 'absolute',
          top: z(2),
          left: on ? z(16) : z(2),
          width: z(14),
          height: z(14),
          borderRadius: z(7),
          backgroundColor: tokens.knob,
          pointerEvents: 'none',
        }}
      />
    </div>
  );
}
