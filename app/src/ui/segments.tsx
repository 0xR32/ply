import type { PublicInstance } from '@gpuix/react';
import type { Ref } from 'react';
import { useChrome } from '../theme/chrome';
import { tokens } from '../theme/tokens';
import { Text } from './text';

/** One choice of a segmented control; `hint` is the digit shown before the label, `swatch` a colour dot. */
export interface SegmentItem<T extends string> {
  value: T;
  label: string;
  hint?: string;
  swatch?: string;
}

/** Props of `Segments`; the owning form handles ←→ (GPUIX key events reach it from the focused control). */
export interface SegmentsProps<T extends string> {
  items: readonly SegmentItem<T>[];
  value: T;
  onChange: (value: T) => void;
  focused?: boolean;
  testId?: string;
  innerRef?: Ref<PublicInstance>;
  autoFocus?: boolean;
  onFocus?: () => void;
  onBlur?: () => void;
}

/** A segmented control: equal-width segments, the chosen one raised with an accent ring. */
export function Segments<T extends string>({
  items,
  value,
  onChange,
  focused = false,
  testId,
  innerRef,
  autoFocus = false,
  onFocus,
  onBlur,
}: SegmentsProps<T>) {
  const { z, accent } = useChrome();
  return (
    <div
      ref={innerRef}
      testId={testId}
      tabIndex={0}
      autoFocus={autoFocus}
      onFocus={onFocus}
      onBlur={onBlur}
      style={{
        display: 'flex',
        gap: z(4),
        padding: z(3),
        borderRadius: z(10),
        backgroundColor: tokens.white[3],
        borderWidth: 1,
        borderColor: focused ? accent.a45 : tokens.white[7],
      }}
    >
      {items.map((item) => {
        const on = item.value === value;
        return (
          <div
            key={item.value}
            testId={testId ? `${testId}-${item.value}` : undefined}
            onClick={() => onChange(item.value)}
            style={{
              height: z(32),
              flexGrow: 1,
              flexBasis: 0,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              gap: z(8),
              paddingLeft: z(12),
              paddingRight: z(12),
              borderRadius: z(7),
              cursor: 'pointer',
              ...(on
                ? { backgroundColor: tokens.segmentActive, borderWidth: 1, borderColor: accent.a45 }
                : { hover: { backgroundColor: tokens.white[3] } }),
            }}
          >
            {item.swatch ? (
              <div
                style={{
                  width: z(8),
                  height: z(8),
                  borderRadius: z(4),
                  backgroundColor: item.swatch,
                  pointerEvents: 'none',
                }}
              />
            ) : null}
            {item.hint ? (
              <Text color={tokens.hint} variant="label" mono>
                {item.hint}
              </Text>
            ) : null}
            <Text color={on ? tokens.text : tokens.text2} weight={500}>
              {item.label}
            </Text>
          </div>
        );
      })}
    </div>
  );
}
