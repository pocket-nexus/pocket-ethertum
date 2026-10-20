// One 480×272 screen with a pad: the PS Vita, whose panel also takes taps,
// and a PSP. In play the world has the screen and the interface is the
// crosshair, the hotbar with the health bar over it and a few marks; a list
// comes up over the world for the title and the menu.
import { createEffect, createSignal, on, untrack } from "solid-js";
import { Show } from "solid-js";
import { Text, View } from "@pocketjs/framework/components";
import { glyph } from "@pocketjs/framework/modality";
import { createGame, createPulse, type Game } from "../game.ts";
import { connectHost } from "../host.ts";
import { Chip, createMenu, Crosshair, Fade, Health, Heading, Hotbar, HOTBAR_WIDTH, Keep, Legend, Loading, Marks, type Menu, Panel, Rows, SLOT_SIDE, Stats, Wordmark } from "../parts.tsx";
import { DIM, INK } from "../theme.ts";

const W = 480, H = 272;
const FOOTER = 24;
const ROW = 28;
/** A row that only informs: the ten controls fit a panel. */
const INFO = 20;
/** A list panel: its heading and the longest list (the seven settings) at `ROW`. */
const PANEL_H = 28 + 7 * ROW + 8;

export default function SingleScreen() {
  const host = connectHost();
  const game = createGame(host);
  const menu = createMenu(game);
  return (
    <View class="relative w-full h-full">
      <Show when={host.mode() === "loading" || host.mode() === "error"}><Loading host={host} width={W} height={H} /></Show>
      <Keep when={host.mode() === "title"} eager width={W} height={H}><Title game={game} menu={menu} /></Keep>
      <Keep when={host.mode() === "play" || host.mode() === "menu"} eager width={W} height={H}><Play game={game} /></Keep>
      <Keep when={host.mode() === "menu"} eager width={W} height={H}><Over game={game} menu={menu} /></Keep>
      {/* The strip stays built and is shown with a list: a list coming up builds nothing. */}
      <View class="absolute" style={{ insetL: 0, insetB: 0, display: game.listing() ? 0 : 1 }}>
        <Legend width={W} left={game.notice()} legend={menu.actions.legend()} />
      </View>
    </View>
  );
}

function Title(props: { game: Game; menu: Menu }) {
  const game = props.game;
  return (
    <View class="relative w-full h-full">
      <View class="absolute bg-gradient-to-r from-[#000000c0] to-[#00000000]" style={{ insetL: 0, insetT: 0, width: 320, height: H }} />
      {/* The name stays out from under a panel opened from the title. */}
      <View class="absolute flex-col" style={{ insetL: 24, insetT: 22, display: game.sheet() === "main" ? 0 : 1 }}>
        <Wordmark large />
        <Text class="text-xs" style={{ marginT: 6, textColor: DIM }}>A voxel sandbox to dig, build and fly through.</Text>
      </View>
      {/* The title's own rows, and the panel a list opened from them comes up in. Both stay built. */}
      <View class="absolute" style={{ insetL: 10, insetT: 112, display: game.sheet() === "main" ? 0 : 1 }}>
        <Rows game={game} menu={props.menu} width={190} rowHeight={ROW} active={() => game.mode() === "title" && game.sheet() === "main"} warm={[game.lists.title]} />
      </View>
      <Keep when={game.sheet() !== "main"} width={W} height={H - FOOTER}>
        <Panel width={W} height={H - FOOTER} panelWidth={340} panelHeight={PANEL_H}>
          <View class="flex-col">
            <Heading text={game.heading()} width={340} height={28} />
            <Rows game={game} menu={props.menu} width={340} rowHeight={ROW} infoHeight={INFO} active={() => game.mode() === "title" && game.sheet() !== "main"} />
          </View>
        </Panel>
      </Keep>
    </View>
  );
}

/** Play: the crosshair, the statistics, the marks, and the hotbar under the health bar. */
function Play(props: { game: Game }) {
  const host = props.game.host;
  const left = (W - HOTBAR_WIDTH) / 2;
  const bottom = 8;
  // What the buttons do shows when play begins, then leaves the view clear.
  const [hint, showHint] = createPulse(host, 6);
  createEffect(on(host.mode, (mode, before) => mode === "play" && before === "title" && showHint()));
  // A slot chosen, or the brush's size changed, is named for a moment over the health bar.
  const [named, showNamed] = createPulse(host, 1.5);
  const [name, setName] = createSignal("");
  const slotName = () => {
    const slot = untrack(host.hotbar)[untrack(host.slot)];
    if (!slot || slot.kind === 0) return "Empty";
    if (slot.kind === 1) return props.game.material(slot.id)?.name ?? `Material ${slot.id}`;
    return untrack(host.items).find((item) => item.id === slot.id)?.name ?? `Item ${slot.id}`;
  };
  createEffect(on(host.slot, () => host.mode() === "play" && (setName(slotName()), showNamed()), { defer: true }));
  createEffect(on(() => host.brush().size, (size) => host.mode() === "play" && (setName(`Brush ${size}`), showNamed()), { defer: true }));
  const stats = () => !!host.options().find((setting) => setting.key === "stats")?.value;
  return (
    <View class="relative w-full h-full">
      <Crosshair width={W} height={H} />
      <View class="absolute" style={{ insetL: 14, insetT: 12 }}><Stats host={host} on={stats()} /></View>
      <View class="absolute items-center justify-center" style={{ insetL: 0, insetT: 40, width: W }}>
        <Fade shown={hint()}><Chip text={`${glyph("select")} statistics · ${glyph("start")} menu`} /></Fade>
      </View>
      <View class="absolute items-center justify-center" style={{ insetL: 0, insetB: bottom + SLOT_SIDE + 16, width: W }}>
        <Fade shown={named()}><Text class="text-sm font-bold" style={{ textColor: INK }}>{name() || " "}</Text></Fade>
      </View>
      <View class="absolute" style={{ insetR: W - left + 8, insetB: bottom }}><Marks host={host} /></View>
      <View class="absolute" style={{ insetL: left, insetB: bottom + SLOT_SIDE + 6 }}><Health host={host} width={HOTBAR_WIDTH} /></View>
      <View class="absolute" style={{ insetL: left, insetB: bottom }}><Hotbar game={props.game} /></View>
    </View>
  );
}

/** The menu over play. */
function Over(props: { game: Game; menu: Menu }) {
  return (
    <Panel width={W} height={H - FOOTER} panelWidth={340} panelHeight={PANEL_H}>
      <View class="flex-col">
        <Heading text={props.game.heading()} width={340} height={28} />
        <Rows game={props.game} menu={props.menu} width={340} rowHeight={ROW} infoHeight={INFO} active={() => props.game.mode() === "menu"} warm={[props.game.lists.menu]} />
      </View>
    </Panel>
  );
}
