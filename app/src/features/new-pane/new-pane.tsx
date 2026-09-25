import type { EventPayload, StyleDesc } from '@gpuix/react';
import { useState } from 'react';
import type { Cli } from '../../state/actions';
import { abbreviateHome, expandHome, selectFocusedPane } from '../../state/selectors';
import { useAppSelector, useDispatch } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Button } from '../../ui/button';
import { useFocusFields } from '../../ui/focus-fields';
import { Kbd } from '../../ui/kbd';
import { Card, CardBar } from '../../ui/overlay-card';
import { type SegmentItem, Segments } from '../../ui/segments';
import { Switch } from '../../ui/switch';
import { Text } from '../../ui/text';

const CLIS: readonly SegmentItem<Cli>[] = [
  { value: 'claude', label: 'Claude Code', hint: '1' },
  { value: 'codex', label: 'Codex', hint: '2' },
  { value: 'shell', label: 'Shell', hint: '3' },
];

type Field = 'cli' | 'dir' | 'worktree' | 'worktree-name' | 'prompt' | 'cancel' | 'open';

const noTabs = (value: string | undefined) => (value ?? '').replaceAll('\t', '');

/** The ⌘N / ⌘T form (spec 7.4): CLI, directory, Claude worktree, first prompt; ⌘⏎ opens, esc cancels. */
export function NewPane({ target }: { target: 'pane' | 'tab' }) {
  const dispatch = useDispatch();
  const { z, accent, fonts, type } = useChrome();
  const home = useAppSelector((s) => s.env.home);
  const startDir = useAppSelector(
    (s) => selectFocusedPane(s)?.cwd ?? s.workspace?.path ?? s.env.home,
  );
  const pending = useAppSelector((s) => s.create.pending);
  const failure = useAppSelector((s) => s.create.error);
  const [cli, setCli] = useState<Cli>('claude');
  const [dir, setDir] = useState(() => abbreviateHome(startDir, home));
  const [worktree, setWorktree] = useState(false);
  const [worktreeName, setWorktreeName] = useState('');
  const [prompt, setPrompt] = useState('');
  const [problem, setProblem] = useState<string | null>(null);
  const order: Field[] = [
    'cli',
    'dir',
    ...(cli === 'claude' ? (['worktree'] as const) : []),
    ...(cli === 'claude' && worktree ? (['worktree-name'] as const) : []),
    ...(cli !== 'shell' ? (['prompt'] as const) : []),
    'cancel',
    'open',
  ];
  const fields = useFocusFields<Field>(order, 'cli');
  const focus = fields.focused;

  const close = () => dispatch({ type: 'overlay/close' });
  const tracked = (field: Field) => ({ innerRef: fields.ref(field), ...fields.track(field) });
  const submit = () => {
    if (pending) return;
    const cwd = expandHome(dir, home);
    if (!cwd.startsWith('/')) {
      setProblem('The directory must be an absolute path or start with ~');
      return;
    }
    const name = worktreeName.trim();
    if (cli === 'claude' && worktree && name === '') {
      setProblem('Name the worktree, or turn it off');
      return;
    }
    setProblem(null);
    const first = prompt.trim();
    dispatch({
      type: 'pane/create',
      request: {
        target,
        cli,
        cwd,
        ...(cli === 'claude' && worktree ? { worktree: name } : {}),
        ...(cli !== 'shell' && first ? { prompt: first } : {}),
      },
    });
  };
  const cycleCli = (delta: number) => {
    const at = CLIS.findIndex((c) => c.value === cli);
    const next = CLIS[(at + delta + CLIS.length) % CLIS.length];
    if (next) setCli(next.value);
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
      case 'left':
      case 'right':
        cycleCli(event.key === 'right' ? 1 : -1);
        return;
      case 'space':
      case 'enter':
        if (at === 'worktree') setWorktree((w) => !w);
        else if (at === 'cancel') close();
        else if (at === 'open') submit();
        return;
      default: {
        const pick = CLIS.find((c) => c.hint === event.key);
        if (at === 'cli' && pick) setCli(pick.value);
      }
    }
  };

  const field = (focused: boolean, mono: boolean): StyleDesc => ({
    height: z(36),
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
  const label = (text: string, extra?: string) => (
    <div style={{ display: 'flex', gap: z(4) }}>
      <Text color={tokens.text2} variant="small">
        {text}
      </Text>
      {extra ? (
        <Text color={tokens.hint} variant="small" weight={400}>
          {extra}
        </Text>
      ) : null}
    </div>
  );
  const cliLabel = CLIS.find((c) => c.value === cli)?.label ?? cli;
  const summary = `${cliLabel} ${target === 'tab' ? 'in a new tab' : 'in this tab'}${cli === 'shell' ? '' : ' · a real CLI session'}`;
  const error = problem ?? failure;
  const verb = target === 'tab' ? 'Open tab' : 'Open pane';

  return (
    <Card width={tokens.layout.newPaneWidth} height={tokens.layout.newPaneHeight} testId="new-pane">
      <div
        onKeyDown={onKeyDown}
        style={{ flexGrow: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}
      >
        <CardBar height={56} edge="top" paddingLeft={20} paddingRight={14} justify="space-between">
          <div style={{ display: 'flex', flexDirection: 'column', gap: z(2) }}>
            <Text color={tokens.text} variant="title" testId="new-pane-heading">
              {target === 'tab' ? 'New tab' : 'New pane'}
            </Text>
            <Text color={tokens.text3} variant="small" weight={400}>
              Model and effort are chosen inside the session.
            </Text>
          </div>
          <div testId="new-pane-close" onClick={close} style={{ cursor: 'pointer' }}>
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
            gap: z(18),
            paddingTop: z(18),
            paddingBottom: z(18),
            paddingLeft: z(20),
            paddingRight: z(20),
          }}
        >
          <div style={{ display: 'flex', flexDirection: 'column', gap: z(8) }}>
            {label('Agent')}
            <Segments
              testId="new-pane-cli"
              items={CLIS}
              value={cli}
              onChange={setCli}
              autoFocus
              focused={focus === 'cli'}
              {...tracked('cli')}
            />
          </div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: z(8) }}>
            {label('Directory')}
            <input
              testId="new-pane-dir"
              value={dir}
              theme={{ caret: accent.base }}
              onChange={(e) => setDir(noTabs(e.value))}
              style={field(focus === 'dir', true)}
              ref={fields.ref('dir')}
              {...fields.track('dir')}
            />
          </div>
          {cli === 'claude' ? (
            <div
              style={{
                display: 'flex',
                flexDirection: 'column',
                gap: z(10),
                padding: z(12),
                borderRadius: z(10),
                backgroundColor: tokens.white[2],
                borderWidth: 1,
                borderColor: tokens.hairline,
              }}
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: z(12) }}>
                <Switch
                  testId="new-pane-worktree"
                  label="Run in a Claude Code worktree"
                  on={worktree}
                  onToggle={() => setWorktree((w) => !w)}
                  focused={focus === 'worktree'}
                  {...tracked('worktree')}
                />
                <div style={{ flexGrow: 1, display: 'flex', flexDirection: 'column', gap: z(2) }}>
                  <Text color={tokens.text} weight={500}>
                    Run in a Claude Code worktree
                  </Text>
                  <Text color={tokens.text3} variant="label" mono>
                    {worktree
                      ? `claude --worktree ${worktreeName.trim() || '<name>'}`
                      : 'Claude Code creates and tracks it'}
                  </Text>
                </div>
              </div>
              {worktree ? (
                <div style={{ display: 'flex', flexDirection: 'column', gap: z(8) }}>
                  {label('Worktree name')}
                  <input
                    testId="new-pane-worktree-name"
                    value={worktreeName}
                    placeholder="my-feature"
                    theme={{ caret: accent.base }}
                    onChange={(e) => setWorktreeName(noTabs(e.value))}
                    style={field(focus === 'worktree-name', true)}
                    ref={fields.ref('worktree-name')}
                    {...fields.track('worktree-name')}
                  />
                </div>
              ) : null}
            </div>
          ) : null}
          {cli !== 'shell' ? (
            <div
              style={{
                flexGrow: 1,
                minHeight: 0,
                display: 'flex',
                flexDirection: 'column',
                gap: z(8),
              }}
            >
              {label('First prompt', '(optional)')}
              <textarea
                testId="new-pane-prompt"
                value={prompt}
                placeholder="What should it work on?"
                theme={{ caret: accent.base }}
                onChange={(e) => setPrompt(noTabs(e.value))}
                style={{
                  ...field(focus === 'prompt', false),
                  height: undefined,
                  flexGrow: 1,
                  minHeight: z(72),
                  paddingTop: z(10),
                  paddingBottom: z(10),
                  lineHeight: z(20),
                }}
                ref={fields.ref('prompt')}
                {...fields.track('prompt')}
              />
            </div>
          ) : null}
        </div>
        <CardBar height={60} edge="bottom" paddingLeft={20} paddingRight={14} gap={8}>
          <div style={{ flexGrow: 1, minWidth: 0, display: 'flex' }}>
            <Text
              color={error ? tokens.red : tokens.text3}
              variant="small"
              weight={400}
              ellipsis
              testId="new-pane-summary"
            >
              {error ?? summary}
            </Text>
          </div>
          <Button
            testId="new-pane-cancel"
            onClick={close}
            focusable
            focused={focus === 'cancel'}
            {...tracked('cancel')}
          >
            <Text color={tokens.text}>Cancel</Text>
          </Button>
          <Button
            testId="new-pane-open"
            variant="primary"
            onClick={submit}
            paddingRight={8}
            gap={10}
            focusable
            focused={focus === 'open'}
            {...tracked('open')}
          >
            <Text color={tokens.onAccent} weight={600}>
              {pending ? 'Opening…' : verb}
            </Text>
            <Kbd label="⌘⏎" color={tokens.onAccent} background={tokens.onAccentKey} ring={null} />
          </Button>
        </CardBar>
      </div>
    </Card>
  );
}
