import { Alert, Badge, Collapse, Descriptions, Empty, Space, Table, Tabs, Tag, Typography } from "antd";
import { messages, type Language } from "./i18n";
import { formatDeviceCode } from "./deviceCode";
import { useMcpReporting, type ConnectedMcp, type DeviceRef, type ReportedTask, type ReportedOperation, type ToolCall } from "./mcpReporting";

export function McpConnectionsPanel({ language }: { language: Language }) {
  const t = messages[language];
  const { status, error } = useMcpReporting();
  const time = (value: number | null) => value ? new Date(value).toLocaleString(language) : "—";
  const stateText = (state: string) => {
    const states: Record<string, string> = {
      connected: t.phaseConnected, authenticated: t.serverConnected, disconnected: t.phaseDisconnected,
      connecting: t.phaseConnecting, waiting_for_bridge: t.phaseConnecting, retrying: t.phaseConnecting,
      reconnecting: t.phaseConnecting, initializing: t.loading, initialization_failed: t.connectFailed,
      resolved: t.mcpResolved, stopped: t.phaseDisconnected, pending: t.mcpPending, accepted: t.mcpPending,
      running: t.mcpBusy, cancel_requested: t.mcpCancelling, succeeded: t.mcpSucceeded, completed: t.mcpSucceeded,
      failed: t.mcpFailed, cancelled: t.mcpCancelled, interrupted: t.mcpInterrupted,
      unconfirmed: t.transferUnconfirmedShort, not_initialized: t.mcpNotStarted,
    };
    return states[state] ?? state;
  };
  const stateTag = (state: string) => <Tag color={
    ["connected", "authenticated", "succeeded", "completed"].includes(state) ? "success"
      : ["failed", "interrupted", "initialization_failed"].includes(state) ? "error"
        : state === "unconfirmed" ? "warning"
          : ["running", "connecting", "retrying", "reconnecting"].includes(state) ? "processing" : "default"
  }>{stateText(state)}</Tag>;

  const clientState = (client: ConnectedMcp) => {
    const { report } = client;
    const running = report.activeCalls.length > 0
      || report.runtime?.tasks.some(task => ["accepted", "running", "cancel_requested"].includes(task.state))
      || report.runtime?.operations.some(operation => ["running", "cancel_requested"].includes(operation.state) && !operation.finishedAtUnixMs);
    if (running) return "busy";
    if (report.runtime?.operations.some(operation => operation.state === "unconfirmed" && !operation.finishedAtUnixMs)) return "unconfirmed";
    return "idle";
  };
  const clientStatusTag = (client: ConnectedMcp) => {
    const state = clientState(client);
    return <Tag color={state === "busy" ? "processing" : state === "unconfirmed" ? "warning" : "default"}>{state === "busy" ? t.mcpBusy : state === "unconfirmed" ? t.transferUnconfirmedShort : t.mcpIdle}</Tag>;
  };

  function details(client: ConnectedMcp) {
    const report = client.report;
    const runtime = report.runtime;
    const deviceName = (ref: DeviceRef | null, code?: string | null) => {
      const device = runtime?.devices.find(device => code ? device.deviceCode === code
        : device.deviceRef.device_id === ref?.device_id && device.deviceRef.deployment_id === ref?.deployment_id && device.deviceRef.tenant_id === ref?.tenant_id);
      const foundCode = code ?? device?.deviceCode;
      const name = device?.alias || device?.name;
      return foundCode ? `${formatDeviceCode(foundCode)}${name ? ` · ${name}` : ""}` : t.unknown;
    };
    const callColumns = [
      { title: t.mcpTool, dataIndex: "tool", key: "tool" },
      { title: t.mcpDevice, key: "device", render: (_: unknown, row: ToolCall) => deviceName(row.deviceRef, row.deviceCode) },
      { title: t.mcpState, key: "state", render: (_: unknown, row: ToolCall) => !row.finishedAtUnixMs ? stateTag("running") : row.succeeded === null ? stateTag("interrupted") : stateTag(row.succeeded ? "succeeded" : "failed") },
      { title: t.mcpStarted, dataIndex: "startedAtUnixMs", key: "started", render: time },
    ];
    const taskColumns = [
      { title: t.mcpTask, dataIndex: "capability", key: "capability" },
      { title: t.mcpDevice, key: "device", render: (_: unknown, row: ReportedTask) => deviceName(row.deviceRef) },
      { title: t.mcpState, dataIndex: "state", key: "state", render: stateTag },
      { title: t.mcpStarted, dataIndex: "createdAtUnixMs", key: "time", render: time },
      { title: t.mcpExitCode, dataIndex: "exitCode", key: "exit", render: (value: number | null) => value ?? "—" },
    ];
    const operationColumns = [
      { title: t.mcpOperation, dataIndex: "kind", key: "kind", render: (value: string) => ({ file_transfer: t.mcpTransfer, terminal: t.terminal, screenshot: t.captureScreenshot, directory: t.directoryBrowse, windows: t.windowList, desktop_input: t.remoteInput } as Record<string, string>)[value] ?? value },
      { title: t.mcpDevice, key: "device", render: (_: unknown, row: ReportedOperation) => deviceName(row.deviceRef, row.deviceCode) },
      { title: t.mcpState, dataIndex: "state", key: "state", render: stateTag },
      { title: t.mcpProgress, key: "progress", render: (_: unknown, row: ReportedOperation) => `${row.completedBytes.toLocaleString()} / ${row.totalBytes.toLocaleString()}` },
    ];
    return <div className="mcp-process-details">
      <Descriptions size="small" column={2} items={[
        { key: "version", label: t.mcpVersion, children: report.version },
        { key: "system", label: t.mcpSystem, children: `${report.os} · ${report.architecture}` },
        { key: "started", label: t.mcpStarted, children: time(report.startedAtUnixMs) },
        { key: "client", label: t.mcpClient, children: [report.clientName, report.clientVersion].filter(Boolean).join(" · ") || "—" },
        { key: "bridge", label: t.mcpControl, children: runtime ? stateTag(runtime.controlPhase) : t.mcpNotStarted },
        { key: "calls", label: t.mcpActiveCalls, children: report.activeCalls.length },
      ]} />
      {runtime?.lastError && <Alert type="error" showIcon title={runtime.lastError} />}
      <Tabs size="small" items={[
        { key: "devices", label: `${t.mcpDevices} (${runtime?.devices.length ?? 0})`, children: runtime?.devices.length ? <div className="mcp-device-list">{runtime.devices.map(device => <div className="mcp-device-row" key={`${device.deviceRef.deployment_id}/${device.deviceRef.tenant_id}/${device.deviceRef.device_id}`}>
          <div><strong>{device.deviceCode ? formatDeviceCode(device.deviceCode) : device.deviceRef.device_id}</strong>
            {(device.alias || device.name) && <div>{device.alias || device.name}</div>}
            {device.environment && <Typography.Text type="secondary">{[device.environment.os_name, device.environment.os_version, device.environment.architecture].filter(Boolean).join(" · ")}</Typography.Text>}
            {device.lastError && <div className="settings-error">{device.lastError}</div>}
          </div>
          <Space size={4}>{stateTag(device.phase)}{device.connectionPath && <Tag>{device.connectionPath === "p2p" ? "P2P" : device.connectionPath === "relay" ? "Relay" : t.unknown}</Tag>}</Space>
        </div>)}</div> : <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t.mcpNoDevices} /> },
        { key: "calls", label: t.mcpCalls, children: <Table size="small" rowKey="id" dataSource={[...report.activeCalls, ...report.recentCalls]} columns={callColumns} pagination={{ pageSize: 20, showSizeChanger: false }} scroll={{ x: 580 }} /> },
        { key: "tasks", label: `${t.mcpTasks} (${runtime?.tasks.length ?? 0})`, children: <Table size="small" rowKey="requestId" dataSource={runtime?.tasks ?? []} columns={taskColumns} pagination={{ pageSize: 20, showSizeChanger: false }} scroll={{ x: 600 }} /> },
        { key: "operations", label: `${t.mcpOperations} (${runtime?.operations.length ?? 0})`, children: <Table size="small" rowKey="id" dataSource={runtime?.operations ?? []} columns={operationColumns} pagination={{ pageSize: 20, showSizeChanger: false }} scroll={{ x: 580 }}
          expandable={{ expandedRowRender: row => <Descriptions size="small" column={1} items={[
            { key: "direction", label: t.mcpDirection, children: row.direction === "upload" ? t.upload : row.direction === "download" ? t.download : row.direction },
            { key: "source", label: t.mcpSource, children: row.source || "—" },
            { key: "destination", label: t.mcpDestination, children: row.destination || "—" },
            { key: "started", label: t.mcpStarted, children: time(row.startedAtUnixMs) },
          ]} /> }} /> },
      ]} />
    </div>;
  }

  return <div className="mcp-reporting-panel">
    <div className="mcp-reporting-heading"><h3>{t.mcpConnections}</h3><Badge count={status?.count ?? 0} showZero overflowCount={Number.MAX_SAFE_INTEGER} /></div>
    {(error || status?.error) && <Alert type="error" showIcon title={t.mcpServiceFailed} description={error || status?.error} />}
    {!status && !error ? <p>{t.loading}</p> : status?.running && <>
      {status.clients.length ? <Collapse size="small" items={status.clients.map(client => ({
        key: client.report.sessionId,
        label: <Space wrap><strong>{client.report.clientName || "MCP"}</strong><Typography.Text type="secondary">PID {client.report.processId}</Typography.Text>{clientStatusTag(client)}</Space>,
        children: details(client),
      }))} /> : <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t.mcpNoConnections} />}
    </>}
  </div>;
}
