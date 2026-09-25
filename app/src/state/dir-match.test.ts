import { describe, expect, test } from 'bun:test';
import { type DirCandidate, matchQuality, rankDirs } from './dir-match';
import { reduce } from './reducer';
import { pathQuery, selectDirSuggestions } from './selectors';
import { makePane, makeState } from './test-support';

const HOME = '/Users/example';

function candidate(
  path: string,
  source: DirCandidate['source'],
  recency = 0,
  segment?: string,
): DirCandidate {
  const shown = path === HOME || path.startsWith(`${HOME}/`) ? `~${path.slice(HOME.length)}` : path;
  return { path, shown, source, recency, ...(segment !== undefined ? { segment } : {}) };
}

const shown = (query: string, candidates: DirCandidate[]) =>
  rankDirs(query, candidates).map((r) => r.shown);

describe('matchQuality', () => {
  test('grades equal, prefix, substring and scattered matches, ignoring case', () => {
    expect(matchQuality('ply', 'PLY')).toBe(3);
    expect(matchQuality('ply', 'ply-web')).toBe(2);
    expect(matchQuality('web', 'ply-web')).toBe(1);
    expect(matchQuality('pw', 'ply-web')).toBe(0);
    expect(matchQuality('wp', 'ply-web')).toBeNull();
    expect(matchQuality('', 'anything')).toBe(2);
  });
});

describe('rankDirs', () => {
  test('a fuzzy subsequence over the ~-abbreviated path; a basename match ranks above a path match', () => {
    const list = [
      candidate(`${HOME}/ply/docs`, 'repo'),
      candidate(`${HOME}/code/deploy`, 'repo'),
      candidate(`${HOME}/code/api`, 'repo'),
    ];
    expect(shown('ply', list)).toEqual(['~/code/deploy', '~/ply/docs']);
    expect(shown('cdapi', list)).toEqual(['~/code/api']);
  });

  test('a better match class ranks higher, and a recent folder counts one class more', () => {
    const list = [
      candidate(`${HOME}/src/ply-web`, 'repo'),
      candidate(`${HOME}/src/ply`, 'repo'),
      candidate(`${HOME}/work/pl-y`, 'recent', 2),
      candidate(`${HOME}/work/deply`, 'recent', 1),
    ];
    expect(shown('ply', list)).toEqual([
      '~/src/ply',
      '~/work/deply',
      '~/src/ply-web',
      '~/work/pl-y',
    ]);
  });

  test('ties go to recents before repositories before completions, then recency, then the shorter path', () => {
    const list = [
      candidate(`${HOME}/a/long/path/ply`, 'repo'),
      candidate(`${HOME}/b/ply`, 'repo'),
      candidate(`${HOME}/code/ply`, 'completion', 0, 'ply'),
      candidate(`${HOME}/old/ply`, 'recent', 1),
      candidate(`${HOME}/new/ply`, 'recent', 2),
    ];
    expect(shown('ply', list)).toEqual([
      '~/new/ply',
      '~/old/ply',
      '~/b/ply',
      '~/a/long/path/ply',
      '~/code/ply',
    ]);
  });

  test('an empty query lists only the recent folders, most recent first, at most eight', () => {
    const recents = Array.from({ length: 10 }, (_, i) =>
      candidate(`${HOME}/r${i}`, 'recent', 10 - i),
    );
    const rows = rankDirs('  ', [candidate(`${HOME}/repo`, 'repo'), ...recents]);
    expect(rows.map((r) => r.shown)).toEqual(recents.slice(0, 8).map((c) => c.shown));
  });

  test('one row per folder, from the best source that matches; rows split into parent and name', () => {
    const rows = rankDirs('ply', [
      candidate(`${HOME}/code/ply`, 'recent', 1),
      candidate(`${HOME}/code/ply`, 'repo'),
      candidate(`${HOME}/code/ply`, 'completion', 0, 'ply'),
    ]);
    expect(rows).toEqual([
      {
        path: `${HOME}/code/ply`,
        shown: '~/code/ply',
        parent: '~/code/',
        name: 'ply',
        source: 'recent',
      },
    ]);
    expect(rankDirs('~', [candidate(HOME, 'recent', 1)])[0]).toMatchObject({
      parent: '',
      name: '~',
    });
  });
});

describe('pathQuery', () => {
  const base = `${HOME}/code/ply`;
  test('splits a path query into the folder it names and the segment typed after it', () => {
    expect(pathQuery('~', HOME, base)).toEqual({ dir: HOME, segment: '' });
    expect(pathQuery('~/code/pl', HOME, base)).toEqual({ dir: `${HOME}/code`, segment: 'pl' });
    expect(pathQuery('/', HOME, base)).toEqual({ dir: '/', segment: '' });
    expect(pathQuery('/Us', HOME, base)).toEqual({ dir: '/', segment: 'Us' });
    expect(pathQuery('./cr', HOME, base)).toEqual({ dir: base, segment: 'cr' });
    expect(pathQuery('.', HOME, base)).toEqual({ dir: base, segment: '.' });
    expect(pathQuery('../', HOME, base)).toEqual({ dir: `${HOME}/code`, segment: '' });
  });

  test('anything else is a search, not a path', () => {
    expect(pathQuery('ply', HOME, base)).toBeNull();
    expect(pathQuery('~other/x', HOME, base)).toBeNull();
    expect(pathQuery('', HOME, base)).toBeNull();
  });
});

describe('the directory search in the store', () => {
  const loaded = makeState([makePane({ id: 1, cwd: `${HOME}/code/ply` })], [{ id: 1 }]);

  test('opening the form puts the focused pane’s folder in the field and keeps what was found before', () => {
    const before = { ...loaded, dirs: { ...loaded.dirs, repos: [`${HOME}/src/x`], open: true } };
    const opened = reduce(before, { type: 'command', id: 'tab.new' });
    expect(opened.dirs).toMatchObject({
      query: '~/code/ply',
      base: `${HOME}/code/ply`,
      open: false,
      repos: [`${HOME}/src/x`],
      completion: null,
    });
    const empty = reduce(makeState([], []), { type: 'command', id: 'pane.new' });
    expect(empty.dirs).toMatchObject({ query: '~', base: HOME });
  });

  test('typing opens the list, a taken folder is written with ~ and closes it, esc only closes it', () => {
    let s = reduce(loaded, { type: 'command', id: 'pane.new' });
    s = reduce(s, { type: 'dirs/query', query: 'pl' });
    expect(s.dirs).toMatchObject({ query: 'pl', open: true });
    expect(reduce(s, { type: 'dirs/close' }).dirs).toMatchObject({ query: 'pl', open: false });
    s = reduce(s, { type: 'dirs/accept', path: `${HOME}/code/api` });
    expect(s.dirs).toMatchObject({ query: '~/code/api', open: false });
    expect(reduce(s, { type: 'dirs/accept', path: '/opt/data' }).dirs.query).toBe('/opt/data');
  });

  test('completions are the listed folder’s children matched by the typed segment; hidden ones only after a dot', () => {
    let s = reduce(loaded, { type: 'command', id: 'pane.new' });
    s = reduce(s, {
      type: 'dirs/completion',
      dir: `${HOME}/code`,
      children: ['.cache', 'api', 'ply', 'ply-web'].map((n) => `${HOME}/code/${n}`),
    });
    const rows = (query: string) =>
      selectDirSuggestions(reduce(s, { type: 'dirs/query', query })).map((r) => r.shown);
    expect(rows('~/code/')).toEqual(['~/code/api', '~/code/ply', '~/code/ply-web']);
    expect(rows('~/code/pl')).toEqual(['~/code/ply', '~/code/ply-web']);
    expect(rows('~/code/.')).toEqual(['~/code/.cache']);
    expect(rows('../')).toEqual(['~/code/api', '~/code/ply', '~/code/ply-web']);
    expect(rows('~/code/pl/deeper')).toEqual(['~/code/ply', '~/code/ply-web']);
    expect(rows('~/other/')).toEqual([]);
    expect(rows('ply')).toEqual([]);
  });

  test('the suggestions are recomputed only when the search changes', () => {
    const s = reduce(
      { ...loaded, dirs: { ...loaded.dirs, recent: [`${HOME}/code/ply`] } },
      { type: 'dirs/query', query: 'ply' },
    );
    const rows = selectDirSuggestions(s);
    expect(rows.map((r) => r.source)).toEqual(['recent']);
    expect(selectDirSuggestions(reduce(s, { type: 'notice/show', text: 'x' }))).toBe(rows);
  });
});
