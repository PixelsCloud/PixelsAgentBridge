# Linux 原生显示阶段验收

本阶段接续 `fa67246`，实现 Linux 原生媒体 worker 的 H.264 解码与 GTK/OpenGL 显示。
正式 Tauri 远程桌面宿主尚未接入，W5 / LT-07.5 继续保持未完成。

## 实现与边界

沿用压缩帧管道和现有显示确认协议。OpenH264 软件解码输出 NV12，Y/UV 两平面
直接上传 GL_R8 / GL_RG8 纹理，由 shader 完成 limited-range BT.709 转换。
没有增加 CPU NV12→RGB 转换或解码后第二份完整帧复制；软件解码器内部已有的
I420→NV12 整理和 CPU→GPU 上传仍存在，不能称为零拷贝或硬件解码。

GtkGLArea 按原比例留黑边，支持四方向旋转和缩放。只有发生实际 render 后的
frame-clock `after-paint` 才确认新帧；静态重绘不增加显示计数。这是 GTK 绘制
完成确认，不是物理显示器扫描完成证明。像素读回仅在生成画面的诊断模式启用。

解析限制 16 MiB 压缩包、连续帧序和固定尺寸；异常包不获接受/显示确认。
空闲管道仍处理 GUI 事件。关闭时释放纹理、GL 对象及信号，修复了销毁窗口后
frame clock 已自行移除信号却重复断开的警告。Linux build.rs 显式声明 GTK3、
libepoxy；Desktop 构建镜像与 Debian 依赖清单同步补充 libepoxy。

显示接口参考 GTK 官方 [GtkGLArea 文档](https://docs.gtk.org/gtk3/class.GLArea.html)
及 [缓冲绑定说明](https://docs.gtk.org/gtk3/method.GLArea.attach_buffers.html)。
没有复制 RustDesk 的显示实现。

## 实机结果

环境：Ubuntu 22.04.5 GNOME Wayland，VM `556688188`，2 vCPU，VMware SVGA3D。
数据为 Apple 生成的 1920×1080 H.264 四象限动画，90 帧、15 FPS；没有保存真实
桌面像素。完整 Linux Desktop Release 内嵌 `--media-worker` 的结果如下：

| 检查 | 结果 |
| --- | --- |
| 0° / 90° / 180° / 270° 原生 Wayland 显示 | 14.952 / 14.990 / 14.984 / 14.992 FPS |
| 四方向图像校验 | 各 90 帧、360 个像素检查，最大 RGB 误差 2 |
| 真实 1920×1080 全屏 drawable | 14.982 FPS，90 帧、360 检查，最大误差 2 |
| 四方向静态缩放/重绘/关闭 | 各仅计 1 帧、16 检查，最大误差 1 |
| 异常数据 | 超大包、半包 EOF、错误帧序、无效 H.264、缺首个 IDR、尺寸不符，共 6 项通过 |
| XWayland 缩放/最小化/恢复/关闭 | 1 帧、16 检查通过，无 GLib 警告 |
| 原生边界测试 | 76 项 ASan/UBSan 检查通过，`-Wall -Wextra -Werror` |
| Windows 完整 Desktop 编译检查 | `cargo check --locked ... -j 8` 通过 |
| Linux 发布产物校验回归 | 5 项测试通过 |

旋转测试 drawable 为 960×600；单独全屏测试才使用 1920×1080。全屏测试临时
调整 VM 模式，结束后核实原 1314×742 模式和布局已恢复。最终自有 native 进程为 0。
以上约 15 FPS 只覆盖解码/显示，不包含真实采集和网络，不能替代端到端性能验收。

完整 Desktop 产物 44,958,792 字节，SHA-256：
`33f8257846d49bc2f33b9f026e0b7ef5f8388fa9d14feaa3a976fc631ce5014c`。
构建串行调度，Cargo 8 jobs、嵌套 CMake 1 job，Windows `/MP1`。
任务 ID、逐项结果及源码/产物哈希见 [结构化证据](remote-desktop-linux-renderer-2026-10-17.json)。

## 未完成项

原生 Wayland 最小化状态观察超时，未记通过。[GTK 3.24.33 Wayland 实现](https://raw.githubusercontent.com/GNOME/gtk/3.24.33/gdk/wayland/gdkwindow-wayland.c)
发送 `set_minimized`，没有可供该测试等待的 iconified 状态更新。后续需要从合成器
或正式窗口操作验证实际最小化/恢复；此次单独的静态重绘测试明确不含最小化。
XWayland 测试使用原 GNOME Wayland 会话，不能冒充完整 Xorg 会话验收。

还需 Linux Tauri 主线程内嵌视图、操作端键鼠采集、硬件解码/DMA-BUF、网络产品
闭环、X11 采集、系统登录、多屏和持续性能。现有 headless 发布 sysroot/运行库
也须在 W8 整合时核查，不能仅凭 Desktop 编译宣称 CLI 安装包已兼容。
未更新本机或 VM 安装版、未替换服务、未发布候选包。
