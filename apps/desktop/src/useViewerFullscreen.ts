import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** Synchronize the web chrome and native video inset, including native-focus Esc. */
export function useViewerFullscreen(onError: (error: string) => void) {
  const [appWindow] = useState(getCurrentWindow);
  const [fullscreen, updateFullscreen] = useState(false);
  const actual = useRef<boolean | null>(null);
  const pending = useRef(false);
  const queued = useRef<boolean | null>(null);
  const nativeRevision = useRef(0);

  async function setFullscreen(value: boolean) {
    if (pending.current) { queued.current = value; return; }
    pending.current = true;
    const revision = nativeRevision.current;
    try {
      const enabled = await invoke<boolean>("desktop_viewer_fullscreen", { fullscreen: value });
      // A native-focus Esc may already have superseded this command's reply.
      if (nativeRevision.current === revision || actual.current === enabled) {
        actual.current = enabled;
        updateFullscreen(enabled);
      }
    } catch (error) { onError(String(error)); }
    finally {
      pending.current = false;
      const next = queued.current;
      queued.current = null;
      if (next !== null) void setFullscreen(next);
    }
  }

  useEffect(() => {
    let active = true;
    const subscriptions: Promise<() => void>[] = [];
    const sync = async () => {
      try {
        const enabled = await appWindow.isFullscreen();
        if (active && actual.current !== enabled) await setFullscreen(enabled);
      } catch (error) { if (active) onError(String(error)); }
    };
    subscriptions.push(listen<boolean>("desktop-viewer-fullscreen", ({ payload }) => {
      if (active) { nativeRevision.current++; actual.current = payload; updateFullscreen(payload); }
    }));
    subscriptions.push(appWindow.onResized(() => void sync()));
    void sync();
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !actual.current) return;
      event.preventDefault(); event.stopImmediatePropagation();
      void setFullscreen(false);
    };
    window.addEventListener("keydown", escape, true);
    return () => {
      active = false;
      window.removeEventListener("keydown", escape, true);
      for (const subscription of subscriptions) void subscription.then(unlisten => unlisten()).catch(() => {});
    };
  }, [appWindow, onError]);

  return { appWindow, fullscreen, toggleFullscreen: () => setFullscreen(!actual.current) };
}
