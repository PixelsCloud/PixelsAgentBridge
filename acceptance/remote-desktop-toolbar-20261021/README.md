# 远程桌面工具栏与全屏调整

Windows 1.3.12 已发布到 COS，安装包解包、包内 Release 查看器、公开更新接口和完整下载校验通过。以下开发阶段记录按原测试范围保留；Mac/Linux 本次改动未发布。

日期：2026-10-21。分支：`fix/remote-desktop-toolbar-20261021`，基线：`b32567b`。

## 行为

- 手动打开的远程桌面在首帧呈现、输入通道就绪后默认启用操作。远端不支持输入时保持禁用；点击停止后，切屏、切换 FPS 和重连保留停止状态。每次新会话申请新的输入租约，不重放旧输入。
- 人工协助入口保留原来的只读起点及显式完成流程。复用现有窗口时也停止操作，包括首次启用尚未返回的情况。
- 安全注意序列按钮直接显示 `Ctrl + Alt + Delete`，只在认证连接报告远端 Windows 时显示；后端同样检查远端系统和输入状态。
- 信息开关移到标题栏，使用 Bug 图标，默认收起。底部原生信息层只展示信息，不再出现独立的“i”开关。
- 调试信息展示连接与控制状态、远端系统、P2P/Relay 路径、显示器、分辨率、编解码器、请求和采集上限、编码目标码率、首帧和输入状态、会话标识与代次。缺失数据标记为未上报；上限与目标不冒充实测指标。
- 全屏调用本地窗口全屏 API，同时隐藏整个自绘标题栏，并将原生视频区域的顶部预留从 42 改为 0。退出恢复原布局，图标在 Expand/Shrink 间切换。
- WebView 与原生视频持有焦点时均处理本地 Esc；退出全屏不向远端发送 Esc。原生键盘自动重复、释放和输入模式 ACK 保留有序处理。延迟的全屏响应不能覆盖较新的 Esc 退出状态。

## 验证

| 检查 | 结果 |
| --- | --- |
| TypeScript `tsc --noEmit` | 通过 |
| Vite 生产构建 | 通过；原有大 chunk 提示仍存在 |
| Playwright 远程桌面、人工协助、终端回归 | 42 项通过 |
| 中文浅色、深色 640 × 400 工具栏及调试面板 | 另 2 项通过；面板不遮挡标题栏 |
| `pab-desktop-media` Rust 单元测试 | 33 项通过，1 项原有手工计时测试忽略 |
| Windows 原生全屏与 Esc 专项 | 通过；4 个布局状态、2 种输入模式，远端 Esc 事件为 0 |
| 桌面 Tauri 后端 `cargo check --lib` | 通过 |

Windows 原生专项使用独立的屏外生成窗口和 D3D11，不读取桌面画面、不向真实远端或全局键盘发送输入。它验证视频区域、Esc 消费、自动重复及调试层显示/隐藏，不代替完整远程设备会话验收。

中文最小窗口截图：[浅色](toolbar-light.png)、[深色](toolbar-dark.png)。截图来自测试页面的模拟会话，不包含真实远端画面。

原始结果：[后端编译](backend-check.txt)、[媒体单元测试](media-unit.txt)、[Windows 原生专项](windows-native.txt)。

macOS、Linux 原生视频区域和键盘路径已同步修改，本轮未在这两个平台编译或实机验收。开发验收时未发布安装包、未覆盖本机客户端；当前 Windows 发布状态见下文发布记录。开发验收时 Cargo 固定 2 个构建作业、嵌套 CMake 1 个作业，未并行运行两项 Cargo 编译。

## 复验命令

先准备对应平台的原生编译工具和 NASM，并设置 `CARGO_BUILD_JOBS=2`、`CMAKE_BUILD_PARALLEL_LEVEL=1`、`RAYON_NUM_THREADS=2`。

```text
# 仓库根目录
cargo test -p pab-desktop-media --lib --jobs 2
cargo test -p pab-desktop-media --test windows_cursor_render --jobs 2 viewer_chrome_fullscreen_inset_and_escape_are_local -- --ignored
cargo check --manifest-path apps/desktop/src-tauri/Cargo.toml --lib --jobs 2

# apps/desktop
node node_modules/typescript/bin/tsc --noEmit
node node_modules/vite/bin/vite.js build
node node_modules/@playwright/test/cli.js test tests/remote-desktop.spec.ts tests/desktop-assistance.spec.ts tests/desktop-terminal.spec.ts --reporter=list
```

## Windows 1.3.12 发布（2026-10-21）

发布源码为 `d51e0127ca50162cf7cda90dcf5e709681d7e425`，来自对本批已验收工具栏工作区的只读快照；原工作区及主工作区未修改。快照文件逐项核对，原始测试日志按原样保留，既有 44 项界面测试、33 项媒体单元测试和 Windows 原生专项直接复用。

Release 使用 Cargo 6 作业、嵌套构建 1 作业、10 核亲和性和同机编译额度监控；沿用 Solid LZMA、64 MB 字典及内部运行时 ZIP 不压缩方案。安装包 [pixels-agent-bridge-windows-x86_64-release-1.3.12-setup.exe](https://pixels-agent-bridge-1251612681.cos.ap-beijing.myqcloud.com/releases/windows/x86_64/1.3.12/pixels-agent-bridge-windows-x86_64-release-1.3.12-setup.exe) 为 55,100,071 字节（52.55 MiB），SHA-256：`fac17f25643784223ae502d57c85098fcd52d6672f7489660df01b2b70e444df`。

实际解包核对四个 x64 程序与构建收据哈希一致，运行时 ZIP、许可证、公开空 TOML 配置及两种 MCP 初始化/工具列表通过。未再次执行安装程序，未覆盖本机安装版。

包内 Release 查看器连接 90：手动打开首帧后自动启用操作通过，随后显式停止操作再验证本地窗口；Windows 专用 Ctrl+Alt+Delete 标签、默认隐藏调试信息、标题栏显示/隐藏原生面板、42 DIP 正常顶部预留、全屏隐藏标题栏与 0 顶部预留、原生视频焦点 Esc 退出及恢复正常布局、最小化隐藏/恢复均通过。全程为同一视频 session，停止操作状态保持；没有向远端注入测试键鼠。原生 Esc 消息只投递到已核对路径和 PID 的私有视频窗口。

私有 GUI 已停止，新建凭据槽撤销，原凭据未改。Cargo 自动生成文件的依赖图与版本/校验和不变，原始生成文件和核对记录保留，发布工作区恢复冻结源码。Ed25519 发布签名与服务返回一致，公开完整下载的大小与 SHA-256 一致。旧版 1.3.11 能发现 1.3.12，1.3.12 不再返回更新。详见 [机器可读发布记录](windows-release.json)。原始私有资料位于 `.build/remote-desktop/windows-toolbar-release-20261021/`；真实远端截图只保留本地。
