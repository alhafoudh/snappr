import { Box, Button, Callout, Code, Flex, Spinner, Text } from "@radix-ui/themes";
import { ExclamationTriangleIcon, ImageIcon } from "@radix-ui/react-icons";
import { convertFileSrc } from "@tauri-apps/api/core";

import type { Photo, RuntimeStatus, Settings, View } from "../types";
import { baseName } from "./Toolbar";

/**
 * Letterboxed, and the box must have a definite size for that to work.
 *
 * The overlayed and original views have different aspect ratios by design - the
 * print's 148:100 against the source frame as shot - so the box has to stay put
 * and the image fit inside it, or the layout jumps on every toggle. With only
 * max-width and max-height the element's box takes the image's own aspect ratio,
 * which leaves object-fit nothing to do and lets a portrait source stretch the
 * frame. No shadow: a filter on a 2000px image repaints on every toggle, and the
 * dark ground already separates it.
 *
 * This is inline rather than a class because object-fit has no Radix prop, and an
 * inline style on an <img> is the pattern Radix's own examples use.
 */
const letterbox = {
  display: "block",
  width: "100%",
  height: "100%",
  minHeight: 0,
  objectFit: "contain",
  objectPosition: "center",
} as const;

export function Stage({
  runtime,
  settings,
  current,
  view,
  dragging,
  onReprocess,
}: {
  runtime: RuntimeStatus;
  settings: Settings | null;
  current: Photo | null;
  view: View;
  dragging: boolean;
  onReprocess: (source: string) => void;
}) {
  const preview = current?.previews?.[view];

  const body = () => {
    // A file is being dragged over the window. The shell owns the drop and does
    // the processing; this only says where it will land.
    if (dragging) {
      return (
        <Callout.Root color="blue" variant="surface">
          <Callout.Icon>
            <ImageIcon />
          </Callout.Icon>
          <Callout.Text>Drop the photo here to overlay it.</Callout.Text>
        </Callout.Root>
      );
    }

    if (!runtime.ready) {
      if (runtime.error) {
        return (
          <Callout.Root color="red" variant="surface">
            <Callout.Icon>
              <ExclamationTriangleIcon />
            </Callout.Icon>
            <Callout.Text>{runtime.error}</Callout.Text>
            <Callout.Text size="1">
              Run <Code>mise run runtime</Code> and restart the app.
            </Callout.Text>
          </Callout.Root>
        );
      }
      return (
        <Flex align="center" gap="2">
          <Spinner />
          <Text color="gray">Starting the image pipeline...</Text>
        </Flex>
      );
    }

    if (!current) {
      return (
        <Text color="gray" align="center" style={{ maxWidth: "52ch" }}>
          {settings?.watch_enabled === false
            ? "Watching is paused. Drop a photo here to overlay it."
            : `AirDrop a photo to ${settings ? baseName(settings.watch_dir) : "the watched folder"}, or drop one here, to get started.`}
        </Text>
      );
    }

    if (preview) {
      return <img src={convertFileSrc(preview)} alt={current.name} draggable={false} style={letterbox} />;
    }

    if (current.status === "failed") {
      return (
        <Callout.Root color="red" variant="surface">
          <Callout.Icon>
            <ExclamationTriangleIcon />
          </Callout.Icon>
          <Callout.Text>
            <Text weight="bold">{current.name}</Text>
          </Callout.Text>
          <Callout.Text>{current.error}</Callout.Text>
          <Box>
            <Button variant="soft" color="red" onClick={() => onReprocess(current.source)}>
              Try again
            </Button>
          </Box>
        </Callout.Root>
      );
    }

    return (
      <Flex align="center" gap="2">
        <Spinner />
        <Text color="gray">Working on {current.name}...</Text>
      </Flex>
    );
  };

  return (
    <Flex flexGrow="1" minHeight="0" align="center" justify="center" px="5" pt="5" pb="3" overflow="hidden">
      {body()}
    </Flex>
  );
}
