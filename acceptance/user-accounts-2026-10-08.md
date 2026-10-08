# HTTP 用户账户、MCP 同步与用户限速验收

日期：2026-10-08。范围：本次源码与隔离测试环境；Windows 安装包已生成（见文末）；未部署生产服务、未替换设备上已安装的程序。

## 实现范围

- 删除 Team 页面、接口、管理命令、成员关系和限速模型；采用新的 `0001_initial.sql`，不兼容旧库。没有为测试删除现网数据。
- Desktop 注册/登录和 Linux 无界面账户命令使用 HTTPS；浏览器 Cookie 与 native Bearer 会话分开。服务端会话无业务到期时间、无数量淘汰；改密保留既有会话，主动退出撤销当前会话。
- 登录不替换设备控制 runtime、端点身份或连接。MCP 从同一系统用户的账户存储读取凭据；Desktop 的无认证 `0.0.0.0:26035` 只广播 revision。漏通知时 3 秒轮询补偿。
- 端点私钥签名与 HTTP 会话共同确定用户；Server/远端不信任客户端提交的用户名或额度。新请求流验证当前上下文；正在执行的流与任务继续使用原所有权。
- Relay 按 user_id 聚合同一节点上多个 MCP 的额度；游客按端点。支持全局默认和用户覆盖值；策略推送、实际应用版本与界面回执分开。
- 任务只存本地。远端任务/传输保存接收时验证的用户；本机排队操作保存提交时的用户，重复请求不会改写。排队期间切换账号时，这两个时间点可能不同，本机历史快照不用于授权。

## 自动化证据

日志保留在本机 `.build/account-*.log`，不提交含环境信息的完整运行日志。可重复执行的测试源代码随本次修改保留。

| 范围 | 结果与证据 |
|---|---|
| agent-core / protocol / Relay 单元 | 14 / 51 / 11 通过；`account-unit-tests.log`。包括存储恢复、地址隔离、用户聚合和切换不重置突发额度 |
| Bridge 单元 | 76 通过；`account-bridge-final-tests.log`。包含排队时 runtime 尚未初始化、A→B→游客、相同 request_id 保持原用户、数据库重开恢复 |
| Server HTTP/数据库 | `web_management` 14 通过；`account-http-final-tests.log`。native/browser 互通与渠道隔离、永久会话、28 个会话共存、改密、禁用/恢复、签名/重放/陈旧版本、用户额度权限 |
| 设备控制基础 | control_plane、guest_registration、server_settings 均通过；`account-http-tests.log` |
| 真实 TLS/WSS/QUIC | `control_tls` 通过；HTTP 用户身份在既有连接中 A→游客→B，已有任务身份不变，其他端点不能接管；`account-tls-tests.log`。最终回归 `account-tls-final-tests.log` 通过（86.14 秒），增加持久化失败后撤销新会话 |
| Executor 归属 | 定向任务发起人快照测试通过；`account-attribution-tests.log` |
| Relay 策略刷新 | 真实 TLS Relay/QUIC 测试通过；强制中继、新准入即时刷新；`account-relay-refresh-tests.log` |
| 实际 stdio MCP | 两个独立进程初始化、报告、登录/退出/切换、保留进程会话、停止单个不影响另一个通过；`account-real-mcp-test.log`。最终 10 个真实 MCP 压力回归通过（73.12 秒），刻意漏发第二次登录通知仍由轮询恢复；`account-ten-mcp-test.log`。另修复退出时未等待已取消报告/连接任务释放的问题，验证其余进程正常退出（exit 0） |
| Desktop 页面 | 27 项已有页面回归 + 1 项新增注册回归通过；`account-desktop-browser.log`、`account-desktop-register.log`；三种语言类型检查、最终资源构建通过 |
| Web 页面 | 6 项浏览器端到端通过；`account-web-browser.log`。真实 HTTPS/PostgreSQL、用户额度覆盖/恢复、在线设备、注册/登录/改密/退出、刷新、三语/响应式/离线、100 个 WS 连接 |
| Windows native CLI | 注册、跨进程恢复、退出、重新登录、revision、凭据清理通过；`.build/account-native-cli-test.py` |

## 真实 Relay 吞吐

`cargo test --locked -p pab-relay --test user_traffic -- --nocapture`

两条独立 QUIC 连接强制使用本地 TLS Relay，禁用直连。两条二进制流持续发送且逐帧校验内容；每阶段预热 1 秒、采样 3 秒，过程中不重连。测试直接应用生产策略快照；HTTP 身份绑定和策略控制通道由其他集成测试分别覆盖。

| 当前身份和策略 | 实收总吞吐 |
|---|---:|
| 两个游客，各 1 Mbps | 1.729 Mbps |
| 切为同一用户，总计 4 Mbps | 3.929 Mbps |
| 用户覆盖值降为总计 2 Mbps | 1.921 Mbps |
| 退出回到两个游客 | 1.916 Mbps |

测试通过，`account-relay-throughput.log`。证明两连接共享用户额度，已连接流随策略改变速度，数据未串流。此结果是一个 Relay 节点的实测，不是多个节点的全局总限速，也不是公网带宽保证。

## macOS 实机

- 设备 `603527578`，macOS 27.0.1 ARM64。仅上传源码至 `/Users/huayang/source/pab-account-check-20261008`，复用编译缓存；没有改动已安装应用或原仓库源码。
- Bridge、Executor、Desktop 编译检查通过；最终日志 `/tmp/pab-account-final-build.log`、`/tmp/pab-account-desktop-final-check.log`。第三方 enigo 有既有 warning。
- 账户存储测试 3 项通过，在 huayang 的 GUI 登录会话运行。测试 token 为固定无效值，绝不连接真实账号。
- 发现并修正：由免费签名的应用直接创建钥匙串项时，另一个可执行文件以及更换二进制后可能无法静默读取；仅固定签名标识不足以保证这个场景。
- 最终使用系统 `/usr/bin/security` 访问用户 Keychain。写入命令经私有 stdin，token 不在 argv/env；读取经私有 stdout，不记录输出。没有 `-A` 放开所有应用、没有自制加密、没有新增 helper/常驻进程，也没有新增 MCP 凭证读取命令。
- Desktop 签名进程写入 → MCP 签名进程读取 → 新 Desktop 进程读取，通过。修改测试二进制内容并改签为另一应用标识后，仍能读取原凭据，随后删除测试凭据，通过。对应远端任务 `54894d60-d0ee-4e74-abd9-4f887600509f`。
- 测试期间发现登录钥匙串锁定，确认是操作系统存储状态；解锁后继续测试。程序访问设置 5 秒上限，失败保留账户元数据，不擅自修改系统钥匙串设置或获取其他系统用户的凭据。

## Linux 无界面

隔离容器 `pab-execution-e8-linux`，源码 `/tmp/pab-account-source`、编译输出 `/opt/pab-account-target`（容器 `/tmp` 不可执行）。账户 3 项测试与 Bridge/Executor 编译检查通过，日志 `/tmp/pab-account-linux-final.log`。不依赖 DBus、桌面会话或 Linux GUI；凭据目录 0700、文件 0600。

## 复测入口

Windows PowerShell；数据库必须是隔离 PostgreSQL，SQLx 为每项测试创建独立数据库：

```powershell
$env:DATABASE_URL='postgres://postgres@127.0.0.1:55435/postgres'
cargo test --locked -p pab-agent-core -p pab-protocol -p pab-relay --lib
cargo test --locked -p pab-bridge --lib
cargo test --locked -p pab-server --test web_management --test control_tls
cargo test --locked -p pab-relay --test policy_refresh --test user_traffic
cargo check --locked --workspace --all-targets
cargo check --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets
```

真实 MCP 测试需要已启动隔离 HTTPS Server、编译好的 `target/debug/pab-mcp.exe` 和 `.build/web-test/cert.pem`：

```powershell
$env:PAB_MCP_SMOKE_EXE=(Resolve-Path target/debug/pab-mcp.exe).Path
$env:PAB_MCP_ACCOUNT_TEST_URL='wss://localhost:38443/control'
$env:PAB_MCP_ACCOUNT_TEST_CA=(Resolve-Path .build/web-test/cert.pem).Path
$env:PAB_MCP_SMOKE_COUNT='10'
cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml real_mcp_stdio_processes_register_before_tools_and_clean_up_on_exit -- --ignored --nocapture
```

浏览器环境见 `WEB_DEPLOYMENT.md`。仅构建前端资源使用 `npm run build:assets`，避免占用安装包版本号。

最终 Windows workspace 全目标检查、Desktop 检查通过；Mac ARM 和 Linux 无界面最终检查通过。`git diff --check` 通过。

## 发布边界

- 已按用户后续要求生成 Windows 安装包，并完成 Server/Web/Relay 公网部署（见下节）；未提交、push 或安装客户端，旧客户端安装不包含本次功能。
- 发布必须使用同批次 Server/Relay/Executor/Desktop/MCP。Relay policy schema 5、设备认证及任务会话 schema 2；按要求删除旧开发库后初始化，重新注册用户和设备。不提供旧协议或旧库兼容。
- 未做 Mac Intel 实机、Windows 90 的安装后完整链路或多 Relay 节点公网验收。当前跨平台证据是上述源码构建/存储实机及隔离集成测试，不能替代安装发布验收。
- 浏览器受 Cookie 保存期限/用户清理数据约束；服务端会话自身永久有效。HTTP 账户 API 从 WSS 地址推导同源 HTTPS，不支持把凭证转发到另一个 API 域名。
- 远端每个新请求流多一次 Server 授权查询；Server 不可用时既有流可继续，新流等待/失败。高频 UI 的额外延迟尚未做公网专项测量。
- 如果凭据落盘失败，同时撤销请求也遭遇断网，不能保证服务端孤立会话已撤销；不会把它报告为已登录，客户端不再持有可恢复的活动凭据。


## Windows 安装包（后续打包请求）

- 命令：`python scripts/build.py desktop --profile release --package`，保留 Cargo 缓存，未执行 clean。
- 发布版本：1.2.52；Executor/MCP 内部版本仍为 1.2.48，Desktop 仍为 1.2.49。
- 产物：`.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.52-setup.exe`，23,402,431 bytes。
- SHA-256：`f85f65fe116e3a45f63df9a9bf1d30c555aadba959384fbce091849cd947d24b`。
- 安装器 FileVersion/ProductVersion、ZIP CRC、三个组件与构建清单的 SHA-256 均已核验。
- 日志：`.build/account-release-package.log`。只打包 Windows 客户端，未安装、未部署；新协议仍要求配套更新 Server、Relay 和远端客户端。

## 公网部署（后续部署请求，2026-10-08）

- 目标：`https://pab.rgaa.vip`（Server/Web）、`https://pab-relay.rgaa.vip`（Relay）；CN 主机 `/opt/pixels-agent-bridge` Compose 项目。
- 增量构建命令：`python scripts/build.py docker --profile release`，日志 `.build/account-server-release-build.log`；保留 BuildKit/Cargo 缓存，无 clean。部署镜像 `pixels-agent-bridge:1.2.53`；Windows 安装包仍为 1.2.52，二者为同批次业务代码，内部模块版本不递增。
- 镜像 ID：`sha256:8301ebcb7b1e4d30227532b035a738da3ceb0135e3ddbd64db4dc29202f1a242`；传输归档 43,263,279 bytes，SHA-256 `b933db027db076caec17d9bfd6a49c57e0f47f2c0e70e70d358e6ede582e59b0`，远端加载前后均核对。
- 新数据库 `pab_accounts_20261008`，初始结构版本 1，无 Team/成员关系表。切换并验收后删除旧运行数据库 `pab_no_id_20261004`，确认不存在；没有迁移旧设备或旧用户。未触碰 PostgreSQL 中其他历史数据库。
- 通过新 HTTP 注册接口重建 Pixels，再通过 `pab-server web-admin Pixels` 授权；沿用受保护本地凭据中的用户名和密码，测试产物不保存口令或会话令牌。
- Server/Relay 容器健康；节点 `cn-primary` 最终 offered/applied policy 均为 7。默认用户 5 Mbps、游客 1 Mbps。
- 公网 HTTPS 验收：管理员管理接口、Web cookie 与 native Bearer 渠道隔离、Team/任务 API 返回 404；重启 Server 后两个会话仍有效；退出只撤销当前会话。测试使用正常证书验证。
- 使用 Windows 安装包对应的 Release `pab-mcp.exe`，隔离用户目录完成公网 HTTP 登录、跨进程恢复、退出和凭据清理。
- 实际 Chrome 公网页面完成普通用户注册、刷新保持登录、退出、Pixels 登录、设备/账户/管理变更/Relay 页面、无 Team 菜单；事件 WebSocket 收到刷新帧，页面 JS 错误为 0。
- 验收账号及其会话/个人命名空间已删除。最终保留 Pixels 管理员，设备数为 0（清库后的预期状态），验收会话数为 0。
- 证据：`.build/account-rollout/{staged,initialized,deployed,public-http-smoke,public-browser-smoke,final}.json`，截图 `.build/account-rollout/public-relay.png`。操作脚本 `.build/account-rollout.py` 的初始化/清理步骤为一次性操作，不应直接重复运行。
- 本次没有安装或重启本机/远端客户端。旧客户端数据及设备身份不能继续使用；应安装同批次客户端并重新注册。真实远端设备控制和公网 Relay 吞吐尚未进行更新后的安装验收，不能用本次 HTTP/Web 验收代替。
