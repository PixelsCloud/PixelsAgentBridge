import { useEffect } from 'react';
import { Tooltip } from 'antd';
import { useResource } from './useResource';
import { useText } from './i18n';
import { byteText } from './usageModel';

interface MachineSample {
  supported: boolean; fresh: boolean;
  host?: { logical_cpus: number; cpu_usage_basis_points?: number | null; memory_total: string; memory_used: string; process_memory?: string; containerized: boolean };
  traffic?: { received_bytes: string; sent_bytes: string; received_bytes_per_second: string; sent_bytes_per_second: string };
}

export function RelayMachineStatus({ nodeId, current }: { nodeId: string; current: boolean }) {
  const t = useText();
  const resource = useResource<MachineSample>(`/relays/${encodeURIComponent(nodeId)}/runtime?view=connections&page_size=1`);
  useEffect(() => {
    const poll = () => { if (document.visibilityState !== 'hidden') resource.refresh(); };
    const timer = window.setInterval(poll, 5000);
    document.addEventListener('visibilitychange', poll);
    return () => { clearInterval(timer); document.removeEventListener('visibilitychange', poll); };
  }, [resource.refresh]);
  const data = resource.data; const host = data?.host; const traffic = data?.traffic;
  const fresh = current && data?.supported && data.fresh && !resource.error;
  const cpu = host?.cpu_usage_basis_points;
  const memory = host && BigInt(host.memory_total) > 0n ? Number(BigInt(host.memory_used) * 1000n / BigInt(host.memory_total)) / 10 : undefined;
  const bytes = (value?: string) => value == null ? '—' : byteText(value);
  const missing = resource.error ? t(resource.error) : data?.supported === false ? t('relayRuntimeUnavailable') : t('relaySampleStale');
  return <div className="relay-machine-status" aria-label={`${t('relayMachineLoad')} ${nodeId}`}>
    <Tooltip title={fresh && host ? `${host.logical_cpus} ${t('relayCores')} · ${t(host.containerized ? 'relayContainer' : 'relayHost')}` : missing}>
      <div><span>CPU</span><strong>{fresh && cpu != null ? `${(cpu / 100).toFixed(1)}%` : '—'}</strong></div>
    </Tooltip>
    <Tooltip title={fresh && host ? `${t('relaySystemMemory')}: ${bytes(host.memory_used)} / ${bytes(host.memory_total)} · ${t('relayProcessMemory')}: ${bytes(host.process_memory)}` : missing}>
      <div><span>{t('relayMemoryShort')}</span><strong>{fresh && memory != null ? `${memory.toFixed(1)}%` : '—'}</strong><small>{fresh && host ? `${bytes(host.memory_used)} / ${bytes(host.memory_total)}` : '—'}</small></div>
    </Tooltip>
    <Tooltip title={fresh && traffic ? `${t('relayNodeReceived')}: ${bytes(traffic.received_bytes)}` : missing}>
      <div><span>{t('relayInboundShort')}</span><strong>{fresh && traffic ? `${bytes(traffic.received_bytes_per_second)}/s` : '—'}</strong></div>
    </Tooltip>
    <Tooltip title={fresh && traffic ? `${t('relayNodeSent')}: ${bytes(traffic.sent_bytes)}` : missing}>
      <div><span>{t('relayOutboundShort')}</span><strong>{fresh && traffic ? `${bytes(traffic.sent_bytes_per_second)}/s` : '—'}</strong></div>
    </Tooltip>
  </div>;
}
