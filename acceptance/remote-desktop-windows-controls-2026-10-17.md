# Windows 首次激活：标准控件与手势对照

日期：2026-10-17；产品基线 `ff51a6c`。本轮没有修改产品输入逻辑。
延续[首次激活诊断](remote-desktop-windows-activation-2026-10-17.md)，确认重复
消息是否影响实际控件和手势，而不只统计自建窗口的按下次数。

## 结果

90 的单次首次按下会被分发为 `WM_LBUTTONDOWN`、`WM_LBUTTONDBLCLK`，随后
才收到唯一的 `WM_LBUTTONUP`。标准 Button 类和启用 `CS_DBLCLKS` 的手势窗口
都能复现。前两条消息时间戳和注入标记相同。

| 场景 | 90 实际结果 | 判定 |
| --- | --- | --- |
| 标准按钮 | 1 次 BN_CLICKED，但多出双击消息 | 动作一次，消息序列不通过 |
| 自动复选框 | 1 次 BN_CLICKED，未选变为选中，但多出双击消息 | 动作一次，消息序列不通过 |
| 双击手势 | DOWN、DBLCLK、UP、DOWN、UP；第二次按下没有形成预期双击 | 不通过 |
| 拖拽 100 像素 | 2 次开始、1 次结束；终点正确，鼠标已释放 | 不通过 |

正常双击预期为 DOWN、UP、DBLCLK、UP。验收同时检查顺序和动作计数；不能
只看到“双击计数等于一”就宣布成功，也不能用按钮最终状态正确掩盖消息异常。

三种注入方式均复现：移动/按下合为一个 INPUT；移动和按下两个 INPUT 放在
同一 SendInput 调用；先移动、等待 100 ms、再发送不含移动标志的按下。
批量形式对应当前产品按下路径的基本结构。产品真实网络路径仍需独立复测。

本机使用相同最终二进制，按钮、复选框、双击、拖拽及批量双击/拖拽共 6 项
全部通过，均无残留按下。90 的原安装服务在测试前后 State、PathName 和 PID
相同；上传的临时二进制在每轮结束后删除，没有替换安装版或修改系统设置。

## 预激活对照的限制

仅对自有测试窗口调用 SetForegroundWindow 后，90 的双击与拖拽消息序列正常。
但是在调用后和实际注入前，GetForegroundWindow 都没有确认目标成为前台。
因此这两项保留失败状态，不能写成“预激活修复成功”。它提供下一步调查线索，
不是产品中自动抢焦点的依据。初始一轮曾仅检查双击计数，后续已增加严格的
按下/抬起及手势开始次数校验；配套 JSON 保存原始消息并独立比较完整顺序。

已对照固定 RustDesk 提交 `7aa98d43cf1962a7a29ec16ffef42974377ef11e` 的
`libs/enigo/src/win/win_impl.rs`：移动和按下分开通过 SendInput 注入。
本轮分开发送对照仍复现，不能声称照搬这个发送形式就解决问题。

根因仍未确定。不新增输入去重、丢弃首击或抢前台逻辑，不关闭 W2 / LT-04.2。
下一步需要比较窗口线程激活状态与实际前台状态，以及已正常显示过的窗口重新
失焦后再次激活的行为，区分测试窗口首次显示和一般应用操作的影响范围。

## 复现与边界

诊断源码：`experiments/remote-desktop/windows_control_fixture.cpp`。仅操作自身
窗口内的控件；每次运行 5 秒，不录制屏幕或键盘，不安装全局钩子。输入前确认
当前桌面、控件无遮挡、没有按住按钮/修饰键。异常路径释放本次按下。

在 Developer Command Prompt 中以单编译进程构建：

```bat
cl /nologo /std:c++17 /EHsc /O2 /MT /DUNICODE /D_UNICODE experiments\remote-desktop\windows_control_fixture.cpp /Fe:pab-control-fixture.exe /link user32.lib
pab-control-fixture.exe button-self
pab-control-fixture.exe checkbox-self
pab-control-fixture.exe doubleclick-self
pab-control-fixture.exe drag-self
```

每个模式可添加 `separate-`、`batch-` 或 `focused-` 前缀；顺序运行，不并行
注入。`focused-` 只对本程序窗口尝试激活，不操作其他应用。退出码 1 表示
测试条件或结果未满足，须查看 JSON，不得自动重放输入。构建最多 1 核。

本次环境复核：90 仍只有一个活动 Mi TV 1920×1080 输出；XRServer 仍返回
四个 1920×1080 输出，但没有可选的 desktop_user 上下文，本轮未执行四屏输入。
Mac 按用户要求使用单屏验收，不改变其锁屏登录和性能要求。

原始消息、任务 ID、源码和二进制 SHA-256 见
[配套结果](remote-desktop-windows-controls-2026-10-17.json)。测试程序不是产品包内容。
