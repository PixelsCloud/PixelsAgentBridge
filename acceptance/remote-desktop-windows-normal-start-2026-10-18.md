# Windows 普通启动窗口的输入对照

日期：2026-10-18；基线 `864bbad`；90 的既有 user 上下文 / session 2。
接续[队列与跨进程诊断](remote-desktop-windows-input-queue-2026-10-18.md)。

新增诊断模式 `external-normal-show-`，仅把自有窗口的首次 ShowWindow 参数
改为 `SW_SHOWNORMAL`。不调用 SetActiveWindow、SetForegroundWindow，不发送
预热点击；输入仍由独立子进程通过 SendInput 发往父进程自有窗口。

Microsoft 的 [ShowWindow 文档](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-showwindow)
区分了正常显示与不激活显示。实际观测比仅推断 API 意义更具体：普通启动后
目标线程活动窗口已建立，但目标仍不是前台；输入前前台仍不是目标。
STARTUPINFO flags 为 256（使用标准句柄），未设置 USESHOWWINDOW，wShowWindow=0
不覆盖本次 ShowWindow 参数。

| 模式（同一最终二进制） | 单击 | 双击 | 拖拽 |
| --- | --- | --- | --- |
| 普通启动 | DOWN, UP；一次按钮动作 | DOWN, UP, DBLCLK, UP | 一次开始/结束，终点 (250,80) |
| 原 SW_SHOWNOACTIVATE，线程此前无活动窗口 | DOWN, DBLCLK, UP；多一次按下 | DOWN, DBLCLK, UP, DOWN, UP | 两次开始、一次结束 |

两轮共 12 组结果一致。最终源码保留原有严格验收条件，普通启动三项通过、原对照
三项失败；没有修改原失败门槛。第一轮曾额外输出重复的 normal-start 汇总，第二轮
删除该汇总后重新编译和执行六组检查，所有原始结果与二进制哈希均保留。
每组输入来源标记、未被外部输入打断及结束左键抬起均验证；临时程序删除，安装服务
的状态、命令行与 PID 前后相同。编译顺序执行，每次固定一核。

结论：不能把该异常表述为所有普通应用的首次点击都会重复；已证实的失败边界是
未建立线程活动窗口的不激活启动路径。**这一边界仍未解决**，本轮也没有经过产品
viewer 或网络输入链路，因此不关闭 LT-04.2，不修改产品输入行为。

入口：[windows_input_trace.cpp](../experiments/remote-desktop/windows_input_trace.cpp)。
构建时共享 fixture 从 Git HEAD 归档，未包含工作区其他会话的 settings-only 改动。
数据：[结构化记录](remote-desktop-windows-normal-start-2026-10-18.json)。
