# TOML 配置与 Linux 一行安装验收

范围：项目自己定义的配置文件改为 TOML；补齐 Linux 一行安装及其发布链路源码。第三方客户端、构建工具和系统强制要求的格式不改；凭据、数据库和发布清单保留原数据格式。配置清单见 `docs/CONFIGURATION.md`。

## 已通过

- Windows：Executor、Bridge/MCP 的 `cargo check`，Desktop 独立 manifest 的 `cargo check`。
- `pab-service-config`：8 项单测，覆盖 TOML 参数解析、错误脱敏、JSON BOM/env 迁移、注释保留、显式缺失配置拒绝、用户数据与机器身份隔离、原子迁移及 TOML 优先级。
- `pab-bridge --lib mcp_tool_settings`：3 项单测，覆盖工具分组保存/读取、旧版本选择迁移、未知字段/重复分组/大小限制及无效保存不破坏旧配置。
- Linux：最终源码构建 Executor、MCP、Document Worker 成功。使用 Docker CPU 配额 10，最终一轮 Cargo `-j 1`、CMake 最多 8 个任务；编译未与其他编译并发。
- Linux 隔离安装：真实 Debug 二进制、离线文档运行库、无网络容器。首次安装、无参数重装、注释及自部署地址保留、CLI 参数传递、无 GUI 动态依赖、非法 TOML 在停止旧程序前失败、停止指定安装路径内的进程、保留外部同名程序、顽固进程退出、卸载后保留设备数据及 TOML 均通过。
- Linux 引导脚本：4 项测试，使用真实临时 Ed25519 签名验证，覆盖清单篡改、错误大小/摘要、危险归档条目及只有验证成功后才调用安装器。
- Linux 打包：Debug/Release 归档允许列表、脚本 LF/执行权限、构建来源篡改拒绝、带版本号 Release 文件名及包内发布清单一致性通过。
- Windows 安装脚本 PowerShell 语法检查、Linux/macOS 安装脚本 Bash 语法检查、TOML 示例解析及官网引导脚本副本一致性通过。

- MCP 真实子进程回归：2 项通过，验证无效 TOML 阻止启动、新进程采用新工具设置、旧进程保持原有工具列表、禁用工具不会初始化远端运行时。
- 官网 HTTP：7 项通过，其中包含 Linux x86_64 stable 版本预留及未发布目标查询，继续拒绝尚未支持的 Linux aarch64。
- 官网 `npm --prefix web run docs:sync`、`docs:check` 通过，TypeScript 与 Vite 构建成功；中英文页面与主仓库 `docs/USAGE_AND_SELF_HOSTING.md` 一致。

## 验证边界

没有更新本机已安装 Pixels，没有修改其他现有 Linux 实例。没有构建、上传或发布新的正式 Release，也没有部署官网。正式发版前仍需用最终签名归档验证目标发行版及真实 systemd 环境；本轮未重新执行 Windows/macOS 的真实安装或 macOS 原生编译。
