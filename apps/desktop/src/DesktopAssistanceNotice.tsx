import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Modal } from "antd";
import type { Language } from "./i18n";
import { formatDeviceCode } from "./deviceCode";

export type Assistance = {
  assistance_id: string;
  status: "waiting" | "active" | "completed" | "cancelled" | "expired";
  request: { device_code: string; reason: string; expected_action: string; screen_id?: string };
};
export function DesktopAssistanceNotice({ language }: { language: Language }) {
  const [requests, setRequests] = useState<Assistance[]>([]);
  useEffect(() => {
    let active = true;
    let pending = false;
    const refresh = async () => {
      if (pending) return;
      pending = true;
      try { const rows = await invoke<Assistance[]>("desktop_assistance_pending"); if (active) setRequests(rows); }
      catch { if (active) setRequests([]); }
      finally { pending = false; }
    };
    void refresh(); const timer = window.setInterval(() => void refresh(), 1000);
    return () => { active = false; window.clearInterval(timer); };
  }, []);
  const current = requests.find((request) => request.status === "waiting");
  // Each request owns its asynchronous action state, including late IPC failures.
  return current ? <AssistancePrompt key={current.assistance_id} current={current} language={language} /> : null;
}

function AssistancePrompt({ current, language }: { current: Assistance; language: Language }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [opened, setOpened] = useState(false);
  const [deliveryFailed, setDeliveryFailed] = useState(false);
  useEffect(() => {
    if (!opened) return;
    let active = true;
    let pending = false;
    let acknowledged = false;
    const acknowledge = async () => {
      if (!active || pending || acknowledged || document.hidden) return;
      pending = true;
      try {
        const displayed = await invoke<boolean>("desktop_assistance_displayed", { id: current.assistance_id });
        if (active) {
          acknowledged = displayed;
          setDeliveryFailed(false);
        }
      } catch { if (active) setDeliveryFailed(true); }
      finally { pending = false; }
    };
    void acknowledge();
    const timer = window.setInterval(() => void acknowledge(), 1000);
    document.addEventListener("visibilitychange", acknowledge);
    return () => {
      active = false;
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", acknowledge);
    };
  }, [opened, current.assistance_id]);
  const t = language === "en"
    ? { title: "AI requests your help", reason: "Reason", action: "Please help with", open: "Open remote desktop", cancel: "Decline", hint: "The window opens in view-only mode. When finished, choose Complete assistance in that window.", failed: "Could not update the assistance request." }
    : language === "zh-TW"
      ? { title: "AI 請求您協助", reason: "原因", action: "需要協助的操作", open: "打開遠端桌面", cancel: "取消協助", hint: "打開後預設僅查看。處理完畢，請在遠端桌面窗口點擊「完成協助」。", failed: "無法更新協助請求。" }
      : { title: "AI 请求您协助", reason: "原因", action: "需要协助的操作", open: "打开远程桌面", cancel: "取消协助", hint: "打开后默认仅查看。处理完毕，请在远程桌面窗口点击“完成协助”。", failed: "无法更新协助请求。" };
  async function act(command: string) {
    setBusy(true); setError("");
    try { await invoke(command, { id: current.assistance_id }); }
    catch (cause) { setError(String(cause) || t.failed); }
    finally { setBusy(false); }
  }
  return <Modal open title={t.title}
    closable={false} mask={{ closable: false }} keyboard={false}
    afterOpenChange={setOpened}
    footer={<><Button disabled={busy} onClick={() => void act("desktop_assistance_cancel")}>{t.cancel}</Button><Button type="primary" loading={busy} onClick={() => void act("desktop_assistance_open")}>{t.open}</Button></>}>
    <p>{formatDeviceCode(current.request.device_code)}</p>
    <p><strong>{t.reason}</strong></p><p style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{current.request.reason}</p>
    <p><strong>{t.action}</strong></p><p style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere" }}>{current.request.expected_action}</p>
    <p>{t.hint}</p>{(error || deliveryFailed) && <p role="alert">{error || t.failed}</p>}
  </Modal>;
}
