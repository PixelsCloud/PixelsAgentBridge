import { OsLogo } from "./OsLogo";
import { messages, type Language } from "./i18n";
import type { ConnectedDevice } from "./operatorTypes";
import { formatDeviceCode } from "./deviceCode";
import { Dropdown, type MenuProps } from "antd";

type Props = {
  language: Language;
  error: string;
  devices: ConnectedDevice[];
  devicePresence: Record<string, { name: string; online: boolean | null }>;
  selectedCode: string;
  onSelect: (device: ConnectedDevice) => void;
  onConnect: (device: ConnectedDevice) => void;
  onDeviceMenu: (device: ConnectedDevice) => MenuProps;
};

export function RemoteConnectionPanel({
  language, error, devices, devicePresence, selectedCode, onSelect, onConnect, onDeviceMenu,
}: Props) {
  const t = messages[language];
  const sortedDevices = [...devices].sort((first, second) =>
    Number(devicePresence[second.deviceCode]?.online === true)
    - Number(devicePresence[first.deviceCode]?.online === true));

  return (
    <section className="surface remote-list">
      <div className="list-heading"><h2>{t.connectedDevices}</h2><span>{devices.length}</span></div>
      {error && <div className="inline-error" role="alert">{error}</div>}
      {devices.length === 0 ? <div className="empty-list">{t.noConnectedDevices}</div> : (
        <div className="device-list">
          {sortedDevices.map((device) => {
            const presence = devicePresence[device.deviceCode];
            const online = presence?.online === true;
            const status = presence?.online === null || presence === undefined
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
                  className={`home-recent-presence ${online ? "online" : presence?.online === false ? "offline" : "unknown"}`}
                  aria-label={status}
                />
                <span className={`home-recent-connection ${device.connected ? "connected" : "disconnected"}`}>
                  {device.connected ? t.connected : t.disconnected}
                  {device.connected && device.connectionPath && (
                    <> · {device.connectionPath === "p2p" ? "P2P" : device.connectionPath === "relay" ? "Relay" : t.connectionPathUnknown}</>
                  )}
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
