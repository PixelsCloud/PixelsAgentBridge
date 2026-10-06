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

上表为首批证据；传输 UI 的后续验证见下文。真实凭据、并发/生命周期矩阵、三平台安装与正式 MCP 宿主验收仍待完成；本批没有安装或重新打包。

## 后续：传输 UI 接入

- 从 MCP 提取共用的持久化传输流程，Desktop 复用同一套实际身份回执、取消发布判定和原记录核对，没有另写一套二进制传输协议。
- UI 在调用前生成原 request ID；提交结果丢失保留 ID 供查询。后端固定设备、Runtime、所有者队列，切换账号后的取消/观察不转向新 Runtime。
- 已取消、已完成与结果待确认分开显示；待确认和取消请求均禁止自动再次传输。晚到的调用回执不会覆盖已收到的完成事件。
- 显式用户选择传入 FileTransferOptions，历史及进度展示实际身份。不存在实际回执时不把所选用户当作已执行用户。
- 共用绝对路径校验；限制 Desktop 同时运行 8 个任务、至多保留 128 个未解决所有者。只淘汰已确认终态记录，结果仍保留于 SQLite 历史。
- 中英文 README 已补充 UI 操作说明。

| 后续验证 | 结果 |
|---|---|
| Windows Bridge / MCP | 69 / 38 全部通过 |
| Mac ARM Bridge / MCP | 69 / 38 全部通过；任务 74c10dff-1e2f-49c7-895a-b2d2047991e1，退出码 0 |
| Linux 无 GUI 容器 Bridge / MCP | 69 / 38 全部通过 |
| Windows / Mac Desktop `cargo check --tests` | 通过 |
| TypeScript / Vite 生产资源构建 | 通过，未递增产品版本 |
| Playwright/Edge | 11/11 通过，新增用户传输、原 ID 观察、取消/完成与晚到回执测试 |

新增 Rust 测试使用真实 SQLite 持久化与 TransferControl，模拟响应丢失，校验重开后的终态/未确认状态和禁止重复派发，并验证初始化等待中可取消；不是公网丢包实测。
浏览器仍使用 Tauri 夹具；安装版双桌面实操、真实网络断开和多账号并发留在 E6–E8。
