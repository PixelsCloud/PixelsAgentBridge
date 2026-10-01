import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Camera, X } from "lucide-react";
import { Button } from "antd";
import { messages, type Language } from "./i18n";

type Screenshot = {
  requestId: string;
  width: number;
  height: number;
  size: number;
  sha256: string;
  format: string;
  dataUrl: string;
};

type DesktopInputEvent =
  | { type: "mouse_move"; x: number; y: number }
  | { type: "mouse_button"; button: "left" | "right" | "middle"; down: boolean }
  | { type: "mouse_wheel"; delta: number }
  | { type: "key"; virtual_key: number; down: boolean }
  | { type: "secure_attention" };

export function ScreenshotBrowser({ code, connected, language, onAuditChange }: {
  code: string;
  connected: boolean;
  language: Language;
  onAuditChange: () => void;
}) {
  const t = messages[language];
  const [image, setImage] = useState<Screenshot | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [expanded, setExpanded] = useState(false);
  const [live, setLive] = useState(false);
  const [liveImage, setLiveImage] = useState<Screenshot | null>(null);
  const lastMove = useRef(0);
  const inputQueue = useRef<Promise<void>>(Promise.resolve());
  const pressedButtons = useRef(new Set<"left" | "right" | "middle">());
  const pressedKeys = useRef(new Set<number>());

  function sendInput(event: DesktopInputEvent): Promise<void> {
    const targetCode = code;
    const pending = inputQueue.current.then(() =>
      invoke<void>("operator_desktop_input", { code: targetCode, event }),
    );
    inputQueue.current = pending.catch((cause) => {
      setError(String(cause));
    });
    return inputQueue.current;
  }

  function releaseHeldInputs() {
    for (const button of pressedButtons.current) {
      void sendInput({ type: "mouse_button", button, down: false });
    }
    pressedButtons.current.clear();
    for (const virtualKey of pressedKeys.current) {
      void sendInput({ type: "key", virtual_key: virtualKey, down: false });
    }
    pressedKeys.current.clear();
  }

  useEffect(() => {
    setImage(null);
    setError("");
    setExpanded(false);
    setLive(false);
    setLiveImage(null);
  }, [code]);

  useEffect(() => {
    if (!connected) setLive(false);
  }, [connected]);

  useEffect(() => {
    if (!live || !connected) releaseHeldInputs();
  }, [live, connected]);

  useEffect(() => () => releaseHeldInputs(), [code]);

  useEffect(() => {
    if (!live || !connected) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const next = await invoke<Screenshot>("operator_preview_desktop", { code });
        if (!stopped) {
          setLiveImage(next);
          setError("");
        }
      } catch (cause) {
        if (!stopped) setError(String(cause));
      }
      if (!stopped) timer = setTimeout(() => void refresh(), 400);
    }
    void refresh();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [code, connected, live]);

  function movePointer(element: HTMLImageElement, clientX: number, clientY: number) {
    const rect = element.getBoundingClientRect();
    const x = Math.round(Math.max(0, Math.min(1, (clientX - rect.left) / rect.width)) * 65535);
    const y = Math.round(Math.max(0, Math.min(1, (clientY - rect.top) / rect.height)) * 65535);
    return sendInput({ type: "mouse_move", x, y });
  }

  function buttonName(button: number): "left" | "right" | "middle" | null {
    return button === 0 ? "left" : button === 1 ? "middle" : button === 2 ? "right" : null;
  }

  async function capture() {
    if (!connected || loading) return;
    setLoading(true);
    setError("");
    try {
      setImage(await invoke<Screenshot>("operator_capture_screenshot", { code }));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
      onAuditChange();
    }
  }

  return <div className="screenshot-browser">
    <p className="form-hint">{t.screenshotHint}</p>
    <Button type="primary" loading={loading} disabled={!connected} onClick={() => void capture()}>
      {loading ? t.loadingHistory : t.captureScreenshot}<Camera size={17} />
    </Button>
    <Button disabled={!connected} onClick={() => {
      setLive(!live);
      if (live) onAuditChange();
    }}>{live ? t.stopRemoteControl : t.startRemoteControl}</Button>
    {live && <Button disabled={!connected} onClick={() => {
      void sendInput({ type: "secure_attention" }).finally(onAuditChange);
    }}>{t.sendSecureAttention}</Button>}
    {error && <div className="inline-error" role="alert">{error}</div>}
    {live && <p className="form-hint">{t.remoteControlHint}</p>}
    {(live ? liveImage : image) && <>
      <div className="screenshot-meta">{(live ? liveImage : image)!.width} × {(live ? liveImage : image)!.height} · {(live ? liveImage : image)!.format.toUpperCase()}</div>
      {live && liveImage ? <div className="screenshot-preview remote-desktop-preview">
        <img
          src={liveImage.dataUrl}
          alt={t.screenshot}
          tabIndex={0}
          onContextMenu={(event) => event.preventDefault()}
          onPointerMove={(event) => {
            if (Date.now() - lastMove.current < 80) return;
            lastMove.current = Date.now();
            void movePointer(event.currentTarget, event.clientX, event.clientY);
          }}
          onPointerDown={(event) => {
            const button = buttonName(event.button);
            if (!button) return;
            event.currentTarget.focus();
            event.currentTarget.setPointerCapture(event.pointerId);
            pressedButtons.current.add(button);
            void movePointer(event.currentTarget, event.clientX, event.clientY);
            void sendInput({ type: "mouse_button", button, down: true });
          }}
          onPointerUp={(event) => {
            const button = buttonName(event.button);
            if (button && pressedButtons.current.delete(button)) {
              void sendInput({ type: "mouse_button", button, down: false });
            }
            if (event.currentTarget.hasPointerCapture(event.pointerId)) {
              event.currentTarget.releasePointerCapture(event.pointerId);
            }
          }}
          onPointerCancel={() => releaseHeldInputs()}
          onBlur={() => releaseHeldInputs()}
          onWheel={(event) => {
            void sendInput({ type: "mouse_wheel", delta: Math.max(-1200, Math.min(1200, Math.round(-event.deltaY))) });
          }}
          onKeyDown={(event) => {
            if (!event.keyCode) return;
            event.preventDefault();
            if (pressedKeys.current.has(event.keyCode)) return;
            pressedKeys.current.add(event.keyCode);
            void sendInput({ type: "key", virtual_key: event.keyCode, down: true });
          }}
          onKeyUp={(event) => {
            if (!event.keyCode) return;
            event.preventDefault();
            if (pressedKeys.current.delete(event.keyCode)) {
              void sendInput({ type: "key", virtual_key: event.keyCode, down: false });
            }
          }}
        />
      </div> : image && <>
      <button className="screenshot-preview" onClick={() => setExpanded(true)} title={t.expandScreenshot}>
        <img src={image.dataUrl} alt={t.screenshot} />
      </button>
      </>}
    </>}
    {expanded && image && <div className="screenshot-lightbox" role="dialog" aria-label={t.screenshot}>
      <button className="screenshot-close" onClick={() => setExpanded(false)}>{t.closeScreenshot} <X size={16} /></button>
      <img src={image.dataUrl} alt={t.screenshot} />
    </div>}
  </div>;
}
