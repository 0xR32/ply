import { type PublicInstance, useGpuix } from '@gpuix/react';
import { type ReactNode, useEffect, useRef } from 'react';
import type { Skill } from '../../state/actions';
import { SKILL_GROUPS } from '../../state/selectors';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Text } from '../../ui/text';

/** Props of `SkillList`; `skills` are already filtered, in `skill.list` order. */
export interface SkillListProps {
  skills: readonly Skill[];
  picked: string | null;
  highlight: number;
  loading: boolean;
  error: string | null;
  onPick: (skill: Skill) => void;
  onHover: (index: number) => void;
}

/** The dispatch form's skill list: one group per source, each row the invocation, its description and its argument hint. */
export function SkillList({
  skills,
  picked,
  highlight,
  loading,
  error,
  onPick,
  onHover,
}: SkillListProps) {
  const { z, accent } = useChrome();
  const { renderer } = useGpuix();
  const listRef = useRef<PublicInstance>(null);
  const hiRef = useRef<PublicInstance>(null);
  useEffect(() => {
    // As in the palette: GPUIX 0.10.0's scrollIntoView leaves a list where it is, so the row's overflow is scrolled here.
    const row = hiRef.current?.id;
    const list = listRef.current?.id;
    if (highlight < 0 || row === undefined || list === undefined) return;
    const r = renderer?.getElementBounds?.(row);
    const view = renderer?.getElementBounds?.(list);
    const offset = renderer?.getScrollOffset?.(list);
    if (!r || !view || !offset) return;
    const [x = 0, y = 0] = offset;
    const top = view.y - y;
    const below = r.y + r.height + z(4) - (top + view.height);
    const above = top + z(4) - r.y;
    if (below > 0) renderer?.scrollTo?.(list, x, y - below);
    else if (above > 0) renderer?.scrollTo?.(list, x, Math.min(0, y + above));
  }, [highlight, renderer, z]);

  const rows: ReactNode[] = [];
  let index = 0;
  for (const group of SKILL_GROUPS) {
    const members = skills.filter((s) => s.source === group.source);
    if (members.length === 0) continue;
    rows.push(
      <div
        key={`group-${group.source}`}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: z(8),
          paddingTop: z(8),
          paddingBottom: z(4),
          paddingLeft: z(8),
        }}
      >
        <Text color={tokens.text3} variant="caption">
          {group.label}
        </Text>
        <Text color={tokens.hint} variant="label" mono>
          {String(members.length)}
        </Text>
      </div>,
    );
    for (const skill of members) {
      const at = index++;
      const on = at === highlight;
      const chosen = skill.invocation === picked;
      rows.push(
        <div
          key={skill.invocation}
          ref={on ? hiRef : undefined}
          testId={`dispatch-skill-${skill.invocation}`}
          onMouseEnter={() => onHover(at)}
          onClick={() => onPick(skill)}
          style={{
            height: z(34),
            flexShrink: 0,
            display: 'flex',
            alignItems: 'center',
            gap: z(10),
            paddingLeft: z(8),
            paddingRight: z(8),
            borderRadius: z(7),
            cursor: 'pointer',
            userSelect: 'none',
            ...(on || chosen
              ? { backgroundColor: accent.a10, borderWidth: 1, borderColor: accent.a28 }
              : {}),
          }}
        >
          <Text color={chosen ? accent.base : tokens.text} variant="small" mono>
            {skill.invocation}
          </Text>
          <Text color={tokens.text3} variant="small" weight={400} ellipsis>
            {skill.description ?? (skill.plugin ? `from ${skill.plugin}` : '')}
          </Text>
          <div style={{ flexGrow: 1 }} />
          {skill.argument_hint ? (
            <Text color={tokens.hint} variant="label" mono>
              {skill.argument_hint}
            </Text>
          ) : null}
        </div>,
      );
    }
  }
  const empty = error ?? (loading ? 'Reading the skills on this machine…' : 'No skills match');
  return (
    <div
      ref={listRef}
      testId="dispatch-skills"
      style={{
        flexGrow: 1,
        minHeight: 0,
        display: 'flex',
        flexDirection: 'column',
        gap: z(1),
        paddingLeft: z(6),
        paddingRight: z(6),
        paddingBottom: z(6),
        overflowY: 'scroll',
      }}
    >
      {rows.length > 0 ? (
        rows
      ) : (
        <div style={{ paddingTop: z(12), paddingLeft: z(8) }}>
          <Text color={error ? tokens.red : tokens.text3} variant="small" weight={400}>
            {empty}
          </Text>
        </div>
      )}
    </div>
  );
}
