import { invoke } from "@tauri-apps/api/core";
import type { ExecutionSelection } from "./executionQueries";

export type NativePath = { encoding: "utf8" | "unix_bytes" | "windows_wide"; value: string };
export type FileEntry = { id: string; name: string; path: NativePath; kind: "file" | "directory" | "symlink" | "other"; size: string | null; modified_at_unix_ms: number | null; hidden: boolean; readonly: boolean; unix_mode: number | null; error: string | null };
export type FilePage = { snapshot_id: string; path: NativePath; entries: FileEntry[]; next_after: string | null; total_entries: string; complete: boolean; observed_at_unix_ms: number };
export type FileLocations = { path_style: "windows" | "posix"; home: NativePath | null; roots: { name: string; path: NativePath }[] };
export type FileResult = { result: "locations"; locations: FileLocations } | { result: "opened"; snapshot_id: string } | { result: "directory"; page: FilePage } | { result: "entry"; entry: FileEntry } | { result: "applied"; path: NativePath } | { result: "closed"; snapshot_id: string } | { result: "error"; error: { code: string; phase: string; message: string } };
export type ManagerReply = { request_id: string; result: FileResult; execution_context: { identity?: { account_name: string }; path_style: "windows" | "posix" } | null };
export type FileFrame = { request_id: string; state: string; reply: ManagerReply | null; message: string | null };
export class FileRequestError extends Error {
  constructor(message: string, public requestId: string, public unconfirmed: boolean) { super(message); this.name = "FileRequestError"; }
}
// Native task errors can arrive as an IPC error string before a FileFrame.
// Only read callers use this hint. Mutations keep their original request ID.
export const fileExecutionChanged = (cause: unknown): boolean => cause instanceof FileRequestError && /\bEnvironmentChanged\b|\[(?:environment_changed|execution_context_changed)\]/.test(cause.message);
export async function observeFileRequest(requestId: string): Promise<FileFrame> { return invoke("file_manager_status", { requestId }); }
export type FileOperation = { action: "locations" } | { action: "open_directory" | "inspect"; path: NativePath } | { action: "read_directory"; snapshot_id: string; after: string | null; limit: number; filter: FileFilter } | { action: "close_directory"; snapshot_id: string } | { action: "create_directory"; parent: NativePath; name: NativePath } | { action: "rename"; path: NativePath; name: NativePath } | { action: "delete"; path: NativePath; recursive: boolean };
export type FileFilter = { sort: "name" | "size" | "modified"; descending: boolean; show_hidden: boolean; query: string };
export const defaultFilter: FileFilter = { sort: "name", descending: false, show_hidden: false, query: "" };
export const utf8Path = (value: string): NativePath => ({ encoding: "utf8", value });
export const nativePathKey = (path: NativePath) => `${path.encoding}:${path.value}`;
export const displayFileText = (value: string) => value.replace(/[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/g, character => `\\u{${character.codePointAt(0)!.toString(16)}}`);
// Display-only: operations and identities always retain the original native path.
const readablePath = (value: string) => displayFileText(value.replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, ""));
export const pathLabel = (path: NativePath): string => {
  if (path.encoding === "utf8") return readablePath(path.value);
  try {
    const bytes = Uint8Array.from(atob(path.value), c => c.charCodeAt(0));
    if (path.encoding === "unix_bytes") return [...bytes].map(v => v >= 33 && v <= 126 && v !== 92 ? String.fromCharCode(v) : `\\x${v.toString(16).padStart(2, "0")}`).join("");
    const units: number[] = [];
    for (let i = 0; i + 1 < bytes.length; i += 2) units.push(bytes[i] | (bytes[i + 1] << 8));
    let result = "";
    for (let i = 0; i < units.length; i++) {
      const unit = units[i];
      if (unit >= 0xd800 && unit <= 0xdbff && units[i + 1] >= 0xdc00 && units[i + 1] <= 0xdfff) result += String.fromCharCode(unit, units[++i]);
      else result += unit >= 0xd800 && unit <= 0xdfff ? `\\u{${unit.toString(16)}}` : String.fromCharCode(unit);
    }
    return readablePath(result);
  } catch { return "[invalid native path]"; }
};
// BigInt keeps sizes >2^53 exact until formatting. Path labels are display-only.
export function fileSize(value: string | null): string {
  if (value === null) return "—";
  const bytes = BigInt(value), units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
  let scale = 1n, unit = 0;
  while (bytes >= scale * 1024n && unit < units.length - 1) { scale *= 1024n; unit++; }
  if (unit === 0) return `${bytes} B`;
  const hundredths = (bytes * 100n + scale / 2n) / scale;
  return `${hundredths / 100n}.${(hundredths % 100n).toString().padStart(2, "0")} ${units[unit]}`;
}
export async function fileRequest(endpoint: string, execution: ExecutionSelection, operation: FileOperation, signal?: AbortSignal, forceRelay = false): Promise<ManagerReply> {
  const requestId = crypto.randomUUID();
  let accepted = false, cancelled = false;
  const cancel = () => {
    if (accepted && !cancelled && operation.action === "open_directory") {
      cancelled = true;
      void invoke("file_manager_cancel", { requestId }).catch(() => undefined);
    }
  };
  signal?.addEventListener("abort", cancel, { once: true });
  try {
    let frame: FileFrame;
    try { frame = await invoke<FileFrame>("file_manager_request", { endpoint, requestId, operation, execution, forceRelay }); }
    catch (cause) { throw new FileRequestError(`${String(cause)} · ${requestId}`, requestId, true); }
    accepted = true;
    if (signal?.aborted) cancel();
    while (["running", "cancel_requested"].includes(frame.state)) {
      await new Promise(resolve => setTimeout(resolve, 200));
      try { frame = await observeFileRequest(requestId); }
      catch (cause) { throw new FileRequestError(`${String(cause)} · ${requestId}`, requestId, true); }
    }
    if (!frame.reply) throw new FileRequestError(`${frame.message || frame.state} · ${requestId}`, requestId, true);
    if (frame.reply.result.result === "error") throw new FileRequestError(`${frame.reply.result.error.message} [${frame.reply.result.error.code}] · ${requestId}`, requestId, frame.reply.result.error.code === "unconfirmed");
    if (signal?.aborted) {
      if (frame.reply.result.result === "opened") {
        await fileRequest(endpoint, execution, { action: "close_directory", snapshot_id: frame.reply.result.snapshot_id }, undefined, forceRelay).catch(() => undefined);
      }
      throw new DOMException("Directory navigation changed", "AbortError");
    }
    return frame.reply;
  } finally { signal?.removeEventListener("abort", cancel); }
}
