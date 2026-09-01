import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Flex, Grid } from "@radix-ui/themes";
import { invoke } from "@tauri-apps/api/core";
import { Menu, type MenuOptions } from "@tauri-apps/api/menu";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm, open } from "@tauri-apps/plugin-dialog";

import { usePhotos } from "./usePhotos";
import { useNow } from "./useNow";
import { Toolbar } from "./components/Toolbar";
import { Stage } from "./components/Stage";
import { Actions } from "./components/Actions";
import { Filmstrip } from "./components/Filmstrip";
import type { Photo, View } from "./types";

export default function App() {
  const {
    photos,
    settings,
    runtime,
    selected,
    setSelected,
    reprocess,
    clearPhoto,
    clearAllPhotos,
    changeWatchDir,
    setWatching,
  } = usePhotos();
  const [view, setView] = useState<View>("overlay");
  const [busy, setBusy] = useState<"share" | "print" | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const shareButton = useRef<HTMLButtonElement>(null);
  const contextMenu = useRef<Menu | null>(null);
  const now = useNow();

  // File drops. The shell owns them (dragDropEnabled is on), so the webview never
  // sees the drop and Rust does the processing; this only paints the target while
  // something is being dragged over the window. The preventDefault pair is
  // insurance for the day someone turns dragDropEnabled off again: without it
  // WKWebView navigates to the dropped file and the app is gone.
  useEffect(() => {
    const swallow = (event: DragEvent) => event.preventDefault();
    window.addEventListener("dragover", swallow);
    window.addEventListener("drop", swallow);

    let cancelled = false;
    let unlisten: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        setDragging(event.payload.type === "enter" || event.payload.type === "over");
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
  const ordered = useMemo(() => [...photos].sort((a, b) => b.added_at - a.added_at), [photos]);
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
  // straight off the edge of.
  useEffect(() => {
    if (!selected) return;
    document
      .querySelector(`[data-source="${CSS.escape(selected)}"]`)
      ?.scrollIntoView({ inline: "nearest", block: "nearest", behavior: "smooth" });
  }, [selected]);

  const act = useCallback(
    async (kind: "share" | "print") => {
      if (!current || !ready || busy) return;
      setBusy(kind);
      setProblem(null);
      try {
        if (kind === "print") {
          await invoke("print_photo", { source: current.source });
        } else {
          // The sheet is anchored to the button, so it needs to know where that is.
          const box = shareButton.current?.getBoundingClientRect();
          const anchor = box
            ? { x: box.left, y: box.top, width: box.width, height: box.height }
            : { x: window.innerWidth / 2, y: window.innerHeight / 2, width: 1, height: 1 };
          await invoke("share_photo", { source: current.source, anchor });
        }
      } catch (error) {
        setProblem(String(error));
      } finally {
        setBusy(null);
      }
    },
    [current, ready, busy],
  );

  // ` toggles overlayed against original. Lightroom's before/after key is \, but
  // ` is the one directly under the operator's left little finger without a
  // reach, which matters when the gesture is used dozens of times an hour.
  // Arrows move along the strip, Tab does too, and Cmd+P prints as everywhere.
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
        // Tab is the filmstrip's next/previous, and the browser's focus
        // traversal is suppressed outright rather than left to walk the toolbar.
        // There is no form here to tab through: every control is a single button
        // the operator reaches with the mouse, and at an event the one thing
        // worth having under a thumb key is the next photo. Unguarded, because
        // this has to hold wherever focus happens to be sitting.
        event.preventDefault();
        step(event.shiftKey ? -1 : 1);
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
        setProblem(String(error));
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
      setProblem(String(error));
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
    if (typeof chosen === "string") await changeWatchDir(chosen);
  };

  const reveal = () => {
    if (!current) return;
    invoke("reveal_photo", { source: current.source }).catch((error) => setProblem(String(error)));
  };

  return (
    <Grid rows="44px minmax(0, 1fr) 140px" height="100vh">
      <Toolbar
        settings={settings}
        count={ordered.length}
        view={view}
        onView={setView}
        viewEnabled={ready}
        onToggleWatching={() => settings && setWatching(!settings.watch_enabled)}
        onPickWatchDir={pickWatchDir}
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
          onReprocess={reprocess}
          onReveal={reveal}
          onShare={() => act("share")}
          onPrint={() => act("print")}
        />
      </Flex>

      <Filmstrip
        photos={ordered}
        selected={selected}
        onSelect={setSelected}
        onCellMenu={cellMenu}
        onStripMenu={stripMenu}
      />
    </Grid>
  );
}
