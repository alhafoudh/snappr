import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { Photo, RuntimeStatus, Settings } from "./types";

/**
 * The whole of the app's state.
 *
 * Rust is the single source of truth and pushes every change as a `photo` event,
 * so nothing here polls and the list cannot drift from the index on disk. The one
 * read is the initial `list_photos`, which repopulates the filmstrip from the
 * persisted index on launch.
 */
export function usePhotos() {
  const [photos, setPhotos] = useState<Photo[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [runtime, setRuntime] = useState<RuntimeStatus>({ ready: false });
  const [selected, setSelected] = useState<string | null>(null);

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
        await listen<Photo>("photo", (event) => {
          merge(event.payload);
          // Follow arrivals, so a photo that has just been AirDropped shows itself
          // without the operator touching anything.
          if (event.payload.status === "processing") setSelected(event.payload.source);
        }),
      );
      // Clearing is a removal, so it cannot travel on the `photo` event: there is
      // no record left to send. Rust names the sources it dropped and the strip
      // forgets them, which keeps the rule that the shell is the only thing that
      // decides what the filmstrip contains.
      keep(
        await listen<string[]>("cleared", (event) => {
          const gone = new Set(event.payload);
          setPhotos((current) => current.filter((photo) => !gone.has(photo.source)));
        }),
      );
      keep(await listen<RuntimeStatus>("runtime", (event) => setRuntime(event.payload)));

      // Then pull. The sidecar becomes ready about a second after launch, which is
      // usually before this component has mounted at all, and Tauri does not replay
      // events - so without this read the UI would sit on "starting..." forever.
      const [loaded, current, status] = await Promise.all([
        invoke<Photo[]>("list_photos"),
        invoke<Settings>("get_settings"),
        invoke<RuntimeStatus>("runtime_status"),
      ]);
      if (cancelled) return;

      setPhotos(loaded);
      setSettings(current);
      setRuntime(status);
      // Land on the newest, which is what an operator wants after a restart.
      if (loaded.length > 0) setSelected(loaded[loaded.length - 1].source);
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

  // Both clears report failure by throwing - the caller shows it - because unlike
  // processing there is no photo record left to carry an error back on.
  const clearPhoto = useCallback(async (source: string) => {
    await invoke("clear_photo", { source });
  }, []);

  const clearAllPhotos = useCallback(async () => {
    await invoke<number>("clear_all_photos");
  }, []);

  const reprocess = useCallback((source: string) => {
    invoke("reprocess", { source }).catch(() => {
      /* the failure comes back as a photo event, so there is nothing to do here */
    });
  }, []);

  const changeWatchDir = useCallback(async (dir: string) => {
    setSettings(await invoke<Settings>("set_watch_dir", { dir }));
  }, []);

  const setWatching = useCallback(async (enabled: boolean) => {
    try {
      setSettings(await invoke<Settings>("set_watching", { enabled }));
    } catch (error) {
      // The shell has already put the setting back to what it could actually
      // do, so re-read it and let the toggle show that rather than the wish.
      console.error(error);
      setSettings(await invoke<Settings>("get_settings"));
    }
  }, []);

  return {
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
  };
}
