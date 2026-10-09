import { useEffect, useState } from 'react';
import { Alert, App, Badge, Button, Card, Col, Form, Input, Modal, Row, Select, Space, Statistic, Table, Typography } from 'antd';
import { api, ApiError } from './api';
import { useText, useDate } from './i18n';
import { useResource } from './useResource';

type SavedDevice = { device_ref: { device_id: string }; code: string; name: string; alias: string; system?: string; revision: number; deleted: boolean; online: boolean | null };
type Changes = { items: SavedDevice[]; cursor: number; has_more: boolean };

export function SavedDevicesPage({ liveRevision }: { liveRevision: number }) {
  const t = useText(); const { message } = App.useApp();
  const [items, setItems] = useState<SavedDevice[]>([]); const [loading, setLoading] = useState(true); const [error, setError] = useState<string>();
  const [revision, setRevision] = useState(0); const [editing, setEditing] = useState<SavedDevice>(); const [removing, setRemoving] = useState<SavedDevice>(); const [busy, setBusy] = useState(false); const [form] = Form.useForm();
  useEffect(() => {
    const abort = new AbortController(); setError(undefined);
    async function load() {
      const map = new Map<string, SavedDevice>(); let cursor = 0;
      do {
        const page = await api<Changes>(`/saved-devices?after=${cursor}&limit=500`, { signal: abort.signal });
        for (const item of page.items) { if (item.deleted) map.delete(item.device_ref.device_id); else map.set(item.device_ref.device_id, item); }
        if (!page.has_more) break;
        if (page.cursor <= cursor) throw new Error('invalid cursor');
        cursor = page.cursor;
      } while (!abort.signal.aborted);
      if (!abort.signal.aborted) setItems([...map.values()]);
    }
    void load().catch(e => { if (!abort.signal.aborted) setError(e instanceof ApiError ? e.code : 'networkError'); }).finally(() => { if (!abort.signal.aborted) setLoading(false); });
    return () => abort.abort();
  }, [revision, liveRevision]);
  async function save(item: SavedDevice, alias: string, deleted: boolean) {
    setBusy(true);
    try {
      await api('/saved-devices', { method: 'POST', body: JSON.stringify({ id: crypto.randomUUID(), device_id: item.device_ref.device_id, expected_revision: item.revision, alias, deleted }) });
      setEditing(undefined); setRemoving(undefined); setRevision(v => v + 1); message.success(t('saved'));
    } catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); setRevision(v => v + 1); } finally { setBusy(false); }
  }
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}>
    <Typography.Title level={2}>{t('savedDevices')}</Typography.Title><Typography.Text type="secondary">{t('savedDevicesHint')}</Typography.Text>
    {error && <Alert type="error" title={t(error)} action={<Button onClick={() => setRevision(v => v + 1)}>{t('retry')}</Button>} />}
    <Card><Table rowKey={v => v.device_ref.device_id} dataSource={items} loading={loading} pagination={{ pageSize: 20, showSizeChanger: true, showTotal: n => `${t('total')} ${n}` }} columns={[
      { title: t('deviceCode'), dataIndex: 'code' }, { title: t('name'), render: (_, item) => item.alias || item.name },
      { title: t('system'), render: (_, item) => item.system ?? t('unknown') },
      { title: t('online'), render: (_, item) => item.online === null ? t('device_verification_required') : <Badge status={item.online ? 'success' : 'default'} text={t(item.online ? 'online' : 'offline')} /> },
      { title: t('actions'), render: (_, item) => <Space><Button onClick={() => { setEditing(item); form.setFieldsValue({ alias: item.alias }); }}>{t('personalAlias')}</Button><Button danger onClick={() => setRemoving(item)}>{t('removeSaved')}</Button></Space> },
    ]} /></Card>
    <Modal title={t('personalAlias')} open={!!editing} maskClosable={false} keyboard={false} closable={!busy} onCancel={() => !busy && setEditing(undefined)} onOk={() => form.submit()} confirmLoading={busy}>
      <Form form={form} onFinish={({ alias }) => editing && void save(editing, alias ?? '', false)}><Form.Item name="alias" rules={[{ max: 128, message: t('invalid_input') }]}><Input maxLength={128} /></Form.Item></Form>
    </Modal>
    <Modal title={t('removeSaved')} open={!!removing} maskClosable={false} keyboard={false} closable={!busy} onCancel={() => !busy && setRemoving(undefined)} onOk={() => removing && void save(removing, removing.alias, true)} confirmLoading={busy}>{t('removeSavedHint')}</Modal>
  </Space>;
}

type Usage = { counters: Record<string, number | boolean>; points: { hour_unix_ms: number; counters: Record<string, number | boolean> }[]; updated_at: number | null };
const byteText = (value: number) => value < 1024 ? `${value} B` : value < 1024 ** 2 ? `${(value / 1024).toFixed(1)} KiB` : value < 1024 ** 3 ? `${(value / 1024 ** 2).toFixed(1)} MiB` : `${(value / 1024 ** 3).toFixed(2)} GiB`;

export function UsagePage({ all, liveRevision }: { all: boolean; liveRevision: number }) {
  const t = useText(); const date = useDate(); const [period, setPeriod] = useState('today'); const [user, setUser] = useState<string>();
  const [population, setPopulation] = useState('all');
  const limits = useResource<{ user_mbps: number; guest_mbps: number }>('/traffic');
  const now = new Date(); const start = new Date(now.getFullYear(), now.getMonth(), period === 'month' ? 1 : now.getDate());
  const until = Math.floor(now.getTime() / 3600000) * 3600000 + 3600000;
  // Storage resolution is one UTC hour; label the rounded boundary for half-hour time zones.
  const from = period === 'today' || period === 'month' ? Math.floor(start.getTime() / 3600000) * 3600000 : until - Number(period === 'lifetime' ? 1 : period) * 86400000;
  const suffix = `${user && all ? `&user=${encodeURIComponent(user)}` : ''}${all ? `&population=${population}` : ''}`;
  const resource = useResource<Usage>(`/usage?scope=${all ? 'all' : 'mine'}&from=${from}&until=${until}&interval=${period === 'lifetime' ? 'lifetime' : 'hour'}${suffix}`);
  const previousUntil = period === 'today' || period === 'month' ? Math.floor(start.getTime() / 3600000) * 3600000 : from;
  const previousStart = new Date(start); if (period === 'month') previousStart.setMonth(previousStart.getMonth() - 1); else previousStart.setDate(previousStart.getDate() - 1);
  const previousFrom = period === 'today' || period === 'month' ? Math.floor(previousStart.getTime() / 3600000) * 3600000 : from - (until - from);
  const previous = useResource<Usage>(`/usage?scope=${all ? 'all' : 'mine'}&from=${previousFrom}&until=${previousUntil}${suffix}`);
  useEffect(resource.refresh, [liveRevision]);
  const metrics = ['relay_upload_bytes', 'relay_download_bytes', 'connections', 'connection_ms', 'uploaded_files', 'downloaded_files', 'uploaded_bytes', 'downloaded_bytes', 'failed_transfers', 'cancelled_transfers'];
  const format = (key: string, n: number) => key.endsWith('_bytes') ? byteText(n) : key === 'connection_ms' ? `${(n / 3600000).toFixed(2)} h` : String(n);
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}>
    <Typography.Title level={2}>{t(all ? 'serviceUsage' : 'myUsage')}</Typography.Title>
    <Space wrap><Select aria-label={t('usagePeriod')} value={period} onChange={setPeriod} options={['today', 'month', '7', '30', '90', 'lifetime'].map(value => ({ value, label: Number.isFinite(Number(value)) ? `${value} ${t('days')}` : t(value) }))} />{all && <><Select aria-label={t('usagePopulation')} value={population} onChange={value => { setPopulation(value); setUser(undefined); }} options={['all', 'users', 'guests'].map(value => ({ value, label: t(`usagePopulation_${value}`) }))} />{population !== 'guests' && <AccountSelector value={user} onChange={setUser} />}</>}<Button onClick={() => { resource.refresh(); previous.refresh(); }}>{t('refresh')}</Button></Space>
    {period !== 'lifetime' && <Typography.Text type="secondary">{date(from)} — {date(until)} ({Intl.DateTimeFormat().resolvedOptions().timeZone})</Typography.Text>}
    {!all && <Typography.Text>{t('userBandwidth')}: {limits.data ? `${limits.data.user_mbps} Mbps` : '—'}</Typography.Text>}
    <Alert type="info" title={t('usageHint')} showIcon />
    {resource.error && <Alert type="error" title={t(resource.error)} />}
    {resource.data?.counters.incomplete && <Alert type="warning" title={t('usageIncomplete')} showIcon />}
    <Typography.Text type="secondary">{resource.data?.updated_at ? `${t('lastSeen')}: ${date(resource.data.updated_at)}` : t('usagePending')}</Typography.Text>
    {period !== 'lifetime' && <Typography.Text type="secondary">{t('previousPeriod')}: {date(previousFrom)} — {date(previousUntil)}</Typography.Text>}
    <Row gutter={[16, 16]}>{metrics.map(key => <Col xs={24} sm={12} lg={8} key={key}><Card loading={resource.loading}><Statistic title={t(key)} value={resource.data?.updated_at ? format(key, Number(resource.data.counters[key] ?? 0)) : '—'} />{period !== 'lifetime' && <Typography.Text type="secondary">{t('previousPeriod')}: {previous.data?.updated_at ? format(key, Number(previous.data.counters[key] ?? 0)) : '—'}</Typography.Text>}</Card></Col>)}</Row>
    {period !== 'lifetime' && <Card title={t('usageTrend')}><Table rowKey="hour_unix_ms" size="small" scroll={{ x: 650 }} dataSource={resource.data?.points ?? []} pagination={{ pageSize: 24 }} columns={[
      { title: t('time'), render: (_, v) => date(v.hour_unix_ms) }, ...['relay_upload_bytes', 'relay_download_bytes', 'uploaded_bytes', 'downloaded_bytes'].map(key => ({ title: t(key), render: (_: unknown, v: Usage['points'][number]) => byteText(Number(v.counters[key] ?? 0)) })),
    ]} /></Card>}
  </Space>;
}

function AccountSelector({ value, onChange }: { value?: string; onChange: (value: string | undefined) => void }) {
  const t = useText(); const [search, setSearch] = useState(''); const [query, setQuery] = useState('');
  useEffect(() => { const timer = setTimeout(() => setQuery(search), 250); return () => clearTimeout(timer); }, [search]);
  const accounts = useResource<{ items: { id: string; username: string }[] }>(`/accounts?page_size=100&q=${encodeURIComponent(query)}`);
  return <Select allowClear showSearch={{ onSearch: setSearch, filterOption: false }} placeholder={t('username')} style={{ minWidth: 180 }} value={value} onChange={onChange} loading={accounts.loading} options={accounts.data?.items.map(v => ({ value: v.id, label: v.username }))} />;
}
