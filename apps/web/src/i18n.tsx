import { createContext, useContext } from 'react';

const zh = {
  incorrect_password: '当前密码不正确',
  "sessionDuration":"会话有效期","deviceActor":"设备端","audit.admin.bootstrap":"初始化管理员","audit.account.updated":"修改账号","audit.account.password_changed":"修改密码","audit.device.rename":"重命名设备","audit.device.claim_requested":"申请认领","audit.device.claim_approved":"设备同意认领","audit.device.claim_rejected":"设备拒绝认领","audit.device.claim_cancelled":"取消认领","audit.device.unbound":"解除设备归属","audit.team.rename":"重命名 Team","audit.team_created":"创建 Team","audit.member_added":"添加成员","audit.member_removed":"移除成员","audit.limits_updated":"修改带宽","audit.traffic.assignment":"修改流量归属",
  nodes: 'Relay 节点', controlStatus: '控制连接', policyApplied: '已应用策略', policyOffered: '服务器策略', lastSeen: '最后上报', relayStatusHint: '此处显示节点控制连接与策略同步状态，不代表端到端传输质量。', service: '服务配置', registration: '开放注册',

  last_admin: '至少需要保留一名有效的服务器管理员', enabled: '已启用', disabled: '已停用', role: '角色', edit: '编辑',
  createTeam: '创建 Team', teamName: 'Team 名称', teamOwner: '创建人', members: '成员', addMember: '添加成员', removeMember: '移除成员',
  member: '成员', teamAdmin: 'Team 管理员', teamOwnerRole: '所有者', totalBandwidth: '总带宽', memberBandwidth: '成员带宽',
  defaultTeam: '默认流量归属', personal: '个人', assignmentHint: '变更后下次登录生效；目标账号必须是有效成员',
  saved: '已保存', removeConfirm: '确认移除此成员？其默认流量归属也会清除。', accountUpdateHint: '保存后该账号的网页会话会退出。停用账号也会影响其认证与 Relay 授权。',
  actor: '操作者', action: '变更', resource: '对象', time: '时间', limits: '带宽限制', effectiveScope: '生效归属',
  teamHint: 'Team 用于共享 Relay 带宽，不授予其他成员的设备访问权限', relayHint: '仅经过 Relay 的流量受此限制，P2P 直连不计入',
  product: 'Pixels Agent Bridge', console: '管理控制台', login: '登录', register: '注册账号', logout: '退出登录',
  username: '用户名', password: '密码', confirmPassword: '确认密码', currentPassword: '当前密码', newPassword: '新密码',
  loginIntro: '管理你的设备与网络连接', registerIntro: '创建账号，开始管理你的设备',
  overview: '概览', devices: '设备列表', onlineDevices: '在线设备',
  accounts: '账号管理', teams: 'Team', relay: 'Relay', events: '管理变更', settings: '设置', account: '我的账号',
  language: '语言', appearance: '外观', light: '亮色', dark: '暗色', about: '关于', version: '版本',
  retry: '重试', save: '保存', cancel: '取消', confirm: '确认', back: '返回', loading: '正在加载',
  sessionExpired: '登录已过期，请重新登录', server_error: '服务暂时不可用，请稍后重试',
  networkError: '无法连接服务器，请检查网络后重试', unauthorized: '用户名或密码错误，或账号不可用',
  invalid_input: '请检查填写的内容', conflict: '数据已变化或名称已存在，请刷新后重试',
  forbidden: '你没有操作权限', not_found: '内容不存在或已无权访问', rate_limited: '请求过于频繁，请稍后重试',
  origin_rejected: '当前页面来源不受信任，请从服务器地址重新打开', registration_disabled: '服务器已关闭注册',
  required: '请填写此项', passwordMin: '密码至少 10 个字符', passwordMismatch: '两次密码不一致',
  usernameHint: '3–64 个字符，不含空格或斜杠', alreadyAccount: '已有账号？登录', createAccount: '创建新账号',
  passwordChanged: '密码已修改，请重新登录', changePassword: '修改密码', admin: '服务器管理员', user: '普通用户',
  noData: '暂无数据', refresh: '刷新', totalDevices: '设备总数', online: '在线', offline: '离线', unknown: '未知',
  deviceCode: '设备码', name: '名称', system: '系统', lastOnline: '最后在线', search: '搜索名称或设备码',
  all: '全部', details: '详情', rename: '重命名',
  copy: '复制信息', copied: '已复制设备信息', actions: '操作', total: '总数', disconnected: '状态更新已中断，正在重连',
  connected: '状态更新正常', deviceOnlineHint: '在线表示设备已连接到服务器', architecture: '架构', agentVersion: '客户端版本',
  created: '登记时间', renamed: '名称已更新', nameHint: '这是服务器设备名称，不影响 Desktop 本地备注',

} as const;
type Key = keyof typeof zh;
const en: Record<Key,string> = {
  incorrect_password: 'The current password is incorrect.',
  "sessionDuration":"Session lifetime","deviceActor":"Device","audit.admin.bootstrap":"Administrator provisioned","audit.account.updated":"Account updated","audit.account.password_changed":"Password changed","audit.device.rename":"Device renamed","audit.device.claim_requested":"Claim requested","audit.device.claim_approved":"Claim approved by device","audit.device.claim_rejected":"Claim rejected by device","audit.device.claim_cancelled":"Claim cancelled","audit.device.unbound":"Device ownership removed","audit.team.rename":"Team renamed","audit.team_created":"Team created","audit.member_added":"Member added","audit.member_removed":"Member removed","audit.limits_updated":"Bandwidth updated","audit.traffic.assignment":"Traffic assignment changed",
  nodes:'Relay nodes',controlStatus:'Control connection',policyApplied:'Applied policy',policyOffered:'Server policy',lastSeen:'Last report',relayStatusHint:'Node control connectivity and policy synchronization do not measure end-to-end transfer quality.',service:'Service configuration',registration:'Public registration',

  last_admin:'At least one active server administrator is required.',enabled:'Enabled',disabled:'Disabled',role:'Role',edit:'Edit',createTeam:'Create team',teamName:'Team name',teamOwner:'Owner account',members:'Members',addMember:'Add member',removeMember:'Remove member',member:'Member',teamAdmin:'Team administrator',teamOwnerRole:'Owner',totalBandwidth:'Total bandwidth',memberBandwidth:'Member bandwidth',defaultTeam:'Default traffic scope',personal:'Personal',assignmentHint:'Applies on the next sign-in. The account must be an active team member.',saved:'Saved',removeConfirm:'Remove this member and clear their default traffic assignment?',accountUpdateHint:'Saving signs out this account’s browser sessions. Disabling also affects authentication and Relay authorization.',actor:'Actor',action:'Change',resource:'Resource',time:'Time',limits:'Bandwidth limits',effectiveScope:'Effective scope',teamHint:'Teams share Relay bandwidth. Membership does not grant access to other members’ devices.',relayHint:'These limits apply to Relay traffic. Direct P2P traffic is excluded.',
  product:'Pixels Agent Bridge',console:'Management console',login:'Sign in',register:'Create account',logout:'Sign out',
  username:'Username',password:'Password',confirmPassword:'Confirm password',currentPassword:'Current password',newPassword:'New password',
  loginIntro:'Manage your devices and network',registerIntro:'Create an account to manage your devices',
  overview:'Overview',devices:'Device list',onlineDevices:'Online devices',accounts:'Accounts',teams:'Teams',relay:'Relay',events:'Management changes',settings:'Settings',account:'My account',
  language:'Language',appearance:'Appearance',light:'Light',dark:'Dark',about:'About',version:'Version',retry:'Retry',save:'Save',cancel:'Cancel',confirm:'Confirm',back:'Back',loading:'Loading',
  sessionExpired:'Your session expired. Please sign in again.',server_error:'The service is unavailable. Please try again.',networkError:'Cannot reach the server. Check your connection.',unauthorized:'Invalid credentials or account unavailable.',invalid_input:'Check the entered values.',conflict:'Data changed or the name is taken. Refresh and try again.',forbidden:'You do not have permission.',not_found:'This resource is unavailable.',rate_limited:'Too many requests. Please try again later.',origin_rejected:'Open this page from the server’s trusted address.',registration_disabled:'Registration is disabled.',
  required:'Required',passwordMin:'Use at least 10 characters.',passwordMismatch:'Passwords do not match.',usernameHint:'3–64 characters, without spaces or slashes',alreadyAccount:'Already registered? Sign in',createAccount:'Create an account',passwordChanged:'Password updated. Please sign in again.',changePassword:'Change password',admin:'Server administrator',user:'User',noData:'No data',refresh:'Refresh',totalDevices:'Total devices',online:'Online',offline:'Offline',unknown:'Unknown',deviceCode:'Device code',name:'Name',system:'System',lastOnline:'Last online',search:'Search name or device code',all:'All',details:'Details',rename:'Rename',copy:'Copy information',copied:'Device information copied',actions:'Actions',total:'Total',disconnected:'Live updates interrupted. Reconnecting…',connected:'Live updates connected',deviceOnlineHint:'Online means the device is connected to the server.',architecture:'Architecture',agentVersion:'Agent version',created:'Registered',renamed:'Name updated',nameHint:'This is the server device name. Desktop local aliases are unchanged.',
};
const tw: Record<Key,string> = {
  ...zh,"sessionDuration":"工作階段有效期","deviceActor":"裝置端","audit.admin.bootstrap":"初始化管理員","audit.account.updated":"修改帳號","audit.account.password_changed":"修改密碼","audit.device.rename":"重新命名裝置","audit.device.claim_requested":"申請認領","audit.device.claim_approved":"裝置同意認領","audit.device.claim_rejected":"裝置拒絕認領","audit.device.claim_cancelled":"取消認領","audit.device.unbound":"解除裝置歸屬","audit.team.rename":"重新命名 Team","audit.team_created":"建立 Team","audit.member_added":"新增成員","audit.member_removed":"移除成員","audit.limits_updated":"修改頻寬","audit.traffic.assignment":"修改流量歸屬",console:'管理控制台',register:'註冊帳號',logout:'登出',login:'登入',username:'使用者名稱',password:'密碼',confirmPassword:'確認密碼',currentPassword:'目前密碼',newPassword:'新密碼',
  loginIntro:'管理你的裝置與網路連線',registerIntro:'建立帳號，開始管理你的裝置',overview:'概覽',devices:'裝置清單',onlineDevices:'線上裝置',accounts:'帳號管理',teams:'Team',events:'管理變更',settings:'設定',account:'我的帳號',language:'語言',appearance:'外觀',light:'亮色',dark:'暗色',about:'關於',version:'版本',retry:'重試',save:'儲存',cancel:'取消',confirm:'確認',back:'返回',loading:'正在載入',
  sessionExpired:'登入已過期，請重新登入',server_error:'服務暫時無法使用，請稍後重試',networkError:'無法連線至伺服器，請檢查網路後重試',unauthorized:'使用者名稱或密碼錯誤，或帳號無法使用',invalid_input:'請檢查填寫的內容',conflict:'資料已變更或名稱已存在，請重新整理後重試',forbidden:'你沒有操作權限',not_found:'內容不存在或已無權存取',rate_limited:'請求過於頻繁，請稍後重試',origin_rejected:'請從伺服器的受信任位址重新開啟',registration_disabled:'伺服器已關閉註冊',required:'請填寫此項',passwordMin:'密碼至少 10 個字元',passwordMismatch:'兩次密碼不一致',usernameHint:'3–64 個字元，不含空格或斜線',alreadyAccount:'已有帳號？登入',createAccount:'建立新帳號',passwordChanged:'密碼已修改，請重新登入',changePassword:'修改密碼',admin:'伺服器管理員',user:'一般使用者',noData:'暫無資料',refresh:'重新整理',totalDevices:'裝置總數',online:'線上',offline:'離線',unknown:'未知',deviceCode:'裝置碼',name:'名稱',system:'系統',lastOnline:'最後上線',search:'搜尋名稱或裝置碼',all:'全部',details:'詳細資訊',rename:'重新命名',copy:'複製資訊',copied:'已複製裝置資訊',actions:'操作',total:'總數',disconnected:'狀態更新已中斷，正在重新連線',connected:'狀態更新正常',deviceOnlineHint:'線上表示裝置已連線至伺服器',architecture:'架構',agentVersion:'用戶端版本',created:'登記時間',renamed:'名稱已更新',nameHint:'這是伺服器裝置名稱，不影響 Desktop 本機備註',
  last_admin:'至少需要保留一名有效的伺服器管理員',enabled:'已啟用',disabled:'已停用',role:'角色',edit:'編輯',createTeam:'建立 Team',teamName:'Team 名稱',teamOwner:'建立者',members:'成員',addMember:'新增成員',removeMember:'移除成員',member:'成員',teamAdmin:'Team 管理員',teamOwnerRole:'擁有者',totalBandwidth:'總頻寬',memberBandwidth:'成員頻寬',defaultTeam:'預設流量歸屬',personal:'個人',assignmentHint:'變更後下次登入生效；目標帳號必須是有效成員',saved:'已儲存',removeConfirm:'確認移除此成員？其預設流量歸屬也會清除。',accountUpdateHint:'儲存後該帳號的網頁工作階段會登出。停用也會影響驗證與 Relay 授權。',actor:'操作者',action:'變更',resource:'物件',time:'時間',limits:'頻寬限制',effectiveScope:'生效歸屬',teamHint:'Team 用於共用 Relay 頻寬，不授予其他成員的裝置存取權限',relayHint:'僅經過 Relay 的流量受此限制，P2P 直連不計入',

};
export type Language = 'zh-CN' | 'zh-TW' | 'en';
Object.assign(tw, { incorrect_password:'目前密碼不正確',nodes:'Relay 節點',controlStatus:'控制連線',policyApplied:'已套用策略',policyOffered:'伺服器策略',lastSeen:'最後回報',relayStatusHint:'此處顯示節點控制連線與策略同步狀態，不代表端到端傳輸品質。',service:'服務設定',registration:'開放註冊' });
export const dictionaries = { 'zh-CN': zh, 'zh-TW': tw, en };
export function detectLanguage(value: string): Language {
  if (/^zh-(TW|HK|MO|Hant)/i.test(value)) return 'zh-TW';
  return /^zh/i.test(value) ? 'zh-CN' : 'en';
}
export const LanguageContext = createContext<Language>('en');
export function useDate() {
  const language = useContext(LanguageContext);
  return (value: number) => new Date(value).toLocaleString(language);
}
export function useText() {
  const language = useContext(LanguageContext);
  return (key: Key | string) => dictionaries[language][(key === 'audit.limits_changed' ? 'audit.limits_updated' : key) as Key] ?? (key.startsWith('audit.') ? key.slice(6) : dictionaries[language].server_error);
}
