import { useEffect, useRef, useState } from 'react';
import { Alert, App, Button, Card, Form, Input, Space, Table, Tag, Typography } from 'antd';
import { useSearchParams } from 'react-router-dom';
import { ApiError, post } from './api';
import { useResource } from './useResource';
import { useText, useDate } from './i18n';
import { formatCode } from './DevicePages';
interface Claim { id: string; device_code: string; name: string | null; status: string; created_at: number; expires_at: number }
export function ClaimsPage({ liveRevision }: { liveRevision: number }) {
  const t = useText(); const date = useDate(); const { message } = App.useApp(); const [params, setParams] = useSearchParams();
  const page = params.get('page') ?? '1'; const resource = useResource<{ items: Claim[]; total: number; page: number; page_size: number }>(`/claims?page=${encodeURIComponent(page)}`);
  const [busy, setBusy] = useState(false); const [error, setError] = useState<string>(); const request = useRef<{ code: string; id: string } | undefined>(undefined);
  useEffect(resource.refresh, [liveRevision]);
  useEffect(() => { if (resource.data?.items.some(item => item.id === request.current?.id && item.status !== 'pending')) request.current = undefined; }, [resource.data]);
  async function submit({ code }: { code: string }) {
    const clean = code.replace(/\s/g, '');
    if (request.current?.code !== clean) request.current = { code: clean, id: crypto.randomUUID() };
    setBusy(true); setError(undefined);
    try { await post('/claims', { device_code: clean, request_id: request.current!.id }); message.success(t('claimSent')); resource.refresh(); }
    catch (e) { setError(e instanceof ApiError ? e.code : 'networkError'); }
    finally { setBusy(false); }
  }
  async function cancel(item: Claim) { try { await post(`/claims/${item.id}/cancel`); resource.refresh(); if (request.current?.id === item.id) request.current = undefined; } catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); resource.refresh(); } }
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}><Typography.Title level={2}>{t('claim')}</Typography.Title>
    <Card><Typography.Paragraph>{t('claimHint')}</Typography.Paragraph>{error && <Alert type="error" title={t(error)} style={{ marginBottom: 16 }}/>}<Form layout="inline" onFinish={submit}>
      <Form.Item name="code" label={t('deviceCode')} rules={[{ required: true, pattern: /^\d{3}\s?\d{3}\s?\d{3}$/, message: t('invalid_input') }]}><Input placeholder="000 000 000" maxLength={11}/></Form.Item><Form.Item><Button type="primary" htmlType="submit" loading={busy}>{t('claim')}</Button></Form.Item>
    </Form></Card><Card extra={<Button onClick={resource.refresh}>{t('refresh')}</Button>}>
      {resource.error && <Alert type="error" title={t(resource.error)} style={{ marginBottom: 16 }}/>}
      <Table<Claim> rowKey="id" dataSource={resource.data?.items ?? []} loading={resource.loading} scroll={{ x: 720 }} pagination={{ current: resource.data?.page ?? 1, pageSize: 20, showSizeChanger: false, total: resource.data?.total ?? 0, showTotal: value => `${t('total')} ${value}`, onChange: value => setParams({ page: String(value) }) }} columns={[
        { title: t('deviceCode'), render: (_, item) => formatCode(item.device_code) }, { title: t('name'), render: (_, item) => item.name ?? '—' },
        { title: t('claim'), render: (_, item) => <Tag color={item.status === 'approved' ? 'success' : item.status === 'pending' ? 'processing' : 'default'}>{t(item.status)}</Tag> },
        { title: t('expires'), render: (_, item) => date(item.expires_at) }, { title: t('actions'), render: (_, item) => item.status === 'pending' && <Button onClick={() => cancel(item)}>{t('cancel')}</Button> },
      ]}/>
    </Card></Space>;
}
