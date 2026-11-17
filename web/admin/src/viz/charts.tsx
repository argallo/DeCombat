// The dashboard's charts (DC_DEFAULTS.md LG-449): a meter, a split bar, columns over time and lines over time, drawn
// with plain elements and one stylesheet (viz.css). Every name and label is put in as text. A chart never is the only
// place a number can be read: each one sits beside the list or the figures it is drawn from.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {type PointerEvent, type ReactNode, useMemo, useState} from 'react';
import {nearest, ticks} from './scale.ts';

/** What a mark is painted with: a state, one of the four series colours, or the grey of everything else. */
export type Paint = 'good' | 'warn' | 'bad' | 'none' | 's1' | 's2' | 's3' | 's4' | 'rest' | 'empty';

const pct = (v: number, top: number) => Math.min(100, Math.max(0, (v / (top || 1)) * 100));

/** One value on a fixed scale, so two meters compare by eye; `tick` marks a second value (the worst). */
export function Meter({value, max, paint, tick, label}: {
  value: number;
  max: number;
  paint: Paint;
  tick?: number | null;
  label: string;
}) {
  return (
    <div className={`viz viz-meter viz-${paint}`} role="img" aria-label={label} title={label}>
      {value > 0 && <span className="fill" style={{width: `${Math.max(3, pct(value, max))}%`}} />}
      {tick != null && tick > value && <span className="tick" style={{left: `calc(${pct(tick, max)}% - 1px)`}} />}
    </div>
  );
}

export interface SplitPart {
  paint: Paint;
  value: number;
  name: string;
}

/** One whole split into its parts, widths by value; a part of nothing draws nothing. */
export function Split({parts, label, thick}: {parts: SplitPart[]; label: string; thick?: boolean}) {
  const shown = parts.filter(p => p.value > 0);
  return (
    <div className={`viz viz-split${thick ? ' thick' : ''}`} role="img" aria-label={label}>
      {shown.length === 0 ? (
        <span className="viz-none" style={{flexGrow: 1, opacity: 0.35}} />
      ) : (
        shown.map(p => (
          <span key={p.name} className={`viz-${p.paint}`} style={{flexGrow: p.value}} title={`${p.name}: ${p.value}`} />
        ))
      )}
    </div>
  );
}

/** The names of a chart's colours, in the order they are drawn. */
export function Legend({items}: {items: {paint: Paint; name: string; line?: boolean}[]}) {
  return (
    <div className="viz viz-legend">
      {items.map(i => (
        <span key={i.name}>
          <span className={`viz-key viz-${i.paint}${i.line ? ' line' : ''}`} />
          {i.name}
        </span>
      ))}
    </div>
  );
}

export interface TipRow {
  paint?: Paint;
  value: string;
  name: string;
}

/** `x` is 0 to 1 across the plot; the box sits beside it, on the side that has the room. */
function Tip({x, head, rows}: {x: number; head: string; rows: TipRow[]}) {
  const side = x < 0.5 ? {left: `calc(${x * 100}% + 12px)`} : {right: `calc(${(1 - x) * 100}% + 12px)`};
  return (
    <div className="viz-tip" style={side}>
      <div className="head">{head}</div>
      {rows.map(r => (
        <div className="row" key={r.name}>
          {r.paint && <span className={`viz-key line viz-${r.paint}`} />}
          <b>{r.value}</b>
          <i>{r.name}</i>
        </div>
      ))}
    </div>
  );
}

function YAxis({values, unit}: {values: number[]; unit: string}) {
  const top = values[values.length - 1] ?? 1;
  return (
    <div className="viz-yaxis" aria-hidden="true">
      {/* the widest value gives the column its width */}
      <span>
        {top}
        {unit}
      </span>
      {values.map(v => (
        <b key={v} style={{bottom: `${pct(v, top)}%`}}>
          {v}
          {v === top ? unit : ''}
        </b>
      ))}
    </div>
  );
}

function Rules({values}: {values: number[]}) {
  const top = values[values.length - 1] ?? 1;
  return (
    <>
      {values.map(v => (
        <span key={v} className={`rule${v === 0 ? ' base' : ''}`} style={{bottom: `${pct(v, top)}%`}} />
      ))}
    </>
  );
}

export interface Column {
  /** Under the column when `mark` is set (the first, the last and a few between). */
  label: string;
  mark?: boolean;
  /** The pointer's box: what span of time this is. */
  title: string;
  parts: SplitPart[];
  /** The list this is counted from does not reach back to this span. */
  unknown?: boolean;
  /** The pointer's box, when it says more than the parts' values (a unit, a second figure). */
  tip?: TipRow[];
}

/** A count per span of time; the parts of a column are stacked from the baseline. */
export function Columns({columns, label, height = 120, legend, unit = ''}: {
  columns: Column[];
  label: string;
  height?: number;
  legend?: ReactNode;
  /** Written after the axis' top value. */
  unit?: string;
}) {
  const [on, setOn] = useState<number | null>(null);
  const total = (c: Column) => c.parts.reduce((n, p) => n + p.value, 0);
  const axis = ticks(Math.max(0, ...columns.map(total)), 3);
  const top = axis[axis.length - 1] ?? 1;
  const hit = on === null ? undefined : columns[on];
  return (
    <div className="viz" role="img" aria-label={label}>
      {legend}
      <div className="viz-chart">
        <YAxis values={axis} unit={unit} />
        <div className="viz-plot" style={{height}} onPointerLeave={() => setOn(null)}>
          <Rules values={axis} />
          <div className="viz-bars">
            {columns.map((c, i) => (
              <div
                key={i}
                className={`slot${on === i ? ' on' : ''}${c.unknown ? ' unknown' : ''}`}
                onPointerEnter={() => setOn(i)}
                onPointerDown={() => setOn(i)}>
                {c.parts
                  .filter(p => p.value > 0)
                  .map(p => (
                    <span key={p.name} className={`seg viz-${p.paint}`} style={{height: `${pct(p.value, top)}%`}} />
                  ))}
              </div>
            ))}
          </div>
          {hit && on !== null && (
            <Tip
              x={(on + 0.5) / columns.length}
              head={hit.title}
              rows={
                hit.unknown
                  ? [{value: 'not known', name: 'older than the list reaches'}]
                  : hit.tip
                    ? hit.tip
                    : total(hit) === 0
                    ? [{value: '0', name: 'none'}]
                    : hit.parts.filter(p => p.value > 0).map(p => ({paint: p.paint, value: String(p.value), name: p.name}))
              }
            />
          )}
        </div>
        <div className="viz-xaxis" aria-hidden="true">
          {columns.map((c, i) => (
            <span key={i} className={i === 0 ? 'first' : i === columns.length - 1 ? 'last' : ''}>
              {c.mark ? c.label : ''}
            </span>
          ))}
        </div>
      </div>
    </div>
  );
}

export interface Series {
  name: string;
  paint: Paint;
  /** Ascending by time (seconds). */
  points: {t: number; v: number}[];
}

/** Values over time on one axis. The pointer finds the time; its box lists every series at that time. */
export function Lines({series, from, to, unit, label, height = 180, floor = 0, breakAfter = 25, ends, time, legend}: {
  series: Series[];
  from: number;
  to: number;
  unit: string;
  label: string;
  height?: number;
  /** The axis reaches at least this value, so a quiet chart is not blown up. */
  floor?: number;
  /** A line is broken where two of its points are further apart than this (seconds). */
  breakAfter?: number;
  /** The words under the two ends of the plot. */
  ends: [string, string];
  /** How the pointer's box names a time. */
  time: (t: number) => string;
  legend?: ReactNode;
}) {
  const [at, setAt] = useState<number | null>(null);
  const times = useMemo(
    () => [...new Set(series.flatMap(s => s.points.map(p => p.t)))].sort((a, b) => a - b),
    [series],
  );
  const span = Math.max(1, to - from);
  const axis = ticks(Math.max(floor, ...series.flatMap(s => s.points.map(p => p.v))), 4);
  const top = axis[axis.length - 1] ?? 1;
  const x = (t: number) => ((t - from) / span) * 1000;
  const y = (v: number) => 1000 - (v / top) * 1000;
  const path = (s: Series) =>
    s.points
      .map((p, i) => {
        const prev = s.points[i - 1];
        const pen = !prev || p.t - prev.t > breakAfter ? 'M' : 'L';
        return `${pen}${x(p.t).toFixed(1)} ${y(p.v).toFixed(1)}`;
      })
      .join(' ');
  const point = (e: PointerEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    if (r.width <= 0) return;
    const i = nearest(times, from + ((e.clientX - r.left) / r.width) * span);
    setAt(i < 0 ? null : i);
  };
  const t = at === null ? undefined : times[at];
  // each series' value at the pointer's time: its report of that 10 s, when it has one
  const here =
    t === undefined
      ? []
      : series.flatMap(s => {
          const i = nearest(s.points.map(p => p.t), t);
          const p = s.points[i];
          return p && Math.abs(p.t - t) <= breakAfter / 2 ? [{s, p}] : [];
        });
  // the grey lines are drawn first, so a coloured one is never under them
  const drawn = [...series.filter(s => s.paint === 'rest'), ...series.filter(s => s.paint !== 'rest')];
  return (
    <div className="viz" role="img" aria-label={label}>
      {legend}
      <div className="viz-chart">
        <YAxis values={axis} unit={unit} />
        <div
          className="viz-plot"
          style={{height}}
          onPointerMove={point}
          onPointerDown={point}
          onPointerLeave={() => setAt(null)}>
          <Rules values={axis} />
          <svg viewBox="0 0 1000 1000" preserveAspectRatio="none" aria-hidden="true">
            {drawn.map(s => (
              <path key={s.name} className={`viz-${s.paint}`} d={path(s)} />
            ))}
          </svg>
          {t !== undefined && (
            <>
              <span className="cross" style={{left: `${x(t) / 10}%`}} />
              {here.map(({s, p}) => (
                <span
                  key={s.name}
                  className={`dot viz-${s.paint}`}
                  style={{left: `${x(p.t) / 10}%`, top: `${y(p.v) / 10}%`}}
                />
              ))}
              <Tip
                x={Math.min(1, Math.max(0, x(t) / 1000))}
                head={time(t)}
                rows={
                  here.length
                    ? [...here]
                        .sort((a, b) => b.p.v - a.p.v)
                        .slice(0, 8)
                        .map(({s, p}) => ({paint: s.paint, value: `${Math.round(p.v)}${unit}`, name: s.name}))
                    : [{value: '-', name: 'no report'}]
                }
              />
            </>
          )}
        </div>
        <div className="viz-xaxis ends" aria-hidden="true">
          <span>{ends[0]}</span>
          <span>{ends[1]}</span>
        </div>
      </div>
    </div>
  );
}

/** A row of dots, one per thing, each in its state's colour (the game servers). */
export function Dots({items, label}: {items: {paint: Paint; name: string}[]; label: string}) {
  return (
    <div className="viz viz-dots" role="img" aria-label={label}>
      {items.map(i => (
        <span key={i.name} className={`viz-${i.paint}`} title={i.name} />
      ))}
    </div>
  );
}

/** The status headline: a coloured sign, the one sentence, and what belongs under it. */
export function Hero({paint, sign, children}: {paint: Paint; sign: ReactNode; children: ReactNode}) {
  return (
    <div className={`viz viz-hero viz-${paint}`} role="status">
      <span className="sign" aria-hidden="true">
        {sign}
      </span>
      <div style={{minWidth: 0}}>{children}</div>
    </div>
  );
}
