import type { EventPayload, StyleDesc } from '@gpuix/react';
import { useEffect, useMemo, useState } from 'react';
import type { AgentCli, DispatchTarget, Skill } from '../../state/actions';
import type { AppState, PaneState } from '../../state/reducer';
import {
  abbreviateHome,
  expandHome,
  matchSkills,
  panePlace,
  selectPaneQueue,
  statusView,
} from '../../state/selectors';
import { useAppSelector, useDispatch } from '../../state/store';
import { escapedPath, keepDroppedFile } from '../../terminal/drop';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Button } from '../../ui/button';
import { useFocusFields } from '../../ui/focus-fields';
import { Icon } from '../../ui/icons';
import { Kbd } from '../../ui/kbd';
import { Card, CardBar } from '../../ui/overlay-card';
import { type SegmentItem, Segments } from '../../ui/segments';
import { Text } from '../../ui/text';
import { SkillList } from './skill-list';

type Target = DispatchTarget['kind'];
type Field = 'target' | 'pane' | 'cli' | 'dir' | 'search' | 'prompt' | 'cancel' | 'send';

const TARGETS: readonly SegmentItem<Target>[] = [
  { value: 'pane', label: 'This pane' },
  { value: 'pool', label: 'Next free pane' },
  { value: 'new', label: 'New pane' },
];

const CLIS: readonly SegmentItem<AgentCli>[] = [
  { value: 'claude', label: 'claude' },
  { value: 'codex', label: 'codex' },
];

const noTabs = (value: string | undefined) => (value ?? '').replaceAll('\t', '');
const selectState = (s: AppState) => s;

function ordinal(n: number): string {
  return n === 2 ? '2nd' : n === 3 ? '3rd' : `${n}th`;
}

/** The panes a task can go to, in tab order: agent panes whose process has not exited. */
function agentPanes(state: AppState): PaneState[] {
  return state.tabs
    .flatMap((t) => t.pane_ids)
    .map((id) => state.panes[id])
    .filter((p): p is PaneState => p !== undefined && p.cli !== 'shell' && p.status !== 'exited');
}

/** The ⌘E form (Ruling R60): send a skill or a prompt to a pane, the next free pane or a new pane; dropped files join the prompt as paths, ⌘⏎ queues, esc closes. */
export function DispatchForm({ paneId }: { paneId?: number }) {
  const dispatch = useDispatch();
  const { z, accent, fonts, type } = useChrome();
  const state = useAppSelector(selectState);
  const home = state.env.home;
  const panes = useMemo(() => agentPanes(state), [state]);
  const opened = paneId === undefined ? undefined : state.panes[paneId];
  const [target, setTarget] = useState<Target>(opened || panes.length > 0 ? 'pane' : 'pool');
  const [chosen, setChosen] = useState<number | undefined>(opened?.id ?? panes[0]?.id);
  const [cli, setCli] = useState<AgentCli>(opened?.cli === 'codex' ? 'codex' : 'claude');
  const [dir, setDir] = useState(
    abbreviateHome(opened?.cwd ?? state.workspace?.path ?? home, home),
  );
  const [query, setQuery] = useState('');
  const [picked, setPicked] = useState<string | null>(null);
  const [text, setText] = useState('');
  const [highlight, setHighlight] = useState(0);
  const [local, setLocal] = useState<{ text: string; refusal: string | null } | null>(null);
  const pane = chosen === undefined ? undefined : state.panes[chosen];
  const skillCli: AgentCli = target === 'pane' ? (pane?.cli === 'codex' ? 'codex' : 'claude') : cli;
  const skillCwd = target === 'pane' ? pane?.cwd : expandHome(dir, home);
  const skillKey = skillCwd?.startsWith('/') ? `${skillCli} ${skillCwd}` : null;
  useEffect(() => {
    if (skillKey === null || skillCwd === undefined) return;
    dispatch({ type: 'skills/query', cli: skillCli, cwd: skillCwd });
  }, [dispatch, skillKey, skillCli, skillCwd]);
  const skills = useMemo(
    () => (state.skills.key === skillKey ? matchSkills(state.skills.list, query) : []),
    [state.skills, skillKey, query],
  );
  const hi = skills.length === 0 ? -1 : Math.min(highlight, skills.length - 1);
  const pickedSkill =
    skills.find((s) => s.invocation === picked) ??
    state.skills.list.find((s) => s.invocation === picked);
  const order: Field[] = [
    'target',
    ...(target === 'pane' ? (['pane'] as const) : (['cli', 'dir'] as const)),
    'search',
    'prompt',
    'cancel',
    'send',
  ];
  const fields = useFocusFields<Field>(order, 'search');
  const focus = fields.focused;
  const tracked = (field: Field) => ({ innerRef: fields.ref(field), ...fields.track(field) });
  const pending = state.taskForm.pending;
  const refusal = state.taskForm.error;
  const setProblem = (text: string | null) => setLocal(text === null ? null : { text, refusal });
  const close = () => dispatch({ type: 'overlay/close' });

  const pick = (skill: Skill | undefined) => {
    if (!skill) return;
    const written = picked !== null && text.startsWith(picked) ? text.slice(picked.length) : text;
    setPicked(skill.invocation);
    setText(`${skill.invocation} ${written.trimStart()}`);
    setProblem(null);
    fields.focus('prompt');
  };
  const submit = () => {
    if (pending) return;
    const body = text.trimEnd();
    if (body.trim() === '') {
      setProblem('Write a prompt or pick a skill');
      return;
    }
    let where: DispatchTarget;
    if (target === 'pane') {
      if (!pane) {
        setProblem('There is no agent pane to send it to');
        return;
      }
      where = { kind: 'pane', paneId: pane.id };
    } else {
      const cwd = expandHome(dir, home);
      if (!cwd.startsWith('/')) {
        setProblem('The folder must be an absolute path or start with ~');
        return;
      }
      where = { kind: target, cli, cwd };
    }
    setProblem(null);
    const skill = picked && (body === picked || body.startsWith(`${picked} `)) ? picked : undefined;
    dispatch({ type: 'task/add', target: where, text: body, ...(skill ? { skill } : {}) });
  };
  const onFileDrop = (event: EventPayload) => {
    const paths = (event.paths ?? []).map((p) => escapedPath(keepDroppedFile(p)));
    if (paths.length === 0) return;
    // A line each: Claude Code reads an image only for a piece of the paste that ends in its path.
    setText((old) => `${old}${old === '' || old.endsWith('\n') ? '' : '\n'}${paths.join('\n')}`);
    setProblem(null);
    fields.focus('prompt');
  };
  const cycle = <T,>(items: readonly T[], current: T | undefined, delta: number): T | undefined => {
    if (items.length === 0) return undefined;
    const at = current === undefined ? -1 : items.indexOf(current);
    return items[(at + delta + items.length) % items.length];
  };
  const onKeyDown = (event: EventPayload) => {
    const m = event.modifiers;
    if (event.key === 'enter' && m?.cmd) {
      submit();
      return;
    }
    if (m?.cmd || m?.ctrl || m?.alt) return;
    const at = fields.current();
    switch (event.key) {
      case 'escape':
        close();
        return;
      case 'tab':
        fields.step(m?.shift ?? false);
        return;
      case 'up':
      case 'down':
        if (at === 'search' && skills.length > 0) {
          const n = skills.length;
          setHighlight((hi + (event.key === 'down' ? 1 : -1) + n) % n);
        }
        return;
      case 'left':
      case 'right': {
        const delta = event.key === 'right' ? 1 : -1;
        if (at === 'target')
          setTarget(
            cycle(
              TARGETS.map((t) => t.value),
              target,
              delta,
            ) ?? target,
          );
        else if (at === 'pane')
          setChosen(
            cycle(
              panes.map((p) => p.id),
              chosen,
              delta,
            ) ?? chosen,
          );
        else if (at === 'cli')
          setCli(
            cycle(
              CLIS.map((c) => c.value),
              cli,
              delta,
            ) ?? cli,
          );
        return;
      }
      case 'enter':
      case 'space':
        if (event.key === 'enter' && at === 'search') pick(skills[hi]);
        else if (at === 'cancel') close();
        else if (at === 'send') submit();
        return;
      default:
        return;
    }
  };

  const fieldBox = (focused: boolean, mono: boolean): StyleDesc => ({
    paddingLeft: z(12),
    paddingRight: z(12),
    borderRadius: z(8),
    backgroundColor: tokens.term,
    borderWidth: 1,
    borderColor: focused ? accent.a45 : tokens.white[9],
    color: tokens.text,
    fontFamily: mono ? fonts.mono : fonts.ui,
    fontSize: mono ? type.terminal.fontSize : type.body.fontSize,
  });
  const field = (focused: boolean, mono: boolean): StyleDesc => ({
    ...fieldBox(focused, mono),
    height: z(36),
  });
  const label = (value: string, extra?: string) => (
    <div style={{ display: 'flex', alignItems: 'center', gap: z(6) }}>
      <Text color={tokens.text2} variant="small">
        {value}
      </Text>
      {extra ? (
        <Text color={tokens.hint} variant="small" weight={400}>
          {extra}
        </Text>
      ) : null}
    </div>
  );

  let summary: string;
  if (target === 'pane') {
    if (!pane) summary = 'No agent pane is open';
    else {
      const queued = selectPaneQueue(state, pane.id).length;
      const place = panePlace(state, pane.id);
      const status = statusView(pane, state.env.shellName).label.toLowerCase();
      const when =
        queued === 0
          ? 'typed when it is your turn there'
          : `goes ${ordinal(queued + 1)} in its queue`;
      summary = `Pane ${place?.pane ?? pane.id} · ${pane.cli} · ${status} · ${when}`;
    }
  } else if (target === 'pool') {
    summary = `The first free ${cli} pane in ${dir} takes it`;
  } else {
    summary = `Opens a ${cli} pane with this as its first prompt`;
  }
  const error = local && local.refusal === refusal ? local.text : refusal;
  const verb = target === 'new' ? 'Open pane' : 'Queue';

  return (
    <Card
      width={tokens.layout.dispatchWidth}
      height={tokens.layout.dispatchHeight}
      testId="dispatch"
    >
      <div
        onKeyDown={onKeyDown}
        onFileDrop={onFileDrop}
        style={{ flexGrow: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}
      >
        <CardBar height={60} edge="top" paddingLeft={20} paddingRight={14} justify="space-between">
          <div style={{ display: 'flex', flexDirection: 'column', gap: z(2) }}>
            <Text color={tokens.text} variant="title">
              Dispatch a task
            </Text>
            <Text color={tokens.text3} variant="small" weight={400}>
              Typed into the pane exactly as written, when it is your turn.
            </Text>
          </div>
          <div
            testId="dispatch-close"
            onClick={close}
            onFileDrop={onFileDrop}
            style={{ cursor: 'pointer' }}
          >
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
            gap: z(14),
            paddingTop: z(16),
            paddingBottom: z(16),
            paddingLeft: z(20),
            paddingRight: z(20),
          }}
        >
          <div style={{ display: 'flex', flexDirection: 'column', gap: z(8) }}>
            {label('Send to')}
            <Segments
              testId="dispatch-target"
              items={TARGETS}
              value={target}
              onChange={setTarget}
              onFileDrop={onFileDrop}
              focused={focus === 'target'}
              {...tracked('target')}
            />
            {target === 'pane' ? (
              <div
                ref={fields.ref('pane')}
                tabIndex={0}
                {...fields.track('pane')}
                style={{
                  display: 'flex',
                  gap: z(6),
                  borderRadius: z(10),
                  ...(focus === 'pane' ? { borderWidth: 1, borderColor: accent.a45 } : {}),
                }}
              >
                {panes.length === 0 ? (
                  <Text color={tokens.text3} variant="small" weight={400}>
                    Open a Claude Code or Codex pane first, or send it to a new pane.
                  </Text>
                ) : (
                  panes.map((p) => (
                    <PaneChip
                      key={p.id}
                      pane={p}
                      chosen={p.id === chosen}
                      onPick={() => setChosen(p.id)}
                      onFileDrop={onFileDrop}
                    />
                  ))
                )}
              </div>
            ) : (
              <div style={{ display: 'flex', alignItems: 'center', gap: z(8) }}>
                <div style={{ width: z(180), flexShrink: 0 }}>
                  <Segments
                    testId="dispatch-cli"
                    items={CLIS}
                    value={cli}
                    onChange={setCli}
                    onFileDrop={onFileDrop}
                    focused={focus === 'cli'}
                    {...tracked('cli')}
                  />
                </div>
                <input
                  testId="dispatch-dir"
                  value={dir}
                  placeholder="~/code/project"
                  theme={{ caret: accent.base }}
                  onChange={(e) => setDir(noTabs(e.value))}
                  style={{ ...field(focus === 'dir', true), flexGrow: 1, minWidth: 0 }}
                  ref={fields.ref('dir')}
                  {...fields.track('dir')}
                />
              </div>
            )}
          </div>
          <div style={{ flexShrink: 0, display: 'flex', flexDirection: 'column', gap: z(8) }}>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
              {label('Skills', `on this machine, for ${skillCli}`)}
              <Text color={tokens.hint} variant="label" mono>
                {skills.length === 1 ? '1 skill' : `${skills.length} skills`}
              </Text>
            </div>
            <div
              style={{
                height: z(tokens.layout.dispatchSkillsHeight),
                display: 'flex',
                flexDirection: 'column',
                overflow: 'hidden',
                borderRadius: z(10),
                backgroundColor: tokens.term,
                borderWidth: 1,
                borderColor: focus === 'search' ? accent.a45 : tokens.white[8],
              }}
            >
              <div
                style={{
                  height: z(38),
                  flexShrink: 0,
                  display: 'flex',
                  alignItems: 'center',
                  gap: z(10),
                  paddingLeft: z(12),
                  paddingRight: z(12),
                  borderBottomWidth: 1,
                  borderColor: tokens.white[6],
                }}
              >
                <Icon name="search" size={14} color={tokens.hint} />
                <input
                  testId="dispatch-search"
                  autoFocus
                  value={query}
                  placeholder="review, plan, audit…"
                  theme={{ caret: accent.base }}
                  onChange={(e) => {
                    setQuery(noTabs(e.value));
                    setHighlight(0);
                  }}
                  onSubmit={() => pick(skills[hi])}
                  style={{
                    height: z(30),
                    flexGrow: 1,
                    minWidth: 0,
                    color: tokens.text,
                    fontFamily: fonts.ui,
                    fontSize: type.body.fontSize,
                  }}
                  ref={fields.ref('search')}
                  {...fields.track('search')}
                />
              </div>
              <SkillList
                skills={skills}
                picked={picked}
                highlight={focus === 'search' ? hi : -1}
                loading={state.skills.loading}
                error={state.skills.key === skillKey ? state.skills.error : null}
                onPick={pick}
                onHover={setHighlight}
                onFileDrop={onFileDrop}
              />
            </div>
          </div>
          <div
            style={{
              flexGrow: 1,
              minHeight: 0,
              display: 'flex',
              flexDirection: 'column',
              gap: z(8),
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
              {label('Prompt')}
              {pickedSkill?.argument_hint ? (
                <Text color={tokens.hint} variant="label" mono>
                  {`arguments ${pickedSkill.argument_hint}`}
                </Text>
              ) : null}
            </div>
            <textarea
              testId="dispatch-prompt"
              value={text}
              placeholder="Pick a skill above, or write what it should do"
              theme={{ caret: accent.base }}
              onChange={(e) => setText(noTabs(e.value))}
              minRows={tokens.layout.dispatchPromptRows}
              maxRows={tokens.layout.dispatchPromptRows}
              style={{
                ...fieldBox(focus === 'prompt', true),
                paddingTop: z(10),
                paddingBottom: z(10),
                lineHeight: z(20),
              }}
              ref={fields.ref('prompt')}
              {...fields.track('prompt')}
            />
          </div>
        </div>
        <CardBar height={60} edge="bottom" paddingLeft={20} paddingRight={14} gap={8}>
          <div style={{ flexGrow: 1, minWidth: 0, display: 'flex' }}>
            <Text
              color={error ? tokens.red : tokens.text3}
              variant="small"
              weight={400}
              ellipsis
              testId="dispatch-summary"
            >
              {error ?? summary}
            </Text>
          </div>
          <Button
            testId="dispatch-cancel"
            onClick={close}
            onFileDrop={onFileDrop}
            focusable
            focused={focus === 'cancel'}
            {...tracked('cancel')}
          >
            <Text color={tokens.text}>Cancel</Text>
          </Button>
          <Button
            testId="dispatch-send"
            variant="primary"
            onClick={submit}
            onFileDrop={onFileDrop}
            paddingRight={8}
            gap={10}
            focusable
            focused={focus === 'send'}
            {...tracked('send')}
          >
            <Text color={tokens.onAccent} weight={600}>
              {pending ? 'Queueing…' : verb}
            </Text>
            <Kbd label="⌘⏎" color={tokens.onAccent} background={tokens.onAccentKey} ring={null} />
          </Button>
        </CardBar>
      </div>
    </Card>
  );
}

/** One pane the form can send to: its place, project, CLI, status and queue length. */
function PaneChip({
  pane,
  chosen,
  onPick,
  onFileDrop,
}: {
  pane: PaneState;
  chosen: boolean;
  onPick: () => void;
  onFileDrop: (event: EventPayload) => void;
}) {
  const { z, accent } = useChrome();
  const place = useAppSelector((s) => panePlace(s, pane.id)?.pane ?? pane.id);
  const queued = useAppSelector((s) => selectPaneQueue(s, pane.id).length);
  const shellName = useAppSelector((s) => s.env.shellName);
  const view = statusView(pane, shellName);
  const colour =
    view.tone === 'running'
      ? accent.base
      : view.tone === 'waiting'
        ? tokens.amber
        : tokens.textSoft;
  const project = pane.project ?? pane.cwd.split('/').filter(Boolean).at(-1) ?? pane.cwd;
  return (
    <div
      testId={`dispatch-pane-${pane.id}`}
      onClick={onPick}
      onFileDrop={onFileDrop}
      style={{
        flexGrow: 1,
        flexBasis: 0,
        minWidth: 0,
        height: z(46),
        display: 'flex',
        flexDirection: 'column',
        justifyContent: 'center',
        gap: z(3),
        paddingLeft: z(10),
        paddingRight: z(10),
        borderRadius: z(9),
        cursor: 'pointer',
        backgroundColor: chosen ? accent.a10 : tokens.white[2],
        borderWidth: 1,
        borderColor: chosen ? accent.a45 : tokens.white[7],
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: z(7), minWidth: 0 }}>
        <Kbd
          label={String(place)}
          height={18}
          minWidth={18}
          paddingX={4}
          color={chosen ? accent.base : tokens.text2}
        />
        <Text color={tokens.text} weight={600} ellipsis>
          {project}
        </Text>
        <Text color={tokens.text2} variant="label" mono>
          {pane.cli}
        </Text>
      </div>
      <Text color={colour} variant="caption" weight={400} ellipsis>
        {`${view.label} · ${queued} queued`}
      </Text>
    </div>
  );
}
