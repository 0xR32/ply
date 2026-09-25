import { useChrome } from '../theme/chrome';
import { tokens } from '../theme/tokens';
import { Text } from './text';

/** Props of `Kbd`; colours default to the footer key style of the canvas. */
export interface KbdProps {
  label: string;
  height?: number;
  minWidth?: number;
  paddingX?: number;
  radius?: number;
  color?: string;
  background?: string;
  ring?: string | null;
  variant?: 'label' | 'key';
  weight?: number;
}

/** A key cap: a mono label in a small rounded box with an inset ring. */
export function Kbd({
  label,
  height = 20,
  minWidth,
  paddingX = 6,
  radius = tokens.radius.key,
  color = tokens.textSoft,
  background = tokens.white[4],
  ring = tokens.white[9],
  variant = 'label',
  weight,
}: KbdProps) {
  const { z } = useChrome();
  return (
    <div
      style={{
        height: z(height),
        ...(minWidth !== undefined ? { minWidth: z(minWidth) } : {}),
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        paddingLeft: z(paddingX),
        paddingRight: z(paddingX),
        borderRadius: z(radius),
        backgroundColor: background,
        ...(ring ? { borderWidth: 1, borderColor: ring } : {}),
      }}
    >
      <Text color={color} variant={variant} mono weight={weight}>
        {label}
      </Text>
    </div>
  );
}
