# E4 应用身份路由与持久化首批验收

日期：2026-10-07。基于 `2fb274e` 继续实现。此报告只覆盖源码、原生后端及协议集成；未重新打包安装，不是正式宿主 E8 验收。

## 实现

- `SystemQuery::Applications` 使用 system-query v12，包含列表、启动、打开文件；必须显式选择当前连接发现的 `desktop_user` 引用。Service/User 模式不能代替桌面用户。
- helper 注册复用现有内核核对 PID 的 worker 通道进行身份证明。Windows 读取进程 token SID/WTS session/logon ID；Mac 读取 audit token UID/GID/audit session；注册消息里的用户名或会话声明不作为身份依据。证明前后检查进程创建标记，排除 PID 重用。
- 只有声明应用能力、身份核验成功的非系统交互 helper 会进入桌面执行上下文列表。旧 helper、SYSTEM/root 登录界面 helper 保持原有窗口能力，不成为应用执行账户。Linux 不添加 GUI 上下文。
- 应用操作在原子接受前冻结身份和具体 helper 通道。原请求 ID 优先读取持久化记录，不依赖当前引用或在线 helper；相同 ID 修改参数冲突。helper 离线后不自动换投新 helper。
- 未派发失败可明确失败；已派发后丢失回复/超时保留 unconfirmed，不自动重放。返回数据的类型和实际执行身份必须与请求一致。每次动作前后核对活动桌面和原生身份。
- Desktop 主程序与会话 helper 已接入处理；桌面独立 Cargo.lock 同步前两次应用后端提交的依赖，未升级第三方版本。
- Windows 原生运行中应用列表和启动实例补充 token SID，只报告所选账户及会话内可验证的进程。API 接受不等于窗口就绪或新建进程，`window_ready` 仍为空，需复用窗口工具观察。

## 验证证据

| 环境 | 检查 | 结果 |
|---|---|---|
| Windows 本机 | workspace `cargo check --locked --workspace --tests`；Desktop 独立 manifest check | 通过 |
| Windows 本机 | 原有及新增 local IPC 回归 20 项，新增结果账户/类型校验后再跑 application_ 3 项 | 全通过 |
| Windows 本机 | app_management 协议 3 项、apps::tests 2 项 | 全通过 |
| Windows 本机 | 原生运行中应用查询 | 29 个实例，每个 SID 都与所选账户一致 |
| Windows 本机 | AppsFolder Notepad ID 打开中文/空格路径文件 | 实际观察唯一新窗口 PID 82048，返回 SID 与 WTS session 2 一致；正常关闭该测试窗口，核对文件内容后清理文件/目录 |
| macOS ARM | workspace check；新增路由两项首轮 | 任务 `24679ea2-ec1e-4aea-a67d-d4ac80fc70b7`，exit 0 |
| macOS ARM | Desktop 独立 locked check；local IPC 21 项；协议 3 项；apps 2 项 | 任务 `52501d10-aeca-476c-b0d9-d33b05af0e90`，exit 0；通过 launchctl asuser 501 + sudo -u huayang 执行，实际原生身份握手 |
| Linux Debian 无 GUI 容器 | Executor tests 编译；协议 3 项；应用拒绝边界 1 项；local IPC 18 项 | 全通过；容器 --rm 已结束 |
| Linux Debian 无 GUI 容器 | apps::tests 3 项 | 全通过，包括无 GUI 产品明确不支持、活动会话/身份检查及响应边界 |

新增路由测试验证：真实进程身份握手和上下文发现、不同用户/会话不能选中、不同 MCP 连接不能使用对方引用、原身份落库、重复运行中请求不重派、离线/新服务对象读取原结果、同 ID 修改参数冲突、派发后断线 unconfirmed、已冻结旧通道不改投替代 helper、错误账户及错误数据类型不能成为成功结果。

协议集成中的应用回复由测试 helper 提供，没有启动真实应用；不能把这些测试当作完整 GUI 工作流。Windows 原生动作另外验证了实际窗口和文件清理。原有 Windows IPC 回归与新增第三项分两次运行，Mac 最终 21 项一起运行。

## 仍需完成

- 三个正式 MCP 入口、工具发现与能力检查、Bridge 历史集成验证、最小 UI。
- Windows 后台应用 helper 供给仍待补足。更正：现有 Default helper 已以登录用户运行，管理员使用 linked elevated token 以控制高权限窗口；只有 Winlogon helper 使用 SYSTEM。应用启动应使用原始 WTS 用户 token，并与窗口控制通道分离。
- 阻塞原生调用的生命周期边界；当前有父端等待预算和 unconfirmed，不宣称可以硬中断 Shell/NSWorkspace 调用，也不把超时当作取消成功。
- Windows 90 多用户及两 MCP 场景、真实 helper 派发到原生后端的端到端操作、Mac 锁屏/注销工作流、未保存文档的正常关闭阻塞。
- 前一报告记录的 Mac TextEdit 文档仍待解锁后单独关闭和清理；本轮未操纵登录界面、未发送键鼠、未重置 TCC。
- 完整 E6 异常矩阵、E7 三平台包与安装、E8 正式工具宿主验收。当前安装包版本和运行进程未更新。
