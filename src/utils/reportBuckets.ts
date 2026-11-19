export interface HalfHourBucket { at: string; ok: number; ng: number }

type ReportGrouping = "halfHour" | "day" | "week";
const dayMs = 24 * 60 * 60 * 1000;
const vietnamOffsetMs = 7 * 60 * 60 * 1000;

/** Display-only aggregation in Vietnam time; ranges are [from, to), weeks start Monday. */
export function groupedReportWindow(values: HalfHourBucket[] | undefined, from?: string, to?: string, page?: number) {
  let firstData = Infinity, lastData = -Infinity;
  for (const bucket of values ?? []) {
    const at = Date.parse(bucket.at);
    if (Number.isFinite(at)) { firstData = Math.min(firstData, at); lastData = Math.max(lastData, at); }
  }
  const start = from ? Date.parse(from) : firstData;
  const end = to ? Date.parse(to) : lastData + 30 * 60 * 1000;
  const validRange = Number.isFinite(start) && Number.isFinite(end) && end > start;
  const days = validRange ? Math.floor((end - 1 + vietnamOffsetMs) / dayMs)
    - Math.floor((start + vietnamOffsetMs) / dayMs) + 1 : 0;
  const grouping: ReportGrouping = days <= 1 ? "halfHour" : days <= 31 ? "day" : "week";
  const duration = grouping === "halfHour" ? 30 * 60 * 1000 : grouping === "day" ? dayMs : 7 * dayMs;
  // 1970-01-05 is Monday. Shift to local time before aligning calendar periods.
  const anchor = grouping === "week" ? 4 * dayMs : 0;
  const align = (at: number) => Math.floor((at + vietnamOffsetMs - anchor) / duration) * duration + anchor - vietnamOffsetMs;
  const first = validRange ? align(start) : 0;
  const count = validRange ? Math.ceil((end - first) / duration) : 0;
  const size = 6;
  const pages = Math.max(1, Math.ceil(count / size));
  const current = Math.max(1, Math.min(page ?? pages, pages));
  const windowEnd = count - (pages - current) * size;
  const windowStart = Math.max(0, windowEnd - size);
  const totals = new Map<number, { ok: number; ng: number }>();
  for (const bucket of values ?? []) {
    const at = Date.parse(bucket.at);
    if (!validRange || !Number.isFinite(at) || at < start || at >= end) continue;
    const key = align(at);
    const total = totals.get(key) ?? { ok: 0, ng: 0 };
    total.ok += bucket.ok;
    total.ng += bucket.ng;
    totals.set(key, total);
  }
  // Build only this window, including zero buckets; do not materialize an entire long range.
  const items = Array.from({ length: values == null ? 0 : Math.max(0, windowEnd - windowStart) }, (_, index) => {
    const at = first + (windowStart + index) * duration;
    return { at: new Date(at).toISOString(), from: new Date(Math.max(at, start)).toISOString(),
      to: new Date(Math.min(at + duration, end)).toISOString(), ...(totals.get(at) ?? { ok: 0, ng: 0 }) };
  });
  return { items, current, pages, grouping };
}

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
