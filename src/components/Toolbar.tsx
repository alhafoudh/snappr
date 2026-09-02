import { Box, Flex, Heading, Separator } from "@radix-ui/themes";

/**
 * The titlebar row.
 *
 * **What is no longer here is the point of it.** The watch toggle, the watched
 * folder and the photo count moved into the zeroth filmstrip cell, next to the
 * photos they are about; the Overlays button was replaced by the panel down the
 * left-hand side; and the before/after switch moved down to the Actions row, with
 * the other things that act on the photo on screen. The bar had become the place
 * everything that was not a photo ended up.
 *
 * What is left is the window's own identity, and the room the traffic lights
 * need. `TOOLBAR_HEIGHT` is exported because `App.tsx`'s grid row is what applies
 * it and `tauri.conf.json` centres the traffic lights against it - three numbers
 * that have to agree, so there is one of them.
 *
 * `data-tauri-drag-region="deep"` rather than a bare attribute: Tauri's drag
 * script (tauri/src/window/scripts/drag.js) treats the bare form as "only direct
 * clicks on this element", so any wrapper element a component library puts
 * between the header and the pointer would silently stop the window dragging.
 * "deep" makes the whole subtree a handle, and the same script already refuses to
 * drag from a clickable element. With every control gone the bar is now entirely
 * drag surface, which is the one thing a titlebar should be.
 */
export const TOOLBAR_HEIGHT = "36px";

export function Toolbar() {
  return (
    // `height="100%"` is load-bearing, and its absence does not look like a bug:
    // a column Flex sizes to its content, so without it the bar was as tall as the
    // title and `flexGrow` on the row inside had nothing to grow into. The grid row
    // was the full height all along - the header just was not filling it, so
    // raising TOOLBAR_HEIGHT appeared to do nothing at all.
    <Flex direction="column" height="100%">
      <Flex asChild align="center" gap="3" pr="3" flexGrow="1">
        <header data-tauri-drag-region="deep">
          {/* Clearance for the traffic lights, which the transparent titlebar
              draws over the content rather than above it. Three buttons 20pt
              apart from the x in `trafficLightPosition`, plus a gap. */}
          <Box width="82px" flexShrink="0" />

          <Heading as="h1" size="4" trim="both">
            Snappr
          </Heading>
        </header>
      </Flex>
      <Separator size="4" />
    </Flex>
  );
}
