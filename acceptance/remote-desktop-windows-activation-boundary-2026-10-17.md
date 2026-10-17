# Windows 首次点击：激活路径对照

产品基线 `6af1fe7`；继续调查 [首次激活诊断](remote-desktop-windows-activation-2026-10-17.md)
及 [正式光标窗口中再次复现](remote-desktop-windows-cursor-product-2026-10-17.md) 的重复按下。
本轮未改变产品输入行为，也未解决此问题。实验进一步排除了消息循环、旧/新
注入 API 和 PE 子系统差异，确认改变目标测试窗口的 `WM_MOUSEACTIVATE`
处理顺序会改变结果。原始消息、任务 ID 和各二进制哈希在
[结构化记录](remote-desktop-windows-activation-boundary-2026-10-17.json)。

## 90 的结果

所有实验在原安装服务上运行独立、有界的自有窗口，未替换服务程序。
每次只注入测试窗口内的标记事件，窗口失效或被遮挡时停止；抬起始终释放
本轮已经发出的按下。每个进程最多 5 秒，编译单核心、依次执行。

| 对照 | 实际消息 / 结果 |
| --- | --- |
| 默认 `SendInput` 与 `PeekMessage` | 一次按下出现 `DOWN, DBLCLK, UP`，失败 |
| `GetMessage` 阻塞消息循环 | 相同失败 |
| 旧 `mouse_event` API | 相同失败 |
| GUI 子系统而非控制台子系统 | 单击、双击、拖拽仍有额外首次按下，失败 |
| 测试窗口在 `WM_MOUSEACTIVATE` 返回 `MA_NOACTIVATE` | 单击 `DOWN, UP`，双击 `DOWN, UP, DBLCLK, UP`；拖拽一次开始、一次结束，全部通过 |
| 测试窗口尝试提前 `SetForegroundWindow` | 消息顺序正常，但注入前前台条件仍为假，不能计为完整通过 |

激活状态追踪显示，默认路径在 `WM_MOUSEACTIVATE` 返回时前台句柄为零，随后
收到 `WM_SETFOCUS/WM_ACTIVATE`，窗口成为前台，再出现相同时间戳和标记的
`DOWN, DBLCLK`。`MA_NOACTIVATE` 对照先收到一次 `DOWN`；标准按钮处理又会
使窗口成为前台，之后收到 `UP`，没有重复按下。这说明对照改变的是激活与
首次点击处理顺序，不能概括为“只要窗口不激活就没有问题”。

Microsoft 定义 `MA_NOACTIVATE` 保留鼠标消息、禁止该次消息驱动的自动激活；
子窗口默认处理会把 `WM_MOUSEACTIVATE` 交给父窗口。这仅用于解释实验，
不证明 OS 或某驱动存在已定位的缺陷。参见
[WM_MOUSEACTIVATE](https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-mouseactivate)。

不能把测试窗口的返回值用于产品修复：产品不拥有远端任意应用的窗口过程，
也不能禁止其正常激活。尚未增加点击去重、丢弃首击、强制提前激活或第三方
进程改动。此问题仍属于 W2 的未完成项。

## 工具保护与验证边界

本机五组对照中，`GetMessage` 和旧 API 两组混入了标记为零的外部鼠标输入，
第一组甚至没有发出自测输入；这两组不能作为回归通过或产品失败证据。
保留原记录，并停止继续在本机运行交互测试。

诊断源码现已在自有窗口收到非本轮标记的按钮事件或键盘按下时结束，仍释放
已经注入的按钮，不恢复用户新移动的指针或切换后的前台。三个最终 90 对照
验证该保护没有改变无干扰实验：默认路径仍失败，不自动激活的单击和拖拽
通过，均无外部输入、无残留按下。外部干扰退出路径尚未单独做自动化验收。

所有四轮远端临时二进制已删除；每轮原服务的状态、命令与 PID 前后相同。
本机诊断进程已结束。实验不涉及账号、密码、锁屏、服务切换或安装版替换。

复现入口为 [windows_control_fixture.cpp](../experiments/remote-desktop/windows_control_fixture.cpp)，
在已获准的交互桌面依次运行 `button-self`、`getmessage-button-self`、
`legacy-button-self`、`noactivate-button-self`、`noactivate-doubleclick-self`、
`noactivate-drag-self`。GUI 子系统对照使用同一源码增加链接参数
`/SUBSYSTEM:WINDOWS /ENTRY:mainCRTStartup`。不要在用户正在操作的桌面上批量运行。
