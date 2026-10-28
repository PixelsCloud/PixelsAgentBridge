import type { NativePath } from "./fileManagerApi";

export type DirectoryTransfer = {
  spec: { id: string; code: string | number; direction: "upload" | "download"; destination: NativePath };
  completed: string;
  phase: string;
};

// These comparisons only invalidate a view. They never authorize a path or an
// operation. Ambiguous aliases may cause an extra scan under the pane's own
// authenticated execution context; displayed/escaped labels are never compared.
function nativeUnits(path: NativePath, style: "windows" | "posix"): string | null {
  try {
    if (style === "windows") {
      if (path.encoding === "unix_bytes") return null;
      let value = path.value;
      if (path.encoding === "windows_wide") {
        const bytes = Uint8Array.from(atob(value), char => char.charCodeAt(0));
        if (bytes.length % 2) return null;
        value = "";
        for (let i = 0; i < bytes.length; i += 2) value += String.fromCharCode(bytes[i] | bytes[i + 1] << 8);
      }
      return value.replace(/\//g, "\\").toUpperCase();
    }
    if (path.encoding === "windows_wide") return null;
    const bytes = path.encoding === "unix_bytes"
      ? Uint8Array.from(atob(path.value), char => char.charCodeAt(0))
      : new TextEncoder().encode(path.value);
    let value = "";
    for (const byte of bytes) value += String.fromCharCode(byte);
    return value;
  } catch { return null; }
}

export function directoryAffected(current: NativePath, destination: NativePath, style: "windows" | "posix"): boolean {
  const currentUnits = nativeUnits(current, style), destinationUnits = nativeUnits(destination, style);
  if (currentUnits === null || destinationUnits === null) return true;
  const separator = style === "windows" ? "\\" : "/";
  const parts = (value: string) => value.split(separator).filter(Boolean);
  if ([...parts(currentUnits), ...parts(destinationUnits)].some(part => part === "." || part === "..")) return true;
  // Relative paths or Win32/verbatim aliases cannot be resolved by the UI.
  if (style === "windows") {
    if (![currentUnits, destinationUnits].every(value => /^[A-Z]:\\|^\\\\/.test(value)) || [currentUnits, destinationUnits].some(value => value.startsWith("\\\\?\\") || value.startsWith("\\\\.\\"))) return true;
  } else if (!currentUnits.startsWith("/") || !destinationUnits.startsWith("/")) return true;
  const trim = (value: string) => { while (value.endsWith(separator)) value = value.slice(0, -1); return value; };
  const a = trim(currentUnits), b = trim(destinationUnits);
  return a === b || a.startsWith(b + separator) || b.startsWith(a + separator);
}

const finalPhases = new Set(["completed", "completed_with_skips", "partial_failed", "failed", "cancelled"]);
export function directoryChangeKey(batches: readonly DirectoryTransfer[], endpoint: string, path: NativePath | null, style: "windows" | "posix"): string {
  if (!path) return "[]";
  return JSON.stringify(batches.filter(batch =>
    batch.spec.destination && (endpoint === "local" ? batch.spec.direction === "download" : batch.spec.direction === "upload" && String(batch.spec.code) === endpoint)
    && (BigInt(batch.completed) > 0n || finalPhases.has(batch.phase))
    && directoryAffected(path, batch.spec.destination, style)
  ).map(batch => [batch.spec.id, batch.completed, finalPhases.has(batch.phase) ? batch.phase : ""]).sort((a, b) => a[0].localeCompare(b[0])));
}
