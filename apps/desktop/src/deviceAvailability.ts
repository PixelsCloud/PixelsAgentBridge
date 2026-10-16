import type { ConnectedDevice } from "./operatorTypes";

export type DevicePresence = { name: string; online: boolean | null };

// A live authenticated connection is stronger evidence than the Server's
// directory snapshot, which can lag or be unavailable during reconnection.
export function deviceOnline(device: ConnectedDevice | undefined, presence: DevicePresence | undefined): boolean | null {
  return device?.connected ? true : presence?.online ?? null;
}
