import {
  Box,
  Button,
  Card,
  Flex,
  IconButton,
  ScrollArea,
  Separator,
  Skeleton,
  Text,
  Tooltip,
} from "@radix-ui/themes";
import {
  ChevronLeftIcon,
  ChevronRightIcon,
  ExclamationTriangleIcon,
  ImageIcon,
  PlusIcon,
} from "@radix-ui/react-icons";
import { convertFileSrc } from "@tauri-apps/api/core";

import { OrientationIcon } from "./OrientationIcon";
import type { OverlayCard, Orientation, Settings } from "../types";

/**
 * The panel's two widths, exported because `App.tsx`'s grid column is what
 * actually applies them - the panel fills whichever column it is given. Two
 * copies of these numbers is how the separator on the right edge ends up with no
 * room to draw in.
 *
 * Wide enough for a landscape card to read at a glance, narrow enough that the
 * photo keeps most of the window; the rail is one icon column. Passed as Radix
 * props, which are typed "enum | string" for exactly this.
 */
export const PANEL_WIDTH = "216px";
export const RAIL_WIDTH = "36px";

/**
 * The render sits over the placeholder, so it must not be cropped or stretched -
 * the whole point is that the artwork and the photo window line up. Inline
 * because object-fit has no Radix prop; the same exception Stage and Filmstrip
 * already carry.
 */
const artwork = {
  display: "block",
  position: "absolute",
  inset: 0,
  width: "100%",
  height: "100%",
  objectFit: "fill",
} as const;

/**
 * The rail's label, reading top to bottom. `vertical-rl` rotates Latin text 90
 * degrees clockwise *and* makes the element tall and narrow, which is the half a
 * `transform: rotate()` does not do - that would leave a wide layout box
 * overflowing a 36px rail. `userSelect` because a label nobody can click should
 * not offer a text cursor either.
 */
const verticalLabel = {
  writingMode: "vertical-rl",
  userSelect: "none",
} as const;

/**
 * The two orientations an overlay can be live for.
 *
 * Two toggle buttons rather than a `SegmentedControl`, which is single-select and
 * so cannot express "both on" - the case the panel exists to make possible. Radix
 * Themes 3 ships no ToggleGroup either, so a pair of IconButtons with
 * `aria-pressed` is the honest control; `gap="1"` and matching sizes are what make
 * them read as one segmented pair.
 *
 * The icons are `OrientationIcon`, which is also what the info line and the
 * filmstrip cell draw - the panel is where they were drawn first, not where they
 * belong.
 */
function Toggles({
  card,
  onToggle,
  disabled,
}: {
  card: OverlayCard;
  onToggle: (orientation: Orientation, on: boolean) => void;
  disabled: boolean;
}) {
  const orientations: Array<[Orientation, boolean]> = [
    ["landscape", card.landscape],
    ["portrait", card.portrait],
  ];

  return (
    <Flex gap="1" flexShrink="0">
      {orientations.map(([orientation, on]) => (
        <Tooltip
          key={orientation}
          content={
            on
              ? `Printing ${orientation} photos with ${card.name} - click to stop`
              : `Use ${card.name} for ${orientation} photos`
          }
        >
          <IconButton
            size="1"
            variant={on ? "solid" : "surface"}
            color={on ? undefined : "gray"}
            disabled={disabled}
            aria-pressed={on}
            aria-label={`${card.name} for ${orientation} photos`}
            onClick={() => onToggle(orientation, !on)}
          >
            <OrientationIcon orientation={orientation} />
          </IconButton>
        </Tooltip>
      ))}
    </Flex>
  );
}

/**
 * The picture, and the whole reason the panel draws one.
 *
 * Three layers, bottom to top: the background the print fills with, then a grey
 * rectangle with a photo icon at exactly the window snappr found, then the
 * rendered artwork - which has that window punched out of it, so the placeholder
 * shows through. An overlay whose magenta marker did not survive its export puts
 * the icon somewhere obviously wrong, or reports no window at all, and either way
 * the operator sees it here rather than on a print.
 */
function Preview({ card, background }: { card: OverlayCard; background: string }) {
  return (
    <Box
      position="relative"
      width="100%"
      style={{
        // The shape the overlay was drawn at, so the artwork is never squashed.
        // 1.48 only until the render lands, so the skeleton is the shape most
        // cards turn out to be rather than a square that jumps.
        aspectRatio: String(card.aspect ?? 1.48),
        background,
        borderRadius: "var(--radius-2)",
        overflow: "hidden",
      }}
    >
      {card.window && (
        <Flex
          position="absolute"
          align="center"
          justify="center"
          style={{
            left: `${card.window.x * 100}%`,
            top: `${card.window.y * 100}%`,
            width: `${card.window.width * 100}%`,
            height: `${card.window.height * 100}%`,
            background: "var(--gray-6)",
            color: "var(--gray-9)",
          }}
        >
          <ImageIcon width="20" height="20" />
        </Flex>
      )}
      {card.thumb ? (
        <img src={convertFileSrc(card.thumb)} alt="" draggable={false} style={artwork} />
      ) : (
        <Skeleton loading height="100%" width="100%">
          <Box height="100%" />
        </Skeleton>
      )}
    </Box>
  );
}

/**
 * The overlay library, left of the photo.
 *
 * The cards are plain Radix `Card`s, deliberately not `RadioCards.Item`s: there is
 * no "selected overlay" here - the two toggles carry all the state, and an overlay
 * can be on for both orientations, neither, or one. That also keeps the panel
 * clear of the documented trap where a Tooltip wrapped around a RadioCards.Item
 * silently kills its checked styling.
 *
 * Remove and Reveal in Finder are on a native context menu, like the filmstrip's,
 * so this is a macOS app all the way down.
 */
export function OverlayPanel({
  cards,
  settings,
  dragging,
  onToggle,
  onAdd,
  onCardMenu,
  onCollapse,
}: {
  cards: OverlayCard[];
  settings: Settings | null;
  /** An SVG is being dragged over the window - the panel is where it will land. */
  dragging: boolean;
  onToggle: (card: OverlayCard, orientation: Orientation, on: boolean) => void;
  onAdd: () => void;
  onCardMenu: (card: OverlayCard) => (event: React.MouseEvent) => void;
  onCollapse: (collapsed: boolean) => void;
}) {
  const collapsed = settings?.overlays_collapsed ?? false;
  const background = settings?.background ?? "#ffffff";
  const live = (orientation: Orientation) => cards.find((card) => card[orientation]);

  if (collapsed) {
    return (
      <Flex direction="row" minHeight="0">
        {/* flexGrow, not a fixed width: the grid column already is the width, and
            a child pinned to the same number leaves the separator below no room
            to draw in - it is 1px wide and gets shrunk to nothing. */}
        <Flex direction="column" align="center" gap="2" py="3" flexGrow="1" minWidth="0">
          <Tooltip content="Show the overlays">
            <IconButton size="1" variant="ghost" color="gray" onClick={() => onCollapse(false)} aria-label="Show the overlays">
              <ChevronRightIcon />
            </IconButton>
          </Tooltip>

          {/* The same label the expanded header carries, turned on its side so
              the rail still says what it is. `writing-mode` rather than a
              `transform`: a rotated transform leaves the layout box unrotated, so
              the text would overflow a 36px rail instead of making it tall and
              narrow. There is no Radix prop for either. */}
          <Text size="1" weight="medium" color="gray" style={verticalLabel}>
            Overlays
          </Text>

          {/* Which orientations are branded, at a glance, without giving the
              photo's width back. Lit means something is being composited. */}
          {(["landscape", "portrait"] as Orientation[]).map((orientation) => {
            const card = live(orientation);
            return (
              <Tooltip
                key={orientation}
                content={card ? `${orientation}: ${card.name}` : `${orientation}: no overlay`}
              >
                <Text as="span" color={card ? "blue" : "gray"} aria-label={`${orientation} overlay`}>
                  <OrientationIcon orientation={orientation} />
                </Text>
              </Tooltip>
            );
          })}
        </Flex>
        <Separator orientation="vertical" size="4" />
      </Flex>
    );
  }

  return (
    <Flex direction="row" minHeight="0">
      {/* flexGrow rather than a fixed width - see the note in the rail above. */}
      <Flex direction="column" minHeight="0" flexGrow="1" minWidth="0">
        <Flex align="center" justify="between" px="3" pt="3" pb="1">
          <Text size="1" weight="medium" color="gray">
            Overlays
          </Text>
          <Tooltip content="Hide the overlays">
            <IconButton size="1" variant="ghost" color="gray" onClick={() => onCollapse(true)} aria-label="Hide the overlays">
              <ChevronLeftIcon />
            </IconButton>
          </Tooltip>
        </Flex>

        <ScrollArea scrollbars="vertical" type="hover">
          <Flex direction="column" gap="2" px="3" pb="2">
            {cards.map((card) => (
              <Box key={`${card.overlay.kind}:${"path" in card.overlay ? card.overlay.path : ""}`}>
                <Card size="1" onContextMenu={onCardMenu(card)}>
                  <Flex direction="column" gap="2" width="100%" minWidth="0">
                    {card.missing ? (
                      // No picture to draw, and the sentence is the whole content
                      // the card has left. Amber rather than red: the overlay is
                      // recoverable by putting the file back, and the print it is
                      // not on has already happened or not.
                      <Flex align="center" gap="1" py="2">
                        <Text color="amber" asChild>
                          <ExclamationTriangleIcon />
                        </Text>
                        <Text size="1" color="amber">
                          File missing
                        </Text>
                      </Flex>
                    ) : (
                      <Preview card={card} background={background} />
                    )}

                    <Flex align="center" justify="between" gap="2">
                      {/* minWidth 0 is what lets the name truncate rather than
                          widening the card to fit a long filename. */}
                      <Box minWidth="0" flexGrow="1">
                        <Tooltip content={card.error || card.warnings[0] || card.name}>
                          <Text
                            size="1"
                            truncate
                            as="div"
                            color={card.error ? "amber" : card.warnings.length > 0 ? "amber" : undefined}
                          >
                            {card.name}
                          </Text>
                        </Tooltip>
                      </Box>
                      <Toggles
                        card={card}
                        disabled={card.missing}
                        onToggle={(orientation, on) => onToggle(card, orientation, on)}
                      />
                    </Flex>

                    {/* The check the picture exists for, said in words as well:
                        a window that was not found leaves the card with no
                        placeholder at all, which on its own reads as a card that
                        has not finished loading. */}
                    {!card.missing && card.thumb && !card.window && (
                      <Text size="1" color="amber">
                        No photo area found
                      </Text>
                    )}
                  </Flex>
                </Card>
              </Box>
            ))}
          </Flex>
        </ScrollArea>

        <Box px="3" pb="3" pt="1">
          <Button
            size="1"
            variant={dragging ? "solid" : "soft"}
            color={dragging ? "blue" : "gray"}
            onClick={onAdd}
            style={{ width: "100%" }}
          >
            <PlusIcon />
            {dragging ? "Drop to add" : "Add overlay"}
          </Button>
        </Box>
      </Flex>
      <Separator orientation="vertical" size="4" />
    </Flex>
  );
}
