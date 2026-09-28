import { type PublicInstance, useGpuix } from '@gpuix/react';
import { type RefObject, useEffect, useRef, useState } from 'react';
import type { AppState, PaneState } from '../../state/reducer';
import {
  isDone,
  type StatusTone,
  selectActiveTask,
  selectPaneQueue,
  statusView,
} from '../../state/selectors';
import { useAppSelector, useDispatch } from '../../state/store';
import { type ChromeTheme, useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { StatusChip } from '../../ui/chip';
import { Icon } from '../../ui/icons';
import { Kbd } from '../../ui/kbd';
import { Text } from '../../ui/text';

/** Which optional header items fit; the least useful goes first as the pane narrows or the text grows. */
export interface HeaderFit {
  progressNumbers: boolean;
  model: boolean;
  progressBar: boolean;
  branch: boolean;
  queue: boolean;
  queueWords: boolean;
  cli: boolean;
}

const FULL_FIT: HeaderFit = {
  progressNumbers: true,
  model: true,
  progressBar: true,
  branch: true,
  queue: true,
  queueWords: true,
  cli: true,
};

/** The fit of a header `width` px wide at chrome `scale`; thresholds are unscaled px, since every item grows with the font. */
export function headerFit(width: number | null, scale: number): HeaderFit {
  if (width === null) return FULL_FIT;
  const w = width / scale;
  return {
    progressNumbers: w >= 480,
    model: w >= 400,
    progressBar: w >= 330,
    branch: w >= 290,
    queue: w >= 260,
    queueWords: w >= 520,
    cli: w >= 240,
  };
}

const MEASURE_MS = 250;

function sameFit(a: HeaderFit, b: HeaderFit): boolean {
  return (Object.keys(a) as (keyof HeaderFit)[]).every((k) => a[k] === b[k]);
}

/** The header's fit from its own box, read through the renderer the way the terminal view reads its size. */
function useHeaderFit(ref: RefObject<PublicInstance | null>, scale: number): HeaderFit {
  const { renderer } = useGpuix();
  const [fit, setFit] = useState<HeaderFit>(FULL_FIT);
  useEffect(() => {
    if (!renderer?.getElementBounds) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const measure = () => {
      const id = ref.current?.id;
      const b = id === undefined ? null : renderer.getElementBounds?.(id);
      if (b) {
        const next = headerFit(b.width, scale);
        setFit((old) => (sameFit(old, next) ? old : next));
      }
      timer = setTimeout(measure, b ? MEASURE_MS : 16);
    };
    measure();
    return () => {
      if (timer) clearTimeout(timer);
    };
  }, [renderer, ref, scale]);
  return fit;
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

/** Props of `PaneHeader`; `position` is the 1-based place in the tab, the grid's order. */
export interface PaneHeaderProps {
  pane: PaneState;
  position: number;
  focused: boolean;
  onActivate: () => void;
}

/** What the header badge says about a pane's task queue: the queued count, held while paused, or the task being typed. */
function queueBadge(state: AppState, paneId: number): string | null {
  const active = selectActiveTask(state, paneId);
  if (active?.state === 'sent') return 'sending';
  const queued = selectPaneQueue(state, paneId).length;
  if (queued === 0) return null;
  return `${queued} ${state.tasks.queues[paneId]?.paused ? 'held' : 'queued'}`;
}

/** The header's queue badge; a click opens the task queue (⌘⇧E). */
function QueueBadge({ paneId, words }: { paneId: number; words: boolean }) {
  const dispatch = useDispatch();
  const { z, accent } = useChrome();
  const full = useAppSelector((s) => queueBadge(s, paneId));
  if (!full) return null;
  const sending = full === 'sending';
  const held = full.endsWith('held');
  const label = words || sending ? full : (full.split(' ')[0] ?? full);
  return (
    <div
      testId={`pane-${paneId}-queue`}
      aria-label={full}
      role="button"
      onClick={() => dispatch({ type: 'command', id: 'task.queue' })}
      style={{
        height: z(22),
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: z(5),
        paddingLeft: z(7),
        paddingRight: z(8),
        borderRadius: z(6),
        backgroundColor: sending ? accent.a10 : tokens.white[4],
        borderWidth: 1,
        borderColor: sending ? accent.a28 : tokens.white[8],
        cursor: 'pointer',
      }}
    >
      <Icon
        name={held ? 'pause' : 'queue'}
        size={held ? 10 : 11}
        color={sending ? accent.base : held ? tokens.text3 : tokens.textSoft}
      />
      <Text
        color={sending ? accent.base : held ? tokens.text3 : tokens.textSoft}
        variant="label"
        mono
      >
        {label}
      </Text>
    </div>
  );
}

/** The last component of `path`, the project shown before plyd has asked git; `undefined` for `/`. */
export function folderName(path: string): string | undefined {
  return path.split('/').filter(Boolean).at(-1);
}

/** The 42 px pane header: position key, project, a bell mark until the pane is looked at, branch, plan progress, the task queue badge, CLI and model, status chip; narrow, it drops the progress numbers, the model, the bar, the branch, the badge and the CLI in that order and clips rather than overlaps. */
export function PaneHeader({ pane, position, focused, onActivate }: PaneHeaderProps) {
  const chrome = useChrome();
  const { z, accent } = chrome;
  const ref = useRef<PublicInstance>(null);
  const fit = useHeaderFit(ref, chrome.scale);
  const shellName = useAppSelector((s) => s.env.shellName);
  const view = statusView(pane, shellName);
  const waiting = view.tone === 'waiting';
  const agent = pane.cli !== 'shell';
  const progress = agent ? pane.progress : undefined;
  const colours = toneColours(view.tone, chrome);
  const worktree = pane.worktree_seen ?? pane.git_worktree;
  const branch = [pane.branch, worktree ? `worktree ${worktree}` : undefined]
    .filter(Boolean)
    .join(' · ');
  const project = pane.project ?? folderName(pane.cwd);
  const barColour = waiting ? tokens.amber : isDone(pane) ? tokens.mint : accent.base;
  return (
    <div
      ref={ref}
      testId={`pane-${pane.id}-header`}
      style={{
        height: z(tokens.layout.paneHeaderHeight),
        flexShrink: 0,
        minWidth: 0,
        overflow: 'hidden',
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
          overflow: 'hidden',
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
        {project ? (
          <div style={{ flexShrink: 0, maxWidth: z(200), minWidth: 0, display: 'flex' }}>
            <Text color={tokens.text} weight={600} ellipsis testId={`pane-${pane.id}-project`}>
              {project}
            </Text>
          </div>
        ) : null}
        {pane.bell ? (
          <div testId={`pane-${pane.id}-bell`} style={{ flexShrink: 0, display: 'flex' }}>
            <Icon name="bell" size={12} color={tokens.amber} />
          </div>
        ) : null}
        {branch && fit.branch ? (
          <div
            testId={`pane-${pane.id}-branch`}
            style={{
              flexShrink: 4,
              minWidth: 0,
              overflow: 'hidden',
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
      {progress && progress.total > 0 && fit.progressBar ? (
        <>
          <div
            testId={`pane-${pane.id}-progress-bar`}
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
          {fit.progressNumbers ? (
            <Text color={tokens.hint} variant="label" mono testId={`pane-${pane.id}-progress`}>
              {`${progress.done}/${progress.total}`}
            </Text>
          ) : null}
        </>
      ) : null}
      {agent && fit.queue ? <QueueBadge paneId={pane.id} words={fit.queueWords} /> : null}
      {agent && fit.cli ? (
        <div
          testId={`pane-${pane.id}-cli`}
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
          {pane.model_seen && fit.model ? (
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
      />
    </div>
  );
}
