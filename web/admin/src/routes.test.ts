// Provenance: ORIGINAL (DeCombat operator tooling)
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {classicFor, navFor, parseHash, toHash} from './routes.ts';

test('an empty address is the overview', () => {
  assert.deepEqual(parseHash(''), {screen: 'overview', arg: ''});
  assert.deepEqual(parseHash('#'), {screen: 'overview', arg: ''});
  assert.deepEqual(parseHash('#/'), {screen: 'overview', arg: ''});
});

test('screens and their argument', () => {
  assert.deepEqual(parseHash('#/matches'), {screen: 'matches', arg: ''});
  assert.deepEqual(parseHash('#/matches/12'), {screen: 'matches', arg: '12'});
  assert.deepEqual(parseHash('#/players/Some%20Name'), {screen: 'players', arg: 'Some Name'});
  assert.deepEqual(parseHash('#/nope'), {screen: null, arg: ''});
  assert.deepEqual(parseHash('#/players/%E0%A4%A'), {screen: 'players', arg: '%E0%A4%A'});
});

test('the hashes of the earlier page keep working', () => {
  assert.deepEqual(parseHash('#live'), {screen: 'overview', arg: ''});
  assert.deepEqual(parseHash('#match=12'), {screen: 'matches', arg: '12'});
  assert.deepEqual(parseHash('#timeline=Some%20Name'), {screen: 'players', arg: 'Some Name'});
  assert.deepEqual(parseHash('#timeline'), {screen: 'players', arg: ''});
  assert.deepEqual(parseHash('#problems'), {screen: 'health', arg: ''});
  assert.deepEqual(parseHash('#alerts'), {screen: 'health', arg: ''});
  assert.deepEqual(parseHash('#updates'), {screen: 'deploy', arg: ''});
  assert.deepEqual(parseHash('#nothing'), {screen: null, arg: ''});
});

test('a hash round trip', () => {
  assert.equal(toHash('players', 'a/b c'), '#/players/a%2Fb%20c');
  assert.deepEqual(parseHash(toHash('players', 'a/b c')), {screen: 'players', arg: 'a/b c'});
  assert.equal(toHash('deploy'), '#/deploy');
});

test('a further instance has no box-wide screens, the loopback port no keys', () => {
  const ids = (onInstance: boolean, web: boolean) => navFor(onInstance, web).flatMap(g => g.items.map(i => i.id));
  assert.deepEqual(ids(false, true), [
    'overview', 'matches', 'health', 'bugs', 'players', 'beta', 'bots', 'deploy', 'packs', 'release', 'keys',
  ]);
  assert.deepEqual(ids(true, true), ['overview', 'matches', 'health', 'players', 'beta', 'bots']);
  assert.ok(!ids(false, false).includes('keys'));
});

test('a screen that has not moved opens the matching tab of the earlier page', () => {
  const at = (hash: string) => classicFor(parseHash(hash));
  assert.deepEqual(at(''), {path: '/ops/classic', hash: '#live'});
  assert.deepEqual(at('#/matches'), {path: '/ops/classic', hash: '#matches'});
  assert.deepEqual(at('#/matches/12'), {path: '/ops/classic', hash: '#match=12'});
  assert.deepEqual(at('#/players/Some%20Name'), {path: '/ops/classic', hash: '#timeline=Some%20Name'});
  assert.deepEqual(at('#/players'), {path: '/ops/classic', hash: '#timeline'});
  assert.deepEqual(at('#/health'), {path: '/ops/classic', hash: '#problems'});
  assert.deepEqual(at('#/bots'), {path: '/ops/classic', hash: '#bots'});
  for (const s of ['bugs', 'deploy', 'packs', 'release']) {
    assert.deepEqual(at('#/' + s), {path: '/ops/classic', hash: '#updates'}, s);
  }
  assert.deepEqual(at('#/beta'), {path: '/ops/beta', hash: ''});
  // an earlier bookmark lands on the same tab again
  assert.deepEqual(at('#bots'), {path: '/ops/classic', hash: '#bots'});
  assert.deepEqual(at('#match=7'), {path: '/ops/classic', hash: '#match=7'});
  assert.deepEqual(at('#nothing'), {path: '/ops/classic', hash: '#live'});
});
