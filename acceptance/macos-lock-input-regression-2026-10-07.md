# Mac 安装版锁屏输入复验：连续输入重试通过

日期：2026-10-07；设备603527578，macOS 27.0.1 ARM64，已安装1.2.30，原生MCP1.2.32。
用户明确授权使用其提供的密码登录测试。凭据不写入测试文件或文档。

## 最终结果更正

用户要求再次尝试后，使用原生 pab_desktop_input 的 legacy mouse_move、mouse_button
按下/松开并立即发送一个字符，截图首次确认密码圆点出现。随后分开等待再输入时仍未解锁。
再次将鼠标定位、点击、输入与 Enter 确认按顺序连续完成，截图确认成功进入 huayang 桌面。
没有改产品源码、替换二进制或重置权限，因此不能把先前失败直接认定为产品输入回归。

本次同时改变了点击路径（monitor_input → legacy events）和操作间隔，未进行受控对照；
等待期间锁屏重置焦点是可能原因，尚未单独证实。后续应在确认密码框接收输入后立即完成登录，
不能因 AX 窗口引用不可用就判定底层输入不能解锁。

解锁后原生窗口查询 operation 95e51bdf-b4e7-472f-8040-aa609e0b08fd 返回可用窗口引用，
包括此前的 TextEdit 测试文档。实际会话核对 task 59fc172d-8004-4b5c-97a4-8b1a7864205d：
IOConsoleUsers 不再有 ScreenIsLocked/ScreenLockedTime，用户与安全会话保持 huayang/501/100002；
4个输入恢复账本仍全部为零。没有遗留按下状态。

以下保留首次失败过程作为对照，最终登录结果以本节为准。

## 实际结果

通过正式 pixels.pab_connect 和 JPEG 截图确认当前为 huayang 的锁屏密码界面。
屏幕2560×1080；重新枚举显示器取得 input_target，使用其原点/逻辑单位映射点击密码框。
monitor_input 返回 completed，指针观察位置与目标一致，但不代表点击已被锁屏消费。
随后使用正式 pab_desktop_input 的逐键 press/release 输入，接口均返回 applied=true；
截图密码框仍显示占位文字，没有密码圆点。Tab/Enter 也未观察到解锁或界面焦点变化。
没有观察到错误密码提示，不能判断为密码错误，也没有成功登录。

仅重启 Bridge 的 gui/501/com.pixelsagentbridge.session-helper，未注销、重启Mac或关闭用户应用。
重新枚举显示器确认helper_instance改变，再定位密码框并发送一个字符，仍无可见输入。
不能把重启helper或接口返回成功记作锁屏输入恢复。

## 诊断事实

- /dev/console 所有者为 huayang/501；IOConsoleUsers 的 LoginDone、OnConsole 均为 Yes，
  ScreenIsLocked=Yes，安全会话100002。这是已登录用户的锁屏，不是注销后的 LoginWindow。
- 活动输入helper运行在UID501。其日志显示具有模拟输入权限，没有实际输入调用错误。
- 已安装pab-desktop具有 __CGPreLoginApp / __cgpreloginapp section；并未漏打此前预登录标记。
- 只读检查4个当前存在的UID501输入恢复账本：每个259字节，非零槽位均为0。
  本轮没有遗留按下状态；这不等同于直接测量所有系统事件源的修饰键状态。
- 当前代码仅在 root 的活动 LoginWindow 会话选择 combined-session / session event tap；
  用户 Aqua helper 使用 private source / HID tap。上述路由差异需要后续定点验证，尚不能认定为根因。

关键原生操作：首次显示器查询 cca0626b-5ace-4aae-8657-fdce6e56d2c7；
首次密码框点击 e38d16c4-1eed-4dca-b48f-8f19862037e6；
helper重启后点击 2621187e-3283-400f-9a8f-507d8d1cdd7a；
安装标记及进程检查 task c798a1f1-85c7-4fd1-aa94-bd28f2b51ff5；
会话/账本核对 task 208437d9-eb79-433c-92a3-f2c0b79f0483。

## 首次失败时的接续判断（已被上述成功复验更新）

既有1.2.10锁屏解锁及1.2.13注销后登录通过的历史证据仍保留在MACOS.md，
但不能据此把当前1.2.30环境也记为通过。需要排查当前安全输入/锁屏会话的事件路由和系统拒绝原因，
先验证单个非敏感按键在锁屏出现，再使用用户授权凭据完成解锁与桌面输入回归。
没有解锁之前，不进行依赖窗口引用的编辑器Save As与UI验收，不继续重复输入密码。
本轮没有修改产品源码、重置TCC、注销用户或替换已安装二进制。
