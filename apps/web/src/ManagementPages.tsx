import { useEffect, useState } from 'react';
import { Alert, App, Button, Card, Descriptions, Form, Input, InputNumber, Modal, Select, Space, Switch, Table, Tag, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { api, ApiError, post, type Viewer } from './api';
import { useResource } from './useResource';
import { useText, useDate } from './i18n';

interface Page<T> { items: T[]; total: number; page: number; page_size: number }
interface Account { id: string; username: string; status: string; server_admin: boolean; revision: number; default_team_id: string | null; default_team_name: string | null }
interface Team { id: string; name: string; total_mbps: number; member_mbps: number; role: string | null; member_count: number }
interface Member { id: string; username: string; role: string; status: string }

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

function AccountPicker({ value, onChange, id }: { value?: string; onChange?: (value: string) => void; id?: string }) {
  const [search, setSearch] = useState(''); const resource = useResource<Page<Account>>(`/accounts?q=${encodeURIComponent(search)}&page_size=100`);
  return <Select id={id} value={value} onChange={onChange} showSearch={{ filterOption: false, onSearch: setSearch }} loading={resource.loading} options={resource.data?.items.filter(v => v.status === 'active').map(v => ({ value: v.id, label: v.username }))}/>;
}

export function AccountsPage({ liveRevision }: { liveRevision: number }) {
  const t = useText(); const { message } = App.useApp(); const resource = usePage<Account>('/accounts', liveRevision);
  const [editing, setEditing] = useState<Account>(); const [assigning, setAssigning] = useState<Account>(); const [busy, setBusy] = useState(false);
  const [form] = Form.useForm(); const [assignment] = Form.useForm();
  const [teamSearch, setTeamSearch] = useState('');
  const teams = useResource<Page<Team>>(assigning ? `/accounts/${assigning.id}/teams?page_size=100&q=${encodeURIComponent(teamSearch)}` : '/teams?page_size=1');
  async function submit(values: { status: string; server_admin: boolean }) {
    if (!editing) return; setBusy(true);
    try { await api(`/accounts/${editing.id}`, { method: 'PATCH', body: JSON.stringify({ ...values, revision: editing.revision }) }); setEditing(undefined); resource.refresh(); message.success(t('saved')); }
    catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); } finally { setBusy(false); }
  }
  async function assign(values: { team_id?: string }) {
    if (!assigning) return; setBusy(true);
    try { await post(`/accounts/${assigning.id}/traffic`, { team_id: values.team_id || null }); setAssigning(undefined); resource.refresh(); message.success(t('saved')); }
    catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); } finally { setBusy(false); }
  }
  return <><DataPage title={t('accounts')} resource={resource} columns={[
    { title: t('username'), dataIndex: 'username' }, { title: t('role'), render: (_, item) => <Tag>{t(item.server_admin ? 'admin' : 'user')}</Tag> },
    { title: t('account'), render: (_, item) => <Tag color={item.status === 'active' ? 'success' : 'default'}>{t(item.status === 'active' ? 'enabled' : 'disabled')}</Tag> },
    { title: t('defaultTeam'), render: (_, item) => item.default_team_name ?? t('personal') },
    { title: t('actions'), render: (_, item) => <Space><Button onClick={() => { form.setFieldsValue(item); setEditing(item); }}>{t('edit')}</Button><Button onClick={() => { assignment.setFieldsValue({ team_id: item.default_team_id ?? '' }); teams.refresh(); setAssigning(item); }}>{t('defaultTeam')}</Button></Space> },
  ]}/>
    <Modal title={editing?.username} open={!!editing} maskClosable={false} keyboard={false} onCancel={() => !busy && setEditing(undefined)} onOk={() => form.submit()} confirmLoading={busy} okText={t('save')} cancelText={t('cancel')}><Alert type="info" title={t('accountUpdateHint')} style={{ marginBottom: 20 }}/><Form form={form} layout="vertical" onFinish={submit}>
      <Form.Item name="status" label={t('account')}><Select options={[{ value: 'active', label: t('enabled') }, { value: 'disabled', label: t('disabled') }]}/></Form.Item><Form.Item name="server_admin" label={t('admin')} valuePropName="checked"><Switch/></Form.Item>
    </Form></Modal>
    <Modal title={t('defaultTeam')} open={!!assigning} maskClosable={false} keyboard={false} onCancel={() => !busy && setAssigning(undefined)} onOk={() => assignment.submit()} confirmLoading={busy} okText={t('save')} cancelText={t('cancel')}><Typography.Paragraph>{t('assignmentHint')}</Typography.Paragraph>{teams.error && <Alert type="error" title={t(teams.error)}/>}<Form form={assignment} onFinish={assign}><Form.Item name="team_id" label={t('defaultTeam')}><Select showSearch={{ filterOption: false, onSearch: setTeamSearch }} loading={teams.loading} options={[{ value: '', label: t('personal') }, ...(teams.data?.items ?? []).map(v => ({ value: v.id, label: v.name }))]}/></Form.Item></Form></Modal>
  </>;
}

export function TeamsPage({ me, liveRevision }: { me: Viewer; liveRevision: number }) {
  const t = useText(); const { message } = App.useApp(); const navigate = useNavigate(); const resource = usePage<Team>('/teams', liveRevision);
  const [open, setOpen] = useState(false); const [editing, setEditing] = useState<Team>(); const [busy, setBusy] = useState(false); const [form] = Form.useForm();
  const [editMode, setEditMode] = useState<'name' | 'limits'>('name');
  async function submit(values: { name: string; owner_id: string; total_mbps: number; member_mbps: number }) {
    setBusy(true);
    try {
      if (editing) { await post(`/teams/${editing.id}/actions`, editMode === 'name' ? { action: 'rename', name: values.name } : { action: 'set_limits', total_mbps: values.total_mbps, member_mbps: values.member_mbps }); }
      else await post('/teams', { name: values.name, owner_id: values.owner_id });
      setOpen(false); resource.refresh(); message.success(t('saved'));
    } catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); resource.refresh(); } finally { setBusy(false); }
  }
  return <><DataPage title={t('teams')} hint={t('teamHint')} resource={resource} extra={me.server_admin && <Button type="primary" onClick={() => { setEditing(undefined); form.resetFields(); setOpen(true); }}>{t('createTeam')}</Button>} columns={[
    { title: t('name'), dataIndex: 'name' }, { title: t('totalBandwidth'), render: (_, item) => `${item.total_mbps} Mbps` }, { title: t('memberBandwidth'), render: (_, item) => `${item.member_mbps} Mbps` },
    { title: t('members'), dataIndex: 'member_count' },
    ...(me.server_admin ? [{ title: t('actions'), key: 'actions', render: (_: unknown, item: Team) => <Space><Button onClick={() => { setEditing(item); setEditMode('name'); form.setFieldsValue(item); setOpen(true); }}>{t('rename')}</Button><Button onClick={() => { setEditing(item); setEditMode('limits'); form.setFieldsValue(item); setOpen(true); }}>{t('limits')}</Button><Button onClick={() => navigate(`/teams/${item.id}`)}>{t('members')}</Button></Space> }] : [{ title: t('role'), key: 'role', render: (_: unknown, item: Team) => t(item.role === 'owner' ? 'teamOwnerRole' : item.role === 'admin' ? 'teamAdmin' : 'member') }]),
  ]}/>
    <Modal title={t(editing ? 'edit' : 'createTeam')} open={open} maskClosable={false} keyboard={false} onCancel={() => !busy && setOpen(false)} onOk={() => form.submit()} confirmLoading={busy} okText={t('save')} cancelText={t('cancel')}><Form form={form} layout="vertical" onFinish={submit}>
      {(!editing || editMode === 'name') && <Form.Item name="name" label={t('teamName')} rules={[{ required: true, whitespace: true, max: 128, message: t('invalid_input') }]}><Input maxLength={128}/></Form.Item>}
      {!editing && <Form.Item name="owner_id" label={t('teamOwner')} rules={[{ required: true, message: t('required') }]}><AccountPicker/></Form.Item>}
      {editing && editMode === 'limits' && <><Form.Item name="total_mbps" label={`${t('totalBandwidth')} (Mbps)`} rules={[{ required: true, message: t('required') }]}><InputNumber min={1} max={2147483647} precision={0}/></Form.Item><Form.Item name="member_mbps" label={`${t('memberBandwidth')} (Mbps)`} rules={[{ required: true, message: t('required') }]}><InputNumber min={1} max={2147483647} precision={0}/></Form.Item></>}
    </Form></Modal>
  </>;
}

export function TeamMembersPage({ liveRevision }: { liveRevision: number }) {
  const { id } = useParams(); const t = useText(); const { message, modal } = App.useApp(); const resource = usePage<Member>(`/teams/${id}/members`, liveRevision);
  const [open, setOpen] = useState(false); const [busy, setBusy] = useState(false); const [form] = Form.useForm();
  async function add(values: { user_id: string; role: string }) { setBusy(true); try { await post(`/teams/${id}/actions`, { action: 'add_member', ...values }); setOpen(false); resource.refresh(); message.success(t('saved')); } catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); } finally { setBusy(false); } }
  const remove = (item: Member) => modal.confirm({ title: t('removeMember'), content: t('removeConfirm'), maskClosable: false, keyboard: false, okText: t('confirm'), cancelText: t('cancel'), onOk: async () => { try { await post(`/teams/${id}/actions`, { action: 'remove_member', user_id: item.id }); resource.refresh(); } catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); throw e; } } });
  return <><DataPage title={t('members')} resource={resource} extra={<Button type="primary" onClick={() => { form.resetFields(); form.setFieldValue('role', 'member'); setOpen(true); }}>{t('addMember')}</Button>} columns={[
    { title: t('username'), dataIndex: 'username' }, { title: t('role'), render: (_, item) => t(item.role === 'owner' ? 'teamOwnerRole' : item.role === 'admin' ? 'teamAdmin' : 'member') },
    { title: t('actions'), render: (_, item) => <Button danger disabled={item.role === 'owner'} onClick={() => remove(item)}>{t('removeMember')}</Button> },
  ]}/><Modal title={t('addMember')} open={open} maskClosable={false} keyboard={false} onCancel={() => !busy && setOpen(false)} onOk={() => form.submit()} confirmLoading={busy} okText={t('save')} cancelText={t('cancel')}><Form layout="vertical" form={form} onFinish={add}>
    <Form.Item name="user_id" label={t('username')} rules={[{ required: true, message: t('required') }]}><AccountPicker/></Form.Item><Form.Item name="role" label={t('role')}><Select options={[{ value: 'member', label: t('member') }, { value: 'admin', label: t('teamAdmin') }]}/></Form.Item>
  </Form></Modal></>;
}

export function AuditPage({ liveRevision }: { liveRevision: number }) {
  const t = useText(); const date = useDate(); const resource = usePage<{ id: string; actor: string | null; action: string; resource_id: string | null; resource_name: string | null; created_at: number }>('/audit', liveRevision);
  return <DataPage title={t('events')} resource={resource} columns={[{ title: t('time'), render: (_, item) => date(item.created_at) }, { title: t('actor'), render: (_, item) => item.actor ?? t('deviceActor') }, { title: t('action'), render: (_, item) => t(`audit.${item.action}`) }, { title: t('resource'), render: (_, item) => item.resource_name ?? item.resource_id, ellipsis: true }]}/>;
}

export function TrafficPage({ liveRevision, me }: { liveRevision: number; me: Viewer }) {
  const t = useText();
  const resource = useResource<{ scopes: { personal_mbps: number; default_tenant_id: string; personal_tenant_id: string; teams: { tenant_id: string; name: string; total_mbps: number; member_mbps: number }[] } }>('/traffic');
  useEffect(resource.refresh, [liveRevision]);
  const scopes = resource.data?.scopes; const active = scopes?.teams.find(v => v.tenant_id === scopes.default_tenant_id);
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}><Typography.Title level={2}>{t('relay')}</Typography.Title><Typography.Paragraph type="secondary">{t('relayHint')}</Typography.Paragraph>
    {resource.error && <Alert type="error" title={t(resource.error)} action={<Button onClick={resource.refresh}>{t('retry')}</Button>}/>}
    <Card title={t('limits')} loading={resource.loading}><Descriptions items={[{ key: 'scope', label: t('effectiveScope'), children: active?.name ?? t('personal') }, { key: 'member', label: t('memberBandwidth'), children: scopes ? `${active?.member_mbps ?? scopes.personal_mbps} Mbps` : '—' }, ...(active ? [{ key: 'total', label: t('totalBandwidth'), children: `${active.total_mbps} Mbps` }] : [])]}/></Card>
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
  const t = useText();
  const resource = useResource<{ registration_enabled: boolean; default_team_mbps: number; default_member_mbps: number; default_personal_mbps: number; policy_revision: number; session_hours: number }>('/service');
  const value = resource.data;
  return <Card title={t('service')} loading={resource.loading}>{resource.error ? <Alert type="error" title={t(resource.error)} action={<Button onClick={resource.refresh}>{t('retry')}</Button>}/> : value && <Descriptions column={1} items={[
    { key:'register', label:t('registration'), children:t(value.registration_enabled ? 'enabled' : 'disabled') },
    { key:'team', label:t('totalBandwidth'), children:`${value.default_team_mbps} Mbps` },
    { key:'member', label:t('memberBandwidth'), children:`${value.default_member_mbps} Mbps` },
    { key:'personal', label:t('personal'), children:`${value.default_personal_mbps} Mbps` },
    { key:'policy', label:t('policyOffered'), children:value.policy_revision },
    { key:'session', label:t('sessionDuration'), children:`${value.session_hours} h` },
  ]}/>}</Card>;
}
