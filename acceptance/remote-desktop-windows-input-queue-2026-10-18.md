# Windows 首次点击：消息队列与跨进程注入对照

日期：2026-10-18；产品基线 `ab879d1`；90，既有 user 执行上下文、session 2。
本轮是诊断进展，没有修改产品输入行为，没有解决全部首次点击边界。
计数、消息来源、回调栈、任务 ID、构建哈希和服务前后状态见
[结构化记录](remote-desktop-windows-input-queue-2026-10-18.json)。

## 发现

1. 首次单击的 `DOWN, DBLCLK, UP` 分别由三次 `PeekMessage(PM_REMOVE)` 返回，
   每次回调对应一次正常 `DispatchMessage`，没有观察到激活过程重入造成的重复回调。
   来源均为注入鼠标，标记一致；两个按下消息的时间戳相同。可确认多出的消息已在队列读取端出现，
   尚不能确定是哪层系统或驱动逻辑造成。
2. 将 SendInput 移至独立子进程，单击、双击、拖拽仍复现同样异常。
   因此“向自身进程注入”不是复现的必要条件。目标仍是自有测试窗口，不能泛化成任意第三方应用。
3. 窗口创建后，仅在所属 UI 线程调用 `SetActiveWindow`，不发送预热点击、不调用
   `SetForegroundWindow`，首次单击即正常。实测前台句柄未变、目标仍非前台，
   其 UI 线程的活动窗口状态已指向目标。跨进程单击、双击、拖拽全部符合顺序和动作计数。
4. 尝试随后 `SetActiveWindow(nullptr)` 的对照未清空该线程活动窗口状态；
   这一组虽然点击正常，但不能证明“清空状态后仍正常”。保留实际 setup 值，不采信无效假设。

本轮进一步限定了[上一轮线程激活结论](remote-desktop-windows-reactivation-2026-10-18.md)：
不必预热点击，不必曾成为前台，初始化自有线程的活动窗口状态就足以改变本环境结果。
所有先前失败窗口都以 `SW_SHOWNOACTIVATE` 启动，且此前没有线程活动窗口；
不能笼统声称任何应用的首次点击都会重复。但该失败边界继续保留，不据此关闭 LT-04.2。

## 12 组实测

| 对照 | 单击 | 双击 | 拖拽 |
| --- | --- | --- | --- |
| 默认同进程注入、未初始化线程活动窗口 | 失败 | 失败 | 本轮未测 |
| 独立进程注入、未初始化线程活动窗口 | 失败 | 失败（两次） | 失败 |
| 自有线程 SetActiveWindow，前台未变 | 通过 | 本轮未测 | 本轮未测 |
| 同上，再尝试清空活动状态 | 点击通过，但清空未生效 | 本轮未测 | 本轮未测 |
| 独立进程注入、自有线程 SetActiveWindow、前台未变 | 通过 | 通过 | 通过 |
| 测试窗口返回 MA_NOACTIVATE 的既有对照 | 通过 | 本轮未测 | 本轮未测 |

成功双击为 `DOWN, UP, DBLCLK, UP`；成功拖拽一次开始、一次结束，最终 (250,80)。
失败双击为 `DOWN, DBLCLK, UP, DOWN, UP`；失败拖拽两次开始、一次结束。
每轮都检查输入标记、窗口归属、活动桌面、按键初态和结束抬起状态。
所有 12 组没有观测到外部输入干扰，结束时左键均释放。

## 实现与验证范围

新增 [windows_input_trace.cpp](../experiments/remote-desktop/windows_input_trace.cpp)，
复用已有五秒自有窗口诊断，观察消息取出、派发和回调栈。
调用栈仅记录模块文件名和模块内偏移，不读取其他应用内容；没有安装全局 hook。
`external-` 由同一诊断二进制创建短命子进程执行输入，限定为父进程测试窗口和鼠标标志；
无通用 shell，也不更换产品服务或认证。

三次编译依次执行，编译器及子进程固定一核。共享 fixture 从 Git HEAD 归档后编译，
未依赖或覆盖工作区已有的未提交 settings-only 改动。无参数退出，不创建测试窗口。
四轮远端上传均按精确路径、确认进程已退出后清理；90 服务 State、PathName、PID 前后一致。
没有在本机用户桌面执行交互诊断。首次准备脚本因 PowerShell 字符串转义报语法错误，
未启动编译/远端动作；改用文件脚本后构建成功。

Microsoft 文档规定双击的四条消息顺序，见
[Responding to Mouse Clicks](https://learn.microsoft.com/en-us/windows/win32/learnwin32/mouse-clicks)。
[SetActiveWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setactivewindow)
说明线程活动窗口与应用前台状态的区别；这里用实际前台句柄核对，而非只凭返回值推断。
消息追踪 API 依据
[GetCurrentInputMessageSource](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getcurrentinputmessagesource)、
[InSendMessageEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-insendmessageex) 和
[CaptureStackBackTrace](https://learn.microsoft.com/en-us/windows/win32/debug/capturestackbacktrace)。
回调栈没有出现第三方模块，并不能排除其他位置的 hook 或驱动影响。

下一步需要验证普通应用启动路径及仍失败的无活动窗口边界，不能把诊断中的
SetActiveWindow 用作远控产品强制操纵外部窗口的修复。没有丢弃首击、点击去重、
AttachThreadInput 或预热点击。长期清单仍为 10/53 完整关闭，43 条未完整关闭。
