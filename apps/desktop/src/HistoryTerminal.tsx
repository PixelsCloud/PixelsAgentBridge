import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { messages, type Language } from "./i18n";

type Event = {
  sequence: number;
  kind: string;
  data: string;
  state: string;
  atUnixMs: number;
};
type History = { output: string; events: Event[] };

export function HistoryTerminal({ id, offset, language }: {
  id: string;
  offset: number;
  language: Language;
}) {
  const t = messages[language];
  const host = useRef<HTMLDivElement>(null);
  const [history, setHistory] = useState<History | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    setHistory(null);
    setError("");
    void invoke<History | null>("operator_terminal_history", { id })
      .then(setHistory)
      .catch((cause) => setError(String(cause)));
  }, [id, offset]);

  useEffect(() => {
    if (!history || !host.current) return;
    const terminal = new Terminal({
      disableStdin: true,
      convertEol: true,
      fontSize: 12,
      scrollback: 20000,
      theme: { background: "#142334", foreground: "#e7f3f1" },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host.current);
    fit.fit();
    const binary = atob(history.output);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
    terminal.write(bytes);
    return () => terminal.dispose();
  }, [history]);

  if (error) return <div className="inline-error" role="alert">{error}</div>;
  if (!history) return <p className="form-hint">{t.terminalHistoryUnavailable}</p>;
  return <div className="terminal-history">
    <div className="terminal-history-canvas" ref={host} />
    <div className="terminal-event-list">
      {history.events.map((event) => <div key={event.sequence}>
        <time>{new Date(event.atUnixMs).toLocaleString(language)}</time>
        <strong>{event.kind}</strong>
        <span>{event.state}</span>
        {event.kind === "input" && <code>{new TextDecoder().decode(Uint8Array.from(atob(event.data), (char) => char.charCodeAt(0)))}</code>}
      </div>)}
    </div>
  </div>;
}
