import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
const params = new URLSearchParams(location.search);
localStorage.setItem("pab.theme", params.get("theme") || "light");
const english = params.get("language") === "en";
const fixture = (window as any).fixture = { calls: [] as any[], hold: false, release: undefined as (() => void) | undefined };
(window as any).__TAURI_INTERNALS__ = {
  invoke: async (command: string, args: any) => {
    fixture.calls.push({ command, args });
    if (command === "confirmation_info") return english
      ? { title: "Take over remote desktop?", body: "This device is being controlled by another session. Taking over will disconnect that session and release its held keys and mouse buttons.", confirm: "Take over", cancel: "Cancel" }
      : { title: "关闭远程桌面？", body: "将断开本次远程桌面连接。其他终端和设备连接将保持运行。", confirm: "确认关闭", cancel: "继续使用" };
    if (command === "confirmation_answer" && fixture.hold) await new Promise<void>(resolve => { fixture.release = resolve; });
    return null;
  },
};
const { default: ConfirmationWindow } = await import("../src/ConfirmationWindow");
createRoot(document.getElementById("root")!).render(<StrictMode><ConfirmationWindow /></StrictMode>);
