import { formatDistanceStrict } from "date-fns";

/**
 * "just now", "3 minutes ago", "2 hours ago" - how long ago a photo landed in the
 * strip. Both arguments are Unix seconds, matching `Photo.added_at` from Rust.
 *
 * The first minute is collapsed to "just now" on purpose: the strip only re-renders
 * once a minute, so "12 seconds ago" would sit there stale for most of that minute.
 * `floor` keeps 59 minutes from rounding up to "1 hour" early.
 */
export function relativeTime(addedAtSeconds: number, nowSeconds: number): string {
  const elapsed = Math.max(0, nowSeconds - addedAtSeconds);
  if (elapsed < 60) return "just now";
  return formatDistanceStrict(new Date(addedAtSeconds * 1000), new Date(nowSeconds * 1000), {
    addSuffix: true,
    roundingMethod: "floor",
  });
}
