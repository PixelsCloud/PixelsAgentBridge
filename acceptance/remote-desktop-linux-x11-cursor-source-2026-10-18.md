# Linux X11 独立光标源阶段记录

日期：2026-10-18。基线 `34bac09`。本记录仅为阶段进展，LT-03.5、W2/W5/W7 未关闭。
机器可读证据见同名 JSON。未更新本机、Ubuntu 或 Apple 的安装版/服务/配置。

## 实现

- X11 新增独立 `cursor SCREEN_ID` 进程，使用现有私有光标包格式和 Rust 视频
  lease、取消、帧序号/配置校验。不是第二套远程认证或网络传输。
- 独立进程拥有自己的 X 连接，避免采集、输入与光标并发使用 Xlib 全局错误处理器。
- XFixes 形状事件触发图像读取，XQueryPointer 采样位置；位置更新不转换/重传图像。
  ARGB32 按 Xlib 的 `unsigned long` 数组逐像素转为预乘 BGRA，不误按 LP64 字节复制。
  图像最大 512×512，校验热点及预乘颜色；保存最新状态，不保留运动历史。
- 屏幕外/其他 root 及全透明形状会隐藏；回到所选屏幕或恢复形状后重新显示。
  250 ms 检查布局变化，发生变化退出旧代次。
- `desktop-separate-cursor` 不再对 XShm 视频混合光标，静态桌面光标运动不需要
  编码新帧。直接诊断 `desktop` 仍保留原嵌入模式作为兼容/对照。
- GNOME 继续使用先前 PipeWire 元数据通道；判断会话时不把 XWayland 当整屏来源。

## 已通过

所有编译总并发最高 8（Linux 4 + Windows 4），嵌套编译上限 1；原生检查单编译器。

1. Linux 媒体 24 项、Windows 媒体 22 项测试通过。完整 Linux Desktop Release 构建
   通过，产物 SHA-256：`f2ff7f78c9d163ac127b43eb5735fa76043ae084c7c0b1d715cbb156fa04ea4c`，
   46,045,256 字节。构建仍有已有 vendor 警告，不是零警告验收。
2. `native_linux_x11_cursor.cpp` 在 ASan/UBSan 下检查 16 个颜色通道、6 种非法形状；
   Xvfb 的实际 XFixes 图像/事件验证移动复用图像、切换形状、透明隐藏、跨 root 和返回。
3. 使用上述完整 Desktop 的 `--media-worker`，`native_cursor_check` 两代次、每代两次
   重开，零视频帧请求仍可读取光标；拒绝无视频/错配置，视频退出后保留的光标句柄关闭。
   `native_cursor_motion_check` 3 次移动仅取最后位置，形状字节为 0，最终读回原位置。
4. 生产 worker pipe 检查拒绝陈旧几何；箭头/十字/透明形状、位置合并、跨 root 恢复通过。
   读取前没有未经请求的历史包；非法光标命令只结束光标，之后改变背景，原视频继续
   输出序号 1 的新画面，解码颜色最大误差 2。父管道 EOF 后约 15 ms 退出，测试子进程
   全部回收。
5. 新视频模式 1024×768 解码得到 2,359,296 个 BGR 颜色通道，与纯色预期最大误差 2。
   原嵌入模式作为正对照，有 287 个通道与背景相差超过 20，确认检查能识别嵌入光标。
   向静态视频给予下一帧额度后，光标移动/换形状未产生视频帧；光标通道正常更新。
6. Ubuntu 22.04 实机 GNOME Wayland 会话 299 使用同一完整 Desktop 内嵌 worker，
   两代次重开、退出回收和 3 次移动合并通过，最终读回原坐标 (777,430)。形状 24×24、
   热点 (4,4)，没有请求编码视频帧；临时二进制目录清理、无新增光标目录。

本次 X11 检查是隔离 Docker 内 Xvfb 的两个 X screen（1024×768、800×600），
不是两个物理显示器，不是同一 RandR root 的双输出，也没有打开正式远程 viewer。
解码使用独立 ffmpeg；它只是测试工具，不加入应用或安装包依赖。

## 发现与未完成

**XFixesHideCursor 未正确反映到独立光标。** 实测 HideCursor 前后，GetCursorImage
返回完全相同的图像、序号，并没有形状事件，当前源仍显示原图。
[X server 实现](https://raw.githubusercontent.com/mirror/xserver/master/xfixes/cursor.c)
将隐藏计数用于 DisplayCursor，但 GetCursorImage 读取当前 sprite，并不查询该隐藏计数。
这与透明图像不同；不能用透明光标通过替代系统隐藏通过。RustDesk 普通 X11 路径也
使用 XFixesGetCursorImage，本轮没有把其实现当作系统隐藏已解决的证据。

后续必须取得实际显示路径的可见性证据，或实现对应平台适配；不能靠猜测超时隐藏、
强制显示或丢输入事件规避。DRM cursor plane 仍未接入，不能关闭整体光标要求。

其他未验收：真实 Xorg 普通桌面/锁屏、多 RandR 输出、热插拔/缩放、形状持续变化与
慢消费者/大图像、正式跨机窗口、帧率/资源曲线及长时间运行。

首次隔离套件的输入检查因缺少 XDG_RUNTIME_DIR 失败；建立仅属于该容器的 0700
运行目录后复测通过。该错误还暴露了运动诊断程序在输入初始化失败时没有等待
异步回收：最初以 Xvfb 作 PID 1 的容器内留下 3 个僵尸进程。已改为错误返回前也
等待 backend idle；在同一无 init 容器故意复现缺目录错误后，无新增进程/僵尸。
随后移除旧容器，在带 init 的全新隔离容器完整复测通过，结束时只有 init/Xvfb。
失败记录保留，没有放宽生产运行目录校验；不把这一诊断程序修复当作整体产品的
长期进程回收验收。

Apple 只读状态复查：`CGSSessionScreenIsLocked=true`，没有测试进程，没有编译进程；
未启动 GUI、未锁屏/解锁，也没有重新申请权限。Mac 源普通桌面实测继续待解锁。

## 复现范围与入口

原生生成/真实 X API 检查：`crates/desktop-media/tests/native_linux_x11_cursor.cpp`
（编译启用 ASan/UBSan，在专用双 screen Xvfb 下传入 `--isolated-display`）。

生产 worker 检查：

```sh
DISPLAY=:91 XDG_SESSION_TYPE=x11 XDG_RUNTIME_DIR=/tmp/private-x11-test \
  python3 experiments/remote-desktop/check_x11_cursor_source.py \
  --isolated-display /path/to/pab-desktop --media-worker
```

此驱动会改根窗口背景/光标并移动指针，只能在一次性的隔离 X server 上运行。
它不会登录、改权限或覆盖安装版；所有通过结论限于上述明确范围。
