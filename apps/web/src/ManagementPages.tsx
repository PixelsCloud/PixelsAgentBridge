import { paginationComponents } from './ConsoleSelect';
import { ConsoleSelect } from './ConsoleSelect';
import { useState } from 'react';
import { Alert, App, Button, Card, Form, Input, Modal, Space, Switch, Table, Tag, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useNavigate } from 'react-router-dom';
import { api, ApiError, type Viewer } from './api';
import { PageHeading } from './PageHeading';
import { useText } from './i18n';
import { RelayNodes } from './RelayNodes';
import { usePage } from './usePagedResource';

interface Account { id: string; username: string; status: string; server_admin: boolean; revision: number; relay_limit_mbps: number | null }


function DataPage<T extends { id: string }>({ title, resource, columns, extra, hint }: { title: string; resource: ReturnType<typeof usePage<T>>; columns: TableColumnsType<T>; extra?: React.ReactNode; hint?: string }) {
  const t = useText();
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}>
    <PageHeading title={title} description={hint ?? t('accountsPageHint')} actions={extra}/>
    <Card className="data-table-card"><Space wrap className="table-toolbar" style={{ marginBottom: 20 }}><Input.Search aria-label={title} key={resource.query} defaultValue={resource.query} onSearch={resource.search} allowClear maxLength={128}/><Button onClick={resource.refresh}>{t('refresh')}</Button></Space>
      {resource.error && <Alert type="error" title={t(resource.error)} showIcon style={{ marginBottom: 16 }}/>}
      <Table<T> rowKey="id" dataSource={resource.data?.items ?? []} columns={columns} loading={resource.loading} scroll={{ x: 800 }} locale={{ emptyText: t('noData') }} pagination={{ components: paginationComponents, current: resource.data?.page ?? 1, pageSize: resource.data?.page_size ?? 20, total: resource.data?.total ?? 0, showSizeChanger: true, pageSizeOptions: [20, 50, 100], showTotal: value => `${t('total')} ${value}`, onChange: resource.paginate }}/>
    </Card>
  </Space>;
}

export function AccountsPage({ liveRevision }: { liveRevision: number }) {
  const navigate = useNavigate();
  const t = useText(); const { message } = App.useApp(); const resource = usePage<Account>('/accounts', liveRevision);
  const [editing, setEditing] = useState<Account>(); const [assigning, setAssigning] = useState<Account>(); const [busy, setBusy] = useState(false);
  const [form] = Form.useForm(); const [assignment] = Form.useForm();
  async function submit(values: { status: string; server_admin: boolean }) {
    if (!editing) return; setBusy(true);
    try { await api(`/accounts/${editing.id}`, { method: 'PATCH', body: JSON.stringify({ ...values, revision: editing.revision }) }); setEditing(undefined); resource.refresh(); message.success(t('saved')); }
    catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); } finally { setBusy(false); }
  }
  async function assign(values: { mbps: number }) {
    if (!assigning) return; setBusy(true);
    try { await api(`/accounts/${assigning.id}/traffic`, { method: 'PUT', body: JSON.stringify({ mbps: values.mbps }) }); setAssigning(undefined); resource.refresh(); message.success(t('saved')); }
    catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); } finally { setBusy(false); }
  }
  return <><DataPage title={t('accounts')} resource={resource} columns={[
    { title: t('username'), render: (_, item) => <Button type="link" onClick={() => navigate(`/accounts/${item.id}`)}>{item.username}</Button> }, { title: t('role'), render: (_, item) => <Tag>{t(item.server_admin ? 'admin' : 'user')}</Tag> },
    { title: t('accountStatus'), render: (_, item) => <Tag color={item.status === 'active' ? 'success' : 'default'}>{t(item.status === 'active' ? 'enabled' : 'disabled')}</Tag> },
    { title: t('userBandwidth'), render: (_, item) => `${item.relay_limit_mbps ?? 10} Mbps` },
    { title: t('actions'), render: (_, item) => <Space><Button onClick={() => { form.setFieldsValue(item); setEditing(item); }}>{t('edit')}</Button><Button onClick={() => { assignment.setFieldsValue({ mbps: item.relay_limit_mbps ?? 10 }); setAssigning(item); }}>{t('limits')}</Button></Space> },
  ]}/>
    <Modal title={editing?.username} open={!!editing} maskClosable={false} keyboard={false} onCancel={() => !busy && setEditing(undefined)} onOk={() => form.submit()} confirmLoading={busy} okText={t('save')} cancelText={t('cancel')}><Alert type="info" title={t('accountUpdateHint')} style={{ marginBottom: 20 }}/><Form form={form} layout="vertical" onFinish={submit}>
      <Form.Item name="status" label={t('accountStatus')}><ConsoleSelect disabled={busy} options={[{ value: 'active', label: t('enabled') }, { value: 'disabled', label: t('disabled') }]}/></Form.Item><Form.Item name="server_admin" label={t('admin')} valuePropName="checked"><Switch/></Form.Item>
    </Form></Modal>
    <Modal title={t('userBandwidth')} open={!!assigning} maskClosable={false} keyboard={false} onCancel={() => !busy && setAssigning(undefined)} onOk={() => assignment.submit()} confirmLoading={busy} okText={t('save')} cancelText={t('cancel')}>
      <Typography.Paragraph>{t('userLimitHint')}</Typography.Paragraph>
      <Form form={assignment} layout="vertical" onFinish={assign} disabled={busy}>
        <Form.Item name="mbps" label="Mbps" rules={[{ required: true, message: t('required') }]}><ConsoleSelect disabled={busy} options={[5,10,20,30,40,50,60,70,80,90,100].map(value => ({ value, label: `${value} Mbps` }))}/></Form.Item>
      </Form>
    </Modal>
  </>;
}

export { AuditPage } from './AuditPage';

export function TrafficPage({ liveRevision, me }: { liveRevision: number; me: Viewer }) {
  const t = useText();
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}><PageHeading title={t('relay')} description={t('relayPageHint')}/>
    {me.server_admin && <RelayNodes liveRevision={liveRevision} viewerId={me.id}/>}
  </Space>;
}


export { ServiceSettings } from './ServiceSettings';
