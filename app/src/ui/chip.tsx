import { useChrome } from '../theme/chrome';
import { Text } from './text';

/** A round status dot. It never animates: any running GPUIX motion redraws the whole window every frame. */
export function Dot({ color, size }: { color: string; size: number }) {
  const { z } = useChrome();
  const s = z(size);
  return (
    <div style={{ position: 'relative', width: s, height: s, flexShrink: 0 }}>
      <div
        style={{
          position: 'absolute',
          top: 0,
          left: 0,
          width: s,
          height: s,
          borderRadius: s / 2,
          backgroundColor: color,
          pointerEvents: 'none',
        }}
      />
    </div>
  );
}

/** Props of `StatusChip`: the label and its colour pair from the status tone. */
export interface StatusChipProps {
  label: string;
  color: string;
  background: string;
  testId?: string;
}

/** The pill at the right of a pane header: dot plus status text, which ellipsizes last when the header runs out of room. */
export function StatusChip({ label, color, background, testId }: StatusChipProps) {
  const { z } = useChrome();
  return (
    <div
      testId={testId}
      style={{
        height: z(22),
        flexShrink: 1,
        minWidth: 0,
        overflow: 'hidden',
        display: 'flex',
        alignItems: 'center',
        gap: z(7),
        paddingLeft: z(8),
        paddingRight: z(9),
        borderRadius: z(11),
        backgroundColor: background,
      }}
    >
      <Dot color={color} size={6} />
      <Text color={color} variant="small" ellipsis>
        {label}
      </Text>
    </div>
  );
}
