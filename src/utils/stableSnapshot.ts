/** Structural sharing for JSON IPC DTOs. Every field is compared, including warnings. */
export function stableSnapshot<T>(previous: T, next: T): T {
  if (Object.is(previous, next)) return previous;
  if (previous == null || next == null || typeof previous !== "object" || typeof next !== "object"
    || Array.isArray(previous) !== Array.isArray(next)) return next;
  const before = previous as Record<string, unknown>, after = next as Record<string, unknown>;
  const keys = Object.keys(after);
  const shared = (Array.isArray(next) ? [] : {}) as Record<string, unknown>;
  let equal = Object.keys(before).length === keys.length;
  for (const key of keys) {
    shared[key] = stableSnapshot(before[key], after[key]);
    equal &&= Object.prototype.hasOwnProperty.call(before, key) && Object.is(shared[key], before[key]);
  }
  return (equal ? previous : shared) as T;
}

/** Share in-flight reads; manual refresh and polling never duplicate the same IPC. */
export function singleFlight<T>(read: () => Promise<T>) {
  let pending: Promise<T> | undefined;
  const get = () => pending ??= Promise.resolve().then(read).finally(() => { pending = undefined; });
  // An explicit settings change must not reuse a read begun before the change.
  get.fresh = async () => {
    if (pending) await pending.catch(() => undefined);
    return get();
  };
  return get;
}
