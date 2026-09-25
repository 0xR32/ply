import { type NativeRenderer, useGpuix } from '@gpuix/react';
import { useEffect } from 'react';

/** The renderer capability the picker needs; tests pass a fake. */
export type PathPrompter = Pick<NativeRenderer, 'promptForPaths'>;

/** The native folder picker as effects take it (`promptForDirectory`), bound to the window's renderer by `BindFolderPicker`. */
export interface FolderPicker {
  /** Opens the picker for one folder; the chosen folder, or `null` when cancelled or before a window is bound. */
  prompt(): Promise<string | null>;
  bind(renderer: PathPrompter | null): void;
}

/** A picker with no renderer yet. */
export function createFolderPicker(): FolderPicker {
  let renderer: PathPrompter | null = null;
  return {
    async prompt() {
      const paths = await renderer?.promptForPaths?.({
        directories: true,
        files: false,
        multiple: false,
        prompt: 'Choose',
      });
      return paths?.[0] ?? null;
    },
    bind(next) {
      renderer = next;
    },
  };
}

/** Binds `picker` to the renderer of the window this mounts in: GPUIX hands JavaScript its renderer only through React. */
export function BindFolderPicker({ picker }: { picker: FolderPicker }): null {
  const { renderer } = useGpuix();
  useEffect(() => {
    picker.bind(renderer);
    return () => picker.bind(null);
  }, [picker, renderer]);
  return null;
}
