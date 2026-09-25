import { type PublicInstance, useGpuix } from '@gpuix/react';
import { useCallback, useMemo, useRef, useState } from 'react';

/** Focus bookkeeping of one overlay form; `current()` asks GPUI, since focus events do not fire for every change. */
export interface FocusFields<F extends string> {
  /** The field drawn with a focus ring. */
  focused: F | null;
  /** A ref callback that registers the host element of `field`. */
  ref(field: F): (instance: PublicInstance | null) => void;
  /** The field GPUI focus is on now, else the last one known. */
  current(): F | null;
  /** Moves focus to the next (or previous) present field in form order, wrapping, as Tab / ⇧Tab do. */
  step(backwards: boolean): void;
  /** `onFocus` / `onBlur` for `field`, keeping the ring in step with clicks. */
  track(field: F): { onFocus: () => void; onBlur: () => void };
}

/** Focus tracking for a form's fields in `order` (the ones shown now); GPUIX binds no Tab, so forms move focus. */
export function useFocusFields<F extends string>(
  order: readonly F[],
  initial: F | null,
): FocusFields<F> {
  const { renderer } = useGpuix();
  const [focused, setFocused] = useState<F | null>(initial);
  const ids = useRef(new Map<F, number>());
  const refs = useRef(new Map<F, (instance: PublicInstance | null) => void>());
  const fieldOf = useCallback((id: number | null | undefined): F | null => {
    for (const [field, fieldId] of ids.current) if (fieldId === id) return field;
    return null;
  }, []);
  return useMemo(
    () => ({
      focused,
      ref(field) {
        let cb = refs.current.get(field);
        if (!cb) {
          cb = (instance) => {
            if (instance) ids.current.set(field, instance.id);
            else ids.current.delete(field);
          };
          refs.current.set(field, cb);
        }
        return cb;
      },
      current() {
        return fieldOf(renderer?.getFocusedElementId?.()) ?? focused;
      },
      step(backwards) {
        // GPUI's focus_prev never leaves a <textarea>, so the order is the form's, not GPUI's tab stops.
        const present = order.filter((f) => ids.current.has(f));
        if (present.length === 0 || !renderer) return;
        const at = fieldOf(renderer.getFocusedElementId?.()) ?? focused;
        const index = at === null ? -1 : present.indexOf(at);
        const delta = backwards ? -1 : 1;
        const start = index < 0 ? (backwards ? 0 : -1) : index;
        const next = present[(start + delta + present.length) % present.length] as F;
        const id = ids.current.get(next);
        if (id !== undefined) renderer.focusElement?.(id);
        setFocused(next);
      },
      track(field) {
        return {
          onFocus: () => setFocused(field),
          onBlur: () => setFocused((f) => (f === field ? null : f)),
        };
      },
    }),
    [focused, renderer, fieldOf, order],
  );
}
