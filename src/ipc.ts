/**
 * The only file that names a Tauri command or an event.
 *
 * Everything below is typed against `src/bindings/`, which `cargo test`
 * generates from the Rust structs themselves - so a renamed field is a
 * TypeScript error here rather than an `undefined` at an event. The names
 * themselves are still strings on the wire, which is exactly why they are all in
 * one file: a renamed command breaks one line, in a place a reader is looking.
 *
 * What this replaces: nineteen `invoke("...")` calls and five `listen("...")`
 * calls spread across the components, each restating its own payload type by
 * hand against a Rust struct nobody checked it against. Three of those types had
 * already drifted - the share sheet's anchor had no TypeScript type at all.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { Anchor } from "./bindings/Anchor";
import type { Notice } from "./bindings/Notice";
import type { Orientation } from "./bindings/Orientation";
import type { Overlay } from "./bindings/Overlay";
import type { OverlayCard } from "./bindings/OverlayCard";
import type { Paper } from "./bindings/Paper";
import type { Payload } from "./bindings/Payload";
import type { Photo } from "./bindings/Photo";
import type { PrintSetup } from "./bindings/PrintSetup";
import type { Printer } from "./bindings/Printer";
import type { RuntimeStatus } from "./bindings/RuntimeStatus";
import type { Settings } from "./bindings/Settings";

/**
 * Every failing command rejects with this.
 *
 * The operator's sentence plus the catalogue code, which is the half that is new:
 * commands used to reject with a bare string, so the frontend could print a
 * failure but never branch on one. The console half stays in Rust and is logged
 * there - it is not on this side to leak.
 */
export type Problem = Payload;

/** Whether a rejection carries a catalogue code, as opposed to being a crash. */
export function isProblem(error: unknown): error is Problem {
  return (
    typeof error === "object" &&
    error !== null &&
    typeof (error as Problem).code === "string" &&
    typeof (error as Problem).message === "string"
  );
}

/**
 * What to show the operator for any rejection.
 *
 * A catalogued problem carries a sentence written for them. Anything else is a
 * bug rather than a situation, so it gets a sentence that says so instead of a
 * stringified exception - which is what the UI used to print.
 */
export function sentence(error: unknown): string {
  if (isProblem(error)) return error.message;
  console.error(error);
  return "Something went wrong inside Snappr. The details are in the log.";
}

export const cmd = {
  listPhotos: () => invoke<Photo[]>("list_photos"),
  selectedPhoto: () => invoke<string | null>("selected_photo"),
  setSelected: (source: string) => invoke<void>("set_selected", { source }),

  /**
   * Puts a yes/no question to the operator in a native sheet, and resolves to
   * what they clicked. The wording comes from `src/confirm.ts`, which is the
   * only file that spells one - call `ask` there, not this.
   *
   * Raised on the Rust side, like every other dialog in the app, which is why
   * `capabilities/default.json` grants no `dialog:*` permission to the webview.
   */
  ask: (title: string, question: string, confirm: string) =>
    invoke<boolean>("ask", { title, question, confirm }),

  getSettings: () => invoke<Settings>("get_settings"),
  setWatchDir: (dir: string) => invoke<Settings>("set_watch_dir", { dir }),
  /**
   * The folder picker, opening in `current` when it still exists. Resolves to
   * null when the operator dismissed it.
   *
   * `current` is `string | null` rather than optional so the caller has to pass
   * something: `JSON.stringify` drops an `undefined` value, and an argument that
   * reaches Rust as an absent key rather than a null is a difference nobody
   * wants to have to remember.
   */
  pickWatchDir: (current: string | null) =>
    invoke<string | null>("pick_watch_dir", { current }),
  setWatching: (enabled: boolean) => invoke<Settings>("set_watching", { enabled }),
  setPanelCollapsed: (collapsed: boolean) =>
    invoke<Settings>("set_panel_collapsed", { collapsed }),

  listOverlays: () => invoke<OverlayCard[]>("list_overlays"),
  setOverlay: (orientation: Orientation, choice: Overlay) =>
    invoke<Settings>("set_overlay", { orientation, choice }),
  addOverlay: (path: string) => invoke<Settings>("add_overlay", { path }),
  /** The SVG picker. Null when the operator dismissed it. */
  pickOverlayFile: () => invoke<string | null>("pick_overlay_file"),
  removeOverlay: (overlay: Overlay) => invoke<Settings>("remove_overlay", { overlay }),
  revealOverlay: (overlay: Overlay) => invoke<void>("reveal_overlay", { overlay }),

  reprocess: (source: string) => invoke<void>("reprocess", { source }),
  clearPhoto: (source: string) => invoke<void>("clear_photo", { source }),
  /** Returns how many cells went, so the caller never has to assume it was all. */
  clearAllPhotos: () => invoke<number>("clear_all_photos"),

  runtimeStatus: () => invoke<RuntimeStatus>("runtime_status"),
  /**
   * `setup` is the settings window's Test Print - an unsaved printer and paper.
   * Null means the saved ones, which is every other caller.
   */
  printPhoto: (source: string, setup: PrintSetup | null = null) =>
    invoke<void>("print_photo", { source, setup }),
  /** The operator's override for the print mark `printPhoto` sets by itself. */
  setPhotoPrinted: (source: string, printed: boolean) =>
    invoke<void>("set_photo_printed", { source, printed }),
  sharePhoto: (source: string, anchor: Anchor) =>
    invoke<void>("share_photo", { source, anchor }),
  revealPhoto: (source: string) => invoke<void>("reveal_photo", { source }),

  // --- The settings window.
  openSettings: () => invoke<void>("open_settings"),
  defaultSettings: () => invoke<Settings>("default_settings"),
  /**
   * Saves the window's form. Only the fields that window edits are taken; the
   * overlay slots and the watch toggle are left as the main window has them.
   */
  saveSettings: (form: Settings) => invoke<Settings>("save_settings", { form }),
  /** The sentences an in-use overlay would carry on a paper of this size. */
  checkOverlayShapes: (shortMm: number, longMm: number) =>
    invoke<string[]>("check_overlay_shapes", { shortMm, longMm }),
  listPrinters: () => invoke<Printer[]>("list_printers"),
  /** `""` is the system default printer. */
  listPapers: (printer: string) => invoke<Paper[]>("list_papers", { printer }),
  /** Resolves to where it was written, or null when the save panel was dismissed. */
  exportCliConfig: () => invoke<string | null>("export_cli_config"),
  revealSettingsFile: () => invoke<void>("reveal_settings_file"),
};

/**
 * The events Rust pushes. Each returns its own unsubscribe, awaited by the
 * caller - `listen` resolves on a microtask, so subscribing has to finish before
 * the first read or an event fires into nothing.
 */
export const on = {
  photo: (handle: (photo: Photo) => void) =>
    listen<Photo>("photo", (event) => handle(event.payload)),
  /** The source paths of cells that are gone. A removal cannot ride on `photo`. */
  cleared: (handle: (sources: string[]) => void) =>
    listen<string[]>("cleared", (event) => handle(event.payload)),
  runtime: (handle: (status: RuntimeStatus) => void) =>
    listen<RuntimeStatus>("runtime", (event) => handle(event.payload)),
  overlays: (handle: (cards: OverlayCard[]) => void) =>
    listen<OverlayCard[]>("overlays", (event) => handle(event.payload)),
  notice: (handle: (notice: Notice) => void) =>
    listen<Notice>("notice", (event) => handle(event.payload)),
  /**
   * The settings as the settings window saved them. Every other setter returns
   * the new settings to its own caller; this one is saved from a different
   * window than the one that has to redraw.
   */
  settings: (handle: (settings: Settings) => void) =>
    listen<Settings>("settings", (event) => handle(event.payload)),
};
