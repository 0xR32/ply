import type { StyleDesc } from '@gpuix/react';
import { useChrome } from '../theme/chrome';
import type { tokens } from '../theme/tokens';

/** Props of `Text`; `color` is required because GPUIX text never inherits it. */
export interface TextProps {
  children: string | number;
  color: string;
  variant?: keyof typeof tokens.type;
  weight?: number;
  mono?: boolean;
  ellipsis?: boolean;
  decoration?: 'line-through' | 'underline';
  testId?: string;
}

/** One single-line run of chrome text at a scaled type-ramp size. */
export function Text({
  children,
  color,
  variant = 'body',
  weight,
  mono = false,
  ellipsis = false,
  decoration,
  testId,
}: TextProps) {
  const { type, fonts } = useChrome();
  const t = type[variant];
  const style: StyleDesc = {
    color,
    fontFamily: mono ? fonts.mono : fonts.ui,
    fontSize: t.fontSize,
    lineHeight: t.lineHeight,
    fontWeight: weight ?? t.fontWeight,
    whiteSpace: 'nowrap',
    flexShrink: ellipsis ? 1 : 0,
    ...(ellipsis ? { minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' as const } : {}),
    ...(decoration ? { textDecoration: decoration } : {}),
  };
  return (
    <text testId={testId} style={style}>
      {String(children)}
    </text>
  );
}
