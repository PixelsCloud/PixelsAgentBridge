# Linux VMware：Wayland 桌面与锁屏的 EGL 读回实验

日期：2026-10-17；工作树基线 `34aad34`。**已取得真实桌面和唤醒后的锁屏画面，
尚未接入产品，不代表系统登录、帧率或 W5 通过。** 机器数据、构建哈希、失败
尝试与恢复状态见 [JSON 记录](remote-desktop-linux-vmware-egl-2026-10-17.json)。

## 定位结果

VM 为 Ubuntu 22.04.5、vmwgfx、GNOME Wayland，内核 `6.8.0-138-generic`。
包管理器报告源包 `linux-hwe-6.8 6.8.0-138.138~22.04.1`。从 Ubuntu 发布记录
取得对应 `.dsc` 和约 10 MB 的 `.diff.gz`，补丁 SHA-256 与 `.dsc` 一致。
相关补丁应用到上游 v6.8 文件时逐行核对每个 hunk 的上下文；没有下载完整
原始源码包，也没有用 v6.12 代替此运行内核作结论。

该 Ubuntu 补丁为 surface framebuffer 增加 `create_handle`，导出其 backing BO
的 GEM handle；PRIME 导出会找到对应 surface，再从调用者的 TTM 文件对象查找
surface 引用。独立 DRM 文件缺少这个引用时，可能返回 ENOENT。先前直接 mmap
得到黑帧，不能证明屏幕本身是黑色；CPU 同步也不等于执行 GPU surface 读回。

实机对照验证了这个方向：同一个活动 framebuffer，独立 card0 fd 的 GETFB2
成功、PRIME 返回 ENOENT；复制当前 GNOME Shell 的 card0 fd 后，PRIME 成功，
导出大小 4,096,000 字节的 DMA-BUF。随后在独立 GBM/EGL 上下文导入，并用
`glReadPixels` 读出 1280×800 真实画面。

来源：

- [Ubuntu 此版本源码发布记录](https://api.launchpad.net/1.0/ubuntu/+archive/primary/+sourcepub/18687679)
- [对应源码包描述与校验和](https://launchpad.net/ubuntu/+archive/primary/+sourcefiles/linux-hwe-6.8/6.8.0-138.138~22.04.1/linux-hwe-6.8_6.8.0-138.138~22.04.1.dsc)
- [上游 v6.8 vmwgfx](https://kernel.googlesource.com/pub/scm/linux/kernel/git/torvalds/linux/+/refs/tags/v6.8/drivers/gpu/drm/vmwgfx/)
- RustDesk 本地 `build.py` 固定的 [libdrmtap 提交](https://github.com/rustdesk-org/libdrmtap/tree/5da68a3a368db569716d0d0f11cefacbb11b2290)：参考其独立导出与 EGL 转换思路；本实验未引入该库，也没有把借用合成器描述符声称为 RustDesk 已验证的 VMware 路径。

## 五次有界尝试

| 尝试 | 结果 |
| --- | --- |
| 初版 EGL reader | PRIME 成功，但 GBM 上请求 PBUFFER 配置失败；无像素结果 |
| GBM WINDOW 配置、无窗口的 EGL context | 桌面读回成功；独立 VIX 截图 1,023,862 / 1,024,000 像素完全相同（99.9865%），差异仅顶栏时钟 |
| 锁屏后直接读取 | LockedHint=yes，但拿到旧桌面；**失败**，不能把非黑像素当作锁屏成功 |
| 锁屏后只设置 PowerSaveMode=0 | 锁屏前为 0、锁屏后为 3；设置回 0 后观察到 framebuffer 变化/ENOENT，独立 VIX 仍是黑屏；**失败** |
| 锁屏后一次临时键盘 Shift 按下/抬起唤醒 | PowerSaveMode 从 3 回到 0，仍 LockedHint=yes；读取到真实时间/日期锁屏界面，独立 VIX 对比 1,022,851 / 1,024,000 像素相同（99.8878%），差异位于锁屏提示的动画区域 |

最后一次 probe 每次 GETFB2 前重新查询 CRTC framebuffer，避免沿用最初 inventory
中的旧 ID。当前仍是单次快照，未实现生产所需的逐帧重试和代次保护。

早期 reader 接入时，Python 外层的 `pixels_read` 字段仍硬编码为 false；保留
原始报告中的矛盾，不修改历史证据。对应子进程退出码、像素统计、图片和独立
VIX 比较共同证明读取发生。最终锁屏实验已改为依据子进程结果记录此字段；
这个字段仍不等于画面内容或登录验收通过。

## 工具、权限与恢复

- [导出探针](../../experiments/remote-desktop/linux_vmw_export_probe.py)：仅允许 root、
  指定 VM 的活动 UID 1000 GNOME Wayland 会话；锁屏检查必须显式传
  `--expect-locked`。工具本身不锁屏、不发送输入。
- [EGL reader](../../experiments/remote-desktop/linux_dma_egl_probe.c)：继承只读导出的
  DMA-BUF，使用自身 render-node/GBM/EGL 上下文读取单帧；无绘制、模式设置或
  显示器绑定操作。像素文件 0600、独占创建、禁止跟随符号链接。
- 在现有 Linux 容器内单个 C 编译进程、容器 1 CPU，以
  `-O2 -Wall -Wextra -Werror` 和 `epoxy gbm libdrm` 构建通过；无并行编译。
- pidfd 复制的描述符共享合成器 DRM 文件。GETFB2 通过 `drm_gem_handle_create`
  新建的 handle 由探针逐一关闭，未关闭合成器原有 handle，也未申请或释放 DRM
  master。各轮 GEM close 错误为零，master 状态保持。
- 锁屏测试沿用用户明确授权；每轮先建立独立 systemd 60 秒解锁定时器，正常路径
  在 finally 中解锁，再停掉定时器。最后一轮临时 uinput 设备仅发送 Shift
  按下和抬起，随后销毁；未提交系统密码。
- 原服务始终 active。最终会话 23 为 active/unlocked；安装二进制、TOML、服务
  unit 哈希与既有基线一致。临时目录、定时器、进程、键盘和媒体 runtime 均为零。
  像素仅留本地忽略目录，不进入仓库。

## 正式接入仍需完成

1. 通用 DRM 导出优先，vmwgfx 特殊路径限定驱动与活动会话；精确核对 PID、UID、
   seat、设备节点和代次。借用的合成器控制描述符不能交给普通采集 worker。
2. 特权部分只导出，DMA-BUF 经已认证本地 IPC 交给非特权转换/编码部分；持续
   EGL 资源复用、帧切换和严格回收，而非每帧重新启动进程。
3. 熄屏时停止发送上个桌面。恢复需要可靠的唤醒、重新枚举和新帧确认；仅设置
   电源状态不是已验证的替代方案。先前缓存帧不应跨锁屏/登录代次显示。
4. 验证真正的密码界面、输入登录、GDM 新会话、用户切换；本轮只恢复原会话，
   没有通过产品完成登录。还需正式独立窗口、>12 FPS、多屏和稳定性验收。

这条实验使先前“VMware 无法读出 Wayland 主画面”的阻碍缩小为可实现的产品
接入工作；W5、W7、W8 继续保持未完成。
