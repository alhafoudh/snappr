import { Box, Button, Card, Flex, RadioCards, ScrollArea, Separator, Skeleton, Text, Tooltip } from "@radix-ui/themes";
import { DotFilledIcon, FileIcon } from "@radix-ui/react-icons";
import { convertFileSrc } from "@tauri-apps/api/core";

import { OrientationIcon } from "./OrientationIcon";
import { baseName } from "../naming";
import type { Photo, RuntimeStatus, Settings } from "../types";

const CELL_WIDTH = 132;
const FRAME_HEIGHT = "84px";

/**
 * contain, not cover: a portrait thumb reads as portrait at a glance, which is
 * the one thing the operator most wants to know before it prints.
 *
 * The glyph in the frame's corner is the explicit answer now; this is what keeps
 * the picture agreeing with it, rather than filling the cell either way.
 */
const thumb = { width: "100%", height: "100%", objectFit: "contain", display: "block" } as const;

/**
 * One filmstrip cell. Points at the 400px thumb, never the overlay PNG: Tauri's
 * asset protocol reads a whole file into memory per request, and the print output
 * is 10-25 MB and 16-bit when the source was HEIC.
 *
 * Selection and status are separate channels on purpose. RadioCards draws the
 * selection; status is a dot in the corner. An earlier version put both on the
 * border, and a selected photo that had a warning simply showed no selection.
 */
function Cell({
  photo,
  unseen,
  onMenu,
}: {
  photo: Photo;
  unseen: boolean;
  onMenu: (event: React.MouseEvent) => void;
}) {
  const status = photo.status === "failed" ? "failed" : photo.warnings.length > 0 ? "warned" : null;

  return (
    <RadioCards.Item value={photo.source} data-source={photo.source} onContextMenu={onMenu}>
      {/* The Tooltip goes *inside* the Item, never around it. Its trigger is
          asChild and sets its own `data-state` on whatever it wraps, and
          RadioGroup's item spreads incoming props after its own `data-state`
          (react-radio-group/dist/index.mjs:94-98) - so wrapping the Item hands
          it data-state="closed" and the checked styling silently never renders.
          The symptom is a selected cell that looks exactly like an idle one. */}
      <Tooltip content={photo.error || photo.warnings[0] || photo.name}>
        <Flex direction="column" gap="1" width="100%" minWidth="0">
          <Box position="relative" height={FRAME_HEIGHT} overflow="hidden">
            {photo.previews ? (
              <img src={convertFileSrc(photo.previews.thumb)} alt="" draggable={false} style={thumb} />
            ) : (
              <Skeleton loading={photo.status === "processing"} height="100%" width="100%">
                <Box height="100%" />
              </Skeleton>
            )}
            {/* Which crop the photo landed in, in the corner the status dot does
                not own. The word is in the label and not on the cell: "Landscape"
                at size 1 is most of a 132px cell, and the filename would have
                nothing left. The full sentence is on the info line under the
                photo, which is where there is room for it. */}
            {photo.orientation && (
              <Box position="absolute" top="0" left="0">
                <Text as="span" color="gray" aria-label={`${photo.orientation} photo`}>
                  <OrientationIcon orientation={photo.orientation} size={12} />
                </Text>
              </Box>
            )}
            {status && (
              <Box position="absolute" top="0" right="0">
                {/* The label is a phrase, not the state's name: a screen reader
                    read the bare words "failed" and "warned" otherwise. */}
                <Text
                  as="span"
                  color={status === "failed" ? "red" : "amber"}
                  aria-label={status === "failed" ? "this photo failed" : "this photo has a warning"}
                >
                  <DotFilledIcon />
                </Text>
              </Box>
            )}
          </Box>
          {/* min-width 0 on the column above is what lets this truncate rather
              than widening the cell to fit the filename. */}
          <Text size="1" truncate>
            {/* A photo that arrived while the operator was elsewhere in the strip
                and has not been looked at since. It goes before the name rather
                than in the frame because both of the frame's top corners are
                already taken - orientation left, status right - and because it is
                a fact about the file, which is what the name is.

                Inside the same `Text` rather than a Flex row beside it, so the
                name keeps one truncating text box and the ellipsis still lands in
                the right place. `verticalAlign` is inline because Radix has no
                prop for it and a 15px icon sits high on a size-1 line. */}
            {unseen && (
              <Text as="span" color="blue" aria-label="not looked at yet">
                <DotFilledIcon style={{ verticalAlign: "middle" }} />
              </Text>
            )}
            {photo.name}
          </Text>
        </Flex>
      </Tooltip>
    </RadioCards.Item>
  );
}

/**
 * The zeroth cell: where the photos come from.
 *
 * This is the head of the strip because that is where the arrivals appear -
 * `ordered` is newest-first, so every photo that lands does so directly beside
 * it, and the control that decides whether they land at all is right there. It
 * used to live in the 44px titlebar, which is app chrome and is not what the
 * watch folder is.
 *
 * **Outside `RadioCards.Root`, and outside the ScrollArea.** Inside the
 * radiogroup it would have to be a radio item, which would make it selectable and
 * put it in the arrow keys' roving focus; inside the scroll area it would slide
 * away from the newest photo the moment the strip filled up.
 *
 * The three-state watch logic is carried over from the toolbar unchanged, and is
 * the point of the cell: `watch_enabled` is what the operator *asked* for, which
 * is not the same as what is happening. A watch can fail to start and leave the
 * setting on, and saying "Watching" then is a lie the bar told for a while.
 */
function WatchCell({
  settings,
  count,
  watchProblem,
  onToggleWatching,
  onPickWatchDir,
}: {
  settings: Settings | null;
  count: number;
  watchProblem: string | null;
  onToggleWatching: () => void;
  onPickWatchDir: () => void;
}) {
  const wanted = settings?.watch_enabled ?? false;
  const failed = wanted && watchProblem !== null;
  const watching = wanted && !failed;

  return (
    // asChild so the cell can be given a height: Card carries no height prop, and
    // the cell has to match the photo cells beside it rather than shrink to its
    // two buttons.
    <Card size="1" asChild>
      <Box height="100%">
        <Flex direction="column" gap="2" justify="center" height="100%">
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
            <Button size="1" variant="soft" color="gray" onClick={onPickWatchDir} disabled={!settings}>
              <FileIcon />
              <Text truncate>{settings ? baseName(settings.watch_dir) : "..."}</Text>
            </Button>
          </Tooltip>

          {/* How many cells are in the strip, in the cell the strip starts with.
              It was a titlebar label, which is the wrong place for a fact about
              the strip - here it is right beside what it counts. */}
          <Text size="1" color="gray" align="center">
            {count} photo{count === 1 ? "" : "s"}
          </Text>
        </Flex>
      </Box>
    </Card>
  );
}

/**
 * RadioCards rather than a row of buttons: the selection ring, the hover and
 * focus states, the roving arrow-key focus and the radiogroup semantics all come
 * with it, which is most of what the old stylesheet was for.
 */
export function Filmstrip({
  photos,
  selected,
  unseen,
  settings,
  runtime,
  count,
  onSelect,
  onCellMenu,
  onStripMenu,
  onToggleWatching,
  onPickWatchDir,
}: {
  photos: Photo[];
  selected: string | null;
  /** Sources that arrived unlooked-at; their cells carry a blue dot. */
  unseen: ReadonlySet<string>;
  settings: Settings | null;
  runtime: RuntimeStatus;
  count: number;
  onSelect: (source: string) => void;
  onCellMenu: (photo: Photo) => (event: React.MouseEvent) => void;
  onStripMenu: (event: React.MouseEvent) => void;
  onToggleWatching: () => void;
  onPickWatchDir: () => void;
}) {
  return (
    // height="100%" for the same reason as the toolbar's: a column Flex sizes to
    // its content, so without it the strip does not fill its grid row and the
    // watch cell has no height to match the photo cells against.
    <Flex direction="column" height="100%" minHeight="0">
      <Separator size="4" />
      <Flex direction="row" minHeight="0" align="stretch">
        <Box pl="3" py="3" flexShrink="0" width={`${CELL_WIDTH + 20}px`}>
          <WatchCell
            settings={settings}
            count={count}
            watchProblem={runtime.watch_error ?? null}
            onToggleWatching={onToggleWatching}
            onPickWatchDir={onPickWatchDir}
          />
        </Box>
        <ScrollArea scrollbars="horizontal" type="hover">
          <Box p="3" onContextMenu={onStripMenu} height="100%">
            <RadioCards.Root
              size="1"
              gap="2"
              columns={`repeat(${Math.max(photos.length, 1)}, ${CELL_WIDTH}px)`}
              value={selected ?? ""}
              onValueChange={onSelect}
              // Radix loops by default and `step` in App.tsx clamps. The strip has
              // two arrow paths - Radix's while it has focus, `step`'s otherwise -
              // and they have to feel the same, so both stop at the ends. Wrapping
              // from the oldest cell onto the newest is a jump across the whole
              // session that nobody asked for.
              loop={false}
              aria-label="Photos"
            >
              {photos.map((photo) => (
                <Cell
                  key={photo.source}
                  photo={photo}
                  unseen={unseen.has(photo.source)}
                  onMenu={onCellMenu(photo)}
                />
              ))}
            </RadioCards.Root>
          </Box>
        </ScrollArea>
      </Flex>
    </Flex>
  );
}
