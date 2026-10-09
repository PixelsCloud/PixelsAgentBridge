# 配置 GitHub 登录

GitHub 登录已实现。用户点击「使用 GitHub 登录」并授权后即可进入：首次使用会自动创建普通账号，不需要另外填写注册表或设置密码；再次登录会进入同一账号。已有密码账号可在「我的」中主动绑定 GitHub。

公开服务已部署 GitHub 登录入口；真实用户授权与 Desktop 实机验收进度见 [验收记录](acceptance/github-login-20261009.md)。以下是独立部署的配置步骤。

1. 登录管理项目的 GitHub 账号，打开 <https://github.com/settings/apps/new>。
2. 按下表填写，其余权限保持默认不申请。

| 字段 | 内容 |
| --- | --- |
| GitHub App name | Pixels Agent Bridge；如重名，添加账号名后缀 |
| Homepage URL | `https://pab.rgaa.vip` |
| Callback URL | `https://pab.rgaa.vip/api/account/github/callback` |
| Webhook → Active | 取消勾选 |
| Repository / Organization permissions | 无需申请 |
| Where can this GitHub App be installed? | Any account |

3. 点击 Create GitHub App。记下 **Client ID**，不是 App ID。
4. 在左侧 **Credentials** 页面找到 **Client secrets**，点击 **Generate a new client secret**。不要点 Generate a private key；下载的 PEM 私钥不用于这里的登录流程。
5. 将下列 JSON 保存到仓库的 `.env/github-app.json`，替换占位符。该目录属于本机私有配置，不提交 Git，也不要将 Secret 发到聊天中。

```json
{
  "client_id": "填入 Client ID",
  "client_secret": "填入刚生成的 Client Secret"
}
```

无需生成 Private Key，无需安装到某个代码仓库。这里使用 GitHub 用户授权完成登录，不访问项目代码。自己的独立部署应改填自己的站点和回调地址。

## 网络出口

浏览器打开 GitHub 后，服务端仍需通过 HTTPS 请求 `github.com/login/oauth/access_token` 和 `api.github.com/user`。浏览器跳转成功不能证明服务端网络正常。部署时应从 **Backend 容器** 验证这两个域名的连通性，不能用固定 GitHub IP 或关闭 TLS 校验解决线路问题。

默认使用正常域名解析和 HTTPS。需要专用出口时，在私有环境配置中设置 `PAB_GITHUB_PROXY_URL`，并通过 `compose.github.yaml` 注入 Backend，例如 `socks5h://private-proxy.example:1080`。支持 HTTP、HTTPS CONNECT 和 SOCKS5H 代理；SOCKS5H 在代理端解析域名。该配置只用于 GitHub HTTP 客户端，不改变 Relay、设备控制、浏览器或 Desktop 的网络路径；不设置时也可使用 reqwest 支持的标准代理环境变量。

可复用已有的受管理代理，或使用 OpenSSH 动态转发；示例见 [GitHub 出口运维](packaging/network/README.md)。出口应限制在私网并具备开机启动、断线恢复和健康监测。代理地址如含凭据，应保留在私有配置，不提交 Git。

建连超时为 5 秒，单次请求总超时为 15 秒。只对尚未发送 HTTP 请求的连接错误重试一次；收到响应、读取失败或响应超时后不自动重放一次性授权码。服务端日志记录 token/profile 阶段、连接/超时/配置/授权码错误分类及 HTTP 状态，不记录授权码、令牌、Secret、完整 URL 或上游错误描述。GitHub 返回 HTTP 200 的 OAuth 错误也按错误处理。失败后应重新点击登录，不刷新旧回调。

Client ID 与 Client Secret 都只保留在私有配置。开源仓库、测试数据及日志不包含部署者的实际 Client ID 或密钥。服务端通过 `PAB_GITHUB_CONFIG_FILE` 指定该 JSON 文件，`PAB_WEB_ORIGIN` 指定本站 HTTPS 地址；不配置时隐藏入口。

服务端读取私有配置并完成授权码交换；Desktop 和 MCP 不接收 GitHub Secret。部署时需挂载配置文件并设置上述环境变量；真实授权验收状态见验收记录。

Docker Compose 独立部署可使用可选的 `packaging/docker/compose.github.yaml`：在私有环境文件中设置 `PAB_GITHUB_CONFIG_PATH` 为 JSON 的绝对路径、`PAB_WEB_ORIGIN` 为自己的 HTTPS 站点地址，并启用 `PAB_REGISTRATION_ENABLED=true` 允许新用户首次登录。启动时同时加载配置：

```sh
docker compose --env-file private.env -f compose.yaml -f compose.github.yaml up -d
```

请让容器中的 `pab` 用户能够读取该配置文件，并限制其他用户访问；文件不进入镜像。后续更新同样加载这两个 Compose 文件。未启用 GitHub 的部署只加载原来的 `compose.yaml` 即可。

数据库新增迁移 5，升级前先备份。升级后若需暂时关闭 GitHub，可移除 GitHub 配置并继续使用新版本服务端；不要直接把旧版二进制运行在已升级数据库上，也不要用旧备份覆盖正在使用的数据。

参考：[GitHub 创建 App](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/registering-a-github-app)、[用户授权流程](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app)。完整开发范围见 `USER_ACCOUNT_PLAN.md` 第 14 节。
