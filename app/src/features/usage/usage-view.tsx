import { useEffect, useState } from 'react';
import type { CliUsage, UsageWindow } from '../../state/actions';
import { useAppSelector } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Card } from '../../ui/overlay-card';
import { Text } from '../../ui/text';

/** How full a window is: `high` over 80 % (amber), `full` at or over 100 % (red). */
export type UsageTone = 'normal' | 'high' | 'full';

/** The tone of a window used `usedPercent`. */
export function usageTone(usedPercent: number): UsageTone {
  if (usedPercent >= 100) return 'full';
  return usedPercent > 80 ? 'high' : 'normal';
}

/** A span of `seconds` in words: `3 d 4 h`, `2 h 13 min`, `13 min`, `under a minute`. */
export function spanText(seconds: number): string {
  const minutes = Math.floor(Math.max(seconds, 0) / 60);
  if (minutes < 1) return 'under a minute';
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const mins = minutes % 60;
  if (days > 0) return hours > 0 ? `${days} d ${hours} h` : `${days} d`;
  if (hours > 0) return mins > 0 ? `${hours} h ${mins} min` : `${hours} h`;
  return `${mins} min`;
}

/** `resets in 2 h 13 min`, or `reset 5 min ago` once the window has started over; empty without a reset time. */
export function resetText(resetsAt: number | undefined, now: number): string {
  if (resetsAt === undefined) return '';
  return resetsAt > now
    ? `resets in ${spanText(resetsAt - now)}`
    : `reset ${spanText(now - resetsAt)} ago`;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

function clock(date: Date): string {
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
}

/** How old a record of `asOf` is: `updated just now`, `updated 12 min ago` within the hour, else `as of 14:02` (or with its date when older than a day). */
export function ageText(asOf: number, now: number): string {
  const age = now - asOf;
  if (age < 60) return 'updated just now';
  if (age < 3600) return `updated ${Math.floor(age / 60)} min ago`;
  const at = new Date(asOf * 1000);
  if (age < 86_400) return `as of ${clock(at)}`;
  return `as of ${at.getDate()} ${MONTHS[at.getMonth()]} ${clock(at)}`;
}

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

function WindowRow({ window, now }: { window: UsageWindow; now: number }) {
  const { z, accent } = useChrome();
  const tone = usageTone(window.used_percent);
  const past = window.resets_at !== undefined && window.resets_at <= now;
  const fill = past
    ? tokens.text3
    : tone === 'full'
      ? tokens.red
      : tone === 'high'
        ? tokens.amber
        : accent.base;
  const reset = resetText(window.resets_at, now);
  return (
    <div testId="usage-window" style={{ display: 'flex', flexDirection: 'column', gap: z(5) }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: z(10) }}>
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: z(8),
            flexGrow: 1,
            flexShrink: 1,
            minWidth: 0,
          }}
        >
          <Text color={tokens.text} variant="small">
            {window.label}
          </Text>
          {window.models.length > 0 ? (
            <Text color={tokens.text3} variant="caption" weight={400} ellipsis>
              {window.models.join(', ')}
            </Text>
          ) : null}
        </div>
        <Text color={tone === 'normal' || past ? tokens.text2 : fill} variant="small" mono>
          {`${Math.round(window.used_percent)} %`}
        </Text>
      </div>
      <div
        testId="usage-bar"
        style={{
          height: z(6),
          borderRadius: z(3),
          backgroundColor: tokens.white[7],
          overflow: 'hidden',
        }}
      >
        <div
          testId={`usage-fill-${past ? 'past' : tone}`}
          style={{
            width: `${Math.min(Math.max(window.used_percent, 0), 100)}%`,
            height: '100%',
            borderRadius: z(3),
            backgroundColor: fill,
          }}
        />
      </div>
      {reset ? (
        <Text color={tokens.hint} variant="caption" weight={400}>
          {reset}
        </Text>
      ) : null}
    </div>
  );
}

function Section({
  name,
  usage,
  state,
  now,
}: {
  name: string;
  usage: CliUsage | undefined;
  state: 'loaded' | 'reading' | 'failed';
  now: number;
}) {
  const { z } = useChrome();
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: z(12) }}>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <Text color={tokens.text} variant="small" weight={600}>
          {name}
        </Text>
        {usage ? (
          <Text color={tokens.text3} variant="caption" weight={400}>
            {ageText(usage.as_of, now)}
          </Text>
        ) : null}
      </div>
      {usage ? (
        usage.windows.map((w) => <WindowRow key={w.label} window={w} now={now} />)
      ) : (
        <Text color={tokens.text3} variant="small" weight={400}>
          {state === 'loaded'
            ? 'No usage recorded yet'
            : state === 'reading'
              ? 'Reading…'
              : 'Unavailable'}
        </Text>
      )}
    </div>
  );
}

/** The hold-⌘U card (Ruling R59): each CLI's usage windows as its own files last recorded them, shown only while ⌘U is held. */
export function UsageView() {
  const { z } = useChrome();
  const { shown, usage, error } = useAppSelector((s) => s.usage);
  const [, tick] = useState(0);
  useEffect(() => {
    if (!shown) return;
    const timer = setInterval(() => tick((n) => n + 1), 15_000);
    return () => clearInterval(timer);
  }, [shown]);
  if (!shown) return null;
  const now = nowSeconds();
  const state = usage !== null ? 'loaded' : error !== null ? 'failed' : 'reading';
  return (
    <div
      testId="usage-layer"
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
        pointerEvents: 'none',
      }}
    >
      <Card width={tokens.layout.usageWidth} testId="usage-view">
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: z(20),
            paddingTop: z(18),
            paddingBottom: z(18),
            paddingLeft: z(20),
            paddingRight: z(20),
          }}
        >
          <Section name="Claude Code" usage={usage?.claude} state={state} now={now} />
          <Section name="Codex" usage={usage?.codex} state={state} now={now} />
          {error ? (
            <Text color={tokens.red} variant="caption" weight={400} ellipsis>
              {error}
            </Text>
          ) : null}
        </div>
      </Card>
    </div>
  );
}
