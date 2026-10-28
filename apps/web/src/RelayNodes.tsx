import { paginationComponents } from './ConsoleSelect';
import { useCallback, useContext, useEffect, useRef, useState } from 'react';
import { Alert, App, Button, Card, Checkbox, Descriptions, Input, InputNumber, Modal, Space, Table, Tabs, Tag, Tooltip, Typography } from 'antd';
import { useSearchParams } from 'react-router-dom';
import { api, ApiError } from './api';
import { LanguageContext, useDate, useText } from './i18n';
import { usePage, type Page } from './usePagedResource';
import { RelayOperationPanel } from './RelayOperation';
import type { RelayIntent } from './relayOperationState';
import { RelaySummary, type RelaySnapshot } from './RelaySummary';
import { RelayMachineStatus } from './RelayMachineStatus';
import { RelayDiagnostics } from './RelayDiagnostics';
import { byteText, counter, durationText } from './usageModel';

export function RelayNodes({ viewerId }: { liveRevision: number; viewerId: string }) {
  const t = useText(); const date = useDate(); const { message, modal } = App.useApp();
  const language = useContext(LanguageContext);
  const [params, setParams] = useSearchParams();
  type Node = { id: string; online: boolean; agent_version: string | null; applied_policy_version?: number | null; offered_policy_version?: number; last_seen: number | null;
    url?:string; ip_addresses?:string[]; machine_ip?:string | null; ip_source?:string; region?:string; capacity_mbps?:number; capacity_mode?:'host'|'configured'|null; telemetry_current?:boolean; telemetry_sampled_at?:number|null; configured?:boolean; state_applied?:boolean; state_drained?:boolean; node_control_version?:number; available?:boolean; sample_current_instance?:boolean; heartbeat_age_ms?:number|null; boot_id?:string|null; admin_state?:'active'|'maintenance'|'disabled'; health?:string; connections?:number; connections_exact?:string|null; uploaded_bytes?:number; downloaded_bytes?:number; operation_id?:string|null; uploaded_bytes_exact?:string|null; downloaded_bytes_exact?:string|null };
  const [editing,setEditing] = useState<{node:Node; state:'active'|'maintenance'|'disabled'; operationId:string}>();
  const [capacityNode, setCapacityNode] = useState<Node>();
  const [machineIp, setMachineIp] = useState('');
  const [capacityValue, setCapacityValue] = useState<number | null>(200);
  const [capacityError, setCapacityError] = useState<string>();
  const [busy,setBusy] = useState(false); const [allowOutage,setAllowOutage] = useState(false); const [error,setError] = useState<string>();
  const submitting = useRef(false);
  const [pendingIntent, setPendingIntent] = useState<RelayIntent>();
  type Selection = { id: string; unconfirmed: boolean; intent?: RelayIntent };
  const storageKey = `pab-relay-operation:${viewerId}`;
  const [selected, setSelected] = useState<Selection | undefined>(() => {
    try {
      const value = JSON.parse(localStorage.getItem(storageKey) ?? 'null');
      if (!value || !/^[0-9a-f-]{36}$/i.test(value.id)) return undefined;
      const intent = value.intent;
      if (intent && (intent.operation_id !== value.id || typeof intent.node_id !== 'string' || !/^[a-zA-Z0-9_-]{1,128}$/.test(intent.node_id) || !['active','maintenance','disabled'].includes(intent.state) || typeof intent.allow_outage !== 'boolean')) return undefined;
      return { id: value.id, intent, unconfirmed: Boolean(value.unconfirmed) };
    } catch { return undefined; }
  });
  useEffect(() => {
    try { if (selected) localStorage.setItem(storageKey, JSON.stringify(selected)); else localStorage.removeItem(storageKey); } catch { /* Server-side row actions still recover the operation when storage is unavailable. */ }
  }, [selected, storageKey]);
  const confirmed = useCallback(() => setSelected(current => current?.unconfirmed ? { ...current, unconfirmed: false } : current), []);
  const resource = usePage<Node>('/relays', 0);
  const [detailId, setDetailId] = useState<string>();
  const [detailTab, setDetailTab] = useState('info');
  const detail = resource.data?.items.find(item => item.id === detailId);
  const cluster = resource.data?.mode === 'cluster';
  const sampleValid = (item: Node) => !resource.error && item.online && item.sample_current_instance !== false;
  const telemetryValid = (item: Node) => sampleValid(item) && item.telemetry_current !== false;
  const showRuntime = cluster || resource.data?.items.some(item => item.telemetry_sampled_at != null);
  const stateLabel = (item: Node) => {
    if (item.state_applied !== undefined && item.operation_id) {
      if (item.admin_state === 'disabled' && (!item.state_applied || !item.state_drained)) return 'relayState.disabling';
      if (item.admin_state === 'active' && !item.state_applied) return 'relayState.resuming';
    }
    if (item.admin_state === 'maintenance' && item.state_applied && item.state_drained) return 'relayState.drained';
    return `relayState.${item.admin_state}`;
  };
  const capacity = (item: Node) => item.capacity_mode === 'host' ? t('relayHostBandwidth') : item.capacity_mbps ? `${item.capacity_mbps} Mbps` : t('unknown');
  const bytes = (item: Node, key: 'uploaded_bytes' | 'downloaded_bytes') => {
    const value = counter({ [key]: item[`${key}_exact`] ?? false }, { [key]: item[key] ?? false }, key);
    return telemetryValid(item) && value !== undefined ? <span title={`${value} B`}>{byteText(value)}</span> : t('unknown');
  };
  useEffect(() => {
    const poll = () => { if (document.visibilityState !== 'hidden') resource.refresh(); };
    const timer = window.setInterval(poll, 5000);
    document.addEventListener('visibilitychange', poll);
    return () => { window.clearInterval(timer); document.removeEventListener('visibilitychange', poll); };
  }, [resource.refresh]);
  const choose = (node:Node,state:'active'|'maintenance'|'disabled') => { setEditing({node,state,operationId:crypto.randomUUID()});setAllowOutage(false);setError(undefined); };
  async function submitIntent(intent: RelayIntent) {
    if (busy || submitting.current) return; submitting.current = true; setBusy(true);setPendingIntent(intent);setError(undefined);
    setSelected({ id: intent.operation_id, intent, unconfirmed: true });
    try { await api(`/relays/${encodeURIComponent(intent.node_id)}/state`,{method:'PATCH',body:JSON.stringify({state:intent.state,operation_id:intent.operation_id,allow_outage:intent.allow_outage})});
      setSelected({ id: intent.operation_id, unconfirmed: false });
      setEditing(undefined);resource.refresh();message.success(t('relayAccepted')); }
    catch(e) {
      if (e instanceof ApiError && e.status < 500) {
        setSelected(undefined);
        setError(e.code); message.error(t(e.code));
      } else { setEditing(undefined); resource.refresh(); }
    }
    finally { submitting.current = false; setBusy(false);setPendingIntent(undefined); }
  }
  function changeState() {
    if (editing) void submitIntent({ node_id: editing.node.id, state: editing.state, operation_id: editing.operationId, allow_outage: allowOutage });
  }
  async function reload() {
    setBusy(true);
    try {await api('/relays/reload',{method:'POST'});resource.refresh();message.success(t('saved'));}
    catch(e){message.error(t(e instanceof ApiError?e.code:'networkError'));}
    finally {setBusy(false);}
  }
  async function saveCapacity() {
    if (!capacityNode || busy || submitting.current || capacityValue == null || !Number.isInteger(capacityValue) || capacityValue < 1 || capacityValue > 2147483647) return;
    submitting.current = true; setBusy(true); setCapacityError(undefined);
    try {
      await api(`/relays/${encodeURIComponent(capacityNode.id)}/capacity`, { method: 'PATCH', body: JSON.stringify({ capacity_mbps: capacityValue, machine_ip: machineIp.trim() }) });
      setCapacityNode(undefined); resource.refresh(); message.success(t('saved'));
    } catch (error) { setCapacityError(error instanceof ApiError ? error.code : 'networkError'); }
    finally { submitting.current = false; setBusy(false); }
  }
  const ipAddresses = (item: Node) => item.ip_addresses?.length ? item.ip_addresses.join(', ') : t('unknown');
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}>
    {resource.error && <Alert type="error" title={t(resource.error)}/>}
    {error && !editing && <Alert type="error" showIcon title={t(error)} closable onClose={() => setError(undefined)}/>}
    {resource.data?.summary && <RelaySummary snapshot={resource.data as Page<Node> & RelaySnapshot} unavailable={!!resource.error} onSelect={status => { const next = new URLSearchParams(params); next.set('status', status); next.delete('page'); setParams(next); }}/>}
    <Card className="relay-nodes-card" title={t('nodes')} extra={<Space wrap><Button loading={busy} disabled={selected?.unconfirmed || !!resource.error || !resource.data} onClick={() => modal.confirm({ title: t('relayReload'), content: t('relayReloadHint'), okText: t('confirm'), cancelText: t('cancel'), onOk: reload })}>{t('relayReload')}</Button><Button onClick={resource.refresh}>{t('refresh')}</Button></Space>}>
    <Space wrap style={{ marginBottom: 16 }}>
      <Input.Search aria-label={t('nodes')} key={resource.query} defaultValue={resource.query} onSearch={resource.search} allowClear maxLength={128}/>
      {params.get('status') && <Button onClick={() => { const next = new URLSearchParams(params); next.delete('status'); next.delete('page'); setParams(next); }}>{t('all')}</Button>}
    </Space>
    <Table<Node> rowKey="id" dataSource={resource.data?.items ?? []} loading={resource.loading} pagination={{ components: paginationComponents, current: resource.data?.page ?? 1, pageSize: resource.data?.page_size ?? 20, total: resource.data?.total ?? 0, showSizeChanger: true, pageSizeOptions: [20,50,100], onChange: resource.paginate }} scroll={{ x: showRuntime ? 1500 : 1100 }} columns={[
      { title: t('name'), render:(_,item)=><Space orientation="vertical" size={0}><Typography.Text strong>{item.id}</Typography.Text>{item.url && <Typography.Text type="secondary" copyable style={{fontSize:12,overflowWrap:'anywhere'}}>{item.url}</Typography.Text>}<Typography.Text type="secondary" style={{fontSize:12,overflowWrap:'anywhere'}}><Tooltip title={t('relayIpHint')}>{t(item.ip_source === 'configured' ? 'relayIp' : 'relayIpDns')}: {ipAddresses(item)}</Tooltip></Typography.Text>{item.region && <Typography.Text type="secondary">{item.region}</Typography.Text>}</Space> },
      { title: t('controlStatus'), render: (_, item) => <Space orientation="vertical" size={4}><Tag color={resource.error ? 'default' : item.online ? 'success' : 'default'}>{resource.error ? t('unknown') : item.health ? t(`relayHealth.${item.health}`) : t(item.online ? 'online' : 'offline')}</Tag>{item.admin_state && <Tag color={item.admin_state==='maintenance'?'warning':undefined}>{t(stateLabel(item))}</Tag>}{item.configured === false && <Tag>{t('relayCount.history')}</Tag>}</Space> },
      { title: t('version'), render: (_, item) => item.agent_version ?? t('unknown') },
      { title: t('relayPolicySync'), render:(_,item)=><Space orientation="vertical" size={2}><span>{t('policyApplied')}: {sampleValid(item) ? item.applied_policy_version ?? '—' : t('unknown')}</span><Typography.Text type="secondary">{t('policyOffered')}: {item.offered_policy_version ?? '—'}</Typography.Text></Space> },
      ...(showRuntime ? [{ title: t('relayCapacity'), render:(_: unknown,item: Node)=><Space orientation="vertical" size={0}><span>{capacity(item)}</span>{cluster && item.configured !== false && <Button type="link" size="small" style={{padding:0,height:'auto'}} aria-label={`${t('relayEditCapacity')} ${item.id}`} disabled={busy || !!resource.error} onClick={() => { setCapacityNode(item); setCapacityValue(item.capacity_mbps || 200); setMachineIp(item.machine_ip ?? ''); setCapacityError(undefined); }}>{t('settings')}</Button>}</Space> },
        { title:<Tooltip title={t('relayConnectionCountHint')}>{t('relayLiveConnections')}</Tooltip>,render:(_: unknown,item: Node)=>telemetryValid(item) ? item.connections_exact ?? item.connections ?? t('unknown') : t('unknown') }] : []),
      { title: <Tooltip title={t('relayMachineLoadHint')}>{t('relayMachineLoad')}</Tooltip>, width: 260, render: (_, item) => <RelayMachineStatus nodeId={item.id} current={sampleValid(item)}/> },
      { title: t('lastSeen'), render: (_, item) => item.last_seen ? date(item.last_seen) : '—' },
      { title:t('actions'),render:(_,item)=><Space wrap><Button size="small" onClick={() => {setDetailTab("info");setDetailId(item.id);}}>{t('details')}</Button><Button size="small" onClick={()=>{setDetailTab("runtime");setDetailId(item.id);}}>{t("relayRuntime")}</Button>{item.operation_id && <Button size="small" disabled={busy || selected?.unconfirmed} onClick={() => setSelected({ id: item.operation_id!, unconfirmed: false })}>{t('relayOperation')}</Button>}{item.admin_state ? <>{item.admin_state!=='maintenance' && <Button size="small" disabled={busy || selected?.unconfirmed || !!resource.error || item.configured === false} onClick={()=>choose(item,'maintenance')}>{t('relayMaintain')}</Button>}{item.admin_state!=='active' && <Button size="small" aria-busy={pendingIntent?.node_id===item.id && pendingIntent.state==='active'} disabled={busy || selected?.unconfirmed || !!resource.error || item.configured === false || item.health!=='healthy'} onClick={()=>void submitIntent({node_id:item.id,state:'active',operation_id:crypto.randomUUID(),allow_outage:false})}>{t(pendingIntent?.node_id===item.id && pendingIntent.state==='active'?'relayResuming':'relayActivate')}</Button>}{item.admin_state!=='disabled' && <Button size="small" danger disabled={busy || selected?.unconfirmed || !!resource.error || item.configured === false} onClick={()=>choose(item,'disabled')}>{t('relayDisable')}</Button>}</> : <Tooltip title={t('relayLegacyActions')}><span tabIndex={0} aria-label={t('relayLegacyActions')}><Button size="small" disabled>{t('relayMaintain')}</Button></span></Tooltip>}</Space> },
    ]}/>
    {selected && <RelayOperationPanel key={selected.id} id={selected.id} onConfirmed={confirmed} busy={busy} onRetry={selected.unconfirmed && selected.intent ? () => void submitIntent(selected.intent!) : undefined}/>}
    <Modal title={`${t(editing?.state==='active'?'relayActivate':editing?.state==='disabled'?'relayDisable':'relayMaintain')} · ${editing?.node.id ?? ''}`} open={!!editing} maskClosable={false} keyboard={false} onCancel={()=>!busy && setEditing(undefined)} onOk={changeState} confirmLoading={busy} okText={t('confirm')} okButtonProps={{ 'aria-label': t('confirm') }} cancelText={t('cancel')}>
      <Typography.Paragraph>{t(editing?.state==='active'?'relayActivateHint':'relayMaintenanceHint')}</Typography.Paragraph>
      {error && <Alert type="error" showIcon title={t(error)} style={{marginBottom:16}}/>}
      {editing?.state!=='active' && <Checkbox checked={allowOutage} disabled={busy} onChange={event=>{setAllowOutage(event.target.checked);setEditing(current=>current && {...current,operationId:crypto.randomUUID()});}}>{t('relayAllowOutage')}</Checkbox>}
    </Modal>
    </Card>
    <Modal title={`${t('relayEditCapacity')} · ${capacityNode?.id ?? ''}`} open={!!capacityNode} maskClosable={false} onCancel={() => !busy && setCapacityNode(undefined)} onOk={saveCapacity} confirmLoading={busy} okText={t('save')} cancelText={t('cancel')} okButtonProps={{disabled:capacityValue == null || !Number.isInteger(capacityValue) || capacityValue < 1 || capacityValue > 2147483647}}>
      <Typography.Paragraph type="secondary">{t('relayCapacityHint')}</Typography.Paragraph>
      {capacityError && <Alert type="error" showIcon title={t(capacityError)} style={{marginBottom:16}}/>}
      <label htmlFor="relay-machine-ip">{t('relayMachineIp')}</label>
      <Input id="relay-machine-ip" aria-label={t('relayMachineIp')} value={machineIp} onChange={event => setMachineIp(event.target.value)} maxLength={45} placeholder={t('relayMachineIpPlaceholder')} disabled={busy} style={{marginTop:8,marginBottom:16}}/>
      <label htmlFor="relay-capacity-input">{t('relayCapacity')} (Mbps)</label>
      <InputNumber id="relay-capacity-input" aria-label={t('relayCapacity')} min={1} max={2147483647} precision={0} value={capacityValue} onChange={setCapacityValue} disabled={busy} style={{display:'block',marginTop:8,width:'100%'}}/>
    </Modal>
    <Modal className="relay-detail" title={detail?.id ?? t('details')} centered width={1100} footer={null} destroyOnHidden open={!!detail} onCancel={() => setDetailId(undefined)} styles={{ body: { maxHeight: 'calc(100dvh - 160px)', overflowY: 'auto' } }}>
      {detail && <Tabs activeKey={detailTab} onChange={setDetailTab} destroyOnHidden items={[
        {key:'runtime',label:t('relayRuntime'),children:<RelayDiagnostics nodeId={detail.id}/>},
        {key:'info',label:t('relayNodeInfo'),children:<><Descriptions column={{xs:1,sm:2}} items={[
        { key: 'version', label: t('version'), children: detail.agent_version ?? t('unknown') },
        { key: 'health', label: t('controlStatus'), children: t(resource.error ? 'unknown' : detail.online ? 'online' : 'offline') },
        { key: 'last', label: t('lastSeen'), children: detail.last_seen ? date(detail.last_seen) : t('unknown') },
        { key: 'age', label: t('relayHeartbeatAge'), children: resource.error || detail.heartbeat_age_ms == null ? t('unknown') : durationText(String(Math.max(0, Math.floor(detail.heartbeat_age_ms))), language) },
        { key: 'window', label: t('relayHeartbeatWindow'), children: `${resource.data?.offline_seconds ?? '—'} s` },
        { key: 'applied', label: t('policyApplied'), children: sampleValid(detail) ? detail.applied_policy_version ?? t('unknown') : t('unknown') },
        { key: 'offered', label: t('policyOffered'), children: detail.offered_policy_version ?? t('unknown') },
        ...(cluster || detail.telemetry_sampled_at != null ? [
          { key: 'url', label: t('relayEndpoint'), children: detail.url ? <Typography.Text copyable>{detail.url}</Typography.Text> : t('unknown') },
          ...(cluster ? [{ key: 'region', label: t('relayRegion'), children: detail.region ?? t('unknown') }] : []),
          { key: 'capacity', label: t('relayCapacity'), children: capacity(detail) },
          { key: 'ips', label: t('relayIp'), children: <Tooltip title={t('relayIpHint')}>{ipAddresses(detail)}</Tooltip> },
          { key: 'connections', label: t('relayLiveConnections'), children: telemetryValid(detail) ? detail.connections_exact ?? detail.connections ?? t('unknown') : t('unknown') },
          { key: 'boot', label: t('relayBootId'), children: detail.boot_id ?? t('unknown') },
          { key: 'sampled', label: t('relayTelemetryAt'), children: detail.telemetry_sampled_at ? date(detail.telemetry_sampled_at) : t('unknown') },
          { key: 'upload', label: t('relay_upload_bytes'), children: bytes(detail, 'uploaded_bytes') },
          { key: 'download', label: t('relay_download_bytes'), children: bytes(detail, 'downloaded_bytes') },
        ] : []),
      ]}/>{!cluster && detail.telemetry_sampled_at == null && <Typography.Paragraph type="secondary">{t('relayLegacyTelemetry')}</Typography.Paragraph>}</>},
      ]}/>}
    </Modal>
  </Space>;
}
