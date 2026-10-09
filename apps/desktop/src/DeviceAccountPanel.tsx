import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Alert, Button, Space, Typography } from "antd";
import { messages, type Language } from "./i18n";

type Status = { account_revision: number; pending: boolean; device?: { status: "associated" | "available" | "unlinked" | "other_account"; revision: number } };

export function DeviceAccountPanel({ language, accountRevision }: { language: Language; accountRevision: number }) {
  const t = messages[language];
  const [status, setStatus] = useState<Status>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  useEffect(() => {
    let alive = true;
    setStatus(undefined); setError(false);
    const refresh = () => { void invoke<Status>("device_account_status").then(value => { if (alive && value.account_revision === accountRevision) setStatus(value); }).catch(() => { if (alive) setError(true); }); };
    refresh(); const timer = setInterval(refresh, 3000);
    return () => { alive = false; clearInterval(timer); };
  }, [accountRevision]);
  async function associate() {
    if (!status?.device || busy) return;
    setBusy(true); setError(false);
    try {
      const result = await invoke<Status>("device_account_associate", { action: status.device.status === "unlinked" ? "associate" : "automatic", revision: status.device.revision, accountRevision });
      if (result.account_revision === accountRevision) setStatus(result);
    } catch { setError(true); } finally { setBusy(false); }
  }
  const current = status?.device?.status;
  return <Space orientation="vertical" style={{ width: "100%", marginTop: 16 }}>
    <Typography.Text strong>{t.deviceAccountTitle}</Typography.Text>
    <Typography.Text type="secondary">{current === "associated" ? t.deviceAccountAssociated : current === "unlinked" ? t.deviceAccountUnlinked : t.deviceAccountPending}</Typography.Text>
    {status?.device && current !== "associated" && <Button loading={busy} onClick={() => void associate()}>{t.deviceAccountAssociate}</Button>}
    {error && <Alert type="warning" title={t.deviceAccountRetry} showIcon />}
  </Space>;
}
