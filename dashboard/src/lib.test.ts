// Run with:  npm test   (Node 22.18+ runs TypeScript directly; no extra packages needed)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  buildFilter,
  cellText,
  columnsOf,
  formatBytes,
  nextSort,
  parseJsonObject,
  parseScalar,
  percent,
  timeAgo,
} from './lib.ts';

test('formatBytes', () => {
  assert.equal(formatBytes(0), '0 B');
  assert.equal(formatBytes(1023), '1023 B');
  assert.equal(formatBytes(1024), '1.0 KiB');
  assert.equal(formatBytes(1536), '1.5 KiB');
  assert.equal(formatBytes(100 * 1024 * 1024), '100 MiB');
  assert.equal(formatBytes(-1), '—');
  assert.equal(formatBytes(NaN), '—');
});

test('parseJsonObject', () => {
  assert.deepEqual(parseJsonObject(''), { ok: true, value: {} });
  assert.deepEqual(parseJsonObject('  {"a": 1} '), { ok: true, value: { a: 1 } });
  for (const bad of ['{', '[1]', '5', 'null', '"x"']) {
    const r = parseJsonObject(bad);
    assert.equal(r.ok, false, bad);
  }
});

test('parseScalar', () => {
  assert.equal(parseScalar('42'), 42);
  assert.equal(parseScalar('-1.5'), -1.5);
  assert.equal(parseScalar('true'), true);
  assert.equal(parseScalar('null'), null);
  assert.equal(parseScalar('"x y"'), 'x y');
  assert.equal(parseScalar('Delhi'), 'Delhi');
  assert.equal(parseScalar(' Delhi '), 'Delhi');
  assert.equal(parseScalar(''), '');
  assert.equal(parseScalar('{"a":1}'), '{"a":1}'); // objects are not accepted as scalars
  assert.equal(parseScalar('[1]'), '[1]');
});

test('buildFilter', () => {
  assert.deepEqual(buildFilter('city', 'eq', 'Delhi'), { city: 'Delhi' });
  assert.deepEqual(buildFilter('age', 'gt', '30'), { age: { $gt: 30 } });
  assert.deepEqual(buildFilter('age', 'lte', '30'), { age: { $lte: 30 } });
  assert.deepEqual(buildFilter('city', 'ne', 'Pune'), { city: { $ne: 'Pune' } });
  assert.deepEqual(buildFilter('  ', 'eq', 'x'), {});
  assert.deepEqual(buildFilter('address.city', 'eq', '"42"'), { 'address.city': '42' });
});

test('columnsOf puts _id first and caps the count', () => {
  const docs = [{ name: 'a', _id: '1' }, { _id: '2', age: 3, city: 'x' }];
  assert.deepEqual(columnsOf(docs), ['_id', 'name', 'age', 'city']);
  assert.deepEqual(columnsOf(docs, 2), ['_id', 'name']);
  assert.deepEqual(columnsOf([]), []);
  assert.deepEqual(columnsOf([{ a: 1 }]), ['a']);
});

test('cellText', () => {
  assert.equal(cellText('abc'), 'abc');
  assert.equal(cellText(5), '5');
  assert.equal(cellText({ a: 1 }), '{"a":1}');
  assert.equal(cellText(undefined), '');
  assert.equal(cellText('x'.repeat(100), 10), 'xxxxxxxxx…');
});

test('nextSort cycles asc -> desc -> none and resets on a new column', () => {
  let s = nextSort(null, 'age');
  assert.deepEqual(s, { field: 'age', order: 'asc' });
  s = nextSort(s, 'age');
  assert.deepEqual(s, { field: 'age', order: 'desc' });
  s = nextSort(s, 'age');
  assert.equal(s, null);
  assert.deepEqual(nextSort({ field: 'age', order: 'desc' }, 'name'), { field: 'name', order: 'asc' });
});

test('percent is clamped', () => {
  assert.equal(percent(50, 100), 50);
  assert.equal(percent(500, 100), 100);
  assert.equal(percent(1, 0), 0);
  assert.equal(percent(-5, 100), 0);
});

test('timeAgo', () => {
  const now = 1_000_000_000_000;
  assert.equal(timeAgo(now, 0), 'never');
  assert.equal(timeAgo(now, undefined), 'never');
  assert.equal(timeAgo(now, now - 2_000), 'just now');
  assert.equal(timeAgo(now, now - 30_000), '30 seconds ago');
  assert.equal(timeAgo(now, now - 60_000), '1 minute ago');
  assert.equal(timeAgo(now, now - 5 * 60_000), '5 minutes ago');
  assert.equal(timeAgo(now, now - 3 * 3600_000), '3 hours ago');
  assert.equal(timeAgo(now, now - 72 * 3600_000), '3 days ago');
  assert.equal(timeAgo(now, now + 5000), 'just now'); // clock skew never gives negative text
});
