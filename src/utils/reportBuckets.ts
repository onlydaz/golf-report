export interface HalfHourBucket { at: string; ok: number; ng: number }
const step = 30 * 60 * 1000;
export const reportWindowBuckets = 336;

/** Page the display, preserving exact 30-minute buckets and the full report filter. */
export function halfHourWindow(values: HalfHourBucket[] | undefined, from?: string, to?: string, page?: number) {
  if (!values) return { items: [], current: 1, pages: 1 };
  const timestamps = (values ?? []).map(item => Date.parse(item.at)).filter(Number.isFinite);
  let first = Infinity, last = -Infinity;
  for (const at of timestamps) { first = Math.min(first, at); last = Math.max(last, at); }
  const start = Math.floor((from ? Date.parse(from) : first) / step) * step;
  const end = to ? Date.parse(to) : last + step;
  const count = Number.isFinite(start) && Number.isFinite(end) && end > start ? Math.ceil((end - start) / step) : 0;
  const pages = Math.max(1, Math.ceil(count / reportWindowBuckets));
  const current = Math.max(1, Math.min(page ?? pages, pages));
  const offset = (current - 1) * reportWindowBuckets;
  const byTime = new Map((values ?? []).map(item => [Date.parse(item.at), item]));
  const items = Array.from({ length: Math.min(reportWindowBuckets, Math.max(0, count - offset)) }, (_, index) => {
    const at = start + (offset + index) * step, value = byTime.get(at);
    return { at: new Date(at).toISOString(), ok: value?.ok ?? 0, ng: value?.ng ?? 0 };
  });
  return { items, current, pages };
}
