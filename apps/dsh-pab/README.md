# Pixels Agent Bridge — DeepSeek Harness panel (`apps/dsh-pab`)

A DeepSeek Harness plugin that puts Pixels Agent Bridge in the Web GUI's right
sidebar: sign in, add devices, open terminals, browse history — without
installing the desktop app on the operator machine.

被控端仍然需要安装 Pixels Agent Bridge 的 Executor（桌面端或 headless 服务）；本插件只负责**控制端**。

## 组成

| 文件 | 作用 |
| --- | --- |
| `package.json` | npm 包清单：`dsh.bundle`（profile 配置层）+ `dsh.client`（浏览器半） |
| `cordis.patch.yml` | 插入 profile 的那一行（`name: dsh-pab`，必须是裸包名） |
| `src/index.js` | Host 半：启动 `pab-mcp`、MCP JSON-RPC 客户端、`/x-pixels/*` 路由、登录/配对 |
| `src/client.js` | Client 半：右侧栏 tab、会话头部按钮、全部 UI（纯 JS，无构建步骤） |
| `scripts/*.mjs` | 自测脚本：MCP 握手、Host 路由、工具名探测 |

## 二进制从哪来

插件**不内置** `pab-mcp`。启动时按顺序解析：

1. `PAB_MCP_PATH` 环境变量；
2. `apps/dsh-pab/native/<platform>-<arch>/pab-mcp[.exe]`（打包安装时提供）；
3. 仓库构建产物 `target/release/pab-mcp[.exe]`；
4. 仓库构建产物 `target/debug/pab-mcp[.exe]`；
5. Windows 上已安装的 `C:\Program Files\PixelsAgentBridge\pab-mcp.exe`；
6. `PATH` 中的 `pab-mcp`。

无论来源，运行时会把它复制到 `%LOCALAPPDATA%\dsh-pab\bin\` 并从那里启动：
DSH 给 workspace-write 准备过的目录带 Low 完整性标签，从那种目录直接运行的进程会以
Low IL 启动、写不了自己的工作区外日志。复制到状态目录可以避开这一点。

`pab-mcp` 用固定文件名独占打开 `mcp.log`，所以每次 spawn 都使用独立日志子目录
（`%LOCALAPPDATA%\dsh-pab\logs\<pid>-<n>`），否则常驻 MCP 与短命 CLI 会互相锁死。

## 安装进 profile

```powershell
# 开发：链接本目录
dsh plugin --profile desktop add D:\GoCloud\PixelsAgentBridge\apps\dsh-pab

# 或手工：在 profile 的 package.json 里加 link 依赖，并把它加进 dsh.profile.bundles
```

安装后重启 DeepSeek Harness：会话头部出现 **Pixels** 按钮，右侧栏多一个 Pixels 标签页。

## 数据与账号

账号会话与设备密码位于 `%LOCALAPPDATA%\PixelsAgentBridge`，与 Pixels 桌面端共用：
桌面端登录过，插件打开即已登录；插件里登录/退出也会影响桌面端。

默认服务为官方服务（`wss://pab.rgaa.vip/control`）。自托管时给插件行传环境变量
`PAB_CONTROL_URL` / `PAB_RELAY_URLS`（或在 profile 配置里设置对应字段）。

## 安全边界

`ctx.webServer` 注册的路由没有任何内建鉴权。本插件要求：

- `Origin` 存在时必须与请求 `Host` 同源；
- 请求必须带自定义头 `x-pixels-client: 1`（跨站页面无法在不触发预检的情况下携带）。

因此它只对同源的页面和本机进程开放，路由也只监听 loopback。

## 远程终端

设备卡片选中后可打开交互式终端，身份二选一：

- **服务账户（System）**：`pab_open_terminal` 的 `execution: { mode: "service" }`，Windows 上即 `NT AUTHORITY\SYSTEM`；
- **用户会话**：从 `pab_list_execution_contexts` 取 `context_ref`，以登录用户身份打开。

终端由 `pab_open_terminal` / `pab_terminal_input` / `pab_terminal_read` / `pab_terminal_resize` / `pab_terminal_close`
驱动，输出按 300ms 轮询读取。前端使用 **xterm.js**（MIT，`assets/` 内置，见 `assets/NOTICE.txt`），
由 Host 的 `/x-pixels/assets/*` 路由提供，首次打开终端时才懒加载。

## 开发与验证

```powershell
node apps\dsh-pab\scripts\host-smoke.mjs   # 不需要启动 DSH，直接跑 Host 路由
node apps\dsh-pab\scripts\smoke.mjs        # 直接对 pab-mcp 做 MCP 握手
node apps\dsh-pab\scripts\tools.mjs system # 按名字过滤工具
```

前端拆成了两层，目的是让界面迭代不用重启：

- `src/client.js` —— 很薄的 loader，只注册插件并从 `/x-pixels/ui.js` 动态 `import()`（带时间戳绕缓存）；
- `src/ui.js` —— 全部界面代码，作为 ES module 由 Host 直接从磁盘读取返回。

所以：

| 改了什么 | 生效方式 |
|---|---|
| `src/ui.js`（界面、交互、样式） | **刷新页面（F5）** 即可 |
| `src/client.js` / `src/index.js`（loader、路由、Host） | 需要重启 DeepSeek Harness |
| `assets/`（xterm） | 刷新页面 |

## 发布打包

发布时把构建好的 `pab-mcp` 装进一个自包含目录（不是提交进 Git）：

```powershell
# 直接用打包脚本（默认 release、宿主平台、输出到 .build/packages）
python scripts/package_dsh_plugin.py --profile release

# 顺带构建 pab-mcp（超过 10 并发的部分会被脚本压到 10）
python scripts/package_dsh_plugin.py --profile release --build

# 交叉构建产物：指定平台和二进制路径
python scripts/package_dsh_plugin.py --platform darwin-arm64 --binary path/to/pab-mcp

# 只出目录、不出 tgz
python scripts/package_dsh_plugin.py --no-tgz
```

产物：

```
.build/packages/dsh-pab-<version>/        完整安装目录
├── package.json  cordis.patch.yml  README.md
├── src/index.js  src/client.js
├── native/win32-x64/pab-mcp.exe
└── release-manifest.json                         版本、平台、二进制大小与 sha256
.build/packages/dsh-pab-<version>.tgz    npm 包（含 native/）
```

安装到操作端：

```powershell
dsh plugin --profile desktop add <发布目录>
```

打包流程也可以从 `scripts/build.py` 触发（会按发布编号递增 `build-version.json`）：

```powershell
python scripts/build.py dsh-pab --profile release --package
```

如果不想动发布编号，直接用上面的 `package_dsh_plugin.py`。

## 后续

- 桌面端集成：`apps/desktop/src-tauri/src/agent_integrations.rs` 已负责写 `~/.dsh/cordis.patch.yml`，
  可在此增加「安装控制面板插件」开关。
