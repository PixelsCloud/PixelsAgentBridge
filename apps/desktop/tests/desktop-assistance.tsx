import { createRoot } from "react-dom/client";
import { ConfigProvider } from "antd";
import { DesktopAssistanceNotice } from "../src/DesktopAssistanceNotice";
const fixture = (window as any).fixture = {
  calls: [] as any[],
  requests: [{ assistance_id: "first", status: "waiting", request: { device_code: "123456789", reason: "Generated UI needs help", expected_action: "Inspect the generated dialog" } }],
  deferred: {} as Record<string, { resolve: () => void; reject: (message: string) => void }>,
  hold: [] as string[],
};
(window as any).__TAURI_INTERNALS__ = {
  invoke: async (command: string, args: unknown) => {
    fixture.calls.push({ command, args });
    if (command === "desktop_assistance_pending") return [...fixture.requests];
    const key = `${command}:${(args as { id?: string })?.id}`;
    if (fixture.hold.includes(key)) {
      return new Promise<void>((resolve, reject) => { fixture.deferred[key] = { resolve, reject }; });
    }
    if (command === "desktop_assistance_cancel") fixture.requests = [];
    if (command === "desktop_assistance_open") fixture.requests = fixture.requests.map((row: any) => ({ ...row, status: "active" }));
    return null;
  },
};
createRoot(document.getElementById("root")!).render(<ConfigProvider><DesktopAssistanceNotice language="en" /></ConfigProvider>);
