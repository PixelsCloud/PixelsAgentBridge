# Linux X11 系统隐藏光标修复

日期：2026-10-20。基线 `098d31e` 加本批变更。LT-03.5 保持部分完成。

## 结果

修复普通单 X screen（含同一 RandR root 的多个输出）中，应用调用
`XFixesHideCursor` 后远端仍显示光标的问题。新增独立于图像/位置采样的
XRes 隐藏资源计数检查，50 ms 检查一次；最后一次 show 或隐藏客户端退出后
恢复显示。隐藏和恢复不转换、重传光标形状，也不请求视频帧。

复用 [10-18 的 XRes 诊断](remote-desktop-linux-xres-visibility-2026-10-18.md)
关于独立 X screen 无法归属的结论。本次只接入能确定归属的单 X screen；
多个独立 X screen 或缺少 XRes 的服务器保留原形状/位置行为，并在 worker
诊断中明确无法观察系统隐藏。不把该降级记作隐藏功能通过，也不把 XFixes
隐藏计数称为全部 X server 隐藏来源的通用 API。

构建增加 `libxres-dev`，Linux Desktop DEB 的运行依赖增加 `libxres1`。
既有认证、传输和视频采集协议没有变化。没有替换本机或虚拟机安装版。

## 验证

- 正式 Linux release 媒体 worker 构建通过；Cargo `-j 10`，容器 CPU 上限 10，
  嵌套 CMake 上限 1，未与其他实际编译并发。媒体库测试 34 通过、1 忽略。
- 新原生回归在独立 Xvfb、ASan/UBSan、`-Wall -Wextra -Werror` 下通过：
  hide、启动时已隐藏、嵌套 hide/show、两个隐藏客户端、断开恢复、12 次客户端
  退出以及图像复用。100 次稳定采样 11.005 ms，没有新增状态修订。
- 受影响的既有双 X screen 测试通过，透明隐藏、跨 root 和返回、形状转换和
  移动缓存保留。该环境的系统 HideCursor 仍未检测，明确保留该负面边界。
- Ubuntu 22.04.5 的实际 Xorg/X11 桌面，单屏 1306×768，使用正式 worker 管道
  验证：初始可见 → 系统隐藏 → 最后隐藏客户端退出后可见，序号 1/2/3、图像 ID
  始终为 1；首个图像 2304 字节，隐藏/恢复图像均为 0 字节。另一个在已隐藏时
  启动的 worker 首包正确为不可见；重复 hide/部分 show/另一客户端仍在隐藏期间
  没有错误的可见更新。父管道 EOF 后 9.076 ms 退出。
- 虚拟机原服务测试前后均 active；临时上传文件已清理，没有修改服务、显示配置、
  根窗口图像或指针位置。隔离容器已停止，未修改已有私有凭据副本。

worker SHA-256：`d6f0117ef0b769fd6673425afe6d70213ef90b8b74facaeb14cdbf3d4ff20108`。
机器可读证据见同名 JSON。

首次实机驱动在 EOF 处把原生 worker 的错误退出码 1 当作失败；前面的隐藏检查
均已通过。修正驱动为同时核对退出码 1 和 `Parent pipe ended` 诊断，保留首次
失败记录；没有因此修改产品退出逻辑。准备阶段使用错误服务名称的检查在启动
worker 前失败，随后按既有资料更正，未操作任何服务。

## 复现与剩余范围

新原生测试：`crates/desktop-media/tests/native_linux_x11_cursor_visibility.cpp`；
隔离驱动：`experiments/remote-desktop/check_x11_cursor_visibility.py`。
生产管道驱动：`experiments/remote-desktop/check_x11_cursor_visibility_worker.py`。
实机选项会短暂隐藏测试桌面指针，只在已授权测试桌面使用 `--session-display`。

本次不关闭整个 LT-03.5：独立 X screen 的隐藏归属、其他隐藏来源及仍缺少的
动态布局/光标窗口验收继续保留；既有输入和跨平台测试不重新列为未做。

机制参考：[X server 的隐藏资源实现](https://raw.githubusercontent.com/mirror/xserver/master/xfixes/cursor.c)、
[XRes 资源查询实现](https://raw.githubusercontent.com/mirror/xserver/master/Xext/xres.c)。
