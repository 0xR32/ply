import { describe, expect, test } from 'bun:test';
import { droppedPathsText } from './drop';

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
