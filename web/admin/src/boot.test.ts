// Provenance: ORIGINAL (DeCombat operator tooling)
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {EMPTY_BOOT, basePath, lobbyLabel, parseBoot} from './boot.ts';

test('nothing or nonsense is the plain single-lobby page', () => {
  assert.deepEqual(parseBoot(null), EMPTY_BOOT);
  assert.deepEqual(parseBoot(''), EMPTY_BOOT);
  assert.deepEqual(parseBoot('{'), EMPTY_BOOT);
  assert.deepEqual(parseBoot('[1]'), {web: false, label: '', current: null, instances: []});
  assert.deepEqual(parseBoot('"x"'), EMPTY_BOOT);
});

test('the admin web with a further instance open', () => {
  const b = parseBoot(
    JSON.stringify({
      web: true,
      label: 'Vanilla lobby',
      current: 'modded',
      instances: [{name: 'modded', label: 'Hacking Mod lobby'}],
    }),
  );
  assert.equal(b.web, true);
  assert.equal(basePath(b), '/i/modded');
  assert.equal(lobbyLabel(b), 'Hacking Mod lobby');
});

test('the own lobby has no base', () => {
  const b = parseBoot('{"web":true,"label":"Vanilla lobby","current":null,"instances":[{"name":"modded","label":"M"}]}');
  assert.equal(basePath(b), '');
  assert.equal(lobbyLabel(b), 'Vanilla lobby');
});

test('a name that is not an instance name never becomes a path', () => {
  const b = parseBoot(
    '{"web":true,"label":"L","current":"../x","instances":[{"name":"../x","label":"bad"},{"name":"ok","label":"Ok"}]}',
  );
  assert.deepEqual(b.instances, [{name: 'ok', label: 'Ok'}]);
  assert.equal(b.current, null);
  assert.equal(basePath(b), '');
  // a current that is not among the instances is dropped as well
  assert.equal(parseBoot('{"current":"ok","instances":[]}').current, null);
});
