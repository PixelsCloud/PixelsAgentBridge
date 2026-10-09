# 配置 GitHub 登录

GitHub 登录主要代码已实现，真实授权和部署验收仍待完成。本页用于创建项目自己的 GitHub App；仅创建 App 不会立即启用登录。

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

Client ID 与 Client Secret 都只保留在私有配置。开源仓库、测试数据及日志不包含部署者的实际 Client ID 或密钥。服务端通过 `PAB_GITHUB_CONFIG_FILE` 指定该 JSON 文件，`PAB_WEB_ORIGIN` 指定本站 HTTPS 地址；不配置时隐藏入口。

服务端读取私有配置并完成授权码交换；Desktop 和 MCP 不接收 GitHub Secret。部署时需挂载配置文件并设置上述环境变量；真实授权验收状态见验收记录。

参考：[GitHub 创建 App](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/registering-a-github-app)、[用户授权流程](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app)。完整开发范围见 `USER_ACCOUNT_PLAN.md` 第 14 节。
