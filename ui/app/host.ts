// The renderer's state as signals, and the way to command it.
import { batch, createSignal, type Accessor } from "solid-js";
import { connectOverlay } from "@pocketjs/framework/overlay-host";
import type { Command, HostState } from "./protocol.ts";

/** Everything but the numbers that change in play. */
type Slow = Omit<HostState, "t">;
/** One signal per member: a statistics line once a second must not re-run
 *  what reads the hotbar. */
type Signals = { [K in keyof Slow]: Accessor<Slow[K]> };

const EMPTY = { kind: 0, id: 0, count: 0 } as const;

const initial: Slow = {
  mode: "loading", message: "", progress: 0, materials: [], items: [],
  hotbar: Array.from({ length: 9 }, () => ({ ...EMPTY })), slot: 0,
  health: 20, healthMax: 20, flying: false, sprinting: false,
  brush: { size: 4, strength: 80, shape: 0, material: 10 }, camera: 0, mods: 0,
  options: [], stats: "", prefs: "", woke: 0,
};

function same(a: unknown, b: unknown): boolean {
  return typeof a === "object" ? JSON.stringify(a) === JSON.stringify(b) : a === b;
}

export interface Host extends Signals {
  send(command: Command): void;
  /** True once the renderer has reported its state. */
  ready: Accessor<boolean>;
  /** The numbers in play as they last arrived (`T` indexes them). */
  t: number[];
  /** Calls `listener` now and in every turn new numbers arrive; returns how
   *  to stop. They change up to 30 times a second, so a listener writes
   *  straight to its nodes (`@pocketjs/framework/hot`), not to a signal. */
  onNumbers(listener: (t: number[]) => void): () => void;
}

export function connectHost(): Host {
  const [ready, setReady] = createSignal(false);
  const set = {} as { [K in keyof Slow]: (value: Slow[K]) => void };
  const listeners = new Set<(t: number[]) => void>();
  const host = {
    ready,
    t: [360, 0, 0, 0],
    onNumbers(listener) {
      listeners.add(listener);
      listener(host.t);
      return () => void listeners.delete(listener);
    },
  } as Host;
  const last = { ...initial };
  for (const key of Object.keys(initial) as (keyof Slow)[]) {
    const [get, put] = createSignal<unknown>(initial[key], { equals: false });
    (host as unknown as Record<string, unknown>)[key] = get;
    (set as unknown as Record<string, unknown>)[key] = put;
  }
  const overlay = connectOverlay<Partial<HostState>, Command>((state) => {
    // One line is one moment: what reads two members sees both changed.
    let slow = false;
    for (const key in state) {
      if (key === "t") host.t = state.t!;
      else slow = true;
    }
    // In play a line holds the numbers alone: no signal is written.
    if (slow || !ready()) {
      batch(() => {
        for (const key of Object.keys(state) as (keyof HostState)[]) {
          if (key === "t" || !(key in initial) || same(last[key], state[key])) continue;
          (last as Record<string, unknown>)[key] = state[key];
          (set[key] as (value: unknown) => void)(state[key]);
        }
        setReady(true);
      });
    }
    if (state.t) for (const listener of listeners) listener(host.t);
  });
  host.send = overlay.send;
  return host;
}
