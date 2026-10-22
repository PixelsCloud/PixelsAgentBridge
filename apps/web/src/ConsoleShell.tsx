import { useState, type ReactNode } from 'react';
import { Button, Drawer, Grid, Layout, Menu, Tooltip } from 'antd';
import { Activity, ArrowLeftRight, ChevronRight, CircleUserRound, LayoutDashboard, List, LogOut, Menu as MenuIcon, Monitor, Network, Settings, ShieldCheck, Users } from 'lucide-react';
import { useLocation, useNavigate } from 'react-router-dom';
import type { Viewer } from './api';
import { useText } from './i18n';
import brand from './brand.svg';

interface ConsoleShellProps {
  me: Viewer;
  management: boolean;
  personal: boolean;
  connected: boolean;
  preferences: ReactNode;
  onScopeChange: (personal: boolean) => void;
  onLogout: () => void;
  children: ReactNode;
}

export function ConsoleShell({ me, management, personal, connected, preferences, onScopeChange, onLogout, children }: ConsoleShellProps) {
  const t = useText();
  const navigate = useNavigate();
  const location = useLocation();
  const screens = Grid.useBreakpoint();
  const mobile = !screens.md;
  const compact = !screens.xl && !mobile;
  const [menuOpen, setMenuOpen] = useState(false);
  const primary = [
    { key: '/', icon: <LayoutDashboard size={18}/>, label: t('overview') },
    { key: '/devices', icon: <Monitor size={18}/>, label: t(management ? 'devices' : 'myDevices') },
    ...(!management ? [{ key: '/saved-devices', icon: <List size={18}/>, label: t('savedDevices') }] : []),
    { key: '/usage', icon: <Activity size={18}/>, label: t(management ? 'serviceUsage' : 'myUsage') },
    ...(management ? [
      { key: '/accounts', icon: <Users size={18}/>, label: t('accounts') },
      { key: '/relay', icon: <Network size={18}/>, label: t('relay') },
      { key: '/audit', icon: <ShieldCheck size={18}/>, label: t('events') },
    ] : []),
  ];
  const secondary = [
    { key: '/account', icon: <CircleUserRound size={18}/>, label: t('account') },
    { key: '/settings', icon: <Settings size={18}/>, label: t('settings') },
  ];
  const active = '/' + (location.pathname.split('/')[1] ?? '');
  const current = [...primary, ...secondary].find(item => item.key === active);
  const go = (key: string) => { navigate(key); setMenuOpen(false); };
  const navigation = <>
    <a className="console-brand" href="/" onClick={event => { event.preventDefault(); go('/'); }} aria-label="Pixels">
      <img className="brand-mark small" src={brand} alt=""/>
      <div><strong><span className="brand-wordmark">Pixels</span><span className="brand-console">console</span></strong><span>Agent Bridge</span></div>
    </a>
    <nav className="console-navigation" aria-label={t('navigation')}>
      <div className="nav-caption nav-section-label">{t(management ? 'serviceManagement' : 'personalCenter')}</div>
      <Menu mode="inline" inlineCollapsed={compact && !mobile} items={primary} selectedKeys={[active]} onClick={({ key }) => go(key)}/>
      <div className="nav-caption nav-section-label secondary-label">{t('preferences')}</div>
      <Menu mode="inline" inlineCollapsed={compact && !mobile} items={secondary} selectedKeys={[active]} onClick={({ key }) => go(key)}/>
    </nav>
    {me.server_admin && <div className="sidebar-bottom">
      <Tooltip title={t(management ? 'switchToPersonal' : 'switchToService')} placement="right">
        <button className="console-scope-switch" aria-label={t(management ? 'switchToPersonal' : 'switchToService')}
          onClick={() => { onScopeChange(!personal); setMenuOpen(false); }}>
          <ArrowLeftRight size={18}/><span>{t(management ? 'switchToPersonal' : 'switchToService')}</span>
        </button>
      </Tooltip>
    </div>}
  </>;

  return <Layout className={`console-layout${compact ? ' console-compact' : ''}`}>
    <a className="skip-link" href="#main-content">{t('skipNavigation')}</a>
    {!mobile && <Layout.Sider className="console-sidebar" width={244} collapsed={compact} collapsedWidth={76} theme="light">{navigation}</Layout.Sider>}
    <Drawer className="console-mobile-menu" title={t('navigation')} placement="left" size={276} open={mobile && menuOpen} onClose={() => setMenuOpen(false)}>{mobile && navigation}</Drawer>
    <Layout className="console-main">
      <Layout.Header className="console-header">
        <div className="header-location">
          {mobile && <Button type="text" aria-label={t('navigation')} icon={<MenuIcon size={20}/>} onClick={() => setMenuOpen(true)}/>}
          <span className="header-scope">{t(management ? 'serviceManagement' : 'personalCenter')}</span>
          <ChevronRight className="breadcrumb-separator" size={14}/><span className="header-page">{current?.label ?? t('details')}</span>
        </div>
        <div className="header-actions">
          <Tooltip title={t(connected ? 'connected' : 'disconnected')}><span className={`live-indicator${connected ? ' is-connected' : ''}`} role="status"><i/>{t(connected ? 'liveUpdates' : 'reconnecting')}</span></Tooltip>
          <span className="header-divider"/>{preferences}
          <Button className="logout-button" type="text" title={t('logout')} aria-label={t('logout')} icon={<LogOut size={17}/>} onClick={onLogout}/>
        </div>
      </Layout.Header>
      <Layout.Content className="console-content" id="main-content" tabIndex={-1}>{children}</Layout.Content>
    </Layout>
  </Layout>;
}
