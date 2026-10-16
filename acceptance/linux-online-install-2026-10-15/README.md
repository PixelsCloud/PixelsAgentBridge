# Linux 一行安装发布验收

日期：2026-10-15。Linux x86_64 stable **1.0.1** 已通过官网发布服务签名上传 COS，并激活为当前版本。官网 `/install.sh` 和 Linux 版本查询接口已部署。

```bash
curl -fsSL https://agent.rgaa.vip/install.sh | bash
```

安装并联网注册后，查看设备码和连接密码：

```bash
sudo /opt/pixels-agent-bridge/run-executor.sh show-access
```

无 systemd 的环境使用 `bash -s -- --no-service`，再由外部进程管理器运行 `/opt/pixels-agent-bridge/run-executor.sh`。Linux 被控端无需登录 Pixels 账号；操作端使用设备码及密码连接。

## 安装包与发布

- 文件：`pixels-agent-bridge-linux-x86_64-release-1.0.1.tar.gz`。
- 大小：47,784,921 字节。
- SHA-256：`6150cabd5fea811cb86ac884ce3f76d6382acfa0ce1f0a74cf99bada2da9c07c`。
- 包内程序从独立干净工作区 `00dd3d88da94a6542b5130ce815574553fff652d` 构建；安装器版本独立于 Rust 组件版本。
- 发布经过版本预留、Ed25519 签名、COS 上传、官网 finalize 校验及公开查询验证，没有直接绕过版本数据库写文件。
- 官网程序部署保留原有 TOML、COS 配置及 SQLite 数据卷，部署前另存数据库一致备份。Windows/macOS 当前发布记录保持不变。
- 官网引导脚本另修复 `/tmp` 挂载 `noexec` 的兼容问题；使用当前用户主目录下的 0700 临时目录，退出时清理。此脚本不在安装包内，因此不改变上述归档摘要。

## 已通过

| 范围 | 验证结果 |
| --- | --- |
| 引导及打包 | 4 项引导测试及 1 项打包测试通过；发布公钥匹配；最终归档允许列表、版本、ELF 和动态依赖预检通过。 |
| 最终包安装 | 离线隔离容器验证安装、重装、CLI、非法配置在停止旧服务前拒绝、进程停止范围、顽固进程处理、卸载保留数据。 |
| systemd | 最终包实际 enable/start/stop、重装、切换手工模式、重新安装和卸载通过；服务由真实 systemd PID 1 管理。 |
| Debian 12 | 新容器直接执行官网命令，经 COS 下载、签名/大小/摘要验证后安装并注册；保留 `/tmp` 的 `noexec` 限制。 |
| Ubuntu 24.04 | 新容器执行官网命令并加 `--no-service`；启动 Executor 后正常注册。 |
| 连接凭据 | 两台环境均用安装后的 `show-access` 读取九位设备码和密码，凭此完成真实 Windows MCP → Linux 连接及目标环境确认。密码未写入公开报告。 |
| 远程使用 | 两台环境均通过命令执行、中文文件写入/读取、交互终端输出、系统日志来源查询、离线 PDF 解析和通过端口转发读取 HTTP 内容。 |
| Linux MCP | 安装后的 MCP 进程正常启动并提供 110 个工具，包含系统日志工具。 |
| 重装保留 | 两台环境重复在线安装后，设备码、密码、端点密钥及 TOML（含用户注释）保持不变。 |
| 启动恢复 | Debian 容器重启后 systemd 自动启动 Executor，随后远程使用验证通过。 |
| 公开入口 | 安装脚本与源码一致；COS 对象存在且大小匹配；Linux 旧版本查询返回 200，当前 1.0.1 查询返回 204。 |

## 清理与范围

临时 Linux 容器已删除，临时本机连接密码及本地设备条目已清理。连接验证会由服务端自动保存账号设备条目；这两个测试条目已在后续 Ubuntu 20.04 兼容验收中按其确切设备 ID 从账号目录删除，并核对删除状态。测试用设备密码文件已删除。本机 Windows Pixels 及原有设备未升级。

本轮是 Debian/Ubuntu 容器中的完整安装与连接使用验收，包含 systemd 启动恢复；没有重启物理 Linux 主机，也不替代各工具全部功能的专项验收。默认 systemd 服务以 root 运行。当前包支持 x86_64、glibc 2.36 及以上，不含 Linux 图形桌面程序。

去敏回执见 [results.json](results.json)。原始私有测试脚本和日志位于被 Git 忽略的 `.build/linux-online-20261015/`。
