# E4 原生应用启动与打开文件

日期：2026-10-07。基线 ce3b5d5，源码版本 1.2.26。本阶段为原生后端及源代码探针，尚未接入公开 MCP、持久化去重或 UI，也未打包安装。

## 实现

- 新增严格的 AppTarget（系统 ID/绝对应用路径）和 launch/open_file 请求。open_file 只接受目标机存在的绝对文件路径，URL、NUL、未知参数均不混入原始命令行。
- Windows 复用 ShellExecuteEx：应用 ID 先在当前用户 AppsFolder 精确解析后通过 PIDL 启动；直接应用路径要求 exe。文件可交给系统默认应用，或使用应用路径/目录 ID。桌面应用 ID 从系统 Link.TargetParsingPath 属性解析；无 exe 的注册应用走系统 ActivateForFile 文件契约，不能把任意 ID 当命令运行。
- Mac 复用 NSWorkspace：按 bundle ID 或 app 路径启动/复用；按指定应用或系统默认应用打开本地文件。使用系统完成回调，等待上限 15 秒；超时表示 unconfirmed，不重新调用启动 API。
- 返回系统接受事实、可核实的进程启动身份、真实执行用户/会话。Mac 只有与调用前观察到的同一 PID/启动身份一致时才报告 reused_instance=true。无证据时返回 null，不猜测新旧进程。
- window_ready 保持 null，调用方复用已有窗口/控件查询确认界面。预检拒绝 action_started=false；进入系统调用后的错误 action_started=true，由后续持久化层保留结果不确定的事实。错误文本按 UTF-8 边界限制长度。

官方接口依据：[Microsoft Shell 启动](https://learn.microsoft.com/en-us/windows/win32/shell/launch)、[IApplicationActivationManager](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-iapplicationactivationmanager)、[Apple NSWorkspace](https://developer.apple.com/documentation/AppKit/NSWorkspace)。沿用已锁定的 Windows/objc2 绑定，未引入另一套应用管理库。

## 实测

Windows 本机 chess / WTS 2：

- 按 Notepad 系统目录 ID 启动新窗口，系统返回 PID 74440，与随后观察到的新 Untitled - Notepad 窗口一致。
- 中文带空格文件分别通过 exe 路径、Notepad 系统 ID、系统默认应用打开；返回 PID 与新窗口 PID 一致，窗口标题包含完整唯一文件名。
- 每轮只对本轮新建且唯一匹配的窗口发送正常关闭，确认窗口消失；文件内容未改变，测试文件与目录已清理。
- 不存在应用 ID、https URL 作为文件路径均在副作用前拒绝。

Mac 603527578 / UID 501 / 图形 session 100002：

- Finder 的 bundle ID 与 app 路径都复用原 PID 601，reused_instance=true，进程启动身份相同。
- TextEdit 按 bundle ID、app 路径与系统默认应用均打开同一份中文文件，返回原 PID 575 与实际用户身份，reused_instance=true。
- 原生 pab_list_windows 观察到匹配测试文件名的 TextEdit 窗口；这证明文档窗口存在，**不证明当前桌面可交互**。
- 无效 bundle ID 和 URL 文件路径在副作用前拒绝。
- Mac 协议 2 项、应用发现边界回归通过；Windows workspace --tests check 通过；Linux 无 GUI 容器应用拒绝/边界 2 项、协议 2 项通过。

Mac 任务：`004eab22-cbe2-49ac-bf5c-f8a3c2343ae5`（Finder/指定应用），`6967905f-319f-4ce7-95c1-b8baef1ce61b`（默认应用/路径），`44bd0346-a298-4e01-ae53-eb4bc80f98df`（预检拒绝），`55450005-741b-41b7-b030-50ccd4133627`（测试），均 exit 0。

## 锁屏观察与待清理

Mac 当前 `IOConsoleUsers` 明确报告 `CGSSessionScreenIsLocked=Yes`，而 SessionGetInfo 仍能识别登录用户图形 session。原生 app 请求可被该用户的 NSWorkspace 接受；现有安装版控制连接指向登录界面，普通用户窗口没有有效控制引用。

因此没有强杀原先已存在的 TextEdit，未声称窗口关闭验收通过。待解锁后，只关闭本轮文档，再核对文件内容并删除：

`/Users/huayang/PAB-应用打开-079111b06d9548e7800daa2e7ea10512.txt`

内容应为 `Pixels app open verification\n`，文件属于 UID 501。该文件与文档仍保留；这是本阶段明确未完成的清理项。

## 后续必做

1. 在已有上下文注册表与桌面 helper 路由中绑定真实账号/登录代次；区分登录图形会话与当前可交互桌面，锁屏时不得换到 SYSTEM/root 后执行用户应用操作。
2. 应用操作先持久化接受和完整指纹，再调用此后端；重复请求、响应丢失、helper 退出都只观察原请求。Windows 原生 Shell/COM 调用的硬超时必须由隔离执行生命周期处理，不能把丢弃线程当成取消。
3. 接入正式 MCP 三入口、能力版本和 UI；保留窗口聚焦/关闭与进程终止的既有接口。
4. 补 Windows 90、商店应用文件契约、Mac 新进程启动、未保存退出阻塞、多窗口/多用户、锁屏切换、E7/E8 安装及原生宿主验收。此报告不把未测分支算作完成。
