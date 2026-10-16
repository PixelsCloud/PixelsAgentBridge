export type DeviceStatus = {
  device_code: string;
  device_id: string;
  temporary_password: string;
  password_version?: number;
  executor_running: boolean;
  control_phase: string;
};

export function currentDeviceStatus(previous: DeviceStatus | null, next: DeviceStatus): DeviceStatus {
  return previous?.device_id === next.device_id && (previous.password_version ?? 0) > (next.password_version ?? 0)
    ? { ...next, temporary_password: previous.temporary_password, password_version: previous.password_version }
    : next;
}
