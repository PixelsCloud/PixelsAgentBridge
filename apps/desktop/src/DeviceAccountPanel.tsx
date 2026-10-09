import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Alert, Button, Modal, Space, Typography } from "antd";
import { messages, type Language } from "./i18n";

type Status = { account_revision: number; pending: boolean; device?: { status: "associated" | "available" | "unlinked" | "other_account"; revision: number } };

export function DeviceAccountPanel({ language, accountRevision }: { language: Language; accountRevision: number }) {
  const t = messages[language];
  const [status, setStatus] = useState<Status>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const [confirm, setConfirm] = useState(false);
  useEffect(() => {
    let alive = true;
    setStatus(undefined); setConfirm(false); setError(false);
    const refresh = () => { void invoke<Status>("device_account_status").then(value => { if (alive && value.account_revision === accountRevision) setStatus(value); }).catch(() => { if (alive) setError(true); }); };
    refresh(); const timer = setInterval(refresh, 3000);
    return () => { alive = false; clearInterval(timer); };
  }, [accountRevision]);
  async function associate() {
    if (!status?.device || busy) return;
    setBusy(true); setError(false);
    try {
      const result = await invoke<Status>("device_account_associate", { action: status.device.status === "other_account" ? "replace" : "associate", revision: status.device.revision, accountRevision });
      if (result.account_revision === accountRevision) setStatus(result);
      setConfirm(false);
    } catch { setError(true); } finally { setBusy(false); }
  }
  const current = status?.device?.status;
  return <Space orientation="vertical" style={{ width: "100%", marginTop: 16 }}>
    <Typography.Text strong>{t.deviceAccountTitle}</Typography.Text>
    <Typography.Text type="secondary">{current === "associated" ? t.deviceAccountAssociated : current === "other_account" ? t.deviceAccountOther : current === "unlinked" ? t.deviceAccountUnlinked : t.deviceAccountPending}</Typography.Text>
    {status?.device && current !== "associated" && <Button loading={busy} onClick={() => current === "other_account" ? setConfirm(true) : void associate()}>{current === "other_account" ? t.deviceAccountReplace : t.deviceAccountAssociate}</Button>}
    {error && <Alert type="warning" title={t.deviceAccountRetry} showIcon />}
    <Modal title={t.deviceAccountReplace} open={confirm} mask={{ closable: false }} keyboard={false} closable={!busy} onCancel={() => !busy && setConfirm(false)} onOk={() => void associate()} confirmLoading={busy}>
      <Typography.Paragraph>{t.deviceAccountReplaceHint}</Typography.Paragraph>
    </Modal>
  </Space>;
}
