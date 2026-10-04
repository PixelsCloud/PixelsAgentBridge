# Server Web deployment / 部署与验证

## Scope / 范围

The React + Ant Design console is served by `pab-server` on its existing HTTPS listener. No Node runtime is required in production. Server stores account, device, Team and Relay management data only; remote task history, command output, files and screenshots remain local to Bridge/Executor. Device **online** means an authenticated device control connection is alive. This release supports one active control Server per deployment.

Web 与现有 `/control`、`/relay-control` 共用 HTTPS 端口，不新增公开端口。任务记录只保存在本地，不上传 Server，也不在 Web 展示。多条设备控制连接使用引用计数，全部断开后才显示离线。Team 仅用于 Relay 流量归属，不授予成员设备访问权。

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

Docker builds the same production Web sources in a Node 22 build stage and places the assets beside the Server executable. See [Docker guide](packaging/docker/README.md). Configure a distinct stable `PAB_RELAY_NODE_ID` for each Relay; the default is `primary`. Legacy Relay versions that do not report a node ID do not appear in the node table.

## Configure / 配置

Configure the database, TLS and Relay control secret. `pab-server init` applies migrations and initializes the singleton server settings. `pab-server serve` also applies outstanding migrations before serving. Server URLs select the environment; no deployment UUID is configured or transmitted.

### Current schema / 当前结构

Development targets the new configuration and data structures only. The initial PostgreSQL schema creates `server_settings` directly, without a deployment UUID. Client databases and device references use the current fields; there is no conversion of old device-reference keys or credential columns.

Use matching Server, Relay, Desktop, MCP and Executor builds. For this development baseline, initialize a fresh PostgreSQL database and fresh client data directories with the control/Relay URLs and existing TLS/authentication settings. Databases created by the previous baseline are not supported, including their migration checksums. Back up any data you need before replacing an environment; recreating it issues new device identities/codes and passwords. No compatibility or automatic data migration is provided.

统一按新配置、新数据结构开发，不再兼容含 deployment ID 的旧结构。PostgreSQL 直接创建 `server_settings`；客户端不再转换旧设备引用或凭据表。部署时使用配套版本、新数据库和新客户端数据目录。需要的数据应提前备份；重新初始化会生成新的设备身份、设备码和密码。

| Setting | Behavior |
|---|---|
| `PAB_DB_NAME` | Compose database name; defaults to `pab`. A distinct name allows a fresh baseline while retaining the previous database for rollback; create that database first if reusing an existing PostgreSQL volume. |
| `PAB_WEB_DIR` | Optional absolute directory of built Web assets; defaults to `web` beside the executable |
| `PAB_WEB_ORIGIN` | Optional **public HTTPS origin**, e.g. `https://bridge.example.com`; set explicitly behind a reverse proxy |
| `PAB_REGISTRATION_ENABLED` | Existing registration switch; reflected by the Web login page |
| `PAB_RELAY_NODE_ID` | Relay process setting; stable ASCII letters, digits, `_`, `-`, `.`; at most64 characters |

Origin must contain only scheme, host and optional port. Proxy HTTPS and WebSocket upgrades on the same public origin, preserve `Host`, and do not cache `/api/`. Never expose the application through plain HTTP: its session cookie is Secure + HttpOnly + SameSite=Strict. No browser credentials are stored in localStorage. Sessions last12 hours; password, account status and role changes revoke existing sessions. Reverse proxies must allow the `/api/web/events` WebSocket and the existing device/Relay control paths.

An [Nginx configuration example](packaging/server/nginx.conf.example) keeps upstream certificate verification enabled and forwards Web and control WebSockets. Replace the public domain, certificate paths and backend certificate name; set `PAB_WEB_ORIGIN` to that exact public HTTPS origin. Run `nginx -t` before reload. This handles the Server HTTPS endpoint; keep the existing separate Relay HTTPS and UDP configuration.

已在隔离 Nginx 容器中验证 HTTPS → HTTPS、Secure/HttpOnly/SameSite Cookie、页面刷新、静态缓存、`/api/web/events` 和 `/control` Upgrade。此结果不等同于正式域名证书、生产网络和现网代理配置已经验收。

浏览器注册普通账号后，在服务端本机明确指定管理员：

```sh
pab-server web-admin existing-username
```

For Compose, run this command in the backend container with its existing environment. Registration never automatically grants administrator access. With registration disabled, provision an existing account through the established control registration flow before disabling registration. At least one active administrator is preserved by Web edits.

管理员在“设备列表”和“在线设备”直接查看、管理全站设备，无需认领或 Desktop 确认。设备归属统计/筛选/列/详情及解绑已移除。“我的设备”与“全部设备”已合并为 `/devices`；旧 `/all-devices` 链接保留筛选参数并跳转。普通账号的查询仍受原有权限过滤，不因入口合并获得全站权限。认领页面、HTTP 接口、Desktop 弹窗/轮询及 CLI 已移除；旧控制协议认领消息返回明确错误。迁移16取消存量待处理申请，保留现有设备身份、归属及历史管理记录。旧解绑接口返回404，不改变设备数据。旧owner筛选API参数返回400，浏览器自动清除旧URL中的owner参数及旧页码。已有底层权限关系与历史记录保留，Team流量归集继续独立工作。

## Upgrade and rollback / 升级与回滚

1. Back up PostgreSQL with `pg_dump` and record the current binaries/image, configuration. Check the backup can be restored into a separate database.
2. Deploy the new binary **together with its matching `web/`**. When switching from the deployment-ID baseline, use a fresh database and run `pab-server init`, with new client data directories and matching client builds as described above. For subsequent versions sharing this baseline, normal `init`/`serve` applies pending migrations. Do not reuse the previous baseline's database or modify its migration checksums.
3. Start Server and Relay. Verify `/health`, `/api/web/config`, login, device visibility, live device status and SPA deep links. Promote the initial administrator explicitly when needed.
4. For rollback, stop the new backend and restore the pre-upgrade database backup plus matching old binaries/image. Do not edit `_sqlx_migrations`, remove Docker database volumes or pretend an older binary can safely downgrade the schema.

在线状态来自当前 Server 内存，重启后由设备重新连接恢复。Relay 节点健康同时绑定 Server 实例和120秒上报时限，避免重启后错误沿用旧在线状态。该状态不表示数据传输路径或吞吐质量。

## Tests / 测试

Use an isolated PostgreSQL database through `DATABASE_URL`; never point integration tests at production. Full plan and measured results: [WEB_DEVELOPMENT.md](WEB_DEVELOPMENT.md).

```sh
cargo test -p pab-server --test web_management
cargo test -p pab-server --test control_tls --test guest_registration
cd apps/web
npm test
npm run build
npm run test:e2e
```

Browser test prerequisites: local HTTPS Server at `https://localhost:38443`, isolated database container `pab-web-test-20261003` / database `pab_web_test`, and Chrome. Build `cargo build -p pab-server --example web-device-fixture` and provide `.build/web-test/cert.pem` to its loopback-only device simulator. `PAB_WEB_TEST_URL` changes the session smoke-test origin; full management fixtures intentionally use the documented fixed local environment and must not target a production URL.

Reproducible local setup (test credentials only):

```sh
docker run -d --name pab-web-test-20261003 -p 127.0.0.1:55435:5432 -e POSTGRES_HOST_AUTH_METHOD=trust -e POSTGRES_DB=pab_web_test postgres:18.6
python -m pip install cryptography
python apps/web/testing/serve.py
# Leave that terminal running; in another terminal:
cd apps/web
npm run test:e2e
```

Build Web first using the earlier commands. Set `DATABASE_URL=postgres://postgres@127.0.0.1:55435/pab_web_test` for Rust tests; SQLx creates separate test databases. The launcher ignores production Relay secret-file settings and copies the executable to `.build/` so Windows rebuilds can continue. Stop the launcher with Ctrl+C. These fixed test container names and ports are deliberate safeguards; do not substitute a production instance.

For Vite development use locally supplied `PAB_WEB_DEV_CERT`, `PAB_WEB_DEV_KEY`, optional `PAB_WEB_BACKEND` and matching backend `PAB_WEB_ORIGIN=https://localhost:1440`. Explicit `PAB_WEB_DEV_SELF_SIGNED=1` is allowed for isolated development only. Production TLS validation must remain enabled.
