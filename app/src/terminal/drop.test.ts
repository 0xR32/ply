import { afterEach, describe, expect, test } from 'bun:test';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { droppedPathsText, dropsDir, keepDroppedFile } from './drop';

describe('droppedPathsText', () => {
  test("escapes a macOS screenshot's spaces with backslashes and adds no trailing space", () => {
    expect(droppedPathsText(['/Users/example/Desktop/Screenshot 2026-09-27 at 10.15.32.png'])).toBe(
      '/Users/example/Desktop/Screenshot\\ 2026-09-27\\ at\\ 10.15.32.png',
    );
  });

  test('escapes every shell metacharacter a terminal escapes, the backslash included', () => {
    expect(droppedPathsText(['/tmp/a\\b ()[]{}<>"\'`!#$&;|*?\tz.png'])).toBe(
      '/tmp/a\\\\b\\ \\(\\)\\[\\]\\{\\}\\<\\>\\"\\\'\\`\\!\\#\\$\\&\\;\\|\\*\\?\\\tz.png',
    );
  });

  test('leaves other characters as they are, the narrow no-break space in a screenshot name included', () => {
    expect(droppedPathsText(['/Users/example/Screenshot 10.15.32 AM-é~+,=@%.png'])).toBe(
      '/Users/example/Screenshot\\ 10.15.32 AM-é~+,=@%.png',
    );
  });

  test('joins several files with one space, in the order dropped', () => {
    expect(droppedPathsText(['/tmp/one.png', '/tmp/two words.jpg'])).toBe(
      '/tmp/one.png /tmp/two\\ words.jpg',
    );
  });

  test('an empty drop is empty text', () => {
    expect(droppedPathsText([])).toBe('');
  });
});

describe('keepDroppedFile', () => {
  const roots: string[] = [];
  afterEach(() => {
    for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
  });
  const sandbox = () => {
    const root = mkdtempSync(join(tmpdir(), 'ply-drop-test-'));
    roots.push(root);
    return { root, drops: join(root, 'ply-drops') };
  };
  const staged = (root: string, name: string, bytes = 'png bytes') => {
    const path = join(root, 'T', 'TemporaryItems', `NSIRD_screencaptureui_${roots.length}`, name);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, bytes);
    return path;
  };

  test('keeps a file macOS staged in TemporaryItems, which it deletes when the drag ends', () => {
    const { root, drops } = sandbox();
    const source = staged(root, 'Screenshot 2026-09-27 at 10.15.32.png');
    const kept = keepDroppedFile(source, drops);
    expect(kept.startsWith(`${drops}/`)).toBe(true);
    expect(basename(kept)).toBe('Screenshot 2026-09-27 at 10.15.32.png');
    rmSync(dirname(source), { recursive: true });
    expect(readFileSync(kept, 'utf8')).toBe('png bytes');
  });

  test('two staged files of one name are kept apart', () => {
    const { root, drops } = sandbox();
    const a = keepDroppedFile(staged(root, 'Screenshot.png', 'a'), drops);
    const b = keepDroppedFile(staged(join(root, 'second'), 'Screenshot.png', 'b'), drops);
    expect(a).not.toBe(b);
    expect([readFileSync(a, 'utf8'), readFileSync(b, 'utf8')]).toEqual(['a', 'b']);
  });

  test('leaves any other path as dropped and writes nothing', () => {
    const { drops } = sandbox();
    expect(keepDroppedFile('/Users/example/Desktop/shot.png', drops)).toBe(
      '/Users/example/Desktop/shot.png',
    );
    expect(existsSync(drops)).toBe(false);
  });

  test('a staged file already gone is left as dropped', () => {
    const { root, drops } = sandbox();
    const gone = join(root, 'T', 'TemporaryItems', 'NSIRD_screencaptureui_x', 'Screenshot.png');
    expect(keepDroppedFile(gone, drops)).toBe(gone);
  });

  test('the drops folder follows PLY_HOME, else the temporary directory', () => {
    expect(dropsDir({ PLY_HOME: '/Users/example/ply-dev' })).toBe(
      '/Users/example/ply-dev/ply-drops',
    );
    expect(dropsDir({})).toBe(join(tmpdir(), 'ply-drops'));
  });
});
