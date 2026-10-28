export type TransferRate = { bytesPerSecond: bigint | null; etaSeconds: bigint | null };
export type RateSnapshot = { spec: { id: string }; revision: string; phase: string; confirmed: string; size: string; scan_complete: boolean };
type Sample = { at: number; bytes: bigint };
type Observation = { revision: bigint; phase: string; confirmed: bigint; seen: number; progressed: number; samples: Sample[]; rate: TransferRate };
export const unknownTransferRate: TransferRate = { bytesPerSecond: null, etaSeconds: null };
const windowMs = 5_000, staleMs = 3_000;

/** Receiver-confirmed logical bytes, sampled with a monotonic local clock.
 * A pause, restart, observation gap or stalled acknowledgement starts a new
 * measurement segment. It never estimates time from sent/unconfirmed bytes.
 */
export class TransferRateTracker {
  private records = new Map<string, Observation>();
  clear() { this.records.clear(); }
  retain(ids: string[]) { const keep = new Set(ids); for (const id of this.records.keys()) if (!keep.has(id)) this.records.delete(id); }
  fresh(id: string, now: number): boolean {
    const record = this.records.get(id);
    return !!record && record.phase === "running" && now >= record.seen && now >= record.progressed && now - record.seen < staleMs && now - record.progressed < staleMs;
  }
  observe(snapshot: RateSnapshot, now: number): TransferRate {
    const id = snapshot.spec.id;
    if (!Number.isFinite(now) || now < 0 || ![snapshot.revision, snapshot.confirmed, snapshot.size].every(value => /^\d+$/.test(value))) {
      this.records.delete(id); return unknownTransferRate;
    }
    const revision = BigInt(snapshot.revision), bytes = BigInt(snapshot.confirmed), size = BigInt(snapshot.size), old = this.records.get(id);
    if (old && revision < old.revision) return this.fresh(id, now) ? old.rate : unknownTransferRate;
    if (snapshot.phase !== "running") {
      this.records.set(id, { revision, phase: snapshot.phase, confirmed: bytes, seen: now, progressed: now, samples: [], rate: unknownTransferRate });
      return unknownTransferRate;
    }
    if (!old || old.phase !== "running" || now < old.seen || bytes < old.confirmed || now - old.seen >= staleMs || now - old.progressed >= staleMs) {
      const progressed = !old || bytes !== old.confirmed || old.phase !== "running" || now < old.seen ? now : old.progressed;
      this.records.set(id, { revision, phase: snapshot.phase, confirmed: bytes, seen: now, progressed, samples: [{ at: now, bytes }], rate: unknownTransferRate });
      return unknownTransferRate;
    }
    if (bytes > old.confirmed) old.progressed = now;
    old.revision = revision; old.confirmed = bytes; old.seen = now;
    if (old.samples[old.samples.length - 1]?.at === now) old.samples[old.samples.length - 1].bytes = bytes;
    else old.samples.push({ at: now, bytes });
    while (old.samples.length > 1 && old.samples[0].at < now - windowMs) old.samples.shift();
    while (old.samples.length > 32) old.samples.shift();
    const first = old.samples[0], elapsed = Math.round(now - first.at), delta = bytes - first.bytes;
    if (elapsed <= 0 || delta <= 0n) return old.rate = unknownTransferRate;
    const milliseconds = BigInt(elapsed), bytesPerSecond = delta * 1000n / milliseconds;
    const divisor = delta * 1000n;
    const etaSeconds = snapshot.scan_complete && size > bytes ? ((size - bytes) * milliseconds + divisor - 1n) / divisor : null;
    return old.rate = { bytesPerSecond, etaSeconds };
  }
}

export function remainingTime(seconds: bigint | null, en: boolean): string {
  if (seconds === null || seconds < 0n) return "—";
  const days = seconds / 86400n, hours = seconds / 3600n % 24n, minutes = seconds / 60n % 60n, rest = seconds % 60n;
  if (days > 99n) return en ? ">99d" : ">99天";
  if (days) return en ? `${days}d ${hours}h` : `${days}天${hours}小时`;
  if (hours) return en ? `${hours}h ${minutes}m` : `${hours}小时${minutes}分钟`;
  if (minutes) return en ? `${minutes}m ${rest}s` : `${minutes}分${rest}秒`;
  return en ? `${rest}s` : `${rest}秒`;
}
