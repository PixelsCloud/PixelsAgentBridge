# 浏览器工具

Pixels 只操作自己启动的浏览器窗口，启动时默认最大化。目标设备已安装 Chrome 时使用 Chrome，否则使用 Edge；两者都没有时返回 `browser_missing`。不连接用户日常浏览器，也不安装或使用 Playwright 扩展。目标设备必须有可用的交互桌面用户；发起方必须登录 Pixels。

每个目标系统用户、Pixels 账号和浏览器种类使用独立的持久资料目录，位于该系统用户的 Pixels 数据目录下。网站 Cookie 等浏览器资料留在目标设备，不上传 Pixels Server。关闭任务和升级应用不会主动删除资料；用户可在设置中清除当前账号选定浏览器的资料，使用中的资料会拒绝清除。部分网站只发浏览器关闭即失效的会话 Cookie，或会主动让登录状态过期，这些站点需要重新登录。Chrome 切换到 Edge 时使用另一份资料，不复制 Cookie。

Agent 连接目标设备后，调用 `pab_list_execution_contexts` 选择目标交互用户，并带 `execution.mode=desktop_user` 和 `context_ref` 调用 `pab_browser_attach`。无需传模式或配置目录。`pab_browser_setup_info` 仅检查运行时及 Chrome/Edge 是否可用。工具会话绑定目标设备、Pixels 账号、发起 MCP 操作者及目标系统用户。有副作用的调用可省略 `request_id`，MCP 会自动生成 UUID 并在 `operation_ref` 返回；需要安全重试时可显式传入稳定的 UUID。遇到 `unconfirmed` 时先用原 `operation_ref` 查询，不能用新 ID 重复提交。

网站要求登录、扫码或二次验证时，Agent 调用 `pab_browser_request_handoff` 暂停浏览器工具，并提示用户在目标设备上 **Pixels 打开的浏览器窗口**完成操作。目标 Desktop 显示待处理提示；Agent 等待并查询 `pab_browser_get_session`，以弹窗结果为准，不再要求用户在聊天里重复选择或回复“已完成”。返回 `ready_to_resume` 时调用 `pab_browser_resume`，再检查网站是否真的已经登录；确认按钮本身不证明登录成功。登录等待最长 1 小时；点击取消后还需二次确认，选择继续等待不会终止会话。确认取消时 `pab_browser_get_session` 返回 `cancelled`，Agent 应结束该任务；超时后会话自动关闭。等待期间浏览器工具和 Pixels 的受控桌面截图/输入入口暂停；通用系统命令不受这一浏览器等待状态完全隔离。首版需要用户能操作目标设备，远程登录接管窗口留到后续版本。不要向 Agent 提供密码、验证码或二维码内容。

`pab_browser_downloads` 只返回该会话中 Playwright 明确报告的文件；可以传准确文件名筛选。上传目标设备文件前，用现有文件工具将所需文件放到目标设备。增强工具另提供截图坐标操作、拖放、控制台消息、网络请求摘要及 PDF 保存。

正式安装包应包含固定版本的 Node.js 和 Playwright MCP 运行时，不要求目标机器执行 npm，也不下载浏览器扩展。浏览器工具验收使用 `acceptance/fixtures/browser-extension-smoke/` 中的本地测试站点，保持目标设备日常浏览器资料不变；安装与升级验收在专用测试机执行。
