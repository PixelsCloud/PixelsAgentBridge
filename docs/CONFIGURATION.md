# 程序配置格式

2026-10-15：项目自己定义的程序配置统一使用 TOML，适用于 Windows、macOS、Linux。第三方或操作系统强制要求的格式保持原样。这条要求也记录在根目录 `AGENTS.md`。

## 配置清单

| 用途 | 当前配置 | 旧格式迁移 |
| --- | --- | --- |
| Windows 安装默认值、Executor、Desktop/MCP 连接默认值 | 安装目录 `config.toml`，通常为 `C:\Program Files\PixelsAgentBridge\config.toml` | 安装器读取旧 `settings.json`，包括 PowerShell UTF-8 BOM |
| macOS 安装默认值 | `/Library/Application Support/PixelsAgentBridge/config.toml` | 安装器读取旧 `settings.env`；原系统级 `operator-server.json` 的默认值由共同安装配置替代 |
| Linux 安装默认值 | `/etc/pixels-agent-bridge/config.toml` | 安装器读取 `/opt/pixels-agent-bridge/settings.env`，成功后将旧文件保留为 `.migrated` 备份 |
| Desktop 用户服务器设置，MCP 共享 | 用户数据目录 `operator-server.toml` | 第一次使用时读取旧 `operator-server.json`，校验后原子写入 TOML；旧文件保留备份，不再参与读取 |
| MCP 工具分组开关 | 用户数据目录 `mcp-tools.toml` | 第一次使用时迁移 `mcp-tools.json`，保留已有选择和版本升级规则 |
| Server | 原有 `pab-server.toml` | 已使用 TOML |
| Relay | 原有 `pab-relay-server.toml` | 已使用 TOML |
| 官网服务 | 原有 `site.toml` | 已使用 TOML |

用户数据目录：Windows `%LOCALAPPDATA%\PixelsAgentBridge`；macOS `~/Library/Application Support/PixelsAgentBridge`；Linux `$XDG_DATA_HOME/pixels-agent-bridge` 或 `~/.local/share/pixels-agent-bridge`。安装配置的 `[operator].data_dir` 可覆盖该位置。

## 读写与升级规则

共同读写实现位于 `crates/service-config`，完整配置示例位于 `packaging/desktop/config.example.toml`。Executor、Desktop、MCP 和开发用 Bridge/Enroll CLI 在启动线程前加载 TOML，现有内部组件通过启动时环境映射获得参数。

未传 `--config` 时读取所在平台安装配置；Desktop/MCP 随后应用用户 `operator-server.toml` 的服务器地址。显式 `--config PATH` 使用指定文件，并跳过用户服务器地址覆盖。显式文件缺失、TOML 语法错误、未知字段和字段类型错误都会报错，不会回退读取旧配置。没有安装配置的开发环境仍可使用进程环境变量。

`[executor]` 和 `[operator]` 分开设置数据与日志目录，避免 MCP 意外使用机器服务的设备身份和数据库。相对路径以 TOML 文件所在目录为基准。设备密码、账号登录态和端点私钥继续由原有凭据存储管理。

安装器通过新包的 `pab-executor configure` 创建或迁移配置，在停止运行服务前完成校验。已有 TOML 会完整保留，包括注释；安装命令的服务地址参数只设置首次安装默认值。后续修改请编辑 TOML，随后重启相应服务与 Desktop/MCP。Linux/macOS 的启动脚本不再 `source` 配置文件。

旧 env 仅按受限的字面量赋值解析，不执行 shell。遇到无法无损解析的表达式、未知参数或非法配置时停止升级，用户需手动转换 TOML。用户偏好通过临时文件原子替换，避免另一 MCP 进程读到写了一半的文件。

## 保持原格式的内容

- Claude、Kimi、OpenCode、Gemini、VS Code 等第三方客户端要求的 JSON/JSONC/YAML；Codex 自身要求的 TOML。
- Tauri 的配置和 capabilities、npm/TypeScript/Playwright/Vite 的构建配置、Docker Compose、launchd plist、systemd unit、SSH 配置、COS CLI 配置等第三方或系统格式。
- 设备身份及登录凭据、数据库、心跳、集成回执、浏览器进程通信、验收结果、构建版本记录、发布目录与签名清单等运行数据。

这些文件不会因为扩展名是 JSON/YAML 而统一改名；只有项目拥有格式定义权的程序配置进入 TOML 改造。
