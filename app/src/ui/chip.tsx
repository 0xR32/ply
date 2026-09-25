import { motion } from '@gpuix/react';
import { useState } from 'react';
import { useChrome } from '../theme/chrome';
import { Text } from './text';

/** A round status dot; `pulse` adds the running ring that grows and fades every 1.8 s (off under reduced motion). */
export function Dot({
  color,
  size,
  pulse = false,
}: {
  color: string;
  size: number;
  pulse?: boolean;
}) {
  const { z, reducedMotion } = useChrome();
  const s = z(size);
  return (
    <div style={{ position: 'relative', width: s, height: s, flexShrink: 0 }}>
      {pulse && !reducedMotion ? <PulseRing color={color} size={s} /> : null}
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

function PulseRing({ color, size }: { color: string; size: number }) {
  const [cycle, setCycle] = useState(0);
  const { z } = useChrome();
  const grown = size + z(12);
  const offset = -(grown - size) / 2;
  return (
    <motion.div
      key={cycle}
      initial={{
        width: size,
        height: size,
        top: 0,
        left: 0,
        opacity: 0.55,
        borderRadius: size / 2,
      }}
      animate={{
        width: grown,
        height: grown,
        top: offset,
        left: offset,
        opacity: 0,
        borderRadius: grown / 2,
      }}
      transition={{ duration: 1.8, ease: 'easeOut' }}
      onMotionComplete={() => setCycle((c) => c + 1)}
      style={{ position: 'absolute', backgroundColor: color, pointerEvents: 'none' }}
    />
  );
}

/** Props of `StatusChip`: the label and its colour pair from the status tone. */
export interface StatusChipProps {
  label: string;
  color: string;
  background: string;
  pulse?: boolean;
  testId?: string;
}

/** The pill at the right of a pane header: dot plus status text. */
export function StatusChip({ label, color, background, pulse = false, testId }: StatusChipProps) {
  const { z } = useChrome();
  return (
    <div
      testId={testId}
      style={{
        height: z(22),
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: z(7),
        paddingLeft: z(8),
        paddingRight: z(9),
        borderRadius: z(11),
        backgroundColor: background,
      }}
    >
      <Dot color={color} size={6} pulse={pulse} />
      <Text color={color} variant="small">
        {label}
      </Text>
    </div>
  );
}
