# Linux DRM 光标原生导出与读取阶段记录

日期：2026-10-18。基线 `fb118a8`，对应 W2/W5。**不是完整 DRM 光标交付验收**。
代码、构建和实测哈希见同名 JSON；必需项关闭数仍为 9/53。

## 实现与实机发现

现有 Ubuntu 22.04 虚拟机使用内核 6.8.0-138-generic、vmwgfx。独立只读观察 fd
仅开启 universal/atomic 时只返回 8 个 primary plane；声明光标热点能力后返回
16 个 plane，其中当前 CRTC 42 的 cursor plane 36 使用 64×64 ARGB8888，热点
(4,4)，线性 pitch 256。只改变本程序新开的观察 fd，没有改 compositor 的 fd、
DRM master、显示模式、安装服务、用户配置或账号。

这不是“找不到就隐藏”的容错。[Linux DRM UAPI 文档](https://docs.kernel.org/gpu/drm-uapi.html#c.DRM_CLIENT_CAP_CURSOR_PLANE_HOTSPOT)
明确说明虚拟驱动会向未声明热点能力的 atomic client 隐藏光标平面。本轮区分：
找不到可用于目标 CRTC 的光标平面为不可观测；已知可用平面但未绑定目标 CRTC
或无 framebuffer 才返回隐藏。按当前 CRTC 绑定选择，不按 possible_crtcs 猜别屏光标。

原生组件 `Exporter::cursor`：

- 保留屏幕、CRTC、connector、左上坐标和热点；校验有符号坐标、尺寸/格式、
  全幅无缩放/旋转的平面状态，导出后重新核对输出及 plane 属性。
- 仅支持有界 ARGB8888/ABGR8888，最大 512×512、映射范围不超过 4 MiB。
  GETFB2 新句柄按现有规则关闭，只给普通用户传图像 DMA-BUF 与 render fd。
- 不可观测、移动期间快照变化、布局变化或不支持的变换不伪装成有效旧光标。
  调度层仍需处理 Refresh 重取及独立故障隔离。
- 普通用户 `ImageReader` 对线性光标使用 START/END READ 同步与只读 mmap，
  按 offset/stride 取像素；ABGR 才交换红蓝。只处理光标小图，不增加整屏映射/复制。
  其余受支持图像保留 EGL 读取；原视频调用、尺寸约束和路径不变。

首次尝试 EGL 导入此光标失败：`Failed referencing shared surface` / `Cannot import
DMA image`。不是权限未申请。补同步线性映射后 UID 1000 实际读到 16,384 字节，
294 个非透明像素，非透明范围 [0,0]–[17,23]，预乘颜色最大违规值 0。
root 测试进程没有创建 EGL 上下文；消费子进程在降低权限前关闭所有控制/观察 fd。

## 已验收范围

1. `native_linux_dma.cpp` 的 **39 项**边界检查通过 ASan/UBSan，包括原 SCM_RIGHTS
   拒绝分支无 fd 泄漏，以及新增单像素光标、512 尺寸、4 MiB 范围、格式和有符号坐标。
   单像素光标有效不放宽原视频的最小尺寸。
2. 原生真实 GPU 检查：最终代码普通桌面 32 次导出无 fd 数增长，普通用户读取成功。
   画面分辨率为当时实际的 1314×704；光标 plane 左上 (772,425)，热点 (4,4)。
   这是 DMA 光标像素证据，不是与实际 viewer 像素逐点比较或输入定位验收。
3. 锁屏验证遵循用户此前授权，测试结束恢复同一会话 299 的解锁状态。中间版本在
   锁屏下读到可见光标。最终版本第一次锁屏时屏幕熄灭，正确拒绝旧 scanout；该失败
   保留。随后测试夹具仅将 Mutter 的运行时 PowerSaveMode 设回 0（非持久配置），
   最终版本在亮屏锁屏状态连续 32 次报告无绑定光标，正确返回隐藏而非缓存旧图。
   **不把中间版本的可见锁屏结果当成最终版本同一情形已通过。**
4. Linux 媒体库 **24 项**单测通过。现有 DMA 视频实机回归取得 3 帧，SIGTERM 后
   无新输出，exporter 正常退出；原会话/服务保持，临时目录和 worker 清理。
5. 完整 Linux Desktop 构建结果与哈希记录在 JSON。此次改动仅在 Linux 原生实现，
   没有复跑或宣称新增 Windows/macOS 平台验收。编译最高 4 个 Cargo jobs，原生检查单 job。

## 下一段必须完成

当前 `export_loop` 尚只处理原视频请求，普通 `dma::capture` 仍显式报告独立光标
不可用。新增组件未公告为正式锁屏光标能力，不能据本记录关闭 LT-03.5。

`LinuxMediaBroker` 同一实例只管理一个 exporter。不能给光标另开第二个连接，
否则会排队等待视频结束。后续应在现有已认证的私有连接区分视频/光标请求，
串行完成 fd 请求/应答，图像转换和网络输出在连接锁之外进行；保持视频/光标
各自序号、最新状态、取消和故障处理。后台光标采样不依赖视频帧额度。

还需验证：最终可见锁屏光标、锁屏/登录/唤醒切换、负坐标与多输出、热点/缩放、
裁剪/旋转/全局 alpha 等适配、形状变化/同一 framebuffer 内容变化、慢消费者、
正式窗口、长时间 fd/GEM/内存稳定性。非线性 cursor EGL 分支尚无真机证据。
X11 的 HideCursor 问题不因本次 Wayland/DRM 组件测试自动关闭。

可复用的只读探针是 `experiments/remote-desktop/probe_linux_drm_cursor.cpp`；原生
导出/普通用户读取检查是 `crates/desktop-media/tests/native_linux_drm_cursor.cpp`。
二者都不改光标位置或注入按键。锁屏/亮屏属于外部测试夹具动作，原生探针不执行。
