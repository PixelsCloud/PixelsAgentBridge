# Pixels Agent Bridge

**让 AI Agent 通过网络操作你的设备。**

[English](README.md) · 简体中文

Pixels Agent Bridge 通过 MCP 将 AI Agent 与你的计算机连接起来。用自然语言让 Agent 跨设备执行命令、管理文件、查询系统信息和操作桌面应用。

## 能做什么

- **命令与终端**：运行程序、使用交互式终端、查看任务输出。
- **文件操作**：上传、下载、搜索、编辑、复制和压缩文件，支持后台传输和进度查询。
- **桌面自动化**：在 Windows、macOS 上获取 JPEG 截图、查询窗口与 UI 控件、发送键鼠输入。
- **系统与开发工具**：查询系统资源、进程和网络，执行平台支持的服务管理、Git 和 Docker 操作。
- **桌面管理**：管理设备、查看本地任务记录和各 Agent 的 MCP 连接。支持简体中文、繁体中文、英文及亮暗主题。
- **账号与 Web 后台**：注册登录、查看设备和在线状态、管理用户、设置用户的 Relay 带宽。任务记录只保存在本地，不上传服务端。

已提供 **Codex、Kimi Code、Claude Code、DeepSeek Harness、OpenCode** 的一键接入。其他客户端可手动配置 stdio MCP。

## 支持平台

| 平台 | 提供的组件 |
|---|---|
| Windows | 桌面应用、后台设备服务、MCP |
| macOS | 桌面应用、后台设备服务、MCP |
| Linux | 无界面设备服务和 MCP，暂无桌面应用 |

具体能力取决于目标系统和权限。macOS 截图、键鼠控制需要相应的系统授权。项目仍在开发中，已测试场景和待验范围见[验收记录](acceptance/README.md)。

## 快速开始

1. **安装应用**：在操作端和目标设备安装对应平台的包，并使用同一个控制服务。参见[安装与打包说明](packaging/desktop/README.md)或 [Linux 无界面安装](packaging/desktop/unix/INSTALL-LINUX.txt)。
2. **保存设备连接**：在目标设备的桌面应用中查看九位设备码和密码，在操作端输入并成功连接一次。之后 MCP 会使用保存的凭据。
3. **接入 Agent**：打开「设置 → AI Agent」，启用需要的客户端，再重启客户端或新建会话。运行中的会话会显示在「MCP 连接」页。
4. **直接交代任务**：

   > 用 Pixels 连接设备 123456789，检查它的操作系统和磁盘剩余空间，并总结结果。

使用设备密码连接不强制登录账号。需要使用账号身份时，可在 Desktop 或 Web 后台注册、登录；登录后保持登录状态，主动退出才注销。已经运行的 MCP 会自动同步账号变化。用户登录不能代替目标设备密码。

启用 GitHub 登录的服务可直接点击**「使用 GitHub 登录」**，无需另填注册表或设置密码，首次授权会自动创建普通账号。已有账号可以在「我的」中绑定 GitHub。自部署请配置自己的 [GitHub App](GITHUB_LOGIN_SETUP.md)。

## 工作方式

![Pixels Agent Bridge 工作流程](diagram/workflow/agent-workflow.svg)

[打开动态流程图](diagram/workflow/agent-workflow.svg)

Agent 调用本地 MCP，由 MCP 连接目标设备的 Executor。控制服务负责设备发现和认证，操作数据优先通过 P2P 传输，无法直连时使用 Relay 中继。

每个 MCP 独立维护连接，Desktop 汇总活动状态并展示本地任务记录。多个 Agent 可以操作同一设备，但同时修改同一文件或操作同一桌面时仍需协调。

## 构建与自部署

桌面端使用 **Rust + Tauri + React + Ant Design**；Web 后台使用 **React + Ant Design**，服务端由 Rust、PostgreSQL 和 Relay 组成。

准备好[构建环境](BUILDING.md)后，在仓库根目录生成 Windows Release 安装包：

```powershell
npm --prefix apps/desktop ci
python scripts/build.py desktop --profile release --package
```

产物位于 `.build/packages/`。安装包版本自动递增，内部组件版本独立维护，构建会复用缓存。

自部署请按 [Web 部署指南](WEB_DEPLOYMENT.md)和 [Docker Compose 说明](packaging/docker/README.md)配置。Server、Relay 和客户端需要使用兼容版本；开发阶段清库重建后，设备需要重新注册。

## 更多文档

- [构建与版本规则](BUILDING.md)
- [开发说明](DEVELOPMENT.md)
- [macOS 安装与权限](MACOS.md)
- [账号功能与验收](acceptance/user-accounts-2026-10-08.md)
- [AI Agent 接入](acceptance/agent-integrations-2026-10-08.md)
- [测试范围与平台限制](acceptance/README.md)

遇到问题可[提交 Issue](https://github.com/PixelsCloud/PixelsAgentBridge/issues)，附上系统、复现步骤和去除凭据后的日志。
