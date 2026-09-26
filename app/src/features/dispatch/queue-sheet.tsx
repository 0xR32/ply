import type { EventPayload } from '@gpuix/react';
import { type ReactNode, useMemo, useState } from 'react';
import type { Task } from '../../state/actions';
import type { AppState, PaneState } from '../../state/reducer';
import {
  abbreviateHome,
  panePlace,
  selectActiveTask,
  selectFinishedTasks,
  selectPaneQueue,
  selectPoolTasks,
  selectQueueCounts,
  statusView,
  type TaskTone,
  taskView,
} from '../../state/selectors';
import { useAppSelector, useDispatch } from '../../state/store';
import { type ChromeTheme, useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Button } from '../../ui/button';
import { Icon, type IconName } from '../../ui/icons';
import { Kbd } from '../../ui/kbd';
import { Card, CardBar } from '../../ui/overlay-card';
import { Text } from '../../ui/text';

const selectState = (s: AppState) => s;

/** One group of the sheet: a pane with its typed and queued tasks, or the pool. */
interface Group {
  key: string;
  pane?: PaneState;
  tasks: Task[];
}

function toneColour(tone: TaskTone, c: ChromeTheme): string {
  switch (tone) {
    case 'running':
    case 'sending':
    case 'typing':
      return c.accent.base;
    case 'waiting':
      return tokens.amber;
    case 'ended':
      return tokens.mint;
    case 'failed':
      return tokens.red;
    case 'queued':
    case 'held':
      return tokens.text2;
    case 'cancelled':
      return tokens.text3;
  }
}

/** The ⌘⇧E task queue (Ruling R60), an overlay: every pane's queue, the pool and the history; ↑↓ ⌥↑↓ ⌫ p ⏎ esc. */
export function QueueSheet() {
  const dispatch = useDispatch();
  const chrome = useChrome();
  const { z, accent } = chrome;
  const state = useAppSelector(selectState);
  const [selected, setSelected] = useState<number | null>(null);
  const [expanded, setExpanded] = useState(false);
  const counts = selectQueueCounts(state);
  const groups = useMemo((): Group[] => {
    const out: Group[] = [];
    for (const tab of state.tabs) {
      for (const id of tab.pane_ids) {
        const pane = state.panes[id];
        if (!pane) continue;
        const active = selectActiveTask(state, id);
        const tasks = [...(active ? [active] : []), ...selectPaneQueue(state, id)];
        if (tasks.length > 0) out.push({ key: `pane-${id}`, pane, tasks });
      }
    }
    const pool = selectPoolTasks(state);
    if (pool.length > 0) out.push({ key: 'pool', tasks: pool });
    return out;
  }, [state]);
  const finished = selectFinishedTasks(state);
  const rows: Task[] = [...groups.flatMap((g) => g.tasks), ...(expanded ? finished : [])];
  const current = rows.find((t) => t.id === selected);

  const close = () => dispatch({ type: 'overlay/close' });
  const move = (task: Task, delta: number) => {
    if (task.state !== 'queued') return;
    dispatch({ type: 'task/move', taskId: task.id, position: Math.max(0, task.position + delta) });
  };
  const cancel = (task: Task) => {
    if (task.state === 'queued') dispatch({ type: 'task/cancel', taskId: task.id });
  };
  const togglePause = (paneId: number) =>
    dispatch({ type: 'queue/pause', paneId, paused: !state.tasks.queues[paneId]?.paused });
  const goTo = (task: Task | undefined) => {
    if (task?.pane_id === undefined || !state.panes[task.pane_id]) return;
    close();
    dispatch({ type: 'pane/focus', paneId: task.pane_id });
  };
  const onKeyDown = (event: EventPayload) => {
    const m = event.modifiers;
    if (m?.cmd || m?.ctrl) return;
    const at = current ? rows.indexOf(current) : -1;
    switch (event.key) {
      case 'escape':
        close();
        return;
      case 'up':
      case 'down': {
        const delta = event.key === 'down' ? 1 : -1;
        if (m?.alt) {
          if (current) move(current, delta);
          return;
        }
        const next = rows[Math.max(0, Math.min(rows.length - 1, at + delta))];
        if (next) setSelected(next.id);
        return;
      }
      case 'backspace':
      case 'delete':
        if (current) cancel(current);
        return;
      case 'p':
        if (current?.pane_id !== undefined && current.state !== 'ended')
          togglePause(current.pane_id);
        return;
      case 'enter':
        goTo(current);
        return;
      default:
        return;
    }
  };

  const iconButton = (testId: string, name: IconName, label: string, onClick: () => void) => (
    <div
      testId={testId}
      role="button"
      aria-label={label}
      onClick={onClick}
      style={{
        width: z(24),
        height: z(24),
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        borderRadius: z(6),
        cursor: 'pointer',
        hover: { backgroundColor: tokens.white[6] },
      }}
    >
      <Icon name={name} size={12} color={tokens.text2} />
    </div>
  );
  const taskRow = (task: Task, pane: PaneState | undefined): ReactNode => {
    const queue = task.pane_id === undefined ? undefined : state.tasks.queues[task.pane_id];
    const view = taskView(task, pane, queue);
    const colour = toneColour(view.tone, chrome);
    const on = task.id === selected;
    const filled = !['queued', 'held', 'typing', 'cancelled'].includes(view.tone);
    const finishedRow = ['ended', 'failed', 'cancelled'].includes(view.tone);
    return (
      <div
        key={task.id}
        testId={`queue-task-${task.id}`}
        style={{
          height: z(34),
          flexShrink: 0,
          display: 'flex',
          alignItems: 'center',
          gap: z(6),
          paddingRight: z(6),
          borderRadius: z(8),
          ...(on ? { backgroundColor: accent.a10, borderWidth: 1, borderColor: accent.a28 } : {}),
        }}
      >
        <div
          onClick={() => setSelected(task.id)}
          style={{
            height: z(34),
            flexGrow: 1,
            minWidth: 0,
            display: 'flex',
            alignItems: 'center',
            gap: z(10),
            paddingLeft: z(14),
            cursor: 'pointer',
          }}
        >
          <div
            style={{
              width: z(7),
              height: z(7),
              flexShrink: 0,
              borderRadius: z(4),
              ...(filled ? { backgroundColor: colour } : { borderWidth: 1.5, borderColor: colour }),
            }}
          />
          <Text
            color={finishedRow ? tokens.textSoft : tokens.text}
            variant="small"
            mono
            ellipsis
            decoration={view.tone === 'cancelled' ? 'line-through' : undefined}
          >
            {task.text}
          </Text>
          <div style={{ flexGrow: 1 }} />
          <Text
            color={view.tone === 'queued' || view.tone === 'held' ? tokens.text3 : colour}
            variant="small"
            weight={400}
          >
            {view.label}
          </Text>
        </div>
        {task.state === 'queued' ? (
          <div style={{ display: 'flex', alignItems: 'center', gap: z(2) }}>
            {iconButton(`queue-task-${task.id}-up`, 'up', 'Move up', () => move(task, -1))}
            {iconButton(`queue-task-${task.id}-down`, 'down', 'Move down', () => move(task, 1))}
            {iconButton(`queue-task-${task.id}-cancel`, 'close', 'Cancel this task', () =>
              cancel(task),
            )}
          </div>
        ) : null}
      </div>
    );
  };

  const body: ReactNode[] = [];
  for (const group of groups) {
    const pane = group.pane;
    const place = pane ? panePlace(state, pane.id) : null;
    const status = pane ? statusView(pane, state.env.shellName) : null;
    const paused = pane ? state.tasks.queues[pane.id]?.paused : undefined;
    const pool = group.tasks[0]?.pool;
    body.push(
      <div
        key={group.key}
        testId={pane ? `queue-group-${pane.id}` : 'queue-group-pool'}
        style={{
          height: z(38),
          flexShrink: 0,
          display: 'flex',
          alignItems: 'center',
          gap: z(9),
          marginTop: z(6),
          paddingLeft: z(8),
          paddingRight: z(8),
        }}
      >
        {pane ? (
          <Kbd
            label={String(place?.pane ?? pane.id)}
            height={20}
            minWidth={20}
            paddingX={4}
            radius={6}
            color={tokens.text2}
          />
        ) : (
          <Icon name="queue" size={14} color={tokens.text3} />
        )}
        <Text color={tokens.text} weight={600}>
          {pane
            ? (pane.project ?? pane.cwd.split('/').filter(Boolean).at(-1) ?? pane.cwd)
            : 'Next free pane'}
        </Text>
        <Text color={tokens.text2} variant="label" mono>
          {pane ? pane.cli : (pool?.cli ?? '')}
        </Text>
        <Text color={tokens.hint} variant="label" mono ellipsis>
          {pane ? (pane.branch ?? '') : abbreviateHome(pool?.cwd ?? '', state.env.home)}
        </Text>
        <div style={{ flexGrow: 1 }} />
        {status ? (
          <Text
            color={
              status.tone === 'waiting'
                ? tokens.amber
                : status.tone === 'running'
                  ? accent.base
                  : tokens.textSoft
            }
            variant="small"
          >
            {status.label}
          </Text>
        ) : null}
        {pane ? (
          <Button
            testId={`queue-pause-${pane.id}`}
            label={`${paused ? 'Resume' : 'Pause'} the queue of pane ${place?.pane ?? pane.id}`}
            onClick={() => togglePause(pane.id)}
            height={24}
            paddingLeft={7}
            paddingRight={9}
            gap={6}
            radius={6}
          >
            <Icon
              name={paused ? 'play' : 'pause'}
              size={10}
              color={paused ? accent.base : tokens.text2}
            />
            <Text color={paused ? accent.base : tokens.text2} variant="small">
              {paused ? 'Resume' : 'Pause'}
            </Text>
          </Button>
        ) : null}
      </div>,
    );
    for (const task of group.tasks) body.push(taskRow(task, pane));
  }
  if (finished.length > 0) {
    const tally = (['ended', 'failed', 'cancelled'] as const)
      .map((s) => [s, finished.filter((t) => t.state === s).length] as const)
      .filter(([, n]) => n > 0)
      .map(([s, n]) => `${n} ${s === 'failed' ? 'not submitted' : s}`)
      .join(' · ');
    body.push(
      <div
        key="finished"
        testId="queue-finished"
        onClick={() => setExpanded((e) => !e)}
        style={{
          height: z(36),
          flexShrink: 0,
          display: 'flex',
          alignItems: 'center',
          gap: z(8),
          marginTop: z(10),
          paddingLeft: z(8),
          borderTopWidth: 1,
          borderColor: tokens.white[6],
          cursor: 'pointer',
        }}
      >
        <Icon name={expanded ? 'up' : 'down'} size={12} color={tokens.text3} />
        <Text color={tokens.text3} variant="small">
          Finished
        </Text>
        <Text color={tokens.hint} variant="label" mono>
          {tally}
        </Text>
      </div>,
    );
    if (expanded) {
      for (const task of finished) {
        body.push(
          taskRow(task, task.pane_id === undefined ? undefined : state.panes[task.pane_id]),
        );
      }
    }
  }

  return (
    <Card width={tokens.layout.queueWidth} height={tokens.layout.queueHeight} testId="queue">
      <div
        onKeyDown={onKeyDown}
        tabIndex={0}
        autoFocus
        style={{ flexGrow: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}
      >
        <CardBar height={60} edge="top" paddingLeft={20} paddingRight={14} gap={10}>
          <div style={{ flexGrow: 1, display: 'flex', flexDirection: 'column', gap: z(2) }}>
            <Text color={tokens.text} variant="title">
              Task queue
            </Text>
            <Text color={tokens.text3} variant="small" weight={400} testId="queue-subtitle">
              {`${counts.queued} queued · ${counts.running} running · ${counts.needsYou} needs you`}
            </Text>
          </div>
          <Button
            testId="queue-dispatch"
            onClick={() => dispatch({ type: 'command', id: 'task.dispatch' })}
            height={30}
            paddingLeft={10}
            paddingRight={6}
          >
            <Icon name="plus" size={13} color={tokens.text} />
            <Text color={tokens.text} weight={500}>
              Dispatch
            </Text>
            <Kbd label="⌘E" color={tokens.text2} background={tokens.white[6]} ring={null} />
          </Button>
          <div testId="queue-close" onClick={close} style={{ cursor: 'pointer' }}>
            <Kbd
              label="esc"
              height={22}
              paddingX={7}
              radius={6}
              color={tokens.text2}
              background={tokens.white[5]}
              ring={tokens.white[10]}
            />
          </div>
        </CardBar>
        <div
          style={{
            flexGrow: 1,
            minHeight: 0,
            display: 'flex',
            flexDirection: 'column',
            gap: z(2),
            paddingTop: z(6),
            paddingBottom: z(10),
            paddingLeft: z(10),
            paddingRight: z(10),
            overflowY: 'scroll',
          }}
        >
          {body.length > 0 ? (
            body
          ) : (
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                gap: z(4),
                paddingTop: z(18),
                paddingLeft: z(10),
              }}
            >
              <Text color={tokens.textSoft}>Nothing queued</Text>
              <Text color={tokens.text3} variant="small" weight={400}>
                ⌘E sends a skill or a prompt to a pane, typed when it is your turn there.
              </Text>
            </div>
          )}
        </div>
        <CardBar height={40} edge="bottom" paddingLeft={20} paddingRight={20} gap={16}>
          {[
            ['↑↓', 'select'],
            ['⌥↑↓', 'reorder'],
            ['⌫', 'cancel'],
            ['p', 'pause pane'],
            ['⏎', 'go to pane'],
          ].map(([glyph, label]) => (
            <div key={label} style={{ display: 'flex', alignItems: 'center', gap: z(6) }}>
              <Text color={tokens.textSoft} variant="label" mono>
                {glyph ?? ''}
              </Text>
              <Text color={tokens.text3} variant="small" weight={400}>
                {label ?? ''}
              </Text>
            </div>
          ))}
        </CardBar>
      </div>
    </Card>
  );
}
