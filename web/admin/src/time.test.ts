// Provenance: ORIGINAL (DeCombat operator tooling)
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {ago, when} from './time.ts';

test('how long ago', () => {
  const now = 1_800_000_000;
  assert.equal(ago(now - 10, now), 'just now');
  assert.equal(ago(now - 89, now), 'just now');
  assert.equal(ago(now - 600, now), '10 min ago');
  assert.equal(ago(now - 7200, now), '2 h ago');
  assert.equal(ago(now - 35 * 3600, now), '35 h ago');
  // older than a day and a half: the date
  assert.match(ago(now - 4 * 86400, now), /^\d{4}-\d{2}-\d{2}$/);
  assert.equal(ago(0, now), '-');
  assert.equal(ago(null, now), '-');
});

test('a time is written with its date', () => {
  assert.match(when(1_800_000_000), /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
  assert.equal(when(undefined), '-');
});
