import type { Tab } from '../../state/actions';
import { type TabDot, tabDot } from '../../state/selectors';
import { useAppSelector, useDispatch } from '../../state/store';
import { type ChromeTheme, useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Dot } from '../../ui/chip';
import { Icon } from '../../ui/icons';
import { Kbd } from '../../ui/kbd';
import { Text } from '../../ui/text';

function dotColour(dot: TabDot, c: ChromeTheme): string {
  switch (dot) {
    case 'waiting':
      return tokens.amber;
    case 'running':
      return c.accent.base;
    case 'done':
      return tokens.mint;
    case 'idle':
      return tokens.dotIdle;
  }
}

function TabButton({ tab, index, active }: { tab: Tab; index: number; active: boolean }) {
  const dispatch = useDispatch();
  const chrome = useChrome();
  const { z, accent } = chrome;
  const dot = useAppSelector((s) => tabDot(s, tab));
  return (
    <div
      testId={`tab-${tab.id}`}
      role="tab"
      aria-selected={active}
      aria-label={`Tab ${index + 1}, ${tab.name}, ${tab.pane_ids.length} panes`}
      onClick={() => dispatch({ type: 'tab/select', tabId: tab.id })}
      style={{
        height: z(30),
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        gap: z(8),
        paddingLeft: z(6),
        paddingRight: z(10),
        borderRadius: z(8),
        cursor: 'pointer',
        ...(active
          ? {
              backgroundColor: tokens.tabActive,
              borderWidth: 1,
              borderColor: tokens.white[8],
              boxShadow: {
                offsetX: 0,
                offsetY: z(4),
                blurRadius: z(14),
                spreadRadius: -z(6),
                color: tokens.tabShadow,
              },
            }
          : { hover: { backgroundColor: tokens.white[3] } }),
      }}
    >
      {index < 9 ? (
        <Kbd
          label={String(index + 1)}
          height={18}
          minWidth={18}
          paddingX={4}
          variant="key"
          color={active ? accent.base : tokens.text3}
          background={active ? accent.a12 : tokens.white[2]}
          ring={active ? accent.a40 : tokens.white[10]}
        />
      ) : null}
      <Text color={active ? tokens.text : tokens.text2} weight={active ? 500 : 400}>
        {tab.name}
      </Text>
      <Dot color={dotColour(dot, chrome)} size={6} />
      <Text color={tokens.hint} variant="label" mono>
        {String(tab.pane_ids.length)}
      </Text>
    </div>
  );
}

/** The tab strip of the top bar: one button per tab (⌘ digit, name, status dot, pane count) and the + button. */
export function TabBar() {
  const dispatch = useDispatch();
  const { z } = useChrome();
  const tabs = useAppSelector((s) => s.tabs);
  const activeId = useAppSelector((s) => s.activeTabId);
  return (
    <div
      testId="tab-bar"
      role="tablist"
      style={{
        minWidth: 0,
        display: 'flex',
        alignItems: 'center',
        gap: z(4),
        padding: z(3),
        borderRadius: z(11),
        backgroundColor: tokens.white[3],
        borderWidth: 1,
        borderColor: tokens.hairline,
        overflow: 'hidden',
      }}
    >
      {tabs.map((tab, index) => (
        <TabButton key={tab.id} tab={tab} index={index} active={tab.id === activeId} />
      ))}
      <div
        testId="tab-new"
        role="button"
        aria-label="New tab"
        onClick={() => dispatch({ type: 'command', id: 'tab.new' })}
        style={{
          width: z(30),
          height: z(30),
          flexShrink: 0,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          borderRadius: z(8),
          cursor: 'pointer',
          hover: { backgroundColor: tokens.white[4] },
        }}
      >
        <Icon name="plus" size={14} color={tokens.text2} />
      </div>
    </div>
  );
}
