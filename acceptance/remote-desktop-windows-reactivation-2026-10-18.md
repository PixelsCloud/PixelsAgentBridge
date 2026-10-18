# Windows 点击重复：首次窗口线程激活与再次激活对照

日期：2026-10-18。产品基线 `40799b2`。设备：90，既有 user 执行上下文、session 2。
原始消息、任务 ID、源码/二进制摘要及服务前后状态见
[结构化记录](remote-desktop-windows-reactivation-2026-10-18.json)。

## 新结论与限制

重复按下与目标窗口所属 UI 线程是否已经激活过有关，不能再笼统地描述为
“任何未激活窗口的第一次点击都会重复”。同一线程先使用过另一个窗口后，目标窗口
即使从未点击过，也可以正常处理单击、双击和拖拽；只使用过另一个线程的窗口，
目标线程的首次点击仍失败。精确 OS/驱动根因尚未确定，产品问题仍未解决。

前序有效失败继续保留，见
[激活路径对照](remote-desktop-windows-activation-boundary-2026-10-17.md)。
本轮全部是有界、自有窗口的独立诊断，不能替代正式网络 viewer 验收。

## 实际对照

每个场景都在自己的窗口内预热一次，释放按下、等待超过系统双击间隔，再测目标窗口。
预热和正式计数分开保存，预热中出现的重复没有被算成“修复成功”。

| 目标窗口和线程的先前状态 | 从哪个窗口切回来 | 单击 / 双击 / 拖拽 |
| --- | --- | --- |
| 目标本身已点击过 | 同线程的自有窗口 | 三项均正常 |
| 目标本身已点击过 | 不同线程的自有窗口 | 三项均正常 |
| 目标从未点击；其线程中的另一个窗口已点击过 | 同线程自有窗口 | 三项均正常 |
| 目标从未点击；只点击过另一个线程的窗口 | 不同线程自有窗口 | 三项仍失败 |

失败组的单击为 `DOWN, DBLCLK, UP`；双击为
`DOWN, DBLCLK, UP, DOWN, UP`；拖拽多出一次开始，只有一次结束。
标准按钮最终执行一次动作，但消息序列仍失败。
通过组的双击顺序严格为 `DOWN, UP, DBLCLK, UP`，拖拽一次开始、一次结束。
所有轮次结束时按钮已释放，没有观察到自有窗口中的外部输入干扰。

前三轮共 12 个场景。最终归档源码重新单核编译后，另做三项单击对照：
同线程预热通过、不同线程且目标首次激活失败、目标已预热后跨线程重新激活通过。
该轮使用 Git HEAD 中的公共诊断源码，排除工作区原有未提交 `settings-only`
分支对归档构建的依赖。总计 15 次测量，4 个编译阶段依次执行、每次限制 1 核。

只读重查 90 的鼠标设置：自动窗口跟踪、跟踪提升 Z 序、点击锁定均关闭，与之前
一致。没有修改用户系统设置。

## 诊断实现与恢复

新增 [windows_reactivation_fixture.cpp](../experiments/remote-desktop/windows_reactivation_fixture.cpp)，
复用原有自有窗口注入和事件计数代码。`cross-thread-` 将同伴窗口放到独立 UI
线程；`peer-first-` 只预热同伴，目标不接收预热点击。输入仅允许落在自有窗口，
在活动桌面及按钮/修饰键初始状态满足条件后执行；已注入的按下始终发送释放。
各进程测量循环最长 8 秒，辅助窗口线程最长 10 秒；远端任务总超时 30 秒。

切换自有同伴后，用有界 `WM_NULL` 消息和 `GetForegroundWindow` 验证激活已完成。
跨输入线程的激活可能异步，不能只凭 `SetForegroundWindow` 返回值断言已经激活；
这是 Microsoft 的[窗口激活等待说明](https://devblogs.microsoft.com/oldnewthing/20161118-00/?p=94745)
描述的诊断方法，仅用于自有窗口的测试准备。

初次驱动传入 `desktop_user` 被当前命令工具参数校验拒绝，没有启动诊断任务；
后续使用已存在的 user 选择重新创建测试。失败尝试及其临时上传清理一并保留。
五轮诊断及只读设置检查的临时上传已删除，90 安装服务的 State、PathName、PID 前后一致；没有
替换服务、锁屏、改账号或安装产品，也没有在本机桌面批量注入输入。

复现命令（Developer Command Prompt，顺序运行；只能在获准操作的测试桌面）：

```bat
cl /nologo /std:c++17 /EHsc /O2 /MT /DUNICODE /D_UNICODE experiments\remote-desktop\windows_reactivation_fixture.cpp /Fe:pab-reactivation-fixture.exe /link user32.lib
pab-reactivation-fixture.exe button-self
pab-reactivation-fixture.exe cross-thread-button-self
pab-reactivation-fixture.exe peer-first-button-self
pab-reactivation-fixture.exe cross-thread-peer-first-button-self
```

可将 `button-self` 替换成 `doubleclick-self` 或 `drag-self`。
无参数返回 2，不创建窗口；退出码 1 必须检查失败或干扰原因，不自动重放输入。

## 后续范围

尚未证明跨进程普通应用的同类边界及系统内部根因。不要据此在产品中插入预热点击、
丢弃首击、任意去重、强制前台激活或附加外部线程输入队列。
W2 / LT-04.2 继续保持未完成；清单仍为 53 条中 10 条完整关闭、43 条未完整关闭。
本轮没有产品发布或 COS 更新。
