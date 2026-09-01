import { Box, Flex, RadioCards, ScrollArea, Separator, Skeleton, Text, Tooltip } from "@radix-ui/themes";
import { DotFilledIcon } from "@radix-ui/react-icons";
import { convertFileSrc } from "@tauri-apps/api/core";

import type { Photo } from "../types";

const CELL_WIDTH = 132;
const FRAME_HEIGHT = "84px";

/**
 * contain, not cover: a portrait thumb reads as portrait at a glance, which is
 * the one thing the operator most wants to know before it prints.
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
function Cell({ photo, onMenu }: { photo: Photo; onMenu: (event: React.MouseEvent) => void }) {
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
            {status && (
              <Box position="absolute" top="0" right="0">
                <Text as="span" color={status === "failed" ? "red" : "amber"} aria-label={status}>
                  <DotFilledIcon />
                </Text>
              </Box>
            )}
          </Box>
          {/* min-width 0 on the column above is what lets this truncate rather
              than widening the cell to fit the filename. */}
          <Text size="1" truncate>
            {photo.name}
          </Text>
        </Flex>
      </Tooltip>
    </RadioCards.Item>
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
  onSelect,
  onCellMenu,
  onStripMenu,
}: {
  photos: Photo[];
  selected: string | null;
  onSelect: (source: string) => void;
  onCellMenu: (photo: Photo) => (event: React.MouseEvent) => void;
  onStripMenu: (event: React.MouseEvent) => void;
}) {
  return (
    <Flex direction="column" minHeight="0">
      <Separator size="4" />
      <ScrollArea scrollbars="horizontal" type="hover">
        <Box p="3" onContextMenu={onStripMenu} height="100%">
          <RadioCards.Root
            size="1"
            gap="2"
            columns={`repeat(${Math.max(photos.length, 1)}, ${CELL_WIDTH}px)`}
            value={selected ?? ""}
            onValueChange={onSelect}
            aria-label="Photos"
          >
            {photos.map((photo) => (
              <Cell key={photo.source} photo={photo} onMenu={onCellMenu(photo)} />
            ))}
          </RadioCards.Root>
        </Box>
      </ScrollArea>
    </Flex>
  );
}
