import { useEffect, useState } from 'react';
import { Alert, App, Button, Card, Descriptions, Form, Input, InputNumber, Modal, Select, Space, Switch, Table, Tag, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useSearchParams } from 'react-router-dom';
import { api, ApiError, type Viewer } from './api';
import { useResource } from './useResource';
import { useText, useDate } from './i18n';

interface Page<T> { items: T[]; total: number; page: number; page_size: number }
interface Account { id: string; username: string; status: string; server_admin: boolean; revision: number; relay_limit_mbps: number | null }

function usePage<T>(path: string, liveRevision: number) {
  const [params, setParams] = useSearchParams();
  const query = new URLSearchParams();
  for (const key of ['q', 'page', 'page_size']) { const value = params.get(key); if (value) query.set(key, value); }
  const resource = useResource<Page<T>>(`${path}?${query}`);
  useEffect(resource.refresh, [liveRevision]);
  const search = (value: string) => { const next = new URLSearchParams(params); value ? next.set('q', value) : next.delete('q'); next.delete('page'); setParams(next); };
  const paginate = (page: number, size: number) => { const next = new URLSearchParams(params); next.set('page', String(page)); next.set('page_size', String(size)); setParams(next); };
  return { ...resource, search, paginate, query: params.get('q') ?? '' };
}

function DataPage<T extends { id: string }>({ title, resource, columns, extra, hint }: { title: string; resource: ReturnType<typeof usePage<T>>; columns: TableColumnsType<T>; extra?: React.ReactNode; hint?: string }) {
  const t = useText();
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}>
    <div className="page-heading"><div><Typography.Title level={2}>{title}</Typography.Title>{hint && <Typography.Text type="secondary">{hint}</Typography.Text>}</div>{extra}</div>
    <Card><Space style={{ marginBottom: 20 }}><Input.Search aria-label={title} key={resource.query} defaultValue={resource.query} onSearch={resource.search} allowClear maxLength={128}/><Button onClick={resource.refresh}>{t('refresh')}</Button></Space>
      {resource.error && <Alert type="error" title={t(resource.error)} showIcon style={{ marginBottom: 16 }}/>}
      <Table<T> rowKey="id" dataSource={resource.data?.items ?? []} columns={columns} loading={resource.loading} scroll={{ x: 800 }} locale={{ emptyText: t('noData') }} pagination={{ current: resource.data?.page ?? 1, pageSize: resource.data?.page_size ?? 20, total: resource.data?.total ?? 0, showSizeChanger: true, pageSizeOptions: [20, 50, 100], showTotal: value => `${t('total')} ${value}`, onChange: resource.paginate }}/>
    </Card>
  </Space>;
}

export function AccountsPage({ liveRevision }: { liveRevision: number }) {
  const t = useText(); const { message } = App.useApp(); const resource = usePage<Account>('/accounts', liveRevision);
  const [editing, setEditing] = useState<Account>(); const [assigning, setAssigning] = useState<Account>(); const [busy, setBusy] = useState(false);
  const [form] = Form.useForm(); const [assignment] = Form.useForm();
  async function submit(values: { status: string; server_admin: boolean }) {
    if (!editing) return; setBusy(true);
    try { await api(`/accounts/${editing.id}`, { method: 'PATCH', body: JSON.stringify({ ...values, revision: editing.revision }) }); setEditing(undefined); resource.refresh(); message.success(t('saved')); }
    catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); } finally { setBusy(false); }
  }
  async function assign(values: { mbps?: number }) {
    if (!assigning) return; setBusy(true);
    try { await api(`/accounts/${assigning.id}/traffic`, { method: 'PUT', body: JSON.stringify({ mbps: values.mbps ?? null }) }); setAssigning(undefined); resource.refresh(); message.success(t('saved')); }
    catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); } finally { setBusy(false); }
  }
  return <><DataPage title={t('accounts')} resource={resource} columns={[
    { title: t('username'), dataIndex: 'username' }, { title: t('role'), render: (_, item) => <Tag>{t(item.server_admin ? 'admin' : 'user')}</Tag> },
    { title: t('account'), render: (_, item) => <Tag color={item.status === 'active' ? 'success' : 'default'}>{t(item.status === 'active' ? 'enabled' : 'disabled')}</Tag> },
    { title: t('userBandwidth'), render: (_, item) => item.relay_limit_mbps == null ? t('defaultLimit') : `${item.relay_limit_mbps} Mbps` },
    { title: t('actions'), render: (_, item) => <Space><Button onClick={() => { form.setFieldsValue(item); setEditing(item); }}>{t('edit')}</Button><Button onClick={() => { assignment.setFieldsValue({ mbps: item.relay_limit_mbps }); setAssigning(item); }}>{t('limits')}</Button></Space> },
  ]}/>
    <Modal title={editing?.username} open={!!editing} maskClosable={false} keyboard={false} onCancel={() => !busy && setEditing(undefined)} onOk={() => form.submit()} confirmLoading={busy} okText={t('save')} cancelText={t('cancel')}><Alert type="info" title={t('accountUpdateHint')} style={{ marginBottom: 20 }}/><Form form={form} layout="vertical" onFinish={submit}>
      <Form.Item name="status" label={t('account')}><Select options={[{ value: 'active', label: t('enabled') }, { value: 'disabled', label: t('disabled') }]}/></Form.Item><Form.Item name="server_admin" label={t('admin')} valuePropName="checked"><Switch/></Form.Item>
    </Form></Modal>
    <Modal title={t('userBandwidth')} open={!!assigning} maskClosable={false} keyboard={false} onCancel={() => !busy && setAssigning(undefined)} onOk={() => assignment.submit()} confirmLoading={busy} okText={t('save')} cancelText={t('cancel')}><Typography.Paragraph>{t('userLimitHint')}</Typography.Paragraph><Form form={assignment} onFinish={assign}><Form.Item name="mbps" label="Mbps"><InputNumber min={1} max={2147483647} precision={0} placeholder={t('defaultLimit')}/></Form.Item></Form></Modal>
  </>;
}

export function AuditPage({ liveRevision }: { liveRevision: number }) {
  const t = useText(); const date = useDate(); const resource = usePage<{ id: string; actor: string | null; action: string; resource_id: string | null; resource_name: string | null; created_at: number }>('/audit', liveRevision);
  return <DataPage title={t('events')} resource={resource} columns={[{ title: t('time'), render: (_, item) => date(item.created_at) }, { title: t('actor'), render: (_, item) => item.actor ?? t('deviceActor') }, { title: t('action'), render: (_, item) => t(`audit.${item.action}`) }, { title: t('resource'), render: (_, item) => item.resource_name ?? item.resource_id, ellipsis: true }]}/>;
}

export function TrafficPage({ liveRevision, me }: { liveRevision: number; me: Viewer }) {
  const t = useText();
  const resource = useResource<{ user_mbps: number; guest_mbps: number }>('/traffic');
  useEffect(resource.refresh, [liveRevision]);
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}><Typography.Title level={2}>{t('relay')}</Typography.Title><Typography.Paragraph type="secondary">{t('relayHint')}</Typography.Paragraph>
    {resource.error && <Alert type="error" title={t(resource.error)} action={<Button onClick={resource.refresh}>{t('retry')}</Button>}/>}
    <Card title={t('limits')} loading={resource.loading}><Descriptions items={[{ key: 'user', label: t('userBandwidth'), children: resource.data ? `${resource.data.user_mbps} Mbps` : '—' }, { key: 'guest', label: t('guestBandwidth'), children: resource.data ? `${resource.data.guest_mbps} Mbps` : '—' }]}/></Card>
    {me.server_admin && <RelayNodes liveRevision={liveRevision}/>}
  </Space>;
}

function RelayNodes({ liveRevision }: { liveRevision: number }) {
  const t = useText(); const date = useDate();
  type Node = { id: string; online: boolean; agent_version: string | null; applied_policy_version: number | null; offered_policy_version: number; last_seen: number };
  const resource = usePage<Node>('/relays', liveRevision);
  return <Card title={t('nodes')} extra={<Button onClick={resource.refresh}>{t('refresh')}</Button>}><Typography.Paragraph type="secondary">{t('relayStatusHint')}</Typography.Paragraph>
    {resource.error && <Alert type="error" title={t(resource.error)}/>}
    <Table<Node> rowKey="id" dataSource={resource.data?.items ?? []} loading={resource.loading} pagination={{ current: resource.data?.page ?? 1, pageSize: resource.data?.page_size ?? 20, total: resource.data?.total ?? 0, showSizeChanger: true, pageSizeOptions: [20,50,100], onChange: resource.paginate }} scroll={{ x: 700 }} columns={[
      { title: t('name'), dataIndex: 'id' }, { title: t('controlStatus'), render: (_, item) => <Tag color={item.online ? 'success' : 'default'}>{t(item.online ? 'online' : 'offline')}</Tag> },
      { title: t('version'), render: (_, item) => item.agent_version ?? t('unknown') }, { title: t('policyApplied'), render: (_, item) => item.applied_policy_version ?? t('unknown') },
      { title: t('policyOffered'), dataIndex: 'offered_policy_version' }, { title: t('lastSeen'), render: (_, item) => date(item.last_seen) },
    ]}/>
  </Card>;
}

export function ServiceSettings() {
  const t = useText(); const { message } = App.useApp(); const [busy, setBusy] = useState(false); const [form] = Form.useForm();
  const resource = useResource<{ registration_enabled: boolean; default_user_mbps: number; default_guest_mbps: number; policy_revision: number }>('/service');
  const value = resource.data;
  useEffect(() => { if (value) form.setFieldsValue({ user_mbps: value.default_user_mbps, guest_mbps: value.default_guest_mbps }); }, [value, form]);
  async function save(limits: { user_mbps: number; guest_mbps: number }) {
    setBusy(true);
    try { await api('/service', { method: 'PUT', body: JSON.stringify(limits) }); resource.refresh(); message.success(t('saved')); }
    catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); } finally { setBusy(false); }
  }
  return <Card title={t('service')} loading={resource.loading}>{resource.error ? <Alert type="error" title={t(resource.error)} action={<Button onClick={resource.refresh}>{t('retry')}</Button>}/> : value && <>
    <Descriptions column={1} items={[
      { key:'register', label:t('registration'), children:t(value.registration_enabled ? 'enabled' : 'disabled') },
      { key:'policy', label:t('policyOffered'), children:value.policy_revision },
      { key:'session', label:t('sessionDuration'), children:t('permanentSession') },
    ]}/>
    <Form form={form} layout="vertical" onFinish={save} disabled={busy}>
      <Form.Item name="user_mbps" label={`${t('userBandwidth')} (Mbps)`} rules={[{ required: true, message: t('required') }]}><InputNumber min={1} max={2147483647} precision={0}/></Form.Item>
      <Form.Item name="guest_mbps" label={`${t('guestBandwidth')} (Mbps)`} rules={[{ required: true, message: t('required') }]}><InputNumber min={1} max={2147483647} precision={0}/></Form.Item>
      <Button type="primary" htmlType="submit" loading={busy}>{t('save')}</Button>
    </Form>
  </>}</Card>;
}
