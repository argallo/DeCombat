// Players: every account as a list that is searched, ordered and filtered, and one person's page: what the account
// is now, its balances, its matches of the last 14 days with the connection of each, and its events (the classic
// tab Players; DC_DEFAULTS.md LG-450). The changes to an account are PlayerActions.tsx (LG-451).
// Provenance: ORIGINAL (DeCombat operator tooling)

import {useCallback, useEffect, useState} from 'react';
import {type Api, ApiError} from '../api.ts';
import {type Health, duration, healthOf, plural} from '../overview.ts';
import {
  type Filter,
  type PlayerPage,
  type PlayerRow,
  type Sort,
  type Stay,
  PAGE,
  SORTS,
  STAY_LIMIT,
  connectionMix,
  countLine,
  leftWell,
  listRequests,
  marks,
  measured,
  median,
  minutesLeft,
  mixLine,
  perDeath,
} from '../players.ts';
import {usePoll} from '../poll.ts';
import {toHash} from '../routes.ts';
import {ago, when} from '../time.ts';
import {
  Badge,
  Banner,
  Button,
  Card,
  CheckboxInput,
  EmptyState,
  Grid,
  HStack,
  List,
  ListItem,
  Selector,
  StatusDot,
  Text,
  TextInput,
  VStack,
} from '../ui/index.ts';
import {MatchesPerDay} from '../viz/activity.tsx';
import {type Column, type Paint, Columns, Legend, Meter, Split} from '../viz/charts.tsx';
import {dayLabel} from '../viz/scale.ts';
import {type EventRow, EventLog} from './EventLog.tsx';
import {RTT_SCALE} from './Overview.tsx';
import {Fact, Part, Quiet, ScreenHead, Tile} from './parts.tsx';
import {PlayerActions} from './PlayerActions.tsx';

const num = (n: number | null | undefined) => Number(n ?? 0).toLocaleString('en-US');

const HEALTH_DOT: Record<Health, 'success' | 'warning' | 'error' | 'neutral'> = {
  good: 'success',
  warn: 'warning',
  bad: 'error',
  none: 'neutral',
};
const HEALTH_SAYS: Record<Health, string> = {
  good: 'Good connection',
  warn: 'Shaky connection',
  bad: 'Bad connection',
  none: 'Connection not measured',
};

// ---- the list

/** What the list showed last, so that the way back from a person's page finds it as it was left. */
let kept: {filter: Filter; text: string; shown: number} = {
  filter: {q: '', sort: 'seen', bots: false, banned: false},
  text: '',
  shown: PAGE,
};

async function loadList(api: Api, f: Filter, shown: number): Promise<PlayerPage> {
  const players: PlayerRow[] = [];
  let total = 0;
  for (const path of listRequests(f, shown)) {
    const page = await api.get<PlayerPage>(path);
    total = page.total;
    players.push(...page.players);
    if (page.players.length === 0 || players.length >= total) break;
  }
  return {total, offset: 0, players};
}

function PlayerList({api}: {api: Api}) {
  const [text, setText] = useState(kept.text);
  const [filter, setFilter] = useState(kept.filter);
  const [shown, setShown] = useState(kept.shown);
  const change = (part: Partial<Filter>) => {
    setFilter(f => ({...f, ...part}));
    setShown(PAGE);
  };
  // typing asks the lobby after a short pause, not at every letter
  useEffect(() => {
    const q = text.trim();
    if (q === filter.q) return;
    const timer = setTimeout(() => {
      setFilter(f => ({...f, q}));
      setShown(PAGE);
    }, 250);
    return () => clearTimeout(timer);
  }, [text, filter.q]);
  useEffect(() => {
    kept = {filter, text, shown};
  }, [filter, text, shown]);

  const poll = usePoll(useCallback(() => loadList(api, filter, shown), [api, filter, shown]), true);
  const page = poll.data;
  const now = Date.now() / 1000;
  const most = Math.max(1, ...(page?.players ?? []).map(p => p.matches));
  return (
    <VStack gap={5}>
      <ScreenHead title="Players" poll={poll} />
      <HStack gap={3} align="end" wrap="wrap">
        <TextInput
          type="search"
          label="Find a player"
          placeholder="Character name or login"
          value={text}
          onChange={setText}
          hasClear
          width={280}
          autoComplete="off"
          hasSpellCheck={false}
          autoCapitalize="none"
          enterKeyHint="search"
        />
        <Selector
          label="Order"
          options={SORTS}
          value={filter.sort}
          onChange={v => change({sort: v as Sort})}
          width={220}
        />
        <CheckboxInput label="Include bots" value={filter.bots} onChange={v => change({bots: v})} />
        <CheckboxInput label="Only banned" value={filter.banned} onChange={v => change({banned: v})} />
      </HStack>
      {page && (
        <VStack gap={2}>
          <Text type="supporting" as="p">
            {countLine(page.players.length, page.total, filter)}
            {page.total > 0 ? ' A player opens with the account, the matches and the events.' : ''}
          </Text>
          {page.players.length > 0 && (
            <Card padding={0}>
              <List hasDividers>
                {page.players.map(p => {
                  const flags = marks(p);
                  return (
                    <ListItem
                      key={p.account}
                      label={
                        <Text type="body" weight="medium">
                          {p.nick || 'No character yet'}
                          <Text type="body" color="secondary">
                            {' '}
                            · {p.login}
                          </Text>
                        </Text>
                      }
                      description={
                        <VStack gap={1}>
                          <Text type="supporting" as="div">
                            {p.online ? 'Online now' : p.last_login ? `Last seen ${ago(p.last_login, now)}` : 'Never signed in'}
                            {' · '}joined {when(p.created).slice(0, 10)} · {num(p.kills)} {p.kills === 1 ? 'kill' : 'kills'},{' '}
                            {num(p.deaths)} {p.deaths === 1 ? 'death' : 'deaths'}
                          </Text>
                          {flags.length > 0 && (
                            <HStack gap={1} wrap="wrap">
                              {flags.map(f => (
                                <Badge key={f.label} variant={f.variant} label={f.label} />
                              ))}
                            </HStack>
                          )}
                        </VStack>
                      }
                      href={toHash('players', p.nick || p.login)}
                      align="first-line"
                      startContent={
                        <StatusDot variant={p.online ? 'success' : 'neutral'} label={p.online ? 'Online' : 'Offline'} />
                      }
                      endContent={
                        // how much they played, against the most of the rows shown
                        <div style={{width: 92}}>
                          <Text type="supporting" as="div" hasTabularNumbers textWrap="nowrap">
                            {plural(p.matches, 'match', 'matches')}
                          </Text>
                          <div className="viz-under">
                            <Meter
                              value={p.matches}
                              max={most}
                              paint="s1"
                              label={`${plural(p.matches, 'match', 'matches')} played; the most in this list is ${most}`}
                            />
                          </div>
                        </div>
                      }
                    />
                  );
                })}
              </List>
            </Card>
          )}
          {page.players.length < page.total && (
            <HStack gap={2}>
              <Button
                label={`Show ${Math.min(PAGE, page.total - page.players.length)} more`}
                variant="secondary"
                size="sm"
                onClick={() => setShown(page.players.length + PAGE)}
              />
            </HStack>
          )}
        </VStack>
      )}
    </VStack>
  );
}

// ---- one person

interface Timeline {
  account: number;
  nick: string;
  login: string;
  events: EventRow[];
  matches: Stay[];
  /** Other accounts the name also fits. */
  other_matches: {account: number; nick: string; login: string}[];
}

/** GET /ops/player. */
interface Account {
  account: number;
  login: string;
  nick: string;
  has_character: boolean;
  created: number;
  last_login: number;
  gm: boolean;
  bot: boolean;
  pool_bot: boolean;
  disabled: boolean;
  has_email: boolean;
  email: string;
  mail_on: boolean;
  exp: number;
  rank: number;
  gp: number;
  nx: number;
  nx_credit: number;
  online: boolean;
  where: string;
  ban: {until: number; permanent: boolean; reason: string; by: string; since: number} | null;
  mute: {minutes_left: number; until: number} | null;
  caps: {nx: number; gp: number; exp: number};
  max_mute_minutes: number;
}

interface Person {
  t: Timeline;
  /** null with `why`: the account's facts did not load, the matches and events did. */
  a: Account | null;
  why: string;
  /** The account's row of the list: its totals since it was made. */
  row: PlayerRow | null;
}

/** null: the lobby has no account or character with that name. */
async function loadPerson(api: Api, name: string): Promise<Person | null> {
  let t: Timeline;
  try {
    t = await api.get<Timeline>(`/ops/timeline?name=${encodeURIComponent(name)}&limit=${STAY_LIMIT}`);
  } catch (e) {
    if (e instanceof ApiError && (e.status === 404 || e.status === 400)) return null;
    throw e;
  }
  const [a, row] = await Promise.all([
    api.get<Account>(`/ops/player?account=${t.account}`).then(
      a => ({a, why: ''}),
      (e: unknown) => {
        // a session that ended is the screen's matter, not this part's
        if (e instanceof ApiError && e.signedOut) throw e;
        return {a: null, why: e instanceof Error ? e.message : String(e)};
      },
    ),
    // the totals are only in the list's rows; without them the page is whole, a tile short
    api
      .get<PlayerPage>(listRequests({q: t.login, sort: 'seen', bots: true, banned: false}, 200)[0] ?? '')
      .then(
        p => p.players.find(r => r.account === t.account) ?? null,
        () => null,
      ),
  ]);
  return {t, ...a, row};
}

/** The mean round trip of each of the newest stays that were measured, oldest on the left. */
function RoundTripPerMatch({stays}: {stays: Stay[]}) {
  const SHOWN = 40;
  const drawn = stays.filter(measured).slice(0, SHOWN).reverse();
  if (drawn.length === 0) return <Quiet>No connection of theirs was measured in the last 14 days.</Quiet>;
  const columns: Column[] = drawn.map((s, i) => {
    const h = healthOf(s.health) as Paint;
    return {
      label: dayLabel(s.joined),
      mark: i === 0 || (i === drawn.length - 1 && drawn.length > 6),
      title: `Match ${s.match}, ${when(s.joined).slice(0, 16)}`,
      parts: [{paint: h, value: Number(s.rtt_mean), name: 'mean round trip'}],
      tip: [
        {paint: h, value: `${Math.round(Number(s.rtt_mean))} ms`, name: 'mean round trip'},
        {value: s.rtt_max == null ? '-' : `${Math.round(s.rtt_max)} ms`, name: 'worst'},
        {value: duration(s.secs), name: `stayed, left by ${s.how || 'unknown'}`},
      ],
    };
  });
  const mix = connectionMix(drawn);
  const names: [Health, string][] = [
    ['good', 'Good'],
    ['warn', 'Shaky'],
    ['bad', 'Bad'],
  ];
  return (
    <Columns
      columns={columns}
      height={140}
      unit=" ms"
      label={`Mean round trip in each of the newest ${plural(drawn.length, 'match', 'matches')}: ${mixLine(mix)}.`}
      legend={<Legend items={names.filter(([h]) => mix[h] > 0).map(([h, name]) => ({paint: h as Paint, name}))} />}
    />
  );
}

/** How many of a person's matches are listed before "Show all". */
const FIRST = 30;

function Stays({stays}: {stays: Stay[]}) {
  const [all, setAll] = useState(false);
  if (stays.length === 0) return <Quiet>No match in the last 14 days.</Quiet>;
  const rows = all ? stays : stays.slice(0, FIRST);
  return (
    <VStack gap={2}>
      <Card padding={0}>
        <List hasDividers>
          {rows.map((s, i) => {
            const h = measured(s) ? healthOf(s.health) : 'none';
            return (
              <ListItem
                key={`${s.match}:${s.joined}:${i}`}
                label={
                  <Text type="body" weight="medium">
                    Match {s.match}
                    <Text type="body" color="secondary">
                      {' '}
                      · {s.mode_key} on {s.map_name}
                    </Text>
                  </Text>
                }
                description={
                  <VStack gap={1}>
                    <Text type="supporting" as="div">
                      {when(s.joined)} · stayed {duration(s.secs)} · room {s.room} · {s.host}
                    </Text>
                    {!leftWell(s.how) && (
                      <HStack gap={1}>
                        <Badge variant="error" label={`left by ${s.how || 'unknown'}${s.code ? ` (${s.code})` : ''}`} />
                      </HStack>
                    )}
                  </VStack>
                }
                href={toHash('matches', String(s.match))}
                align="first-line"
                startContent={<StatusDot variant={HEALTH_DOT[h]} label={HEALTH_SAYS[h]} />}
                endContent={
                  h === 'none' ? (
                    <Text type="supporting" textWrap="nowrap">
                      not measured
                    </Text>
                  ) : (
                    // the same fixed scale as everywhere, so two matches compare by eye
                    <div style={{width: 92}}>
                      <Text type="supporting" as="div" hasTabularNumbers textWrap="nowrap">
                        {Math.round(Number(s.rtt_mean))} ms
                      </Text>
                      <div className="viz-under">
                        <Meter
                          value={Number(s.rtt_mean)}
                          max={RTT_SCALE}
                          paint={h}
                          tick={s.rtt_max}
                          label={`Round trip ${s.rtt_mean} ms, worst ${s.rtt_max ?? '-'} ms, on a scale of 0 to ${RTT_SCALE} ms`}
                        />
                      </div>
                    </div>
                  )
                }
              />
            );
          })}
        </List>
      </Card>
      {stays.length > FIRST && (
        <HStack gap={2}>
          <Button
            label={all ? `Show the newest ${FIRST}` : `Show all ${stays.length}`}
            variant="secondary"
            size="sm"
            onClick={() => setAll(!all)}
          />
        </HStack>
      )}
    </VStack>
  );
}

function PersonTiles({a, row, stays}: {a: Account | null; row: PlayerRow | null; stays: Stay[]}) {
  const mix = connectionMix(stays);
  const typical = median(stays.filter(measured).map(s => Number(s.rtt_mean)));
  const badly = stays.filter(s => !leftWell(s.how)).length;
  const key = (paint: Paint, name: string) => (
    <div className="viz-note">
      <Legend items={[{paint, name}]} />
    </div>
  );
  return (
    <Grid columns={{minWidth: 150, max: 6}} gap={3}>
      {a && <Tile label="Rank" value={String(a.rank)} note={`${num(a.exp)} EXP`} />}
      {a && <Tile label="GP" value={num(a.gp)} />}
      {a && <Tile label="NX" value={num(a.nx)} note={a.nx_credit ? `and ${num(a.nx_credit)} NX credit` : undefined} />}
      {row && (
        <Tile
          label="Kills for every death"
          value={perDeath(row.kills, row.deaths)}
          visual={
            <Split
              label={`${num(row.kills)} kills, ${num(row.deaths)} deaths`}
              parts={[
                {paint: 's1', value: row.kills, name: 'kills'},
                {paint: 's2', value: row.deaths, name: 'deaths'},
              ]}
            />
          }
          note={
            <div className="viz-note">
              <Legend
                items={[
                  {paint: 's1', name: `${num(row.kills)} kills`},
                  {paint: 's2', name: `${num(row.deaths)} deaths`},
                ]}
              />
            </div>
          }
        />
      )}
      <Tile
        label="Matches, 14 days"
        value={String(stays.length)}
        visual={
          <Split
            label={`${stays.length - badly} left the usual way, ${badly} another way`}
            parts={[
              {paint: 's1', value: stays.length - badly, name: 'left the usual way'},
              {paint: 'bad', value: badly, name: 'left another way'},
            ]}
          />
        }
        note={
          badly > 0
            ? key('bad', `${badly} left another way`)
            : row
              ? `${num(row.matches)} since the account was made`
              : undefined
        }
      />
      <Tile
        label="Round trip, typical"
        value={typical == null ? '-' : `${Math.round(typical)} ms`}
        visual={
          <Split
            label={`The connections of their matches: ${mixLine(mix) || 'none'}`}
            parts={[
              {paint: 'good', value: mix.good, name: 'good'},
              {paint: 'warn', value: mix.warn, name: 'shaky'},
              {paint: 'bad', value: mix.bad, name: 'bad'},
              {paint: 'none', value: mix.none, name: 'not measured'},
            ]}
          />
        }
        note={stays.length ? mixLine(mix) : undefined}
      />
    </Grid>
  );
}

function AccountFacts({a}: {a: Account}) {
  const kind = `${a.gm ? 'GM account' : 'Player'}${a.bot ? (a.pool_bot ? ', bot (pool seat)' : ', bot') : ''}`;
  return (
    <Card padding={3}>
      <Grid columns={{minWidth: 150, max: 5}} gap={3}>
        <Fact label="Login" value={a.login} />
        <Fact label="Character" value={a.has_character ? a.nick : 'none yet'} />
        <Fact label="Account number" value={String(a.account)} />
        <Fact label="Kind" value={kind} />
        <Fact label="Created" value={when(a.created)} />
        <Fact label="Last login" value={a.last_login ? when(a.last_login) : 'never'} />
        <Fact label="Can log in" value={a.disabled ? 'No, disabled' : 'Yes'} />
        <Fact label="Chat" value={a.mute ? `Muted, ${minutesLeft(a.mute.minutes_left)}` : 'Not muted'} />
        <Fact label="Recovery address" value={a.has_email ? a.email : 'none'} />
        <Fact
          label="Ban"
          value={!a.ban ? 'Not banned' : a.ban.permanent ? 'Banned for good' : `Banned until ${when(a.ban.until)}`}
        />
      </Grid>
    </Card>
  );
}

function PersonPage({api, name}: {api: Api; name: string}) {
  const poll = usePoll(useCallback(async () => ({p: await loadPerson(api, name)}), [api, name]));
  const p = poll.data?.p;
  const back = <Button label="All players" variant="secondary" size="sm" href={toHash('players')} />;
  if (poll.data && !p) {
    return (
      <EmptyState
        title={`There is no player "${name}"`}
        description="No account has that login and no character that name. The list finds a player by a part of either."
        headingLevel={1}
        actions={back}
      />
    );
  }
  const now = Date.now() / 1000;
  const a = p?.a ?? null;
  return (
    <VStack gap={5}>
      <HStack gap={2}>{back}</HStack>
      <ScreenHead title={p ? p.t.nick || p.t.login : name} poll={poll} />
      {p && (
        <>
          <VStack gap={2}>
            <HStack gap={2} align="center" wrap="wrap">
              {a && <StatusDot variant={a.online ? 'success' : 'neutral'} label={a.online ? 'Online' : 'Offline'} />}
              <Text type="large">
                {a
                  ? a.online
                    ? `Online, ${a.where}`
                    : a.last_login
                      ? `Offline, last seen ${ago(a.last_login, now)}`
                      : 'Never signed in'
                  : `Account ${p.t.account}`}
              </Text>
            </HStack>
            <Text type="body" color="secondary" as="p">
              Login {p.t.login}, account {p.t.account}.
            </Text>
            {a && marks({banned: false, disabled: false, muted: !!a.mute, bot: a.bot, gm: a.gm}).length > 0 && (
              <HStack gap={1} wrap="wrap">
                {marks({banned: false, disabled: false, muted: !!a.mute, bot: a.bot, gm: a.gm}).map(f => (
                  <Badge
                    key={f.label}
                    variant={f.variant}
                    label={f.label === 'Muted' && a.mute ? `Muted, ${minutesLeft(a.mute.minutes_left)}` : f.label}
                  />
                ))}
              </HStack>
            )}
            {p.t.other_matches.length > 0 && (
              <HStack gap={2} align="center" wrap="wrap">
                <Text type="supporting">The name also fits:</Text>
                {p.t.other_matches.map(o => (
                  <Button
                    key={o.account}
                    label={o.nick || o.login}
                    variant="secondary"
                    size="sm"
                    href={toHash('players', o.nick || o.login)}
                  />
                ))}
              </HStack>
            )}
          </VStack>
          {a?.ban && (
            <Banner
              status="error"
              title={a.ban.permanent ? 'Banned for good.' : `Banned until ${when(a.ban.until)}.`}
              description={`By ${a.ban.by} on ${when(a.ban.since).slice(0, 10)}${a.ban.reason ? `: ${a.ban.reason}` : ''}`}
            />
          )}
          {a?.disabled && (
            <Banner
              status="warning"
              title="The login is disabled."
              description="They cannot log in until it is enabled. A session that was open stays open."
            />
          )}
          {!a && (
            <Banner status="error" title="The account's facts did not load." description={p.why} />
          )}
          <PersonTiles a={a} row={p.row} stays={p.t.matches} />
          <Grid columns={{minWidth: 320, max: 2}} gap={4} align="start">
            <Part title="Round trip per match" note="The mean of each match, the newest on the right.">
              <Card padding={3}>
                <RoundTripPerMatch stays={p.t.matches} />
              </Card>
            </Part>
            <Part title="Matches per day" note="The last 14 days, by how they left.">
              <Card padding={3}>
                {p.t.matches.length === 0 ? (
                  <Quiet>No match in the last 14 days.</Quiet>
                ) : (
                  <MatchesPerDay
                    rows={p.t.matches.map(s => ({started: s.joined, outcome: leftWell(s.how) ? 'result' : s.how}))}
                    now={now}
                    height={140}
                    limit={STAY_LIMIT}
                    words={['left the usual way', 'left another way']}
                  />
                )}
              </Card>
            </Part>
          </Grid>
          {a && (
            <Part title="Account">
              <AccountFacts a={a} />
            </Part>
          )}
          {a && (
            <Part title="Changes" note="Each one asks before it is made and says what will happen.">
              <PlayerActions
                api={api}
                t={a}
                name={name}
                changed={poll.refresh}
                deleted={() => {
                  location.hash = toHash('players');
                }}
              />
            </Part>
          )}
          <Part
            title="Matches"
            note={
              p.t.matches.length >= STAY_LIMIT
                ? `The newest ${STAY_LIMIT}. A match opens with everybody's connection.`
                : "The last 14 days. A match opens with everybody's connection."
            }>
            <Stays stays={p.t.matches} />
          </Part>
          <Part title="Events">
            <EventLog events={p.t.events} now={now} empty="No events." ofOne />
          </Part>
        </>
      )}
    </VStack>
  );
}

export function Players({api, name}: {api: Api; name: string}) {
  return name ? <PersonPage api={api} name={name} /> : <PlayerList api={api} />;
}
