# Pixels 浏览器验收站点

当前产品只启动 Pixels 专属 Chrome/Edge，并用独立资料目录保存网站状态；不使用扩展或日常浏览器资料。本目录的站点只监听 `127.0.0.1:17841`，提供模拟账号、表单、弹窗、上传、下载、拖放和导航。模拟账号 `demo/demo` 仅用于本地站点；“Continue as demo (test only)”可无密码测试人工接管和持久登录。

在构建好 `browser-runtime` 后，分别以 Chrome 和 Edge 运行：

```text
node acceptance/fixtures/browser-extension-smoke/pixels-persistent-profile-smoke.mjs <pinned-node> <playwright-mcp-cli> <browser-executable> <chrome|msedge>
node acceptance/fixtures/browser-extension-smoke/pixels-browser-tool-smoke.mjs <pinned-node> <playwright-mcp-cli> <browser-executable> <chrome|msedge>
```

第一个脚本验证持久资料登录后关闭、重开仍可登录；第二个验证实际浏览器工具，包括表单、弹窗、上传下载、截图、标签页、拖放、网页日志和 PDF。脚本各自创建并清理专用临时资料，不接触真实网站账号或日常浏览器。

远程正式路由用 `pixels-route-smoke.mjs`。先在**目标设备**的本地回环地址启动 `acceptance-server.mjs`，再在发起设备设置 `PAB_MCP_EXE`、`PAB_TEST_CODE` 和正常的 Pixels MCP 账号/服务配置，运行脚本。该脚本会通过 Pixels MCP 验证连接、浏览器点击和登录持久性；目标 Executor 和 Desktop 必须是包含浏览器能力的新版本。不要在仓库内写入真实设备码或登录资料。

目录中仍有早期扩展可行性实验的文件。它们不参与当前产品运行或安装包构建，也不能代替正式 Pixels MCP 路由验收。
