# Linux 操作端独立光标

2026-10-18，基线 `2157ad3`。接入 Linux 操作端对已有 Windows 独立光标源的
显示；Mac/Linux 被控端独立光标源、真实跨机器窗口、多屏及长期性能另行验收。
W2/W5/W7 保持未完成。

## 实现

- 正式 Desktop 在 Linux 上复用已有光标会话与可靠流、单观看者和认证。
  GTK 主线程负责上传，Rust 的光标 ACK 独立于视频呈现；最多一个待确认提交。
  取消或失败后句柄不可复用，关闭 generation 后排队任务跳过原生对象。
- 在现有 OpenGL 3.2 视频 fragment 合成预乘 BGRA 或 AND/XOR 光标。
  形状只在变化时上传，移动仅改 uniform；不增加桌面 CPU 回读、整帧复制
  或颜色转换。保留原 NV12 纹理，静帧期间光标重绘不增加视频呈现计数。
- 热点按显示像素比例映射，位置使用选定屏幕坐标；GL 的左下角原点转换到
  桌面左上角，处理旋转后的尺寸、缩放和留白裁剪。
- 仅修改 GtkGLArea 自己注册的 input-only 子窗口光标，不改变共享宿主窗口。
  操作开启且独立光标跟踪有效时，在图像区域使用空白本地指针；留白、离开、
  停止操作、跟踪结束、窗口退出和输入失败恢复原指针。

GtkGLArea 的绘制窗口与输入子窗口不同，依据
[GTK 实现](https://github.com/GNOME/gtk/blob/gtk-3-24/gtk/gtkglarea.c) 和
[GdkWindow 光标接口](https://docs.gtk.org/gdk3/method.Window.set_cursor.html)
限定修改范围。测试不依赖 GTK 私有结构布局。

## 实机检查

Ubuntu 22.04 虚拟机、GNOME Wayland 会话、VMware SVGA3D。
分别以原生 Wayland 和 XWayland 启动隔离检查程序；不改变服务、安装版或会话。

| 项目 | 每个后端结果 |
| --- | --- |
| 产品着色器 + 独立 CPU 参考 | 112 组通过 |
| 4 方向 × 2 格式 × 2 缩放，中心、边缘和极端离屏 | 80 组逐像素通过 |
| 比较像素 | 7,864,320 |
| 最大通道误差 | 1，仅透明混合允许量化误差；AND/XOR 必须精确相等 |
| 隐藏/清除/跟踪状态 | 32 组通过 |
| 截断命令 | 80 次拒绝 |
| 自有 widget 指针状态 | 只查看、启用、留白、离开、再进入、停止、源结束和析构通过 |
| Rust GTK 宿主 | 8 帧、光标独立 ACK、无新增视频计数、取消、坏码流、关闭及原内容恢复通过 |

Linux 媒体库 21 项单元测试通过，包含取消光标提交后句柄不可复用、停止后
跳过已排队原生操作。原生检查使用 `-Wall -Wextra -Werror`；实机运行启用
`G_DEBUG=fatal-warnings`。完整 Linux Desktop 构建结果见结构化记录。

GPU 检查使用生成的 NV12 和光标；本地指针检查仅向自有 widget 发送事件并
读取其 GDK cursor 属性，没有向其他应用注入输入。**这些结果不等于 Linux
正式跨机器窗口、物理系统指针显示或 >12 FPS 性能已通过。**

两项诊断保留：

1. 首次测试错误地要求空白光标的类型恒为 `GDK_BLANK_CURSOR`，Wayland 返回
   `-1`，X11 返回 `-2`。产品无需修改；测试改为与同后端通过公开空白光标 API
   创建的参考对象类型比较，并确认只修改自有子窗口、各停止路径恢复原属性。
2. Rust 宿主退出时 VMware 驱动仍输出 `context mismatch in svga_surface_destroy`。
   前阶段原生 GTK 基线也复现过该告警，见
   [宿主记录](remote-desktop-linux-host-2026-10-17.md)。本轮没有忽略退出码或
   GLib 告警；测试退出码均为 0，但不能据此声明 GPU 资源长期稳定性通过。

## 复现与收尾

在具备 GTK3/epoxy 开发依赖的 Linux checkout 中，以一个编译任务构建：

```sh
c++ -std=c++17 -O2 -Wall -Wextra -Werror \
  crates/desktop-media/tests/native_linux_cursor.cpp \
  $(pkg-config --cflags --libs gtk+-3.0 epoxy) -o /tmp/pab-linux-cursor-test
G_DEBUG=fatal-warnings GDK_BACKEND=wayland /tmp/pab-linux-cursor-test
G_DEBUG=fatal-warnings GDK_BACKEND=x11 /tmp/pab-linux-cursor-test
```

程序仅创建自有窗口，必须在可用的用户图形会话运行。XWayland 不代表
原生 Xorg 登录会话验收。两轮实机检查的暂存程序和目录均已清理，活动会话保持。
产物哈希、失败与成功轮次及源码哈希见
[结构化记录](remote-desktop-linux-cursor-render-2026-10-18.json)。

下一步：正式 Linux 独立窗口→Windows 的光标/输入故障隔离，以及 Mac/Linux
独立光标源。后续继续登录、多屏、输入与长期性能清单。
