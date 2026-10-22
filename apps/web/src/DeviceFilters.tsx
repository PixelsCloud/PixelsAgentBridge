import { useEffect, useState } from 'react';
import { Alert, Button, Form, Input, Tag } from 'antd';
import { Filter, RotateCcw } from 'lucide-react';
import { useText } from './i18n';
import { applyDeviceFilters, deviceFilterKeys, deviceFilterValues, deviceQuery, type DeviceFilterValues } from './deviceDiagnosticsModel';
import { useLiveRefresh, useResource } from './useResource';

export function DeviceFilters({ params, admin, liveRevision, onApply, onRemove }: {
  params: URLSearchParams; admin: boolean; liveRevision: number; onApply: (params: URLSearchParams) => void; onRemove: (key: string) => void;
}) {
  const t = useText(); const [form] = Form.useForm<DeviceFilterValues>();
  const draftVersion = Form.useWatch('version', form);
  const [dirty, setDirty] = useState(false);
  const applied = deviceFilterValues(params);
  const query = params.toString();
  // Keep every version in the account scope available while other filters change.
  const versionScope = deviceQuery(params, admin, true);
  for (const key of deviceFilterKeys) versionScope.delete(key);
  const versions = useResource<{ versions: { value: string | null; count: number }[]; versions_truncated: boolean }>(`/devices/diagnostics?${versionScope}`);
  useLiveRefresh(versions.refresh, liveRevision);
  const versionValues = [...new Set([...(versions.data?.versions ?? []).flatMap(group => group.value ? [group.value] : []), ...[applied.version, draftVersion].filter((value): value is string => !!value)])];
  useEffect(() => { form.setFieldsValue(deviceFilterValues(new URLSearchParams(query))); setDirty(false); }, [query, form]);
  const labels = { q: t('search'), status: t('online'), system: t('system'), architecture: t('architecture'), version_state: t('diagnosticsVersionState'), version: t('agentVersion') };
  const systems: Record<string, string> = { windows: 'Windows', linux: 'Linux', macos: 'macOS', android: 'Android' };
  const display = (key: string, value: string) => value === 'unknown' ? t('unknown') : key === 'status' ? t(value) : key === 'version_state' ? t(`diagnosticsVersion.${value}`) : key === 'system' ? systems[value] ?? value : value;
  const submit = (values: DeviceFilterValues) => { onApply(applyDeviceFilters(params, values)); setDirty(false); };
  return <Form className="device-filter-form" form={form} layout="vertical" initialValues={applied} onFinish={submit}
    onValuesChange={changed => {
      setDirty(true);
      if (changed.version?.trim()) form.setFieldValue('version_state', '');
      if (changed.version_state === 'unknown') form.setFieldValue('version', '');
    }}>
    <div className="device-filter-fields">
      <Form.Item name="q" label={t('deviceSearch')}><Input aria-label={t('search')} placeholder={t('search')} allowClear maxLength={128}/></Form.Item>
      <Form.Item name="status" label={t('online')}><select className="device-filter-select" aria-label={t('online')}>{['', 'online', 'offline'].map(value => <option value={value} key={value}>{t(value || 'all')}</option>)}</select></Form.Item>
      <Form.Item name="system" label={t('system')}><select className="device-filter-select" aria-label={t('system')}>{['', 'windows', 'linux', 'macos', 'android', 'unknown'].map(value => <option value={value} key={value}>{value ? display('system', value) : t('all')}</option>)}</select></Form.Item>
      <Form.Item name="architecture" label={t('architecture')}><select className="device-filter-select" aria-label={t('architecture')}>{['', 'x86_64', 'aarch64', 'unknown'].map(value => <option value={value} key={value}>{value ? display('architecture', value) : t('all')}</option>)}</select></Form.Item>
      <Form.Item name="version_state" label={t('diagnosticsVersionState')}><select className="device-filter-select" aria-label={t('diagnosticsVersionState')}>{['', 'reported', 'unknown'].map(value => <option value={value} key={value}>{value ? t(`diagnosticsVersion.${value}`) : t('all')}</option>)}</select></Form.Item>
      <Form.Item name="version" label={t('agentVersion')}><select className="device-filter-select" aria-label={t('agentVersion')} aria-busy={versions.loading}><option value="">{t('all')}</option>{versionValues.map(value => <option key={value} value={value}>{value}</option>)}</select></Form.Item>
    </div>
    {versions.error && <Alert type="warning" title={t(versions.error)} action={<Button size="small" onClick={versions.refresh}>{t('refresh')}</Button>}/>}
    <div className="device-filter-actions">
      <Button type="primary" htmlType="submit" icon={<Filter size={15}/>}>{t('auditApply')}</Button>
      <Button icon={<RotateCcw size={14}/>} onClick={() => {
        const empty = Object.fromEntries(deviceFilterKeys.map(key => [key, ''])) as DeviceFilterValues;
        form.setFieldsValue(empty); submit(empty);
      }}>{t('auditClear')}</Button>
      {dirty && <span className="device-filter-pending" role="status">{t('deviceFiltersPending')}</span>}
    </div>
    <div className="device-active-filters" role="group" aria-label={t('deviceActiveFilters')}>
      {deviceFilterKeys.some(key => applied[key]) ? <>
        <span>{t('deviceActiveFilters')}</span>
        {deviceFilterKeys.filter(key => applied[key]).map(key => <Tag key={key} closable onClose={event => { event.preventDefault(); onRemove(key); }}>
          <span title={applied[key]}>{labels[key]}: {display(key, applied[key])}</span>
        </Tag>)}
      </> : <span>{t('deviceNoFilters')}</span>}
    </div>
  </Form>;
}
