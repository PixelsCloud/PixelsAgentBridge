import { useEffect } from "react";
import { App, ConfigProvider, theme } from "antd";
import { invoke } from "@tauri-apps/api/core";
import "./confirmationWindow.css";

type Question = { title: string; body: string; confirm: string; cancel: string };
const dark = localStorage.getItem("pab.theme") === "dark";

export default function ConfirmationWindow() {
  return <ConfigProvider theme={{ algorithm: dark ? theme.darkAlgorithm : theme.defaultAlgorithm, token: { colorPrimary: "#218574", fontSize: 13, borderRadius: 7 } }}><App><Confirmation /></App></ConfigProvider>;
}

function Confirmation() {
  const { modal } = App.useApp();
  useEffect(() => {
    document.documentElement.dataset.theme = dark ? "dark" : "light";
    let active = true;
    let answering = false;
    let frame = 0;
    let dialog: ReturnType<typeof modal.confirm> | undefined;
    const answer = async (accepted: boolean) => {
      if (!active || answering) return;
      answering = true;
      try { await invoke("confirmation_answer", { accepted }); }
      catch { answering = false; throw new Error("Confirmation is closed"); }
    };
    void invoke<Question>("confirmation_info").then(question => {
      if (!active) return;
      dialog = modal.confirm({
        title: question.title, content: question.body,
        okText: question.confirm, cancelText: question.cancel,
        okButtonProps: { danger: true }, focusable: { autoFocusButton: "cancel" },
        mask: false, keyboard: true, closable: false, width: "100%",
        className: "app-confirmation", style: { top: 0, margin: 0, paddingBottom: 0 },
        onOk: () => answer(true), onCancel: () => answer(false),
      });
      const ready = () => {
        if (!active) return;
        const content = document.querySelector(".app-confirmation .ant-modal-container, .app-confirmation .ant-modal-content");
        if (!content) { frame = requestAnimationFrame(ready); return; }
        void invoke("confirmation_ready", { height: Math.ceil(content.getBoundingClientRect().height) }).catch(() => { dialog?.destroy(); });
      };
      frame = requestAnimationFrame(ready);
    }).catch(() => { /* The host cancels a dialog that cannot load. */ });
    return () => { active = false; cancelAnimationFrame(frame); dialog?.destroy(); };
  }, [modal]);
  return null;
}
