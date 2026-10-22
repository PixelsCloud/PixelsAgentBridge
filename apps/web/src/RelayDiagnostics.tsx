import { useContext, useEffect, useState } from 'react';
import { Alert, Button, Card, Collapse, Descriptions, Empty, Input, Space, Table, Tabs, Tag, Tooltip, Typography } from 'antd';
import { Activity, ArrowDownUp, Cpu, Network } from 'lucide-react';
import { ConsoleSelect, paginationComponents } from './ConsoleSelect';
import { LanguageContext, useDate, useText } from './i18n';
import { useResource } from './useResource';
import { byteText, durationText } from './usageModel';

type Identity = { endpoint:string;kind:string;device_id?:string;name?:string;code?:string;username?:string;system?:string;os_name?:string;os_version?:string;architecture?:string;agent_version?:string };
type Row = { id:string;identity?:Identity;a?:Identity;b?:Identity;status:string;duration_ms:string;connected_at?:number;started_at?:number;last_forwarded_at?:number;closed_at?:number;a_to_b_bytes?:string;b_to_a_bytes?:string;a_to_b_packets?:string;b_to_a_packets?:string;a_to_b_rate?:string;b_to_a_rate?:string };
type Runtime = { supported:boolean;fresh:boolean;sampled_at?:number;host?:{hostname?:string;os:string;os_version?:string;architecture:string;cpu_model?:string;logical_cpus:number;cpu_usage_basis_points?:number;memory_total:string;memory_used:string;host_uptime_ms:string;process_uptime_ms:string;process_memory?:string;containerized:boolean};traffic?:Record<string,string>;summary?:{connections:string|null;connected_endpoints:number|null;active_links:number|null;idle_links:number|null;recent_closed_links:number;connections_truncated:boolean;links_truncated:boolean};items:Row[];total:number;page:number;page_size:number };

export function RelayDiagnostics({ nodeId }: { nodeId:string }) {
  const t=useText();const date=useDate();const language=useContext(LanguageContext);
  const [view,setView]=useState('connections');const [status,setStatus]=useState('all');const [search,setSearch]=useState('');const [page,setPage]=useState(1);const [pageSize,setPageSize]=useState(20);
  const query=new URLSearchParams({view,status,q:search,page:String(page),page_size:String(pageSize)});
  const resource=useResource<Runtime>(`/relays/${encodeURIComponent(nodeId)}/runtime?${query}`,{retainPreviousData:true});
  const data=resource.data;const fresh=!!data?.fresh&&!resource.error;const host=data?.host;const traffic=data?.traffic;const summary=data?.summary;
  useEffect(()=>{
    const poll=()=>{if(document.visibilityState!=='hidden')resource.refresh();};const timer=window.setInterval(poll,5000);document.addEventListener('visibilitychange',poll);
    return()=>{clearInterval(timer);document.removeEventListener('visibilitychange',poll);};
  },[resource.refresh]);
  const bytes=(v?:string)=>v==null?'—':byteText(v);
  const duration=(v?:string)=>v==null?'—':durationText(v,language);
  function endpoint(value?:Identity){if(!value)return '—';return <Space orientation="vertical" size={1} className="relay-endpoint-identity"><Typography.Text strong>{value.name??value.username??t(`relayIdentity.${value.kind}`)}</Typography.Text><Typography.Text type="secondary">{[value.code,value.name?value.username:undefined,value.system,value.architecture,value.agent_version].filter(Boolean).join(' · ')||t(`relayIdentity.${value.kind}`)}</Typography.Text><Typography.Text copyable={{text:value.endpoint}} title={value.endpoint} type="secondary">{value.endpoint.slice(0,12)}…</Typography.Text></Space>;}
  function state(row:Row){const status=!fresh&&row.status!=='closed'?'unknown':row.status;return <Tag color={status==='active'||status==='connected'?'success':status==='idle'?'default':undefined}>{t(`relayRuntimeState.${status}`)}</Tag>;}
  return <Space orientation="vertical" size={16} className="relay-runtime" data-testid="relay-runtime" style={{width:'100%'}}>
    <div className="relay-runtime-heading"><Space><Activity size={17}/><Typography.Text strong>{t('relayRuntime')}</Typography.Text>{data?.supported&&<Tag>{t(fresh?'relaySampleLive':'relaySampleStale')}</Tag>}</Space><Button onClick={resource.refresh}>{t('refresh')}</Button></div>
    {resource.error&&<Alert type="error" showIcon title={t(resource.error)}/>}
    {data&&!data.supported?<Empty description={t('relayRuntimeUnavailable')}/>:<>
      <div className="relay-runtime-stats">{[
        ['relayClientConnections',summary?.connections],['relayConnectedEndpoints',summary?.connected_endpoints],['relayActiveLinks',summary?.active_links],['relayIdleLinks',summary?.idle_links],
      ].map(([label,value])=><div key={label}><span>{t(String(label))}</span><strong>{fresh?value??'—':'—'}</strong></div>)}</div>
      {host&&<Card size="small" title={<Space><Cpu size={16}/>{t('relayHostInfo')}</Space>} extra={<Tag>{t(host.containerized?'relayContainer':'relayHost')}</Tag>}>
        <Descriptions column={{xs:1,sm:2}} size="small" items={[
          {key:'hostname',label:t('relayHostname'),children:host.hostname??t('unknown')},
          {key:'system',label:t('system'),children:[host.os_version??host.os,host.architecture].join(' · ')},
          {key:'cpu',label:'CPU',children:[host.cpu_model,`${host.logical_cpus} ${t('relayCores')}`].filter(Boolean).join(' · ')},
          {key:'cpuUsage',label:t('relayCpuUsage'),children:fresh&&host.cpu_usage_basis_points!=null?`${(host.cpu_usage_basis_points/100).toFixed(1)}%`:'—'},
          {key:'memory',label:t('relaySystemMemory'),children:fresh?`${bytes(host.memory_used)} / ${bytes(host.memory_total)}`:'—'},
          {key:'processMemory',label:t('relayProcessMemory'),children:fresh?bytes(host.process_memory):'—'},
          {key:'uptime',label:t('relayHostUptime'),children:duration(host.host_uptime_ms)},
          {key:'processUptime',label:t('relayProcessUptime'),children:duration(host.process_uptime_ms)},
        ]}/>
      </Card>}
      {traffic&&<Collapse size="small" items={[{key:'traffic',label:<Space><ArrowDownUp size={16}/>{t('relayTrafficDetails')}</Space>,children:<Descriptions column={{xs:1,sm:2}} size="small" items={[
        {key:'rx',label:t('relayNodeReceived'),children:bytes(traffic.received_bytes)},
        {key:'tx',label:t('relayNodeSent'),children:bytes(traffic.sent_bytes)},
        {key:'rxRate',label:t('relayReceiveRate'),children:fresh?`${bytes(traffic.received_bytes_per_second)}/s`:'—'},
        {key:'txRate',label:t('relaySendRate'),children:fresh?`${bytes(traffic.sent_bytes_per_second)}/s`:'—'},
        ...['received_packets','sent_packets','dropped_packets','accepted_connections','disconnected_connections','ping_frames'].map(key=>({key,label:t(`relayTraffic.${key}`),children:BigInt(traffic[key]??'0').toLocaleString(language)})),
        {key:'limited',label:t('relayRateLimitedBytes'),children:bytes(traffic.rate_limited_bytes)},
      ]}/>}]} />}
      <Tabs activeKey={view} onChange={value=>{setView(value);setStatus('all');setPage(1);}} items={[{key:'connections',label:<Space><Network size={16}/>{t('relayConnectionsDetail')}</Space>},{key:'links',label:<Space><ArrowDownUp size={16}/>{t('relayLinksDetail')}</Space>}]}/>
      <Space wrap><Input.Search aria-label={t('relayRuntimeSearch')} placeholder={t('relayRuntimeSearch')} onSearch={value=>{setSearch(value);setPage(1);}} allowClear maxLength={128} style={{width:260,maxWidth:'100%'}}/><ConsoleSelect aria-label={t('relayConnectionStatus')} value={status} onChange={value=>{setStatus(value);setPage(1);}} options={(view==='connections'?['all','connected','closed']:['all','active','idle','closed']).map(value=>({value,label:value==='all'?t('all'):t(`relayRuntimeState.${value}`)}))}/>{view==='links'&&<Tooltip title={t('relayLinkSemantics')}><Typography.Text type="secondary">{t('relayEndpointPair')}</Typography.Text></Tooltip>}</Space>
      {summary&&(summary.connections_truncated||summary.links_truncated)&&<Typography.Text type="secondary">{t('relayInventoryLimited')}</Typography.Text>}
      <Table<Row> size="small" rowKey="id" dataSource={data?.items??[]} loading={!data&&resource.loading} scroll={{x:view==='connections'?750:1050}} expandable={{expandedRowRender:row=><Descriptions size="small" column={{xs:1,sm:2}} items={[
        {key:'id',label:t('relayRecordId'),children:<Typography.Text copyable>{row.id}</Typography.Text>},
        {key:'start',label:t(view==='connections'?'relayConnectedAt':'relayFirstForwarded'),children:date(row.connected_at??row.started_at??0)},
        {key:'end',label:t('relayDisconnectedAt'),children:row.closed_at?date(row.closed_at):'—'},
        ...(view==='links'?[
          {key:'ab',label:'A → B '+t('relayDataFrames'),children:BigInt(row.a_to_b_packets??'0').toLocaleString(language)},
          {key:'ba',label:'B → A '+t('relayDataFrames'),children:BigInt(row.b_to_a_packets??'0').toLocaleString(language)},
        ]:[{key:'system',label:t('system'),children:[row.identity?.os_name,row.identity?.os_version,row.identity?.architecture].filter(Boolean).join(' · ')||t('unknown')}]),
      ]}/>}} pagination={{components:paginationComponents,current:data?.page??page,pageSize:data?.page_size??pageSize,total:data?.total??0,showSizeChanger:true,onChange:(page,size)=>{setPage(page);setPageSize(size);}}} columns={view==='connections'?[
        {title:t('relayConnectedMachine'),render:(_,row)=>endpoint(row.identity)},
        {title:t('relayConnectionStatus'),render:(_,row)=>state(row)},
        {title:t('relayConnectedAt'),render:(_,row)=>row.connected_at?date(row.connected_at):'—'},
        {title:t('relayConnectedDuration'),render:(_,row)=>duration(row.duration_ms)},
        {title:t('relayDisconnectedAt'),render:(_,row)=>row.closed_at?date(row.closed_at):'—'},
      ]:[
        {title:t('relaySideA'),render:(_,row)=>endpoint(row.a)},
        {title:t('relaySideB'),render:(_,row)=>endpoint(row.b)},
        {title:t('relayConnectionStatus'),render:(_,row)=>state(row)},
        {title:t('relayObservedDuration'),render:(_,row)=><Tooltip title={row.started_at?date(row.started_at):undefined}>{duration(row.duration_ms)}</Tooltip>},
        {title:t('relayBidirectionalTraffic'),render:(_,row)=><Space orientation="vertical" size={1}><span>A → B {bytes(row.a_to_b_bytes)}</span><span>B → A {bytes(row.b_to_a_bytes)}</span></Space>},
        {title:t('relaySampleRate'),render:(_,row)=><Space orientation="vertical" size={1}><span>A → B {fresh?`${bytes(row.a_to_b_rate)}/s`:'—'}</span><span>B → A {fresh?`${bytes(row.b_to_a_rate)}/s`:'—'}</span></Space>},
        {title:t('relayLastForwarded'),render:(_,row)=>row.last_forwarded_at?date(row.last_forwarded_at):'—'},
      ]}/>
      {data?.sampled_at&&<Typography.Text type="secondary">{t('relayTelemetryAt')}: {date(data.sampled_at)}</Typography.Text>}
    </>}
  </Space>;
}
