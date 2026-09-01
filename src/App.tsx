import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { Menu, type MenuOptions } from "@tauri-apps/api/menu";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm, open } from "@tauri-apps/plugin-dialog";

import { usePhotos } from "./usePhotos";
import { useNow } from "./useNow";
import { relativeTime } from "./relativeTime";
import type { Photo, View } from "./types";
import "./app.css";

/**
 * The filmstrip cell. Points at the 400px thumb, never the overlay PNG: Tauri's
 * asset protocol reads a whole file into memory per request, and the print output
 * is 10-25 MB and 16-bit when the source was HEIC.
 *
 * Selection and status are separate channels on purpose. Selection is the border;
 * status is a dot in the corner. An earlier version put both on the border, and a
 * selected photo that had a warning simply showed no selection.
 */
function Cell({
  photo,
  active,
  onSelect,
  onMenu,
}: {
  photo: Photo;
  active: boolean;
  onSelect: () => void;
  onMenu: (event: React.MouseEvent) => void;
}) {
  const status = photo.status === "failed" ? "failed" : photo.warnings.length > 0 ? "warned" : null;
  const tooltip = photo.error || photo.warnings[0] || photo.name;

  return (
    <button
      className={`cell ${active ? "is-selected" : ""}`}
      onClick={onSelect}
      onContextMenu={onMenu}
      title={tooltip}
      aria-pressed={active}
      data-source={photo.source}
    >
      <span className="cell__frame">
        {photo.previews ? (
          <img src={convertFileSrc(photo.previews.thumb)} alt="" draggable={false} />
        ) : (
          <span className={`cell__placeholder ${photo.status === "processing" ? "is-working" : ""}`} />
        )}
        {status && <span className={`cell__badge cell__badge--${status}`} aria-label={status} />}
      </span>
      <span className="cell__name">{photo.name}</span>
    </button>
  );
}

function folderName(path: string) {
  return path.replace(/\/+$/, "").split("/").pop() || path;
}

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

  // Lightroom's before/after key is \, so it is the one an operator's hands will
  // already know. Arrows move along the strip; Cmd+P prints, as everywhere.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        event.preventDefault();
        step(event.key === "ArrowLeft" ? -1 : 1);
      } else if (event.key === "\\") {
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

  const preview = current?.previews?.[view];

  return (
    <div className={`app ${dragging ? "is-dragging" : ""}`}>
      {/* data-tauri-drag-region is what makes a transparent titlebar's row
          draggable; without it the only grab handle is the sliver above it. */}
      <header className="bar" data-tauri-drag-region>
        <h1 className="bar__title">Snappr</h1>
        <button
          className={`bar__watch ${settings?.watch_enabled ? "is-on" : ""}`}
          onClick={() => settings && setWatching(!settings.watch_enabled)}
          disabled={!settings}
          aria-pressed={settings?.watch_enabled ?? false}
          title={settings?.watch_enabled ? "Watching the folder - click to pause" : "Paused - click to watch the folder"}
        >
          <span className="bar__dot" aria-hidden="true" />
          {settings?.watch_enabled ? "Watching" : "Paused"}
        </button>
        <button
          className="bar__dir"
          onClick={pickWatchDir}
          title={settings ? `Watching ${settings.watch_dir} - click to change` : ""}
        >
          <span className="bar__dir-icon" aria-hidden="true" />
          {settings ? folderName(settings.watch_dir) : "..."}
        </button>
        <span className="bar__count">
          {ordered.length} photo{ordered.length === 1 ? "" : "s"}
        </span>
        <div className="toggle" role="group" aria-label="Which version to show">
          <button className={view === "overlay" ? "is-on" : ""} onClick={() => setView("overlay")} disabled={!ready} aria-pressed={view === "overlay"}>
            Overlayed
          </button>
          <button className={view === "source" ? "is-on" : ""} onClick={() => setView("source")} disabled={!ready} aria-pressed={view === "source"}>
            Original
          </button>
        </div>
      </header>

      <main className="stage">
        <div className="stage__canvas">
          {(() => {
            if (!runtime.ready) {
              return (
                <p className="notice">
                  {runtime.error ?? "Starting the image pipeline..."}
                  {runtime.error && <span className="notice__hint">Run `mise run runtime` and restart the app.</span>}
                </p>
              );
            }
            if (!current) {
              return settings?.watch_enabled === false ? (
                <p className="notice">Watching is paused. Drop a photo here to overlay it.</p>
              ) : (
                <p className="notice">
                  AirDrop a photo to {settings ? folderName(settings.watch_dir) : "the watched folder"}, or drop one here, to get started.
                </p>
              );
            }
            if (preview) {
              return <img className="stage__image" src={convertFileSrc(preview)} alt={current.name} draggable={false} />;
            }
            if (current.status === "failed") {
              return (
                <p className="notice notice--failed">
                  <strong>{current.name}</strong>
                  {current.error}
                  <button className="button" onClick={() => reprocess(current.source)}>Try again</button>
                </p>
              );
            }
            return <p className="notice">Working on {current.name}...</p>;
          })()}
        </div>

        <footer className="actions">
          <div className="actions__info">
            {current?.warnings[0] && <span className="actions__warning" title={current.warnings.join("\n")}>{current.warnings[0]}</span>}
            {problem && <span className="actions__problem">{problem}</span>}
            {ready && !current.warnings.length && !problem && (
              <span className="actions__meta">
                {current.output_width} x {current.output_height} px, {current.depth}-bit
                {current.bytes ? `, ${(current.bytes / 1024 / 1024).toFixed(1)} MB` : ""}
                {", added "}
                <time dateTime={new Date(current.added_at * 1000).toISOString()} title={new Date(current.added_at * 1000).toLocaleTimeString()}>
                  {relativeTime(current.added_at, now)}
                </time>
              </span>
            )}
          </div>
          <div className="actions__buttons">
            <button
              className="button"
              onClick={() => current && reprocess(current.source)}
              disabled={!current || current.status === "processing" || current.status === "queued" || busy !== null}
              title="Run the overlay again for this photo"
            >
              {current?.status === "failed" ? "Try again" : "Reprocess"}
            </button>
            <button className="button" onClick={() => current && invoke("reveal_photo", { source: current.source }).catch((e) => setProblem(String(e)))} disabled={!ready} title="Show the PNG in Finder">
              Reveal
            </button>
            <button ref={shareButton} className="button" onClick={() => act("share")} disabled={!ready || busy !== null}>
              {busy === "share" ? "Preparing..." : "Share"}
            </button>
            <button className="button button--primary" onClick={() => act("print")} disabled={!ready || busy !== null} title="Print (Cmd+P)">
              {busy === "print" ? "Opening..." : "Print"}
            </button>
          </div>
        </footer>
      </main>

      <nav className="strip" aria-label="Photos" onContextMenu={stripMenu}>
        {ordered.map((photo) => (
          <Cell
            key={photo.source}
            photo={photo}
            active={photo.source === selected}
            onSelect={() => setSelected(photo.source)}
            onMenu={cellMenu(photo)}
          />
        ))}
      </nav>
    </div>
  );
}
