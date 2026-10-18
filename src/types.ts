/**
 * The types the components use.
 *
 * **Everything that crosses the IPC boundary is re-exported from
 * `src/bindings/`, which `cargo test` generates from the Rust structs.** This
 * file used to declare all of it by hand - nineteen `Settings` fields, `Photo`,
 * `Exif`, `OverlayCard` - mirroring structs nothing checked it against. Three had
 * already drifted: `Photo.orientation` was narrowed here to a union the Rust side
 * did not guarantee, `mac::Anchor` had no type at all, and a `Settings` field
 * added in Rust was simply invisible.
 *
 * What is still written here is what is genuinely the frontend's: two unions that
 * describe UI state and exist nowhere in Rust.
 */

export type { Anchor } from "./bindings/Anchor";
export type { Exif } from "./bindings/Exif";
export type { Level } from "./bindings/Level";
export type { Notice } from "./bindings/Notice";
export type { Orientation } from "./bindings/Orientation";
export type { Overlay } from "./bindings/Overlay";
export type { OverlayCard } from "./bindings/OverlayCard";
export type { Paper } from "./bindings/Paper";
export type { Payload } from "./bindings/Payload";
export type { Photo } from "./bindings/Photo";
export type { Previews } from "./bindings/Previews";
export type { PrintSetup } from "./bindings/PrintSetup";
export type { Printer } from "./bindings/Printer";
export type { RuntimeStatus } from "./bindings/RuntimeStatus";
export type { Settings } from "./bindings/Settings";
export type { Status } from "./bindings/Status";
export type { Window } from "./bindings/Window";

/** Which side of the overlayed/original toggle is showing. */
export type View = "overlay" | "source";

/**
 * What is being dragged over the window, decided from the paths Tauri reports on
 * `enter`. SVGs are overlays and everything else is a photo, so the right target
 * can light up - the drop itself is routed the same way, on the Rust side.
 */
export type Drag = "photos" | "overlays" | null;
