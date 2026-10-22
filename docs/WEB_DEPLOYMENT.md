# Server 与 Web 部署

Server 提供账号、设备管理、汇总用量与 Relay 管理服务，Web 静态资源由 Server 的
HTTPS 监听器提供，生产环境无需 Node.js。设备控制和 Relay 控制使用同一服务上的
WebSocket 路径。任务输出、操作记录、文件和截图保留在客户端，不上传管理后台。

## 构建与安装

在仓库根目录执行：

```sh
npm --prefix apps/web ci
python scripts/build.py server --profile release --package
```

归档包含 Server、Relay、健康检查程序、Web 资源、部署模板和许可声明，并生成校验清单。
构建规则见 [BUILDING.md](../BUILDING.md)，容器部署见
[Docker Compose 说明](../packaging/docker/README.md)。私有配置和密钥不进入归档或镜像。

## 配置与首次启动

按部署模板为 Server 和每个 Relay 创建独立 TOML 配置。配置数据库、HTTPS 证书、
控制服务地址和 Relay 认证信息；每个 Relay 使用稳定且唯一的 `node_id`。
密钥与实际配置保存在受限的部署目录，不写入仓库或公共示例。
配置字段见 [配置参考](CONFIGURATION.md)。

```sh
pab-server check --config /etc/pab/pab-server.toml
pab-server init --config /etc/pab/pab-server.toml
pab-server serve --config /etc/pab/pab-server.toml
```

首次部署使用独立 PostgreSQL 数据库。`init` 初始化服务设置并应用数据库迁移；
`serve` 也会检查并应用待执行迁移。已有部署按下方升级流程保留数据，不能照首次部署
步骤删除数据库或客户端身份。

浏览器注册普通账号后，在服务端明确提升已有账号：

```sh
pab-server web-admin existing-username --config /etc/pab/pab-server.toml
```

普通注册不会自动获得管理员权限。可选的外部登录见
[GitHub 登录配置](GITHUB_LOGIN_SETUP.md)。

## HTTPS 与代理

`web.origin` 必须是实际的公共 HTTPS Origin，例如 `https://bridge.example.com`，
仅包含协议、主机和可选端口。`web.assets` 指向 Web 构建目录，配置中的相对路径
以 TOML 所在目录为基准。域名、证书名称和代理目标必须一致。

代理保留 `Host`，支持 `/api/web/events`、`/control`、`/relay-control` 的
WebSocket Upgrade，不缓存 `/api/`。浏览器会话使用安全 Cookie；代理到后端的
证书校验保持开启。参考 [Nginx 模板](../packaging/server/nginx.conf.example)。
Relay 的 HTTPS 与 UDP 服务另外按节点配置开放。

## 升级与回退

1. 备份数据库、私有配置和 Relay 的持久化队列，记录正在运行的版本。
2. 使用兼容的 Server、Relay 和客户端版本。升级 Server 后逐个处理 Relay；
   节点维护、恢复及滚动升级见 [Relay 运维](RELAY_CLUSTER_OPERATIONS.md)。
3. 保留数据卷、设备身份和凭据；不要清库、删除迁移记录或修改迁移校验值来绕过错误。
4. 旧程序可能无法读取升级后的数据库。回退前在独立数据库恢复升级前备份并验证，
   保留当前数据库及升级后的新增数据，不直接覆盖现网数据。

配置修改后重建受影响的容器或重启服务，使新配置生效。实际私钥、共享密钥、
数据库口令与 OAuth Secret 通过受限的本地配置提供，不在日志中输出。

## 部署验证

检查 `/health`、登录与退出、设备重新上线、Relay 策略同步，再执行一次实际设备连接
和任务调用。健康端点成功不等同于完整转发链路已经通过。
设备在线统计表示控制连接状态；Relay 详情中的接入连接与正在转发的业务链路应分别理解。

自动化测试使用隔离数据库和测试账号，入口见
[开发与检查约定](../BUILDING.md#development)。不要把测试脚本指向生产环境。
