// Provenance: ORIGINAL (DeCombat operator tooling)
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {type Live, type Snapshot, duration, findings, headline, worstHealth} from './overview.ts';

const NOW = 1_800_000_000;

function live(over: Partial<Live> = {}): Live {
  return {
    lobby: {online: 3, build: 'abc12345', uptime_secs: 7200},
    matches: [],
    rooms: [],
    hosts: [{name: 'this server (lobby)', local: true, online: true, version_matches: true, games: 0}],
    ...over,
  };
}

function snap(over: Partial<Snapshot> = {}): Snapshot {
  return {live: live(), alerts: null, problems: null, bots: null, updates: null, release: null, triage: null, ...over};
}

test('a quiet lobby is all clear', () => {
  const f = findings(snap({alerts: [{name: 'disk', ok: true}], problems: [{at: NOW - 7200}]}), NOW);
  assert.deepEqual(f, []);
  assert.deepEqual(headline(f), {level: 'good', text: 'All clear.'});
});

test('the serious things come first and the headline names the worst', () => {
  const s = snap({
    live: live({
      hosts: [
        {name: 'us-east', online: true, version: 'old00000', version_matches: false, games: 0},
        {name: 'us-west', online: false, games: 0},
      ],
    }),
    alerts: [
      {name: 'disk', ok: false},
      {name: 'mail', ok: false},
      {name: 'db', ok: true},
    ],
    problems: [{at: NOW - 60}, {at: NOW - 3599}, {at: NOW - 3601}],
  });
  const f = findings(s, NOW);
  assert.deepEqual(
    f.map(x => [x.level, x.text, x.screen]),
    [
      ['bad', 'us-west is offline: rooms on it cannot start.', 'overview'],
      ['warn', 'us-east runs build old00000, the lobby runs abc12345.', 'overview'],
      ['warn', '2 alert checks failing: disk, mail.', 'health'],
      ['warn', '2 problems logged in the last hour.', 'health'],
    ],
  );
  assert.deepEqual(headline(f), {level: 'bad', text: 'us-west is offline: rooms on it cannot start.'});
});

test('a bad connection links to its match; bots do not count', () => {
  const m = (match: number, room: number, players: Live['matches'][number]['players']) => ({
    match,
    room,
    mode: 'elimination',
    map: 'Junk Flea',
    host: 'us-west',
    port: 27016,
    ready: true,
    players,
  });
  const one = findings(
    snap({
      live: live({
        matches: [
          m(41, 3, [
            {nick: 'ann', health: 'bad'},
            {nick: 'bot1', bot: true, health: 'bad'},
            {nick: 'bob', health: 'warn'},
          ]),
        ],
      }),
    }),
    NOW,
  );
  assert.deepEqual(one, [
    {level: 'bad', text: 'ann has a bad connection in room 3.', screen: 'matches', arg: '41'},
    {level: 'warn', text: "bob's connection is shaky.", screen: 'matches', arg: '41'},
  ]);
  // two matches: the list of matches, not one of them
  const two = findings(
    snap({live: live({matches: [m(41, 3, [{nick: 'ann', health: 'bad'}]), m(42, 4, [{nick: 'cy', health: 'bad'}])]})}),
    NOW,
  );
  assert.deepEqual(two, [
    {level: 'bad', text: '2 players have a bad connection: ann, cy.', screen: 'matches', arg: ''},
  ]);
  assert.equal(worstHealth([{nick: 'bot1', bot: true, health: 'bad'}]), 'none');
  assert.equal(worstHealth([{nick: 'a', health: 'good'}, {nick: 'b'}]), 'good');
  assert.equal(worstHealth([{nick: 'a', health: 'good'}, {nick: 'b', health: 'warn'}]), 'warn');
  assert.equal(worstHealth([{nick: 'a', health: 'warn'}, {nick: 'b', health: 'bad'}]), 'bad');
});

test('what only waits is listed and leaves the headline clear', () => {
  const f = findings(
    snap({
      updates: {set_up: true, available: {ok: true, new: [{}, {}, {}]}},
      release: {set_up: true, status: {phase: 'ready', next_tag: 'v0.4.0'}},
      triage: {set_up: true, open: 1, state: {ok: true}},
      bots: {managed: true, running: 13, wanted: 13},
    }),
    NOW,
  );
  assert.deepEqual(
    f.map(x => [x.level, x.text, x.screen]),
    [
      ['info', '3 new commits on master.', 'deploy'],
      ['info', 'Release v0.4.0 is ready to tag.', 'release'],
      ['info', '1 open bug report.', 'bugs'],
    ],
  );
  assert.deepEqual(headline(f), {level: 'good', text: 'All clear.'});
});

test('deploys, the release, bug reports and bots that are wrong', () => {
  const f = findings(
    snap({
      updates: {
        set_up: true,
        status: {ok: false, finished: NOW - 600, short: 'deadbeef'},
        available: {ok: false, new: [{}]},
      },
      release: {set_up: true, status: {phase: 'failed'}},
      triage: {set_up: true, open: 0, state: {ok: false, message: 'x'}},
      bots: {managed: true, running: 11, wanted: 13},
    }),
    NOW,
  );
  assert.deepEqual(
    f.map(x => [x.level, x.text]),
    [
      ['warn', '11 of 13 bots running.'],
      ['warn', 'The deploy of deadbeef failed.'],
      ['warn', 'The check for new commits is failing.'],
      ['warn', 'The public release failed.'],
      ['warn', 'The bug report poll is failing.'],
      ['info', '1 new commit on master.'],
    ],
  );
  // a failed deploy of more than an hour ago is history; one under way hides the new commits
  const old = findings(snap({updates: {set_up: true, status: {ok: false, finished: NOW - 3700, short: 'x'}}}), NOW);
  assert.deepEqual(old, []);
  const busy = findings(
    snap({updates: {set_up: true, deploying: true, status: {short: 'cafe0001'}, available: {new: [{}]}}}),
    NOW,
  );
  assert.deepEqual(busy.map(x => x.text), ['Deploying cafe0001.']);
  // not set up, or bots not run by this lobby: nothing to say
  assert.deepEqual(
    findings(
      snap({
        updates: {set_up: false, available: {new: [{}]}},
        release: {set_up: false, status: {phase: 'failed'}},
        triage: {set_up: false, open: 4},
        bots: {managed: false, running: 0, wanted: 5},
      }),
      NOW,
    ),
    [],
  );
  const noexe = findings(snap({bots: {managed: true, exe_missing: true, running: 0, wanted: 5}}), NOW);
  assert.deepEqual(noexe.map(x => [x.level, x.text]), [['bad', 'dc-botclient is missing: no bot can start.']]);
});

test('a lobby that is shutting down says so first', () => {
  const l = live();
  l.lobby.stopping = true;
  const f = findings(snap({live: l, alerts: [{name: 'disk', ok: false}]}), NOW);
  assert.equal(f[0]?.text, 'The lobby is shutting down.');
  assert.equal(headline(f).level, 'bad');
});

test('durations', () => {
  assert.equal(duration(42), '42 s');
  assert.equal(duration(720), '12 min');
  assert.equal(duration(12600), '3.5 h');
  assert.equal(duration(4 * 86400), '4 days');
});
