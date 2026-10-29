import { userFacingError } from "./userFacingError";
import { useState } from "react";
import { RefreshCw, Search } from "lucide-react";
import { OsLogo } from "./OsLogo";
import { messages, type Language } from "./i18n";
import type { ConnectedDevice } from "./operatorTypes";
import { formatDeviceCode } from "./deviceCode";
import { Dropdown, Input, type MenuProps } from "antd";
import { deviceOnline } from "./deviceAvailability";

type Props = {
  language: Language;
  error: string;
  devices: ConnectedDevice[];
  refreshing: boolean;
  onRefresh: () => void;
  devicePresence: Record<string, { name: string; online: boolean | null }>;
  selectedCode: string;
  onSelect: (device: ConnectedDevice) => void;
  onConnect: (device: ConnectedDevice) => void;
  onDeviceMenu: (device: ConnectedDevice) => MenuProps;
};

export function RemoteConnectionPanel({
  language, error, devices, refreshing, onRefresh, devicePresence, selectedCode, onSelect, onConnect, onDeviceMenu,
}: Props) {
  const t = messages[language];
  const [search, setSearch] = useState("");
  const query = search.trim().toLocaleLowerCase();
  const codeQuery = query.replace(/[\s-]/g, "");
  const sortedDevices = devices.filter((device) => !query
    || (codeQuery.length > 0 && device.deviceCode.includes(codeQuery))
    || [device.alias, devicePresence[device.deviceCode]?.name, device.deviceCode, formatDeviceCode(device.deviceCode)]
      .some((value) => value?.toLocaleLowerCase().includes(query))).sort((first, second) =>
    Number(deviceOnline(second, devicePresence[second.deviceCode]) === true)
    - Number(deviceOnline(first, devicePresence[first.deviceCode]) === true));

  return (
    <section className="surface remote-list">
      <div className="list-heading device-list-heading">
        <h2>{t.connectedDevices}</h2>
        <div className="device-list-heading-actions">
          <span className="count-badge">{devices.length}</span>
          <button type="button" className="credential-icon-button device-list-refresh" aria-label={t.refreshDevices}
            title={t.refreshDevices} aria-busy={refreshing} disabled={refreshing} onClick={onRefresh}>
            <RefreshCw size={16} className={refreshing ? "password-refreshing" : undefined} />
          </button>
        </div>
      </div>
      <Input className="device-search" aria-label={t.searchDevices} placeholder={t.searchDevices}
        prefix={<Search size={15} />} allowClear value={search} onChange={(event) => setSearch(event.target.value)} />
      {error && <div className="inline-error" role="alert">{userFacingError(error, language)}</div>}
      {sortedDevices.length === 0 ? <div className="empty-list">{devices.length === 0 ? t.noConnectedDevices : t.noMatchingDevices}</div> : (
        <div className="device-list">
          {sortedDevices.map((device) => {
            const presence = devicePresence[device.deviceCode];
            const availability = deviceOnline(device, presence);
            const online = availability === true;
            const status = availability === null
              ? t.unknown
              : online ? t.online : t.offline;
            const deviceName = device.alias || presence?.name || t.unnamedDevice;
            return (
              <Dropdown key={device.deviceCode} trigger={["contextMenu"]} menu={onDeviceMenu(device)}>
              <button
                className={`home-recent-device ${selectedCode === device.deviceCode ? "selected" : ""}`}
                title={`${formatDeviceCode(device.deviceCode)} · ${deviceName} · ${status}`}
                onClick={() => onSelect(device)}
                onDoubleClick={() => onConnect(device)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    onConnect(device);
                  }
                }}
              >
                <span className="home-recent-device-icon"><OsLogo family={device.osFamily} /></span>
                <span className="home-recent-device-info">
                  <strong>{formatDeviceCode(device.deviceCode)}</strong>
                  <small>{deviceName}</small>
                </span>
                <span
                  className={`home-recent-presence ${online ? "online" : availability === false ? "offline" : "unknown"}`}
                  aria-label={status}
                />
                <span className={`home-recent-connection ${online ? "connected" : "disconnected"}`}>
                  {device.connected ? device.connectionPath === "p2p" ? "P2P" : device.connectionPath === "relay" ? "Relay" : t.connectionPathUnknown : status}
                </span>
              </button>
              </Dropdown>
            );
          })}
        </div>
      )}
    </section>
  );
}
