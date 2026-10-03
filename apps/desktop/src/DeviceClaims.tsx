import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Alert, App, Button, Modal, Space, Table } from 'antd';
import type { Language } from './i18n';

type Claim = { claim_id: string; username: string; expires_at_unix_ms: number };
const labels = {
  'zh-CN': { title: '设备认领申请', pending: '有账号申请将本机添加到其设备列表', view: '查看申请', user: '申请账号', expires: '到期时间', approve: '同意', reject: '拒绝', confirm: '同意后，此账号可在 Web 管理本机归属。设备码和临时密码连接方式不变。', cancel: '取消', close: '关闭' },
  'zh-TW': { title: '裝置認領申請', pending: '有帳號申請將本機新增至其裝置清單', view: '查看申請', user: '申請帳號', expires: '到期時間', approve: '同意', reject: '拒絕', confirm: '同意後，此帳號可在 Web 管理本機歸屬。裝置碼和臨時密碼連線方式不變。', cancel: '取消', close: '關閉' },
  en: { title: 'Device ownership requests', pending: 'An account wants to add this device to its device list.', view: 'Review requests', user: 'Account', expires: 'Expires', approve: 'Approve', reject: 'Reject', confirm: 'This account will manage device ownership on the Web. Connections still use the device code and temporary password.', cancel: 'Cancel', close: 'Close' },
};

export function DeviceClaims({ language, enabled }: { language: Language; enabled: boolean }) {
  const t = labels[language]; const { message, modal } = App.useApp();
  const [claims, setClaims] = useState<Claim[]>([]); const [open, setOpen] = useState(false); const [busy, setBusy] = useState<string>();
  useEffect(() => {
    let stopped = false; let timer: ReturnType<typeof setTimeout>;
    if (!enabled) { setClaims([]); return; }
    async function poll() {
      try { const next = await invoke<Claim[]>('pending_device_claims'); if (!stopped) setClaims(next); }
      catch { if (!stopped) setClaims([]); }
      finally { if (!stopped) timer = setTimeout(poll, 15000); }
    }
    void poll(); return () => { stopped = true; clearTimeout(timer); };
  }, [enabled]);
  async function decide(claim: Claim, approve: boolean) {
    setBusy(claim.claim_id);
    try { await invoke(approve ? 'approve_claim' : 'reject_device_claim', { claimId: claim.claim_id }); setClaims(await invoke<Claim[]>('pending_device_claims')); }
    catch (e) { message.error(String(e)); throw e; } finally { setBusy(undefined); }
  }
  const approve = (claim: Claim) => modal.confirm({ title: `${t.approve}: ${claim.username}`, content: t.confirm, maskClosable: false, keyboard: false, okText: t.approve, cancelText: t.cancel, onOk: () => decide(claim, true) });
  return <>{claims.length > 0 && <Alert type="info" title={t.pending} showIcon action={<Button onClick={() => setOpen(true)}>{t.view}</Button>} style={{ marginBottom: 12 }}/>}
    <Modal title={t.title} open={open} maskClosable={false} keyboard={false} onCancel={() => !busy && setOpen(false)} footer={<Button disabled={!!busy} onClick={() => setOpen(false)}>{t.close}</Button>} width={620}>
      <Table rowKey="claim_id" dataSource={claims} pagination={false} columns={[
        { title: t.user, dataIndex: 'username' }, { title: t.expires, render: (_, claim) => new Date(claim.expires_at_unix_ms).toLocaleTimeString() },
        { key: 'actions', render: (_, claim) => <Space><Button type="primary" disabled={!!busy} onClick={() => approve(claim)}>{t.approve}</Button><Button danger loading={busy === claim.claim_id} disabled={!!busy} onClick={() => void decide(claim, false).catch(() => {})}>{t.reject}</Button></Space> },
      ]}/>
    </Modal>
  </>;
}
