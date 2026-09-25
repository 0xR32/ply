import { createContext, useCallback, useContext, useRef, useSyncExternalStore } from 'react';
import type { Action } from './actions';
import { type AppState, reduce } from './reducer';

/** Runs after the reducer for every action with the new and the previous state; it must not throw. */
export type Effect = (action: Action, next: AppState, prev: AppState) => void;

/** The single app store (spec 9.2): synchronous, re-entrant dispatches are queued and applied in order. */
export interface Store {
  getState(): AppState;
  dispatch(action: Action): void;
  subscribe(listener: () => void): () => void;
  addEffect(effect: Effect): () => void;
}

/** A store starting at `initial`; listeners and effects run after every action that is applied. */
export function createStore(initial: AppState): Store {
  let state = initial;
  const listeners = new Set<() => void>();
  const effects = new Set<Effect>();
  const queue: Action[] = [];
  let draining = false;

  function apply(action: Action): void {
    const prev = state;
    state = reduce(prev, action);
    for (const effect of effects) effect(action, state, prev);
    if (state !== prev) for (const listener of listeners) listener();
  }

  return {
    getState: () => state,
    dispatch(action) {
      queue.push(action);
      if (draining) return;
      draining = true;
      try {
        for (let next = queue.shift(); next; next = queue.shift()) apply(next);
      } finally {
        draining = false;
      }
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    addEffect(effect) {
      effects.add(effect);
      return () => effects.delete(effect);
    },
  };
}

/** Carries the store to features; the app renders exactly one provider. */
export const StoreContext = createContext<Store | null>(null);

/** The store of the nearest provider; throws outside one, which is a wiring bug. */
export function useStore(): Store {
  const store = useContext(StoreContext);
  if (!store) throw new Error('useStore outside a StoreContext provider');
  return store;
}

/** A slice of state; recomputed only when the state object changes, and kept while `equal` says it is unchanged. */
export function useAppSelector<T>(
  selector: (state: AppState) => T,
  equal: (a: T, b: T) => boolean = Object.is,
): T {
  const store = useStore();
  const cache = useRef<{ state: AppState; selector: (s: AppState) => T; value: T } | null>(null);
  const read = useCallback(() => {
    const state = store.getState();
    const last = cache.current;
    if (last && last.state === state && last.selector === selector) return last.value;
    const value = selector(state);
    const kept = last && equal(last.value, value) ? last.value : value;
    cache.current = { state, selector, value: kept };
    return kept;
  }, [store, selector, equal]);
  return useSyncExternalStore(store.subscribe, read, read);
}

/** The store's dispatch, stable for the store's lifetime. */
export function useDispatch(): (action: Action) => void {
  return useStore().dispatch;
}

/** Shallow equality for arrays and plain objects, for selectors that build a fresh container each time. */
export function shallowEqual<T>(a: T, b: T): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  const ka = Object.keys(a) as (keyof T)[];
  const kb = Object.keys(b) as (keyof T)[];
  if (ka.length !== kb.length) return false;
  return ka.every((k) => Object.is(a[k], b[k]));
}
