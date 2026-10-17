# Linux 操作端输入与正式窗口入口阶段

接续 `5aaeedb`，Linux Tauri 独立窗口已接到进程内 GTK/NV12 显示与原生输入。
本阶段通过原生窗口和 Rust 桥接实测，完整 Desktop 编译通过；尚未通过实际
Tauri 网络连接验收，不关闭 W5 / LT-07.5，也不作为可发布完成状态。

## 实现与行为

GTK 主线程取得宿主 GtkWindow，后台软件解码和有界显示队列保持原有流程。
原生输入使用已有八字节事件接口与 Rust 输入队列，启停模式通过有序确认切换；
窗口退役会让等待中的模式操作失败，避免关闭窗口后继续等确认。失焦、隐藏、
尺寸变化和禁用操作会释放按键与鼠标状态。事件内容不写入日志。

鼠标位置复用渲染的等比例 viewport，处理 GTK scale factor 和 GL 左下原点，
黑边不接受普通点击，拖拽越界夹到画面边界。坐标使用显示方向，不二次逆旋转。
物理键经 evdev/XKB 编码映射到现有虚拟键接口，分别处理左右修饰键。

真实测试发现 XWayland 首次滚轮没有输入包。GTK 3.24.33 XI2 源码中首次读数
用于建立滚动基准；选择平滑事件时，对应的离散滚轮事件又被过滤。现为 X11
窗口选择离散滚轮事件，Wayland 保留平滑路径。X11 触控板滚动因此按档处理。
依据为 [GTK XI2 基准处理](https://raw.githubusercontent.com/GNOME/gtk/3.24.33/gdk/x11/gdkdevice-xi2.c)
和 [GDK 事件过滤](https://raw.githubusercontent.com/GNOME/gtk/3.24.33/gdk/gdkwindow.c)。
修复前失败与修复后通过均保留证据，没有用额外滚轮预热事件掩盖首次丢失。

## 验证结果

测试机器为 Ubuntu 22.04.5 GNOME Wayland 虚拟机 `556688188`，单屏、100% 缩放。
仅创建自有测试窗口，没有替换本机或 VM 安装版，没有改变显示器布局。

- Wayland 与 XWayland 各通过四个画面旋转方向的真实输入：GNOME 虚拟输入设备
  → 窗口系统 → GTK 事件 → 原生事件接口。包括点击、拖拽坐标、物理 A/Shift/Ctrl、
  首次滚轮 -120 和隐藏时释放。每组八个子进程均回收。这里的旋转指显示画面，
  不是四块物理显示器；XWayland 也不代表完整 Xorg 登录会话已验证。
- GTK/WebKit 宿主 ASan/UBSan 测试通过 338 项检查、20 次创建/销毁，原页面状态
  保留、20 个 GL widget 收到析构通知。该组输入断言用合成 GTK 信号，单独记录，
  不混作上面的真实输入测试。
- Rust 图形桥接在 Wayland 和 XWayland 各通过：8 帧生成画面、输入启停确认、
  输入启用排队时退役、旧代次拒绝、隐藏恢复、取消、解码失败、宿主恢复及关闭。
- Linux 媒体库 12 项测试、原生 ASan/UBSan 边界 254 项检查通过；边界包含
  四方向、1/2/3 倍缩放、奇数尺寸、黑边/拖拽和键码。Windows Desktop 编译检查
  通过；完整 Linux Desktop Release 编译通过。编译总并发不超过 10。
- 最后检查自有 native 测试进程为零。VM 账号元数据表明本机已有登录状态，
  可继续准备独立产品测试配置，未输出或转移令牌内容。

源码、产物哈希和任务 ID 见 [结构化证据](remote-desktop-linux-viewer-input-2026-10-17.json)。
实测程序为 `experiments/remote-desktop/verify_linux_viewer_input.py`，接收自有
`pab-linux-host-test` 与 `pab-linux-media-worker-cargo` 路径；需授权的图形会话。

## 未完成项

Linux↔Windows 实际产品网络会话、XRServer 四屏点击、其他键盘布局/输入法、
Linux 硬件解码、完整 Xorg 被控端、登录管理器和持续性能验收继续待处理。
当前输入验证不证明系统登录成功或网络动态画面超过 12 FPS。

VM 的 SVGA 释放诊断仍出现；上一阶段纯 GTK 对照也复现，但不能据此宣称无
显存泄漏。ASan/UBSan 测试未开启 LeakSanitizer，持续资源观察仍属于 W7。
