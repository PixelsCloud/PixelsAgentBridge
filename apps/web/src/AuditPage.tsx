import { ConsoleSelect } from './ConsoleSelect';
import { useEffect, useState } from 'react';
import { Alert, Button, Card, Descriptions, Form, Input, Space, Table, Tag, Typography } from 'antd';
import { useSearchParams } from 'react-router-dom';
import { ApiError } from './api';
import { useDate, useText } from './i18n';
import { PageHeading } from './PageHeading';
import { useResource } from './useResource';
import { localDateTime, parseLocalDateTime } from './usageModel';
import { auditActions, auditFilters, auditQuery, type AuditEvent, type AuditPageResult } from './auditModel';

export function AuditPage() {
  const t = useText(); const date = useDate(); const [params, setParams] = useSearchParams(); const [form] = Form.useForm();
  const query = auditQuery(params); const resource = useResource<AuditPageResult>(`/audit?${query}`);
  const filters = new URLSearchParams(query); filters.delete('cursor'); const filterStamp = filters.toString();
  const [exporting, setExporting] = useState(false); const [exportError, setExportError] = useState<string>(); const [truncated, setTruncated] = useState(false);
  useEffect(() => {
    form.resetFields();
    form.setFieldsValue({ ...Object.fromEntries(params), start: params.has('from') ? localDateTime(Number(params.get('from'))) : '', end: params.has('until') ? localDateTime(Number(params.get('until'))) : '' });
    setExportError(undefined); setTruncated(false);
  }, [filterStamp, form]);
  useEffect(() => {
    if (!params.has('cursor') && resource.data?.snapshot_cursor) { const next = new URLSearchParams(params); next.set('cursor', resource.data.snapshot_cursor); setParams(next, { replace: true }); }
  }, [resource.data, params, setParams]);
  const refresh = () => { const next = new URLSearchParams(params); next.delete('cursor'); next.delete('page'); setTruncated(false); setExportError(undefined); setParams(next); resource.refresh(); };
  async function apply() {
    const values = await form.validateFields();
    const from = values.start ? parseLocalDateTime(values.start) : undefined; const until = values.end ? parseLocalDateTime(values.end) : undefined;
    if ((values.start && from === undefined) || (values.end && until === undefined) || (from !== undefined && until !== undefined && until <= from)) {
      form.setFields([{ name: 'end', errors: [t('invalid_input')] }]); return;
    }
    setParams(auditFilters(new URLSearchParams(), { ...values, from: from?.toString() ?? '', until: until?.toString() ?? '', page_size: params.get('page_size') ?? '20' }));
    setExportError(undefined); setTruncated(false);
  }
  async function download() {
    setExporting(true); setExportError(undefined); setTruncated(false);
    try {
      const response = await fetch(`/api/web/audit/export?${query}`, { credentials: 'same-origin' });
      if (!response.ok) {
        const error = await response.json().catch(() => ({ code: 'server_error' }));
        if (response.status === 401) globalThis.dispatchEvent(new Event('pab-session-expired'));
        throw new ApiError(response.status, error.code ?? 'server_error');
      }
      const blob = await response.blob(); const url = URL.createObjectURL(blob); const link = document.createElement('a');
      link.href = url; link.download = 'pixels-audit.csv'; document.body.append(link); link.click(); link.remove(); globalThis.setTimeout(() => URL.revokeObjectURL(url), 1000);
      setTruncated(response.headers.get('x-audit-truncated') === 'true');
    } catch (error) { setExportError(error instanceof ApiError ? error.code : 'networkError'); }
    finally { setExporting(false); }
  }
  const value = (v: unknown, key?: string) => {
    if (v === undefined || v === null) return '—';
    if (key === 'password_changed') return t(v ? 'auditPasswordChanged' : 'auditNotChanged');
    if (key === 'error_code' && typeof v === 'string') return t(v);
    if (key === 'scope' && typeof v === 'string') return t(`auditScope.${v}`);
    if (key === 'registration_source' && typeof v === 'string') return t(`policySource.${v}`);
    if (key === 'population' && typeof v === 'string') return t(`usagePopulation_${v}`);
    if (key === 'interval' && typeof v === 'string') return t(v === 'lifetime' ? 'lifetime' : `usageInterval.${v}`);
    if ((key === 'from' || key === 'until') && typeof v === 'number') return date(v);
    return typeof v === 'boolean' ? t(v ? 'enabled' : 'disabled') : typeof v === 'object' ? JSON.stringify(v, null, 2) : String(v);
  };
  const expanded = (item: AuditEvent) => <Space orientation="vertical" style={{ width: '100%' }}>
    <Descriptions column={{ xs: 1, md: 2 }} items={[
      { key: 'request', label: t('auditRequestId'), children: item.request_id ? <Typography.Text copyable>{item.request_id}</Typography.Text> : t('auditNotCollected') },
      { key: 'operation', label: t('auditOperationId'), children: item.operation_id ? <Typography.Text copyable>{item.operation_id}</Typography.Text> : '—' },
      { key: 'event', label: t('auditEventId'), children: <Typography.Text copyable>{item.id}</Typography.Text> },
      { key: 'time', label: t('time'), children: new Date(item.created_at).toISOString() },
    ]}/>
    {item.change_state === 'historical_unknown' ? <Alert type="info" title={t('auditHistoricalUnknown')}/> : item.change_state === 'not_applicable' ? <Typography.Text type="secondary">{t('auditNoFieldChange')}</Typography.Text> :
      <Table size="small" pagination={false} rowKey="field" scroll={{ x: 480 }} dataSource={[...new Set([...Object.keys(item.before ?? {}), ...Object.keys(item.after ?? {})])].map(field => ({ field, before: item.before?.[field], after: item.after?.[field] }))} columns={[
        { title: t('auditField'), render: (_, row) => t(`auditField.${row.field}`) },
        { title: t('auditBefore'), render: (_, row) => <Typography.Text style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{value(row.before, row.field)}</Typography.Text> },
        { title: t('auditAfter'), render: (_, row) => <Typography.Text style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{value(row.after, row.field)}</Typography.Text> },
      ]}/>}
    {item.details && Object.keys(item.details).length > 0 && <Descriptions column={1} items={Object.entries(item.details).map(([key, itemValue]) => ({ key, label: t(`auditDetail.${key}`), children: <Typography.Text style={{ overflowWrap: 'anywhere' }}>{value(itemValue, key)}</Typography.Text> }))}/>}
  </Space>;
  const data = resource.error ? undefined : resource.data;
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}>
    <PageHeading title={t('events')} description={t('auditPageHint')}/>
    <Card>
      <Form form={form} layout="vertical" onFinish={() => void apply()}>
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(200px, 100%), 1fr))', gap: '0 16px' }}>
          <Form.Item name="action" label={t('action')}><ConsoleSelect allowClear options={auditActions.map(action => ({ value: action, label: t(`audit.${action}`) }))}/></Form.Item>
          <Form.Item name="result" label={t('auditResult')}><ConsoleSelect allowClear options={['succeeded', 'denied', 'failed', 'historical_unknown'].map(result => ({ value: result, label: t(`auditResult.${result}`) }))}/></Form.Item>
          <Form.Item name="resource_kind" label={t('auditResourceKind')}><ConsoleSelect allowClear options={['account', 'device', 'relay', 'server', 'audit', 'unknown'].map(kind => ({ value: kind, label: t(`auditResource.${kind}`) }))}/></Form.Item>
          <Form.Item name="resource" label={t('auditResourceKey')}><Input maxLength={128} allowClear/></Form.Item>
          <Form.Item name="start" label={t('auditFrom')}><Input type="datetime-local" step={60}/></Form.Item>
          <Form.Item name="end" label={t('auditUntil')}><Input type="datetime-local" step={60}/></Form.Item>
        </div>
        <Space wrap><Button type="primary" htmlType="submit">{t('auditApply')}</Button><Button onClick={() => { form.resetFields(); setParams(new URLSearchParams()); }}>{t('auditClear')}</Button><Button onClick={refresh}>{t('refresh')}</Button><Button loading={exporting} disabled={!!resource.error || resource.loading || !data} onClick={() => void download()}>{t('auditExport')}</Button></Space>
      </Form>
    </Card>
    {resource.error && <Alert type="error" title={t(resource.error)}/>}
    {exportError && <Alert type="error" title={t('auditExportFailed')} description={t(exportError)}/>}
    {truncated && <Alert type="warning" title={t('auditExportTruncated')} showIcon/>}
    <Card>
      <Table<AuditEvent> rowKey="id" loading={resource.loading} dataSource={data?.items ?? []} scroll={{ x: 1050 }} pagination={false} expandable={{ expandedRowRender: expanded }} columns={[
        { title: t('time'), render: (_, item) => date(item.created_at) },
        { title: t('actor'), render: (_, item) => <Space orientation="vertical"><Typography.Text>{item.actor ?? t('auditNotCollected')}</Typography.Text>{item.actor_id && <Typography.Text copyable type="secondary">{item.actor_id}</Typography.Text>}</Space> },
        { title: t('action'), render: (_, item) => t(`audit.${item.action}`) },
        { title: t('auditResult'), render: (_, item) => <Tag color={item.result === 'succeeded' ? 'success' : item.result === 'denied' ? 'warning' : item.result === 'failed' ? 'error' : 'default'}>{t(`auditResult.${item.result}`)}</Tag> },
        { title: t('resource'), render: (_, item) => <Space orientation="vertical"><Typography.Text>{item.resource_name ?? t('auditNotCollected')}</Typography.Text><Typography.Text type="secondary" style={{ overflowWrap: 'anywhere' }}>{item.resource_key ?? item.resource_id ?? '—'}</Typography.Text></Space> },
      ]}/>
      <Space wrap style={{ marginTop: 16 }}><Typography.Text>{t('total')} {data?.total ?? '—'}</Typography.Text><ConsoleSelect aria-label={t('auditPageSize')} value={Number(params.get('page_size') ?? 20)} options={[20, 50, 100].map(value => ({ value, label: String(value) }))} onChange={value => setParams(auditFilters(params, { page_size: String(value) }))}/><Button disabled={!data?.previous_cursor} onClick={() => { const next = new URLSearchParams(params); next.set('cursor', data!.previous_cursor!); setParams(next); }}>{t('auditPrevious')}</Button><Button disabled={!data?.next_cursor} onClick={() => { const next = new URLSearchParams(params); next.set('cursor', data!.next_cursor!); setParams(next); }}>{t('auditNext')}</Button></Space>
    </Card>
  </Space>;
}
