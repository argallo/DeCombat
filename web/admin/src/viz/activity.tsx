// The two charts of what happened lately, each counted from a list a screen already reads: matches per day (the
// lobby keeps 14 days) and problems per hour (DC_DEFAULTS.md LG-449). A list the lobby cut at its limit does not
// reach all the way back; the spans before its oldest row are drawn as not known, never as nought.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {type Column, Columns, Legend} from './charts.tsx';
import {binOf, dayLabel, hourLabel, starts, wholeFrom} from './scale.ts';

/** The list of GET /ops/matches is asked with this limit. */
export const MATCH_LIMIT = 100;
/** The problem log of GET /ops/events is asked with this limit. */
export const PROBLEM_LIMIT = 300;

const DAYS = 14;
const HOURS = 24;

const plural = (n: number, one: string, many = one + 's') => `${n} ${n === 1 ? one : many}`;

/** Marks the first, the last and about every `every`-th label, so they do not run into each other. */
const marked = (i: number, n: number, every: number) => i === 0 || i === n - 1 || (i % every === 0 && n - 1 - i >= every - 1);

export function MatchesPerDay({rows, now, height}: {
  rows: {started: number; outcome: string}[];
  now: number;
  height?: number;
}) {
  const from = starts(now, DAYS, 'day');
  const whole = wholeFrom(rows.map(r => r.started), MATCH_LIMIT);
  const fine = from.map(() => 0);
  const other = from.map(() => 0);
  for (const r of rows) {
    const i = binOf(r.started, from);
    if (i < 0) continue;
    if (r.outcome === 'result' || r.outcome === 'running') fine[i] = (fine[i] ?? 0) + 1;
    else other[i] = (other[i] ?? 0) + 1;
  }
  const columns: Column[] = from.map((t, i) => ({
    label: dayLabel(t),
    mark: marked(i, DAYS, 4),
    title: i === DAYS - 1 ? `${dayLabel(t)} (today)` : dayLabel(t),
    // a day that ended before the list's oldest match is not in the list at all
    unknown: (from[i + 1] ?? Infinity) <= whole,
    parts: [
      {paint: 's1', value: fine[i] ?? 0, name: 'ended with a result, or running'},
      {paint: 'bad', value: other[i] ?? 0, name: 'ended another way'},
    ],
  }));
  const total = fine.reduce((a, b) => a + b, 0) + other.reduce((a, b) => a + b, 0);
  const bad = other.reduce((a, b) => a + b, 0);
  return (
    <Columns
      columns={columns}
      height={height}
      label={`Matches per day over ${DAYS} days: ${plural(total, 'match', 'matches')}, ${bad} of them ended another way than with a result.`}
      legend={
        <Legend
          items={[
            {paint: 's1', name: 'Ended with a result, or running'},
            {paint: 'bad', name: 'Ended another way'},
          ]}
        />
      }
    />
  );
}

export function ProblemsPerHour({events, now, height}: {events: {at: number}[]; now: number; height?: number}) {
  const from = starts(now, HOURS, 'hour');
  const whole = wholeFrom(events.map(e => e.at), PROBLEM_LIMIT);
  const count = from.map(() => 0);
  for (const e of events) {
    const i = binOf(e.at, from);
    if (i >= 0) count[i] = (count[i] ?? 0) + 1;
  }
  const columns: Column[] = from.map((t, i) => ({
    label: hourLabel(t),
    mark: marked(i, HOURS, 6),
    title: `${dayLabel(t)}, ${hourLabel(t)} to ${hourLabel(t + 3600)}`,
    unknown: (from[i + 1] ?? Infinity) <= whole,
    parts: [{paint: 's2', value: count[i] ?? 0, name: 'problems'}],
  }));
  const total = count.reduce((a, b) => a + b, 0);
  return (
    <Columns
      columns={columns}
      height={height}
      label={`Problems per hour over ${HOURS} hours: ${plural(total, 'problem')}.`}
    />
  );
}
