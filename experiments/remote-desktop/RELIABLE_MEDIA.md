# 原生媒体接入可靠流实验

`reliable-media` 将 P1 Windows 媒体链路接到 P2 的真实 `DesktopHost` /
`RemoteDesktopSession`，使用项目的 `PabEndpoint`、`PabConnection` 和可靠分帧。
不是内存假码流，也不是安装版已完成远程桌面的声明。

## 运行路径

```text
Windows DXGI BGRA GPU texture
  -> NVENC direct texture input
  -> bounded compressed H.264 anonymous pipe
  -> DesktopSource / DesktopHost
  -> existing QUIC reliable frames (64 KiB chunks)
  -> RemoteDesktopSession
  -> compressed H.264 anonymous pipe
  -> Windows Media Foundation D3D11 hardware decode
  -> GPU NV12/BGRA conversion into native swapchain
  -> DXGI display confirmation
  -> cumulative Presented acknowledgment
```

管道传输的是压缩码流，不增加原始像素 CPU 回读或中间 GPU 纹理复制。
驱动内部复制仍未知。颜色正确性模式只用于合成测试图，额外回读四个像素；
真实桌面性能模式不做这个检查，也不保存视频、截图或输入正文。

`media_child.rs` 的媒体源按需读取一帧；主机会话的四帧回压控制请求速率。
关闭、切屏、取消以及错误会终止媒体子进程，并异步等待其退出。每个 backend
共享一个 permit，旧子进程未被回收前，新 generation 不会启动第二个采集进程。
进程启动本身不显示控制台；只有明确的原生查看器及 `animate` 测试图窗口可见。

屏幕枚举提供稳定于当前桌面会话的 adapter LUID + output name、负原点、尺寸、
DPI、旋转及主屏标记；启动采集时重新按 ID 查找，并核对实际尺寸。重新选择同一
屏幕会重建编码器、解码器及查看窗口，验证新首帧及旧资源释放。当前两台测试机
各只有一个可枚举屏幕，因此不能把这个结果计为真实双屏切换验收。

## 构建与复现

先按本目录 README 准备固定版本的 NVENC 头文件。编译命令顺序执行；启动前
检查其他构建，确保包括嵌套构建在内总并发不超过 10。

```text
python experiments/remote-desktop/build_stream_media_probe.py
cargo build -p pab-remote-desktop --example reliable-media -j 8
python experiments/remote-desktop/verify_reliable_media.py --binary target/debug/examples/reliable-media.exe
python experiments/remote-desktop/verify_media_window_close.py --binary target/debug/examples/reliable-media.exe
python experiments/remote-desktop/verify_media_window_close.py --binary target/debug/examples/reliable-media.exe --kill-parent
```

实际 target 目录可由 Cargo 环境配置决定。MSVC 脚本为单编译任务，继承最多
10 个逻辑处理器的亲和性；Rust 构建必须与同机其他会话合计预算。

直接运行一个回合：

```text
reliable-media.exe loopback PATH_TO_MEDIA_HELPER 30 150 animate normal restart
```

每代先显示一秒目标帧数作为预热，再统计指定帧数，避免冷启动前积累四帧导致
虚高 FPS。报告记录 DXGI 确认的显示帧数、接收后解码/显示 P95、同连接旧请求
最大耗时。这里的 P95 不包含远端采集和网络延迟，不能称为跨机器端到端延迟。
native helper 运行截止为 90 秒，Rust 实验总截止为 80 秒。

## 独立端点的边界

`client` / `server` 只供隔离网络实验。两端生成临时 QUIC 密钥，客户端先输出
公钥，服务端只接受该公钥；服务端地址 offer 经已有授权通道交给客户端。
它们不读取、复制安装版设备密钥，不注册新设备，不修改服务端准入策略。

临时端点不能自动借用安装版的 Relay 授权。跨 NAT 的正式验收必须把 native
backend 放到正常 `DeviceSessionAcceptor` 路径中，复用现有认证与控制服务给出的
连接授权，而不是给独立探针放宽线上准入。本轮对 90 的独立网络连接尝试超时，
没有跨机器图像通过记录；随后只在两台机器各自验证本地 QUIC 媒体闭环。

## 尚未产品化

- 生产 Executor 尚未注册该实验 backend；Windows 服务 Session 0 到 Default /
  Winlogon helper 的路由、锁屏/登录切换仍需接入和实测。
- 实验 renderer 是固定 960×540 窗口，要求低延迟解码器逐帧产生输出；延迟输出
  明确报错。生产异步解码、动态窗口大小、保比例布局与鼠标映射另行实现。
- 旋转屏幕目前明确拒绝采集/显示；DPI/旋转元数据存在不等于已验证输入映射。
- 静态桌面长时间没有新帧会触发实验超时；生产静帧保留及空闲状态处理待实现。
- 没有软件编码后备、AMD/Intel 路径、macOS/Linux native backend、光标或键鼠输入。
- 没有正式 Desktop 入口、AI 人工协助、打包发布，也没有 Relay/限速条件下的
  视频与终端/文件传输竞争验收。

后续接入顺序：交互桌面 helper 媒体请求/取消路由 → 注册生产 backend → 通过
现有 Bridge 连接打开独立查看器 → 本机接收 90 的真实画面 → 多屏、输入与登录桌面。
