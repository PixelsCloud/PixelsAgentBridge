# Linux GNOME 采集 worker 阶段验收

日期：2026-10-17。基于 `cac21bf` 后的独立开发分支增量。
源文件、构建产物 SHA-256、实机任务编号和原始统计见
[机器可读记录](remote-desktop-linux-capture-2026-10-17.json)。

本阶段完成 Linux GNOME 采集 worker 与随包软件编解码的实机验证，未完成整个
Linux 远控。尚未注册 Linux 产品媒体 helper，未验证网络、原生显示、输入或登录。
W5 / LT-07 保持未完成。

## 环境与路径选择

- 设备 556688188，Ubuntu 22.04.5、GNOME Wayland、内核 6.8.0-40-generic，
  VMware SVGA II / vmwgfx，2 vCPU。旧记录的 X11 不能代表本次实际会话。
- 默认单屏 Virtual-1 为 1314×742、100% 缩放。测试使用现有 1920×1080 模式，
  只调用临时布局方法；每次都在 finally 中恢复原模式并核对布局。
- 编译在独立 Docker 环境完成，Cargo 8 jobs、嵌套原生构建 1 job；独立 C/C++
  验证使用单条编译命令。VM 未安装编译工具链，未替换服务或 Desktop 安装版。
- 只读 DRM 探针打开自己的 card0 fd，主画面 GETFB2 返回 ENODEV(19)。64×64
  光标平面可 PRIME 导出，不能将它算作桌面采集成功。检查固定 libdrmtap 后，
  确认该调用失败也会中止它的此条路径，故继续实测 GNOME/PipeWire CPU 后备。

## 正式源码实现

`crates/desktop-media/native/linux` 经正式 Cargo build.rs 编译，实验目录不是
产品依赖。使用现有 inventory/capture 私有管道和内置 OpenH264 C ABI，尚未改动
认证、网络协议、终端、截图或既有输入路由。

显示器由 GNOME DisplayConfig 枚举，ID 绑定连接器，不依赖列表序号。记录负原点、
逻辑尺寸、实际像素和缩放；PipeWire 已输出旋转后的画面，因此纹理 rotation 为
identity。采集前复核布局 serial，布局变动、会话关闭或 PipeWire 格式变化均使旧
worker 失败，交由上层建立新代次，不在旧几何信息下偷偷继续。

采集仅保留最新一个待编码的池缓冲，回收被替换的旧缓冲。编码期间持有 PipeWire
loop 锁，避免 remove_buffer 在读取时解除映射；编码结束即归还原始缓冲，在发送
压缩包前释放 loop 锁。传输积压不累积原始帧，也不预先编码无人读取的帧。
检查数据类型、stride、offset、extent、格式、尺寸、损坏标志和包大小。

本条是 BGRx CPU 映射 → 必要的 BT.709 I420 转换 → 软件 H.264 编码路径，明确
不是硬件零拷贝。未来硬件路径仍应直接传递可用资源。静态桌面等待新采集帧，
不会为了达到帧率指标主动复制旧画面；测试以解码后真实变化的画面计数。

## 实测

| 检查 | 结果 |
| --- | --- |
| 初步 PipeWire + 同进程软件编解码，1080p | 69/69 帧解码，缓冲错误 0；有效采样约 12.8 FPS，仅可行性探针 |
| 正式 C++ worker，1314×742 | 90 帧解码，89 次相邻变化，预热后约 13.89 FPS |
| 正式 Cargo Release worker，1080p 全屏动画 | 90 帧解码，89 次相邻变化；预热后实际变化约 **13.74 FPS** |
| 1080p 捕获输出 | 714839 字节 H.264；未保存屏幕像素或视频文件 |
| 原生边界检查 | 28 项通过，AddressSanitizer + UndefinedBehaviorSanitizer 无报告 |
| 实机生命周期 | 8 组通过，9 个子进程全部回收 |
| 测试后状态 | 原分辨率/布局恢复，临时 native 进程 0 |
| 构建 | Linux 正式 Release 成功；Windows 公共媒体包 check 成功 |

生命周期检查覆盖：不存在的屏幕、非法管道命令、等待帧时父管道关闭、不读取视频
时父管道关闭、连续三次开启/关闭、分辨率变化使旧采集退出、恢复后重新采集。
这是同一 VM 上采集/编码和软件解码的组合吞吐，未包含远程网络和屏幕呈现；
不能将 13.74 FPS 标成产品远程桌面最终显示帧率。

验证期间发现两个问题并修正：GNOME Version 的 D-Bus 类型是 int32，初始实现误
按 uint32 拒绝；Python 动态测试窗曾被 GTK 默认绘制覆盖，虽然解码帧数达到 90，
但变化计数只有 1，因此明确判为失败。修正绘制回调后，才记录上述动态性能结果。

## 复现与边界

正式构建：`cargo build --locked --release -j 8 -p pab-desktop-media --bin pab-media-worker`。
需系统开发库 gio-2.0、libpipewire-0.3 及 NASM，现有 Linux Desktop 构建镜像/
安装包已声明相应 PipeWire 依赖。无额外运行时编解码下载。

主动桌面测试使用 `experiments/remote-desktop/verify_linux_worker.py`，临时 1080p
切换由 `linux_capture_1080.py` 包装；生命周期由 `verify_linux_worker_lifecycle.py`
执行。必须在实际用户会话运行，设置其 DBUS_SESSION_BUS_ADDRESS/XDG_RUNTIME_DIR。
测试不保存应用配置，JSON 仅为运行结果，未违反应用配置 TOML 约定。

当前适配实测的 Mutter ScreenCast v4 私有接口，仅在此明确支持范围内启用。
不代表所有 Wayland compositor；X11、greeter、锁屏登录、硬件编解码、奇数像素
尺寸、Linux 真实多屏、长时运行和安装升级均未验收。正式 NativeBackend 仍按原有
协议获取屏幕和帧，后续需连接用户会话 helper、输入和操作端原生视图。

参考资料（独立实现，没有复制 RustDesk 或 libdrmtap 源文件进入产品）：

- [Mutter 42.9 ScreenCast 接口](https://raw.githubusercontent.com/GNOME/mutter/42.9/data/dbus-interfaces/org.gnome.Mutter.ScreenCast.xml)
- [Mutter 42.9 DisplayConfig 接口](https://raw.githubusercontent.com/GNOME/mutter/42.9/data/dbus-interfaces/org.gnome.Mutter.DisplayConfig.xml)
- [PipeWire 0.3.48 视频协商示例](https://raw.githubusercontent.com/PipeWire/pipewire/0.3.48/src/examples/video-play.c)
- [Linux 6.8 DRM UAPI](https://www.kernel.org/doc/html/v6.8/gpu/drm-uapi.html)
- [Linux 6.8 vmwgfx 帧缓冲实现](https://raw.githubusercontent.com/torvalds/linux/v6.8/drivers/gpu/drm/vmwgfx/vmwgfx_kms.c)

固定 RustDesk/libdrmtap 参考提交见 `experiments/remote-desktop/sources.toml`。
