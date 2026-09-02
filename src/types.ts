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
   * The branding, one SVG per orientation. `null` is a real setting and the
   * default of a fresh install: that orientation prints the crop with nothing
   * composited over it. snappr ships no branding of its own.
   */
  overlay_svg_landscape: string | null;
  overlay_svg_portrait: string | null;
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

export interface RuntimeStatus {
  ready: boolean;
  error?: string;
}

/** Which side of the overlayed/original toggle is showing. */
export type View = "overlay" | "source";

/** The two overlay slots, which are also the two crop shapes. */
export type Orientation = "landscape" | "portrait";
