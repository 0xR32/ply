import type { EventPayload, PublicInstance } from '@gpuix/react';
import { type ReactNode, useEffect, useMemo, useRef, useState } from 'react';
import type { AppState } from '../../state/reducer';
import { useAppSelector, useDispatch } from '../../state/store';
import { type ChromeTheme, useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Dot } from '../../ui/chip';
import { Icon } from '../../ui/icons';
import { Kbd } from '../../ui/kbd';
import { Card, CardBar } from '../../ui/overlay-card';
import { Text } from '../../ui/text';
import { type ItemDot, type PaletteItem, paletteItems } from './commands';

function dotColour(dot: ItemDot, c: ChromeTheme): string {
  switch (dot) {
    case 'accent':
      return c.accent.base;
    case 'amber':
      return tokens.amber;
    case 'mint':
      return tokens.mint;
    case 'dim':
      return tokens.text3;
  }
}

const selectAll = (s: AppState) => s;

/** The ⌘K palette: filter commands, tabs and panes; ↑↓ move, ⏎ runs, esc closes, a digit jumps to that tab. */
export function Palette() {
  const dispatch = useDispatch();
  const chrome = useChrome();
  const { z, accent, fonts, type } = chrome;
  const state = useAppSelector(selectAll);
  const [query, setQuery] = useState('');
  const [highlight, setHighlight] = useState(0);
  const items = useMemo(
    () => paletteItems(state, query, Math.floor(Date.now() / 1000)),
    [state, query],
  );
  const hi = items.length === 0 ? -1 : Math.min(highlight, items.length - 1);
  const hiRef = useRef<PublicInstance>(null);
  useEffect(() => {
    if (hi >= 0) hiRef.current?.scrollIntoView?.();
  }, [hi]);

  const close = () => dispatch({ type: 'overlay/close' });
  const run = (item: PaletteItem | undefined) => {
    if (!item) return;
    for (const action of item.actions) dispatch(action);
  };
  const onKeyDown = (event: EventPayload) => {
    const m = event.modifiers;
    if (m?.cmd || m?.ctrl || m?.alt) return;
    const n = items.length;
    if (event.key === 'up' && n > 0) setHighlight((hi - 1 + n) % n);
    else if (event.key === 'down' && n > 0) setHighlight((hi + 1) % n);
    else if (event.key === 'escape') close();
    else if (query === '' && /^[1-9]$/.test(event.key ?? '')) {
      const tab = state.tabs[Number(event.key) - 1];
      if (tab) {
        close();
        dispatch({ type: 'tab/select', tabId: tab.id });
      }
    }
  };

  const rows: ReactNode[] = [];
  let section: string | null = null;
  items.forEach((item, i) => {
    if (item.section !== section) {
      section = item.section;
      rows.push(
        <div
          key={`section-${section}`}
          style={{ paddingTop: z(i === 0 ? 12 : 10), paddingBottom: z(6), paddingLeft: z(10) }}
        >
          <Text color={tokens.text3} variant="caption">
            {section}
          </Text>
        </div>,
      );
    }
    const on = i === hi;
    rows.push(
      <div
        key={item.id}
        ref={on ? hiRef : undefined}
        testId={`palette-item-${item.id}`}
        onMouseEnter={() => setHighlight(i)}
        onClick={() => run(item)}
        style={{
          height: z(40),
          flexShrink: 0,
          display: 'flex',
          alignItems: 'center',
          gap: z(12),
          paddingLeft: z(10),
          paddingRight: z(10),
          borderRadius: z(9),
          cursor: 'pointer',
          userSelect: 'none',
          ...(on ? { backgroundColor: accent.a10, borderWidth: 1, borderColor: accent.a28 } : {}),
        }}
      >
        <Dot color={dotColour(item.dot, chrome)} size={6} />
        <Text color={tokens.text} weight={500}>
          {item.label}
        </Text>
        <Text color={tokens.text3} ellipsis>
          {item.hint}
        </Text>
        <div style={{ flexGrow: 1 }} />
        {item.keys ? (
          <Kbd
            label={item.keys}
            height={22}
            paddingX={7}
            radius={6}
            color={on ? accent.base : tokens.text2}
            background={tokens.white[4]}
            ring={on ? accent.a45 : tokens.white[10]}
          />
        ) : null}
      </div>,
    );
  });

  return (
    <Card width={tokens.layout.paletteWidth} height={tokens.layout.paletteHeight} testId="palette">
      <div
        onKeyDown={onKeyDown}
        style={{ flexGrow: 1, minHeight: 0, display: 'flex', flexDirection: 'column' }}
      >
        <CardBar height={56} edge="top" paddingLeft={18} paddingRight={14}>
          <Icon name="search" size={16} color={tokens.text3} />
          <input
            testId="palette-input"
            autoFocus
            value={query}
            placeholder="Search commands, tabs and panes"
            theme={{ caret: accent.base }}
            onChange={(e) => {
              setQuery((e.value ?? '').replaceAll('\t', ''));
              setHighlight(0);
            }}
            onSubmit={() => run(items[hi])}
            style={{
              height: z(36),
              flexGrow: 1,
              minWidth: 0,
              color: tokens.text,
              fontFamily: fonts.ui,
              fontSize: z(15),
              lineHeight: type.wordmark.lineHeight,
            }}
          />
          <div testId="palette-close" onClick={close} style={{ cursor: 'pointer' }}>
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
          testId="palette-list"
          style={{
            flexGrow: 1,
            minHeight: 0,
            display: 'flex',
            flexDirection: 'column',
            gap: z(2),
            paddingLeft: z(8),
            paddingRight: z(8),
            paddingBottom: z(6),
            overflowY: 'scroll',
          }}
        >
          {rows.length > 0 ? (
            rows
          ) : (
            <div style={{ paddingTop: z(14), paddingLeft: z(10) }}>
              <Text color={tokens.text3}>No matching command, tab or pane</Text>
            </div>
          )}
        </div>
        <CardBar height={40} edge="bottom" paddingLeft={18} paddingRight={18} gap={16}>
          {[
            ['↑↓', 'move'],
            ['⏎', 'run'],
            ['1–9', 'jump to a tab'],
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
