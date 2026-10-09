# GitHub 登录实施与验收

状态：主要代码已实现，本地自动测试通过；Server/Web 1.2.66 已部署，90 Windows 1.2.67 与 Mac 1.2.68 均完成覆盖安装。Mac Desktop 与 Safari Web 的真实 GitHub 登录、自动建号、同一身份复用、设备自动关联和 Desktop 重启保持登录已验证。Windows 仅验证跳转和取消，未登录 GitHub。私有 App 配置未输出或提交。沿用 USER_ACCOUNT_PLAN.md 第 14 节。

## 交付顺序

1. 外部身份及短期授权事务表、共用持久会话签发、无密码用户处理。
2. Server OAuth/PKCE、Web 登录与绑定/解绑、Desktop 回环回调与一次性兑换。
3. React/Ant Design 入口、取消/错误处理、中英繁体文案；复用账号保存、IPC 通知及设备关联。
4. 隔离 PostgreSQL 与模拟 GitHub 服务测试，再执行 Web/Desktop 构建及实机验收。
5. 配置实际部署，真实 GitHub 授权由账号持有者完成；记录已验证与待验证项。

## 必测边界

- 同一 GitHub ID 首次/重复登录和改名仍对应同一 PAB 用户；与本地同名账号不合并。
- 绑定冲突、并发登录/绑定、停用账号、注册关闭、解绑后仍有登录方式。
- state 和独立 Lax 事务 Cookie 不匹配、过期/重复回调、PKCE 错误、拒绝授权。
- 一次性兑换绑定本机校验值，错误校验值不得消耗合法兑换；过期、重放、应用取消/退出。
- 返回地址仅 IPv4 回环随机端口固定路径，长期会话令牌不进入 URL；GitHub Secret/Token 不下发客户端。
- 密码登录回归、主动退出、持久会话、现有账号 IPC 热更新、Windows/macOS 编译，Linux headless 保持密码 CLI。
- GitHub 网络失败/限流、响应过大、跨站请求、不可信跳转与请求日志脱敏。

现有 Mac 点击修复及 1.2.63/64 安装状态保持不变，不清库、不全量 clean，不覆盖用户已有未提交文件。

## 本轮验证结果

- Server：17 项库测试及 21 项 Web 管理集成测试通过；使用隔离 PostgreSQL 数据库和模拟 GitHub 服务。
- Desktop：2 项原生回调测试通过，覆盖 Host/state、单次消费和退出释放监听端口。
- Desktop 浏览器：2 项测试通过，覆盖重命名弹窗保存和 GitHub 等待授权时取消。
- Desktop、Web 前端构建通过；Linux Server/Relay Release Docker 镜像 1.2.66 构建成功，Server/Web 已部署，Relay 保留 1.2.61。
- 修复重命名弹窗误用详情面板草稿，保存按钮现在使用弹窗自己的输入值。标题读取打包版本，显示 `Pixels Agent Bridge (v 版本号)`。
- 本机安装未替换，也未停止 Desktop、Executor 或 MCP。上一轮 Pixels MCP 返回 `Transport closed`；本轮用户重启会话后已恢复，远端安装进度见下。
- 未完成：已有密码账号的真实 GitHub 绑定/解绑、多 MCP 热更新实测、Windows 真实授权完成；网络超时、上游限流和超大响应尚未逐项注入测试。头像元数据已存储，页面展示尚未接入。

## 2026-10-09 部署及浏览器验证

- 新镜像在独立容器与隔离数据库中启动成功，`github_enabled=true`。
- 新增 2 项 Playwright 测试通过：空白登录表单可直接跳转 GitHub，拒绝授权及回调重放有明确结果；跨站请求及非回环地址拒绝，不产生登录会话。测试使用虚构 App 配置，并拦截外部授权页面。
- 同一 Release 测试容器的密码账号浏览器回归通过：注册、刷新保持会话、个人设备列表、主题偏好、错误/正确旧密码、更改密码后保持登录、主动退出和重新登录。
- 生产数据库先完成 `pg_dump` 备份，再将备份恢复到临时数据库，使用新镜像演练迁移 1–4 到 1–5。原账号、设备身份、密码摘要和登录会话均保持。演练数据库完成后删除。
- 部署时仅更新 Backend；Relay 容器未重建，健康检查正常。生产 4 个既有账号、5 台设备、5 个会话保持，设备身份及密码摘要校验一致；升级不清库。
- 私有 GitHub JSON 通过 SSH 上传到服务端 secrets 目录，只读挂载给 Backend，未进入源码、镜像或公开日志。
- 公开站点真实浏览器检查通过：「使用 GitHub 登录」显示，点击后到达 GitHub 官方登录页面。尚未代替用户完成 GitHub 授权，不将跳转成功视为完整登录验收。
- 新 Windows 安装包 1.2.67 增量编译完成：`pixels-agent-bridge-windows-x86_64-release-1.2.67-setup.exe`，23,690,711 字节，SHA-256 `3c73fbab365a9d5373ffbc29da6a33dca5ebb55f05895532c03f69960fc7c172`。已安装到 90；本机已安装程序保持运行，未安装或停止服务。
- 独立部署新增可选 `compose.github.yaml`，配置检查通过；中英文 README 补充无需另填注册表的 GitHub 登录说明。待提交源码已与私有配置中的实际 Client ID、Secret 比对，未发现泄露。

## 2026-10-09 远端覆盖安装

- 90：通过原生 Pixels MCP 核对 Windows 主机与目标 IP 后，二进制上传 1.2.67 安装包，校验 SHA-256。独立计划任务停止服务、备份机器 SQLite 及端点密钥，再执行默认目录静默覆盖安装。安装器退出 0，三个程序的安装哈希与 Release 清单全部一致，注册表版本为 `1.2.67 release`。
- 90：服务恢复 Running，并产生新的 Authenticated 心跳；MCP 使用原设备码重新连接成功，device/tenant 身份与端点密钥保持。本机用户未登录 PAB 账号，未发现需要保留的账号 JSON；不将此视为账号登录保持验收。完成的安装计划任务已移除，结果和备份保留。
- 90：原生应用启动与窗口截图确认标题 `v 1.2.67`、本机服务运行、服务端已连接。登录弹窗显示 GitHub 入口；点击后 Chrome 到达 GitHub 登录页面。取消等待后 Desktop 仅监听正常的 26035 端口，OAuth 临时监听已释放；未代替用户输入 GitHub 凭据或完成授权。
- Mac：源码快进至 `fcee84e`，保留构建缓存和固定签名。使用仓库 macOS 构建入口生成 aarch64 Release 1.2.68。后台启动环境问题在预检阶段修正，crates.io 短暂超时后自动恢复，未更换签名或清理数据。
- Mac：新 PKG 32,366,859 字节，SHA-256 `8e01205dbe6a374c8e26c53ffde1e9376a9cb5d03b9df19c550aa9b2f8feae2d`，已通过原生 MCP 下载并在本机重新校验。安装前使用 SQLite backup API 备份机器/用户数据库；独立 launchd 任务安装退出 0，所有安装文件哈希匹配，设备码、设备 ID、端点密钥、1 份账号元数据及 Desktop/Executor/MCP 的固定签名要求全部保持。
- Mac：MCP 使用原设备码重新连接成功，Executor 新心跳为 Authenticated。系统报告 `CGSSessionScreenIsLocked=true`，当前截屏只有锁屏背景，窗口不能获得可控制的 Accessibility 引用；未将此当作 GitHub 界面通过或权限丢失。此轮不注销账号、不重置 TCC，解锁后的完整界面验收待继续。
- 两边安装任务均已卸载，安装日志和备份保留。Mac 构建分配的版本 1.2.68 已同步回本地 `build-version.json`；内部 Rust/npm/Tauri 版本不变。本机 Desktop、Executor、MCP 未重装或停止。

## 2026-10-09 Mac 真实 GitHub 登录验收

- 用户要求在 Mac 完成实际登录。开始时 Mac 已解锁，Safari 已有 GitHub 登录态；不读取浏览器 Cookie、不索取 GitHub 密码，不将授权码、令牌、Client ID 或实际 GitHub 用户标识写入验收文档。
- 从 Mac Desktop 退出原密码账号，在空白登录表单点击「使用 GitHub 登录」。Safari 显示项目 App 的真实授权页面，授权后经 Server 回到本机临时监听，Desktop 自动进入「我的」。未填写新用户名、密码或注册表。
- Server 首次创建 1 个无密码普通用户，Desktop 显示 GitHub 已关联且不能解除唯一登录方式，本机自动关联成功。设备码、端点身份与机器控制密码未改变。
- 在同一 Mac Safari 打开公开 Web，点击 GitHub 登录后直接进入个人中心；Web 和 Desktop 对应同一个 PAB 用户，GitHub 身份及用户数量均保持 1，没有重复注册。Web 概览显示 1 台设备在线。
- 数据库只读聚合核对：1 个 GitHub 账号、1 个无密码账号、1 个普通用户、2 个持久会话（Desktop 与 Web）、1 台关联设备。未授予管理员权限。
- 仅重启 Mac Desktop GUI（未停止 Executor/MCP），新 GUI 进程仍显示同一 GitHub 用户，本机服务和服务端连接正常，证明本机凭据保存与重启恢复可用。
- 最终 Mac 保持该 GitHub 账号登录并归属该账号；原密码账号及其他设备不删除、不合并。90 仍未登录 GitHub。实际授权、原生回调、设备关联及重启保持的核心登录流程通过。

## 2026-10-09 更换 App 后的服务端网络故障

- 用户提供新 App 私有配置后，仅替换服务器只读配置并重建 Backend；原配置留有私有备份，客户端、数据库和 Relay 未重装或重启。浏览器检查确认使用新 Client ID、正确回调与 S256 PKCE；跳转检查不代表授权码兑换成功。
- 随后真实 Web 登录报告 `github_unavailable`。在服务器及 Backend 容器复现：`api.github.com` 可访问，默认 DNS 返回的 `github.com` 地址在 TCP 建连阶段超时，因此无法访问授权码兑换接口。
- 另一个公共 DNS 返回的 GitHub 地址通过同容器 HTTPS 证书验证及接口访问。暂时在私有部署 Compose 的 Backend `extra_hosts` 中指定该地址，保持请求域名和 TLS 验证，仅重建 Backend；这是一项部署网络绕行，固定地址不作为源码默认配置，后续需在默认线路恢复后移除或更换可靠出口。
- 修复前通过显式解析、修复后通过容器正常解析，使用无效诊断授权码请求兑换接口，均收到 GitHub 的 `bad_verification_code`，未返回 `incorrect_client_credentials`。该探测不创建用户或会话，也不能替代新 App 的真实授权验收；需用户重新发起登录完成最后验证。原失败回调不可重放。
- 所有真实 Client ID、Secret、用户标识和配置备份均保留在私有位置，未写入本记录。

## 2026-10-09 GitHub 正常网络出口修复（替代上述临时配置）

- 云主机元数据确认：现有 Web/账号 Backend 位于北京，另一台现有服务器位于新加坡。业务 Backend 未迁移；仅 GitHub 服务端 HTTP 请求使用新加坡出口。复查时北京默认 GitHub 地址再次可达，说明此前是间歇性建连故障，不能把固定 IP 视为长期修复。
- 新增 `PAB_GITHUB_PROXY_URL`，复用 reqwest 的 HTTP/HTTPS CONNECT、SOCKS5H 实现，不自行实现代理协议。生产使用 OpenSSH 动态转发，在出口侧按域名解析，GitHub HTTPS 仍由 Backend 验证证书，无固定 GitHub IP、TLS 解密或第三方代理服务。
- 专用 SSH 用户/密钥只允许 `github.com:443`、`api.github.com:443`，禁止命令会话和其他转发目的地。代理仅监听 Docker 私网接口，防火墙仅允许应用桥接网络进入，没有公网代理端口。systemd 开机启动、保活及重启配置已部署；结束通道主进程后自动恢复及 HTTPS 再次可用均实测通过。
- 每分钟健康探测已部署，探测不携带 OAuth 凭据；注入不可用代理后服务检查失败，恢复配置后检查成功。当前提供 systemd 状态和 journal 记录，未另接外部消息告警渠道。
- 建连超时 5 秒、单次请求总超时 15 秒，仅建连错误重试一次。响应超时、连接在请求后中断、HTTP 错误不自动重放一次性授权码。错误日志只记录阶段/类别/状态；GitHub 返回 HTTP 200 的 OAuth 错误也能归类，避免把错误 Secret 当成无法解析的成功响应。
- 新增 7 项测试通过：代理配置校验、SOCKS5H 远端解析、建连恢复、HTTP 503 不重放、响应中断不重放、响应超时不重放、HTTP 200 OAuth 错误解析。原有账号及 Web 管理回归通过。
- 仅 Backend 部署到增量构建的 1.2.70。检查实际 Compose、容器 ExtraHosts 与 `/etc/hosts`，均已移除 GitHub 固定 IP；原数据库及 Relay 容器未重建，本机及远端 Desktop/MCP 未重装。
- 公开浏览器检查使用新 App、正确回调及 S256 PKCE。通过真实生产 `/github/start` 和 `/callback` 发起无效诊断授权码：实际 Rust 客户端已启用代理，并收到 GitHub `bad_verification_code`，日志分类为 `authorization_code`，没有超时或错误创建用户。该验证证明兑换链路可达，不冒充新 App 的真实用户授权登录验收。
- 运维配置及撤回步骤见 `packaging/network/README.md`；真实出口地址、SSH 私钥及 App 凭据只存放在私有配置。后续出口或 GitHub 故障仍可能发生，应由健康监测定位，不作“永远不会失败”的承诺。
