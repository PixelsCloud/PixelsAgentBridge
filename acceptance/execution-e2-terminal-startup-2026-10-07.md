# E2 终端启动方式与真实参数验证

日期：2026-10-07。源码版本 1.2.26；本轮未安装新产品包，未递增正式版本。

## 已实现

`TerminalOpened` 增加可选 `startup`：实际启动参数 `arguments` 和模式 `mode`。
Executor 启动终端后保留这一会话的 shell/参数，在重复请求中返回同一份信息。
指定用户 worker 的启动回执必须匹配预期 shell 及完整参数/模式，否则拒绝确认。
Bridge、MCP `pab_open_terminal`、Tauri 返回该字段，Desktop 显示 shell 和参数。
旧端缺少字段时保留 None/null；不根据操作系统猜测启动方式。

| 平台 | 实际启动 | mode |
|---|---|---|
| Windows | powershell.exe -NoLogo -NoProfile | interactive_no_profile |
| macOS | /bin/zsh -l -i | interactive_login |
| Linux headless | /bin/sh -i | interactive |

本轮没有修改原有 shell 选择；明确报告 Mac 的登录式交互启动及其配置加载行为。
命令工具原有的无隐式 shell 语义保持。

## 验证证据

- 三平台协议测试 `terminal_startup_roundtrip_and_older_peer_is_unknown` 通过：
  新结构序列化/反序列化及旧端未知字段状态。
- 扩展原有 `native_user_terminal_acceptance`：在真实 PTY 中通过 Windows CIM 或 Unix `ps`
  查询 shell 进程命令行，要求实际参数出现，再继续原身份、中文、resize、重复打开/关闭、
  跨连接拒绝、持久化身份和断线回收断言。查询命令本身不包含被验证的启动参数。
- Windows Server 2022（211399447）WTS 1、2 两个账户分别通过，原生 MCP 任务
  `1df62665-201f-43d4-841c-5e44ec2677f7`，exit 0，两项测试通过。
  使用上传到专用目录的源码 worker/test exe，不替换安装程序。
- macOS 27.0.1 ARM64（603527578）UID 501 通过，任务
  `d78d3cfe-c09a-4d60-8a03-b7ad3547b810`，exit 0；同任务包含源码编译、协议测试及 Desktop cargo check。
- Debian bookworm 无 GUI 容器 UID 23001 通过；真实 `/bin/sh -i`、原生 UID worker。
  源码 Executor build、用户终端集成和协议测试均 exit 0。
- Windows Executor/Bridge tests 编译检查、Windows/macOS Desktop tests 编译检查通过。
- TypeScript + Vite 构建通过（保留既有大 chunk 提示）；11 项浏览器回归通过。
  浏览器测试使用真实 React 与 Tauri 夹具，不能代表已安装 UI。

## 清理及限制

Windows 专用目录 `C:\ProgramData\PAB-terminal-startup-20261007` 在核对绝对路径、非 reparse、
两个二进制 SHA-256、没有目录内运行进程后删除；清理任务
`ff2b0a0d-bc18-43d4-9b22-91974d32b139` exit 0。
Mac 源码 worker 剩余 0，检查任务 `08ae0dfc-112d-4355-9eeb-8fb0b050fb31` exit 0。
上传源码恢复为 UID 501/GID 20；未改 shell 配置、TCC、真实用户凭据。Linux 测试容器自动删除。

尚未验证：正式新版 MCP/Executor 安装后的 `pab_open_terminal` 完整返回、已安装 Desktop 展示、
慢/异常用户启动脚本的完整矩阵。这些继续纳入 E6/E8，不以本次源码账户验证替代。
