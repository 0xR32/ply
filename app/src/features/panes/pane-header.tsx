import { useEffect, useState } from 'react';
import type { PaneState } from '../../state/reducer';
import { isDone, paneTitle, type StatusTone, statusView } from '../../state/selectors';
import { useAppSelector } from '../../state/store';
import { type ChromeTheme, useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { StatusChip } from '../../ui/chip';
import { Icon } from '../../ui/icons';
import { Kbd } from '../../ui/kbd';
import { Text } from '../../ui/text';

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

function useNowSeconds(ticking: boolean): number {
  const [now, setNow] = useState(nowSeconds);
  useEffect(() => {
    if (!ticking) return;
    setNow(nowSeconds());
    const timer = setInterval(() => setNow(nowSeconds()), 1000);
    return () => clearInterval(timer);
  }, [ticking]);
  return now;
}

function toneColours(tone: StatusTone, c: ChromeTheme): { color: string; background: string } {
  switch (tone) {
    case 'running':
      return { color: c.accent.base, background: c.accent.a10 };
    case 'waiting':
      return { color: tokens.amber, background: tokens.amberA[12] };
    case 'done':
      return { color: tokens.mint, background: tokens.mintA10 };
    case 'failed':
      return { color: tokens.red, background: tokens.redA12 };
    case 'idle':
      return { color: tokens.textSoft, background: tokens.white[5] };
    case 'starting':
    case 'lost':
      return { color: tokens.text2, background: tokens.white[5] };
    case 'shell':
    case 'exited':
      return { color: tokens.text3, background: tokens.white[4] };
  }
}

/** Props of `PaneHeader`; `position` is the 1-based place in the tab (main pane first). */
export interface PaneHeaderProps {
  pane: PaneState;
  position: number;
  focused: boolean;
  onActivate: () => void;
}

/** The 42 px pane header: position key, title, branch, plan progress, CLI and model, status chip. */
export function PaneHeader({ pane, position, focused, onActivate }: PaneHeaderProps) {
  const chrome = useChrome();
  const { z, accent } = chrome;
  const shellName = useAppSelector((s) => s.env.shellName);
  const ticking = pane.status === 'running' && pane.statusSince !== undefined;
  const view = statusView(pane, shellName, useNowSeconds(ticking));
  const waiting = view.tone === 'waiting';
  const agent = pane.cli !== 'shell';
  const progress = agent ? pane.progress : undefined;
  const colours = toneColours(view.tone, chrome);
  const branch = [pane.branch, pane.worktree_seen ? `worktree ${pane.worktree_seen}` : undefined]
    .filter(Boolean)
    .join(' · ');
  const barColour = waiting ? tokens.amber : isDone(pane) ? tokens.mint : accent.base;
  return (
    <div
      style={{
        height: z(tokens.layout.paneHeaderHeight),
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: z(10),
        paddingLeft: z(8),
        paddingRight: z(10),
      }}
    >
      <div
        testId={`pane-${pane.id}-focus`}
        onClick={onActivate}
        style={{
          height: z(tokens.layout.paneHeaderHeight),
          flexGrow: 1,
          minWidth: 0,
          display: 'flex',
          alignItems: 'center',
          gap: z(10),
          cursor: 'pointer',
        }}
      >
        <Kbd
          label={String(position)}
          height={20}
          minWidth={20}
          paddingX={4}
          radius={6}
          weight={500}
          color={focused ? accent.base : waiting ? tokens.amber : tokens.text2}
          background={focused ? accent.a14 : tokens.white[3]}
          ring={focused ? accent.a45 : tokens.white[10]}
        />
        <Text color={tokens.text} weight={500} ellipsis testId={`pane-${pane.id}-title`}>
          {paneTitle(pane)}
        </Text>
        {branch ? (
          <div
            style={{
              flexShrink: 4,
              minWidth: z(72),
              display: 'flex',
              alignItems: 'center',
              gap: z(5),
            }}
          >
            <Icon name="branch" size={12} color={tokens.hint} />
            <Text color={tokens.hint} variant="label" mono ellipsis>
              {branch}
            </Text>
          </div>
        ) : null}
      </div>
      {progress && progress.total > 0 ? (
        <>
          <div
            style={{
              width: z(56),
              height: z(4),
              flexShrink: 0,
              borderRadius: z(2),
              backgroundColor: tokens.white[7],
              overflow: 'hidden',
            }}
          >
            <div
              style={{
                height: z(4),
                width: `${Math.round((100 * Math.min(progress.done, progress.total)) / progress.total)}%`,
                borderRadius: z(2),
                backgroundColor: barColour,
              }}
            />
          </div>
          <Text color={tokens.hint} variant="label" mono testId={`pane-${pane.id}-progress`}>
            {`${progress.done}/${progress.total}`}
          </Text>
        </>
      ) : null}
      {agent ? (
        <div
          style={{
            height: z(22),
            flexShrink: 0,
            display: 'flex',
            alignItems: 'center',
            gap: z(6),
            paddingLeft: z(8),
            paddingRight: z(8),
            borderRadius: z(6),
            backgroundColor: tokens.white[4],
          }}
        >
          <Text color={tokens.text} variant="label" mono>
            {pane.cli}
          </Text>
          {pane.model_seen ? (
            <Text color={tokens.text2} variant="label" mono testId={`pane-${pane.id}-model`}>
              {pane.model_seen}
            </Text>
          ) : null}
        </div>
      ) : null}
      <StatusChip
        testId={`pane-${pane.id}-status`}
        label={view.label}
        color={colours.color}
        background={colours.background}
        pulse={view.pulse}
      />
    </div>
  );
}
