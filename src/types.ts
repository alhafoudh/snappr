// Mirrors src-tauri/src/photo.rs. Kept hand-written rather than generated: the
// surface is small, and one file to update is cheaper than a codegen step.

export type Status = "queued" | "processing" | "done" | "failed";

export interface Previews {
  thumb: string;
  overlay: string;
  source: string;
}

export interface Photo {
  source: string;
  name: string;
  status: Status;
  added_at: number;
  destination?: string;
  previews?: Previews;
  orientation?: "landscape" | "portrait";
  output_width?: number;
  output_height?: number;
  depth?: number;
  bytes?: number;
  error?: string;
  warnings: string[];
}

export interface Settings {
  watch_dir: string;
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
