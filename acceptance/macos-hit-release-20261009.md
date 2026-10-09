# macOS 窗口命中修复与客户端发布

状态：开发、两平台单元测试、两平台打包/覆盖安装、Mac 实机输入验收完成；Windows 新 MCP 的会话工具验收待 Codex 重启。

## 范围

1. 修复 Mac 窗口绑定点击对 AX 控件命中接口的依赖，改用 AppKit 原生鼠标窗口命中检测；保留前台、窗口身份、坐标和遮挡校验。
2. 区分 Accessibility 不支持、响应失败、无效窗口和权限问题，错误包含具体属性/动作。
3. 将修复和已完成的账号 UI 改动发布为 Windows/Mac 安装包；增量编译，不清空缓存、数据库或固定签名证书。
4. 验证升级后的本机账号、设备码、Mac 授权、连接和输入；随后继续 USER_ACCOUNT_PLAN.md 第 14 节 GitHub 登录任务。

## 原因与方案

原实现通过 `AXUIElementCopyElementAtPosition` 查询控件再读取 `AXWindow`，目标应用未实现控件命中时返回 `-25208`，无响应时可能返回 `-25204`。这不等于操作系统未授权。

使用已有 objc2-app-kit 绑定的 `NSWindow.windowNumber(at:belowWindowWithWindowNumber:)`，输入 `NSEvent.mouseLocation` 的 AppKit 屏幕坐标，直接比较 xcap 的窗口 ID。无需自行转换 Y 轴或处理 Retina 比例，不使用窗口外接矩形猜测命中。苹果接口按实际鼠标按下规则处理遮挡、透明区域和忽略鼠标事件的窗口。

1.2.62 首次安装后点击成功，但重复 focus 后，系统级 `AXFocusedApplication` 查询返回 `-25212`/`-25204`。1.2.64 改为 `NSWorkspace.frontmostApplication` 验证前台 PID，再直接查询该进程 `AXFocusedWindow` 并比较保留的 AX 对象身份。切换期间没有前台窗口返回 false，已有 focus 观察循环继续等待；其他 AX 错误仍然中止输入。没有降低为仅比较应用名、窗口标题或矩形。

参考：[Apple 窗口鼠标命中](https://developer.apple.com/documentation/appkit/nswindow/windownumber(at:belowwindowwithwindownumber:))、[AX 控件命中](https://developer.apple.com/documentation/applicationservices/1462077-axuielementcopyelementatposition)、[AX 未实现错误](https://developer.apple.com/documentation/applicationservices/axerror/kaxerrornotimplemented)。

## 验证记录

- Windows `cargo test -p pab-desktop-control --lib`：40 passed、5 ignored（既有平台/交互测试），无失败。
- Mac 同命令：47 passed，无失败。
- Mac Swift 只读原型：指定 Pixels 窗口内位置返回 1147，与 CGWindowList/xcap 窗口 ID 一致。
- 旧安装版 click operation `f2aafec9-77ba-405e-9b4a-1537dd61a208` 返回 `-25204` 未确认。未盲目重放；后续验证以新安装版和重新观察的窗口为准。
- Mac 构建使用独立源码目录 `/Users/huayang/source/pab-mac-hit-20261009`，复用原仓库 target/node_modules；不覆盖用户原仓库的未提交修改。
- Mac 1.2.62 覆盖安装：文件哈希、设备 ID/码、端点密钥、账号元数据、三个程序的固定签名身份全部保持。首次窗口点击 `b94e9a87-21cf-42da-9f66-2e0198d1bd2f` 成功并进入「我的」页面；后续发现前台查询问题，最终验收改用 1.2.64。
- focus 补充修复后 Mac 再次执行单元测试：47 passed。Windows 实现未受该 macOS 条件编译改动影响。

## 产物

| 平台 | 安装包 | SHA-256 |
| --- | --- | --- |
| Windows x86_64 | `pixels-agent-bridge-windows-x86_64-release-1.2.63-setup.exe` | `5cc4edf50ce5f1d6c23d8a8292d01c2060b41e674a1f43a2f4a29e876ae961f8` |
| macOS aarch64 | `pixels-agent-bridge-macos-aarch64-release-1.2.64-setup.pkg` | `4a386469973e41a6305a3a18c9da26f6c83168ddcbf831245f2f919603c677bf` |

内部版本保持 Executor/MCP 1.2.48、Desktop 1.2.49。Mac 使用既有固定免费证书签应用和内部程序，PKG 未进行付费 Developer ID 签名/公证。

## Mac 最终安装与实机结果

- 1.2.64 覆盖安装成功，安装器退出 0；逐项安装文件哈希匹配，设备 ID/码、端点密钥、账号元数据和固定签名身份全部保持。结果 `/private/var/tmp/pab-mac-hit-install-1264/result.json`。未重置 TCC。
- 原生 `pixels.pab_desktop_input` focus + 点击进入「我的」：`6b27b508-f8d3-4c61-bf87-3d96dc01377b` 两步完成，截图确认 home、已关联、云列表已同步，服务正常；`.build/mac-hit-final-account.jpg`。
- 创建独立的非激活浮层遮挡「设备列表」点击位置，窗口绑定点击 `ef7a1756-afc8-49d4-9e1f-a1769b73ab03` 返回 `pointer targets another window; click stopped`；截图仍在「我的」。鼠标移动已发生，所以操作保守标记 unconfirmed，没有盲目重放。
- 关闭原浮层，再以 `ignoresMouseEvents=true` 创建同位置浮层；`b478bc2d-1531-4a1a-87ff-47268d32396f` focus + 点击完成，截图确认进入设备列表；`.build/mac-hit-final-devices.jpg`。两个测试浮层进程均已按核对后的 PID 清理。
- 使用升级前 window_ref 的 `49e7e88e-cdcd-4bff-9f7c-a4773cd17f9f` 在动作开始前拒绝（`action_started=false`），没有误用新窗口。
- 测试结束重新 focus + 点击返回「我的」，两步完成。实际截屏和输入可用；从后台启动的独立 CLI 进程权限探测曾返回 false，不将不同责任进程的 TCC 结果当成已安装 GUI 未授权。
- Mac 最终 PKG 已通过实际 MCP 二进制传输下载到 Windows，哈希一致；operation `d179962f-f83c-48e9-89f7-652d1bd06037`。测试安装 launchd 任务已卸载，备份/日志保留。

## 待完成

- 替换本机会话 MCP 后，需在新 Codex 会话验证新 MCP 原生工具。
- GitHub 登录继续按独立任务实现；创建 App 的步骤见 `GITHUB_LOGIN_SETUP.md`。

## Windows 覆盖安装

1.2.63 默认目录静默安装退出 0，三个程序的安装哈希均与 Release manifest 相同。安装前备份两个 SQLite，安装后设备 ID/码、端点密钥、账号元数据均保持；服务 Running，并观察到安装后的新 Authenticated 心跳。结果 `.build/mac-hit-install-1263/result.json`，没有删除数据库。

新安装 `pab-mcp account status` 返回原 home 账号、revision=1；`account devices status` 返回 pending=0、syncing=false、error=null、conflicts=[]。这是安装后二进制 CLI 检查，不代替新 Codex 会话的 MCP 工具验收。
