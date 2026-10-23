# Relay 多节点与滚动升级

此文适用于通过 `relay.directory` 配置节点目录的管理服务和 Relay。中央 Server 仍是单活控制面；Relay 可以同时运行在原生 Windows、Linux 或容器中。Windows Server 直接运行 `pab-server.exe` 与 `pab-relay-server.exe`，不需要 WSL。PostgreSQL 可以部署在另一台机器。

## 配置与首次切换

完整配置见 `packaging/server/cluster.example.toml`。每个节点必须有独立且固定指向该进程的 HTTPS URL、稳定 `node_id`、独立控制密钥。不能把一个 Relay URL 随机负载均衡到多个不共享连接表的进程。TLS 校验保持开启；自签名部署需要在操控端、Executor、Relay 中显式配置可信 CA。

1. 备份数据库、Server/Relay TOML、证书、Relay 用量队列，先在隔离环境验证升级。新迁移 `0007_relay_cluster.sql` 增加集群表，不删除已有账号、设备或策略记录。
2. 为各 Relay 准备新版本、独立密钥和 URL。客户端 TOML 中现有 Relay 地址继续作为启动种子。先升级客户端代码，保持原单节点模式可用。
3. 首次由旧单节点模式切到集群模式时，停止旧 Relay，等待其最后一次策略有效期结束（当前生产配置为 60 秒），再重启启用了集群配置的 Server。Server 会拒绝旧节点仍在有效期内的切换，避免同一账号同时获得旧完整额度与新集群额度。
4. 启动各新 Relay。初始策略、身份与预算就绪后加入目录。新启动的 Server 有 15 秒预算保护窗口，以覆盖上一个进程的有效预算；这段时间不会放行新的账号转发额度。
5. 在管理后台检查节点状态，运行真实转发探针。普通 `/healthz` 只说明监听进程响应，不能代替真实转发校验。

`RelayAdminState` 的管理意图与健康状态分开：`active / maintenance / disabled`；`starting / healthy / suspect / offline / incompatible`。只有 active 且 healthy 的节点参与新分配。默认心跳 5 秒、失联 15 秒、恢复观察 10 秒、维护等待 30 秒；均在 Server TOML 配置。降低剩余容量只影响可用带宽，不阻止有备用节点的维护。

节点异常退出或数据库恢复后，目录可能暂时保留旧进程的有效心跳。新进程会按重连间隔等待旧心跳过期，并在启动阶段最多重试 60 秒；管理服务暂时不可达也使用这一有界等待。等待期间可正常停止服务。身份、协议或证书校验失败会立即报错。超过等待期限后，应检查重复节点 ID、管理服务和心跳配置，再重新启动。

## 原生安装与升级

管理服务包包含 `pab-server`、Web 资源及部署资料；Relay 包包含 `pab-relay-server`、真实转发探针及节点部署资料。两种包分别安装和升级。只复制样例再填写私有配置，升级时不要用样例覆盖现有 TOML。脚本对服务路径和专属目录进行校验，不允许操作 `PixelsAgentBridgeExecutor`。

Windows 管理员 PowerShell：

```powershell
& .\windows\install.ps1 -Component Server -Action Install `
  -SourceDirectory $PWD -InstallDirectory 'C:\PixelsServer' -ConfigFile 'C:\PixelsServer\server.toml'
& .\windows\install.ps1 -Component Relay -Action Install `
  -SourceDirectory $PWD -InstallDirectory 'C:\PixelsRelay' -ConfigFile 'C:\PixelsRelay\relay.toml'
```

Linux（Python 3.11+、systemd）：

```sh
sudo python3 linux/install.py --component server --action install \
  --source "$PWD" --root /opt/pixels-server --config /etc/pixels/server.toml
sudo python3 linux/install.py --component relay --action install \
  --source "$PWD" --root /opt/pixels-relay --config /etc/pixels/relay.toml
```

更新使用 `Update/update`；回滚使用 `Rollback/rollback`。程序按 SHA-256 存入版本目录，服务配置指向具体版本。配置校验失败时不会停止正在工作的服务；新版本启动失败时恢复原服务。Windows SCM / Linux systemd 配置进程异常重启。服务停止等待有上限，Remove/remove 仅移除服务，不删除配置、数据、日志或版本目录。

`Status/status` 返回实际进程路径、SHA-256、版本目录和安装锁状态。`Restore/restore` 要求显式旧版本 SHA-256，只能恢复安装记录中的版本；它可恢复“进程在停止服务后异常退出”的情况，不会盲目切换到另一个历史版本。

Windows 安装助手直接访问 SCM 和进程映像查询 API，不依赖 WMI。部署指令超时不代表安装失败：先查看 `Status` 的安装锁与运行映像摘要，使用原进度文件恢复，避免重复替换已经成功启动的服务。

## 自动滚动升级

配置示例：`packaging/server/rollout.example.toml`。配置为私有 TOML，凭据不要加入 Git。操控机器需 Python 3.11+，SSH 模式另需 Paramiko，以及与操控机器 OS 相匹配的 `pab-relay-healthcheck`。SSH 校验已固定的主机公钥，不自动信任新主机。目标上的服务必须已由对应原生安装脚本安装；Linux SSH 用户为 root，或使用具备所需 sudo 权限的部署账号。

```sh
python roll_relays.py --config private-rollout.toml
python roll_relays.py --config private-rollout.toml --apply
```

第一条命令只核对包摘要和中央目录。第二条命令逐节点执行：

1. 校验完整包 SHA-256、全部归档路径、解压上限；只暂存 Relay 与安装助手，核对远端程序 SHA-256，并使用原配置预检。此时旧服务继续运行。
2. 在本地进度文件中先记录维护操作 UUID，再请求维护。丢失响应时复用原操作，不创建重复维护。默认 30 秒后继续，不等待长连接自然结束。
3. 替换并启动新版本。若 SSH 中断，恢复时查询安装锁与实际进程 SHA-256，不重复执行结果未知的更新。
4. 等待控制健康，通过临时密钥执行两个方向各 4096 字节的 TLS Relay 转发，核对来源和内容，再恢复 active，观察后处理下一节点。
5. 新版本失败时恢复记录中的旧 SHA-256，再次验证健康和转发；恢复成功也停止本批次。恢复失败则保持维护状态，报告明确原因。

使用相同 TOML 和 `state_file` 重跑会恢复原进度；已经完成的节点不会重复重启。下一批次使用新的 state_file。整个进度文件不能丢弃后盲目重试。没有健康备用节点时拒绝维护，只有显式 `allow_outage=true` 才允许单节点短时停机。

容器节点使用 `deployment="docker"`，填写原有 Compose 项目目录、配置文件和 Relay 服务名，并将镜像固定为 `registry/image@sha256:...`。脚本先拉取和核对镜像，维护后只对该 Relay 执行 `up --no-deps --force-recreate`；沿用原来的挂载、证书、TOML 和用量卷，不执行全栈重建或数据卷删除。旧镜像 ID 留作精确回退，升级期间不要清理镜像。原生 Linux 若使用普通 SSH 部署账号，可设 `sudo=true`；需预先赋予对应 sudo 权限，并让部署账号可写专属 `install_directory/incoming` 暂存目录，版本文件和服务本身仍由 root 管理。

探针身份仅存在约 10 秒，限定一个临时公钥对和最多 64 KiB；它不能授予现有设备访问权，不创建用户设备，也不把探针流量计入用户用量。配置示例 `healthcheck.example.toml`。管理员自动化接口只接受管理员 Bearer token，不能用浏览器 Cookie 绕过。

## 客户端与任务行为

客户端在已有认证 WSS 上周期获取完整目录；不增加常驻控制连接。候选按权重和容量分散，健康主节点保持粘性；节点维护或失联后切换备用节点，使用退避与抖动。设备地址变化会重新上报。上一有效目录作为有界、按控制地址和端点身份隔离的运行缓存保存，最长使用 24 小时；缓存不赋予连接或转发权限。旧 Server 不支持目录时使用 TOML 启动种子。

Relay 不是业务任务执行器。通道短断且 QUIC 连接仍存活时允许继续传输；QUIC 确认结束时，长期终端显示结束、端口转发关闭。已经接受的任务继续按原 task/request ID 查询；不能重放命令、键盘输入或有副作用的点击来伪装成无缝恢复。工具负责发现、重连和状态查询，不要求 AI 自行维护 Relay 身份或生成操作 UUID。

账号上传和下载在所有节点共享一个额度：数据库持久记录短期额度预约，节点本地按预算限速。数据库不处理每个数据包。重启、控制中断和时钟异常时，未到期额度保留，节点不会因反复刷新而重置令牌桶。扩容可提高不同账号的总承载量，不会把单账号额度乘以节点数。

## 回退与排障

- 单节点回退：使用滚动操作的恢复路径，确认旧版本仍兼容集群控制协议。不要直接把旧协议 Relay 放入新集群。
- 整体退回旧单节点模式：先停止所有集群 Relay，等待有效预算到期（最长 15 秒），恢复 Server 单节点配置并重启，再启动匹配的旧 Relay。数据库中的未到期预算会阻止不安全回退。不自动执行数据库降级；需要恢复数据库时，先在独立库验证备份。
- 节点 starting/offline：检查 TLS、控制地址、节点密钥、重复 node_id、策略同步和队列目录可写性。两个并行进程不能共用同一节点 ID。
- 维护进度中断：查看原 operation ID 与 state_file。管理意图持久保留；超过操作租期只解除全局升级锁，不自动激活未知版本。
- 没有备用容量：可添加节点并重载 Server TOML；现有健康会话不会因新节点加入被强制全量搬迁。
- 保留旧程序、原 TOML、证书和用量队列直至确认升级完成。不要执行清空数据卷、数据库或全量容器重建的命令。
