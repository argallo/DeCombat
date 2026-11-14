// What the Overview draws from: the lobby's answers it reads, and what they say is wrong or waiting right now
// (DC_DEFAULTS.md LG-445). No drawing here, so the rules can be tested without a browser.
// Provenance: ORIGINAL (DeCombat operator tooling)

import type {ScreenId} from './routes.ts';

export type Health = 'good' | 'warn' | 'bad' | 'none';

export interface LivePlayer {
  nick: string;
  bot?: boolean;
  health?: string | null;
  rtt?: number | null;
  rtt_max?: number | null;
}

export interface LiveMatch {
  match: number | string;
  room: number | string;
  mode: string;
  map: string | number;
  host: string;
  port: number;
  ready?: boolean;
  players: LivePlayer[];
}

export interface LiveHost {
  name: string;
  local?: boolean;
  online: boolean;
  version?: string | null;
  version_matches?: boolean;
  online_secs?: number;
  games: number;
  address?: string | null;
}

export interface LiveRoom {
  id: string;
  title: string;
  state: string;
  mode: string;
  players: number;
  max_players: number;
  bots: number;
  members: string[];
}

/** GET /ops/live. */
export interface Live {
  lobby: {online: number; build: string; uptime_secs: number; stopping?: boolean};
  matches: LiveMatch[];
  rooms: LiveRoom[];
  hosts: LiveHost[];
}

/** One row of GET /ops/alerts. */
export interface AlertCheck {
  name: string;
  ok: boolean;
  text?: string;
}

/** One row of GET /ops/events. */
export interface OpsEvent {
  at: number;
}

/** The part of GET /ops/bots the Overview reads. */
export interface BotsState {
  managed?: boolean;
  exe_missing?: boolean;
  running?: number;
  wanted?: number;
}

/** The part of GET /ops/updates the Overview reads. */
export interface Updates {
  set_up?: boolean;
  deploying?: boolean;
  pending?: boolean;
  status?: {ok?: boolean; finished?: number; short?: string; sha?: string} | null;
  available?: {ok?: boolean; new?: unknown[]} | null;
}

/** The part of GET /ops/updates/release the Overview reads. */
export interface Release {
  set_up?: boolean;
  status?: {phase?: string; next_tag?: string} | null;
}

/** The part of GET /ops/triage the Overview reads. */
export interface Triage {
  set_up?: boolean;
  open?: number;
  state?: {ok?: boolean; message?: string} | null;
}

/** Everything one refresh read. Only `live` is needed; the others are null when not asked or not answered. */
export interface Snapshot {
  live: Live;
  alerts: AlertCheck[] | null;
  problems: OpsEvent[] | null;
  bots: BotsState | null;
  updates: Updates | null;
  release: Release | null;
  triage: Triage | null;
}

/** bad: broken now. warn: degraded or failing. info: waiting for the operator, nothing is wrong. */
export type Level = 'bad' | 'warn' | 'info';

export interface Finding {
  level: Level;
  text: string;
  /** The screen where it is looked at or fixed. */
  screen: ScreenId;
  /** What that screen opens (a match id), "" for its front page. */
  arg: string;
}

export const plural = (n: number, one: string, many = one + 's') => `${n} ${n === 1 ? one : many}`;

export function healthOf(h: string | null | undefined): Health {
  return h === 'good' || h === 'warn' || h === 'bad' ? h : 'none';
}

/** The worst connection among a match's people (bots do not count); "none" without a report. */
export function worstHealth(players: LivePlayer[]): Health {
  let worst: Health = 'none';
  for (const p of players) {
    if (p.bot) continue;
    const h = healthOf(p.health);
    if (h === 'bad') return 'bad';
    if (h === 'warn' || (h === 'good' && worst === 'none')) worst = h;
  }
  return worst;
}

const RANK: Record<Level, number> = {bad: 0, warn: 1, info: 2};

/** One match id when every named player is in the same match, else "" (the list of matches). */
function oneMatch(list: {match: string}[]): string {
  const first = list[0]?.match ?? '';
  return list.every(x => x.match === first) ? first : '';
}

/**
 * What is wrong or waiting, the most serious first (within a level in the order below). `nowSecs` is the clock the
 * events' `at` is compared with.
 */
export function findings(s: Snapshot, nowSecs: number): Finding[] {
  const out: Finding[] = [];
  const add = (level: Level, text: string, screen: ScreenId, arg = '') => out.push({level, text, screen, arg});
  const {live} = s;

  if (live.lobby.stopping) add('bad', 'The lobby is shutting down.', 'overview');
  for (const h of live.hosts) {
    if (!h.online) add('bad', `${h.name} is offline: rooms on it cannot start.`, 'overview');
    else if (h.version_matches === false) {
      add('warn', `${h.name} runs build ${h.version ?? '?'}, the lobby runs ${live.lobby.build}.`, 'overview');
    }
  }

  const bad: {nick: string; room: string; match: string}[] = [];
  const shaky: typeof bad = [];
  for (const m of live.matches) {
    for (const p of m.players) {
      if (p.bot) continue;
      const who = {nick: p.nick, room: String(m.room), match: String(m.match ?? '')};
      if (p.health === 'bad') bad.push(who);
      else if (p.health === 'warn') shaky.push(who);
    }
  }
  if (bad.length === 1 && bad[0]) {
    add('bad', `${bad[0].nick} has a bad connection in room ${bad[0].room}.`, 'matches', bad[0].match);
  } else if (bad.length > 1) {
    const names = bad.map(b => b.nick).join(', ');
    add('bad', `${bad.length} players have a bad connection: ${names}.`, 'matches', oneMatch(bad));
  }
  if (shaky.length === 1 && shaky[0]) {
    add('warn', `${shaky[0].nick}'s connection is shaky.`, 'matches', shaky[0].match);
  } else if (shaky.length > 1) {
    add('warn', `${shaky.length} players' connections are shaky.`, 'matches', oneMatch(shaky));
  }

  const failing = (s.alerts ?? []).filter(a => !a.ok);
  if (failing.length) {
    add('warn', `${plural(failing.length, 'alert check')} failing: ${failing.map(a => a.name).join(', ')}.`, 'health');
  }
  const recent = (s.problems ?? []).filter(e => e.at >= nowSecs - 3600).length;
  if (recent) add('warn', `${plural(recent, 'problem')} logged in the last hour.`, 'health');

  const b = s.bots;
  if (b?.managed) {
    if (b.exe_missing) add('bad', 'dc-botclient is missing: no bot can start.', 'bots');
    else if ((b.running ?? 0) < (b.wanted ?? 0)) {
      add('warn', `${b.running ?? 0} of ${plural(b.wanted ?? 0, 'bot')} running.`, 'bots');
    }
  }

  const u = s.updates;
  if (u?.set_up) {
    const st = u.status;
    const what = st?.short || (st?.sha ?? '').slice(0, 8) || 'the build';
    if (u.deploying) add('info', `Deploying ${what}.`, 'deploy');
    else if (u.pending) add('info', 'A deploy is requested.', 'deploy');
    else if (st && st.ok === false && st.finished && nowSecs - st.finished < 3600) {
      add('warn', `The deploy of ${what} failed.`, 'deploy');
    }
    if (u.available?.ok === false) add('warn', 'The check for new commits is failing.', 'deploy');
    const news = u.available?.new?.length ?? 0;
    if (news && !u.deploying && !u.pending) add('info', `${plural(news, 'new commit')} on master.`, 'deploy');
  }

  const r = s.release;
  if (r?.set_up && r.status) {
    if (r.status.phase === 'failed') add('warn', 'The public release failed.', 'release');
    else if (r.status.phase === 'ready') {
      add('info', `Release ${r.status.next_tag || ''} is ready to tag.`.replace('  ', ' '), 'release');
    }
  }

  const t = s.triage;
  if (t?.set_up) {
    if (t.state?.ok === false) add('warn', 'The bug report poll is failing.', 'bugs');
    if ((t.open ?? 0) > 0) add('info', `${plural(t.open ?? 0, 'open bug report')}.`, 'bugs');
  }

  // a stable sort keeps the order above within a level
  return out.sort((x, y) => RANK[x.level] - RANK[y.level]);
}

/** The headline: the worst thing that is wrong; something only waiting does not change "All clear." */
export function headline(list: Finding[]): {level: 'good' | 'warn' | 'bad'; text: string} {
  const worst = list.find(f => f.level !== 'info');
  return worst ? {level: worst.level === 'bad' ? 'bad' : 'warn', text: worst.text} : {level: 'good', text: 'All clear.'};
}

/** "42 s", "12 min", "3.5 h", "4 days". */
export function duration(secs: number): string {
  const s = Math.round(secs || 0);
  if (s < 90) return `${s} s`;
  if (s < 5400) return `${Math.round(s / 60)} min`;
  if (s < 172800) return `${(s / 3600).toFixed(1)} h`;
  return `${Math.round(s / 86400)} days`;
}
