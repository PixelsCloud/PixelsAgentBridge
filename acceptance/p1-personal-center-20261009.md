# P1 个人中心、设备关联和用量验收

日期：2026-10-09。代码基于 `4ece8fa` 后的 P1 功能提交。当前状态：实现、接口集成、三平台原生客户端隔离实测与 1.2.57 构建通过，Server/Relay 已部署，Windows/Mac 已覆盖安装。用户已通过本机 Desktop 登录 `home`，运行中 MCP 身份同步、生产个人 Web 页面与真实传输统计通过。Mac 登录后的 Desktop GUI 和生产管理员页面仍未单独实测，不把开发二进制实测等同于这些界面验收。

## 交付行为

普通账号的 Web 显示个人概览、我的设备、远程设备、用量、账号和设置。管理员可以切换个人中心与服务管理；用户、Relay 和服务设置仍仅管理员可用。

Desktop/CLI 登录后自动关联本机；退出保留关联，其他账号不能静默抢占。更换关联须显式确认。Web 解除后，自动重试和重新登录不会重新绑定。设备 ID、设备码、密码认证链路保持不变。

目标通过密码认证后，目标端回执触发远程设备自动保存。个人备注、移除、明确导入、冲突处理和增量同步按服务地址及用户 ID 隔离。未知系统保持未知，必须连接确认平台后才能验证传输路径。

Relay 记录实际成功转发的加密数据。客户端记录连接次数/时长和文件成功/失败/取消汇总。服务端不保存命令、输出、路径、设备密码或任务详情。批次重发幂等，账号切换不改写旧数据，统计不足时明确显示缺口。

## 环境与保护

- Windows：本仓库 Debug 二进制、独立数据目录、随机 loopback IPC/TLS 端口；10 个真实 MCP stdio 进程。
- macOS：设备 `419438177`，macOS 27.0.1 / aarch64；隔离源码 `/Users/huayang/source/pab-p1-account-20261009`，复用原项目编译缓存，未覆盖原项目的未提交源码。2 个真实 MCP stdio 进程。
- Linux：Debian Bookworm 构建容器，独立数据目录和测试数据库；2 个真实 MCP stdio 进程，无桌面依赖。
- PostgreSQL 测试库使用 SQLx 临时库、`pab_account_test` 或 `p1_` 前缀。没有清空生产库、重新注册生产设备或操作真实用户列表。
- Mac 的凭据测试通过已登录 `huayang` 的 GUI bootstrap 执行。直接从服务 bootstrap 启动的测试曾因 Keychain 上下文失败；没有绕过 Keychain，也没有重置 TCC 或改动用户输入状态。
- 为 Mac 的隔离验收安装了 Homebrew PostgreSQL 17；没有启用登录自启。测试 cluster 位于隔离源码 `.build/p1-pg`，监听 loopback 55439，测试结束已正常关闭。

## 已通过的自动化

| 验证 | 结果 | 证据 |
|---|---|---|
| Server Web/账号/关系/列表/统计集成 | 21/21 | `.build/p1-server-final.log` |
| Windows Bridge 单元及 SQLite 集成 | 79/79 | `.build/p1-bridge-final.log` |
| Mac Core / Bridge / Protocol | 16 + 79 + 51 = 146 通过 | MCP task `a266cf12-725a-47df-9a39-374981ad8f6b` |
| Linux Core / Bridge / Protocol 基线 | 15 + 79 + 51 = 145 通过 | `.build/p1-linux-tests.log`；随后新代码又通过原生全链路 |
| Relay 实际转发及多流用户限速 | 1/1 | `.build/p1-relay-traffic-tests.log` |
| 不可变批次重启、队列满、SQLite FULL | 3/3 | `.build/p1-spool-final.log` |
| Executor 本机证明范围校验 | 1/1 | `.build/p1-local-proof-tests.log` |
| Web 真实浏览器场景 | 7 项均通过（6 项完整运行 + 修正等待后的 session 单项） | `.build/p1-web-final-e2e.log`、`.build/p1-web-session-final.log` |
| Windows Desktop Rust + 两个前端 TypeScript | 通过 | `.build/p1-desktop-final.log`、前端 tsc/build 输出 |
| Server/Relay/Executor/Bridge 全 targets 检查 | 通过 | `.build/p1-all-final.log` |
| Mac Desktop Rust 检查 | 通过 | 同上 Mac task 的 stderr |

Server 集成覆盖签名域、挑战重放、重复请求、并发关联、旧版本换绑/解除拒绝、跨用户列表/详情/统计隔离、客户端伪造 Relay 计数拒绝、同 ID 不同内容拒绝、未来/过旧时间拒绝、小时/日/累计一致、1000 条列表分页中并发删除。

SQLite FULL 使用数据库 `max_page_count` 触发真实错误码 13，确认未确认批次仍保留；没有把宿主磁盘填满。队列上限测试填充 20000 项，确认新批次拒绝、旧批次重试仍成功，ACK 后能继续写入。Relay 实际转发测试比较认证前后用户桶，不能用文件大小直接当加密 Relay 字节。

浏览器覆盖注册/登录/刷新/退出、密码修改、普通与管理员范围、双客户端列表修改、删除、用量、关联解除、三语言/亮暗/窄屏、网络恢复、100 个事件连接。session 用例原来点击退出后立即 reload 会取消尚未完成的请求，已改为等待退出完成再刷新。

## 三平台真实业务链路

可复现入口：`scripts/run_personal_account_acceptance.py`。需要已创建、名称以 `p1_` 开头的 PostgreSQL 库；脚本启动并停止自己的 Server、Relay、Executor 和 MCP，密码通过 stdin，不修改已安装服务。

```sh
P1_DATABASE_URL=postgres://user@127.0.0.1:5432/p1_acceptance \
python3 scripts/run_personal_account_acceptance.py --binaries target/debug
```

Windows 使用 Git 自带的 OpenSSL（PATH 中旧 Strawberry OpenSSL 1.0.2 不支持所需参数）：

```powershell
$env:P1_DATABASE_URL='postgres://postgres@127.0.0.1:55435/p1_account_acceptance'
python scripts/run_personal_account_acceptance.py --binaries target/debug --mcp-count 10 --openssl 'C:\Program Files\Git\usr\bin\openssl.exe'
```

三平台均通过：

1. HTTP 注册 → 本机受保护 IPC 签名 → Web 只显示该用户设备。
2. 退出保留关系；第二个账号登录出现冲突；显式换绑后旧账号不可见，设备身份不变。
3. Web 解除 → Executor 重启 → 重登仍保持解除 → 主动重新关联成功。
4. 多个独立 MCP 实际连接目标，目标回执自动保存设备，连接汇总上报到个人页面。
5. 第二个独立数据目录通过 HTTP 登录同一账号，未共享设备密码或 SQLite，仍同步设备列表和备注。
6. 实际上传 4096 字节随机二进制文件，校验完整内容，并验证成功文件数为 1、完成字节为 4096。

证据位置：

- Windows：`.build/p1-windows-full-acceptance.log`；run `f468736729b74531a979b196d9a2f4d5`。
- macOS：MCP task `00fcd306-67d5-4e60-9e7d-fb4b98676b12`；run `b63e20ddb5664cd0995562517e8337a4`。
- Linux：`.build/p1-linux-acceptance.log`；run `c938456129e6497dbd0ac277b79a00d8`。
- Windows 强制 Relay 模式：`.build/p1-relay-full-acceptance.log`；run `1bcdceecde374a3e935dd9201b3ac7da`。加 `--relay-only`，真实加密流量上下行均通过 Relay 持久化队列到达账号用量。

测试曾使用无服务的 Relay 占位地址，MCP 的上线等待按预期超时；已改为真实隔离 Relay。Mac 测试证书最初带 CA 标记导致 TLS 拒绝，已改为服务器证书 `CA:FALSE`，保留证书验证。

## 1.2.57 发布与安装

全部产物位于 `.build/packages/`，汇总校验文件为 `P1-1.2.57-SHA256.json`。Windows/macOS 为 Release；Linux 无界面和 Server/Relay 本次使用已实测的 Debug 二进制，不能描述为 Release 性能验收。内部模块版本保持不变，仅打包版本递增。

| 产物 | SHA-256 |
|---|---|
| `pixels-agent-bridge-windows-x86_64-release-1.2.57-setup.exe` | `7d083a316e926160ec3fe3ff8da291be9f011392a50ca131acc24057d5b4965f` |
| `pixels-agent-bridge-macos-aarch64-release-1.2.57-setup.pkg` | `089f220abf6b122d18007d4089032d7752847255e45e8cacdb9c86cd40bf436d` |
| `pixels-agent-bridge-linux-x86_64-debug-1.2.57.tar.gz` | `83937fe210a7f52fcb29c1908499c134910c804cdeafe3e2644eafc9b6dd43f9` |
| `pixels-agent-bridge-server-linux-x86_64-debug-1.2.57.tar.gz` | `f79341682d4c832dc3cf77c505a3ed12a358e06ea89cef3669f08248d5712358` |

Server/Relay 部署前已备份 PostgreSQL 与配置到 `/opt/pixels-agent-bridge/backups/p1-20261009-042235`，并检查备份可列出内容。数据库仍为 `pab_accounts_20261008`；前后均为 2 个用户、4 台设备，设备身份摘要均为 `e81d17227bae0d8bad3b4118b633d2d0`。只新增迁移 2—4，没有清库。两服务镜像为 `pixels-agent-bridge:1.2.57`，镜像 ID 为 `sha256:33b2b1dac9d7b7336c6476f236ab8aece7567e17d7fb690d1760f7b618640960`，健康检查通过。

公网首页、JS/CSS 与发布资产逐字节一致；匿名访问设备、远程列表、用量接口均返回 401。证据为 `.build/p1-production-release.json`、`.build/p1-production-http-smoke.json`。未使用真实用户密码做生产账号写入烟测，账号权限完整流程使用隔离测试库完成。

Mac 安装最初被运行中的 Desktop/MCP 拦截，未替换程序；停止重复启动的安装任务后，关闭本应用的 helper/MCP，再通过 `KeepAlive=false` 的独立一次性任务安装，退出码为 0。任务已移除。三个已安装二进制均与发布归档的哈希一致，`codesign --verify --deep --strict` 通过，Executor/helper 正常运行，Desktop 已重新打开。设备 ID `fbe6603e-27bd-460f-bc0b-abe142a3fc35`、设备码 `419438177` 保持不变，MCP 已重新连接并执行命令。账号状态为游客，没有现有登录会话可用于证明升级后的登录保持。

本次修正了 Windows 检出源码在 Mac 打包时带入 CRLF 的问题：Unix 脚本、plist 和 PKG 前后置脚本打包时归一为 LF。Windows 包测试通过；Mac PKG 6/6 包测试通过，并已完成实际安装。固定免费证书未更换；PKG 未使用付费 Installer 签名或公证。

Windows 本机在 SQLite 在线备份后执行 1.2.57 安装包 `/S`，安装退出码为 0。三个已安装程序均匹配 `.build/builds/desktop-release.json`；Executor 为 Running，控制连接为 Authenticated，Relay 已连接。设备 ID `aeed3b2e-336d-4c55-b987-b7275893cba3`、设备码 `993139780` 均保持不变；账号仍为游客。证据 `.build/p1-windows-install-verified.json`、`.build/p1-windows-install.exit`。当前 Codex session 的旧 MCP 已被安装器停止，必须重启 session 才能用新 MCP 继续工具验收。

回退须遵循 `WEB_DEPLOYMENT.md`：旧 Server 会拒绝新增 SQLx 迁移版本，应将升级前备份恢复到独立数据库再切换，保留更新后的库和队列，不能删除迁移记录或覆盖当前生产库。

### Session 重启后复验

2026-10-09 13:15 后启动的新 MCP 进程路径为 `C:\Program Files\PixelsAgentBridge\pab-mcp.exe`，文件哈希仍为发布记录中的 `5330710c0a506c0b51e01455133d044c773f3804541203416dce48a3b8b07d1a`。实际调用 `pixels.pab_list_devices`、`pixels.pab_connect`、`pixels.pab_run_command`、`pixels.pab_file_hash` 和 `pixels.pab_get_operation` 均成功。Mac 设备码仍为 `419438177`，身份与安装前一致；异步哈希读完 26377952 字节，结果匹配安装后的 MCP 发布哈希。命令 task `22e89bc0-f85d-4976-8ffc-ed88503f52d8`；哈希 operation `a899806b-e86a-420e-8fad-7541f9e5cda8`。

Desktop 的 `0.0.0.0:26035` 上报服务正常，识别当前 Codex MCP（PID 32352），显示控制连接已认证、Mac 已连接，以及成功任务状态。快照 `.build/p1-new-session-desktop.json`。上报的 `1.2.48` 为按用户要求保持不变的内部模块版本，不代表仍运行旧安装包。此时本机账号 revision 为 0、user 为 null，已经请求用户通过 Desktop 登录，等待验证运行中的连接更新账号身份。

### 本机真实账号登录后验收

用户在 Windows Desktop 登录普通账号 `home`（`78e7862f-93c8-4757-8d62-20b8acb78102`）后，原 MCP PID 32352 的 local/server/remote revision 均变为 1，Relay `cn-primary` 确认策略版本 371。最初的身份同步观察中，Mac 连接仍保持原 `changedAtUnixMs`，无需重新启动 MCP 或重新输入设备密码。随后远程命令 task `71d013b1-1e3a-49b4-8013-ad79dd13949a` 的 `initiating_user` 为 `home`，执行成功。

使用当前 Desktop 登录会话进行了生产 HTTP 查询和独立无头 Chrome 个人页面实测；凭据只在内存与子进程 stdin 中传递，没有写入截图、日志或浏览器存储文件，没有改写账号密码，也没有模拟管理员权限。复用了已建立会话，不将此项描述为重新输入密码的 Web 登录表单测试。

- “我的设备”仅有已自动关联且在线的本机 `993139780`；Mac `419438177` 仅出现在个人“远程设备”，在线且已验证。三个游客历史条目保留，未自动导入。同步 pending=0、无错误、无冲突。
- 普通用户读取全站设备、全站用量、账号管理均返回 403；读取未关联 Mac 的“我的设备”详情返回 404，已保存远程设备不赋予本机关联管理权。
- `/devices`、`/saved-devices`、`/usage` 三个生产页面可见对应设备与统计，刷新保留会话，无 pageerror。有效账号带宽显示 5 Mbps（游客 1 Mbps）。截图等待表格加载结束，设备码断言接受页面的三位分组空格。
- 实际上传 4096 字节随机文件至 Mac 的隔离测试目录，operation `034ec6ef-0487-49d0-90e6-61eca81b91d7` 标记发起用户为 `home`；源端与目标端 SHA-256 均为 `05dc74fd95c386e9fdb9f338681eb9bdc14584e7852429119f365d7abd53f649`。上传前后个人统计准确增加 1 个成功文件、4096 字节。连接时长及 Relay 汇总也有实际上报，不把文件大小当作 Relay 计数。
- 测试文件已通过 `pab_file_delete` 删除，operation `20de5402-86a3-4230-a28d-235d59eff001` 确认为 completed。未更改用户个人备注、关联关系或游客列表。

证据：`.build/p1-home-live-acceptance.json`、`.build/p1-home-before-upload.json`、`.build/p1-home-runtime.json`，截图 `.build/p1-home-devices.png`、`.build/p1-home-saved-devices.png`、`.build/p1-home-usage.png`。本次仅补充验收与文档，没有重新编译、打包或部署。

## 尚未完成及验证范围

以下是本记录形成时的状态。2026-10-09 的后续 Mac/管理员 GUI、同账号多设备自动关联、Release 部署与限速验收已完成，详见 [P1 收尾验收](p1-closeout-20261009.md)。该记录也替代本文早期的「第二个账号登录冲突、显式换绑」规则；当前为登录自动更换本机归属。

- Windows Desktop 登录后的真实业务链路、生产普通用户页面已通过；Mac 登录后的 Desktop GUI、生产管理员登录后页面尚未单独实测（其账号隔离和管理员页面已有隔离环境测试）。
- Windows 覆盖安装、新 session MCP、已建立连接中的真实账号身份更新均已通过。
- 实机断电、宿主物理磁盘耗尽没有执行；使用数据库 FULL、队列上限、重启恢复和重复 ACK 场景验证对应边界。
- Relay 统计按节点聚合，不是跨节点统一限额或计费数据。客户端统计为自报；最后一个未持久化窗口可能丢失，页面显示缺口。
