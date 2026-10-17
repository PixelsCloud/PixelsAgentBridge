# macOS 操作端：工具栏布局、窗口输入与关闭释放

日期：2026-10-17；代码基线 `715d1d3`。Apple 为 macOS 27.0.1 arm64，
1920×1080、1×；目标 90 为 Windows Server 2022、1920×1080。
本轮使用现有认证、真实产品协议和独立远程窗口，未替换安装版。

## 问题与修复

Mac 操作端首次连接 90 后，原生画面覆盖了 WebView 工具栏第二行。
原布局按整个 contentView 减去 72 点计算；全尺寸 contentView 还包含标题栏。
改为相对 NSWindow.contentLayoutGuide 约束原生视图，顶部另留 72 点。
标题栏、窗口尺寸和全屏变化由同一可用内容区域驱动；独立原型窗口不受影响。

新增 AppKit 回归测试覆盖普通/全尺寸内容视图、三个尺寸和样式切换共 8 个状态，
检查工具栏命中区域与原生视图关闭清理。以 HEAD 原始 presenter.h 编译同一测试
可复现 `Native media overlaps toolbar/titlebar`；修复后 8 项通过。
实际产品窗口 1100×760、全屏 1920×1080 及返回窗口后，工具栏均完整显示，
全屏、启用/停止操作按钮实际可用。截图摘要与构建哈希见配套 JSON。

回归测试须在 Mac 图形登录会话中手动运行；只创建自己的测试窗口，不采集屏幕：

```sh
xcrun clang++ -std=c++17 -fobjc-arc -fblocks -O1 \
  -Icrates/desktop-media/native/macos \
  crates/desktop-media/tests/native_macos_layout.mm -o /tmp/pab-native-layout \
  -framework Foundation -framework AppKit -framework ApplicationServices \
  -framework VideoToolbox -framework CoreVideo -framework Metal \
  -framework QuartzCore -framework Carbon -framework IOKit
/tmp/pab-native-layout
```

正式 Mac Release Desktop 构建使用 4 jobs，媒体库 11 项测试通过。
测试包沿用固定路径和签名身份；没有再次请求用户授权。
实验 MCP 客户端支持显式 argv，从而可传入隔离 TOML 的 `--config`，
避免 macOS 默认配置落到安装目录。真实 Mac MCP 已完成认证连接。

## 产品输入与 AI 协助

通过真实 AI 协助提示点击进入窗口，默认只查看，然后点击启用操作。
测试输入先进入 Mac 产品原生视图，再经网络到 90；只向自建测试窗口操作。

| 操作 | 90 实际观察 |
| --- | --- |
| 窗口画面中心点击 | (960,540)，clicks=1 |
| 第二个窗口坐标点击 | (1059,599)，clicks=2 |
| Ctrl 按下/抬起 | down=1、up=1、held=false |
| 滚轮 | wheel=120 |
| 停止操作后再次点击画面 | clicks 保持 2 |
| 全屏中心点击 | (960,540)，clicks=3 |
| 重新打开窗口 | 默认恢复只查看 |
| 第二轮按住 Ctrl 后关闭窗口 | down=1/up=0/held=true → down=1/up=1/held=false |

第二轮关闭后的释放是在清理 Mac 本机 Ctrl 之前从 90 读取，且检查 fixture
输出新鲜度小于 2 秒。首轮 fixture 的 120 秒期限结束后有一次关闭探测；
其旧输出不用于关闭释放结论，另起第二轮完成上述新鲜样本验证。
协助状态两次观察到 waiting→active→waiting，关闭窗口不误报 completed。

第二轮首次激活 fixture 时，一次物理点击产生两次按下，复现已有 90 问题；
首轮未出现。本轮不修复、不去重，也不将这项标为通过。对 90 的预先 focus
请求曾被 Windows 拒绝，之后由产品画面点击激活，没有绕过系统焦点限制。

## 恢复与范围

90 原服务恢复 Running，测试进程与按键释放标志均为 0，随后删除 5 个测试任务
和暂存目录。Apple 候选 GUI 停止、协助取消，候选进程为 0；固定测试包和隔离
profile 保留供后续验证，安装程序与配置未覆盖。本机安装版未替换。

首轮 fixture 完整记录已下载并提取状态变化；第二轮完整日志下载在服务恢复时
连接超时，未取得全日志，第二轮证据为保存到本机的两次成功且新鲜的远端探测。
本轮不声称 Mac 操作端动态显示 FPS、Mac 系统登录、多屏、输入法、键盘布局、
弱网或长期稳定性通过。W4/W7 保持未完成，没有发布安装包。

机器记录：[remote-desktop-macos-operator-layout-2026-10-17.json](remote-desktop-macos-operator-layout-2026-10-17.json)。
