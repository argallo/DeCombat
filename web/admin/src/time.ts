// Times as the dashboard writes them: the lobby sends seconds since 1970.
// Provenance: ORIGINAL (DeCombat operator tooling)

const pad = (n: number) => String(n).padStart(2, '0');

/** "2026-11-11 05:19:07" in the browser's time zone; "-" without a time. */
export function when(secs: number | null | undefined): string {
  if (!secs) return '-';
  const d = new Date(secs * 1000);
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  );
}

/** "just now", "12 min ago", "3 h ago", then the date. `nowSecs` is the clock to compare with. */
export function ago(secs: number | null | undefined, nowSecs: number): string {
  if (!secs) return '-';
  const s = nowSecs - secs;
  if (s < 90) return 'just now';
  if (s < 5400) return `${Math.round(s / 60)} min ago`;
  if (s < 129600) return `${Math.round(s / 3600)} h ago`;
  return when(secs).slice(0, 10);
}

/** The time of day the way the browser writes it. */
export function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], {hour: '2-digit', minute: '2-digit', second: '2-digit'});
}
