// The start page: what is wrong or waiting right now, the lobby's numbers, the running matches with every person's
// connection, the game servers and the open rooms (DC_DEFAULTS.md LG-445), drawn with the charts of LG-449. It only
// reads; the rules of the headline and the list are in ../overview.ts.
// Provenance: ORIGINAL (DeCombat operator tooling)

import {useCallback} from 'react';
import type {Api} from '../api.ts';
import {
  type AlertCheck,
  type BotsState,
  type Finding,
  type Health,
  type Level,
  type Live,
  type LiveHost,
  type LiveMatch,
  type LiveRoom,
  type OpsEvent,
  type Release,
  type Snapshot,
  type Triage,
  type Updates,
  duration,
  findings,
  headline,
  healthOf,
  plural,
  worstHealth,
} from '../overview.ts';
import {usePoll} from '../poll.ts';
import {CLASSIC, NAV, toHash} from '../routes.ts';
import {
  Badge,
  Banner,
  Button,
  Card,
  Grid,
  HStack,
  Heading,
  Icon,
  List,
  ListItem,
  StatusDot,
  Text,
  VStack,
} from '../ui/index.ts';
import {MATCH_LIMIT, MatchesPerDay, PROBLEM_LIMIT, ProblemsPerHour} from '../viz/activity.tsx';
import {Dots, Hero, Legend, Meter, type Paint, Split} from '../viz/charts.tsx';
import {Part, Quiet, ScreenHead, Tile} from './parts.tsx';

const HEALTH_WORD: Record<Health, string> = {good: 'Good', warn: 'Shaky', bad: 'Bad', none: 'No data'};
const HEALTH_BADGE: Record<Health, 'success' | 'warning' | 'error' | 'neutral'> = {
  good: 'success',
  warn: 'warning',
  bad: 'error',
  none: 'neutral',
};
const LEVEL_DOT: Record<Level, 'error' | 'warning' | 'accent'> = {bad: 'error', warn: 'warning', info: 'accent'};
const LEVEL_WORD: Record<Level, string> = {bad: 'Broken', warn: 'Needs a look', info: 'Waiting'};
const SCREEN_LABEL = new Map(NAV.flatMap(g => g.items).map(i => [i.id, i.label]));

/** A round trip is drawn on this fixed scale (ms), so two people compare by eye. */
export const RTT_SCALE = 300;

interface Reading {
  snap: Snapshot;
  /** null: the list of matches did not answer (its chart is left out). */
  matches: {started: number; outcome: string}[] | null;
}

/** One refresh: the lobby's live state, and beside it whatever else answers (each of those may be missing). */
async function load(api: Api): Promise<Reading> {
  const maybe = <T,>(path: string) => api.get<T>(path).catch(() => null);
  // deploys, the release and the bug reports are for the whole box: a further instance refuses them
  const box = api.boot.current === null;
  const [live, alerts, problems, bots, updates, release, triage, matches] = await Promise.all([
    api.get<Live>('/ops/live'),
    maybe<AlertCheck[]>('/ops/alerts'),
    maybe<OpsEvent[]>(`/ops/events?problems=1&limit=${PROBLEM_LIMIT}`),
    maybe<BotsState>('/ops/bots'),
    box ? maybe<Updates>('/ops/updates') : null,
    box ? maybe<Release>('/ops/updates/release') : null,
    box ? maybe<Triage>('/ops/triage') : null,
    maybe<Reading['matches']>(`/ops/matches?limit=${MATCH_LIMIT}`),
  ]);
  return {snap: {live, alerts, problems, bots, updates, release, triage}, matches};
}

function Attention({list}: {list: Finding[]}) {
  if (list.length === 0) return <Quiet>Nothing needs you right now.</Quiet>;
  return (
    <Card padding={0}>
      <List hasDividers>
        {list.map((f, i) => (
          <ListItem
            key={i}
            label={<Text type="body">{f.text}</Text>}
            description={`${LEVEL_WORD[f.level]} · ${SCREEN_LABEL.get(f.screen) ?? f.screen}`}
            href={toHash(f.screen, f.arg)}
            align="first-line"
            startContent={<StatusDot variant={LEVEL_DOT[f.level]} label={LEVEL_WORD[f.level]} />}
          />
        ))}
      </List>
    </Card>
  );
}

function Tiles({live, bots}: {live: Live; bots: BotsState | null}) {
  const people = live.matches.flatMap(m => m.players.filter(p => !p.bot));
  const by = (h: Health) => people.filter(p => healthOf(p.health) === h).length;
  const inRooms = live.rooms.reduce((n, r) => n + r.players, 0);
  const botsInRooms = live.rooms.reduce((n, r) => n + r.bots, 0);
  const elsewhere = Math.max(0, live.lobby.online - inRooms - botsInRooms);
  const seats = live.rooms.reduce((n, r) => n + r.max_players, 0);
  const full = live.rooms.filter(r => r.players + r.bots >= r.max_players).length;
  const up = live.hosts.filter(h => h.online).length;
  const down = live.hosts.length - up;
  const mix = [
    {paint: 'good' as Paint, value: by('good'), name: 'good'},
    {paint: 'warn' as Paint, value: by('warn'), name: 'shaky'},
    {paint: 'bad' as Paint, value: by('bad'), name: 'bad'},
    {paint: 'none' as Paint, value: by('none'), name: 'no data'},
  ];
  const where = [
    {paint: 's1' as Paint, value: inRooms, name: 'people in rooms'},
    {paint: 's2' as Paint, value: botsInRooms, name: 'bots in rooms'},
    {paint: 'rest' as Paint, value: elsewhere, name: 'not in a room'},
  ];
  const keys = (parts: typeof mix) => (
    <div className="viz-note">
      <Legend items={parts.filter(p => p.value > 0).map(p => ({paint: p.paint, name: `${p.value} ${p.name}`}))} />
    </div>
  );
  return (
    <Grid columns={{minWidth: 170, max: 5}} gap={3}>
      <Tile
        label="Players online"
        value={String(live.lobby.online)}
        visual={<Split parts={where} label="Where the players online are" />}
        note={live.lobby.online ? keys(where) : undefined}
      />
      <Tile
        label="Matches running"
        value={String(live.matches.length)}
        visual={<Split parts={mix} label="The connections of the people playing" />}
        note={people.length ? keys(mix) : live.matches.length ? 'Only bots are playing.' : undefined}
      />
      <Tile
        label="Rooms open"
        value={String(live.rooms.length)}
        visual={
          <Meter
            value={inRooms + botsInRooms}
            max={seats || 1}
            paint="s1"
            label={`${inRooms + botsInRooms} of ${seats} seats taken`}
          />
        }
        note={
          live.rooms.length
            ? `${inRooms + botsInRooms} of ${plural(seats, 'seat')} taken${full ? `, ${full} full` : ''}`
            : undefined
        }
      />
      <Tile
        label="Game servers online"
        value={`${up} of ${live.hosts.length}`}
        visual={
          <Dots
            label={`${up} of ${live.hosts.length} game servers online`}
            items={live.hosts.map(h => ({
              paint: !h.online ? 'bad' : h.version_matches === false ? 'warn' : 'good',
              name: `${h.local ? 'This server (lobby)' : h.name}: ${!h.online ? 'offline' : h.version_matches === false ? 'other build' : 'online'}`,
            }))}
          />
        }
        note={down ? `${down} offline` : 'All online'}
      />
      {bots?.managed && (
        <Tile
          label="Bots running"
          value={`${bots.running ?? 0} of ${bots.wanted ?? 0}`}
          visual={
            <Meter
              value={bots.running ?? 0}
              max={bots.wanted || 1}
              paint={(bots.running ?? 0) < (bots.wanted ?? 0) ? 'warn' : 'good'}
              label={`${bots.running ?? 0} of ${bots.wanted ?? 0} bots running`}
            />
          }
          note={
            bots.exe_missing
              ? 'dc-botclient is missing'
              : (bots.running ?? 0) < (bots.wanted ?? 0)
                ? `${(bots.wanted ?? 0) - (bots.running ?? 0)} not running`
                : 'All running'
          }
        />
      )}
    </Grid>
  );
}

function MatchCard({m}: {m: LiveMatch}) {
  const people = m.players.filter(p => !p.bot);
  const bots = m.players.length - people.length;
  const worst = worstHealth(m.players);
  const id = String(m.match ?? '');
  return (
    <Card padding={3}>
      <VStack gap={2}>
        <VStack gap={1}>
          <Heading level={3}>
            Room {m.room}: {m.mode} on {m.map}
          </Heading>
          <Text type="supporting">
            {m.host} port {m.port}
            {m.ready === false ? ', not ready yet' : ''} · {plural(people.length, 'person', 'people')}
            {bots ? `, ${plural(bots, 'bot')}` : ''}
          </Text>
          {/* a row of its own: beside the heading a badge is squeezed to nothing on a phone */}
          <HStack gap={2}>
            <Badge variant={HEALTH_BADGE[worst]} label={`Connections: ${HEALTH_WORD[worst]}`} />
          </HStack>
        </VStack>
        {people.length > 0 && (
          <List density="compact" hasDividers edge="flush">
            {people.map(p => {
              const h = healthOf(p.health);
              const worstMs = p.rtt_max != null && p.rtt != null && p.rtt_max > p.rtt ? Math.round(p.rtt_max) : null;
              return (
                <ListItem
                  key={p.nick}
                  label={p.nick}
                  href={toHash('players', p.nick)}
                  description={
                    p.rtt == null ? undefined : (
                      // the classic page's bar: the round trip on a fixed scale, the tick is the window's worst
                      <div className="viz-under">
                        <Meter
                          value={p.rtt}
                          max={RTT_SCALE}
                          paint={h}
                          tick={worstMs}
                          label={`Round trip ${Math.round(p.rtt)} ms${worstMs ? `, worst ${worstMs} ms` : ''}, on a scale of 0 to ${RTT_SCALE} ms`}
                        />
                      </div>
                    )
                  }
                  endContent={
                    <HStack gap={2} align="center">
                      <Text type="supporting" hasTabularNumbers>
                        {p.rtt == null ? 'no report yet' : `${Math.round(p.rtt)} ms${worstMs ? `, worst ${worstMs}` : ''}`}
                      </Text>
                      <Badge variant={HEALTH_BADGE[h]} label={HEALTH_WORD[h]} />
                    </HStack>
                  }
                />
              );
            })}
          </List>
        )}
        {id && <Button label="Open the match" variant="secondary" size="sm" href={toHash('matches', id)} />}
      </VStack>
    </Card>
  );
}

function Servers({hosts}: {hosts: LiveHost[]}) {
  return (
    <Card padding={0}>
      <List hasDividers>
        {hosts.map(h => {
          const facts = [
            h.version ? `build ${h.version}` : null,
            plural(h.games, 'match', 'matches'),
            h.online && h.online_secs != null ? `up ${duration(h.online_secs)}` : null,
            h.address || null,
          ].filter(Boolean);
          const state = !h.online ? 'Offline' : h.version_matches === false ? 'Other build' : 'Online';
          const variant = !h.online ? 'error' : h.version_matches === false ? 'warning' : 'success';
          return (
            <ListItem
              key={h.name}
              label={h.local ? 'This server (lobby)' : h.name}
              description={
                <Text type="supporting" as="div">
                  {facts.join(' · ')}
                </Text>
              }
              align="first-line"
              startContent={<StatusDot variant={variant} label={state} />}
              endContent={<Badge variant={variant} label={state} />}
            />
          );
        })}
      </List>
    </Card>
  );
}

function Rooms({rooms}: {rooms: LiveRoom[]}) {
  if (rooms.length === 0) return <Quiet>No room is open.</Quiet>;
  return (
    <Card padding={0}>
      <List hasDividers>
        {rooms.map(r => (
          <ListItem
            key={r.id}
            label={r.title || `Room ${r.id}`}
            description={
              <VStack gap={1}>
                <Text type="supporting" as="div">
                  {r.id} · {r.mode} · {r.players} of {r.max_players} people
                  {r.bots ? `, ${plural(r.bots, 'bot')}` : ''}
                  {r.members.length ? ` · ${r.members.join(', ')}` : ''}
                </Text>
                <Split
                  label={`${r.players} people and ${r.bots} bots on ${r.max_players} seats`}
                  parts={[
                    {paint: 's1', value: r.players, name: 'people'},
                    {paint: 's2', value: r.bots, name: 'bots'},
                    {paint: 'empty', value: Math.max(0, r.max_players - r.players - r.bots), name: 'free seats'},
                  ]}
                />
              </VStack>
            }
            align="first-line"
            endContent={<Badge variant={r.state === 'playing' ? 'info' : 'neutral'} label={r.state} />}
          />
        ))}
      </List>
    </Card>
  );
}

export function Overview({api}: {api: Api}) {
  const poll = usePoll(useCallback(() => load(api), [api]));
  const snap = poll.data?.snap;
  const now = Date.now() / 1000;
  const list = snap ? findings(snap, now) : [];
  const head = headline(list);
  const live = snap?.live;
  const more = list.filter(f => f.level !== 'info').length - 1;

  return (
    <VStack gap={5}>
      <ScreenHead title="Overview" poll={poll} />
      {live && (
        <>
          <Card padding={4} variant={head.level === 'good' ? 'green' : head.level === 'bad' ? 'red' : 'yellow'}>
            <Hero
              paint={head.level}
              sign={<Icon icon={head.level === 'good' ? 'check' : head.level === 'bad' ? 'close' : 'warning'} size="lg" />}>
              <VStack gap={1}>
                <p className="say">{head.text}</p>
                <Text type="body" color="secondary" as="p">
                  {more > 0 ? `And ${plural(more, 'more thing')} below. ` : ''}
                  {plural(live.lobby.online, 'player')} online, {plural(live.matches.length, 'match', 'matches')}{' '}
                  running, {plural(live.rooms.length, 'room')} open. Lobby build {live.lobby.build}, up{' '}
                  {duration(live.lobby.uptime_secs)}.
                </Text>
              </VStack>
            </Hero>
          </Card>
          <Tiles live={live} bots={snap.bots} />
          <Part title="Needs attention">
            <Attention list={list} />
          </Part>
          <Part title="Running matches">
            {live.matches.length === 0 ? (
              <Quiet>
                No match is running. A room that starts a match shows up here with every person's connection.
              </Quiet>
            ) : (
              <Grid columns={{minWidth: 300, max: 3}} gap={3} align="start">
                {live.matches.map(m => (
                  <MatchCard key={`${m.host}:${m.port}:${m.match}`} m={m} />
                ))}
              </Grid>
            )}
          </Part>
          <Grid columns={{minWidth: 300, max: 2}} gap={5} align="start">
            <Part title="Game servers">
              <Servers hosts={live.hosts} />
            </Part>
            <Part title="Rooms">
              <Rooms rooms={live.rooms} />
            </Part>
          </Grid>
          <Grid columns={{minWidth: 300, max: 2}} gap={5} align="start">
            {poll.data?.matches && (
              <Part title="Matches per day" note="The last 14 days.">
                {poll.data.matches.length === 0 ? (
                  <Quiet>No match recorded yet.</Quiet>
                ) : (
                  <Card padding={3}>
                    <MatchesPerDay rows={poll.data.matches} now={now} />
                  </Card>
                )}
              </Part>
            )}
            {snap.problems && (
              <Part title="Problems per hour" note="The last 24 hours.">
                {snap.problems.length === 0 ? (
                  <Quiet>No problems logged.</Quiet>
                ) : (
                  <Card padding={3}>
                    <ProblemsPerHour events={snap.problems} now={now} />
                  </Card>
                )}
              </Part>
            )}
          </Grid>
        </>
      )}

      <Banner
        status="info"
        title="The new dashboard is arriving screen by screen."
        description="A screen that has not moved yet opens on the classic dashboard, where everything works as before."
        endContent={<Button label="Open the classic dashboard" href={api.page(CLASSIC) + '#live'} />}
      />
    </VStack>
  );
}
