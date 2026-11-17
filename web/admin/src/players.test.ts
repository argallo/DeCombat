// Provenance: ORIGINAL (DeCombat operator tooling)
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {
  type Filter,
  type Stay,
  connectionMix,
  countLine,
  leftWell,
  listRequests,
  marks,
  median,
  mixLine,
  perDeath,
} from './players.ts';

const F: Filter = {q: '', sort: 'seen', bots: false, banned: false};

const stay = (part: Partial<Stay>): Stay => ({
  match: 1,
  joined: 100,
  secs: 60,
  how: 'end',
  room: '1-1',
  map_name: 'Map',
  mode_key: 'tdm',
  host: 'h',
  outcome: 'result',
  has_link: true,
  health: 'good',
  rtt_mean: 40,
  ...part,
});

test('the list is asked for in answers of at most 200 rows', () => {
  assert.deepEqual(listRequests(F, 50), ['/ops/players?offset=0&limit=50&sort=seen&bots=0&banned=0&q=']);
  assert.deepEqual(listRequests({q: ' Al Pha ', sort: 'matches', bots: true, banned: true}, 450), [
    '/ops/players?offset=0&limit=200&sort=matches&bots=1&banned=1&q=Al+Pha',
    '/ops/players?offset=200&limit=200&sort=matches&bots=1&banned=1&q=Al+Pha',
    '/ops/players?offset=400&limit=50&sort=matches&bots=1&banned=1&q=Al+Pha',
  ]);
  assert.equal(listRequests(F, 200).length, 1);
  assert.equal(listRequests(F, 0).length, 1);
});

test("a row's marks, the gravest first", () => {
  const none = {banned: false, disabled: false, muted: false, bot: false, gm: false};
  assert.deepEqual(marks(none), []);
  assert.deepEqual(
    marks({banned: true, disabled: true, muted: true, bot: true, gm: true}).map(m => m.label),
    ['Banned', 'Login disabled', 'Muted', 'GM', 'Bot'],
  );
  assert.equal(marks({...none, banned: true})[0]?.variant, 'error');
});

test('the line above the list', () => {
  assert.equal(countLine(50, 1204, F), 'Showing 50 of 1,204 players.');
  assert.equal(countLine(50, 1204, {...F, bots: true}), 'Showing 50 of 1,204 players, bots included.');
  assert.equal(countLine(7, 7, F), 'All 7 players.');
  assert.equal(countLine(1, 1, {...F, banned: true}), '1 banned player.');
  assert.equal(countLine(0, 0, F), 'No accounts yet.');
  assert.equal(countLine(0, 0, {...F, q: 'x'}), 'No player matches that name.');
  assert.equal(countLine(0, 0, {...F, banned: true}), 'No banned player.');
  assert.equal(countLine(0, 0, {...F, banned: true, q: 'x'}), 'No banned player matches that name.');
});

test('kills for every death', () => {
  assert.equal(perDeath(30, 20), '1.50');
  assert.equal(perDeath(5, 0), '-');
});

test("what is counted from a person's matches", () => {
  assert.ok(leftWell('bye') && leftWell('end'));
  assert.ok(!leftWell('silent') && !leftWell('crash') && !leftWell(''));
  const stays = [
    stay({}),
    stay({health: 'warn'}),
    stay({health: 'bad'}),
    stay({has_link: false, health: 'none', rtt_mean: 0}),
    stay({health: 'good', rtt_mean: null}),
  ];
  const mix = connectionMix(stays);
  assert.deepEqual(mix, {good: 1, warn: 1, bad: 1, none: 2});
  assert.equal(mixLine(mix), '1 good, 1 shaky, 1 bad, 2 not measured');
  assert.equal(mixLine({good: 3, warn: 0, bad: 0, none: 0}), '3 good');
  assert.equal(median([]), null);
  assert.equal(median([30, 10, 20]), 20);
  assert.equal(median([40, 10, 20, 30]), 25);
});
