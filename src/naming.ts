/**
 * The last segment of a path, for a folder or a file.
 *
 * Its own module rather than an export from a component: the watch cell, the
 * Stage and the overlay panel all want it, and importing a helper out of
 * `Toolbar.tsx` was how it ended up being the reason those files knew about the
 * toolbar at all.
 *
 * There is deliberately no `overlayName` here any more. An overlay's display
 * name arrives on `OverlayCard.name`, computed in `Overlay::label` on the Rust
 * side, so there is exactly one place it is spelled.
 */
export function baseName(path: string) {
  return path.replace(/\/+$/, "").split("/").pop() || path;
}
