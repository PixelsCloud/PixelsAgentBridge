import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Terminal } from "@xterm/xterm";
import { Terminal as TerminalIcon } from "lucide-react";
import { Button } from "antd";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { messages, type Language } from "./i18n";

type Opened = { sessionId: string; shell: string; cols: number; rows: number };
type Output = { data: string; ended: boolean };

function decode(data: string): Uint8Array<ArrayBuffer> {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

export function TerminalBrowser({ code, connected, language, visible, onAuditChange }: {
  code: string;
  connected: boolean;
  language: Language;
  visible: boolean;
  onAuditChange: () => void;
}) {
  const t = messages[language];
  const host = useRef<HTMLDivElement>(null);
  const terminal = useRef<Terminal | null>(null);
  const fit = useRef<FitAddon | null>(null);
  const session = useRef<string | null>(null);
  const queue = useRef(Promise.resolve());
  const [opened, setOpened] = useState<Opened | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    if (!host.current) return;
    const instance = new Terminal({
      convertEol: true,
      cursorBlink: true,
      fontSize: 13,
      fontFamily: "Cascadia Mono, Consolas, monospace",
      theme: { background: "#142334", foreground: "#e7f3f1", cursor: "#8cd6c9" },
    });
    const addon = new FitAddon();
    instance.loadAddon(addon);
    instance.open(host.current);
    addon.fit();
    terminal.current = instance;
    fit.current = addon;
    const input = instance.onData((data) => {
      const id = session.current;
      if (!id) return;
      queue.current = queue.current.then(async () => {
        const bytes = new TextEncoder().encode(data);
        for (let offset = 0; offset < bytes.length; offset += 4096) {
          const chunk = bytes.slice(offset, offset + 4096);
          let binary = "";
          for (const byte of chunk) binary += String.fromCharCode(byte);
          await invoke("operator_terminal_input", { id, data: btoa(binary) });
        }
      }).catch((cause) => {
        setError(String(cause));
      });
    });
    const observer = new ResizeObserver(() => {
      if (!host.current || host.current.clientWidth === 0) return;
      addon.fit();
      const id = session.current;
      if (!id || instance.cols < 20 || instance.rows < 5) return;
      queue.current = queue.current.then(() => invoke<void>("operator_terminal_resize", {
        id,
        cols: instance.cols,
        rows: instance.rows,
      })).catch((cause) => setError(String(cause)));
    });
    observer.observe(host.current);
    return () => {
      observer.disconnect();
      input.dispose();
      instance.dispose();
      terminal.current = null;
      fit.current = null;
      const id = session.current;
      session.current = null;
      if (id) void invoke("operator_terminal_close", { id }).catch(() => undefined);
    };
  }, [code]);

  useEffect(() => {
    setOpened(null);
  }, [code]);

  useEffect(() => {
    if (visible) fit.current?.fit();
  }, [visible]);

  useEffect(() => {
    const id = opened?.sessionId;
    if (!id) return;
    let cancelled = false;
    async function poll() {
      while (!cancelled && session.current === id) {
        try {
          const output = await invoke<Output>("operator_terminal_read", { id });
          if (cancelled) return;
          setError("");
          if (output.data) terminal.current?.write(decode(output.data));
          if (output.ended) {
            session.current = null;
            onAuditChange();
            return;
          }
        } catch (cause) {
          if (cancelled) return;
          setError(String(cause));
          await new Promise((resolve) => window.setTimeout(resolve, 3000));
          continue;
        }
        await new Promise((resolve) => window.setTimeout(resolve, 200));
      }
    }
    void poll();
    return () => { cancelled = true; };
  }, [opened?.sessionId]);

  async function start() {
    if (!connected || busy || session.current) return;
    setBusy(true);
    setError("");
    terminal.current?.clear();
    try {
      fit.current?.fit();
      const result = await invoke<Opened>("operator_open_terminal", {
        code,
        cols: Math.max(20, terminal.current?.cols ?? 80),
        rows: Math.max(5, terminal.current?.rows ?? 24),
      });
      session.current = result.sessionId;
      setOpened(result);
      terminal.current?.focus();
      onAuditChange();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function stop() {
    const id = session.current;
    if (!id) return;
    session.current = null;
    try {
      await queue.current;
      await invoke("operator_terminal_close", { id });
      onAuditChange();
    } catch (cause) {
      setError(String(cause));
    }
  }

  return <div className="terminal-browser" style={{ display: visible ? undefined : "none" }}>
    <p className="form-hint">{t.terminalHint}</p>
    <div className="terminal-actions">
      <Button type="primary" loading={busy} disabled={!connected || !!session.current} onClick={() => void start()}>{t.openTerminal}<TerminalIcon size={17} /></Button>
      <Button disabled={!session.current} onClick={() => void stop()}>{t.closeTerminal}</Button>
      {opened && <span>{opened.shell}</span>}
    </div>
    {error && <div className="inline-error" role="alert">{error}</div>}
    <div className="terminal-canvas" ref={host} />
  </div>;
}
