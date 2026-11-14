// Provenance: ORIGINAL (DeCombat operator tooling)

import assert from 'node:assert/strict';
import {test} from 'node:test';
import {binOf, nearest, starts, ticks, wholeFrom} from './scale.ts';

test('axis steps are round, start at 0 and reach the highest value', () => {
  assert.deepEqual(ticks(0), [0, 1]);
  assert.deepEqual(ticks(1), [0, 1]);
  assert.deepEqual(ticks(3), [0, 1, 2, 3]);
  assert.deepEqual(ticks(7), [0, 2, 4, 6, 8]);
  assert.deepEqual(ticks(12), [0, 5, 10, 15]);
  assert.deepEqual(ticks(100), [0, 50, 100]);
  assert.deepEqual(ticks(388), [0, 100, 200, 300, 400]);
  assert.deepEqual(ticks(912), [0, 500, 1000]);
  assert.deepEqual(ticks(300, 3), [0, 100, 200, 300]);
});

test('the last days and hours are whole local spans and the newest holds now', () => {
  const now = new Date(2026, 10, 11, 15, 42, 7).getTime() / 1000;
  const days = starts(now, 14, 'day');
  assert.equal(days.length, 14);
  assert.equal(days[13], new Date(2026, 10, 11).getTime() / 1000);
  assert.equal(days[0], new Date(2026, 9, 29).getTime() / 1000);
  const hours = starts(now, 24, 'hour');
  assert.equal(hours.length, 24);
  assert.equal(hours[23], new Date(2026, 10, 11, 15).getTime() / 1000);
  assert.equal(hours[0], new Date(2026, 10, 10, 16).getTime() / 1000);
});

test('a time falls into the span that starts at or before it', () => {
  const from = [100, 200, 300];
  assert.equal(binOf(99, from), -1);
  assert.equal(binOf(100, from), 0);
  assert.equal(binOf(199, from), 0);
  assert.equal(binOf(200, from), 1);
  assert.equal(binOf(5000, from), 2);
  assert.equal(binOf(5, []), -1);
});

test('a list cut at its limit is whole only from its oldest row', () => {
  assert.equal(wholeFrom([50, 40, 30], 100), 0);
  assert.equal(wholeFrom([50, 40, 30], 3), 30);
  assert.equal(wholeFrom([], 3), 0);
});

test('the nearest report to a pointer', () => {
  const t = [10, 20, 30, 40];
  assert.equal(nearest([], 5), -1);
  assert.equal(nearest([7], 500), 0);
  assert.equal(nearest(t, 0), 0);
  assert.equal(nearest(t, 14), 0);
  assert.equal(nearest(t, 16), 1);
  assert.equal(nearest(t, 25), 1);
  assert.equal(nearest(t, 39), 3);
  assert.equal(nearest(t, 999), 3);
});
