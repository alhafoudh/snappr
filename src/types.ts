// Mirrors src-tauri/src/photo.rs. Kept hand-written rather than generated: the
// surface is small, and one file to update is cheaper than a codegen step.

export type Status = "queued" | "processing" | "done" | "failed";

export interface Previews {
  thumb: string;
  overlay: string;
  source: string;
}

/**
 * What the camera said about the photo. Every field is independently optional -
 * cameras vary wildly in what they write, and a screenshot or a scan carries none of
 * them, which is why `Photo.exif` itself is optional too.
 *
 * The strings arrive ready to print ("f/1.8", "1/120s", "6.8mm"): the tag-by-tag
 * knowledge of how to spell them lives in `Snappr::Exif`, next to the tags.
 */
export interface Exif {
  camera?: string;
  lens?: string;
  focal_length?: string;
  focal_length_35mm?: number;
  aperture?: string;
  shutter?: string;
  iso?: number;
  /** ISO 8601, and naive when the camera recorded no zone. */
  taken_at?: string;
}

export interface Photo {
  source: string;
  name: string;
  status: Status;
  /** When snappr first saw the file, which is not when the shutter fired - see exif. */
  added_at: number;
  destination?: string;
  previews?: Previews;
  orientation?: "landscape" | "portrait";
  output_width?: number;
  output_height?: number;
  depth?: number;
  bytes?: number;
  exif?: Exif;
  error?: string;
  warnings: string[];
}

export interface Settings {
  watch_dir: string;
  watch_enabled: boolean;
  /**
   * What each orientation composites. Landscape starts on the shipped sample,
   * portrait on nothing - there is no portrait sample drawn.
   */
  overlay_landscape: Overlay;
  overlay_portrait: Overlay;
  extensions: string[];
  output_suffix: string;
  background: string;
  png_compression: number;
  print_short_mm: number;
  print_long_mm: number;
  preview_max_px: number;
  preview_thumb_px: number;
  preview_quality: number;
  stability_checks: number;
  stability_timeout_secs: number;
  printer: string;
  /** The driver's PPD paper keyword, e.g. "Postcard.Fullbleed" on the SELPHY. */
  print_paper_name: string;
}

/**
 * How loud a problem is. An error means the thing the operator asked for did not
 * happen; a warning means it happened, differently.
 */
export type Level = "error" | "warning";

/**
 * A one-off message about something the operator just did.
 *
 * Carries only the sentence meant for them - the technical half was logged on the
 * Rust or Ruby side and deliberately never crosses this boundary. See
 * `src-tauri/src/problem.rs` and `lib/snappr/problem.rb`, which are the only two
 * places either half is written.
 */
export interface Notice {
  level: Level;
  text: string;
}

export interface RuntimeStatus {
  ready: boolean;
  /**
   * The pipeline is unusable. Only ever set when `ready` is false, which is the
   * only case `Stage` renders it - see the same note in `lib.rs`.
   */
  error?: string;
  /**
   * Watching was asked for and did not start. Shown on the watch pill, which
   * stays visible whatever the Stage is showing. Its own field rather than
   * sharing `notice`, because the pill offers a retry and must only offer it
   * when this is the reason.
   */
  watch_error?: string;
  /**
   * A one-off thing to say once, discovered before the window existed - today
   * only "your settings could not be read". Shown on the info line on mount.
   */
  notice?: string;
}

/** Which side of the overlayed/original toggle is showing. */
export type View = "overlay" | "source";

/** The two overlay slots, which are also the two crop shapes. */
export type Orientation = "landscape" | "portrait";

/**
 * What an orientation composites over the crop. Mirrors the adjacently-tagged
 * `Overlay` enum in `src-tauri/src/settings.rs`.
 *
 * `sample` is the one overlay snappr ships, and only landscape has one - a
 * portrait `sample` composites nothing, exactly like `none`. `none` is a real
 * setting: the photo is still cropped to the print ratio, with nothing drawn on it.
 */
export type Overlay =
  | { kind: "sample" }
  | { kind: "none" }
  | { kind: "custom"; path: string };
