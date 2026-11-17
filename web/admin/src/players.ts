// The rules of the Players screen (DC_DEFAULTS.md LG-450): how the list is asked for, what a row is marked with and
// what is counted from a person's matches. No drawing here, so it is tested without a browser.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {type Health, healthOf, plural} from './overview.ts';

/** One row of GET /ops/players. */
export interface PlayerRow {
  account: number;
  online: boolean;
  login: string;
  /** "" while the account has no character. */
  nick: string;
  created: number;
  last_login: number;
  matches: number;
  kills: number;
  deaths: number;
  gm: boolean;
  bot: boolean;
  banned: boolean;
  disabled: boolean;
  muted: boolean;
}

export interface PlayerPage {
  /** How many accounts pass the filter. */
  total: number;
  offset: number;
  players: PlayerRow[];
}

export type Sort = 'seen' | 'new' | 'matches';

export interface Filter {
  /** A part of the character name or the login. */
  q: string;
  sort: Sort;
  /** Bots are left out unless this is set. */
  bots: boolean;
  /** Only the banned accounts. */
  banned: boolean;
}

export const SORTS: {value: Sort; label: string}[] = [
  {value: 'seen', label: 'Last seen first'},
  {value: 'new', label: 'Newest accounts first'},
  {value: 'matches', label: 'Most matches first'},
];

/** How many rows the list starts with and how many "Show more" adds. */
export const PAGE = 50;
/** The most rows the lobby gives in one answer. */
export const MOST = 200;
/** A person's page asks for this many matches and events. */
export const STAY_LIMIT = 300;

/** The requests that read the first `shown` rows of the list, each for at most [`MOST`] rows. */
export function listRequests(f: Filter, shown: number): string[] {
  const out: string[] = [];
  for (let offset = 0; offset < Math.max(1, shown); offset += MOST) {
    const q = new URLSearchParams({
      offset: String(offset),
      limit: String(Math.min(MOST, Math.max(1, shown) - offset)),
      sort: f.sort,
      bots: f.bots ? '1' : '0',
      banned: f.banned ? '1' : '0',
      q: f.q.trim(),
    });
    out.push(`/ops/players?${q}`);
  }
  return out;
}

export interface Mark {
  variant: 'error' | 'warning' | 'neutral' | 'info';
  label: string;
}

/** What a row says about the account beside its name, the gravest first. */
export function marks(p: Pick<PlayerRow, 'banned' | 'disabled' | 'muted' | 'bot' | 'gm'>): Mark[] {
  const out: Mark[] = [];
  if (p.banned) out.push({variant: 'error', label: 'Banned'});
  if (p.disabled) out.push({variant: 'warning', label: 'Login disabled'});
  if (p.muted) out.push({variant: 'neutral', label: 'Muted'});
  if (p.gm) out.push({variant: 'info', label: 'GM'});
  if (p.bot) out.push({variant: 'neutral', label: 'Bot'});
  return out;
}

/** "Showing 50 of 1,204 players, bots included.", "All 7 players.", or why the list is empty. */
export function countLine(shown: number, total: number, f: Filter): string {
  if (total === 0) {
    if (f.banned) return f.q.trim() ? 'No banned player matches that name.' : 'No banned player.';
    return f.q.trim() ? 'No player matches that name.' : 'No accounts yet.';
  }
  const n = total.toLocaleString('en-US');
  const what = `${f.banned ? 'banned ' : ''}${total === 1 ? 'player' : 'players'}`;
  const tail = f.bots ? ', bots included.' : '.';
  if (shown >= total) return `${total === 1 ? '1' : `All ${n}`} ${what}${tail}`;
  return `Showing ${shown} of ${n} ${what}${tail}`;
}

/** Kills for every death, "-" before the first death. */
export function perDeath(kills: number, deaths: number): string {
  return deaths > 0 ? (kills / deaths).toFixed(2) : '-';
}

/** One stay of a person in a match (GET /ops/timeline, `matches`): written when their connection ended. */
export interface Stay {
  match: number;
  joined: number;
  secs: number;
  /** How the stay ended: "bye" and "end" are the usual ways. */
  how: string;
  code?: number | string | null;
  bot?: boolean;
  has_link?: boolean;
  health?: string | null;
  rtt_mean?: number | null;
  rtt_p95?: number | null;
  rtt_max?: number | null;
  room: string;
  map_name: string;
  mode_key: string;
  host: string;
  outcome: string;
}

/** The person left the usual way: they quit, or the match ended. */
export const leftWell = (how: string) => how === 'bye' || how === 'end';

/** A stay whose connection the game server measured. */
export const measured = (s: Stay) => s.has_link !== false && s.rtt_mean != null && healthOf(s.health) !== 'none';

/** How many of the stays had a good, a shaky and a bad connection, and how many were not measured. */
export function connectionMix(stays: Stay[]): Record<Health, number> {
  const out: Record<Health, number> = {good: 0, warn: 0, bad: 0, none: 0};
  for (const s of stays) out[measured(s) ? healthOf(s.health) : 'none']++;
  return out;
}

/** The middle value (the mean of the two middle ones), null for none. */
export function median(values: number[]): number | null {
  if (values.length === 0) return null;
  const v = [...values].sort((a, b) => a - b);
  const mid = v.length >> 1;
  return v.length % 2 ? (v[mid] ?? null) : ((v[mid - 1] ?? 0) + (v[mid] ?? 0)) / 2;
}

/** "3 good, 1 shaky" from a mix, leaving out what did not happen. */
export function mixLine(mix: Record<Health, number>): string {
  const words: [Health, string][] = [
    ['good', 'good'],
    ['warn', 'shaky'],
    ['bad', 'bad'],
    ['none', 'not measured'],
  ];
  return words
    .filter(([h]) => mix[h] > 0)
    .map(([h, w]) => `${mix[h]} ${w}`)
    .join(', ');
}

/** "12 minutes left" of a mute. */
export const minutesLeft = (n: number) => `${plural(n, 'minute')} left`;
