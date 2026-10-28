import { useContext } from 'react';
import { LanguageContext } from './i18n';

const zh = {
  title: '概览', description: '用户、设备与流量，一站掌握。', updated: '状态采样',
  registered: '注册用户', administrators: '管理员', enabled: '启用', disabled: '停用',
  devices: '登记设备', reported: '运行上报', unreported: '未上报', online: '在线', offline: '离线', control: '当前控制通道连接状态',
  relayNodes: 'Relay 节点', relayAvailable: '可用',
  usage: '全站用量', today: '今日', week: '近 7 天', month: '本月', lifetime: '累计',
  relay: 'Relay 流量', p2p: 'P2P 流量', duration: '累计连接时长', connections: '成功连接', durationHint: '各连接使用时长累计', connectionsHint: '已建立的远程连接', previous: '较上一周期', selectedPeriod: '所选周期内', sinceStart: '自开始记录以来',
  relayTrend: 'Relay 流量趋势', p2pTrend: 'P2P 流量趋势', durationTrend: '连接时长趋势', durationTab: '连接时长', upload: '上行', download: '下行', p2pUpload: '上传', p2pDownload: '下载', hourly: '按小时', daily: '按天', monthTrend: '本月趋势', noReports: '暂无上报数据',
  transfer: '文件传输', completed: '个完成', files: '个文件', failed: '失败', cancelled: '取消',
  nodes: 'Relay 运行', currentConnections: '当前连接', capacity: '节点带宽', received: '本次启动接收', sent: '本次启动发送', policyReady: '策略已同步', policyPending: '策略同步中', heartbeat: '前心跳', service: '服务', database: '数据库', healthy: '正常', check: '待检查', available: '可接收连接', maintenance: '维护', abnormal: '异常', noNodes: '暂无节点',
  platforms: '平台分布', systems: '系统', architectures: '架构', versions: '版本', client: '客户端',
  ranking: '用户用量排行', rankingSort: '排行指标', byRelay: '按 Relay 下行', byP2p: '按 P2P 下载', byDuration: '按连接时长', user: '用户', relayDown: 'Relay 下行', p2pDown: 'P2P 下载', allUsage: '完整用量', guest: '游客',
  deviceDetails: '设备明细', deviceSearch: '搜索设备或用户', allDevices: '全部设备', attention: '待关注', reportOrder: '按运行上报排序', deviceOwner: '设备 / 归属用户', system: '系统', reportAt: '运行上报', unbound: '未绑定用户', connected: '控制通道已连接', disconnected: '控制通道未连接', noDevices: '没有符合条件的设备', previousPage: '上一页设备', nextPage: '下一页设备',
  missing: '未上报', unknown: '未知', notConfigured: '未配置', justNow: '刚刚', ago: '前', units: '单位', byteUnit: '字节', durationUnit: '连接时长', scopeHint: '用量按所选周期统计 · 设备与平台为全站状态',
};
type Key = keyof typeof zh;
const en: Record<Key, string> = {
  title: 'Overview', description: 'Users, devices and traffic at a glance.', updated: 'Sampled',
  registered: 'Registered users', administrators: 'Admins', enabled: 'Enabled', disabled: 'Disabled',
  devices: 'Devices', reported: 'Runtime reports', unreported: 'Not reported', online: 'Online', offline: 'Offline', control: 'Current control-channel connections',
  relayNodes: 'Relay nodes', relayAvailable: 'Available',
  usage: 'Service usage', today: 'Today', week: '7 days', month: 'This month', lifetime: 'Lifetime',
  relay: 'Relay traffic', p2p: 'P2P traffic', duration: 'Connection time', connections: 'Connections', durationHint: 'Time accumulated across connections', connectionsHint: 'Successful remote connections', previous: 'vs previous period', selectedPeriod: 'Selected period', sinceStart: 'Since recording began',
  relayTrend: 'Relay traffic trend', p2pTrend: 'P2P traffic trend', durationTrend: 'Connection time trend', durationTab: 'Connection time', upload: 'Upload', download: 'Download', p2pUpload: 'Upload', p2pDownload: 'Download', hourly: 'Hourly', daily: 'Daily', monthTrend: 'This month’s trend', noReports: 'No reports yet',
  transfer: 'File transfers', completed: 'completed', files: 'files', failed: 'Failed', cancelled: 'Cancelled',
  nodes: 'Relay runtime', currentConnections: 'Live connections', capacity: 'Bandwidth', received: 'Received since start', sent: 'Sent since start', policyReady: 'Policy synchronized', policyPending: 'Synchronizing policy', heartbeat: 'since heartbeat', service: 'Service', database: 'Database', healthy: 'Healthy', check: 'Check needed', available: 'Accepting connections', maintenance: 'Maintenance', abnormal: 'Abnormal', noNodes: 'No nodes',
  platforms: 'Platforms', systems: 'System', architectures: 'Arch', versions: 'Version', client: 'Client',
  ranking: 'Usage ranking', rankingSort: 'Ranking metric', byRelay: 'By Relay download', byP2p: 'By P2P download', byDuration: 'By connection time', user: 'User', relayDown: 'Relay download', p2pDown: 'P2P download', allUsage: 'Full usage', guest: 'Guests',
  deviceDetails: 'Device details', deviceSearch: 'Search device or user', allDevices: 'All devices', attention: 'Needs attention', reportOrder: 'Latest runtime reports first', deviceOwner: 'Device / account', system: 'System', reportAt: 'Runtime report', unbound: 'Unlinked', connected: 'Control channel connected', disconnected: 'Control channel disconnected', noDevices: 'No matching devices', previousPage: 'Previous devices', nextPage: 'Next devices',
  missing: 'Not reported', unknown: 'Unknown', notConfigured: 'Not configured', justNow: 'Just now', ago: 'ago', units: 'Unit', byteUnit: 'Bytes', durationUnit: 'Connection time', scopeHint: 'Usage follows the selected period · Device and platform totals cover the service',
};
export type OverviewText = (key: Key) => string;
export function useOverviewText(): OverviewText {
  const language = useContext(LanguageContext);
  return key => (language === 'zh-CN' ? zh : en)[key];
}
