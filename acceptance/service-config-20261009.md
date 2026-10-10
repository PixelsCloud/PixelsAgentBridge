# Server / Relay 配置收敛验收

日期：2026-10-09。

本文保留当时的配置与测速结果。2026-10-10 的 1.2.74 已取消游客控制与启动带宽配置，
账号默认带宽改为 10 Mbps；当前规则及部署结果见[登录控制验收](login-required-control.md)。

## 完成范围

- `pab-server` 读取 `pab-server.toml`；`pab-relay-server` 读取 `pab-relay-server.toml`。支持 `--config <path>`，默认从工作目录读取，文件内相对路径以 TOML 所在目录为基准。
- Server 集中管理数据库、TLS、Web、注册开关、GitHub 凭据/代理、Relay 默认限速和控制密钥、日志。
- Relay 集中管理节点标识、统计队列路径、TLS/监听地址、Server 控制连接和日志。节点标识贯穿策略同步、重连与统计上报。
- 删除服务端旧环境变量配置入口、独立 GitHub JSON 和 GitHub Compose overlay。没有兼容分支。
- Compose 只保留编排、挂载、网络、镜像和 PostgreSQL 容器初始化参数。每个程序只读挂载自己的 TOML；健康检查仍校验 CA 与证书名称。
- Desktop、Executor、MCP 的客户端配置行为不变。本次配置调整无需重置数据库或重新安装客户端。

## 限速规则

Server `[relay] default_user_mbps` / `default_guest_mbps` 是启动默认值。
显式值在 `init` 和 `serve` 启动时应用；省略则保留数据库值，新数据库缺省为 5/1 Mbps。
账号单独设置的限速优先，不被启动配置覆盖。Web 运行时修改保留；若 TOML 显式配置，重启后再次应用 TOML。
变更默认值才增加策略版本，重复启动不增加。值必须是 1～2147483647 的整数。

## 已验证

在 Windows 开发机使用增量编译、隔离 PostgreSQL 和本机 TLS 测试服务执行，没有停止或替换已安装的客户端。

| 验证 | 结果 |
| --- | --- |
| `cargo check --locked -p pab-server -p pab-relay --all-targets` | 通过 |
| `pab-service-config` 与 Relay 单元测试 | 14 项通过 |
| Server 单元测试（包括 GitHub HTTP/代理与登录流程） | 26 项通过 |
| `control_tls`：真实 WSS 控制、设备与 Relay 策略 | 1 项通过 |
| `server_settings`：初始值、已有值更新、账号覆盖、非法值原子拒绝 | 4 项通过 |
| `web_management`：会话、权限、设备与统计 | 21 项通过 |
| `config_startup`：实际 Server 可执行文件读取 TOML、初始化与重启 | 1 项通过 |
| `policy_refresh`：实际 TLS/QUIC Relay，配置节点 ID 与统计路径 | 1 项通过 |
| Compose 渲染与 Python 测试配置生成 | 通过 |

`config_startup` 额外验证：旧环境变量不覆盖文件；非法限速不修改数据库；省略字段保留已有值；重启不重复增加策略版本；TLS 校验开启；静态资源、日志相对路径正确；反向代理 Host 不覆盖配置的 Web Origin。

配置单元测试覆盖未知字段、错误类型、UTF-8 BOM、错误信息不回显凭据，以及重复/缺失 `--config` 参数。

## 部署状态

后续已完成部署：Server 和 Relay 均更新至 Linux Release 镜像 **1.2.71**，复用 Cargo 缓存增量构建。
现网环境变量和 GitHub JSON 的值转换为各自 TOML，保留数据库地址、控制密钥、GitHub 凭据和受管理代理。数据库未重启；账号、设备身份及会话保留；部署前已备份数据库与配置。

两个服务 TLS 健康检查通过且重启计数为零。GitHub 开始授权、PKCE、回调以及服务端访问 GitHub 的诊断验证通过：故意提交的无效授权码被上游拒绝，日志分类为 `authorization_code`，并非网络或客户端凭据失败；本轮没有使用真实用户重新授权。

默认限速暂按建议设置为登录用户 **100 Mbps**、访客 **1 Mbps**。测试账号无单独覆盖，实际有效限速为 100 Mbps；Relay 确认应用的策略版本与 Server 下发版本一致。

## 部署前后测速

使用同一个 Windows 目标、同一个随机 8 MiB 文件，通过本机会话原生 `pixels.pab_upload_file` / `pixels.pab_download_file` 传输。连接路径为 Relay；下载文件 SHA-256 均与原文件一致。速率包含工具的完整传输耗时，每个方向各测一次，不代表多轮统计峰值。

| 方向 | 部署前（5 Mbps） | 部署后（100 Mbps） |
| --- | --- | --- |
| 上传 | 19.948 秒，0.421 MB/s | 23.771 秒，0.353 MB/s |
| 下载 | 21.603 秒，0.388 MB/s | 21.022 秒，0.399 MB/s |

旁路验证：通过 SSH 从同一台中继服务器向本机发送 8 MiB 随机数据，耗时 18.411 秒，约 0.456 MB/s（0.435 MiB/s）。该传输不经过 Pixels 的文件协议或应用限速。

结论：配置和 100 Mbps 策略已生效，但本轮没有明显提速。旁路传输也受限，瓶颈指向服务器公网出口或链路，不能继续归因于旧的 5 Mbps 应用限速。主机负载低，`tc` 未发现额外整形限速；云端套餐带宽上限尚未核实，未修改云资源规格。

测试文件已清理。测试目标标识、原始操作记录与部署回滚位置保留在私有 `.build/` 证据文件中，不写入公开文档。本机 Desktop、Executor、MCP 未重装或停止。

模板与操作说明：[服务端部署](../packaging/docker/README.md)。
