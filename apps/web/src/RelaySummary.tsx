import { Button, Card, Descriptions, Space, Tag } from 'antd';
import { useDate, useText } from './i18n';

export const relayCategories = ['configured', 'available', 'maintenance', 'disabled', 'abnormal', 'history'] as const;
export type RelayCategory = typeof relayCategories[number];
export interface RelaySnapshot {
  summary: Record<RelayCategory, number>;
  sampled_at: number;
  offline_seconds: number;
  mode: 'cluster' | 'legacy';
  semantics_version: number;
  health?: { server: 'responding'; database: 'reachable'; source: string; checked_at: number; latency_grade: 'normal' | 'slow' };
}

export function RelaySummary({ snapshot, onSelect, unavailable = false }: { snapshot: RelaySnapshot; onSelect: (value: RelayCategory) => void; unavailable?: boolean }) {
  const t = useText(); const date = useDate();
  return <Space className="relay-summary" orientation="vertical" size={16} style={{ width: '100%' }}>
    <Card title={t('relayServiceStatus')} className="relay-service-card">
      <Descriptions column={{ xs: 1, sm: 2, xl: 4 }} items={[
        { key: 'nodes', label: t('nodes'), children: snapshot.summary.configured },
        { key: 'control', label: t('relayManagementApi'), children: <Tag color={!unavailable && snapshot.health ? 'success' : 'default'}>{t(!unavailable && snapshot.health ? 'online' : 'unknown')}</Tag> },
        { key: 'database', label: t('relayDatabase'), children: t(!unavailable && snapshot.health ? 'relayDatabaseReady' : 'unknown') },
        { key: 'sample', label: t('relaySnapshot'), children: date(snapshot.sampled_at) },
      ]}/>
    </Card>
    <div className="relay-statistics">{relayCategories.map(category => <Button aria-label={`${t(`relayCount.${category}`)}: ${snapshot.summary[category]}`} className={`relay-statistic relay-category-${category}`} key={category} onClick={() => onSelect(category)}><span>{t(`relayCount.${category}`)}</span><strong>{snapshot.summary[category]}</strong></Button>)}</div>
  </Space>;
}
