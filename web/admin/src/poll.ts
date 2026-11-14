// One screen's reading of the lobby: asked when the screen opens, every 30 s while the tab is visible, when the tab
// is looked at again, and when the operator presses Refresh. A failed reading keeps the last good one.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {useCallback, useEffect, useRef, useState} from 'react';

export const REFRESH_MS = 30_000;

export interface Poll<T> {
  /** The last good reading, null before the first. */
  data: T | null;
  /** When `data` was read (ms), 0 before the first. */
  at: number;
  /** Why the newest reading failed, null when it did not. */
  error: Error | null;
  refresh: () => Promise<void>;
}

/** `load` must keep its identity while it reads the same thing (useCallback): a new one starts over. */
export function usePoll<T>(load: () => Promise<T>): Poll<T> {
  const [data, setData] = useState<T | null>(null);
  const [at, setAt] = useState(0);
  const [error, setError] = useState<Error | null>(null);
  const gone = useRef(false);

  const refresh = useCallback(async () => {
    try {
      const d = await load();
      if (gone.current) return;
      setData(d);
      setAt(Date.now());
      setError(null);
    } catch (e) {
      if (!gone.current) setError(e instanceof Error ? e : new Error(String(e)));
    }
  }, [load]);

  useEffect(() => {
    gone.current = false;
    setData(null);
    setAt(0);
    setError(null);
    void refresh();
    // a hidden tab asks nothing; it catches up when it is looked at again
    const timer = setInterval(() => {
      if (!document.hidden) void refresh();
    }, REFRESH_MS);
    const seen = () => {
      if (!document.hidden) void refresh();
    };
    document.addEventListener('visibilitychange', seen);
    return () => {
      gone.current = true;
      clearInterval(timer);
      document.removeEventListener('visibilitychange', seen);
    };
  }, [refresh]);

  return {data, at, error, refresh};
}
