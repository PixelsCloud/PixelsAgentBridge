import { useEffect, useState } from 'react';
import { Input, Typography } from 'antd';
import { api } from './api';
import { useResource } from './useResource';
import { useText } from './i18n';

export function AccountSelector({ value, onChange }: { value?: string; onChange: (value: string | undefined) => void }) {
  const t = useText(); const [search, setSearch] = useState(''); const [query, setQuery] = useState('');
  useEffect(() => { const timer = setTimeout(() => setQuery(search), 250); return () => clearTimeout(timer); }, [search]);
  const accounts = useResource<{ items: { id: string; username: string }[]; total: number }>(`/accounts?page_size=100&q=${encodeURIComponent(query)}`);
  const [selected, setSelected] = useState<{ id: string; username: string }>();
  useEffect(() => {
    setSelected(undefined); if (!value) return;
    const controller = new AbortController();
    api<{ id: string; username: string }>(`/accounts/${encodeURIComponent(value)}`, { signal: controller.signal })
      .then(result => { if (!controller.signal.aborted) setSelected({ id: result.id, username: result.username }); }).catch(() => {});
    return () => controller.abort();
  }, [value]);
  const items = accounts.data?.items ?? [];
  const options = selected && selected.id === value && !items.some(item => item.id === value) ? [selected, ...items] : items;
  return <div className="usage-account-selector">
    {(search || (accounts.data?.total ?? 0) > 100) && <Input aria-label={t('username')} placeholder={t('username')} value={search} onChange={event => setSearch(event.target.value)} allowClear/>}
    <select className="console-select" aria-label={t('username')} value={value ?? ''} onChange={event => onChange(event.target.value || undefined)} aria-busy={accounts.loading}>
      <option value="">{t('username')} · {t('all')}</option>
      {value && !options.some(item => item.id === value) && <option value={value}>{value}</option>}
      {options.map(item => <option key={item.id} value={item.id}>{item.username}</option>)}
    </select>
    {accounts.error && <Typography.Text type="danger">{t(accounts.error)}</Typography.Text>}
  </div>;
}
