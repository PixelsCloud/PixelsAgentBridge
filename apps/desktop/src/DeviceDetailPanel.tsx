import { useState, type ReactNode } from "react";
import { ArrowUpRight } from "lucide-react";
import { messages, type Language } from "./i18n";
import { formatDeviceCode } from "./deviceCode";
import { OsLogo } from "./OsLogo";
import type { ConnectedDevice } from "./operatorTypes";

type DetailTab = "info" | "history" | "command";

type Props = {
  language: Language;
  selected: ConnectedDevice | undefined;
  presence: { name: string; online: boolean | null } | undefined;
  aliasDraft: string;
  onAliasDraftChange: (value: string) => void;
  onRenameDevice: () => void;
  history: ReactNode;
  command: ReactNode;
};

export function DeviceDetailPanel({
  language, selected, presence, aliasDraft, onAliasDraftChange, onRenameDevice,
  history, command,
}: Props) {
  const t = messages[language];
  const [tab, setTab] = useState<DetailTab>("info");
  const [historyOpened, setHistoryOpened] = useState(false);
  const [commandOpened, setCommandOpened] = useState(false);
  const name = selected?.alias || presence?.name || t.unnamedDevice;
  const online = presence?.online === true ? t.online : presence?.online === false ? t.offline : t.unknown;
  const connection = selected?.connected ? t.connected : t.disconnected;
  const path = selected?.connected
    ? selected.connectionPath === "p2p"
      ? "P2P"
      : selected.connectionPath === "relay"
        ? "Relay"
        : t.connectionPathUnknown
    : "—";

  function openTab(next: DetailTab) {
    setTab(next);
    if (next === "history") setHistoryOpened(true);
    if (next === "command") setCommandOpened(true);
  }

  return (
    <section className="surface device-detail">
      <div className="device-detail-tabs" role="tablist" aria-label={t.deviceDetails}>
        <button role="tab" aria-selected={tab === "info"} className={tab === "info" ? "active" : ""} onClick={() => openTab("info")}>{t.deviceBasicInfo}</button>
        <button role="tab" aria-selected={tab === "history"} className={tab === "history" ? "active" : ""} onClick={() => openTab("history")}>{t.deviceTaskHistory}</button>
        <button role="tab" aria-selected={tab === "command"} className={tab === "command" ? "active" : ""} onClick={() => openTab("command")}>{t.commandTitle}</button>
      </div>

      {!selected ? (
        <div className="empty-panel"><span><ArrowUpRight /></span><strong>{t.selectDevice}</strong><p>{t.selectDeviceHint}</p></div>
      ) : <>
        <div className="device-basic-info" role="tabpanel" hidden={tab !== "info"}>
          <div className="device-basic-heading">
            <span className="device-basic-os"><OsLogo family={selected.osFamily} /></span>
            <div><strong>{formatDeviceCode(selected.deviceCode)}</strong><span>{name}</span></div>
          </div>
          <div className="device-basic-grid">
            <div><span>{t.targetOs}</span><strong>{selected.osFamily}</strong></div>
            <div><span>{t.deviceAvailability}</span><strong>{online}</strong></div>
            <div><span>{t.deviceConnection}</span><strong>{connection}</strong></div>
            <div><span>{t.deviceConnectionPath}</span><strong>{path}</strong></div>
          </div>
          <div className="device-basic-alias">
            <label htmlFor="detail-device-alias">{t.deviceAlias}</label>
            <div>
              <input
                id="detail-device-alias"
                value={aliasDraft}
                maxLength={64}
                onChange={(event) => onAliasDraftChange(event.target.value)}
              />
              <button className="icon-text-button" disabled={aliasDraft.trim() === selected.alias} onClick={onRenameDevice}>{t.saveName}</button>
            </div>
          </div>
        </div>
        {historyOpened && (
          <div className="device-detail-content" role="tabpanel" hidden={tab !== "history"}>
            {history}
          </div>
        )}
        {commandOpened && (
          <div className="device-detail-content" role="tabpanel" hidden={tab !== "command"}>
            {command}
          </div>
        )}
      </>}
    </section>
  );
}
