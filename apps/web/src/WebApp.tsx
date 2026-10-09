import { lazy, Suspense, useEffect, useState } from 'react';
import { Alert, App, Button, Card, ConfigProvider, Descriptions, Form, Input, Layout, Menu, Result, Segmented, Select, Space, Spin, Tag, Typography, theme } from 'antd';
import zhCN from 'antd/locale/zh_CN';
import zhTW from 'antd/locale/zh_TW';
import enUS from 'antd/locale/en_US';
import { Activity, LayoutDashboard, LogOut, Monitor, Moon, Settings, Sun, User, Users, Network, List } from 'lucide-react';
import { Navigate, Route, Routes, useLocation, useNavigate } from 'react-router-dom';
import { api, ApiError, post, type Viewer, type WebConfig } from './api';
import { AuthPage } from './AuthPage';
import { detectLanguage, LanguageContext, useText, type Language } from './i18n';
import { DeviceDetail, DeviceListPage, Overview } from './DevicePages';
import { useLiveUpdates } from './useLiveUpdates';
import brand from './brand.svg';
const AccountsPage = lazy(() => import('./ManagementPages').then(m => ({ default: m.AccountsPage })));
const AuditPage = lazy(() => import('./ManagementPages').then(m => ({ default: m.AuditPage })));
const TrafficPage = lazy(() => import('./ManagementPages').then(m => ({ default: m.TrafficPage })));
const ServiceSettings = lazy(() => import('./ManagementPages').then(m => ({ default: m.ServiceSettings })));

function stored(key: string) { try { return localStorage.getItem(key); } catch { return null; } }
function savePreference(key: string, value: string) { try { localStorage.setItem(key, value); } catch { /* Privacy modes can deny preference storage. */ } }

export function WebApp() {
  const [language, setLanguage] = useState<Language>(() => { const value = stored('pab-web-language'); return value === 'zh-CN' || value === 'zh-TW' || value === 'en' ? value : detectLanguage(navigator.language); });
  const [dark, setDark] = useState(() => stored('pab-web-theme') === 'dark' || (!stored('pab-web-theme') && matchMedia('(prefers-color-scheme: dark)').matches));
  const changeLanguage = (value: Language) => { setLanguage(value); savePreference('pab-web-language', value); };
  const changeTheme = (value: boolean) => { setDark(value); savePreference('pab-web-theme', value ? 'dark' : 'light'); };
  useEffect(() => { document.documentElement.lang = language; }, [language]);
  useEffect(() => { document.documentElement.dataset.theme = dark ? 'dark' : 'light'; }, [dark]);
  return <LanguageContext.Provider value={language}><ConfigProvider locale={language === 'zh-CN' ? zhCN : language === 'zh-TW' ? zhTW : enUS}
    theme={{ algorithm: dark ? theme.darkAlgorithm : theme.defaultAlgorithm, components: { Button: { primaryColor: dark ? '#102728' : '#ffffff' } }, token: { colorPrimary: dark ? '#79c5bc' : '#087f80', colorInfo: dark ? '#79c5bc' : '#087f80', borderRadius: 10, fontFamily: 'Inter, Segoe UI, Microsoft YaHei, sans-serif', colorBgLayout: dark ? '#181d23' : '#f3f6f8', colorBgContainer: dark ? '#232a32' : '#ffffff' } }}>
    <App><Console language={language} dark={dark} setLanguage={changeLanguage} setDark={changeTheme}/></App>
  </ConfigProvider></LanguageContext.Provider>;
}

interface Preferences { language: Language; dark: boolean; setLanguage: (value: Language) => void; setDark: (value: boolean) => void }
function Console(props: Preferences) {
  const t = useText(); const { message } = App.useApp(); const navigate = useNavigate(); const location = useLocation();
  const [me, setMe] = useState<Viewer | null>(); const [config, setConfig] = useState<WebConfig>(); const [error, setError] = useState<string>(); const [revision, setRevision] = useState(0);
  const { revision: liveRevision, connected } = useLiveUpdates(!!me);
  useEffect(() => {
    const controller = new AbortController(); setError(undefined);
    Promise.all([api<WebConfig>('/config', { signal: controller.signal }), api<Viewer>('/session', { signal: controller.signal }).catch(e => { if (e instanceof ApiError && e.status === 401) return null; throw e; })])
      .then(([cfg, user]) => { if (!controller.signal.aborted) { setConfig(cfg); setMe(user); } })
      .catch(e => { if (!controller.signal.aborted) setError(e instanceof ApiError ? e.code : 'networkError'); });
    return () => controller.abort();
  }, [revision]);
  useEffect(() => {
    const expired = () => { setMe(null); message.warning(t('sessionExpired')); };
    window.addEventListener('pab-session-expired', expired);
    return () => window.removeEventListener('pab-session-expired', expired);
  }, [t]);
  const preferences = <Space><Select aria-label={t('language')} value={props.language} onChange={props.setLanguage} options={[{ value: 'zh-CN', label: '简体中文' }, { value: 'zh-TW', label: '繁體中文' }, { value: 'en', label: 'English' }]} style={{ width: 120 }}/><Button aria-label={t(props.dark ? 'light' : 'dark')} icon={props.dark ? <Sun size={18}/> : <Moon size={18}/>} onClick={() => props.setDark(!props.dark)}/></Space>;
  if (error) return <div className="full-center"><Result status="error" title={t(error)} extra={<Button onClick={() => setRevision(v => v + 1)}>{t('retry')}</Button>}/></div>;
  if (me === undefined || !config) return <div className="full-center"><Spin size="large"/></div>;
  if (!me) return <><div className="auth-preferences">{preferences}</div><AuthPage registration={config.registration_enabled} onLogin={value => { setMe(value); navigate('/'); }}/></>;
  const items = [
    { key: '/', icon: <LayoutDashboard size={18}/>, label: t('overview') },
    { key: '/devices', icon: <Monitor size={18}/>, label: t('devices') },
    { key: '/online', icon: <Activity size={18}/>, label: t('onlineDevices') },
    ...(me.server_admin ? [{ key: '/accounts', icon: <Users size={18}/>, label: t('accounts') }] : []),
    { key: '/relay', icon: <Network size={18}/>, label: t('relay') },
    ...(me.server_admin ? [{ key: '/audit', icon: <List size={18}/>, label: t('events') }] : []),
    { key: '/account', icon: <User size={18}/>, label: t('account') },
    { key: '/settings', icon: <Settings size={18}/>, label: t('settings') },
  ];
  async function logout() { try { await post('/logout'); setMe(null); navigate('/'); } catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); } }
  return <Layout className="console-layout"><Layout.Sider width={224} breakpoint="lg" collapsedWidth={64} className="console-sidebar" theme={props.dark ? 'dark' : 'light'}>
    <div className="console-brand"><img className="brand-mark small" src={brand} alt=""/><div><strong>Pixels</strong><span>Agent Bridge</span></div></div>
    <Menu mode="inline" theme={props.dark ? 'dark' : 'light'} items={items} selectedKeys={[location.pathname.startsWith('/devices/') ? '/devices' : location.pathname]} onClick={({ key }) => navigate(key)}/>
  </Layout.Sider><Layout><Layout.Header className="console-header"><Typography.Text type="secondary">{t('console')}</Typography.Text><Space size={16}>{preferences}<Tag>{me.username}</Tag><Button aria-label={t('logout')} icon={<LogOut size={16}/>} onClick={logout}/></Space></Layout.Header>
    <Layout.Content className="console-content">{!connected && <Alert type="warning" title={t('disconnected')} showIcon style={{ marginBottom: 16 }}/>}<Suspense fallback={<Spin/>}><Routes>
      <Route path="/" element={<Overview liveRevision={liveRevision}/>}/>
      <Route path="/devices" element={<DeviceListPage key="list" me={me} mode="list" liveRevision={liveRevision}/>}/>
      <Route path="/online" element={<DeviceListPage key="online" me={me} mode="online" liveRevision={liveRevision}/>}/>
      <Route path="/all-devices" element={<Navigate to={{ pathname: '/devices', search: location.search, hash: location.hash }} replace/>}/>
      <Route path="/devices/:id" element={<DeviceDetail liveRevision={liveRevision}/>}/>
      <Route path="/accounts" element={me.server_admin ? <AccountsPage liveRevision={liveRevision}/> : <Result status="403" title={t('forbidden')}/>}/>
      <Route path="/audit" element={me.server_admin ? <AuditPage liveRevision={liveRevision}/> : <Result status="403" title={t('forbidden')}/>}/>
      <Route path="/relay" element={<TrafficPage liveRevision={liveRevision} me={me}/>}/>
      <Route path="/account" element={<Account me={me}/>}/>
      <Route path="/settings" element={<PreferencesPanel {...props} version={config.version} admin={me.server_admin}/>}/>
      <Route path="*" element={<Navigate to="/" replace/>}/>
    </Routes></Suspense></Layout.Content></Layout></Layout>;
}

function PreferencesPanel(props: Preferences & { version: string; admin: boolean }) {
  const t = useText();
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}><Typography.Title level={2}>{t('settings')}</Typography.Title>
    <Card title={t('appearance')}><Segmented value={props.dark ? 'dark' : 'light'} options={[{ value: 'light', label: t('light') }, { value: 'dark', label: t('dark') }]} onChange={value => props.setDark(value === 'dark')}/></Card>
    <Card title={t('language')}><Select value={props.language} onChange={props.setLanguage} style={{ width: 240 }} options={[{ value: 'zh-CN', label: '简体中文' }, { value: 'zh-TW', label: '繁體中文' }, { value: 'en', label: 'English' }]}/></Card>
    {props.admin && <ServiceSettings/>}<Card title={t('about')}><Descriptions items={[{ key: 'product', label: t('product'), children: props.version }]}/></Card>
  </Space>;
}

function Account({ me }: { me: Viewer }) {
  const t = useText(); const { message } = App.useApp(); const [busy, setBusy] = useState(false); const [error, setError] = useState<string>();
  async function submit(values: { current_password: string; new_password: string }) {
    setBusy(true); setError(undefined);
    try { await post('/password', { current_password: values.current_password, new_password: values.new_password }); message.success(t('passwordChanged')); }
    catch (e) { setError(e instanceof ApiError ? e.code : 'networkError'); } finally { setBusy(false); }
  }
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}><Typography.Title level={2}>{t('account')}</Typography.Title>
    <Card><Descriptions items={[{ key: 'user', label: t('username'), children: me.username }, { key: 'role', label: t('account'), children: t(me.server_admin ? 'admin' : 'user') }]}/></Card>
    <Card title={t('changePassword')}><Form layout="vertical" onFinish={submit} style={{ maxWidth: 440 }} requiredMark={false}>
      {error && <Alert type="error" title={t(error)} showIcon style={{ marginBottom: 20 }}/>}
      <Form.Item name="current_password" label={t('currentPassword')} rules={[{ required: true, message: t('required') }]}><Input.Password autoComplete="current-password" maxLength={1024}/></Form.Item>
      <Form.Item name="new_password" label={t('newPassword')} extra={t('passwordMin')} rules={[{ required: true, message: t('required') }, { min: 8, message: t('passwordMin') }]}><Input.Password autoComplete="new-password" maxLength={1024}/></Form.Item>
      <Form.Item name="confirm" label={t('confirmPassword')} dependencies={['new_password']} rules={[{ required: true, message: t('required') }, ({ getFieldValue }) => ({ validator: (_, value) => value === getFieldValue('new_password') ? Promise.resolve() : Promise.reject(new Error(t('passwordMismatch'))) })]}><Input.Password autoComplete="new-password"/></Form.Item>
      <Button type="primary" htmlType="submit" loading={busy}>{t('save')}</Button>
    </Form></Card>
  </Space>;
}
