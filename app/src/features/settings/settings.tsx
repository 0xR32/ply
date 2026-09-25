import type { EventPayload } from '@gpuix/react';
import type { ReactNode } from 'react';
import type { AccentName, OptionAsMeta, Settings as SettingsValue } from '../../state/actions';
import { useAppSelector, useDispatch } from '../../state/store';
import { useChrome } from '../../theme/chrome';
import { accentAlternatives, tokens } from '../../theme/tokens';
import { useFocusFields } from '../../ui/focus-fields';
import { Kbd } from '../../ui/kbd';
import { Card, CardBar } from '../../ui/overlay-card';
import { type SegmentItem, Segments } from '../../ui/segments';
import { Switch } from '../../ui/switch';
import { Text } from '../../ui/text';

const ACCENTS: readonly SegmentItem<AccentName>[] = (
  Object.keys(accentAlternatives) as AccentName[]
).map((name) => ({
  value: name,
  label: name[0]?.toUpperCase() + name.slice(1),
  swatch: accentAlternatives[name],
}));

const META: readonly SegmentItem<OptionAsMeta>[] = [
  { value: 'off', label: 'Off' },
  { value: 'left', label: 'Left ⌥' },
  { value: 'right', label: 'Right ⌥' },
  { value: 'both', label: 'Both' },
];

type Field = 'accent' | 'meta' | 'awake' | 'colours';

const ORDER: readonly Field[] = ['accent', 'meta', 'awake', 'colours'];

function cycle<T extends string>(items: readonly SegmentItem<T>[], value: T, delta: number): T {
  const at = items.findIndex((i) => i.value === value);
  return items[(at + delta + items.length) % items.length]?.value ?? value;
}

/** The ⌘, overlay: accent, ⌥ as Meta, keep awake and Claude Code colours, saved through `settings.set`. */
export function Settings() {
  const dispatch = useDispatch();
  const { z } = useChrome();
  const settings = useAppSelector((s) => s.settings);
  const fields = useFocusFields<Field>(ORDER, 'accent');
  const change = (patch: Partial<SettingsValue>) =>
    dispatch({ type: 'settings/change', settings: { ...settings, ...patch } });
  const tracked = (field: Field) => ({
    focused: fields.focused === field,
    innerRef: fields.ref(field),
    ...fields.track(field),
  });
  const onKeyDown = (event: EventPayload) => {
    const m = event.modifiers;
    if (m?.cmd || m?.ctrl || m?.alt) return;
    const at = fields.current();
    const delta = event.key === 'right' ? 1 : event.key === 'left' ? -1 : 0;
    const press = event.key === 'space' || event.key === 'enter';
    if (event.key === 'escape') dispatch({ type: 'overlay/close' });
    else if (event.key === 'tab') fields.step(m?.shift ?? false);
    else if (delta !== 0 && at === 'accent') {
      change({ accent: cycle(ACCENTS, settings.accent, delta) });
    } else if (delta !== 0 && at === 'meta') {
      change({ option_as_meta: cycle(META, settings.option_as_meta, delta) });
    } else if (press && at === 'awake') {
      change({ keep_awake_while_running: !settings.keep_awake_while_running });
    } else if (press && at === 'colours') {
      change({ use_ply_colours_in_claude: !settings.use_ply_colours_in_claude });
    }
  };
  const row = (title: string, hint: string, control: ReactNode) => (
    <div style={{ display: 'flex', flexDirection: 'column', gap: z(8) }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: z(2) }}>
        <Text color={tokens.text} weight={500}>
          {title}
        </Text>
        <Text color={tokens.text3} variant="small" weight={400}>
          {hint}
        </Text>
      </div>
      {control}
    </div>
  );
  const toggle = (title: string, hint: string, control: ReactNode) => (
    <div style={{ display: 'flex', alignItems: 'center', gap: z(12) }}>
      {control}
      <div style={{ flexGrow: 1, display: 'flex', flexDirection: 'column', gap: z(2) }}>
        <Text color={tokens.text} weight={500}>
          {title}
        </Text>
        <Text color={tokens.text3} variant="small" weight={400}>
          {hint}
        </Text>
      </div>
    </div>
  );
  return (
    <Card width={tokens.layout.newPaneWidth} testId="settings">
      <div onKeyDown={onKeyDown} style={{ display: 'flex', flexDirection: 'column' }}>
        <CardBar height={56} edge="top" paddingLeft={20} paddingRight={14} justify="space-between">
          <div style={{ display: 'flex', flexDirection: 'column', gap: z(2) }}>
            <Text color={tokens.text} variant="title">
              Settings
            </Text>
            <Text color={tokens.text3} variant="small" weight={400}>
              Stored by plyd in config.toml; changes apply at once.
            </Text>
          </div>
          <div
            testId="settings-close"
            onClick={() => dispatch({ type: 'overlay/close' })}
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
            display: 'flex',
            flexDirection: 'column',
            gap: z(20),
            paddingTop: z(18),
            paddingBottom: z(22),
            paddingLeft: z(20),
            paddingRight: z(20),
          }}
        >
          {row(
            'Accent',
            'Focus ring, running status and the terminal cursor.',
            <Segments
              testId="settings-accent"
              items={ACCENTS}
              value={settings.accent}
              onChange={(accent) => change({ accent })}
              autoFocus
              {...tracked('accent')}
            />,
          )}
          {row(
            '⌥ as Meta',
            'Off keeps ⌥ for typing characters such as @ [ ] { } | ~ on many layouts.',
            <Segments
              testId="settings-meta"
              items={META}
              value={settings.option_as_meta}
              onChange={(option_as_meta) => change({ option_as_meta })}
              {...tracked('meta')}
            />,
          )}
          {toggle(
            'Keep the Mac awake while an agent runs',
            'Idle sleep waits while any pane is running; closing the lid still sleeps.',
            <Switch
              testId="settings-awake"
              label="Keep the Mac awake while an agent runs"
              on={settings.keep_awake_while_running}
              onToggle={() =>
                change({ keep_awake_while_running: !settings.keep_awake_while_running })
              }
              {...tracked('awake')}
            />,
          )}
          {toggle(
            'Use ply colours in Claude Code',
            'Starts Claude Code with its dark-ansi theme; applies to new Claude panes.',
            <Switch
              testId="settings-colours"
              label="Use ply colours in Claude Code"
              on={settings.use_ply_colours_in_claude}
              onToggle={() =>
                change({ use_ply_colours_in_claude: !settings.use_ply_colours_in_claude })
              }
              {...tracked('colours')}
            />,
          )}
        </div>
      </div>
    </Card>
  );
}
