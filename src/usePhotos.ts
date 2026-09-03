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
   * Whether there is no selection at all, which is the whole of what decides if an
   * arriving photo may take one.
   *
   * **snappr picks a photo only when there is none to take away.** So the first
   * arrival of a session shows itself - an app whose out-of-the-box behaviour is
   * "nothing visibly happens" gives the operator nothing to react to - and every
   * arrival after that lands quietly and says so with a dot. See `unseen`, which is
   * what makes that safe: the notification used to *be* the switch, and the dot is
   * what replaced it.
   *
   * Two narrower tests were tried first and both are wrong, which is worth keeping
   * because both read as obviously right:
   *
   * - **"at the head of the strip"** - follow only while the selection is the newest
   *   photo. It is self-perpetuating: following an arrival makes that arrival the
   *   newest *and* selects it, so the test is true again immediately and the chain
   *   never breaks. An operator who does not deliberately arrow backwards sees the
   *   old behaviour unchanged, which is exactly how it was caught.
   * - **"focus is inside the strip"** - clicking a cell leaves focus there for the
   *   rest of the session, so following would switch off for good on the first
   *   click.
   *
   * A ref because the `photo` handler is registered once and cannot close over
   * state. It is updated after render, so the handler reads the state as it was
   * *before* this arrival, which is the question being asked.
   */
  const nothingSelected = useRef(true);
  useEffect(() => {
    nothingSelected.current = selected === null;
  }, [selected]);

  /**
   * Photos that landed while the operator was elsewhere in the strip and have not
   * been looked at since. Their cells carry a blue dot before the filename.
   *
   * This is the other half of `nothingSelected`. Following an arrival *was* the
   * notification;
   * once an arrival can land without taking the selection, something has to say it
   * happened, or staying on the photo in hand quietly costs the operator the next
   * one. The count in the zeroth cell going up is not enough - it does not say
   * *which*.
   *
   * Session state, deliberately not in the index: it is a fact about what this
   * operator has looked at, not about the photo. Persisting it would also mean
   * deciding what a restored strip is, and every answer there is wrong - the whole
   * strip dotted is a wall of blue, none of it dotted silently discards the mark on
   * the one photo they had not got to yet.
   */
  const [unseen, setUnseen] = useState<ReadonlySet<string>>(new Set());

  /**
   * Selecting a photo is looking at it, so the mark goes. Reconciled against the
   * selection rather than cleared wherever something selects - the same rule the
   * selection itself follows below - because the paths in are the keyboard, the
   * mouse, a right-click, a retry and the reconciliation, and a dot left on the
   * photo filling the window is the one state that reads as a bug.
   */
  useEffect(() => {
    if (selected === null || !unseen.has(selected)) return;
    setUnseen((current) => {
      const next = new Set(current);
      next.delete(selected);
      return next;
    });
  }, [selected, unseen]);

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
          // An arrival is either followed or marked, never both and never neither.
          // Following shows it, so there is nothing left to announce; a photo that
          // landed quietly has to say so itself, or the operator's own decision to
          // stay on the photo in hand would cost them the next one. Only the first
          // arrival of a session is followed - see `nothingSelected` and `unseen`.
          if (photo.status === "processing") {
            if (nothingSelected.current) setSelected(photo.source);
            else setUnseen((current) => new Set(current).add(photo.source));
          }
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
          // A cell that is gone cannot carry a dot, so its mark goes with it. The
          // same-object early return keeps a clear that touched nothing unseen from
          // costing a render.
          setUnseen((current) => {
            const left = [...current].filter((source) => !gone.has(source));
            return left.length === current.size ? current : new Set(left);
          });
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
    unseen,
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
