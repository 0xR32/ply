import { describe, expect, test } from 'bun:test';
import type { PathPromptOptions } from '@gpuix/react';
import { createFolderPicker, type PathPrompter } from './folder-picker';

function fakeRenderer(answer: string[] | null) {
  const asked: (PathPromptOptions | null | undefined)[] = [];
  const renderer: PathPrompter = {
    promptForPaths: async (options) => {
      asked.push(options);
      return answer;
    },
  };
  return { renderer, asked };
}

describe('the folder picker', () => {
  test('asks the renderer for one folder and answers the chosen one', async () => {
    const picker = createFolderPicker();
    const { renderer, asked } = fakeRenderer(['/Users/example/code/ply']);
    picker.bind(renderer);
    expect(await picker.prompt()).toBe('/Users/example/code/ply');
    expect(asked).toEqual([{ directories: true, files: false, multiple: false, prompt: 'Choose' }]);
  });

  test('answers null when cancelled, and before a window is bound or after it is gone', async () => {
    const picker = createFolderPicker();
    expect(await picker.prompt()).toBeNull();
    picker.bind(fakeRenderer(null).renderer);
    expect(await picker.prompt()).toBeNull();
    picker.bind(fakeRenderer(['/Users/example']).renderer);
    picker.bind(null);
    expect(await picker.prompt()).toBeNull();
  });
});
