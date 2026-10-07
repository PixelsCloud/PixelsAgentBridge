import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Select } from "antd";
import { messages, type Language } from "./i18n";
import type { TaskEntry, TaskUpdate } from "./operatorTypes";

type Encoding = "utf8" | "gbk" | "gb18030" | "big5" | "utf16_le" | "utf16_be";
const options: { value: Encoding; label: string }[] = [
  { value: "utf8", label: "UTF-8" }, { value: "gbk", label: "GBK / CP936" },
  { value: "gb18030", label: "GB18030" }, { value: "big5", label: "Big5 / CP950" },
  { value: "utf16_le", label: "UTF-16 LE" }, { value: "utf16_be", label: "UTF-16 BE" },
];

// This view only re-reads bytes of the selected task. Its generation prevents an
// older encoding/task response from being appended after the user changes it.
export function CommandOutput({ task, language }: { task: TaskEntry; language: Language }) {
  const t = messages[language];
  const [encoding, setEncoding] = useState<Encoding>("utf8");
  const [output, setOutput] = useState<TaskUpdate>(task);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const generation = useRef(0);
  const current = useRef<TaskUpdate>(task);
  const active = useRef(false);

  async function read(reset: boolean, version: number, selected: Encoding) {
    if (active.current && !reset) return;
    active.current = true;
    setBusy(true);
    const previous = current.current;
    try {
      const update = await invoke<TaskUpdate>("operator_task", {
        taskId: task.id, encoding: selected,
        stdoutOffset: reset ? 0 : previous.stdoutOffset,
        stderrOffset: reset ? 0 : previous.stderrOffset,
      });
      if (generation.current !== version) return;
      const next = { ...update,
        stdout: (reset ? "" : previous.stdout) + update.stdout,
        stderr: (reset ? "" : previous.stderr) + update.stderr,
        decodingReplacements: update.decodingReplacements || (!reset && previous.decodingReplacements),
        outputGap: update.outputGap || (!reset && previous.outputGap),
      };
      current.current = next;
      setOutput(next);
      setError("");
    } catch (cause) {
      if (generation.current === version) setError(`${t.taskRefreshFailed}: ${String(cause)}`);
    } finally {
      if (generation.current === version) { active.current = false; setBusy(false); }
    }
  }

  useEffect(() => {
    const version = ++generation.current;
    const blank = { ...task, stdout: "", stderr: "", stdoutOffset: 0, stderrOffset: 0,
      complete: false, decodingReplacements: false, outputGap: false };
    current.current = blank;
    setOutput(blank);
    void read(true, version, encoding);
    return () => { ++generation.current; active.current = false; };
  }, [task.id, encoding]);

  useEffect(() => {
    if (!current.current.complete) void read(false, generation.current, encoding);
  }, [task.state, task.stdoutOffset, task.stderrOffset]);

  return <>
    <div className="command-audit"><span>{t.outputEncoding}</span>
      <Select aria-label={t.outputEncoding} value={encoding} options={options} style={{ minWidth: 165 }}
        onChange={setEncoding} />
    </div>
    {output.decodingReplacements && <p className="inline-error">{t.outputEncodingInvalid}</p>}
    {output.outputGap && <p className="inline-error">{t.outputRetainedGap}</p>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    <pre>{output.stdout || (!output.stderr && t.waitingOutput)}</pre>
    {output.stderr && <pre className="stderr-output">{output.stderr}</pre>}
    {(!output.complete || error) && <Button type="text" loading={busy}
      onClick={() => void read(false, generation.current, encoding)}>{t.loadMoreOutput}</Button>}
  </>;
}
