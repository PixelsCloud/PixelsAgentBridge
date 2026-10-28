import { useState } from 'react';
import { Alert, Badge, Button, Modal, Space, Table, Tag, Typography } from 'antd';
import { useNavigate } from 'react-router-dom';
import { paginationComponents } from './ConsoleSelect';
import { formatCode } from './DevicePages';
import { useDate, useText } from './i18n';
import { useLiveRefresh, useResource } from './useResource';
import type { Page } from './usePagedResource';

interface AccountDevice { id: string; code: string; name: string; online: boolean; system: string | null; os_name?: string | null; client_version?: string | null; alias?: string; deleted?: boolean; created_at: number }

export function AccountDevicesModal({ account, kind = 'associated', liveRevision, onClose }: {
  account: { id: string; username: string }; kind?: 'associated' | 'added'; liveRevision: number; onClose: () => void;
}) {
  const t = useText(); const date = useDate(); const added = kind === 'added'; const navigate = useNavigate();
  const [pagination, setPagination] = useState({ page: 1, size: 20 });
  const query = new URLSearchParams({ scope: 'all', user: account.id, page: String(pagination.page), page_size: String(pagination.size) });
  const resource = useResource<Page<AccountDevice>>(added ? `/accounts/${account.id}/saved-devices?page=${pagination.page}&page_size=${pagination.size}` : `/devices?${query}`);
  useLiveRefresh(resource.refresh, liveRevision);
  return <Modal title={`${t(added ? 'addedDevices' : 'associatedDevices')} · ${account.username}`} open mask={{ closable: false }} keyboard={false} closable={false}
    width={960} footer={<Button onClick={onClose}>{t('close')}</Button>}>
    <Space orientation="vertical" size={16} style={{ width: '100%' }}>
      <Typography.Text type="secondary">{t(added ? 'addedDevicesHint' : 'accountDeviceAssociationHint')}</Typography.Text>
      <Button onClick={resource.refresh} loading={resource.loading} style={{ alignSelf: 'start' }}>{t('refresh')}</Button>
      {resource.error && <Alert type="error" title={t(resource.error)} action={<Button onClick={resource.refresh}>{t('retry')}</Button>}/>}
      <Table<AccountDevice> rowKey="id" loading={resource.loading} dataSource={resource.error ? [] : resource.data?.items ?? []}
        scroll={{ x: 720 }} locale={{ emptyText: t('noData') }} columns={[
          { title: t('deviceCode'), key: 'code', render: (_, item) => <Button type="link" onClick={() => { onClose(); navigate(`/devices/${item.id}`); }}>{formatCode(item.code)}</Button> },
          { title: t('name'), dataIndex: 'name', key: 'name' },
          ...(added ? [{ title: t('personalAlias'), key: 'alias', render: (_: unknown, item: AccountDevice) => item.alias || '—' }] : []),
          { title: t('online'), key: 'online', render: (_, item) => <Badge status={item.online ? 'success' : 'default'} text={t(item.online ? 'online' : 'offline')}/> },
          { title: t('system'), key: 'system', render: (_, item) => item.os_name ?? item.system ?? t('unknown') },
          ...(added ? [
            { title: t('addedDeviceStatus'), key: 'status', render: (_: unknown, item: AccountDevice) => <Tag color={item.deleted ? 'default' : 'success'}>{t(item.deleted ? 'addedDeviceRemoved' : 'addedDeviceSaved')}</Tag> },
            { title: t('deviceAddedAt'), key: 'created', render: (_: unknown, item: AccountDevice) => date(item.created_at) },
          ] : [{ title: t('agentVersion'), key: 'version', render: (_: unknown, item: AccountDevice) => item.client_version || t('clientVersionUnreported') }]),
        ]} pagination={{ components: paginationComponents, current: resource.data?.page ?? pagination.page, pageSize: resource.data?.page_size ?? pagination.size,
          total: resource.data?.total ?? 0, showSizeChanger: true, pageSizeOptions: [20, 50, 100], showTotal: total => `${t('total')} ${total}`,
          onChange: (page, size) => setPagination({ page, size }) }}/>
    </Space>
  </Modal>;
}
