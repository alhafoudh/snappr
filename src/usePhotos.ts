import { useCallback, useEffect, useRef, useState } from "react";

import { cmd, on } from "./ipc";
import type { Orientation, Overlay, OverlayCard, Photo, RuntimeStatus, Settings } from "./types";

/**
 * The whole of the app's state.
 *
 * Rust is the single source of truth and pushes every change as a `photo` event,
 * so nothing here polls and the list cannot drift from the index on disk. The reads
 * are the initial ones, which repopulate the filmstrip and the selection from the
 * persisted index on launch.
 */
export function usePhotos() {
  const [photos, setPhotos] = useState<Photo[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [runtime, setRuntime] = useState<RuntimeStatus>({ ready: false });
  const [selected, setSelected] = useState<string | null>(null);
  /**
   * The overlay panel's cards. A whole-list event rather than the per-record
   * merge `photo` needs: there are a handful of these, they are re-emitted as
   * each render lands, and Rust rebuilding the list is cheaper than either side
   * reconciling one.
   */
  const [overlays, setOverlays] = useState<OverlayCard[]>([]);

  /**
   * Whether the operator is still sitting at the head of the strip, which is what
   * decides whether an arriving photo may take the selection.
   *
   * Following arrivals is the kiosk's whole point - a photo AirDropped while nobody
   * is at the keyboard has to show itself. But it must not yank the operator out of
   * a photo they navigated to on purpose: the selection moved under them, and since
   * the strip's arrow keys carry on from wherever focus is, their place was gone.
   *
   * "At the head" rather than "not touching the keyboard": it re-arms itself by
   * arrowing back to the newest cell, it reads the same for the mouse and the
   * keyboard, and a followed arrival leaves the operator at the head again, so an
   * unattended run behaves exactly as it always did. Focus was the other candidate
   * and is wrong - clicking a cell leaves focus in the strip for the rest of the
   * session, so a focus test would switch following off for good on the first click.
   *
   * A ref because the `photo` handler is registered once and cannot close over
   * state. It is updated after render, so the handler reads where the operator was
   * *before* this arrival, which is the question being asked.
   */
  const atHead = useRef(true);
  useEffect(() => {
    atHead.current = selected === null || photos[photos.length - 1]?.source === selected;
  }, [photos, selected]);

  // Photos are keyed by source path, so an update to one that is already known
  // replaces it in place rather than appending a duplicate.
  const merge = useCallback((incoming: Photo) => {
    setPhotos((current) => {
      const at = current.findIndex((p) => p.source === incoming.source);
      if (at === -1) return [...current, incoming];
      const next = [...current];
      next[at] = incoming;
      return next;
    });
  }, []);

  useEffect(() => {
    let cancelled = false;
    const disposers: Array<() => void> = [];

    // StrictMode mounts twice in development, so cleanup can run while the awaits
    // below are still in flight. Anything that arrives after that has no one to
    // unsubscribe it, hence disposing on the spot.
    const keep = (off: () => void) => (cancelled ? off() : disposers.push(off));

    (async () => {
      // Subscribe BEFORE reading the current state, and await the subscriptions:
      // listen() resolves on a microtask, so fetching first leaves a window in
      // which an event fires and is lost.
      keep(
        await on.photo((photo) => {
          merge(photo);
          // Follow arrivals, so a photo that has just been AirDropped shows itself
          // without the operator touching anything - unless they have moved back
          // along the strip, in which case it lands quietly and leaves them where
          // they are. See `atHead`.
          if (photo.status === "processing" && atHead.current) setSelected(photo.source);
        }),
      );
      // Clearing is a removal, so it cannot travel on the `photo` event: there is
      // no record left to send. Rust names the sources it dropped and the strip
      // forgets them, which keeps the rule that the shell is the only thing that
      // decides what the filmstrip contains.
      keep(
        await on.cleared((sources) => {
          const gone = new Set(sources);
          setPhotos((current) => current.filter((photo) => !gone.has(photo.source)));
        }),
      );
      keep(await on.runtime(setRuntime));
      keep(await on.overlays(setOverlays));

      // Then pull. The pipeline is ready within a millisecond of launch, which is
      // long before this component has mounted, and Tauri does not replay events -
      // so without this read the UI would sit on "getting ready" forever.
      const [loaded, current, status, remembered, cards] = await Promise.all([
        cmd.listPhotos(),
        cmd.getSettings(),
        cmd.runtimeStatus(),
        cmd.selectedPhoto(),
        cmd.listOverlays(),
      ]);
      if (cancelled) return;

      setPhotos(loaded);
      setSettings(current);
      setRuntime(status);
      setOverlays(cards);
      // Reopen the cell the operator was last on. A remembered source that is no
      // longer in the list falls back to the newest, which is what the app did
      // before it remembered anything.
      if (loaded.length > 0) {
        const found = remembered && loaded.some((photo) => photo.source === remembered);
        setSelected(found ? remembered : loaded[loaded.length - 1].source);
      }
    })();

    return () => {
      cancelled = true;
      disposers.forEach((off) => off());
    };
  }, [merge]);

  // Selection is reconciled against the list rather than fixed up wherever a photo
  // is removed, so clearing one cell, clearing all of them and a record vanishing
  // for any other reason all land in the same place. The rule is the one used on
  // launch: the newest of what is left, and nothing at all once the strip is empty.
  useEffect(() => {
    if (selected === null || photos.some((photo) => photo.source === selected)) return;
    setSelected(photos.length > 0 ? photos[photos.length - 1].source : null);
  }, [photos, selected]);

  // Persist the selection from here rather than from a wrapped setSelected, so the
  // auto-follow of an arriving photo and the keyboard both go through one path.
  // Re-sending the value that was just restored is free: Rust only writes when it
  // actually changes.
  useEffect(() => {
    if (selected === null) return;
    cmd.setSelected(selected).catch((error) => {
      // Losing the selection across a restart is not worth interrupting a print for.
      console.error(error);
    });
  }, [selected]);

  // Both clears report failure by throwing - the caller shows it - because unlike
  // processing there is no photo record left to carry an error back on.
  const clearPhoto = useCallback(async (source: string) => {
    await cmd.clearPhoto(source);
  }, []);

  const clearAllPhotos = useCallback(async () => {
    await cmd.clearAllPhotos();
  }, []);

  const reprocess = useCallback((source: string) => {
    cmd.reprocess(source).catch(() => {
      /* the failure comes back as a photo event, so there is nothing to do here */
    });
  }, []);

  // Throws on failure, following the convention above: the caller shows it. It
  // had no catch anywhere for a while, so "that is not a folder" and any
  // settings-save failure were lost as unhandled rejections.
  const changeWatchDir = useCallback(async (dir: string) => {
    setSettings(await cmd.setWatchDir(dir));
  }, []);

  // One of the panel's overlays, or nothing - both are real settings. The next
  // photo reads the slots as it is queued, so it uses this; the ones already in
  // the strip keep the output they have.
  //
  // Nothing here has to clear the overlay off whatever card had it: a slot holds
  // one value, so setting it is the whole of switching it over. The `overlays`
  // event that follows carries the new toggle states for every card.
  const changeOverlay = useCallback(async (orientation: Orientation, choice: Overlay) => {
    setSettings(await cmd.setOverlay(orientation, choice));
  }, []);

  // Adding does not switch anything on - the card arrives with both toggles dark.
  // The rendered picture follows on the `overlays` event once the worker gets to it.
  const addOverlay = useCallback(async (path: string) => {
    setSettings(await cmd.addOverlay(path));
  }, []);

  // Removes the card and empties any slot that used it. Deletes nothing the
  // operator can see: the SVG is theirs and stays where it is.
  const removeOverlay = useCallback(async (overlay: Overlay) => {
    setSettings(await cmd.removeOverlay(overlay));
  }, []);

  const setPanelCollapsed = useCallback(async (collapsed: boolean) => {
    setSettings(await cmd.setPanelCollapsed(collapsed));
  }, []);

  const setWatching = useCallback(async (enabled: boolean) => {
    try {
      setSettings(await cmd.setWatching(enabled));
    } catch (error) {
      // The shell has already put the setting back to what it could actually
      // do, so re-read it and let the toggle show that rather than the wish -
      // and only then rethrow, so the caller can say why. This used to stop at
      // console.error, which left the toggle snapping back with no explanation
      // anywhere on screen.
      setSettings(await cmd.getSettings());
      throw error;
    }
  }, []);

  return {
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
  };
}
