// The arithmetic of the dashboard's charts (DC_DEFAULTS.md LG-449): axis steps, the day and hour spans a count is
// taken over, and which report a pointer is nearest to. No drawing here, so it is tested without a browser.
// Provenance: ORIGINAL (DeCombat operator tooling)

/** Axis values from 0 up to the first step at or above `max`, at most `most` steps, in steps of 1, 2 or 5 x 10^n. */
export function ticks(max: number, most = 4): number[] {
  const top = Math.max(1, max);
  const raw = top / most;
  const pow = 10 ** Math.floor(Math.log10(raw));
  // every value drawn is a count or a number of milliseconds: a step below 1 says nothing
  const step = Math.max(1, [1, 2, 5, 10].map(k => k * pow).find(s => s >= raw) ?? 10 * pow);
  const out: number[] = [];
  for (let i = 0; i * step < top; i++) out.push(i * step);
  out.push(out.length * step);
  return out;
}

/** The starts (seconds) of the last `n` local days or hours, oldest first; the last one holds `now`. */
export function starts(now: number, n: number, unit: 'day' | 'hour'): number[] {
  const d = new Date(now * 1000);
  if (unit === 'day') d.setHours(0, 0, 0, 0);
  else d.setMinutes(0, 0, 0);
  const out: number[] = [];
  for (let i = n - 1; i >= 0; i--) {
    const s = new Date(d);
    // by the calendar, so a day of 23 or 25 hours is still one day
    if (unit === 'day') s.setDate(d.getDate() - i);
    else s.setHours(d.getHours() - i);
    out.push(s.getTime() / 1000);
  }
  return out;
}

/** Which span a time falls into (-1: before the first one). `from` is ascending; the last span has no end. */
export function binOf(t: number, from: number[]): number {
  let at = -1;
  for (let i = 0; i < from.length; i++) {
    if (t >= (from[i] ?? Infinity)) at = i;
    else break;
  }
  return at;
}

/**
 * From when a list the lobby cut at `limit` rows is whole: the list holds the newest rows, so with `limit` rows in
 * hand anything older than its oldest row may be missing. 0: nothing was cut.
 */
export function wholeFrom(times: number[], limit: number): number {
  if (times.length < limit) return 0;
  return times.reduce((a, b) => Math.min(a, b), Infinity);
}

/** The index of the value nearest to `t` in an ascending list (-1 for an empty one). */
export function nearest(sorted: number[], t: number): number {
  if (sorted.length === 0) return -1;
  let lo = 0;
  let hi = sorted.length - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if ((sorted[mid] ?? 0) <= t) lo = mid;
    else hi = mid;
  }
  return Math.abs((sorted[lo] ?? 0) - t) <= Math.abs((sorted[hi] ?? 0) - t) ? lo : hi;
}

export const dayLabel = (t: number) =>
  new Date(t * 1000).toLocaleDateString(undefined, {month: 'short', day: 'numeric'});

export const hourLabel = (t: number) => `${String(new Date(t * 1000).getHours()).padStart(2, '0')}:00`;
