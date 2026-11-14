// Matches: the recent matches the lobby recorded, and one match's page with every person's connection over the whole
// match, its events and the reports the game server sent every 10 s (the classic tabs Matches and "match <id>").
// It only reads (DC_DEFAULTS.md LG-447); the charts are those of LG-449.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {useCallback, useState} from 'react';
import {type Api, ApiError} from '../api.ts';
import {type Health, duration, healthOf, plural} from '../overview.ts';
import {usePoll} from '../poll.ts';
import {toHash} from '../routes.ts';
import {when} from '../time.ts';
import {
  Badge,
  Button,
  Card,
  EmptyState,
  Grid,
  HStack,
  Heading,
  List,
  ListItem,
  StatusDot,
  Table,
  Text,
  VStack,
  pixel,
} from '../ui/index.ts';
import {MATCH_LIMIT, MatchesPerDay} from '../viz/activity.tsx';
import {Legend, Lines, Meter, type Paint, type Series, Split} from '../viz/charts.tsx';
import {type EventRow, EventLog} from './EventLog.tsx';
import {RTT_SCALE} from './Overview.tsx';
import {Part, Quiet, ScreenHead} from './parts.tsx';

type Num = number | null | undefined;

/** One row of GET /ops/matches, and `match` of GET /ops/match. */
interface MatchRow {
  id: number;
  started: number;
  ended?: number | null;
  room: string | number;
  mode_key: string;
  map_name: string;
  host: string;
  host_version?: string | null;
  port?: number;
  players: number;
  bots: number;
  outcome: string;
  winner?: string | number | null;
  alpha?: Num;
  bravo?: Num;
  silent_drops: number;
}

interface Person {
  account: number;
  nick: string;
  bot?: boolean;
  health?: string | null;
  joined: number;
  secs: number;
  how: string;
  code?: number | string | null;
  rtt_mean: Num;
  rtt_p95: Num;
  rtt_max: Num;
  jitter: Num;
  input_rate: Num;
  gap_max: Num;
  resends: Num;
  reliable: Num;
  flow_drops: Num;
  dg_in: Num;
  dg_out: Num;
  max_datagram: Num;
}

/** One 10 s report of one person's link. */
interface Report extends Record<string, unknown> {
  at: number;
  client?: string;
  account: number;
  health?: string | null;
  rtt: Num;
  rtt_min: Num;
  rtt_max: Num;
  rtt_n: Num;
  jitter: Num;
  input: Num;
  gap: Num;
  resends: Num;
  drops: Num;
  dg_in: Num;
  dg_out: Num;
}

interface Detail {
  match: MatchRow;
  players: Person[];
  net: Report[];
  events: EventRow[];
}

const HEALTH_WORD: Record<Health, string> = {good: 'Good', warn: 'Shaky', bad: 'Bad', none: 'No data'};
const HEALTH_BADGE: Record<Health, 'success' | 'warning' | 'error' | 'neutral'> = {
  good: 'success',
  warn: 'warning',
  bad: 'error',
  none: 'neutral',
};

const n = (v: Num, unit = '') => (v == null ? '-' : `${v}${unit}`);

function HealthBadge({health}: {health: string | null | undefined}) {
  const h = healthOf(health);
  return <Badge variant={HEALTH_BADGE[h]} label={HEALTH_WORD[h]} />;
}

/** How a match ended: the usual words are plain, anything else is marked. */
function OutcomeBadge({outcome}: {outcome: string}) {
  if (outcome === 'running') return <Badge variant="info" label="Running" />;
  if (outcome === 'result') return <Badge variant="neutral" label="Finished" />;
  return <Badge variant="error" label={outcome} />;
}

function length(m: MatchRow): string {
  return m.ended ? duration(m.ended - m.started) : 'running';
}

// ---- the list

function MatchList({api}: {api: Api}) {
  const poll = usePoll(useCallback(() => api.get<MatchRow[]>(`/ops/matches?limit=${MATCH_LIMIT}`), [api]));
  const rows = poll.data;
  return (
    <VStack gap={5}>
      <ScreenHead title="Matches" poll={poll} />
      {rows && rows.length > 0 && (
        <Part title="Matches per day" note="The last 14 days, counted from the list below.">
          <Card padding={3}>
            <MatchesPerDay rows={rows} now={Date.now() / 1000} height={140} />
          </Card>
        </Part>
      )}
      {rows && (
        <Part title="Recent matches" note="The newest 100 of the last 14 days. A match opens with every person's connection.">
          {rows.length === 0 ? (
            <Quiet>No match recorded yet.</Quiet>
          ) : (
            <Card padding={0}>
              <List hasDividers>
                {rows.map(m => (
                  <ListItem
                    key={m.id}
                    label={
                      <Text type="body" weight="medium">
                        Match {m.id}
                        <Text type="body" color="secondary">
                          {' '}
                          · {m.mode_key} on {m.map_name}
                        </Text>
                      </Text>
                    }
                    description={
                      <Text type="supporting" as="div">
                        {when(m.started)} · {length(m)} · room {m.room} · {m.host}
                        {m.host_version ? ` (build ${m.host_version})` : ''} · {plural(m.players, 'person', 'people')}
                        {m.bots ? `, ${plural(m.bots, 'bot')}` : ''}
                      </Text>
                    }
                    href={toHash('matches', String(m.id))}
                    align="first-line"
                    startContent={
                      <StatusDot
                        variant={m.outcome === 'running' ? 'accent' : m.outcome === 'result' ? 'success' : 'error'}
                        label={m.outcome === 'running' ? 'Running' : m.outcome === 'result' ? 'Finished' : m.outcome}
                        isPulsing={m.outcome === 'running'}
                      />
                    }
                    endContent={
                      // one under the other: side by side they take the text's room on a phone
                      <VStack gap={1} align="end">
                        <OutcomeBadge outcome={m.outcome} />
                        {m.silent_drops > 0 && (
                          <Badge variant="warning" label={plural(m.silent_drops, 'silent drop')} />
                        )}
                      </VStack>
                    }
                  />
                ))}
              </List>
            </Card>
          )}
        </Part>
      )}
    </VStack>
  );
}

// ---- one match

/** One person's round trip per 10 s report as a small line chart; the dashed line is 100 ms. */
function Spark({points}: {points: Report[]}) {
  if (points.length === 0) return <Text type="supporting">no 10 s reports</Text>;
  const W = 240;
  const H = 44;
  const P = 4;
  const top = Math.max(100, ...points.map(p => Number(p.rtt_max ?? p.rtt ?? 0)));
  const x = (i: number) => P + (points.length < 2 ? 0 : (i * (W - 2 * P)) / (points.length - 1));
  const y = (v: number) => H - P - (v / top) * (H - 2 * P);
  const path = points.map((p, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)} ${y(Number(p.rtt ?? 0)).toFixed(1)}`).join(' ');
  return (
    <svg
      width="100%"
      height={H}
      viewBox={`0 0 ${W} ${H}`}
      preserveAspectRatio="none"
      role="img"
      aria-label={`Round trip per 10 s, ${points.length} reports, highest ${Math.round(top)} ms`}>
      <line x1={P} x2={W - P} y1={y(100)} y2={y(100)} stroke="currentColor" strokeOpacity={0.25} strokeDasharray="3 3" />
      <path
        d={`${path} L${x(points.length - 1).toFixed(1)} ${H - P} L${x(0).toFixed(1)} ${H - P} Z`}
        fill="currentColor"
        fillOpacity={0.08}
      />
      <path d={path} fill="none" stroke="currentColor" strokeWidth={1.5} vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function Fact({label, value}: {label: string; value: string}) {
  return (
    <VStack gap={0}>
      <Text type="supporting">{label}</Text>
      <Text type="body" hasTabularNumbers>
        {value}
      </Text>
    </VStack>
  );
}

function PersonCard({p, reports, paint}: {p: Person; reports: Report[]; paint?: Paint}) {
  const left = p.how === 'bye' || p.how === 'end';
  return (
    <Card padding={3}>
      <VStack gap={2}>
        <HStack gap={2} align="center" wrap="wrap">
          {/* the colour of this person's line in the chart above */}
          {paint && <span className={`viz viz-key viz-${paint}`} aria-hidden="true" />}
          {p.bot ? (
            <Heading level={3}>{p.nick} (bot)</Heading>
          ) : (
            <Button label={p.nick} variant="secondary" size="sm" href={toHash('players', p.nick)} />
          )}
          <HealthBadge health={p.health} />
          {!left && <Badge variant="error" label={`left by ${p.how}${p.code ? ` (${p.code})` : ''}`} />}
        </HStack>
        <Text type="supporting">
          Joined {when(p.joined).slice(11)}, stayed {duration(p.secs)}
          {left ? `, left by ${p.how}` : ''}.
        </Text>
        {!p.bot && p.rtt_mean != null && (
          <Meter
            value={p.rtt_mean}
            max={RTT_SCALE}
            paint={healthOf(p.health)}
            tick={p.rtt_max}
            label={`Round trip ${p.rtt_mean} ms, worst ${n(p.rtt_max)} ms, on a scale of 0 to ${RTT_SCALE} ms`}
          />
        )}
        <Grid columns={3} gap={2}>
          <Fact label="Round trip" value={n(p.rtt_mean, ' ms')} />
          <Fact label="p95" value={n(p.rtt_p95, ' ms')} />
          <Fact label="Worst" value={n(p.rtt_max, ' ms')} />
          <Fact label="Jitter" value={n(p.jitter, ' ms')} />
          <Fact label="Input" value={n(p.input_rate, '/s')} />
          <Fact label="Longest gap" value={n(p.gap_max, ' ms')} />
          <Fact label="Resends" value={p.resends == null ? '-' : `${p.resends} of ${n(p.reliable)}`} />
          <Fact label="Drops" value={n(p.flow_drops)} />
          <Fact label="Largest" value={n(p.max_datagram, ' B')} />
          <Fact label="Packets in" value={n(p.dg_in)} />
          <Fact label="Packets out" value={n(p.dg_out)} />
        </Grid>
        <VStack gap={0.5}>
          <Text type="supporting">Round trip per 10 s</Text>
          <Spark points={reports} />
        </VStack>
      </VStack>
    </Card>
  );
}

function Reports({net, nick}: {net: Report[]; nick: Map<number, string>}) {
  const [open, setOpen] = useState(false);
  if (net.length === 0) return <Quiet>The game server sent no report (an older build, or a match of a few seconds).</Quiet>;
  if (!open) {
    return (
      <HStack gap={2}>
        <Button label={`Show the ${plural(net.length, 'report')}`} variant="secondary" size="sm" onClick={() => setOpen(true)} />
      </HStack>
    );
  }
  const num = (key: keyof Report, header: string, unit = '') => ({
    key: String(key),
    header,
    width: pixel(96),
    align: 'end' as const,
    renderCell: (w: Report) => n(w[key] as Num, unit),
  });
  return (
    // the one place a table stays a table on a phone: it scrolls sideways inside its card
    <Card padding={0}>
      <div style={{overflowX: 'auto'}}>
        <div style={{minWidth: 1560}}>
          <Table<Report>
            data={net}
            density="compact"
            hasHover
            columns={[
              {key: 'at', header: 'Time', width: pixel(90), renderCell: w => when(w.at).slice(11)},
              {key: 'account', header: 'Player', width: pixel(200), renderCell: w => nick.get(w.account) ?? String(w.account)},
              {key: 'health', header: 'Connection', width: pixel(110), renderCell: w => <HealthBadge health={w.health} />},
              num('rtt', 'Round trip', ' ms'),
              num('rtt_min', 'Best'),
              num('rtt_max', 'Worst'),
              num('rtt_n', 'Samples'),
              num('jitter', 'Jitter'),
              num('input', 'Input/s'),
              num('gap', 'Longest gap'),
              num('resends', 'Resends'),
              num('drops', 'Drops'),
              num('dg_in', 'In'),
              num('dg_out', 'Out'),
              {key: 'client', header: 'Client', width: pixel(90), renderCell: w => w.client ?? '-'},
            ]}
          />
        </div>
      </div>
    </Card>
  );
}

/** How many people have a line of their own colour in the chart of the whole match; the others are grey. */
const NAMED = 4;
const SLOTS: Paint[] = ['s1', 's2', 's3', 's4'];

/**
 * Whose line is coloured: the people (not bots) with the highest mean round trip, at most four, each with the
 * colour of their place in the lobby's order, so that a refresh does not repaint them.
 */
function paints(people: Person[], reports: Map<number, Report[]>): Map<number, Paint> {
  const drawn = people.filter(p => !p.bot && (reports.get(p.account)?.length ?? 0) > 0);
  const named = new Set(
    [...drawn]
      .sort((a, b) => Number(b.rtt_mean ?? 0) - Number(a.rtt_mean ?? 0))
      .slice(0, NAMED)
      .map(p => p.account),
  );
  const out = new Map<number, Paint>();
  let slot = 0;
  for (const p of drawn) {
    if (out.has(p.account)) continue;
    out.set(p.account, named.has(p.account) ? (SLOTS[slot++] ?? 'rest') : 'rest');
  }
  return out;
}

function RoundTrips({m, people, reports, paint}: {
  m: MatchRow;
  people: Person[];
  reports: Map<number, Report[]>;
  paint: Map<number, Paint>;
}) {
  const series: Series[] = [];
  for (const [account, of] of paint) {
    const nick = people.find(p => p.account === account)?.nick ?? String(account);
    const points = (reports.get(account) ?? [])
      .filter(w => w.rtt != null)
      .map(w => ({t: w.at, v: Number(w.rtt)}))
      .sort((a, b) => a.t - b.t);
    series.push({name: nick, paint: of, points});
  }
  if (series.length === 0) return <Quiet>The game server sent no report of a person's connection.</Quiet>;
  const last = Math.max(m.started + 1, ...series.flatMap(s => s.points.map(p => p.t)));
  const rest = series.filter(s => s.paint === 'rest').length;
  return (
    <Card padding={3}>
      <Lines
        series={series}
        from={m.started}
        to={m.ended || last}
        unit=" ms"
        floor={100}
        ends={[when(m.started).slice(11), m.ended ? when(m.ended).slice(11) : 'now']}
        time={t => when(t).slice(11)}
        label={`Round trip of ${plural(series.length, 'person', 'people')} per 10 s over the match, in ms.`}
        legend={
          <Legend
            items={[
              ...series.filter(s => s.paint !== 'rest').map(s => ({paint: s.paint, name: s.name, line: true})),
              ...(rest ? [{paint: 'rest' as Paint, name: plural(rest, 'other'), line: true}] : []),
            ]}
          />
        }
      />
    </Card>
  );
}

/** The two sides' scores as one bar. */
function Score({m}: {m: MatchRow}) {
  if (m.alpha == null || m.bravo == null || m.alpha + m.bravo <= 0) return null;
  return (
    <VStack gap={1} maxWidth={520}>
      <HStack gap={2} justify="between">
        <Text type="body" weight={m.alpha >= m.bravo ? 'semibold' : 'normal'}>
          Alpha {m.alpha}
        </Text>
        <Text type="body" weight={m.bravo >= m.alpha ? 'semibold' : 'normal'}>
          {m.bravo} Bravo
        </Text>
      </HStack>
      <Split
        thick
        label={`Alpha ${m.alpha}, Bravo ${m.bravo}`}
        parts={[
          {paint: 's1', value: m.alpha, name: 'Alpha'},
          {paint: 's2', value: m.bravo, name: 'Bravo'},
        ]}
      />
    </VStack>
  );
}

/** null: the lobby has no such match (an id of more than 14 days ago, or a mistyped one). */
async function loadMatch(api: Api, id: string): Promise<Detail | null> {
  try {
    return await api.get<Detail>(`/ops/match?id=${encodeURIComponent(id)}`);
  } catch (e) {
    if (e instanceof ApiError && (e.status === 404 || e.status === 400)) return null;
    throw e;
  }
}

function MatchPage({api, id}: {api: Api; id: string}) {
  const poll = usePoll(useCallback(async () => ({d: await loadMatch(api, id)}), [api, id]));
  const d = poll.data?.d;
  const back = <Button label="All matches" variant="secondary" size="sm" href={toHash('matches')} />;
  if (poll.data && !d) {
    return (
      <EmptyState
        title={`There is no match ${id}`}
        description="The lobby keeps matches for 14 days."
        headingLevel={1}
        actions={back}
      />
    );
  }
  const m = d?.match;
  const byAccount = new Map<number, Report[]>();
  const nick = new Map<number, string>();
  for (const w of d?.net ?? []) byAccount.set(w.account, [...(byAccount.get(w.account) ?? []), w]);
  for (const p of d?.players ?? []) nick.set(p.account, p.nick);
  const paint = paints(d?.players ?? [], byAccount);
  return (
    <VStack gap={5}>
      <HStack gap={2}>{back}</HStack>
      <ScreenHead title={`Match ${id}`} poll={poll} />
      {d && m && (
        <>
          <VStack gap={2}>
            <Text type="large">
              Room {m.room}, {m.mode_key} on {m.map_name}
            </Text>
            <Text type="body" color="secondary" as="p">
              Played on {m.host} (build {m.host_version || 'unknown'}) port {n(m.port)}, from {when(m.started)} to{' '}
              {m.ended ? when(m.ended) : 'now'} ({length(m)}). {plural(m.players, 'person', 'people')},{' '}
              {plural(m.bots, 'bot')}.
            </Text>
            <HStack gap={1} align="center" wrap="wrap">
              <OutcomeBadge outcome={m.outcome} />
              {m.outcome !== 'running' && (
                <Badge variant="neutral" label={`Winner ${n(m.winner as Num)} (${n(m.alpha)} to ${n(m.bravo)})`} />
              )}
              <Badge
                variant={m.silent_drops > 0 ? 'warning' : 'neutral'}
                label={plural(m.silent_drops, 'silent drop')}
              />
            </HStack>
            {m.outcome !== 'running' && <Score m={m} />}
          </VStack>
          <Part
            title="Round trip over the match"
            note={
              paint.size > NAMED
                ? `Every person's round trip per 10 s. The ${NAMED} with the highest are coloured.`
                : "Every person's round trip per 10 s."
            }>
            <RoundTrips m={m} people={d.players} reports={byAccount} paint={paint} />
          </Part>
          <Part title="People over the whole match">
            {d.players.length === 0 ? (
              <Quiet>Nobody was recorded in this match.</Quiet>
            ) : (
              <Grid columns={{minWidth: 300, max: 3}} gap={3} align="start">
                {d.players.map(p => (
                  <PersonCard
                    key={`${p.account}:${p.joined}`}
                    p={p}
                    reports={byAccount.get(p.account) ?? []}
                    paint={paint.get(p.account) === 'rest' ? undefined : paint.get(p.account)}
                  />
                ))}
              </Grid>
            )}
          </Part>
          <Part title="Events">
            <EventLog events={d.events} now={Date.now() / 1000} empty="No events." />
          </Part>
          <Part title="Connection reports, every 10 s">
            <Reports net={d.net} nick={nick} />
          </Part>
        </>
      )}
    </VStack>
  );
}

export function Matches({api, id}: {api: Api; id: string}) {
  return id ? <MatchPage api={api} id={id} /> : <MatchList api={api} />;
}
