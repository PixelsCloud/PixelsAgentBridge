# Linux DMA：会话 broker 与媒体 helper 路由

日期：2026-10-17；基线 `01a537e`。本轮已把 DMA 组件接入 supervisor 和正常
采集的启动后备，增加 Linux 系统 helper 的选择优先级。**实机通过的是生产
broker/用户启动器/采集组件及独立 IPC 路由测试，尚未验收完整 supervisor 服务
到远程窗口、PipeWire 拒绝后的实际自动后备、锁屏登录或 W5。** 原始结果、
代码和产物哈希见 [JSON](remote-desktop-linux-dma-broker-2026-10-17.json)。

## 实现

- 系统 supervisor 在 root 拥有的 helper 目录创建本地 `SOCK_SEQPACKET` socket。
  broker 校验内核提供的 UID、PID、helper 进程组及活动图形会话；不增加远程
  认证步骤或网络监听。使用既有 `PAB_DATA_DIR` 定位运行时通道，无新应用配置。
- `LinuxMediaEnvironment::pin_process` 在前后复核活动合成器身份，并取得 pidfd。
  独立 root `export-session` 进程按 connector 找到该合成器的 DRM 设备；用户
  不能传入任意设备路径，PID 复用也不会把句柄复制请求转到新进程。
- vmwgfx 保留已验证的原合成器 DRM 文件路径，其他驱动使用独立 primary fd。
  图像转换、软件编码仍在普通用户进程执行；控制 fd 不跨到用户 worker。
  其他 GPU 驱动的可用性仍需实机验收。
- supervisor 监督导出进程，在会话退出时先停止并回收导出，再结束 helper。
  下一次采集排队等待旧导出进程退出，避免用户 worker 退出早于导出进程 EOF
  导致立即重开被拒绝。导出进程设置父进程退出信号；继承独立高位 fd，不覆盖
  Rust 子进程的启动错误管道。
- GNOME PipeWire 启动失败时，先销毁失败的采集对象，再尝试 DMA；此时尚未输出
  压缩流头。已经开始的流发生锁屏、熄屏或布局变化仍终止当前代次，未实现
  无缝恢复或登录输入，不能把该后备当作完整锁屏能力。
- Linux 新连接优先选择系统媒体 helper，退出后回退到 GUI helper。优先标记
  只影响已认证提供者的选择，不授予权限。同类保留实际注册顺序；已有流绑定
  原提供者，Windows/macOS 的最新注册选择保持原行为。

## 最终产物与实机检查

最终 Desktop SHA-256：`404f80d3287856e844ac816084b7a0a4f1797e3913d719f7647629e315ce1cfa`。
构建前明确清理 `pab-os-control`、`pab-desktop-media`、`pab-executor`、`pab-desktop`
的 Release 产物，随后顺序重建 Desktop、检查程序和 Executor 测试程序；容器
8 CPU、Cargo 8 jobs、嵌套构建 1 job。会话环境白名单单元检查通过。

这样处理是因为测试快照原来使用保留时间戳的复制方法，可能把构建期间修改的
文件以较早时间戳送入另一组 Cargo feature 缓存。构建脚本已改为内容变化时
重新写入快照。本次最终结论只使用清理后产物；前两轮观察保留为中间记录。
代码字节与最终构建快照逐项一致，哈希保存在 JSON。

设备为 Ubuntu 22.04.5 VM，GNOME Wayland、活动用户 UID 1000、会话 23，1280×800。
测试通过独立 systemd unit 运行 root 包装器，再使用生产 `PreparedUser` 启动
用户 helper；由生产 broker 启动 root 导出程序。每轮采集都使用 Desktop 内嵌
worker，软件解码仅在 VM 内存中完成，没有保存桌面像素。

| 最终检查 | 结果 |
| --- | --- |
| DMA 第一次打开 | 90 帧，89 次采样变化，预热后 14.383 FPS，普通用户编码/解码 |
| DMA 关闭后再次打开 | 90 帧，89 次采样变化，预热后 14.730 FPS |
| 同产物 PipeWire 回归 | 90 帧，89 次采样变化，预热后 13.910 FPS |
| Linux helper 选择 | 实机现有内核身份挑战 + 两个模拟媒体提供者；后注册 GUI 不抢占系统 helper，系统提供者退出后 GUI 接续；1 项通过 |
| 清理 | broker socket、临时 profile、产物目录、导出/采集进程清空；原会话 active/unlocked，安装版服务 active |

三组动态检查均显式要求至少 12 FPS 的变化帧采集/解码速率，不满足就失败。
它们不是网络、真实呈现、1080p、色彩精度、鼠标或多屏的证明。实机 IPC 测试
验证提供者选择和二进制数据流向，使用模拟媒体载荷，不冒充 GUI 产品验收。

安装版 Executor、TOML 配置、service unit 前后哈希一致。本轮没有锁屏、注销、
输入密码、修改安装目录或更新本机 Windows 安装版。各轮测试包装器均正常结束，
没有把观察超时当作进程退出。

## 后续要求

继续在正式 `--media-supervisor` → helper → NativeBackend 路径验收 PipeWire
拒绝后的自动后备；完成唤醒、锁屏输入、GDM、登录和会话切换恢复。DRM 裁剪、
缩放、旋转、光标与真实多屏仍待补齐；网络窗口、九方向、稳定性、三平台打包、
主线集成、COS 和升级验收保持原要求，均不由本记录关闭。
