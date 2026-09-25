import type { DirSource, DirSuggestion } from '../../state/dir-match';
import { useChrome } from '../../theme/chrome';
import { tokens } from '../../theme/tokens';
import { Text } from '../../ui/text';

const TAGS: Record<DirSource, string | null> = { recent: 'recent', repo: 'repo', completion: null };

/** Props of `DirSuggestions`; `width` is unscaled canvas pixels. */
export interface DirSuggestionsProps {
  rows: readonly DirSuggestion[];
  highlight: number;
  width: number;
  onHover: (index: number) => void;
  onPick: (row: DirSuggestion) => void;
}

/** The Directory field's suggestion list, floating under the field: ~-abbreviated paths with the folder name emphasised and a tag for recents and repositories. */
export function DirSuggestions({ rows, highlight, width, onHover, onPick }: DirSuggestionsProps) {
  const { z, accent } = useChrome();
  return (
    <anchored side="bottom" align="start" gap={z(6)} fit="snap" snapMargin={z(8)} deferred>
      <div
        testId="new-pane-dir-list"
        style={{
          width: z(width),
          display: 'flex',
          flexDirection: 'column',
          gap: z(2),
          padding: z(4),
          borderRadius: z(10),
          backgroundColor: tokens.menu,
          borderWidth: 1,
          borderColor: tokens.white[10],
          boxShadow: {
            offsetX: 0,
            offsetY: 16,
            blurRadius: 40,
            spreadRadius: -12,
            color: tokens.overlayShadow,
          },
        }}
      >
        {rows.map((row, i) => {
          const on = i === highlight;
          const tag = TAGS[row.source];
          return (
            <div
              key={row.path}
              testId={`new-pane-dir-option-${i}`}
              onMouseEnter={() => onHover(i)}
              onClick={() => onPick(row)}
              style={{
                height: z(30),
                flexShrink: 0,
                display: 'flex',
                alignItems: 'center',
                gap: z(8),
                paddingLeft: z(10),
                paddingRight: z(8),
                borderRadius: z(7),
                cursor: 'pointer',
                userSelect: 'none',
                borderWidth: 1,
                borderColor: on ? accent.a28 : tokens.menu,
                backgroundColor: on ? accent.a10 : tokens.menu,
              }}
            >
              <div style={{ flexGrow: 1, minWidth: 0, display: 'flex', overflow: 'hidden' }}>
                {row.parent ? (
                  <Text color={tokens.text3} mono ellipsis>
                    {row.parent}
                  </Text>
                ) : null}
                <Text color={tokens.text} mono weight={600}>
                  {row.name}
                </Text>
              </div>
              {tag ? (
                <div
                  style={{
                    height: z(18),
                    flexShrink: 0,
                    display: 'flex',
                    alignItems: 'center',
                    paddingLeft: z(6),
                    paddingRight: z(6),
                    borderRadius: z(5),
                    backgroundColor: tokens.white[5],
                  }}
                >
                  <Text color={on ? accent.base : tokens.text3} variant="label">
                    {tag}
                  </Text>
                </div>
              ) : null}
            </div>
          );
        })}
      </div>
    </anchored>
  );
}
