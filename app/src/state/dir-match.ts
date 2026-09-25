/** Where a suggested folder comes from (Ruling R57), in rank order. */
export type DirSource = 'recent' | 'repo' | 'completion';

/** One folder to rank: `shown` is its ~-abbreviated path, the text a query is matched against. */
export interface DirCandidate {
  path: string;
  shown: string;
  source: DirSource;
  /** Higher is more recent; 0 for folders without a date (repositories, completions). */
  recency: number;
  /** A completion is matched only by this: the path segment typed after the folder it was listed from. */
  segment?: string;
}

/** One row of the suggestion list: `parent` (with its trailing `/`) and `name` together are `shown`. */
export interface DirSuggestion {
  path: string;
  shown: string;
  parent: string;
  name: string;
  source: DirSource;
}

/** The most rows the list shows. */
export const MAX_DIR_SUGGESTIONS = 8;

const SOURCE_RANK: Record<DirSource, number> = { recent: 0, repo: 1, completion: 2 };

/** How well `query` matches `text`, ignoring case: 3 equal, 2 prefix, 1 substring, 0 scattered subsequence; `null` when its characters do not occur in order. */
export function matchQuality(query: string, text: string): number | null {
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  let at = 0;
  for (const c of q) {
    at = t.indexOf(c, at);
    if (at < 0) return null;
    at += c.length;
  }
  if (t === q) return 3;
  if (t.startsWith(q)) return 2;
  return t.includes(q) ? 1 : 0;
}

function split(shown: string): { parent: string; name: string } {
  const cut = shown.lastIndexOf('/');
  const name = shown.slice(cut + 1);
  return name === '' ? { parent: '', name: shown } : { parent: shown.slice(0, cut + 1), name };
}

/** The best `limit` candidates for `query`, one row per folder: an empty query lists the recent folders by recency; otherwise a basename match outranks a path match, a better match class ranks higher, a recent folder counts one class more, and ties go by source, recency, then the shorter path. */
export function rankDirs(
  query: string,
  candidates: readonly DirCandidate[],
  limit = MAX_DIR_SUGGESTIONS,
): DirSuggestion[] {
  const q = query.trim();
  const last = q.slice(q.lastIndexOf('/') + 1);
  const scored: { c: DirCandidate; score: number; parent: string; name: string }[] = [];
  const seen = new Set<string>();
  for (const c of candidates) {
    if (seen.has(c.path)) continue;
    const { parent, name } = split(c.shown);
    let score: number;
    if (q === '') {
      if (c.source !== 'recent') continue;
      score = 0;
    } else if (c.segment !== undefined) {
      const quality = matchQuality(c.segment, name);
      if (quality === null) continue;
      score = 100 + (c.segment === '' ? 0 : quality * 10);
    } else {
      const onPath = matchQuality(q, c.shown);
      if (onPath === null) continue;
      const onName = last === '' ? 0 : matchQuality(last, name);
      score = onName === null ? onPath * 10 : 100 + onName * 10;
      if (c.source === 'recent') score += 10;
    }
    seen.add(c.path);
    scored.push({ c, score, parent, name });
  }
  scored.sort(
    (a, b) =>
      b.score - a.score ||
      SOURCE_RANK[a.c.source] - SOURCE_RANK[b.c.source] ||
      b.c.recency - a.c.recency ||
      a.c.shown.length - b.c.shown.length ||
      (a.c.shown < b.c.shown ? -1 : a.c.shown > b.c.shown ? 1 : 0),
  );
  return scored.slice(0, limit).map(({ c, parent, name }) => ({
    path: c.path,
    shown: c.shown,
    parent,
    name,
    source: c.source,
  }));
}
