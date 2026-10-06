# E5：执行身份与应用面板首批验证

本报告是源码与浏览器验证，不代表安装后的正式宿主验收。产品版本仍为 1.2.26。

## 本批改动

- 命令、目录、终端可选择已发现的远程执行用户；默认 service 明确保留。失效的显式选择禁用提交，不回退 service。
- Windows/macOS 应用面板显式选择桌面会话，复用已有查询、启动、打开文件协议；原操作结果未确认时只能观察原 ID，不自动重放。
- 终端、命令、文件/Git/传输历史展示已观察到的实际执行身份；旧记录显示“未记录”，不把请求账户当作实际身份。
- 终端身份写入 SQLite 并校验不可替换；设备切换后才返回的终端关闭原会话，避免遗留。终端 resize 去重并移到动画帧，修复 ResizeObserver 循环。
- 三语、亮暗主题沿用 React + Ant Design。Mac 也显示已有窗口功能入口。

## 证据

| 范围 | 结果 |
|---|---|
| Windows Bridge 单元/数据库/协议测试 | 68/68，通过；包括实际身份持久化、重开和多种历史查询 |
| macOS ARM Bridge 测试 | 68/68，通过；任务 42b3cba3-f141-40f9-8089-349da8c42067，退出码 0 |
| Linux 无 GUI 容器 Bridge 测试 | 68/68，通过 |
| Windows、macOS Desktop `cargo check --tests` | 通过 |
| Windows、Linux `cargo check --workspace --tests` | 通过 |
| TypeScript 与 `npm run build:assets` | 通过；保留已有 Vite chunk 大小提示 |
| Playwright/Windows Edge | 9/9，通过：失效选择、原 ID 观察、提交前拒绝、中文文件路径、晚到终端清理、三语 × 两主题布局 |

浏览器测试使用真实 React 面板与确定性 Tauri 回执夹具，不操作真实桌面。运行：
`cd apps/desktop; npm run test:execution-ui`。Windows 使用已安装的 Edge；其他构建机先执行 `npx playwright install chromium`。

## 未完成范围

文件传输 UI 仍需接入现有异步队列与执行用户选择，保留原连接的取消和结果核对。随后继续真实凭据、并发/生命周期矩阵、三平台安装与正式 MCP 宿主验收；本批没有安装或重新打包。
