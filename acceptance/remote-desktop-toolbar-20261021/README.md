# 远程桌面工具栏与全屏调整

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

macOS、Linux 原生视频区域和键盘路径已同步修改，本轮未在这两个平台编译或实机验收。未发布安装包，未覆盖正在运行的本机客户端。Cargo 固定 2 个构建作业、嵌套 CMake 1 个作业，未并行运行两项 Cargo 编译。

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
