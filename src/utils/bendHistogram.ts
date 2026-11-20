/** Exact frequency counts from individual attempts; inclusive limits, 10 mm bins.
 * Interior bins are [lower, upper); the last bin includes the upper limit.
 */
export function buildBendHistogram(values: readonly number[], lower: number | null, upper: number | null) {
  if (lower == null || upper == null || !Number.isFinite(lower) || !Number.isFinite(upper) || upper <= lower) return null;
  const intervalCount = Math.ceil((upper - lower) / 10);
  const bins = [
    { lower: null as number | null, upper: lower as number | null, count: 0, outside: true },
    ...Array.from({ length: intervalCount }, (_, index) => ({
      lower: lower + index * 10, upper: Math.min(upper, lower + (index + 1) * 10), count: 0, outside: false,
    })),
    { lower: upper as number | null, upper: null as number | null, count: 0, outside: true },
  ];
  let total = 0;
  for (const value of values) {
    if (!Number.isFinite(value)) continue;
    const index = value < lower ? 0 : value > upper ? bins.length - 1
      : 1 + Math.min(intervalCount - 1, Math.floor((value - lower) / 10));
    bins[index].count++;
    total++;
  }
  const outside = bins[0].count + bins[bins.length - 1].count;
  return {
    bins: bins.map((bin, index) => ({ ...bin, index, percent: total ? 100 * bin.count / total : 0 })),
    total, outside, within: total - outside, passRate: total ? 100 * (total - outside) / total : null,
  };
}
