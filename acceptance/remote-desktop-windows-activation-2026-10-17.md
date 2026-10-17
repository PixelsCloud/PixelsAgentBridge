# Windows 首次激活重复按下：消息来源诊断

日期：2026-10-17；产品基线 `e493928`。90 原安装服务始终运行；本轮没有替换
服务或产品文件。独立 Win32 程序在 Administrator session 2 创建自己的窗口，
仅向自身未激活窗口中心发出一条合并移动/按下 INPUT，随后一条抬起 INPUT。
程序 5 秒退出；每次编译限制为 1 核，无并行编译。

## 新证据

- 原生 Windows 10.0.20348 上仍稳定复现两次 WM_LBUTTONDOWN、一次 WM_LBUTTONUP。
- 两条按下均来自 DispatchMessage，具有同一消息时间戳、同一 dwExtraInfo=100；
  GetCurrentInputMessageSource 均报告 IMDT_MOUSE / IMO_INJECTED。
- 有界低层鼠标观察仅记录自己的标记、自己的窗口上的按钮事件；实际只看到
  **一次低层按下、一次低层抬起**，随后窗口队列分发两次按下。
- 读取测试进程模块列表，未观察到第三方 DLL；这不能排除进程外软件或驱动影响。
- 同一初始诊断二进制在本机 Windows 上只出现一次按下、一次抬起。

因此新增证据把重复定位到该环境的窗口消息分发阶段，不能解释为产品协议重复
发送。仍不足以确定具体 OS 缺陷、驱动或外部组件，也不等于所有真实应用都会
执行两次点击动作。产品没有增加去重、额外预激活或丢弃首击。

## 对照

| 自身测试窗口变体 | 90 结果 |
| --- | --- |
| 默认 DefWindowProc | 2 次按下 |
| WM_GETOBJECT 直接返回 0 | 2 次按下 |
| WM_MOUSEACTIVATE 显式返回 MA_ACTIVATE | 2 次按下 |
| 将轮询 Sleep 替换为 MsgWaitForMultipleObjects 等待消息 | 2 次按下 |

MA_ACTIVATE 的定义是不丢弃激活点击；参见
[Microsoft 窗口激活说明](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-mouse-input#window-activation)。
注入来源的枚举定义参见
[INPUT_MESSAGE_ORIGIN_ID](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ne-winuser-input_message_origin_id)。
这些定义只用于解读诊断，不构成平台缺陷根因的证明。

## 可复现工具与边界

保存诊断源码 `experiments/remote-desktop/windows_activation_fixture.cpp`，不加入
产品构建。使用 Developer Command Prompt 单编译进程构建：

```bat
cl /nologo /std:c++17 /EHsc /O2 /MT /DUNICODE /D_UNICODE experiments\remote-desktop\windows_activation_fixture.cpp /Fe:pab-activation-fixture.exe /link user32.lib
pab-activation-fixture.exe combined-tagged-self
pab-activation-fixture.exe combined-tagged-nouia-self
pab-activation-fixture.exe combined-tagged-activate-self
pab-activation-fixture.exe combined-tagged-responsive-self
```

必须位于获准测试的已登录桌面。程序先确认自身窗口无遮挡且没有按住按钮/修饰键，
才输入；不记录键盘内容，不阻断鼠标事件，退出时移除观察钩子。未提供 `-self`
模式会拒绝运行。上面的各命令应顺序执行，不并行注入。

原始消息、远端 task ID 和本机对照保存在配套 JSON。变体源码逐次编译并上传，
最终归档将 include 改成仓库相对路径并限制为显式有界自测模式。
归档源码已单核重新编译；无参数启动按预期返回 2，没有打开窗口。
所有远端诊断进程和 4 个上传二进制已清理，原服务 Running、路径不变。
此问题保持未解决，不能据此关闭 W2；后续仍需标准控件实际动作、拖拽和双击验收。
