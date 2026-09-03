import { forwardRef } from "react";
import { Box, Button, Flex, Kbd, SegmentedControl, Text, Tooltip } from "@radix-ui/themes";
import { MagnifyingGlassIcon, ReloadIcon, Share1Icon } from "@radix-ui/react-icons";

import { OrientationIcon, orientationLabel } from "./OrientationIcon";
import { PrinterIcon } from "./PrinterIcon";
import type { Exif, Level, Notice, Photo, View } from "../types";
import { relativeTime } from "../relativeTime";

/**
 * How far the shutter time may sit from the arrival time before it is worth saying
 * out loud, in seconds.
 *
 * A photo AirDropped straight off the phone was taken seconds ago, and "added 2
 * minutes ago" already tells the operator everything. One out of the camera roll was
 * taken last March, and then the arrival time is actively misleading about what they
 * are looking at. Ten minutes separates the two without the line growing a group it
 * does not need at an event, which is the common case.
 */
const TAKEN_AT_DRIFT = 10 * 60;

/**
 * Date and clock time, no year and no seconds: "21 Mar, 12:40".
 *
 * The time earns its place - the group only shows at all for a photo that did not come
 * straight off the phone, and "which of this morning's shots is this" is exactly the
 * question the operator is asking then. The year does not: undefined locale means the
 * operator's own format, and the full timestamp is in the hover.
 */
const TAKEN_AT_FORMAT: Intl.DateTimeFormatOptions = {
  day: "numeric",
  month: "short",
  hour: "2-digit",
  minute: "2-digit",
};

/** The camera, plus the lens when Snappr::Exif judged it to be saying something new. */
function cameraSummary(exif?: Exif) {
  return [exif?.camera, exif?.lens].filter(Boolean).join(" ") || null;
}

/** The exposure, in the order a camera's own display puts it. */
function exposureSummary(exif?: Exif) {
  const iso = exif?.iso ? `ISO ${exif.iso}` : null;
  return [exif?.focal_length, exif?.aperture, exif?.shutter, iso].filter(Boolean).join(" ") || null;
}

/** Unix seconds for an EXIF timestamp, or null if the camera wrote something unusable. */
function takenAt(exif?: Exif) {
  if (!exif?.taken_at) return null;
  // A naive timestamp - the zone is a separate tag plenty of cameras never write - is
  // parsed as local, which is the right reading of a camera clock with no zone on it.
  const at = new Date(exif.taken_at).getTime();
  return Number.isFinite(at) ? at / 1000 : null;
}

/**
 * One problem on the info line: red for a failure, amber for something that
 * happened differently than asked.
 *
 * The Tooltip is not decoration. The line truncates, so without it a message
 * longer than the row is simply unreadable - which is what the `problem` branch
 * used to do while the warnings branch beside it had a tooltip all along. `full`
 * is for when there is more to say than the line shows, as with several warnings
 * on one photo; it is the same message either way, never the technical half.
 */
function NoticeLine({ level, text, full }: { level: Level; text: string; full?: string }) {
  return (
    <Tooltip content={full ?? text}>
      <Text size="1" color={level === "error" ? "red" : "amber"} truncate>
        {text}
      </Text>
    </Tooltip>
  );
}

/**
 * Which of the two crops the photo landed in, and so which overlay was composited
 * over it and which way it was cropped.
 *
 * `Photo.orientation` is geometric and measured after `autorot`, never the EXIF
 * `Orientation` tag - it is the same answer `Overlay#orientation_of` used to pick
 * the overlay, which is what makes it worth printing rather than deriving from the
 * dimensions here. Absent until the photo completes, like every other fact on this
 * line, so a queued or failed photo just drops the group.
 *
 * It leads the print facts rather than joining the camera groups above them, and
 * there is deliberately no `·` after it: it qualifies the dimensions it precedes.
 * The icon is 12px rather than its default 16 because the line is `Text size="1"`,
 * and a 16px glyph would set the row's line height.
 */
function OrientationGroup({ photo }: { photo: Photo }) {
  if (!photo.orientation) return null;
  return (
    <>
      <OrientationIcon orientation={photo.orientation} size={12} />
      {` ${orientationLabel(photo.orientation)} `}
    </>
  );
}

/**
 * The information line under the photo, in priority order: what went wrong with
 * the photo, then what went wrong with the last thing the operator asked for,
 * then the facts about the print that is about to be made.
 */
function Info({ current, ready, problem, now }: { current: Photo | null; ready: boolean; problem: Notice | null; now: number }) {
  if (current?.warnings.length) {
    return <NoticeLine level="warning" text={current.warnings[0]} full={current.warnings.join("\n")} />;
  }
  if (problem) {
    return <NoticeLine level={problem.level} text={problem.text} />;
  }
  if (ready && current) {
    const added = new Date(current.added_at * 1000);
    const exif = current.exif;
    const taken = takenAt(exif);

    // The camera's own facts first, then the print's. The photo is what the operator
    // and the guest are both looking at; the pixel count is the machine's business.
    // Each group drops out when the source carries nothing for it, so a screenshot
    // gives exactly the line this showed before EXIF was read at all.
    const groups = [
      cameraSummary(exif),
      exposureSummary(exif),
      // Only when the arrival time is misleading about the photo - see TAKEN_AT_DRIFT.
      taken !== null && Math.abs(taken - current.added_at) > TAKEN_AT_DRIFT
        ? `taken ${new Date(taken * 1000).toLocaleString(undefined, TAKEN_AT_FORMAT)}`
        : null,
    ].filter(Boolean);

    // The line truncates, so everything it can lose goes in the tooltip in full. That
    // includes the two facts deliberately kept off the line: the 35mm equivalent, and
    // the exact clock times behind "added 2 minutes ago".
    const detail = (
      <Flex direction="column" gap="1">
        {exif?.camera && <Text size="1">{[exif.camera, exif.lens].filter(Boolean).join(" ")}</Text>}
        {exposureSummary(exif) && (
          <Text size="1">
            {exposureSummary(exif)}
            {exif?.focal_length_35mm ? ` (${exif.focal_length_35mm}mm equivalent)` : ""}
          </Text>
        )}
        {taken !== null && <Text size="1">Taken {new Date(taken * 1000).toLocaleString()}</Text>}
        <Text size="1">Added {added.toLocaleString()}</Text>
        <Text size="1">
          <OrientationGroup photo={current} />
          {current.output_width} x {current.output_height} px, {current.depth}-bit
          {current.bytes ? `, ${(current.bytes / 1024 / 1024).toFixed(1)} MB` : ""}
        </Text>
      </Flex>
    );

    return (
      <Tooltip content={detail}>
        <Text size="1" color="gray" truncate>
          {groups.map((group) => `${group} · `).join("")}
          <OrientationGroup photo={current} />
          {current.output_width} x {current.output_height} px, {current.depth}-bit
          {current.bytes ? `, ${(current.bytes / 1024 / 1024).toFixed(1)} MB` : ""}
          {", added "}
          {/* No title of its own: the Radix tooltip above owns the hover for the whole
              line, and a native one underneath it would race and win. */}
          <time dateTime={added.toISOString()}>{relativeTime(current.added_at, now)}</time>
        </Text>
      </Tooltip>
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
    problem: Notice | null;
    now: number;
    view: View;
    onView: (view: View) => void;
    onReprocess: (source: string) => void;
    onReveal: () => void;
    onShare: () => void;
    onPrint: () => void;
  }
>(function Actions(
  { current, ready, busy, problem, now, view, onView, onReprocess, onReveal, onShare, onPrint },
  shareRef,
) {
  const working = current?.status === "processing" || current?.status === "queued";

  return (
    <Flex align="center" gap="4" px="5" pb="3" height="52px" flexShrink="0">
      <Box flexGrow="1" minWidth="0">
        <Info current={current} ready={ready} problem={problem} now={now} />
      </Box>

      {/* The before/after switch sits with the buttons rather than in the
          titlebar, because it acts on the photo the way they do and reads as one
          row of things you can do to what is on screen. `mr="2"` on top of the
          row's own gap keeps it a group of its own - it changes what you are
          looking at, where the four to its right act on it. */}
      <Flex flexShrink="0" mr="2">
        {/* No `size`, like the buttons to its right: both take Radix's default
            size 2, which is what makes the control the same height as them. */}
        <SegmentedControl.Root
          value={view}
          disabled={!ready}
          onValueChange={(value) => onView(value as View)}
          aria-label="Which version to show"
        >
          {/* disabled lives on the Root: SegmentedControl.Item's type removes it. */}
          <SegmentedControl.Item value="overlay">Overlayed</SegmentedControl.Item>
          <SegmentedControl.Item value="source">Original</SegmentedControl.Item>
        </SegmentedControl.Root>
      </Flex>

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
