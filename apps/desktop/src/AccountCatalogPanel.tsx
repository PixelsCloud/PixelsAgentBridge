import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Alert, Button, Modal, Space, Typography } from 'antd';
import type { Language } from './i18n';

const text = {
  'zh-CN': { title: '远程设备列表同步', synced: '已同步', pending: '待同步', syncing: '正在同步', retry: '重试同步', error: '同步暂未完成，设备连接仍可正常使用。', import: '导入本地设备列表', importHint: '将本机游客模式保存的设备导入当前账号，仅上传设备码、名称、系统和备注。密码保留在本地；不会获得新的控制权限，也不会恢复已经移除的条目。', conflict: '列表已在另一客户端修改', local: '使用我的修改', remote: '保留云端修改', removed: '移除条目', actionFailed: '操作未完成，请刷新后重试。' },
  'zh-TW': { title: '遠端裝置清單同步', synced: '已同步', pending: '待同步', syncing: '正在同步', retry: '重試同步', error: '同步尚未完成，裝置連線仍可正常使用。', import: '匯入本機裝置清單', importHint: '將本機訪客模式儲存的裝置匯入目前帳號，僅上傳裝置碼、名稱、系統與備註。密碼保留在本機；不會取得新的控制權限，也不會恢復已移除的項目。', conflict: '清單已在其他用戶端修改', local: '使用我的修改', remote: '保留雲端修改', removed: '移除項目', actionFailed: '操作未完成，請重新整理後重試。' },
  en: { title: 'Remote device sync', synced: 'Synced', pending: 'Pending', syncing: 'Syncing', retry: 'Retry sync', error: 'Sync is pending. Device connections remain available.', import: 'Import local device list', importHint: 'Import devices saved in guest mode into this account. Only codes, names, systems and aliases are uploaded. Passwords stay local. This grants no additional access and does not restore removed entries.', conflict: 'Changed on another client', local: 'Use my change', remote: 'Keep cloud change', removed: 'Remove entry', actionFailed: 'Could not complete the action. Refresh and retry.' },
};
type Status = { account_revision: number; pending: number; syncing: boolean; error?: string; local_import_count: number; conflicts: { device_ref: { device_id: string }; code: string; alias: string; deleted: boolean }[] };

export function AccountCatalogPanel({ language, accountRevision }: { language: Language; accountRevision: number }) {
  const t = text[language]; const [status, setStatus] = useState<Status>(); const [busy, setBusy] = useState(false); const [error, setError] = useState(false); const [confirm, setConfirm] = useState(false); const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    let alive = true;
    const update = () => void invoke<Status>('account_catalog_status').then(value => { if (alive && value.account_revision === accountRevision) setStatus(value); }).catch(() => { if (alive) setError(true); });
    update(); const timer = setInterval(update, 3000); return () => { alive = false; clearInterval(timer); };
  }, [accountRevision, refresh]);
  async function action(action: string, device?: string) {
    setBusy(true); setError(false);
    try { await invoke('account_catalog_action', { revision: accountRevision, action, device }); setConfirm(false); setRefresh(v => v + 1); }
    catch { setError(true); } finally { setBusy(false); }
  }
  return <Space orientation="vertical" style={{ width: '100%', marginTop: 16 }}>
    <Typography.Text strong>{t.title}</Typography.Text>
    <Space wrap><Typography.Text type="secondary">{status?.syncing ? t.syncing : !status || status.pending ? `${t.pending}${status ? ` (${status.pending})` : ''}` : t.synced}</Typography.Text><Button size="small" loading={busy} onClick={() => void action('retry')}>{t.retry}</Button>
      {!!status?.local_import_count && <Button size="small" disabled={busy} onClick={() => setConfirm(true)}>{t.import} ({status.local_import_count})</Button>}</Space>
    {(error || status?.error) && <Alert type="warning" showIcon title={error ? t.actionFailed : t.error} />}
    {status?.conflicts.map(item => <Alert key={item.device_ref.device_id} type="warning" title={`${t.conflict}: ${item.code}`} description={<Space orientation="vertical"><Typography.Text>{item.deleted ? t.removed : item.alias}</Typography.Text><Space wrap><Button disabled={busy} onClick={() => void action('keep_local', item.device_ref.device_id)}>{t.local}</Button><Button disabled={busy} onClick={() => void action('keep_remote', item.device_ref.device_id)}>{t.remote}</Button></Space></Space>} />)}
    <Modal title={t.import} open={confirm} mask={{ closable: false }} keyboard={false} closable={!busy} confirmLoading={busy} onCancel={() => !busy && setConfirm(false)} onOk={() => void action('import')}>{t.importHint}</Modal>
  </Space>;
}
