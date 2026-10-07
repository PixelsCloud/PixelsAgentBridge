# 执行身份、用户环境与应用管理：跨平台长任务

规划日期：2026-10-06。代码基线：main / dfe7221；已验证的 Finder 修复和 1.2.26 构建版本同步已提交并 push。
状态（2026-10-07）：E0 原型以及命令、终端、Git、文件、目录、传输的指定用户执行已接入源码；分层回归与三平台原生账户测试已推进。当前缺口是用户环境/凭据、应用入口、最小 UI、完整异常矩阵及安装后的正式宿主验收；详细证据见第 12 节。已有工具不重做，源码集成不等同最终安装交付。

阶段证据：[`acceptance/execution-e0-2026-10-06.md`](acceptance/execution-e0-2026-10-06.md)。
原型证明切换后创建文件和运行既有 portable-pty 可行；没有将原型误标为完整 MCP 功能交付。

2026-10-06 初始进度（历史记录，最新状态见第 12 节）：
- Finder 修复已在 dfe7221 提交；E0 原生身份原型及终端重复关闭修复已在 2da524a 提交并 push。
- E1 基础类型已实现：严格 execution 选择、原生身份观察、按设备/调用方/连接绑定的上下文注册表。
  两个 MCP 即使共享 endpoint key，也不能复用或释放彼此的执行上下文。
- 现有 ExecutionContext 新增可选 identity，历史记录缺失时明确表示未观察；原生环境 revision v2 纳入账户及登录代次。
- **E1 尚未整体完成**：上下文查询工具、能力协商、工具参数、持久化指纹接入仍待实现。
  当前没有向 MCP 声明可切换用户的正式能力，已安装软件未更换。

## 0. 已完成与实际缺口（范围校正）

上一版将“已有实现”“接入新身份”和“回归验证”统一写成必做，容易误认为全部要重新开发。
以本节为范围依据：下面各章节的测试要求，只用于证明新增身份/应用入口没有破坏已有行为。

| 能力 | 已有证据 | 本轮增量 |
|---|---|---|
| 原生命令、cwd、env、stdin、超时与输出 | protocol/task_session.rs已有CommandTaskSpec/CommandOptions；正式pab_run_command已可用 | 增加明确目标用户，按目标用户解析环境/程序；不重写命令工具 |
| 执行环境回传与变化检测 | protocol/platform.rs已有OS、架构、cwd、environment_revision及ExpectedEnvironment | 补实际账户/会话；不重新设计整套ExecutionContext |
| 交互终端创建/输入/读取/resize/关闭 | terminal基于portable-pty，五个正式终端工具已存在 | 创建终端时使用指定账户及环境，后续固定身份 |
| Git工具、仓库锁、异步执行、凭据错误 | executor/task_service/git.rs及现有正式Git工具 | Git子进程和仓库文件访问切换到目标账户；不是再开发Git功能 |
| 文件、归档、hash、异步二进制传输 | 现有filesystem及upload/download工具 | 在指定身份下进行访问/创建/发布；保留原接口和数据协议 |
| 去重、取消、恢复、操作查询、本地历史 | 已有操作框架与验收记录 | 将身份纳入指纹和结果，补身份相关异常用例 |
| 窗口、显示器、JPEG截图、键鼠、四个UI工具 | UI_AUTOMATION_ROADMAP和原生宿主两端两轮验收 | 针对身份路由的回归；不重新做截图、控件、输入系统 |
| 窗口激活、关闭、进程终止 | pab_focus_window / pab_window_control / pab_terminate_process | 优先复用；不能把窗口关闭等同整个多窗口应用已退出 |
| Finder查询 | 1.2.26修复、安装及21项原生回归通过 | 先提交现有变更；不再列为开发任务 |
| Desktop、主题、三语、本地任务与状态上报 | 现有React/Ant Design及Desktop实现 | 仅加执行身份必要字段/选择入口；不重做设置、主题或任务页 |
| Windows/Mac包、免费签名、版本递增 | 现有build.py和安装验收 | 使用原管线构建更新，测试新增身份兼容性 |
| Linux无界面产品 | 已有命令与文件实机记录 | 维护现有产品，新增用户身份实测；不能将所有工具在Linux都算作已验收 |

真正新增的两块：
1. **跨平台目标用户执行**：查可用身份 → 选定账户/会话 → 现有工具以该身份执行 → 返回实际身份。
2. **Windows/macOS结构化应用入口**：应用发现、启动、打开文件；窗口聚焦/关闭等能力复用已有工具。

已有service账户执行的功能是基线，不是待开发功能。不能仅因本轮列了用例就重复编写整个子系统。
主要源码核对：crates/protocol/src/task_session.rs、platform.rs、crates/terminal/src/lib.rs、
crates/executor/src/task_service/git.rs；已有实机证据见acceptance/2026-10-06.md、
acceptance/ui-u0-2026-10-06.md和acceptance/finder-window-fix-2026-10-06.md。

## 1. 目标与平台边界

让 Agent 明确“在哪台设备、以哪个用户、在哪个会话和目录中”执行操作。
命令、终端、Git、文件操作使用一致的执行身份；Windows/macOS 提供应用启动、激活、正常退出和打开文件。
减少为切换用户、获取正确环境、启动桌面应用而拼接 PowerShell、sudo、launchctl 的需求。

| 能力 | Windows 桌面版 | macOS 桌面版 | Linux 无界面版 |
|---|---|---|---|
| 查询可用目标身份、补实际账户字段 | 新增 | 新增 | 新增 |
| 命令、交互终端、Git、文件/二进制传输 | 现有工具接入身份 | 现有工具接入身份 | 现有工具接入身份 |
| 目标用户目录、环境、凭据和文件归属 | 增强 | 增强 | 增强 |
| 应用发现、启动、打开文件入口 | 新增；其余复用 | 新增；其余复用 | 明确不支持桌面操作 |
| 窗口、截图、控件、桌面输入 | 已实现，针对性回归 | 已实现，针对性回归 | 无桌面能力，不启动图形辅助进程 |
| GUI 客户端与相关设置 | 沿用 React + Ant Design | 沿用 React + Ant Design | 本轮不开发 |

Linux 已有无界面版，本轮是保持并增强该产品，不是宣布“不支持 Linux”。
已有 X11/桌面实验代码不作为当前 Linux 产品承诺；清理 README 的交付状态描述，不因文案调整随意删除旧实现。
Linux 服务启动、注册、网络连接、身份切换及非图形工具不得依赖 DISPLAY、WAYLAND_DISPLAY、桌面 helper 或 GUI 登录。

不恢复 Bridge Host、Server 任务记录、deployment ID、设备连接租期或逐条审批。
任务仍只保存在本地，各 MCP 独立连接；内部执行工作进程不承载共享网络连接，不增加独立安装产品。

## 2. 开始前的事实核对与原型门槛

1. 先提交已验证的 Finder 修复及 1.2.26 版本变更，再 push；保留本机和 Mac 工作树中未知来源的改动。
2. 核对 Windows 本机、90（211399447）、Mac（603527578）的安装版、服务身份和当前会话。
3. 找到可用 Linux 无界面测试环境。优先现有 Pixels 设备；否则搭建真实 Linux VM/容器组合：
   VM/systemd 验证安装及服务，无 logind 容器验证核心能力不依赖登录管理器。
   没有图形桌面不是阻塞；缺少 Linux 运行环境时先完成可独立工作，但不能把核心 Linux 验收记为通过。
4. 逐项盘点命令/终端/Git/文件/上传下载执行入口，确认目前真实权限边界，而非只改工具参数。
5. 仅针对新增身份切换、用户PTY和应用入口做选型原型；既有UIA/AX、截图、文件传输库不重新选型。记录新增依赖及边界。

优先复用现有 windows/windows-sys、rustix/libc、portable-pty、Git、会话 helper、NSWorkspace 绑定。
portable-pty 是否能在目标用户的 Windows token 下创建 ConPTY 必须先实测；不能把环境变量改成用户名当作身份切换。
平台进程管理优先放入已有平台层；共享解析/状态机放公共 Rust 层，避免业务代码散落平台判断。

官方依据：
- [Microsoft CreateProcessAsUserW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessasuserw)：用户 token、环境与 profile 是不同工作，必须分别处理并管理生命周期。
- [Apple NSWorkspace](https://developer.apple.com/documentation/AppKit/NSWorkspace)：复用系统应用启动、运行实例与文件打开机制，实际会话路由另行验证。
- [Linux initgroups](https://man7.org/linux/man-pages/man3/initgroups.3.html) / [posix_spawn](https://man7.org/linux/man-pages/man3/posix_spawn.3.html)：补充组与进程创建属性需要明确处理，不假定 spawn 自动完成任意用户切换。

这些接口是选型起点；原型结果决定具体组合，不在没有验证时承诺任意账户均可切换。

## 3. 统一执行上下文

拟新增一个查询入口 `pab_list_execution_contexts`；与现有 `pab_list_sessions` 分工：
前者返回可用于工具执行的上下文及不可用原因，后者仍描述 OS 会话。
工具名和精确 schema 在 E1 冻结，避免为同一概念增加重复入口。

执行参数拟统一为 `execution`，支持：

| 模式 | 含义 | 规则 |
|---|---|---|
| service | Executor 的服务账户 | 未传 execution 时保持此确定行为，并明确返回实际账户 |
| user | 明确选定的系统用户 | 使用查询返回的上下文引用；按平台实际可用凭据/token 决定能否执行 |
| desktop_user | 明确的交互桌面会话 | Windows/macOS 使用；多个候选时必须显式选择，不猜最近活动用户 |

关键约束：
- 身份可用性按能力区分：能执行命令不代表能访问图形桌面、凭据库或用户服务。
- Windows 任意未登录账户没有可用 token 时明确拒绝；本轮不增加密码保管或交互登录系统。
- Linux 的指定用户不要求登录会话；服务自身权限不足则明确拒绝，不回退 root。
- 上下文绑定设备、账户稳定标识（SID/UID及必要账户核验）、适用会话和调用方。
  会话重建/注销、账户删除或权限变化时重新核验；缓存引用不构成设备连接租期。
- 请求接受时解析一次实际身份，后续重试、查询和去重不再选择另一个当前用户。
- 有关 execution 的所有参数参与请求指纹；相同 request_id 换用户/目录/环境属于冲突。
- 查询结果返回用户名、稳定账户标识、会话、适用能力、缺失条件；不返回密码、token、私钥或完整环境。
- 操作结果返回 requested/actual 身份、实际 cwd、执行域及环境来源/修订标识。
  去重返回原记录，不把后来登录的用户写成原任务的执行者。
- UI 控件和窗口操作继续绑定原 window_ref/helper 会话，不通过 run-as 参数转移已有引用的归属。
- GUI 可保存用户的明确选择，但不会建立影响所有 MCP 的全局“当前执行用户”。

兼容策略：
新增能力版本后再使用 execution；旧端不支持时在执行前提示升级，禁止静默忽略。
缺省 service 是明确的现有行为，工具文档必须说明；要操作用户工作区时显式选择对应上下文。

## 4. 环境、目录和凭据

### 环境与 cwd
- 按选定账户构造 HOME/USER/LOGNAME、USERPROFILE/APPDATA 等适用变量；不继承服务账户的个人目录。
- 未传 cwd 时使用该用户可访问的 home；无法确定/访问则报错。service 模式的既有默认目录须明确记录。
- program 名称使用目标环境 PATH 解析；精确路径不经过隐式 shell。
- 定义环境覆盖优先级：基础系统 → 目标账户环境 → 允许的调用方覆盖。
  身份决定的用户目录变量不能通过覆盖伪装为另一个用户。
- 非交互命令默认不执行 shell 启动脚本。终端显式报告使用的 shell 与启动模式，不默默加载 login shell。
- 对已登录会话的环境仅从该会话获取所需项；不把整个服务环境、其他用户环境复制过去。
- Mac Homebrew、Windows 用户工具目录、Linux 用户 PATH 必须用实机确认；不硬编码 huayang/chess 的路径到产品。

### 凭据
- Git 使用所选用户自己的 config、SSH 配置、known_hosts 和 credential helper。
- SSH_AUTH_SOCK、Windows SSH agent、钥匙串等仅在相应用户/会话真实可用时接入。
- Keychain 锁定、代理不存在、认证需交互时返回明确状态/有界超时，不谎报为仓库不存在。
- 不复制私钥，不创建全局 safe.directory，不自动修改远端配置或仓库归属。
- 不把凭据、环境值、Git认证URL或输入全文写入任务摘要和诊断报告。

## 5. 三平台执行后端

### Windows
复用 WTS/进程 token 与官方进程创建接口；核对用户 profile、环境块、会话和桌面。
命令/PTY/应用进程实际使用选定 token；不能让应用落在 Session 0 后报告已打开。
文件可在明确用户上下文的受控执行线程/进程中操作；异步任务不能跨线程泄漏 impersonation。
优先采用按用户隔离的现有程序内部工作模式，统一生命周期；句柄和 token 不成为客户端可伪造参数。
验证普通账户、管理员登录账户、SYSTEM 服务、RDP/控制台、多会话和注销；管理员账户不等于自动获得完整提升 token。
不触发新增逐命令批准；OS 明确要求而当前条件不具备时直接报告原因。

### macOS
命令/文件在明确 UID、主组及补充组下执行；GUI 工作仍路由到匹配的用户 launchd/桌面会话。
普通用户命令与 GUI bootstrap 域是不同概念，不能只改 UID 后声称桌面环境正确。
保留现有固定免费签名、TCC 申请和会话 helper，不把用户授权转移到随机新二进制。
不在多线程 Executor 全局调用 setuid/seteuid；身份切换在专用子进程安全创建路径完成。

### Linux 无界面版
root 服务可在明确授权范围内切换系统用户；普通用户服务只报告实际可用身份。
正确设置 UID/GID/补充组、umask、home、PATH、cwd，并关闭无关继承句柄。
无需 logind、D-Bus session 或显示服务即可运行核心工具；系统服务管理需要 systemd 时单独报告依赖。
核对无界面构建及打包依赖，避免为核心 Executor 强制安装桌面显示栈。
图形工具返回稳定 unsupported/no_desktop，不尝试启动桌面进程或连接随机显示器。

执行隔离不使用每条命令拼接 sudo/runas 的方案。可使用成熟接口和有限内部工作进程；
其数量、队列、超时、退出回收均有上限，不常驻为新的共享连接中心。
E0 必须验证取消/超时能回收我们启动的工作进程；不能以停止等待冒充停止执行。

## 6. 现有工具接入清单

| 工具族 | 本轮必须覆盖 |
|---|---|
| 命令 | program/argv/stdin、环境、cwd、stdout/stderr、实际身份、取消与退出码 |
| 终端 | 创建时固定身份与环境；后续 session_id 不能换用户；resize/read/input/close 回归 |
| Git | status/diff/log、add/commit、branch/checkout、fetch/pull/push；hooks与凭据也使用目标用户 |
| 文件 | list/stat/read/write/patch/search/hash/mkdir/copy/move/delete/archive |
| 二进制传输 | upload在目标用户权限下建临时文件并发布；download按目标用户权限打开源文件 |
| 操作记录 | 原请求去重、取消、进度、恢复、离线查询，保留实际身份及原始执行事实 |

文件身份一致性要求：
- 在最终选定账户下完成打开/临时文件/发布，不先 root 写文件再 chown 掩盖问题。
- 新建文件所有者符合执行账户；覆盖已有文件保留现有工具已承诺的元数据语义。
- 保留 ACL/权限边界和现有路径锁、hash 前置条件、防冲突策略；不为成功而扩大权限。
- 权限拒绝、只读目录、符号链接/路径替换和跨卷操作必须有明确失败/部分完成结果。
- 所有工具族共用解析和执行机制，不能只给命令添加 execution 而文件仍由 root/SYSTEM 操作。

### 二进制传输接入顺序（目录接入后的下一增量）

源码核对：现有异步队列、二进制分帧、临时文件、SHA-256、路径锁及提交前持久化均已存在；本轮复用。
当前 `TaskStore::get_transfer` 会以服务身份检查未确认上传的目标文件，这一逻辑必须在接入用户参数时一起分流，不能只改上传/下载的打开动作。

1. 抽出可复用的流式传输执行接口，将网络收发、父进程记录/锁协调与文件操作分开；使用已有 Tokio 和 user-worker IPC，不引入新的常驻进程或把整个文件读进内存。
2. 在接收任务的同一事务记录选择参数指纹和实际 UID/SID/会话。新能力版本阻止旧端忽略用户选择；相同 ID 只能观察原传输，同 ID 换身份/路径/内容拒绝。
3. 用户进程负责读取、临时文件、哈希及最终发布；父进程持有共享锁与数据库。发布必须等待持久化确认；网络断开或 worker 退出后先收回进程再释放锁。
4. 断点恢复使用原身份和原内容约束。原账户不可用、上下文失效或权限拒绝时明确停止，绝不恢复成 service；已发布但丢回执按原账户只读核对，不自动再写一次。
5. 接入现有 MCP 上传/下载、异步队列、进度/取消/观察和本地历史。远端身份不改变 MCP 所在机器的本地源/目标路径身份。
6. 三平台验证大于多帧的二进制文件、零字节、中文路径、哈希一致、普通用户归属、权限拒绝、共享路径冲突、断点/内容改变、提交前后断线、取消及重复 ID。正式安装后的公开参数验收仍属于 E8。

## 7. Windows/macOS 应用管理

拟新增三个入口，E1核对目录后冻结：
- `pab_list_apps`：按名称/稳定标识搜索已安装应用或运行实例，有界返回并区分两种对象。
- `pab_launch_app`：用明确应用标识或本机路径在指定桌面会话启动，返回实际实例/进程身份。
- `pab_open_file`：在指定应用或系统默认应用中打开目标机文件；首期只处理本地文件，不扩展任意 URL scheme。

激活窗口复用pab_focus_window，正常关闭窗口复用pab_window_control，显式进程终止复用
pab_terminate_process。不预先再新增activate/quit同义工具。应用级激活/退出若存在跨窗口、
多进程的确切缺口，在E0列明现有工具无法完成的场景再决定接口；不得把现有窗口关闭包装成
“整个应用已退出”。端到端测试同时检查窗口和进程实际状态，必要时通过已有原生控件退出应用。

Windows 优先官方 Shell/应用注册及进程接口；macOS 优先 NSWorkspace/LaunchServices。
应用目录按受支持来源枚举，不递归扫全磁盘，也不承诺列出所有便携应用。
应用标识、PID、窗口不是同一个东西；运行实例引用必须含进程启动身份和所属会话。
单实例应用可能复用已有进程，必须返回观察到的事实；无法归因时不能编造“新进程”。
启动接受、进程存在、窗口就绪分别报告；可组合现有窗口/控件 wait 观察实际页面，不使用固定 sleep 当成功。
退出遇到未保存内容时返回待处理状态，不自动丢弃用户数据；强制终止沿用既有显式工具与身份核对。
所有修改工具按 request_id 去重；超时/响应丢失后观察原请求，不重放 launch/open/quit。
Linux 无界面版统一返回不支持桌面能力，命令启动后台程序继续由 pab_run_command 提供。

## 8. MCP、Desktop与可观察性

- MCP 工具描述说明默认身份、如何列出/选择上下文、环境与凭据限制；复杂错误给出可执行下一步。
- Desktop 沿用 React + Ant Design，补充执行用户/会话选择、实际身份和应用操作面板。
- 中/繁/英、亮/暗主题同步；任务摘要不展示敏感环境或输入值。
- 新操作记录本地保存，Server/Web不新增用户执行记录或任务内容。
- 能力协商区分服务执行、用户执行、桌面会话、应用管理，不能只按OS名称猜测支持。
- 错误至少区分用户不存在、上下文过期、会话不可用、身份切换失败、profile/环境失败、
  cwd权限、凭据不可用、应用不存在、窗口未就绪、操作未确认和不支持的平台。
- 所有调用仍可走既有本地 Desktop HTTP/WebSocket 状态上报；不增加逐项审批或新的认证产品流程。

## 9. 开发阶段与退出门槛

| 阶段 | 工作 | 完成证据 |
|---|---|---|
| E0 基线与增量原型 | 提交已完成Finder修复；仅验证新身份/用户PTY/文件权限原型和Linux环境 | 用目标账户实际创建进程和文件；确认真实UID/SID、cwd、组与归属 |
| E1 协议与状态 | execution schema、上下文查询、能力版本、错误、指纹和历史字段 | 严格解析、旧端拒绝、跨连接/过期上下文、同ID换身份冲突测试 |
| E2 执行后端 | Windows token/profile；Mac UID与会话；Linux无界面身份隔离 | 无全局身份泄漏；队列/句柄/子进程回收；权限不足不回退 |
| E3 现有工具增强 | 向命令、终端、Git、文件/传输接入新身份，不重做原功能 | 每条入口实际身份测试，普通用户工作区不再依赖系统账户凭据 |
| E4 应用入口增量 | 发现/启动/打开文件；复用现有窗口与进程控制和异步框架 | 启动/复用/激活/打开/正常退出工作流实测，Linux明确不支持 |
| E5 最小UI与文档改动 | 增加身份字段/选择和应用入口；沿用三语主题，修正文档 | 页面实际操作；任务记录身份正确且无敏感值 |
| E6 异常回归 | 三平台测试矩阵、并发/断线/注销/取消/权限与资源 | 无未解释失败、无重复副作用、测试资源清理 |
| E7 完整包交付 | Windows、Mac ARM/Intel、Linux无界面包及安装 | 哈希/版本/服务/设备身份/Mac签名和TCC保留 |
| E8 正式宿主验收 | 新会话原生工具端到端、报告、提交push | 三平台应有能力通过，源码/文档/产物一致，宿主验收不以脚本替代 |

提交按可审查阶段组织；发生失败先确定原操作状态和原因，再修复，不能重复安装/输入。
测试不消费产品版本；使用现有 build.py 自动分配 patch，跨机器串行协调版本，不手写第二次递增。
依赖或后端选型改变时先更新本规划依据，不能悄悄缩减到单平台或只改表层UI。

## 10. 测试矩阵

| 类别 | 场景与断言 |
|---|---|
| 账户解析 | 同名本地/域账户、大小写差异、用户名Unicode、UID/SID不匹配、删除/重建；明确选择或拒绝 |
| 默认与显式身份 | service缺省确定；显式user正确；无会话/多会话/不可用token不猜测和回退 |
| 环境 | 用户home/PATH/cwd、Unicode和空格路径、环境覆盖冲突、shell启动文件边界；不泄漏服务凭据 |
| 文件权限 | 目标用户只读/可写目录、拒绝其他用户私有目录、补充组、umask、ACL、文件所有者、覆盖及跨卷 |
| Git | 用户仓库无dubious ownership；作者身份；本地bare remote端到端；目标用户SSH/agent真实认证；无代理/凭据锁定有界失败 |
| 终端 | 三平台PTY真实whoami/id、中文输入输出、resize、结束、取消；会话固定身份，关闭后无工作进程残留 |
| 应用 | 两平台启动新实例/复用实例、指定会话激活、中文路径打开文件、正常退出、未保存阻塞、应用不存在 |
| 并发 | 两MCP选择不同用户、同用户两任务、同时文件/Git操作；身份与环境不串线，锁域正确 |
| 去重与恢复 | 相同ID只执行一次、同ID换身份冲突；丢响应/断线/Executor重启保留原操作，未确认不自动重放 |
| 生命周期 | 用户注销/切换、helper退出、上下文过期、PID重用；已运行任务与未提交任务分别记录实际结果 |
| Linux headless | DISPLAY/WAYLAND_DISPLAY为空，无桌面和logind；核心工具成功，桌面工具稳定拒绝 |
| 系统差异 | Windows控制台/RDP与管理员/普通账户；Mac已登录/无GUI域和现有TCC；Linux root服务/非root服务 |
| 资源 | 队列满、重复失败、长命令、慢凭据helper、超时；进程/线程/句柄/FD数量稳定 |
| 日志隐私 | 测试假密钥标记/假环境秘密不得进入默认日志、task摘要或报告；记录操作ID和布尔验证事实 |
| 旧能力回归 | 文件哈希/异步传输、截图JPEG、窗口/控件、按键释放、任务观察、Mac Finder修复均不退化 |

测试分层：
1. 纯协议/解析与平台适配单元测试。
2. 隔离账户、目录、Git仓库和可控应用的集成测试，验证实际效果而非只验证API返回。
3. Windows本机+90、Mac ARM、Linux无界面运行测试；Intel构建签名检查与实机声明分开。
4. 安装后当前AI宿主通过正式 pixels.pab_* 复验；独立stdio只作为补充证据。

优先在自建临时账户/目录和专用测试应用中验证。创建/删除账户、改权限、注销等按测试环境授权执行；
不改用户真实账户配置、不重置TCC、不把未保存文档用于退出测试。
确需用户正在使用的桌面注销时，单独确认已保存，其他独立测试继续。
报告沿用 scripts/acceptance.py 的pass/fail/skip/blocked/unconfirmed及资源清理模型。
没有Linux核心实测不能验收E8；无Intel/多屏硬件可明确列未测，不以此阻塞已有硬件的交付。

## 11. 端到端最终验收

Windows、macOS各执行至少两轮：
1. 通过正式工具查询并选择当前普通用户执行上下文。
2. 用命令及终端核对真实账户、home/cwd，并创建归属正确的测试文件。
3. 对该用户的测试仓库完成status/修改/commit和隔离远端push/pull，核对作者、凭据来源和文件归属。
4. 启动测试编辑器 → 控件输入中文 → 保存对话框选择测试路径 → 文件工具核对内容和所有者。
5. 激活已有应用 → 正常退出；另测未保存文档的退出阻塞，不强制丢弃。
6. 关闭终端和测试应用，核对引用失效、无残留按键、无遗留子进程。

Linux无界面端各以service/可用指定用户执行至少两轮：
1. 不提供显示环境/GUI登录，启动服务并通过正式工具连接。
2. 命令、PTY、全部用户文件路径、上传下载和Git工作流验证实际身份与归属。
3. 缺少logind/用户agent时区分核心可用与具体依赖不可用；桌面工具返回不支持。
4. 取消长任务、断线重连、服务重启后观察原操作；重复ID不重复执行。
5. 清理测试目录/仓库/进程，安装升级后设备身份和服务状态正确。

交付清单：
- Finder修复先独立提交；本轮实现按阶段提交，最终SSH push核对远端commit。
- Windows x86_64完整安装包、Mac ARM/Intel完整包、Linux无界面包；沿用当前免费签名方案。
- 已安装后验收报告、各包SHA-256/实际版本、清理结果和明确未测项。
- README中英文、MACOS、Linux安装说明和tool-coverage更新。
- Desktop三语亮/暗色展示及实际身份字段验证。
- 当前AI会话因MCP更新需要重载时，先完成其余工作并明确接续；重载前不能宣称原生宿主验收完成。

## 12. 执行状态

| 项目 | 当前状态 |
|---|---|
| 本规划 | 已完成 |
| Finder修复 | 1.2.26已安装验证，已提交push：dfe7221 |
| E0用户执行原型 | Windows两个WTS用户、Mac UID 501、Linux无GUI账户均已实测，见 acceptance/execution-e0-2026-10-06.md；应用发现及启动/打开首批原型已实测，完整应用工作流仍待完成 |
| E1身份协议基础 | 实际身份观察、连接绑定注册表以及命令/终端/Git/文件/目录/传输的选择、去重及记录已接入源码；完整生命周期矩阵及正式安装验收待完成 |
| E1账户发现 | 已实现 pab_list_execution_contexts，三平台相关测试通过；当前AI宿主已加载68工具，三平台正式上下文查询及选择已使用，详见各E8报告；无需重新开发或重复等待首次工具加载 |
| E2本地通道/生命周期 | 内核核对PID的通道、内部user-worker命令路由、Windows Job/Unix进程组回收已实现；三平台原生测试通过，完整异常矩阵仍待完成 |
| E3指定用户命令 | v3参数、身份复核、原记录去重、中文输入输出、超时和取消已实现；Windows两个用户/Mac/Linux实测通过，见 acceptance/execution-e2-command-2026-10-06.md；新版已安装，正式宿主验收待完成 |
| E3指定用户终端 | 复用portable-pty接入user-worker；身份冻结、连接隔离、重复打开/关闭、中文及断线清理三平台源码集成通过，见 acceptance/execution-e3-terminal-2026-10-07.md；正式宿主与完整异常矩阵待验收 |
| E3指定用户Git | 八工具v11接入user-worker，父进程共享仓库锁与持久化，原身份push核对；三平台源码集成通过，见 acceptance/execution-e3-git-2026-10-07.md；真实凭据和完整异常矩阵待验收 |
| E3指定用户文件后端 | 复用文件引擎接入原生user-worker，二进制帧、父端路径锁和持久化确认；Windows两用户/Mac/Linux实测与文件回归通过，见 acceptance/execution-e3-filesystem-worker-2026-10-07.md；公开工具接入进展见下一行 |
| E3文件公开工具接入 | 12工具filesystem v5、原子身份记录、重连查重、原用户发布核对及异步配额/取消已接入；三平台源码集成和读写QUIC通过，见 acceptance/execution-e3-filesystem-2026-10-07.md；Service 发布恢复分流已修复并通过三平台回归，完整异常矩阵待完成 |
| E3目录列表用户接入 | filesystem v6复用文件流程，分页保留实际身份，默认service沿用原入口；三平台原生用户和QUIC验证见 acceptance/execution-e3-directory-2026-10-07.md；正式安装验收待完成 |
| E3传输用户后端 | 已抽出复用传输引擎并接入原生user-worker，二进制IPC、父端记录确认及共享路径锁、按身份区分续传临时文件；三平台普通回归及原生用户测试通过，见 acceptance/execution-e3-transfer-worker-2026-10-07.md |
| E3传输协议与Executor | transfer v2、接受事务、完整参数和身份冻结、重复请求查原记录、显式续传约束、原用户只读发布核对已完成；三平台原生QUIC及回归通过，见 acceptance/execution-e3-transfer-executor-2026-10-07.md |
| E3传输Bridge/MCP | execution/resume_from、能力检查、二进制前身份回执、队列指纹及实际身份、查询/后台恢复核对已接入；分层源码证据见 acceptance/execution-e3-transfer-bridge-2026-10-07.md；正式安装宿主验收待完成 |
| E2用户环境首批 | 用户 PATH、Mac Homebrew/系统 paths、选中用户 SSH agent 查询及身份环境覆盖校验已实现；Mac/Linux 原生切换实测通过，见 acceptance/execution-e2-environment-2026-10-07.md；真实认证等仍待验收 |
| E2终端启动信息 | shell实际参数/启动模式贯通Executor、Bridge/MCP及Desktop；三平台真实用户PTY参数、身份、中文及清理通过，见 acceptance/execution-e2-terminal-startup-2026-10-07.md；安装后回执/UI及异常启动脚本矩阵仍待完成 |
| E4应用发现后端 | Windows AppsFolder/可见应用进程、Mac 标准应用目录/NSWorkspace、搜索与有界响应已实测，Linux明确不支持；见 acceptance/execution-e4-app-discovery-2026-10-07.md。启动/打开、身份路由和正式工具的进度见后续三行 |
| E4启动/打开后端 | Windows Shell、Mac NSWorkspace 的 ID/路径启动及指定/默认应用打开文件已实现并实测首批流程，见 acceptance/execution-e4-app-actions-2026-10-07.md；桌面身份路由/记录/正式工具已继续接入，见后续两行。Mac锁屏下保留一份测试文档待解锁后清理 |
| E4应用身份路由/记录 | system-query v12、内核核验helper进程身份、连接绑定desktop_user上下文、接受前冻结通道/身份及原记录去重已接入；双桌面编译/IPC与Linux拒绝边界通过，见 acceptance/execution-e4-app-routing-2026-10-07.md；正式入口见下一行；完整生命周期仍待完成 |
| E4应用MCP/Bridge入口 | 三个正式工具、严格解析/v12协商、原身份结果验证/历史去重及活动操作计数已接入；三平台各67项Bridge、38项MCP、7项stdio测试通过，见 acceptance/execution-e4-app-mcp-2026-10-07.md；安装宿主验收仍待完成 |
| E4 Windows应用helper | 各活动WTS用户原始令牌、旧桌面路由隔离和原生调用阻塞退出已实现；原回归见 acceptance/execution-e4-application-helper-2026-10-07.md。新增真实独立进程20秒退出/子进程存活测试及双桌面原记录重开不重放检查通过，见 acceptance/execution-e6-application-deadline-2026-10-07.md；安装后多会话与完整故障注入仍待验收 |
| E5最小UI源码 | 命令/目录/终端/传输用户选择、双桌面应用入口及实际身份历史已接入；传输复用MCP队列，原连接取消/观察与未确认结果保留。三平台各69项Bridge和38项MCP测试、11项浏览器测试通过，见 acceptance/execution-e5-ui-2026-10-07.md；已安装桌面实际操作待验收 |
| E6真实SSH | Mac/Linux原生用户worker + 独立OpenSSH agent通过；Windows已补齐Git for Windows OpenSSH真实push/fetch、错误主机密钥与缺失agent拒绝、恢复和原身份核对。见 acceptance/execution-e6-ssh-2026-10-07.md 及 acceptance/execution-e6-windows-ssh-2026-10-07.md；Keychain与慢凭据helper见后续两行 |
| E6 Mac Keychain | 正式用户命令调用Security API及真实user-worker Git fetch均验证独立Keychain的可读、锁定拒绝和恢复；实际身份、20秒边界、结果隐私及用户默认Keychain/搜索列表不变通过。见 acceptance/execution-e6-keychain-2026-10-07.md；不承诺自动解锁登录Keychain或任意第三方helper兼容 |
| E6慢Git凭据程序 | Windows实测发现取消后宽限退出让helper继续写入，已修复为收到最终响应后立即回收进程树并保留仓库锁；Windows与Linux两用户通过，见 acceptance/execution-e6-slow-credential-2026-10-07.md；Mac退出竞态已补修并通过5轮正常工作流/慢凭据/12项回归，见 acceptance/execution-e6-macos-exit-race-2026-10-07.md；修复已纳入Windows/Linux 1.2.29及Mac 1.2.30安装包 |
| E2–E8剩余增量 | 主体实现及四类产物已完成；剩余是环境/凭据边界、完整异常矩阵、已安装UI与正式宿主验收，不重复开发现有能力 |
| 已有工具、桌面能力、打包和记录框架 | 复用，仅做受影响范围的回归 |
| Linux无界面交付入口 | 已移除Linux包的Desktop/前端依赖，Executor+MCP在精简镜像编译、手动安装/强制回收与真实systemd安装生命周期通过，见 acceptance/execution-linux-headless-package-2026-10-07.md；正式版本包及安装已完成；真实设备身份/已完成任务的重启与同版本重装保留通过，见 acceptance/execution-e7-linux-persistence-2026-10-07.md；指定用户正式宿主验收仍待完成 |
| E7首批产物与安装 | Windows/Linux 1.2.27、Mac ARM/Intel 1.2.28完整产物已生成；本机/90/Mac ARM/Linux测试端已安装，哈希/身份/服务通过，远端原生连接首轮通过，见 acceptance/execution-e7-packages-2026-10-07.md；当前AI会话已恢复并加载68工具，完整E8仍待完成 |
| E7 Linux持久化 | 实际注册设备的服务重启、容器重启和1.2.27同版本重装通过，设备身份/凭据/原任务/事件/输出保留，服务器重新认证；见 acceptance/execution-e7-linux-persistence-2026-10-07.md；运行中恢复及跨版本升级不在本证据范围 |
| E8 Windows应用首轮 | 原生MCP已完成解锁、应用发现、用户文件创建、记事本打开/中文控件编辑/保存提示/正常退出/内容归属核对及清理；见 acceptance/execution-e8-windows-app-first-2026-10-07.md；focus被系统策略拒绝，未冒充成功，完整两轮仍待完成 |
| E8 Linux正式工具首轮 | pabuser1 的实际身份、中文文件、八种Git操作及上传下载哈希/所有者通过；终端关闭暴露NotFound，不能记为完整一轮。见 acceptance/execution-e8-linux-first-2026-10-07.md；测试树保留待升级核对及最终清理 |
| E8终端关闭修复 | 已修复过早删除会话、用户工作进程退出丢失输出、Bridge关闭与轮询竞争及20次读取上限；三平台QUIC、真实用户PTY、700003字节归档/并发和断线状态回归通过，见 acceptance/execution-e8-terminal-close-2026-10-07.md；安装后旧MCP连接新版三平台Executor的指定用户终端关闭/中文归档通过。新MCP的大输出并发复验待会话重载 |
| E7修复版本 | Windows/Linux 1.2.29、Mac ARM/Intel 1.2.30完整包已生成；本机/90/Mac ARM/Linux均升级并核对身份/文件哈希，Mac固定证书保留；Linux旧任务/事件/输出/用户文件跨版本保留通过。见 acceptance/execution-e7-refresh-2026-10-07.md；Intel仍无实机，新MCP正式验收待重载 |
| 下一动作 | 本机升级已结束旧MCP，重载会话后继续正式工具两轮工作流及大输出并发归档；Windows helper完整生命周期、E6剩余矩阵及Mac解锁后UI验收仍待完成。已有能力不重新实现；源码测试、首轮局部通过、安装后完整通过分别记录 |
