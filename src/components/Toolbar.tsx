import { Box, Button, Flex, Heading, SegmentedControl, Separator, Text, Tooltip } from "@radix-ui/themes";
import { DotFilledIcon } from "@radix-ui/react-icons";

import type { Overlay, Settings, View } from "../types";

/** The last segment of a path, for a folder or a file. */
export function baseName(path: string) {
  return path.replace(/\/+$/, "").split("/").pop() || path;
}

/**
 * What the shipped overlay is called everywhere it is named. One constant because
 * it appears in the menu twice and in the tooltip, and the three have to agree -
 * the operator has to recognise the thing the Reveal item hands them as the thing
 * the tick is on.
 */
export const SAMPLE_NAME = "Sample overlay";

/** What a slot is set to, for a menu label or a tooltip. */
export function overlayName(overlay: Overlay) {
  switch (overlay.kind) {
    case "sample":
      return SAMPLE_NAME;
    case "custom":
      return baseName(overlay.path);
    case "none":
      return "No overlay";
  }
}

/**
 * The titlebar row.
 *
 * `data-tauri-drag-region="deep"` rather than a bare attribute: Tauri's drag
 * script (tauri/src/window/scripts/drag.js) treats the bare form as "only direct
 * clicks on this element", so any wrapper element a component library puts
 * between the header and the pointer would silently stop the window dragging.
 * "deep" makes the whole subtree a handle, and the same script already refuses
 * to drag from a clickable element, so the buttons keep working without the
 * -webkit-app-region rule the old stylesheet carried.
 */
export function Toolbar({
  settings,
  count,
  view,
  onView,
  viewEnabled,
  onToggleWatching,
  onPickWatchDir,
  onOverlays,
  watchProblem,
}: {
  settings: Settings | null;
  count: number;
  view: View;
  onView: (view: View) => void;
  viewEnabled: boolean;
  onToggleWatching: () => void;
  onPickWatchDir: () => void;
  onOverlays: () => void;
  /**
   * Why nothing is being watched, when the operator has asked for it and it did
   * not work. The pill is this message's home rather than the Stage: at an event
   * the Stage always has a photo in it, and "not watching" has to be visible
   * while it does.
   */
  watchProblem: string | null;
}) {
  // Three states, not two. `watch_enabled` is what the operator asked for, which
  // is not the same as what is happening - a watch can fail to start and leave
  // the setting on. Saying "Watching" then is a lie, and it was one the bar told
  // for a while after a boot-time failure.
  const wanted = settings?.watch_enabled ?? false;
  const failed = wanted && watchProblem !== null;
  const watching = wanted && !failed;
  // Named so the tooltip says what will actually be printed, rather than making
  // "none" look like a loading state. Cleared is a real, working setting.
  // Whether anything will actually be drawn, which is what the label reports.
  // A fresh install is on the sample, so "No overlays" now means the operator
  // turned both off rather than that nothing has loaded yet.
  const landscape = settings?.overlay_landscape;
  const portrait = settings?.overlay_portrait;
  const draws = landscape?.kind !== "none" || portrait?.kind !== "none";

  return (
    <Flex direction="column">
      <Flex asChild align="center" gap="3" pr="3" flexGrow="1">
        <header data-tauri-drag-region="deep">
          {/* Clearance for the traffic lights, which the transparent titlebar
              draws over the content rather than above it. */}
          <Box width="82px" flexShrink="0" />

          <Heading as="h1" size="4" trim="both">
            Snappr
          </Heading>

          <Tooltip
            content={
              failed
                ? `${watchProblem} Click to try again.`
                : watching
                  ? "Watching the folder - click to pause"
                  : "Paused - click to watch the folder"
            }
          >
            <Button
              size="1"
              variant="outline"
              color={failed ? "red" : "gray"}
              onClick={onToggleWatching}
              disabled={!settings}
              aria-pressed={watching}
            >
              {/* A dot rather than a switch: the word says what it is doing and
                  the dot says it at a glance from across the room. Red is the
                  third state - asked for, and not happening. */}
              <Text color={failed ? "red" : watching ? "blue" : "gray"} asChild>
                <DotFilledIcon />
              </Text>
              {failed ? "Not watching" : watching ? "Watching" : "Paused"}
            </Button>
          </Tooltip>

          <Tooltip content={settings ? `Watching ${settings.watch_dir} - click to change` : "Loading..."}>
            <Button size="1" variant="ghost" color="gray" onClick={onPickWatchDir}>
              <Text truncate style={{ maxWidth: "34ch" }}>
                {settings ? baseName(settings.watch_dir) : "..."}
              </Text>
            </Button>
          </Tooltip>

          {/* One button rather than two, because the bar is 44px and already
              dense. It opens a native menu with a submenu per orientation - see
              the note in App.tsx on why the menus here are the OS's, not Radix's. */}
          <Tooltip
            content={
              settings
                ? `Landscape: ${landscape ? overlayName(landscape) : "..."}\n` +
                  `Portrait: ${portrait ? overlayName(portrait) : "..."}`
                : "Loading..."
            }
          >
            <Button
              size="1"
              variant="ghost"
              color="gray"
              onClick={onOverlays}
              disabled={!settings}
              aria-label="Overlays"
            >
              <Text color={draws ? undefined : "gray"}>{draws ? "Overlays" : "No overlays"}</Text>
            </Button>
          </Tooltip>

          <Text size="1" color="gray">
            {count} photo{count === 1 ? "" : "s"}
          </Text>

          <SegmentedControl.Root
            ml="auto"
            size="1"
            value={view}
            disabled={!viewEnabled}
            onValueChange={(value) => onView(value as View)}
            aria-label="Which version to show"
          >
            {/* disabled lives on the Root: SegmentedControl.Item's type removes it. */}
            <SegmentedControl.Item value="overlay">Overlayed</SegmentedControl.Item>
            <SegmentedControl.Item value="source">Original</SegmentedControl.Item>
          </SegmentedControl.Root>
        </header>
      </Flex>
      <Separator size="4" />
    </Flex>
  );
}
