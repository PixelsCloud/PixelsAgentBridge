import { paginationComponents } from './ConsoleSelect';
import { useEffect, useState } from 'react';
import { Alert, App, Badge, Button, Card, Form, Input, Modal, Space, Table, Typography } from 'antd';
import { api, ApiError } from './api';
import { useText, useDate } from './i18n';
import { PageHeading } from './PageHeading';
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
    <PageHeading title={t('savedDevices')} description={t('savedDevicesHint')}/>
    {error && <Alert type="error" title={t(error)} action={<Button onClick={() => setRevision(v => v + 1)}>{t('retry')}</Button>} />}
    <Card><Table rowKey={v => v.device_ref.device_id} dataSource={items} loading={loading} pagination={{ components: paginationComponents, defaultPageSize: 20, showSizeChanger: true, showTotal: n => `${t('total')} ${n}` }} columns={[
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

export { UsagePage } from './UsagePage';
