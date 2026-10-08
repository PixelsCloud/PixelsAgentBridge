# 多 AI Agent MCP 接入

## 目标与范围

设置中的“连接 AI Agent”当前提供 Codex、Kimi Code、Claude Code、DeepSeek Harness、OpenCode 五个独立接入项。复用现有 stdio MCP，每个客户端启动独立进程，沿用 Desktop 状态上报；不引入 Bridge Host 或新的 MCP 网络服务。早期四/六客户端的验收记录保留在下文；Cursor 后按用户要求移除，最新变更见文末。

Windows、macOS 共用适配逻辑，覆盖安装路径含空格、GUI PATH 不完整、自定义配置目录。Linux 无界面版继续使用同一 MCP 配置方式，不增加桌面产品。

## 开发步骤

- [x] 统一客户端状态模型及 Tauri 接口；状态查询只读，各客户端错误相互隔离。
- [x] 保留 Codex 接入及 Pixels 自动批准策略，修复状态读取修改配置的问题。
- [x] Kimi Code：检测版本/配置目录，合并用户 MCP JSON 和 Pixels 权限规则。
- [x] Claude Code：用户级 MCP 注册及 Pixels 工具允许规则；尊重自定义配置目录。
- [x] DeepSeek Harness：使用官方 MCP 插件和用户配置层；验证权限、配置覆盖和工具发现。
- [x] Cursor、OpenCode：官方用户配置、客户端检测、JSONC 保留注释、独立启停与验证。
- [x] 设置 UI：六个客户端独立状态、启用/停用、刷新；简中、繁中、英文；亮暗主题。
- [x] 配置回读、MCP 握手验证、超时及冲突处理；重复操作幂等，停用只处理 Pixels。
- [x] 自动化测试、前端构建、真实客户端验证及结果记录。

## 配置原则

启用时预批准 Pixels 工具，不逐条弹审批；不将其他客户端工具全局放开。优先使用客户端官方机制，不执行配置中的任意表达式。只修改 Pixels 相关配置，保留其他服务、权限、注释和用户设置；损坏配置不覆盖，同名第三方服务不替换。写入采用同目录临时文件替换并处理冲突，修改失败不能显示成功。

“已配置”仅表示客户端配置已核对，不等于客户端当前已启动或连接；运行连接仍显示在现有 MCP 连接列表。启用后提示新建会话或重启客户端。未检测到客户端、MCP 文件缺失、配置冲突、错误分别反馈。

## 测试计划

1. 配置单元测试：全新、已有其他服务、重复启用、停用、旧安装路径、自定义目录、中文/空格路径、同名冲突、损坏文件、权限已有规则、禁用/拒绝规则。
2. 边界测试：读状态不写文件；单客户端异常不影响其他行；命令超时终止子进程；失败不报告成功；只移除本模块创建的权限条目；并发写入冲突不覆盖用户修改。
3. UI：六行独立 loading/error，刷新，启用/停用，三语言、亮暗模式、窗口宽度变化。
4. MCP：initialize、tools/list、全部工具 schema、文字/结构化结果、图片能力、长调用和取消。
5. 真实客户端：Windows/macOS 分别检查启动加载、设备查询、连接、无逐条审批、命令执行、传输、截图、停用和恢复。不可用客户端/模型/平台明确列为未验证，不以配置测试冒充端到端通过。
6. Linux 无界面：验证生成的 POSIX MCP 配置及已有启动脚本，不发布 Linux 桌面。

## 官方依据

- Kimi MCP：https://www.kimi.com/code/docs/en/kimi-code-cli/customization/mcp.html
- Kimi 配置：https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/config-files.html
- Claude MCP：https://code.claude.com/docs/en/mcp
- Claude 权限：https://code.claude.com/docs/en/permissions
- DeepSeek MCP：https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/mcp/mcp-client/README.md
- DeepSeek 配置层：https://github.com/deepseek-ai/deepseek-harness/blob/master/apps/cli/README.md

## 验证记录

实现与已执行验证完成。本次未打安装包，保留当前工作树已有的 1.2.45 旧数据库修复。

### 配置位置

| 客户端 | 默认用户配置 | 自定义目录 | 接入方式 |
| --- | --- | --- | --- |
| Codex | `~/.codex/config.toml` | `CODEX_HOME` | `mcp_servers.pixels`，Pixels 工具预批准 |
| Kimi Code 2.x+ | `~/.kimi-code/mcp.json`、`config.toml` | `KIMI_CODE_HOME` | stdio；最前面的 Pixels allow 规则 |
| Claude Code | `~/.claude.json`、`~/.claude/settings.json` | `CLAUDE_CONFIG_DIR` | 用户级 stdio；`mcp__pixels__*` allow |
| DeepSeek Harness | `~/.dsh/cordis.patch.yml` | `DSH_HOME` | 官方 `@deepseek-ai/dsh-mcp-client` 插件 |

自定义 Claude 目录下使用 `.claude.json` 和 `settings.json`。Kimi 旧 Python CLI 的 `.kimi` 配置不迁移，界面提示升级。Windows 查找 PATH、npm 用户目录；macOS 额外查找 Homebrew 和用户安装目录。调用脚本使用独立 argv，路径支持空格和中文。

Kimi/Claude 停用仅移除本模块添加的权限；Claude 用用户目录下的 `pixels-bridge-integration.json` 标记归属。DeepSeek 仅编辑带 Pixels 标记的 YAML 块，保留其他注释和 `!!js` 表达式原文，不执行表达式。损坏配置、同名第三方条目、Claude 冲突规则、DeepSeek 外部 Pixels 覆盖项均拒绝覆盖。非空 flow-style YAML 和配置符号链接需先由用户整理，界面返回明确错误。

### 已执行验证

| 范围 | 结果与证据 |
| --- | --- |
| Windows Rust 配置模块 | 15 项常规测试通过；另外执行真实 MCP 探测及四客户端配置导出测试通过 |
| macOS ARM64 配置模块 | 16 项通过（含符号链接保护及写入失败回滚）；另外安装目录 `run-mcp.sh` 探测和四客户端配置导出通过 |
| MCP stdio 回归 | 8 项通过：工具目录/schema、新旧协议、结构化错误、设置禁用、异步查询/取消/去重、双进程与崩溃恢复 |
| MCP 目录单元测试 | 1 项通过 |
| Playwright | 21 项通过：新接入面板 10 项、现有历史/设置回归 11 项；三语言、亮暗主题、窄窗口、独立忙碌/失败/刷新 |
| TypeScript/Vite | `npm run build:assets` 通过；已有 bundle 大小警告仍存在 |
| Windows Codex | 隔离 `CODEX_HOME` 中 `codex mcp list --json` 识别生成配置 |
| Windows Kimi Code 2.1.1 | `doctor` 通过；官方 Web API 连接测试通过，发现 68 个工具 |
| Windows Claude Code 2.1.293 | 隔离安装与配置目录，`claude mcp get pixels` 返回 Connected |
| Windows DeepSeek Harness 0.2.0-rc.2 | 真实 CLI 合并用户 YAML；官方 MCP 插件发现 68 个工具，调用空命令返回结构化错误 |
| macOS Codex 0.155.0 | 隔离配置被真实 CLI 识别；启动路径含空格 |

所有客户端验证使用隔离目录，没有修改用户当前的客户端配置或终止既有会话。Mac 使用通过 Pixels 上传的隔离测试 crate，配置模块原样复用（只移除 Tauri 命令属性）；依赖经本地 cargo vendor 离线传输，不等同于完整 macOS Desktop 构建或新包安装验收。

### 实测修复的协议问题

新版 Claude 实际连接暴露了 `tools/list` 返回缺少 `resultType`、`ttlMs`、`cacheScope` 的问题。服务端现在完整返回现代 MCP 元数据，由 rmcp 自动适配旧协议。2026-07-28 和 2025-11-25 的真实 stdio 测试均通过；不依靠修改客户端绕过校验。

### 复现与产物

```powershell
cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib agent_integrations
cargo test --locked -p pab-bridge --test mcp_operations
cargo test --locked -p pab-bridge --bin pab-mcp catalog_is_compatible_with_sdk
cd apps/desktop
npm run build:assets
npx playwright test agent-integrations-ui.spec.ts history-settings-ui.spec.ts
```

可选真实探测测试：指定绝对路径 `PAB_INTEGRATION_FIXTURES`、`PAB_INTEGRATION_MCP`，运行 `export_and_probe_real_client_fixtures -- --include-ignored`。应使用隔离数据目录，避免影响实际配置。

本机日志：`.build/agent-integrations-rust-tests.log`、`agent-integrations-ui-tests.log`、`agent-mcp-protocol-tests.log`、`agent-claude-get.log`、`agent-kimi-tools-test.log`、`agent-dsh-tools-test.log`。这些临时文件不提交；UI 截图保存在 Playwright test-results。Mac 日志在仓库 `.build/agent-integrations-macos-tests.log` 和 `agent-integrations-macos-probe.log`。

### 验证边界 / 发布前剩余项

- 尚未通过四种客户端的真实模型账号分别完成远程命令、文件传输和截图对话；不宣称端到端业务工作流全部验收。
- Mac 的 Kimi/Claude/DeepSeek 客户端完整安装会话未验证；Linux 无界面实际客户端加载未验证，本次不增加 Linux 桌面。
- 本机用户级配置无法替代项目级配置、企业策略或客户端自身的安全确认。明确存在的冲突会报错；用户后来添加的覆盖配置仍需在对应客户端检查。
- 新包需要同时更新 Desktop 和 MCP 才能包含现代协议修复；本任务只完成源码、构建检查和上述测试，未生成新安装包。

## Windows 安装包补充验收

2026-10-08 应用户要求执行 `python scripts/build.py desktop --profile release --package`，统一分配版本 **1.2.46**。Desktop、Executor、MCP 与前端均重新编译，ZIP 和 NSIS EXE 生成成功。上文“未打包”描述的是开发验收时点，本次已完成 Windows 打包；未安装或替换正在运行的程序。

- 安装包：`.build/packages/pixels-agent-bridge-windows-x86_64-release-setup.exe`
- 大小：22,891,701 bytes
- ProductVersion：`1.2.46`
- SHA-256：`fdaaca650d85cdda998fc3400d256e3720ce1356c4bf4c04cbde07283361d848`
- 构建记录及打包器已验证三个 EXE、前端归档和版本一致性；最终安装包哈希再次核对通过。
- 本次 Release MCP 在隔离数据目录下以 2025-11-25 与 2026-07-28 协议探测，均返回 68 个工具；现代协议元数据完整。
- 构建日志：`.build/agent-integrations-package.log`。

## Cursor / OpenCode 与连接名称、检测修复

2026-10-08 后续增量，**尚未包含在 1.2.46 安装包中**。

### 实现

- MCP 在 initialize、现代 server/discover、ping、tools/list 和 tools/call 时取得客户端身份并上报；覆盖无需 legacy initialize 的现代客户端。只在身份变化时更新，避免空闲连接始终显示 MCP。
- UI 将已知身份显示为产品名称；详情保留原始名称及版本。同一客户端的多条会话仍按 sessionId/PID 分开，未知名称原样显示。
- DeepSeek 检测同时覆盖 PATH、用户 npm 全局目录及 npm/npx 缓存中的真实包和入口文件；刷新重新扫描，不执行 npx 或下载包。当前 Windows 的 global/npx 运行方式已核对。
- Cursor 检测编辑器和独立 Agent CLI；MCP 固定写入 `~/.cursor/mcp.json`。`CURSOR_CONFIG_DIR` **只影响 CLI 的 cli-config.json 和授权记录目录，不改变 MCP 路径**，此区别经实际 CLI 验证。
- Cursor CLI 添加 `Mcp(pixels:*)`，停用只撤销本模块新增项。编辑器的自动运行模式和首次服务信任需要在 Cursor 内设置；不覆盖编辑器 UI 的整个工具允许列表。界面明确提示。
- OpenCode 检测标准程序、`.opencode/bin` 和 npm/npx 安装。默认 `~/.config/opencode/`，支持 `XDG_CONFIG_HOME`、`OPENCODE_CONFIG` 文件和 `OPENCODE_CONFIG_DIR` 目录（后者优先）。目录内优先已有 opencode.jsonc、opencode.json、config.json，默认创建 opencode.jsonc。
- OpenCode 使用 `mcp.pixels.type=local`、命令数组及启用标记。`permission.pixels_*=allow` 置于最后，符合最后匹配规则；原有标量策略转换为 `*` 后保留，关闭时还原原 Pixels 策略及顺序。
- 使用 [dprint/jsonc-parser 的 CST](https://docs.rs/jsonc-parser/latest/jsonc_parser/cst/index.html) 编辑 JSONC，保留注释和无关配置；拒绝重复键、同名第三方条目、多配置文件中的 Pixels 重复项及工具禁用冲突。继续使用已有写入快照、原子替换及回滚。

### 官方参考

- [Cursor MCP](https://prod.cursor.com/docs/cli/mcp)、[CLI 配置](https://prod.cursor.com/docs/cli/reference/configuration)、[CLI 权限](https://prod.cursor.com/docs/cli/reference/permissions)、[编辑器权限](https://prod.cursor.com/docs/reference/permissions)。
- [OpenCode MCP](https://docs.opencode.ai/docs/mcp-servers/)、[配置层](https://docs.opencode.ai/docs/config/)、[权限顺序](https://docs.opencode.ai/docs/permissions/)。

### 增量测试

- Windows 配置模块：23 项通过；另运行真实 MCP 握手与六客户端隔离配置导出测试通过。
- macOS ARM64：相同配置源文件的隔离 crate 24 项通过（另含符号链接回滚），另通过已安装 `run-mcp.sh` 的真实握手与六客户端配置导出。使用 Pixels 原生 MCP 上传和执行；无安装替换、无 TCC 修改。
- MCP 身份测试：6 个客户端身份、legacy initialize 与现代 discover/tools/list，通过真实本地 WebSocket 检查首次工具调用前的身份上报。
- UI：12 项通过，覆盖六客户端、三语言/亮暗/窄窗口、重复会话名称、刷新检测、独立错误与忙碌；TypeScript/Vite 构建通过。
- Windows Cursor Agent `2026.09.28-64d2043`：隔离 HOME、USERPROFILE、CURSOR_CONFIG_DIR，实际 CLI 发现 68 个工具。只放 MCP 到自定义目录时客户端找不到服务；固定用户 MCP 路径验证后修正实现。
- Windows OpenCode `1.17.11`：隔离 XDG/配置/数据目录，真实 `mcp list` 返回 `pixels connected`。

日志位于 `.build/agent-six-clients-*.log`、`agent-cursor-mcp-list-tools-pixels.log`、`agent-opencode-mcp-list.log`，不提交临时测试产物。

上述配置与协议验收不等于六个模型账号的业务对话全流程验收。Cursor 编辑器内实际调用、Mac 上 Cursor/OpenCode 完整客户端会话、项目或企业策略覆盖仍未验证。用户级配置不能覆盖所有项目和企业策略；当前安装包需后续重新构建才包含本节改动。

## Windows 1.2.47 安装包

2026-10-08 应用户要求重新执行 `python scripts/build.py desktop --profile release --package`，自动递增至 **1.2.47**。本次包已包含 Cursor/OpenCode、客户端名称上报与 DeepSeek 检测修复，取代上文增量完成时“未打包”的状态。

- 安装包：`.build/packages/pixels-agent-bridge-windows-x86_64-release-setup.exe`
- 大小：22,975,357 bytes
- SHA-256：`dd0f056d4a710ad972b8286a06c38fbe3dbc3dc528f448287f650af9f3d116eb`
- Windows ProductVersion 及清单哈希核对通过；打包器核对 Desktop、Executor、MCP 构建版本。
- 新 Release MCP 的 2025-11-25 与 2026-07-28 协议握手/工具列表均通过，分别返回 68 个工具。
- 构建日志：`.build/agent-six-clients-package.log`。本次未安装或替换运行中的程序，未重新构建 macOS 安装包。

## 移除 Cursor 接入，MCP 连接独立页

2026-10-08 按用户要求移除 Cursor 的一键接入入口、检测及配置写入逻辑，保留其余五个客户端。已有 MCP 会话继续按上报信息正常展示。

- 左侧主导航在“设备列表”下面新增“MCP 连接”，简中、繁中、英文同步更新。
- 连接列表从“设置 → AI Agent”移出，独立页保留连接总数、客户端身份、PID、状态及设备/工具调用/任务/传输详情。
- 独立页滚动，四边沿用 10px 工作区间距；切换离开时解除订阅，重新进入获取最新快照并恢复实时更新。
- Windows 配置模块 22 项通过；Playwright 18 项通过，其中 6 项使用完整 App 验证三语言、亮暗模式、导航顺序、详情、实时清空、切页订阅清理与重新加载，以及设置仅保留五个接入项。
- TypeScript/Vite 构建通过；Windows 安装包由统一构建入口自动递增至 1.2.48，构建日志 `.build/mcp-page-package.log`。
- macOS 相同配置源码隔离 crate 23 项测试通过；另外复测 6 项主导航选中状态与页面切换，通过。
- Windows 1.2.48 已生成并核验 ProductVersion、文件大小（22,981,170 bytes）及 SHA-256：`ab93633a01a5a20cc500fa35f98e31f7c8dede0c7943358c60a1662fad1064fd`。MCP 新旧协议均返回 68 个工具。本次未执行安装。
- 用户要求避免每次整编后，调整统一构建入口为受影响组件版本更新，公共库不随安装包递增。23 项构建测试通过，含真实 Cargo 缓存命中验证；本次 1.2.48 产物已记录为后续增量基线，未为构建脚本调整再重编产品。
- 后续按用户要求，Windows EXE / macOS PKG 打包器强制在安装包文件名中包含已核验版本。当前 Windows 产物已重命名为 `pixels-agent-bridge-windows-x86_64-release-1.2.48-setup.exe`，同步更新校验清单；重命名前后 SHA-256 相同，版本仍为 1.2.48，没有重新编译或递增。脚本语法检查通过，macOS 打包输入测试 4 项通过、2 项平台/实包测试跳过。

## Desktop 关闭窗口隐藏到托盘

- 右上角 X 和系统关闭窗口操作（Windows Alt+F4）由原生 CloseRequested 统一拦截，隐藏主窗口，保留 Desktop 进程及 MCP 状态上报。
- 新增原生托盘图标；左键点击或菜单“显示窗口”恢复、取消最小化并聚焦主窗口。只有托盘“退出”走正常退出及 MCP 清理；设置中的程序重启仍可使用。
- 托盘菜单和关闭按钮提示跟随简中、繁中、英文偏好。macOS Dock 重新打开事件恢复窗口。
- Windows `cargo check --locked --lib`、TypeScript/Vite 构建通过；Playwright 关闭按钮调用及托盘语言同步测试 1 项通过。日志：`.build/desktop-tray-check.log`、`.build/desktop-tray-assets.log`、`.build/desktop-tray-ui.log`。
- UI 测试使用 Tauri mock，尚未完成真实原生托盘点击及 macOS 运行验收。本次未重新打包，1.2.48 安装包不包含此项改动。

## 侧栏账户入口与登录对话框

- 在“我的设备”上方增加 Ant Design 头像和账户名称组件；未登录使用通用用户图标，登录后使用用户名首字符头像。账户状态由 App 统一读取和管理，跨页面保持同步。
- 未登录点击账户入口或“我的”导航打开登录对话框；点击遮罩、按 Escape 不关闭，空闲时可通过 X 关闭并清空输入。提交期间防止重复提交及关闭，失败留在对话框内提示并可重试。
- 登录成功跳转“我的”；已登录点击入口直接跳转。“我的”仅显示头像、用户名、账号 ID、可用的团队名称及退出按钮，无登录输入框。退出成功回到首页，失败保留现有身份；读取身份失败提供重试，避免误显示为未登录。
- TypeScript/Vite 构建通过，账户及已有导航/Agent 接入 Playwright 共 27 项通过，涵盖三语言、亮暗主题、登录/退出、遮罩与 Escape、失败重试、密码清理、长用户名、提交中交互及已有身份读取。简中亮暗截图检查通过。
- 测试使用 Tauri mock，复用原生登录/退出命令，未对现网账号执行登录验收；未重新编译 Rust 或打包。日志：`.build/sidebar-account-assets.log`、`.build/sidebar-account-ui.log`。

## 仅安装包发布版本自动递增

- 按用户最新要求移除清单版本同步和组件指纹方案，统一构建只递增 `build-version.json` 中的发布版本；所有 Rust、npm、Tauri 内部版本保持现值，源码变化交给 Cargo 正常增量判断。
- EXE 文件名、安装器版本及清单仍取发布构建记录。macOS 的归档/PKG 发布版本与 App 内部版本解耦，完整 App 和二进制仍通过构建记录逐项验证 SHA-256。
- 16 项版本/编排/缓存测试通过，含真实 Cargo 二次构建全部四个模拟组件 `fresh=true`、全部内部清单与锁文件逐字节不变、发布版本进位和跨进程互斥；归档/PKG 测试 5 项通过，2 项需 POSIX 或实际 PKG 的测试跳过。三平台归档测试中 Mac App 内部版本为 0.1.0、发布版本为 1.2.0，验证不再要求相等。
- 日志：`.build/installer-version-only-tests.log`、`.build/installer-version-package-tests.log`。本轮未执行 macOS 原生打包。

## Windows 1.2.49 安装包

- 已生成 `.build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.49-setup.exe`，包含托盘隐藏/退出、侧栏账户入口、登录对话框与用户信息页。
- 大小 23,032,212 bytes；ProductVersion 1.2.49；SHA-256 `5dd9014a527a719876b6b8c8d18a33d56b4b26c1fde651a86202123249f71fbe`。独立读取文件属性、大小和哈希，与安装包清单核验一致。
- 本次开始构建时旧规则已分配 1.2.49 并更新 Desktop 内部版本；用户随后要求仅安装包递增，构建脚本已调整，保留本次已分配版本和进行中的编译。此后所有组件内部版本固定，后续发布不再改写清单。本次 Executor/MCP 保持 1.2.48，Cargo 命中缓存，检查用时约 5 秒。
- 构建日志 `.build/sidebar-account-package.log`。未执行安装或部署；原生托盘交互仍需安装后验收。
