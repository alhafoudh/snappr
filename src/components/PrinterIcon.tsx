/**
 * A printer, drawn rather than imported: @radix-ui/react-icons has none.
 * `OrientationIcon` is the same pattern for the same reason.
 *
 * Shared by the two things that mean "print" - the Print button in the actions
 * bar, and the mark on a filmstrip cell that has already been printed - so the
 * cell's glyph is recognisably the button that put it there. It lived inside
 * `Actions.tsx` while the button was its only call site.
 *
 * Drawn on a 15x15 grid rather than OrientationIcon's 16, because that is the box
 * `@radix-ui/react-icons` uses and the button sits it beside `ReloadIcon` and
 * `MagnifyingGlassIcon`. `size` scales the rendered box only, so the strokes
 * shrink with it.
 *
 * `aria-hidden`: both call sites carry their own label - the button has its text,
 * the cell an `aria-label` - so the icon has nothing to add.
 */
export function PrinterIcon({ size = 15 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 15 15"
      fill="none"
      stroke="currentColor"
      strokeLinejoin="round"
      style={{ verticalAlign: "-0.15em" }}
      aria-hidden
      focusable="false"
    >
      <path d="M4.5 4.5v-3h6v3" />
      <rect x="1.5" y="4.5" width="12" height="5" rx="1" />
      <path d="M4.5 9.5h6v4h-6z" />
      <path d="M11 7h0.6" strokeLinecap="round" />
    </svg>
  );
}
