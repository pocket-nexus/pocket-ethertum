// The pieces every presentation is built from. A device chooses where they
// go and how large they are; what the hotbar, the health bar, a list row or
// the crosshair looks like is decided here once.
//
// The numbers in play (the clock, the player's position) change up to 30
// times a second. Each is written straight to its node
// (`@pocketjs/framework/hot`) in a cell of fixed size, so a new value costs
// one native call and no layout. Everything else is a signal.
import { createEffect, createMemo, createSignal, For, Index, on, onCleanup, onMount, Show, untrack, type JSX } from "solid-js";
import { useActions, type ActionsHandle } from "@pocketjs/framework/actions";
import { Image, Text, View } from "@pocketjs/framework/components";
import type { SurfaceId } from "@pocketjs/framework/display";
import { createGesture } from "@pocketjs/framework/gesture";
import { text as hotText } from "@pocketjs/framework/hot";
import { BTN } from "@pocketjs/framework/input";
import { onFrame } from "@pocketjs/framework/lifecycle";
import { modality } from "@pocketjs/framework/modality";
import type { NodeMirror } from "@pocketjs/framework/renderer";
import { clock, iconPicture, read, tenths, tilePicture, type Game, type Row } from "./game.ts";
import type { Host } from "./host.ts";
import { T, type Slot } from "./protocol.ts";
import { ALARM, DIM, ETHER, FAINT, GLASS, HAIRLINE, INK, NIGHT, PANEL, PLATE, SLOT, TRACK, WASH } from "./theme.ts";

/**
 * A view a finger can tap, on the surface it is drawn on. It stays out of
 * the focus order: a pad has its own button for the same verb. While a
 * finger is on it the view dims, or with `onHeld` its owner is told and
 * draws the press itself.
 */
export function Touchable(props: { surface?: SurfaceId; onTap?: () => void; onHeld?: (held: boolean) => void; class?: string; style?: Record<string, number | string>; children?: JSX.Element }) {
  let node: NodeMirror | undefined;
  const [down, setDown] = createSignal(false);
  const hold = (held: boolean) => (props.onHeld ? props.onHeld(held) : setDown(held));
  createGesture({
    surface: props.surface,
    region: { node: () => node },
    onDown: () => hold(!!props.onTap),
    onUp: () => hold(false),
    onCancel: () => hold(false),
    onTap: () => props.onTap?.(),
  });
  return <View ref={node} class={props.class} style={{ ...props.style, opacity: down() ? 0.55 : 1 }}>{props.children}</View>;
}

/** A pill of text over the world. */
export function Chip(props: { text: string; color?: string }) {
  return (
    <View class="rounded px-2 py-1" style={{ bgColor: "#0a0d10b0" }}>
      <Text class="text-xs font-bold" style={{ textColor: props.color ?? INK }}>{props.text}</Text>
    </View>
  );
}

/** The name, as large as the surface has room for. */
export function Wordmark(props: { large?: boolean }) {
  return (
    <View class="flex-col">
      <Text class="text-sm font-bold tracking-wide" style={{ textColor: ETHER }}>POCKET</Text>
      <Show when={props.large} fallback={<Text class="text-2xl font-bold" style={{ textColor: INK }}>ETHERTUM</Text>}>
        <Text class="text-4xl font-bold" style={{ textColor: INK }}>ETHERTUM</Text>
      </Show>
    </View>
  );
}

/** The aim: a cross at the middle of the screen, white over a dark rim so it
 *  reads over sky and over snow. The ray the brush follows starts there. */
export function Crosshair(props: { width: number; height: number }) {
  const cx = props.width / 2, cy = props.height / 2;
  const bar = (w: number, h: number, color: string) => <View class="absolute" style={{ insetL: cx - w / 2, insetT: cy - h / 2, width: w, height: h, bgColor: color }} />;
  return (
    <>
      {bar(16, 4, "#00000080")}
      {bar(4, 16, "#00000080")}
      {bar(14, 2, INK)}
      {bar(2, 14, INK)}
    </>
  );
}

/** Side of a hotbar slot, its picture, and the gap between slots. */
export const SLOT_SIDE = 34;
export const PICTURE = 32;
export const SLOT_GAP = 2;
/** Width of the hotbar's nine slots. */
export const HOTBAR_WIDTH = 9 * SLOT_SIDE + 8 * SLOT_GAP;

/** One slot: its picture, its count at the lower right, and a white rim while selected. */
function SlotView(props: { game: Game; slot: () => Slot; index: number; selected: boolean; onTap: () => void }) {
  const host = props.game.host;
  const picture = createMemo(() => {
    const s = props.slot();
    if (s.kind === 1) return tilePicture(props.game.material(s.id)?.picture ?? s.id - 1);
    if (s.kind === 2) return iconPicture(host.items().find((item) => item.id === s.id)?.picture ?? s.id - 1);
    return "";
  });
  const inset = (SLOT_SIDE - PICTURE) / 2;
  return (
    <Touchable class="relative" style={{ width: SLOT_SIDE, height: SLOT_SIDE, bgColor: SLOT }} onTap={props.onTap}>
      <Show when={picture()}>
        <Image src={picture()} class="absolute" style={{ insetL: inset, insetT: inset, width: PICTURE, height: PICTURE }} />
      </Show>
      <Show when={props.slot().count > 0}>
        <Text class="absolute text-xs font-bold" style={{ insetR: 3, insetB: 1, textColor: "#bebebe" }}>{`${props.slot().count}`}</Text>
      </Show>
      <View class="absolute" style={{ insetL: 0, insetT: 0, width: SLOT_SIDE, height: SLOT_SIDE, borderWidth: 2, borderColor: INK, opacity: props.selected ? 1 : 0 }} />
    </Touchable>
  );
}

/** The hotbar: nine slots in a row. A finger chooses a slot as the d-pad does. */
export function Hotbar(props: { game: Game }) {
  const host = props.game.host;
  return (
    <View class="flex-row" style={{ width: HOTBAR_WIDTH, height: SLOT_SIDE, gap: SLOT_GAP }}>
      <Index each={host.hotbar()}>
        {(slot, index) => <SlotView game={props.game} slot={slot} index={index} selected={host.slot() === index} onTap={() => host.send({ type: "slot", index })} />}
      </Index>
    </View>
  );
}

/** Health as the reference draws it: a white bar over a dark track, the hotbar's width. */
export function Health(props: { host: Host; width: number }) {
  const part = () => Math.max(0, Math.min(1, props.host.health() / Math.max(1, props.host.healthMax())));
  return (
    <View class="relative rounded" style={{ width: props.width, height: 4, bgColor: TRACK }}>
      <View class="absolute rounded" style={{ insetL: 0, insetT: 0, width: Math.round(props.width * part()), height: 4, bgColor: part() < 0.3 ? ALARM : INK }} />
    </View>
  );
}

/** Marks for what the player is doing that the world does not show: flying, sprinting. */
export function Marks(props: { host: Host }) {
  const mark = (label: string) => (
    <View class="flex-row items-center gap-1 rounded px-2" style={{ height: 16, bgColor: "#0a0d10a0" }}>
      <View class="rounded-full w-[6] h-[6]" style={{ bgColor: ETHER }} />
      <Text class="text-xs font-bold tracking-wide" style={{ textColor: INK }}>{label}</Text>
    </View>
  );
  return (
    <View class="flex-col items-end gap-1">
      <Show when={props.host.flying()}>{mark("FLYING")}</Show>
      <Show when={props.host.sprinting()}>{mark("SPRINT")}</Show>
    </View>
  );
}

/** Dark glass a little larger than a readout, behind it. */
function Plate(props: { width: number; height: number }) {
  return <View class="absolute rounded" style={{ insetL: -6, insetT: -4, width: props.width + 12, height: props.height + 8, bgColor: PLATE }} />;
}

/**
 * The developer statistics, while their setting is on: the renderer's line,
 * and under it where the player is and the world's hour. The position is
 * written to its node at the left of a cell of fixed size.
 */
export function Stats(props: { host: Host; on: boolean }) {
  let place: NodeMirror | undefined;
  let shown = "";
  onMount(() => onCleanup(props.host.onNumbers((t) => {
    if (!props.on) return;
    const now = `XYZ ${tenths(t[T.x])} / ${tenths(t[T.y])} / ${tenths(t[T.z])} · ${clock(t[T.minutes])}`;
    if (now === shown) return;
    shown = now;
    hotText(place, now);
  })));
  return (
    <View class="relative flex-col" style={{ width: 300, display: props.on ? 0 : 1 }}>
      <Plate width={300} height={30} />
      <Text class="text-xs" style={{ width: 300, height: 15, textColor: DIM }}>{props.host.stats() || " "}</Text>
      <Text ref={place} class="text-xs" style={{ width: 300, height: 15, textColor: DIM }}>XYZ 0.0 / 0.0 / 0.0 · 06:00</Text>
    </View>
  );
}

/** The strip along the bottom of a screen with buttons: a notice at the left,
 *  what the buttons do at the right. */
export function Legend(props: { width: number; left?: string; legend: string }) {
  return (
    <View class="relative" style={{ width: props.width, height: 24, bgColor: GLASS }}>
      <View class="absolute" style={{ insetL: 0, insetT: 0, width: props.width, height: 1, bgColor: HAIRLINE }} />
      <Text class="absolute text-xs" style={{ insetL: 10, insetT: 5, textColor: DIM }}>{props.left ?? ""}</Text>
      <Text class="absolute text-xs" style={{ insetR: 10, insetT: 5, textColor: INK }}>{props.legend}</Text>
    </View>
  );
}

/** A list's focus, and the buttons that drive it. One for the life of a
 *  presentation: it watches the pad in play too, so a button held when a list
 *  opens is not taken for a press. */
export interface Menu {
  focus: () => number;
  press(index: number): void;
  actions: ActionsHandle;
}

export function createMenu(game: Game): Menu {
  const [focus, setFocus] = createSignal(0);
  const pressable = (row: Row | undefined) => !!row?.press;
  // A list starts on its first row that acts; a switch that changes under the focus keeps it.
  createEffect(on(() => `${game.mode()} ${game.sheet()}`, () => setFocus(Math.max(0, game.rows().findIndex(pressable)))));
  const press = (index: number) => {
    const row = game.rows()[index];
    if (!pressable(row)) return;
    setFocus(index);
    row!.press!();
  };
  const step = (by: number) => {
    const rows = game.rows();
    for (let at = focus() + by; at >= 0 && at < rows.length; at += by) {
      if (pressable(rows[at])) return setFocus(at);
    }
  };
  let previous = ~0;
  onFrame((buttons) => {
    const pressed = buttons & ~previous;
    previous = buttons;
    if (!game.listing()) return;
    if (pressed & BTN.UP) step(-1);
    if (pressed & BTN.DOWN) step(1);
    const row = game.rows()[focus()];
    if (row?.adjust && pressed & BTN.LEFT) row.adjust(-1);
    if (row?.adjust && pressed & BTN.RIGHT) row.adjust(1);
  });
  // The bindings are read every turn: built again only when the mode or the list changes.
  const actions = useActions(createMemo(() => ({
    confirm: { label: "select", run: () => press(focus()), when: () => game.listing() && pressable(game.rows()[focus()]) },
    back: { label: game.backLabel(), run: game.back, when: () => game.listing() && !!game.backLabel() },
    // START opens the menu over play, and closes it.
    media: { label: game.mode() === "play" ? "menu" : undefined, run: () => (game.mode() === "menu" ? game.back() : game.menu()), when: () => game.mode() === "play" || (game.mode() === "menu" && game.sheet() === "main") },
  })));
  return { focus, press, actions };
}

/** A switch: its knob at the right when on. */
function Switch(props: { on: boolean }) {
  return (
    <View class="relative rounded-full w-[34] h-[18]" style={{ bgColor: props.on ? ETHER : "#ffffff30" }}>
      <View class="absolute rounded-full w-[14] h-[14] transition-transform duration-150" style={{ insetL: 2, insetT: 2, translateX: props.on ? 16 : 0, bgColor: props.on ? NIGHT : INK }} />
    </View>
  );
}

/** What a row that acts shows: its label, at the right its switch or its value (with ‹ › where the
 *  d-pad steps it, and its picture), and a rule under it. */
export function Face(props: { row: Row; width: number; height: number; focused: () => boolean }) {
  const mid = () => props.height / 2 - 9;
  return (
    <>
      <Text class="absolute text-sm font-bold" style={{ insetL: 14, insetT: mid(), textColor: INK }}>{read(props.row.label)}</Text>
      <Show when={props.row.on}>
        <View class="absolute" style={{ insetR: 14, insetT: mid() }}><Switch on={props.row.on!()} /></View>
      </Show>
      <Show when={props.row.value !== undefined}>
        <View class="absolute flex-row items-center gap-2" style={{ insetR: 14, insetT: mid() }}>
          <Show when={props.row.picture}>
            <Image src={props.row.picture!()} style={{ width: 16, height: 16 }} />
          </Show>
          <Text class="text-sm" style={{ textColor: DIM }}>{props.row.adjust && props.focused() ? `‹ ${read(props.row.value!)} ›` : read(props.row.value!)}</Text>
        </View>
      </Show>
      <View class="absolute" style={{ insetL: 0, insetB: 0, width: props.width, height: 1, bgColor: HAIRLINE }} />
    </>
  );
}

/** The bar at the left of a list's mark; the mark itself is a wash a row tall. */
export function Mark(props: { height: number }) {
  return <View class="absolute" style={{ insetL: 0, insetT: 0, width: 3, height: props.height, bgColor: ETHER }} />;
}

/**
 * The rows of the list that is up. A row that acts takes a tap and the
 * list's mark: one bar, so moving it changes one node. A row that only
 * informs is a label and what it means, in a shorter line.
 *
 * Every list this view has shown stays built, hidden: showing one again
 * builds nothing, and a switch that turns writes its own row. `warm` lists
 * are built with the view, before they are first shown.
 */
export function Rows(props: { game: Game; menu: Menu; width: number; rowHeight: number; infoHeight?: number; surface?: SurfaceId; active?: () => boolean; warm?: Row[][] }) {
  const info = () => props.infoHeight ?? 22;
  const live = () => props.active?.() ?? true;
  const rows = createMemo<Row[]>((before) => (live() ? props.game.rows() : before), props.warm?.[0] ?? []);
  const focus = createMemo<number>((before) => (live() ? props.menu.focus() : before), 0);
  const built = createMemo<Row[][]>((before) => (!rows().length || before.includes(rows()) ? before : [...before, rows()]), props.warm ?? []);
  const [held, setHeld] = createSignal(-1);
  const marked = () => (modality.buttons ? focus() : held());
  return (
    <View class="relative flex-col" style={{ width: props.width }}>
      <Show when={rows().some((row) => row.press)}>
        <View class={modality.buttons ? "absolute transition-transform duration-100 ease-out" : "absolute"} style={{ insetL: 0, insetT: 0, width: props.width, height: props.rowHeight, translateY: Math.max(0, marked()) * props.rowHeight, display: marked() < 0 ? 1 : 0, bgColor: WASH }}>
          <Mark height={props.rowHeight} />
        </View>
      </Show>
      <For each={built()}>
        {(list) => {
          // A list keeps the row height it was built with.
          const height = untrack(() => props.rowHeight);
          return (
            <View class="flex-col" style={{ width: props.width, display: list === rows() ? 0 : 1 }}>
              <For each={list}>
                {(row, index) => (
                  <Show
                    when={row.press}
                    fallback={
                      <View class="relative" style={{ width: props.width, height: info() }}>
                        <Text class="absolute text-xs font-bold" style={{ insetL: 14, insetT: info() / 2 - 8, textColor: ETHER }}>{read(row.label)}</Text>
                        <Text class="absolute text-xs" style={{ insetL: 14 + Math.min(96, props.width * 0.3), insetT: info() / 2 - 8, textColor: DIM }}>{read(row.value ?? "")}</Text>
                      </View>
                    }
                  >
                    <Touchable surface={props.surface} class="relative" style={{ width: props.width, height }} onTap={() => props.menu.press(index())} onHeld={modality.buttons ? undefined : (on) => setHeld(on ? index() : -1)}>
                      <Face row={row} width={props.width} height={height} focused={() => list === rows() && focus() === index()} />
                    </Touchable>
                  </Show>
                )}
              </For>
            </View>
          );
        }}
      </For>
    </View>
  );
}

/**
 * A screen that is built the first time `when` holds (at once with `eager`)
 * and then stays, shown or hidden. Showing it again builds nothing.
 */
export function Keep(props: { when: boolean; eager?: boolean; width: number; height: number; children: JSX.Element }) {
  const [built, setBuilt] = createSignal(!!props.eager);
  createEffect(() => {
    if (props.when) setBuilt(true);
  });
  return (
    <View class="absolute" style={{ insetL: 0, insetT: 0, width: props.width, height: props.height, display: props.when ? 0 : 1, hitPass: 1 }}>
      <Show when={built()}>{props.children}</Show>
    </View>
  );
}

/** A list's heading over a rule. */
export function Heading(props: { text: string; width: number; height: number }) {
  return (
    <View class="relative" style={{ width: props.width, height: props.height }}>
      <Text class="absolute text-xs font-bold tracking-wide" style={{ insetL: 14, insetT: props.height / 2 - 8, textColor: DIM }}>{props.text}</Text>
      <View class="absolute" style={{ insetL: 0, insetB: 0, width: props.width, height: 1, bgColor: HAIRLINE }} />
    </View>
  );
}

/** A view that fades with `shown`. */
export function Fade(props: { shown: boolean; children: JSX.Element }) {
  return <View class={props.shown ? "opacity-100 transition-opacity duration-300" : "opacity-0 transition-opacity duration-300"}>{props.children}</View>;
}

/** A panel over a scrim, in the middle of a surface. No clip: the Vita's host draws one as a pass
 *  over the whole screen's stencil, and the rows fit the panel. */
export function Panel(props: { width: number; height: number; panelWidth: number; panelHeight: number; children: JSX.Element }) {
  return (
    <View class="absolute" style={{ insetL: 0, insetT: 0, width: props.width, height: props.height, bgColor: "#00000080" }}>
      <View class="absolute rounded-lg flex-row" style={{ insetL: (props.width - props.panelWidth) / 2, insetT: Math.max(4, (props.height - props.panelHeight) / 2), width: props.panelWidth, height: props.panelHeight, bgColor: PANEL, borderWidth: 1, borderColor: HAIRLINE }}>
        {props.children}
      </View>
    </View>
  );
}

/** The screen while the world is prepared, and when it could not be. */
export function Loading(props: { host: Host; width: number; height: number }) {
  const failed = () => props.host.mode() === "error";
  const BAR = 200;
  return (
    <View class="items-center justify-center flex-col gap-3" style={{ width: props.width, height: props.height, bgColor: NIGHT }}>
      <Wordmark />
      <Text class="text-xs" style={{ textColor: failed() ? ALARM : DIM }}>{props.host.message() || "Preparing the world"}</Text>
      <View class="relative rounded" style={{ width: BAR, height: 3, bgColor: "#ffffff20", display: failed() ? 1 : 0 }}>
        <View class="absolute rounded" style={{ insetL: 0, insetT: 0, width: Math.round((BAR * Math.max(0, Math.min(100, props.host.progress()))) / 100), height: 3, bgColor: ETHER }} />
      </View>
    </View>
  );
}

export { FAINT };
