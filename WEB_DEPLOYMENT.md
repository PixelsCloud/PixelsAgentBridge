# Server Web deployment / 部署与验证

## Scope / 范围

The React + Ant Design console is served by `pab-server` on its existing HTTPS listener. No Node runtime is required in production. Server stores account, ownership, Team and Relay management data only; remote task history, command output, files and screenshots remain local to Bridge/Executor. Device **online** means an authenticated device control connection is alive. This release supports one active control Server per deployment.

Web 与现有 `/control`、`/relay-control` 共用 HTTPS 端口，不新增公开端口。任务记录只保存在本地，不上传 Server，也不在 Web 展示。多条设备控制连接使用引用计数，全部断开后才显示离线。Team 仅用于 Relay 流量归属，不授予成员设备访问权。

## Build / 构建

```sh
cd apps/web
npm ci
npm run build
cd ../..
cargo build --locked --release -p pab-server --bin pab-server -p pab-relay --bin pab-relay-server
python packaging/server/build.py --platform windows --profile release
# Linux: use Linux-built binaries and --platform linux.
```

The independent archive contains both binaries, `web/`, this guide and an adjacent SHA-256 manifest. It never includes `.env`, TLS keys, database credentials or test fixtures. Existing Desktop installer names are unchanged.

Docker builds the same production Web sources in a Node 22 build stage and places the assets beside the Server executable. See [Docker guide](packaging/docker/README.md). Configure a distinct stable `PAB_RELAY_NODE_ID` for each Relay; the default is `primary`. Legacy Relay versions that do not report a node ID do not appear in the node table.

## Configure / 配置

Keep the existing database, deployment UUID, TLS and Relay control secret configuration. `pab-server init` applies migrations and initializes an empty deployment. `pab-server serve` also applies outstanding migrations before serving.

| Setting | Behavior |
|---|---|
| `PAB_WEB_DIR` | Optional absolute directory of built Web assets; defaults to `web` beside the executable |
| `PAB_WEB_ORIGIN` | Optional **public HTTPS origin**, e.g. `https://bridge.example.com`; set explicitly behind a reverse proxy |
| `PAB_REGISTRATION_ENABLED` | Existing registration switch; reflected by the Web login page |
| `PAB_RELAY_NODE_ID` | Relay process setting; stable ASCII letters, digits, `_`, `-`, `.`; at most64 characters |

Origin must contain only scheme, host and optional port. Proxy HTTPS and WebSocket upgrades on the same public origin, preserve `Host`, and do not cache `/api/`. Never expose the application through plain HTTP: its session cookie is Secure + HttpOnly + SameSite=Strict. No browser credentials are stored in localStorage. Sessions last12 hours; password, account status and role changes revoke existing sessions. Reverse proxies must allow the `/api/web/events` WebSocket and the existing device/Relay control paths.

浏览器注册普通账号后，在服务端本机明确指定管理员：

```sh
pab-server web-admin existing-username
```

For Compose, run this command in the backend container with its existing environment. Registration never automatically grants administrator access. With registration disabled, provision an existing account through the established control registration flow before disabling registration. At least one active administrator is preserved by Web edits.

Web 设备认领：输入九位设备码 → 目标设备 Desktop 确认/拒绝 → Web 实时更新。申请10分钟过期，也可以主动取消。旧 Desktop 不支持待确认列表时需先升级 Desktop/Executor；原有设备连接协议不受影响。解绑仅清除归属，不删除设备身份、设备码或本地任务数据。

## Upgrade and rollback / 升级与回滚

1. Back up PostgreSQL with `pg_dump` and record the current binaries/image, configuration and deployment UUID. Check the backup can be restored into a separate database.
2. Stop the old backend, deploy the new binary **together with its matching `web/`**, then run `pab-server migrate` (or normal `init`/`serve`). New migrations13–15 add management sessions/audit, claim resolution and Relay health. Published migrations1–12 are unchanged.
3. Start Server and Relay. Verify `/health`, `/api/web/config`, login, current ownership, live device status and SPA deep links. Promote the initial administrator explicitly when needed.
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

Build Web first using the earlier commands. Set `DATABASE_URL=postgres://postgres@127.0.0.1:55435/pab_web_test` for Rust tests; SQLx creates separate test databases. The launcher ignores production deployment IDs/secret-file settings and copies the executable to `.build/` so Windows rebuilds can continue. Stop the launcher with Ctrl+C. These fixed test container names and ports are deliberate safeguards; do not substitute a production instance.

For Vite development use locally supplied `PAB_WEB_DEV_CERT`, `PAB_WEB_DEV_KEY`, optional `PAB_WEB_BACKEND` and matching backend `PAB_WEB_ORIGIN=https://localhost:1440`. Explicit `PAB_WEB_DEV_SELF_SIGNED=1` is allowed for isolated development only. Production TLS validation must remain enabled.
