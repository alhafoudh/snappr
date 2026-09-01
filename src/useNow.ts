import { useEffect, useState } from "react";

const seconds = () => Math.floor(Date.now() / 1000);

/**
 * The current time in Unix seconds, refreshed on an interval. One instance in `App`
 * feeds every filmstrip cell, so the strip re-renders once a minute rather than
 * running a timer per thumb.
 */
export function useNow(intervalMs = 60_000): number {
  const [now, setNow] = useState(seconds);

  useEffect(() => {
    const timer = setInterval(() => setNow(seconds()), intervalMs);
    return () => clearInterval(timer);
  }, [intervalMs]);

  return now;
}
