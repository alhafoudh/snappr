import { forwardRef } from "react";
import { Box, Button, Flex, Kbd, Text, Tooltip } from "@radix-ui/themes";
import { MagnifyingGlassIcon, ReloadIcon, Share1Icon } from "@radix-ui/react-icons";

import type { Photo } from "../types";
import { relativeTime } from "../relativeTime";

/**
 * @radix-ui/react-icons has no printer, so this is drawn on its 15x15 grid to
 * sit level with the ReloadIcon and MagnifyingGlassIcon beside it.
 */
function PrinterIcon() {
  return (
    <svg
      width="15"
      height="15"
      viewBox="0 0 15 15"
      fill="none"
      stroke="currentColor"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M4.5 4.5v-3h6v3" />
      <rect x="1.5" y="4.5" width="12" height="5" rx="1" />
      <path d="M4.5 9.5h6v4h-6z" />
      <path d="M11 7h0.6" strokeLinecap="round" />
    </svg>
  );
}

/**
 * The information line under the photo, in priority order: what went wrong with
 * the photo, then what went wrong with the last thing the operator asked for,
 * then the facts about the print that is about to be made.
 */
function Info({ current, ready, problem, now }: { current: Photo | null; ready: boolean; problem: string | null; now: number }) {
  if (current?.warnings.length) {
    return (
      <Tooltip content={current.warnings.join("\n")}>
        <Text size="1" color="amber" truncate>
          {current.warnings[0]}
        </Text>
      </Tooltip>
    );
  }
  if (problem) {
    return (
      <Text size="1" color="red" truncate>
        {problem}
      </Text>
    );
  }
  if (ready && current) {
    const added = new Date(current.added_at * 1000);
    return (
      <Text size="1" color="gray" truncate>
        {current.output_width} x {current.output_height} px, {current.depth}-bit
        {current.bytes ? `, ${(current.bytes / 1024 / 1024).toFixed(1)} MB` : ""}
        {", added "}
        <time dateTime={added.toISOString()} title={added.toLocaleTimeString()}>
          {relativeTime(current.added_at, now)}
        </time>
      </Text>
    );
  }
  return null;
}

/**
 * Share takes a ref because the macOS share sheet is a popover anchored to a
 * rectangle, and Rust needs this button's rectangle to put it there. Radix's
 * Button forwards its ref to the underlying <button>, so getBoundingClientRect
 * still reaches a real element.
 */
export const Actions = forwardRef<
  HTMLButtonElement,
  {
    current: Photo | null;
    ready: boolean;
    busy: "share" | "print" | null;
    problem: string | null;
    now: number;
    onReprocess: (source: string) => void;
    onReveal: () => void;
    onShare: () => void;
    onPrint: () => void;
  }
>(function Actions({ current, ready, busy, problem, now, onReprocess, onReveal, onShare, onPrint }, shareRef) {
  const working = current?.status === "processing" || current?.status === "queued";

  return (
    <Flex align="center" gap="4" px="5" pb="3" height="52px" flexShrink="0">
      <Box flexGrow="1" minWidth="0">
        <Info current={current} ready={ready} problem={problem} now={now} />
      </Box>

      <Flex gap="2" flexShrink="0">
        <Tooltip content="Run the overlay again for this photo">
          <Button
            variant="soft"
            color="gray"
            onClick={() => current && onReprocess(current.source)}
            disabled={!current || working || busy !== null}
          >
            <ReloadIcon />
            {current?.status === "failed" ? "Try again" : "Reprocess"}
          </Button>
        </Tooltip>

        <Tooltip content="Show the PNG in Finder">
          <Button variant="soft" color="gray" onClick={onReveal} disabled={!ready}>
            <MagnifyingGlassIcon />
            Reveal
          </Button>
        </Tooltip>

        {/* The share sheet hands over the 10-25 MB print PNG, so it takes a
            moment to appear. `loading` says so without the row changing width. */}
        <Button
          ref={shareRef}
          variant="soft"
          onClick={onShare}
          disabled={!ready || busy !== null}
          loading={busy === "share"}
        >
          <Share1Icon />
          Share
        </Button>

        <Tooltip content={<Flex align="center" gap="2">Print <Kbd>⌘P</Kbd></Flex>}>
          <Button onClick={onPrint} disabled={!ready || busy !== null} loading={busy === "print"}>
            <PrinterIcon />
            Print
          </Button>
        </Tooltip>
      </Flex>
    </Flex>
  );
});
