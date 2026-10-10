# Server Web deployment / 部署与验证

## Scope / 范围

**P1 更新（2026-10-09）：** 个人设备、远程列表、用量统计使用新增的 `0002`—`0004` 数据库结构。对已运行的 HTTP 账号版本执行普通 `pab-server init`，保留账户、会话、设备身份及设备码。下文 2026-10-08 的一次性开发清库记录不适用于 P1，不能再次照着清空现网。

先备份数据库与部署配置，更新 Server，再更新 Relay，最后更新 Desktop/Executor/MCP。Relay 新增 Relay TOML `usage_dir` 持久化目录；Docker 镜像与挂载目录必须可由 `pab` 用户写入。每个 Relay 使用独立、稳定的节点 ID 和自己的队列。回退保留新增表和队列，不运行降级删表或设备重注册。

**P1 回退边界：** 旧 Server 二进制不包含 `0002`—`0004`，SQLx 会拒绝带有这些迁移记录的数据库，不能只换回旧镜像。需要回退时，在维护窗口将升级前备份恢复到另一个数据库，再让旧镜像指向该恢复库；保留当前数据库和 Relay 队列，单独核对升级后的新增写入。不得删除迁移记录来强行启动旧版本，也不得覆盖当前生产库。

The React + Ant Design console is served by `pab-server` on its existing HTTPS listener. No Node runtime is required in production. Server stores account, device, user bandwidth and Relay management data only; remote task history, command output, files and screenshots remain local to Bridge/Executor. Device **online** means an authenticated device control connection is alive. This release supports one active control Server per deployment.

Web 与现有 `/control`、`/relay-control` 共用 HTTPS 端口，不新增公开端口。任务记录只保存在本地，不上传 Server，也不在 Web 展示。多条设备控制连接使用引用计数，全部断开后才显示离线。Relay 根据当前已验证的用户聚合限速；未登录端点使用游客限速。用户登录不授予设备访问权。

## Build / 构建

```sh
cd apps/web
npm ci
cd ../..
python scripts/build.py server --profile release --package
# Native Windows/Linux host: the archive format follows the host platform.
```

The independent archive contains both binaries, `web/`, this guide and an adjacent SHA-256 manifest. It never includes `.env`, TLS keys, database credentials or test fixtures. Existing Desktop installer names are unchanged.

The unified build allocates one product version for Server, Relay and Web; see [BUILDING.md](BUILDING.md). Repackaging existing verified artifacts keeps their original version.

Docker builds the same production Web sources in a Node 22 build stage and places the assets beside the Server executable. See [Docker guide](packaging/docker/README.md). Configure a distinct stable `node_id` in Relay TOML for each Relay; the default is `primary`. Legacy Relay versions that do not report a node ID do not appear in the node table.

## Configure / 配置

Each server executable reads its own TOML; see [configuration templates and Compose setup](packaging/docker/README.md). Use `--config <path>` for all Server commands. Configure the database, TLS and Relay control secret. `pab-server init` applies migrations and initializes the singleton server settings. `pab-server serve` also applies outstanding migrations before serving. Server URLs select the environment; no deployment UUID is configured or transmitted.

### Current schema / 当前结构

Development targets the new configuration and data structures only. The initial PostgreSQL schema creates `server_settings` directly, without a deployment UUID. Client databases and device references use the current fields; there is no conversion of old device-reference keys or credential columns.

Use matching Server, Relay, Desktop, MCP and Executor builds. For this development baseline, initialize a fresh PostgreSQL database and fresh client data directories with the control/Relay URLs and existing TLS/authentication settings. Databases created by the previous baseline are not supported, including their migration checksums. Back up any data you need before replacing an environment; recreating it issues new device identities/codes and passwords. No compatibility or automatic data migration is provided.

统一按新配置、新数据结构开发，不再兼容含 deployment ID 的旧结构。PostgreSQL 直接创建 `server_settings`；客户端不再转换旧设备引用或凭据表。部署时使用配套版本、新数据库和新客户端数据目录。需要的数据应提前备份；重新初始化会生成新的设备身份、设备码和密码。

| Setting | Behavior |
|---|---|
| `PAB_DB_NAME` | Compose database name; defaults to `pab`. Create a fresh database before initializing the new development baseline when reusing an existing PostgreSQL volume. Delete the explicitly identified old development database after the switch; legacy data migration is not supported. |
| `web.assets` | Built Web assets path in Server TOML; relative paths resolve against the TOML directory |
| `web.origin` | Required **public HTTPS origin**, e.g. `https://bridge.example.com` |
| `web.registration_enabled` | Existing registration switch; reflected by the Web login page |
| `node_id` in Relay TOML | Relay process setting; stable ASCII letters, digits, `_`, `-`, `.`; at most64 characters |

Origin must contain only scheme, host and optional port. Proxy HTTPS and WebSocket upgrades on the same public origin, preserve `Host`, and do not cache `/api/`. Never expose the application through plain HTTP: its session cookie is Secure + HttpOnly + SameSite=Strict. No browser credentials are stored in localStorage. Server sessions do not expire; explicit logout revokes the current session. Password changes preserve sessions, and disabling an account blocks access without deleting them. Browser cookies have a 400-day retention period refreshed by the current-session endpoint. Reverse proxies must allow the `/api/web/events` WebSocket and the existing device/Relay control paths.

An [Nginx configuration example](packaging/server/nginx.conf.example) keeps upstream certificate verification enabled and forwards Web and control WebSockets. Replace the public domain, certificate paths and backend certificate name; set `web.origin` to that exact public HTTPS origin. Run `nginx -t` before reload. This handles the Server HTTPS endpoint; keep the existing separate Relay HTTPS and UDP configuration.

已在隔离 Nginx 容器中验证 HTTPS → HTTPS、Secure/HttpOnly/SameSite Cookie、页面刷新、静态缓存、`/api/web/events` 和 `/control` Upgrade。此结果不等同于正式域名证书、生产网络和现网代理配置已经验收。

浏览器注册普通账号后，在服务端本机明确指定管理员：

```sh
pab-server web-admin existing-username
```

For Compose, run this command in the backend container with its existing environment. Registration never automatically grants administrator access. With registration disabled, provision an existing account through the established control registration flow before disabling registration. At least one active administrator is preserved by Web edits.

管理员在“设备列表”和“在线设备”直接查看、管理全站设备，无需认领或 Desktop 确认。设备归属统计/筛选/列/详情及解绑已移除。“我的设备”与“全部设备”已合并为 `/devices`；旧 `/all-devices` 链接保留筛选参数并跳转。普通账号的查询仍受原有权限过滤，不因入口合并获得全站权限。认领页面、HTTP 接口、Desktop 弹窗/轮询及 CLI 已移除；旧控制协议认领消息返回明确错误。开发阶段采用全新初始数据库，不迁移旧数据。旧解绑接口返回404，不改变设备数据。旧owner筛选API参数返回400，浏览器自动清除旧URL中的owner参数及旧页码。不提供团队、成员及团队流量归集。

## Development database reset / 开发阶段数据库重建

1. Stop the old Server and clients before switching to this account release. Remove the explicitly selected old development database and create an empty database; no legacy migration or compatibility layer is provided.
2. Deploy matching Server, Web, Relay, Executor and Desktop/MCP builds. Run `pab-server init`, register users and devices again and promote the administrator explicitly. Clear the old client data when switching the development baseline.
3. Verify `/health`, `/api/web/config`, HTTP registration/login/logout, device visibility and online status, MCP account synchronization and Relay policy acknowledgments. Normal subsequent restarts preserve the new database and permanent sessions.
4. Do not mix new binaries with old schemas or edit `_sqlx_migrations` to bypass a checksum error. Only reset the named development/deployment database, never unrelated database volumes.

在线状态来自当前 Server 内存，重启后由设备重新连接恢复。Relay 节点健康同时绑定 Server 实例和120秒上报时限，避免重启后错误沿用旧在线状态。该状态不表示数据传输路径或吞吐质量。

## Tests / 测试

Use an isolated PostgreSQL database through `DATABASE_URL`; never point integration tests at production. Full plan and measured results: [WEB_DEVELOPMENT.md](WEB_DEVELOPMENT.md).

```sh
cargo test -p pab-server --test web_management
cargo test -p pab-server --test control_tls --test guest_registration
cd apps/web
npm test
npm run build:assets
npm run test:e2e
```

Browser test prerequisites: local HTTPS Server at `https://localhost:38443`, isolated database container `pab-web-test-20261003` / database `pab_account_test`, and Chrome. Build `cargo build -p pab-server --example web-device-fixture` and provide `.build/web-test/cert.pem` to its loopback-only device simulator. `PAB_WEB_TEST_URL` changes the session smoke-test origin; full management fixtures intentionally use the documented fixed local environment and must not target a production URL.

Reproducible local setup (test credentials only):

```sh
docker run -d --name pab-web-test-20261003 -p 127.0.0.1:55435:5432 -e POSTGRES_HOST_AUTH_METHOD=trust -e POSTGRES_DB=pab_account_test postgres:18.6
python -m pip install cryptography
python apps/web/testing/serve.py
# Leave that terminal running; in another terminal:
cd apps/web
npm run test:e2e
```

Build Web first using the earlier commands. Set `DATABASE_URL=postgres://postgres@127.0.0.1:55435/pab_account_test` for Rust tests; SQLx creates separate test databases. The launcher ignores production Relay secret-file settings and copies the executable to `.build/` so Windows rebuilds can continue. Stop the launcher with Ctrl+C. These fixed test container names and ports are deliberate safeguards; do not substitute a production instance.

For Vite development use locally supplied `PAB_WEB_DEV_CERT`, `PAB_WEB_DEV_KEY`, optional `PAB_WEB_BACKEND` and matching Server TOML `web.origin = "https://localhost:1440"`. Explicit `PAB_WEB_DEV_SELF_SIGNED=1` is allowed for isolated development only. Production TLS validation must remain enabled.

## HTTP accounts and fresh database (2026-10-08)

The account release replaces the development schema with `0001_initial.sql`. Stop the old Server before removing its development database and creating an empty one. Do not run the new binary against an old schema or mark old migrations as applied. This intentionally discards old accounts, sessions, device registrations and management history. Register accounts and devices again and initialize the administrator explicitly. No compatibility migration is provided.

Browser accounts use `/api/web/register`, `/api/web/session`, `/api/web/logout`; native Desktop/headless clients use `/api/account/register`, `/api/account/session`, `/api/account/logout`. Native responses return a bearer token; browser responses use an HttpOnly secure cookie. Server sessions have no expiration or concurrent-session limit; logout revokes only that session. A password change does not log out other devices. Browser cookie lifetime is capped by browsers and refreshed when retrieving the session.

An MCP signs `/api/account/endpoint-context` with its own endpoint key. The Server derives the user from the bearer session and broadcasts policy changes. Device passwords and endpoint permissions remain independent. Desktop's unauthenticated `0.0.0.0:26035` WebSocket sends only account revision hints; it never sends bearer credentials. MCPs read the current OS user's protected credential store. Windows uses Credential Manager, macOS uses Keychain through the system `/usr/bin/security` client (private stdin/stdout pipes, no credentials in argv), and headless Linux uses a user-only file (directory 0700, file 0600).

Relay defaults are `default_user_mbps` and `default_guest_mbps`; administrators can override `users.relay_limit_mbps` through the Accounts page. All endpoints of a user share one bucket per Relay node. There is no distributed quota across independent Relay nodes. MCP details distinguish the Server's accepted revision from the Relay node's actually applied policy version.

Deploy matching Server, Relay, Executor and Desktop/MCP binaries together: relay policy schema 5, device authentication schema 2 and device task schema 2. Local task records remain local. This source change has not itself been deployed or packaged.
