import { expect, test } from '@playwright/test';
import { ProgressEstimator } from '../../src/lib/components/shared/progress-estimate';

test('estimates require five seconds of observed progress and never infer a persisted start', () => {
  const estimator = new ProgressEstimator();
  estimator.observe('restored:preparing', 600, 1_000);
  expect(estimator.estimate(5_000, 7_200)).toBeNull();
  estimator.observe('restored:preparing', 608, 5_000);
  expect(estimator.estimate(5_000, 7_200)).toBeNull();
  estimator.observe('restored:preparing', 610, 6_000);
  expect(estimator.estimate(6_000, 7_200)).toEqual({
    kind: 'estimate',
    speed: 2,
    remainingSeconds: 3_295,
  });
});

test('the recent observation window adapts when a faster section finishes', () => {
  const estimator = new ProgressEstimator();
  for (let second = 0; second <= 60; second++) {
    estimator.observe('job:running', second <= 30 ? second * 4 : 120 + second - 30, second * 1_000);
  }
  expect(estimator.estimate(60_000, 200)).toEqual({
    kind: 'estimate',
    speed: 1,
    remainingSeconds: 50,
  });
});

test('stalling ages the estimate, hides stale numbers, and requires fresh samples on recovery', () => {
  const estimator = new ProgressEstimator();
  estimator.observe('job:preparing', 0, 0);
  estimator.observe('job:preparing', 10, 5_000);
  expect(estimator.estimate(5_000, 100)).toEqual({
    kind: 'estimate',
    speed: 2,
    remainingSeconds: 45,
  });
  expect(estimator.estimate(10_000, 100)).toEqual({
    kind: 'estimate',
    speed: 1,
    remainingSeconds: 90,
  });
  estimator.observe('job:preparing', 10, 20_000);
  expect(estimator.estimate(20_000, 100)).toEqual({ kind: 'waiting' });
  estimator.observe('job:preparing', 20, 21_000);
  expect(estimator.estimate(21_000, 100)).toBeNull();
  estimator.observe('job:preparing', 30, 26_000);
  expect(estimator.estimate(26_000, 100)).toEqual({
    kind: 'estimate',
    speed: 2,
    remainingSeconds: 35,
  });
});

test('phase changes, job changes, regressions, and missing progress each discard the old estimate', () => {
  for (const [nextKey, nextProgress] of [
    ['job:running', 20],
    ['other:preparing', 20],
    ['job:preparing', 5],
    ['job:preparing', null],
    [null, 20],
  ] as const) {
    const estimator = new ProgressEstimator();
    estimator.observe('job:preparing', 0, 0);
    estimator.observe('job:preparing', 10, 5_000);
    expect(estimator.estimate(5_000, 100)?.kind).toBe('estimate');
    estimator.observe(nextKey, nextProgress, 6_000);
    expect(estimator.estimate(6_000, 100)).toBeNull();
  }
});

test('speed can be observed without a duration but ETA requires a positive unfinished duration', () => {
  const estimator = new ProgressEstimator();
  estimator.observe('job:finalizing', 0, 0);
  estimator.observe('job:finalizing', 10, 5_000);
  for (const duration of [null, 0, -1, Number.NaN, Number.POSITIVE_INFINITY, 10, 9]) {
    expect(estimator.estimate(5_000, duration)).toEqual({
      kind: 'estimate',
      speed: 2,
      remainingSeconds: null,
    });
  }
});
