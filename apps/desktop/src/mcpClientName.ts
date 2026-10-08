// Keep the reported identity in details; only normalize known product names.
export function mcpClientName(name: string | null, unknown: string): string {
  const raw = name?.trim();
  if (!raw) return unknown;
  const aliases: Record<string, string> = {
    codex: "Codex", "codex-cli": "Codex", codex_cli_rs: "Codex", "codex-mcp-client": "Codex",
    "kimi-code": "Kimi Code", "kimi-cli": "Kimi Code", kimi: "Kimi Code",
    "claude-code": "Claude Code", "claude-cli": "Claude Code", claude: "Claude Code",
    "dsh-mcp-client": "DeepSeek Harness", "deepseek-harness": "DeepSeek Harness", dsh: "DeepSeek Harness",
    cursor: "Cursor", "cursor-agent": "Cursor", "cursor-vscode": "Cursor",
    opencode: "OpenCode", "opencode-ai": "OpenCode",
  };
  return aliases[raw.toLowerCase()] ?? raw;
}
