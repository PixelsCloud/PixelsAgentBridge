import { useEffect, useState } from 'react';
import { Alert, App, Badge, Button, Card, Col, Descriptions, Dropdown, Form, Input, Modal, Row, Select, Space, Statistic, Table, Typography } from 'antd';
import { Copy, Ellipsis, Monitor, RefreshCw } from 'lucide-react';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { api, ApiError, type Viewer } from './api';
import { useResource } from './useResource';
import { useText, useDate } from './i18n';

export interface Device { id: string; code: string; name: string; revision: number; status: string; system: string | null; os_name: string | null; os_version: string | null; architecture: string | null; agent_version: string | null; created_at: number; last_online_at: number | null; online: boolean; association_revision?: number; associated_with_me?: boolean }
interface DeviceList { items: Device[]; total: number; page: number; page_size: number }
export const formatCode = (code: string) => code.replace(/(\d{3})(?=\d)/g, '$1 ');

export function Overview({ liveRevision, all = false }: { liveRevision: number; all?: boolean }) {
  const t = useText(); const navigate = useNavigate();
  const resource = useResource<Record<string, number>>(`/overview?scope=${all ? 'all' : 'mine'}`);
  useEffect(resource.refresh, [liveRevision]);
  return <Space orientation="vertical" size={24} style={{ width: '100%' }}>
    <Typography.Title level={2}>{t('overview')}</Typography.Title>
    {resource.error && <Alert type="error" title={t(resource.error)} action={<Button onClick={resource.refresh}>{t('retry')}</Button>}/>}
    <Row gutter={[20, 20]}>{(['total', 'online', 'offline'] as const).map(key => <Col xs={24} sm={8} xl={8} key={key}><Card loading={resource.loading}><Statistic title={t(key === 'total' ? 'totalDevices' : key)} value={resource.data?.[key] ?? '—'} valueRender={value => <Button type="link" style={{ fontSize: 'inherit', height: 'auto', padding: 0 }} aria-label={t(key === 'total' ? 'totalDevices' : key)} onClick={() => navigate(key === 'total' ? '/devices' : `/devices?status=${key}`)}>{value}</Button>}/></Card></Col>)}</Row>
    <Row gutter={[20, 20]}>{['accounts', 'relays'].filter(key => resource.data?.[key] !== undefined).map(key => <Col xs={24} sm={12} xl={6} key={key}><Card><Statistic title={t(key === 'relays' ? 'nodes' : key)} value={resource.data?.[key] ?? 0}/><Button type="link" onClick={() => navigate(key === 'relays' ? '/relay' : `/${key}`)}>{t('details')}</Button></Card></Col>)}</Row>
    <Card><Space orientation="vertical" size={16}><Monitor size={32}/><Typography.Title level={4}>{t('devices')}</Typography.Title><Typography.Text type="secondary">{t('deviceOnlineHint')}</Typography.Text><Button type="primary" onClick={() => navigate('/devices')}>{t('devices')}</Button></Space></Card>
  </Space>;
}

export function DeviceListPage({ me, liveRevision }: { me: Viewer; liveRevision: number }) {
  const t = useText(); const date = useDate(); const navigate = useNavigate(); const { message } = App.useApp();
  const [params, setParams] = useSearchParams();
  useEffect(() => {
    if (params.has('owner')) {
      const next = new URLSearchParams(params); next.delete('owner'); next.delete('page');
      setParams(next, { replace: true });
    }
  }, [params, setParams]);
  const query = new URLSearchParams();
  for (const key of ['q', 'page', 'page_size', 'system', 'status']) { const value = params.get(key); if (value) query.set(key, value); }
  query.set('scope', me.server_admin ? 'all' : 'mine');
  const resource = useResource<DeviceList>(`/devices?${query}`);
  const [search, setSearch] = useState(params.get('q') ?? '');
  useEffect(() => setSearch(params.get('q') ?? ''), [params]);
  useEffect(resource.refresh, [liveRevision]);
  const update = (key: string, value?: string) => { const next = new URLSearchParams(params); value ? next.set(key, value) : next.delete(key); next.delete('page'); setParams(next); };
  const copy = async (item: Device) => { try { await navigator.clipboard.writeText(`${t('deviceCode')}: ${item.code.replace(/\s/g, '')}\n${t('name')}: ${item.name}`); message.success(t('copied')); } catch { message.error(t('networkError')); } };
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}>
    <div className="page-heading"><div><Typography.Title level={2}>{t(me.server_admin ? 'devices' : 'myDevices')}</Typography.Title><Typography.Text type="secondary">{t('deviceOnlineHint')}</Typography.Text></div><Button icon={<RefreshCw size={16}/>} onClick={resource.refresh} loading={resource.loading}>{t('refresh')}</Button></div>
    <Card><Space wrap style={{ marginBottom: 20 }}><Input.Search placeholder={t('search')} value={search} onChange={e => setSearch(e.target.value)} onSearch={value => update('q', value.trim())} allowClear style={{ width: 280 }} maxLength={128}/>
      <Select aria-label={t('online')} value={params.get('status') ?? ''} style={{ width: 130 }} onChange={value => update('status', value)} options={['', 'online', 'offline'].map(value => ({ value, label: t(value || 'all') }))}/>
      <Select aria-label={t('system')} value={params.get('system') ?? ''} style={{ width: 140 }} onChange={value => update('system', value)} options={[{ value: '', label: t('system') }, ...['windows', 'linux', 'macos'].map(value => ({ value, label: value === 'macos' ? 'macOS' : value === 'windows' ? 'Windows' : 'Linux' }))]}/>
    </Space>
      {resource.error && <Alert type="error" title={t(resource.error)} showIcon style={{ marginBottom: 16 }}/>}
      <Table<Device> rowKey="id" loading={resource.loading} dataSource={resource.data?.items ?? []} scroll={{ x: 850 }} locale={{ emptyText: t('noData') }}
        pagination={{ current: resource.data?.page ?? Number(params.get('page') || 1), pageSize: resource.data?.page_size ?? 20, total: resource.data?.total ?? 0, showSizeChanger: true, pageSizeOptions: [20, 50, 100], showTotal: total => `${t('total')} ${total}`, onChange: (page, size) => { const next = new URLSearchParams(params); next.set('page', String(page)); next.set('page_size', String(size)); setParams(next); } }}
        columns={[
          { title: t('deviceCode'), key: 'device', render: (_, item) => <Button type="link" className="device-link" onClick={() => navigate(`/devices/${item.id}`)}><Monitor size={22}/><div><strong>{formatCode(item.code)}</strong><span>{item.name}</span></div></Button> },
          { title: t('online'), key: 'online', render: (_, item) => <Badge status={item.online ? 'success' : 'default'} text={t(item.online ? 'online' : 'offline')}/> },
          { title: t('system'), key: 'system', render: (_, item) => item.os_name ?? item.system ?? t('unknown') },
          { title: t('lastOnline'), key: 'last', render: (_, item) => item.online ? t('online') : item.last_online_at ? date(item.last_online_at) : '—' },
          { title: t('actions'), key: 'actions', width: 72, render: (_, item) => <Dropdown menu={{ items: [{ key: 'details', label: t('details') }, { key: 'copy', label: t('copy'), icon: <Copy size={14}/> }], onClick: ({ key }) => key === 'copy' ? void copy(item) : navigate(`/devices/${item.id}`) }} trigger={['click']}><Button aria-label={t('actions')} icon={<Ellipsis size={18}/>}/></Dropdown> },
        ]}/>
    </Card>
  </Space>;
}

export function DeviceDetail({ liveRevision }: { liveRevision: number }) {
  const { id } = useParams(); const t = useText(); const date = useDate(); const { message } = App.useApp(); const navigate = useNavigate();
  const resource = useResource<Device>(`/devices/${id}`); const item = resource.data;
  const [open, setOpen] = useState(false); const [busy, setBusy] = useState(false); const [form] = Form.useForm();
  const [editing, setEditing] = useState<Device>();
  const [unlinking, setUnlinking] = useState<Device>();
  useEffect(() => { setOpen(false); setEditing(undefined); setUnlinking(undefined); }, [id]);
  useEffect(resource.refresh, [liveRevision]);
  async function rename(values: { name: string }) {
    if (!editing) return; setBusy(true);
    try { await api(`/devices/${editing.id}`, { method: 'PATCH', body: JSON.stringify({ name: values.name, revision: editing.revision }) }); setOpen(false); resource.refresh(); message.success(t('renamed')); }
    catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); } finally { setBusy(false); }
  }
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}><Button onClick={() => navigate(-1)} style={{ alignSelf: 'start' }}>{t('back')}</Button>
    {resource.error ? <Alert type="error" title={t(resource.error)} action={<Button onClick={resource.refresh}>{t('retry')}</Button>}/> : <Card loading={resource.loading} title={item ? formatCode(item.code) : t('details')} extra={<Button disabled={!item} onClick={() => { setEditing(item); form.setFieldsValue({ name: item?.name }); setOpen(true); }}>{t('rename')}</Button>}>
      {item && <Descriptions column={{ xs: 1, sm: 2 }} items={[
        { key: 'name', label: t('name'), children: item.name }, { key: 'state', label: t('online'), children: <Badge status={item.online ? 'success' : 'default'} text={t(item.online ? 'online' : 'offline')}/> },
        { key: 'system', label: t('system'), children: [item.os_name, item.os_version].filter(Boolean).join(' ') || t('unknown') },
        { key: 'arch', label: t('architecture'), children: item.architecture ?? t('unknown') }, { key: 'agent', label: t('agentVersion'), children: item.agent_version ?? t('unknown') },
        { key: 'created', label: t('created'), children: date(item.created_at) }, { key: 'seen', label: t('lastOnline'), children: item.online ? t('online') : item.last_online_at ? date(item.last_online_at) : '—' },
      ]}/>}
    </Card>}
    {item?.associated_with_me && <Button danger onClick={() => setUnlinking(item)}>{t('unlinkDevice')}</Button>}
    <Modal title={t('unlinkDevice')} open={!!unlinking} maskClosable={false} keyboard={false} closable={!busy} onCancel={() => !busy && setUnlinking(undefined)} confirmLoading={busy} onOk={async () => {
      if (!unlinking) return; setBusy(true);
      try { await api(`/devices/${unlinking.id}/association`, { method: 'DELETE', body: JSON.stringify({ revision: unlinking.association_revision }) }); setUnlinking(undefined); navigate('/devices'); }
      catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); resource.refresh(); } finally { setBusy(false); }
    }}>{t('unlinkDeviceHint')}</Modal>
    <Modal title={t('rename')} open={open} maskClosable={false} keyboard={false} onCancel={() => !busy && setOpen(false)} onOk={() => form.submit()} confirmLoading={busy} okText={t('save')} cancelText={t('cancel')}>
      <Typography.Paragraph type="secondary">{t('nameHint')}</Typography.Paragraph><Form form={form} layout="vertical" onFinish={rename}><Form.Item name="name" label={t('name')} rules={[{ required: true, whitespace: true, max: 128, message: t('invalid_input') }]}><Input maxLength={128}/></Form.Item></Form>
    </Modal>
  </Space>;
}
