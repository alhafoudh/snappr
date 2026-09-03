import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Box, Flex, Grid } from "@radix-ui/themes";
import { Menu, type MenuOptions } from "@tauri-apps/api/menu";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm, open } from "@tauri-apps/plugin-dialog";

import { usePhotos } from "./usePhotos";
import { useNow } from "./useNow";
import { Toolbar, TOOLBAR_HEIGHT } from "./components/Toolbar";
import { Stage } from "./components/Stage";
import { Actions } from "./components/Actions";
import { Filmstrip } from "./components/Filmstrip";
import { OverlayPanel, PANEL_WIDTH, RAIL_WIDTH } from "./components/OverlayPanel";
import { cmd, on, sentence } from "./ipc";
import type { Anchor, Drag, Notice, Orientation, OverlayCard, Photo, View } from "./types";

export default function App() {
  const {
    photos,
    settings,
    runtime,
    selected,
    overlays,
    setSelected,
    reprocess,
    clearPhoto,
    clearAllPhotos,
    changeWatchDir,
    changeOverlay,
    addOverlay,
    removeOverlay,
    setPanelCollapsed,
    setWatching,
  } = usePhotos();
  const [view, setView] = useState<View>("overlay");
  const [busy, setBusy] = useState<"share" | "print" | null>(null);
  /**
   * The last thing that went wrong with something the operator asked for.
   *
   * A level rather than a bare string because not everything here is a failure:
   * a print that fell back to custom paper still printed, and amber says that
   * where red would have the operator reaching for Try again.
   */
  const [problem, setProblem] = useState<Notice | null>(null);
  const [dragging, setDragging] = useState<Drag>(null);
  const shareButton = useRef<HTMLButtonElement>(null);
  const contextMenu = useRef<Menu | null>(null);
  const now = useNow();

  /**
   * Whatever a command rejected with.
   *
   * A rejection now carries the catalogue's own sentence plus a code, so `sentence`
   * hands back the operator's half and keeps anything uncatalogued - which is a
   * bug, not a situation - from reaching the screen as a stringified exception.
   * This used to be `String(error)` over a bare string.
   */
  const fail = useCallback(
    (error: unknown) => setProblem({ level: "error", text: sentence(error) }),
    [],
  );

  /**
   * Notices pushed from Rust rather than returned from a command, for an outcome
   * discovered after the command had already returned. Today that is only
   * `mac::print`'s main-thread closure - an unreadable print file, and the
   * printer having no paper of the right size - both of which used to be logged
   * and shown nowhere, so pressing Print did nothing visible at all.
   */
  useEffect(() => {
    let cancelled = false;
    let off: (() => void) | undefined;
    on.notice(setProblem).then((stop) => (cancelled ? stop() : (off = stop)));
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  /**
   * A problem belongs to the photo it happened to. Without this a failed print's
   * red line sits under whatever the operator selects next and reads as if *that*
   * photo is broken.
   *
   * Only when the selection moves *between* photos, though. The first selection
   * of the session arrives from `null` a moment after mount, and clearing on
   * that would swallow the boot notice below before it had been read.
   */
  const lastSelected = useRef(selected);
  useEffect(() => {
    if (lastSelected.current !== null && lastSelected.current !== selected) setProblem(null);
    lastSelected.current = selected;
  }, [selected]);

  /**
   * The one-off that happened before the window existed - today only "your
   * settings could not be read and have been reset". It arrives on the runtime
   * status rather than as an event, because Tauri does not replay events and
   * this is discovered in `.setup()`.
   *
   * Said once, and then it behaves like any other transient message: the next
   * action or selection clears it. `said` is a ref rather than state so a later
   * runtime update - the sidecar coming up, the watch failing - cannot bring it
   * back after the operator has moved on.
   */
  const notified = useRef(false);
  useEffect(() => {
    if (!runtime.notice || notified.current) return;
    notified.current = true;
    setProblem({ level: "warning", text: runtime.notice });
  }, [runtime.notice]);

  // File drops. The shell owns them (dragDropEnabled is on), so the webview never
  // sees the drop and Rust does the processing; this only paints the target while
  // something is being dragged over the window. The preventDefault pair is
  // insurance for the day someone turns dragDropEnabled off again: without it
  // WKWebView navigates to the dropped file and the app is gone.
  //
  // The *kind* of drag matters now, because a drop can be either a photo or an
  // overlay. It is decided from the paths - SVGs are overlays - and never from
  // where the pointer is: Rust routes the drop the same way, and it has to,
  // because a drop on the Dock icon has no position at all. So a drop target
  // shaped like the panel would be a promise the Dock could not keep.
  //
  // `enter` is the only event carrying paths, so the classification is kept and
  // `over` leaves it alone rather than resetting it.
  useEffect(() => {
    const swallow = (event: DragEvent) => event.preventDefault();
    window.addEventListener("dragover", swallow);
    window.addEventListener("drop", swallow);

    let cancelled = false;
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter") {
          const svgs = event.payload.paths.filter((path) => path.toLowerCase().endsWith(".svg"));
          // A mixed drop counts as photos: that is the half with somewhere to
          // land on screen, and both halves are handled either way.
          setDragging(svgs.length === event.payload.paths.length && svgs.length > 0 ? "overlays" : "photos");
        } else if (event.payload.type !== "over") {
          setDragging(null);
        }
      })
      .then((off) => (cancelled ? off() : (unlisten = off)));

    return () => {
      cancelled = true;
      unlisten?.();
      window.removeEventListener("dragover", swallow);
      window.removeEventListener("drop", swallow);
    };
  }, []);

  // Newest first, so the strip is left-aligned and each photo that arrives lands
  // at the left-hand end - the operator's eye never has to follow the newest cell
  // along the strip. `ordered` is the visual order, so the arrow keys, `step` and
  // the scroll-into-view below all follow from it without a second index.
  //
  // **Reversed, not sorted on `added_at`, and that is a bug fix rather than a
  // tidy-up.** `added_at` is whole seconds, so photos landing in the same second
  // tie, and a stable sort leaves a tie in `photos` order - oldest first, inverted
  // against the rest of the strip. A batch of arrivals therefore read backwards
  // inside themselves and the arrow keys walked that: Right from the newest cell
  // jumped to the oldest of the batch. Arrival order is the ground truth on both
  // sides - `Index::upsert` pushes, `list_photos` sorts nothing, `merge` appends and
  // replaces in place - and `added_at` is derived from it, so reversing is the same
  // ordering at full resolution. Do not put the sort back.
  const ordered = useMemo(() => [...photos].reverse(), [photos]);
  const current = ordered.find((p) => p.source === selected) ?? null;
  const ready = current?.status === "done" && !!current.previews;

  const step = useCallback(
    (by: number) => {
      if (ordered.length === 0) return;
      const at = ordered.findIndex((p) => p.source === selected);
      const next = Math.min(Math.max((at === -1 ? 0 : at) + by, 0), ordered.length - 1);
      setSelected(ordered[next].source);
    },
    [ordered, selected, setSelected],
  );

  // Keep the selected cell on screen, which the keyboard would otherwise walk
  // straight off the edge of - and keep the strip's *focus* on it too.
  //
  // The second half is not cosmetic. RadioCards navigates from the focused item and
  // never from the checked one - `react-roving-focus` computes the next cell as
  // `candidateNodes.indexOf(event.currentTarget)` - so a selection changed by
  // anything but an arrow press leaves focus behind on the old cell, and the next
  // arrow press steps from there. That reads as the selection teleporting into the
  // middle of the strip, and an arriving photo did it every time.
  //
  // Only when focus is already inside the strip, and inside the radiogroup this very
  // cell belongs to: otherwise an arrival would snatch focus off the button the
  // operator was reaching for.
  useEffect(() => {
    if (!selected) return;
    const cell = document.querySelector(`[data-source="${CSS.escape(selected)}"]`);
    if (!(cell instanceof HTMLElement)) return;
    cell.scrollIntoView({ inline: "nearest", block: "nearest", behavior: "smooth" });

    const strip = cell.closest('[role="radiogroup"]');
    if (strip?.contains(document.activeElement) && document.activeElement !== cell) {
      // preventScroll because the smooth scroll above owns the scrolling. On an
      // ordinary arrow press Radix has already focused this cell, so the guard above
      // makes this a no-op rather than a second focus event.
      cell.focus({ preventScroll: true });
    }
  }, [selected]);

  const act = useCallback(
    async (kind: "share" | "print") => {
      if (!current || !ready || busy) return;
      setBusy(kind);
      setProblem(null);
      try {
        if (kind === "print") {
          await cmd.printPhoto(current.source);
        } else {
          // The sheet is anchored to the button, so it needs to know where that
          // is. `Anchor` is a generated type now, so a renamed field on the Rust
          // side is an error here rather than a share sheet that opens in the
          // wrong place.
          const box = shareButton.current?.getBoundingClientRect();
          const anchor: Anchor = box
            ? { x: box.left, y: box.top, width: box.width, height: box.height }
            : { x: window.innerWidth / 2, y: window.innerHeight / 2, width: 1, height: 1 };
          await cmd.sharePhoto(current.source, anchor);
        }
      } catch (error) {
        fail(error);
      } finally {
        setBusy(null);
      }
    },
    [current, ready, busy],
  );

  // ` toggles overlayed against original. Lightroom's before/after key is \, but
  // ` is the one directly under the operator's left little finger without a
  // reach, which matters when the gesture is used dozens of times an hour.
  // Arrows move along the strip, Tab does nothing, and Cmd+P prints as everywhere.
  //
  // The filmstrip is a radiogroup and the view toggle a toggle group, and both own
  // the arrow keys for their own roving focus while they have it. Without the
  // guard, one arrow press inside the strip both moves the radiogroup's selection
  // and steps this handler, and the selection jumps two cells at a time.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      const inWidget = target?.closest('[role="radiogroup"], [role="group"], input, textarea');

      if (event.key === "Tab") {
        // Tab is swallowed and does nothing at all. The branch exists purely to
        // suppress the browser's focus traversal rather than leave it to walk the
        // toolbar: there is no form here to tab through, every control is a single
        // button the operator reaches with the mouse, and the arrows are the
        // strip's navigation. Do not delete it as an empty branch - without the
        // preventDefault the focus ring is back. Unguarded, because this has to
        // hold wherever focus happens to be sitting.
        event.preventDefault();
      } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        if (inWidget) return;
        event.preventDefault();
        step(event.key === "ArrowLeft" ? -1 : 1);
      } else if (event.key === "`") {
        event.preventDefault();
        setView((v) => (v === "overlay" ? "source" : "overlay"));
      } else if (event.key === "p" && event.metaKey) {
        event.preventDefault();
        act("print");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [step, act]);

  // A native menu rather than an HTML one: this is a macOS app, and the OS gets
  // placement, keyboard handling and dismissal right for nothing.
  //
  // The menu is closed on the *next* right-click, not as soon as popup() resolves.
  // Each item's action arrives over a channel belonging to the menu's resource on
  // the Rust side, so freeing it the instant the menu is dismissed races the click
  // that dismissed it. Holding one alive bounds the cost to a single resource.
  const popup = useCallback(async (items: MenuOptions["items"]) => {
    const menu = await Menu.new({ items });
    void contextMenu.current?.close();
    contextMenu.current = menu;
    // No position argument: Tauri pops the menu at the pointer, which is where a
    // context menu belongs and is one fewer coordinate space to get wrong.
    await menu.popup();
  }, []);

  useEffect(() => () => void contextMenu.current?.close(), []);

  const clearOne = useCallback(
    async (photo: Photo) => {
      setProblem(null);
      try {
        await clearPhoto(photo.source);
      } catch (error) {
        fail(error);
      }
    },
    [clearPhoto],
  );

  // Clearing one cell asks nothing: it is undone by dropping the photo back on the
  // window. Clearing the strip at an event throws away the whole session's worth of
  // cells at once, so that one asks first. Neither deletes anything on disk, and
  // the wording says so - the point of clearing is a filmstrip the operator can see
  // the current queue in, not housekeeping.
  const clearEverything = useCallback(async () => {
    const count = ordered.length;
    if (count === 0) return;
    const ok = await confirm(
      `Clear ${count} photo${count === 1 ? "" : "s"} from the filmstrip? The photos and any prints already made stay on disk.`,
      { title: "Clear the filmstrip", kind: "warning", okLabel: "Clear All", cancelLabel: "Cancel" },
    );
    if (!ok) return;

    setProblem(null);
    try {
      await clearAllPhotos();
    } catch (error) {
      fail(error);
    }
  }, [ordered.length, clearAllPhotos]);

  const clearAllItem = () => ({
    id: "clear-all",
    text: "Clear All",
    enabled: ordered.length > 0,
    action: () => void clearEverything(),
  });

  // Right-clicking a cell selects it first, the way Finder does, so the menu can
  // never turn out to have acted on a photo other than the one under the pointer.
  const cellMenu = (photo: Photo) => (event: React.MouseEvent) => {
    event.preventDefault();
    // Without this the strip's handler fires on the way up and pops a second menu.
    event.stopPropagation();
    setSelected(photo.source);
    void popup([
      { id: "clear", text: "Clear", action: () => void clearOne(photo) },
      { item: "Separator" },
      clearAllItem(),
    ]);
  };

  // "Clear All" is on the cell's menu as well as the strip's because a full strip
  // leaves almost nothing to right-click: the cells cover their own background, so
  // a strip-only menu would be unreachable exactly when it is most wanted.
  const stripMenu = (event: React.MouseEvent) => {
    event.preventDefault();
    void popup([clearAllItem()]);
  };

  const pickWatchDir = async () => {
    const chosen = await open({ directory: true, multiple: false, defaultPath: settings?.watch_dir });
    if (typeof chosen !== "string") return;
    setProblem(null);
    try {
      await changeWatchDir(chosen);
    } catch (error) {
      fail(error);
    }
  };

  // The toggle can fail - the folder may have gone since it was picked - and the
  // shell puts the setting back when it does, so without the catch the switch
  // snapped back to Paused with nothing anywhere saying why.
  //
  // In the failed state `watch_enabled` is still true: the operator asked for
  // watching and it did not start. Toggling from that would *pause*, which is
  // the opposite of the retry the pill offers - hence the explicit `true`.
  const toggleWatching = async () => {
    if (!settings) return;
    setProblem(null);
    const next = runtime.watch_error ? true : !settings.watch_enabled;
    try {
      await setWatching(next);
    } catch (error) {
      fail(error);
    }
  };

  // Lighting a card's orientation icon is a plain slot assignment: a slot holds
  // one value, so setting it is also what turns the previous card's icon off.
  // There is nothing here that has to go and clear the other cards.
  const toggleOverlay = useCallback(
    async (card: OverlayCard, orientation: Orientation, on: boolean) => {
      setProblem(null);
      try {
        await changeOverlay(orientation, on ? card.overlay : { kind: "none" });
      } catch (error) {
        fail(error);
      }
    },
    [changeOverlay],
  );

  // Adds a card, and only that - the operator lights the orientation they want
  // afterwards. An add that quietly took a slot would overwrite a choice they had
  // already made.
  const pickOverlay = async () => {
    const chosen = await open({ multiple: false, filters: [{ name: "SVG", extensions: ["svg"] }] });
    if (typeof chosen !== "string") return;
    setProblem(null);
    try {
      await addOverlay(chosen);
    } catch (error) {
      fail(error);
    }
  };

  const forgetOverlay = async (card: OverlayCard) => {
    setProblem(null);
    try {
      await removeOverlay(card.overlay);
    } catch (error) {
      fail(error);
    }
  };

  // Native, like the filmstrip's - see the note on popup() above. Remove is absent
  // rather than disabled on the sample: it lives in the app bundle and is not the
  // operator's to delete, and a greyed-out item invites a click that can never
  // work.
  const cardMenu = (card: OverlayCard) => (event: React.MouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    void popup([
      {
        id: "reveal-overlay",
        text: "Reveal in Finder",
        enabled: !card.missing,
        action: () => {
          cmd.revealOverlay(card.overlay).catch(fail);
        },
      },
      ...(card.removable
        ? [
            { item: "Separator" as const },
            { id: "remove-overlay", text: "Remove", action: () => void forgetOverlay(card) },
          ]
        : []),
    ]);
  };

  const collapsePanel = async (collapsed: boolean) => {
    try {
      await setPanelCollapsed(collapsed);
    } catch (error) {
      fail(error);
    }
  };

  const reveal = () => {
    if (!current) return;
    cmd.revealPhoto(current.source).catch((error) => fail(error));
  };

  // The bar and the strip span both columns; only the middle row is split. The
  // overlay panel belongs beside the *photo* - it is what will be drawn over it -
  // while the strip stays full width so the zeroth cell sits at the window's edge
  // and the photos run the whole way across.
  return (
    <Grid
      rows={`${TOOLBAR_HEIGHT} minmax(0, 1fr) 140px`}
      columns={`${(settings?.overlays_collapsed ?? false) ? RAIL_WIDTH : PANEL_WIDTH} minmax(0, 1fr)`}
      height="100vh"
    >
      <Box gridColumn="1 / -1">
        <Toolbar />
      </Box>

      <OverlayPanel
        cards={overlays}
        settings={settings}
        dragging={dragging === "overlays"}
        onToggle={(card, orientation, on) => void toggleOverlay(card, orientation, on)}
        onAdd={() => void pickOverlay()}
        onCardMenu={cardMenu}
        onCollapse={(collapsed) => void collapsePanel(collapsed)}
      />

      <Flex direction="column" minHeight="0">
        <Stage
          runtime={runtime}
          settings={settings}
          current={current}
          view={view}
          dragging={dragging}
          onReprocess={reprocess}
        />
        <Actions
          ref={shareButton}
          current={current}
          ready={ready}
          busy={busy}
          problem={problem}
          now={now}
          view={view}
          onView={setView}
          onReprocess={reprocess}
          onReveal={reveal}
          onShare={() => act("share")}
          onPrint={() => act("print")}
        />
      </Flex>

      <Box gridColumn="1 / -1" minHeight="0">
        <Filmstrip
          photos={ordered}
          selected={selected}
          settings={settings}
          runtime={runtime}
          count={ordered.length}
          onSelect={setSelected}
          onCellMenu={cellMenu}
          onStripMenu={stripMenu}
          onToggleWatching={() => void toggleWatching()}
          onPickWatchDir={pickWatchDir}
        />
      </Box>
    </Grid>
  );
}
