// Small, dependency-free statistics for the benchmark harness. Everything
// random goes through a seeded PRNG so reports are reproducible.

/** mulberry32: a fast 32-bit PRNG; returns floats in [0, 1). */
export function rng(seed = 1) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const ascending = (a, b) => a - b;

/** Quantile of an ascending-sorted array, linear interpolation (Hyndman-Fan type 7, R's default). */
export function quantileSorted(sorted, p) {
  if (sorted.length === 0) return NaN;
  const h = (sorted.length - 1) * p;
  const lo = Math.floor(h);
  const hi = Math.ceil(h);
  return sorted[lo] + (h - lo) * (sorted[hi] - sorted[lo]);
}

export function quantile(xs, p) {
  return quantileSorted([...xs].sort(ascending), p);
}

export function median(xs) {
  return quantile(xs, 0.5);
}

/** n, median, quartiles, IQR, min, max, mean, and IQR relative to the median. */
export function summarize(xs) {
  const s = [...xs].sort(ascending);
  const q1 = quantileSorted(s, 0.25);
  const med = quantileSorted(s, 0.5);
  const q3 = quantileSorted(s, 0.75);
  return {
    n: s.length,
    median: med,
    q1,
    q3,
    iqr: q3 - q1,
    iqr_rel: med > 0 ? (q3 - q1) / med : NaN,
    min: s.length ? s[0] : NaN,
    max: s.length ? s[s.length - 1] : NaN,
    mean: s.length ? s.reduce((a, b) => a + b, 0) / s.length : NaN,
  };
}

/** A same-size sample drawn with replacement. */
export function resample(xs, rand) {
  return Array.from({ length: xs.length }, () => xs[Math.floor(rand() * xs.length)]);
}

/**
 * Percentile-bootstrap CI of median(a) / median(b): each resample draws `a`
 * and `b` independently with replacement. Returns the point ratio and the
 * (1 - alpha) interval.
 */
export function bootstrapMedianRatio(a, b, { resamples = 10000, seed = 1, alpha = 0.05 } = {}) {
  const ratio = median(a) / median(b);
  if (a.length === 0 || b.length === 0) return { ratio: NaN, lo: NaN, hi: NaN };
  if (a === b) return { ratio: 1, lo: 1, hi: 1 };
  const rand = rng(seed);
  const stats = new Float64Array(resamples);
  for (let i = 0; i < resamples; i++) {
    stats[i] = median(resample(a, rand)) / median(resample(b, rand));
  }
  stats.sort();
  return {
    ratio,
    lo: quantileSorted(stats, alpha / 2),
    hi: quantileSorted(stats, 1 - alpha / 2),
  };
}

/** Ordinary least squares y = intercept + slope * x. */
export function ols(xs, ys) {
  const n = xs.length;
  const mx = xs.reduce((a, b) => a + b, 0) / n;
  const my = ys.reduce((a, b) => a + b, 0) / n;
  let sxx = 0;
  let sxy = 0;
  let syy = 0;
  for (let i = 0; i < n; i++) {
    sxx += (xs[i] - mx) ** 2;
    sxy += (xs[i] - mx) * (ys[i] - my);
    syy += (ys[i] - my) ** 2;
  }
  const slope = sxx > 0 ? sxy / sxx : NaN;
  const intercept = my - slope * mx;
  const r2 = sxx > 0 && syy > 0 ? (sxy * sxy) / (sxx * syy) : NaN;
  return { intercept, slope, r2 };
}

/**
 * Fit median(y) = fixed + perUnit * x across groups ({ x, ys }), with a
 * stratified percentile bootstrap: every resample redraws each group's runs
 * with replacement and refits on the resampled medians. Needs >= 2 distinct x.
 */
export function bootstrapFit(groups, { resamples = 10000, seed = 1, alpha = 0.05 } = {}) {
  const usable = groups.filter((g) => g.ys.length > 0);
  if (new Set(usable.map((g) => g.x)).size < 2) return null;
  const xs = usable.map((g) => g.x);
  const point = ols(
    xs,
    usable.map((g) => median(g.ys)),
  );
  const rand = rng(seed);
  const fixed = new Float64Array(resamples);
  const slope = new Float64Array(resamples);
  for (let i = 0; i < resamples; i++) {
    const fit = ols(
      xs,
      usable.map((g) => median(resample(g.ys, rand))),
    );
    fixed[i] = fit.intercept;
    slope[i] = fit.slope;
  }
  fixed.sort();
  slope.sort();
  const ci = (sorted, est) => ({
    est,
    lo: quantileSorted(sorted, alpha / 2),
    hi: quantileSorted(sorted, 1 - alpha / 2),
  });
  return {
    fixed: ci(fixed, point.intercept),
    per_unit: ci(slope, point.slope),
    r2: point.r2,
  };
}
