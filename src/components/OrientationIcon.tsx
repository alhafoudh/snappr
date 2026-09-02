import type { Orientation } from "../types";

/**
 * The two paper shapes, drawn rather than imported: @radix-ui/react-icons has no
 * landscape/portrait pair, and two rectangles say it better than any glyph would.
 * `PrinterIcon` in `Actions.tsx` is the same pattern for the same reason.
 *
 * Shared by everything that names an orientation - the overlay panel's toggles and
 * its collapsed rail, the info line under the photo, and the filmstrip cell - so
 * that "landscape" looks like one thing wherever it is said.
 *
 * `size` scales the rendered box only; the viewBox stays 16x16, so the rectangle
 * and its stroke shrink together. `verticalAlign` is what puts it on the baseline
 * inside the info line's `Text`; it does nothing at the call sites that are flex
 * items, where `vertical-align` is ignored.
 *
 * `aria-hidden` throughout: every call site either sits beside the word or carries
 * an `aria-label` of its own, so the icon has nothing to add to it.
 */
export function OrientationIcon({ orientation, size = 16 }: { orientation: Orientation; size?: number }) {
  const [width, height] = orientation === "landscape" ? [14, 10] : [10, 14];
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      style={{ verticalAlign: "-0.15em" }}
      aria-hidden
      focusable="false"
    >
      <rect
        x={(16 - width) / 2}
        y={(16 - height) / 2}
        width={width}
        height={height}
        rx="1.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
      />
    </svg>
  );
}

/** The word for an orientation, capitalised for the start of a group on the info line. */
export function orientationLabel(orientation: Orientation) {
  return orientation === "landscape" ? "Landscape" : "Portrait";
}
