# GitHub 登录实施与验收

状态：主要代码已实现，本地自动测试通过；真实 GitHub 授权、部署和远端实机验收尚未完成。私有 App 配置已提供，未输出或提交密钥。沿用 USER_ACCOUNT_PLAN.md 第 14 节。

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
- Desktop、Web 前端构建通过；Linux Server/Relay Release Docker 镜像 1.2.66 构建成功，尚未部署。
- 修复重命名弹窗误用详情面板草稿，保存按钮现在使用弹窗自己的输入值。标题读取打包版本，显示 `Pixels Agent Bridge (v 版本号)`。
- 本机安装未替换，也未停止 Desktop、Executor 或 MCP。当前 Pixels MCP 返回 `Transport closed`，远端 Windows/macOS 验收需恢复当前会话的 MCP 连接后继续。
- 未完成：真实 GitHub 登录/绑定授权、macOS 新代码编译与实机验收、登录后的多 MCP 热更新实测；网络超时、上游限流和超大响应尚未逐项注入测试。头像元数据已存储，页面展示尚未接入。
