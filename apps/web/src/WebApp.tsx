import { ConsoleSelect } from './ConsoleSelect';
import { lazy, Suspense, useEffect, useState } from 'react';
import { Alert, App, Button, Card, ConfigProvider, Descriptions, Result, Segmented, Space, Spin, theme } from 'antd';
import zhCN from 'antd/locale/zh_CN';
import enUS from 'antd/locale/en_US';
import { Moon, Sun } from 'lucide-react';
import { Navigate, Route, Routes, useLocation, useNavigate } from 'react-router-dom';
import { api, ApiError, post, type Viewer, type WebConfig } from './api';
import { AuthPage } from './AuthPage';
import { AccountPage } from './AccountPage';
import { detectLanguage, LanguageContext, useText, type Language } from './i18n';
import { DeviceDetail, DeviceListPage, Overview } from './DevicePages';
import { useLiveUpdates } from './useLiveUpdates';
import { ConsoleShell } from './ConsoleShell';
import { PageHeading } from './PageHeading';
import { SavedDevicesPage, UsagePage } from './PersonalPages';
const AccountDetailPage = lazy(() => import('./AccountDetailPage').then(m => ({ default: m.AccountDetailPage })));
const AccountsPage = lazy(() => import('./ManagementPages').then(m => ({ default: m.AccountsPage })));
const AuditPage = lazy(() => import('./ManagementPages').then(m => ({ default: m.AuditPage })));
const TrafficPage = lazy(() => import('./ManagementPages').then(m => ({ default: m.TrafficPage })));
const ServiceSettings = lazy(() => import('./ManagementPages').then(m => ({ default: m.ServiceSettings })));

function stored(key: string) { try { return localStorage.getItem(key); } catch { return null; } }
function savePreference(key: string, value: string) { try { localStorage.setItem(key, value); } catch { /* Privacy modes can deny preference storage. */ } }

export function WebApp() {
  const [language, setLanguage] = useState<Language>(() => { const value = stored('pab-web-language'); return value === 'zh-TW' ? 'zh-CN' : value === 'zh-CN' || value === 'en' ? value : detectLanguage(navigator.language); });
  const [dark, setDark] = useState(() => stored('pab-web-theme') === 'dark' || (!stored('pab-web-theme') && matchMedia('(prefers-color-scheme: dark)').matches));
  const changeLanguage = (value: Language) => { setLanguage(value); savePreference('pab-web-language', value); };
  const changeTheme = (value: boolean) => { setDark(value); savePreference('pab-web-theme', value ? 'dark' : 'light'); };
  useEffect(() => { document.documentElement.lang = language; savePreference('pab-web-language', language); }, [language]);
  useEffect(() => { document.documentElement.dataset.theme = dark ? 'dark' : 'light'; }, [dark]);
  return <LanguageContext.Provider value={language}><ConfigProvider locale={language === 'zh-CN' ? zhCN : enUS}
    theme={{ algorithm: dark ? theme.darkAlgorithm : theme.defaultAlgorithm, components: {
      Button: { primaryColor: dark ? '#102a28' : '#ffffff', fontWeight: 500, defaultShadow: 'none', primaryShadow: '0 2px 3px #126b5f16' },
      Card: { headerFontSize: 15, headerHeight: 60, bodyPadding: 24 },
      Table: { headerBg: dark ? '#1d2929' : '#f7f9f9', headerColor: dark ? '#acbfbc' : '#657b79', headerSplitColor: 'transparent', cellPaddingBlock: 17 },
      Menu: { itemHeight: 44, itemMarginInline: 0, itemBorderRadius: 8, itemSelectedBg: dark ? '#203d38' : '#e6f3ef', itemSelectedColor: dark ? '#8bdbbd' : '#187b65', iconSize: 18 },
    }, token: {
      colorPrimary: dark ? '#82d6ba' : '#21836c', colorInfo: dark ? '#82d6ba' : '#21836c', colorSuccess: dark ? '#82d6ba' : '#279777',
      colorText: dark ? '#e3eeeb' : '#213c38', colorTextSecondary: dark ? '#97aba7' : '#667b75', colorBorder: dark ? '#344542' : '#dce6e2', colorBorderSecondary: dark ? '#2b3b38' : '#e7edea',
      colorInfoBg: dark ? '#203a32' : '#eef7f2', colorInfoBorder: dark ? '#345347' : '#d4e9dd',
      borderRadius: 8, borderRadiusLG: 14, controlHeight: 36, fontSize: 13,
      fontFamily: 'Inter, -apple-system, BlinkMacSystemFont, Segoe UI, Microsoft YaHei, sans-serif',
      colorBgLayout: dark ? '#141e1d' : '#f5f8f6', colorBgContainer: dark ? '#1a2725' : '#ffffff',
    } }}>
    <App><Console language={language} dark={dark} setLanguage={changeLanguage} setDark={changeTheme}/></App>
  </ConfigProvider></LanguageContext.Provider>;
}

interface Preferences { language: Language; dark: boolean; setLanguage: (value: Language) => void; setDark: (value: boolean) => void }
function Console(props: Preferences) {
  const [personal, setPersonal] = useState(false);
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
  const preferences = <Space className="console-preferences" size={8}><ConsoleSelect aria-label={t('language')} value={props.language} onChange={props.setLanguage} options={[{ value: 'zh-CN', label: '简体中文' }, { value: 'en', label: 'English' }]} className="language-select" style={{ width: 112 }}/><Button aria-label={t(props.dark ? 'light' : 'dark')} icon={props.dark ? <Sun size={18}/> : <Moon size={18}/>} onClick={() => props.setDark(!props.dark)}/></Space>;
  if (error) return <div className="full-center"><Result status="error" title={t(error)} extra={<Button onClick={() => setRevision(v => v + 1)}>{t('retry')}</Button>}/></div>;
  if (me === undefined || !config) return <div className="full-center"><Spin size="large"/></div>;
  if (!me) return <><div className="auth-preferences">{preferences}</div><AuthPage registration={config.registration_enabled} github={config.github_enabled} onLogin={value => { setMe(value); navigate('/'); }}/></>;
  const management = me.server_admin && !personal;
  const viewMe = { ...me, server_admin: management };
  async function logout() { try { await post('/logout'); setMe(null); navigate('/'); } catch (e) { message.error(t(e instanceof ApiError ? e.code : 'networkError')); } }
  return <ConsoleShell me={me} management={management} personal={personal} connected={connected} preferences={preferences}
    onScopeChange={value => { setPersonal(value); navigate('/'); }} onLogout={logout}>
    {new URLSearchParams(location.search).get("github_error") && <Alert type="error" title={t(new URLSearchParams(location.search).get("github_error")!)} showIcon style={{ marginBottom: 16 }}/>}{!connected && <Alert type="warning" title={t('disconnected')} showIcon style={{ marginBottom: 16 }}/>}<Suspense fallback={<Spin/>}><Routes key={`${me.id}:${management}`}>
      <Route path="/" element={<Overview all={management} liveRevision={liveRevision}/>}/>
      <Route path="/devices" element={<DeviceListPage me={viewMe} liveRevision={liveRevision}/>}/>
      <Route path="/online" element={<Navigate to={`/devices?${new URLSearchParams({ ...Object.fromEntries(new URLSearchParams(location.search)), status: 'online' })}`} replace/>}/>
      <Route path="/saved-devices" element={<SavedDevicesPage liveRevision={liveRevision} />} />
      <Route path="/usage" element={<UsagePage all={management} liveRevision={liveRevision} />} />
      <Route path="/all-devices" element={<Navigate to={{ pathname: '/devices', search: location.search, hash: location.hash }} replace/>}/>
      <Route path="/devices/:id" element={<DeviceDetail liveRevision={liveRevision} me={me}/>}/>
      <Route path="/accounts" element={me.server_admin ? <AccountsPage liveRevision={liveRevision}/> : <Result status="403" title={t('forbidden')}/>}/>
      <Route path="/accounts/:id" element={me.server_admin ? <AccountDetailPage me={me} liveRevision={liveRevision}/> : <Result status="403" title={t('forbidden')}/>}/>
      <Route path="/audit" element={me.server_admin ? <AuditPage/> : <Result status="403" title={t('forbidden')}/>}/>
      <Route path="/relay" element={me.server_admin ? <TrafficPage liveRevision={liveRevision} me={me}/> : <Navigate to="/usage" replace/>}/>
      <Route path="/account" element={<AccountPage me={me}/>}/>
      <Route path="/settings" element={<PreferencesPanel {...props} version={config.version} admin={me.server_admin} liveRevision={liveRevision}/>}/>
      <Route path="*" element={<Navigate to="/" replace/>}/>
    </Routes></Suspense></ConsoleShell>;
}

function PreferencesPanel(props: Preferences & { version: string; admin: boolean; liveRevision: number }) {
  const t = useText();
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}><PageHeading title={t('settings')} description={t('settingsPageHint')}/>
    <div className="preferences-grid"><Card title={t('appearance')}><Segmented value={props.dark ? 'dark' : 'light'} options={[{ value: 'light', label: t('light') }, { value: 'dark', label: t('dark') }]} onChange={value => props.setDark(value === 'dark')}/></Card>
    <Card title={t('language')}><ConsoleSelect aria-label={t('language')} value={props.language} onChange={props.setLanguage} style={{ width: 240, maxWidth: '100%' }} options={[{ value: 'zh-CN', label: '简体中文' }, { value: 'en', label: 'English' }]}/></Card></div>
    {props.admin && <ServiceSettings liveRevision={props.liveRevision}/>}<Card title={t('about')}><Descriptions items={[{ key: 'product', label: t('product'), children: props.version }]}/></Card>
  </Space>;
}
