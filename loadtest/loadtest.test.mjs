import { test } from 'node:test';
import assert from 'node:assert/strict';
import { percentile, summarize } from './loadtest.mjs';

test('percentile picks ranks from a sorted list', () => {
  const v = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
  assert.equal(percentile(v, 0), 1);
  assert.equal(percentile(v, 1), 10);
  assert.equal(percentile(v, 0.5), 6); // index round(4.5) = 5
  assert.ok(Number.isNaN(percentile([], 0.5)));
  assert.equal(percentile([7], 0.99), 7);
});

test('summarize counts statuses and computes throughput', () => {
  const samples = [
    { ms: 10, status: 200 }, { ms: 20, status: 200 }, { ms: 30, status: 201 },
    { ms: 40, status: 503 }, { ms: 50, status: 'network_error' },
  ];
  const s = summarize(samples, 2);
  assert.equal(s.requests, 5);
  assert.equal(s.requests_per_sec, 2.5);
  assert.equal(s.max_ms, 50);
  assert.deepEqual(s.by_status, { 200: 2, 201: 1, 503: 1, network_error: 1 });
  assert.equal(s.p50_ms, 30);
});
