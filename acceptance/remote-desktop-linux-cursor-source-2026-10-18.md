# Linux 独立光标源阶段验收（2026-10-18）

状态：GNOME Wayland 源已实现并通过真实 worker 生命周期、移动和合并更新检查；
**没有完成正式远程窗口、X11 或锁屏 DMA 光标验收，W2/W5/W7 保持未关闭。**

## 实现与边界

- 复用已有 PipeWire 屏幕流的 `SPA_META_Cursor`；不创建第二套屏幕采集、不再读取
  一份桌面图像。GNOME version-4 适配器选择 `cursor-mode=2`，视频不嵌入光标。
- 在回收 PipeWire 缓冲前，提取有界形状/热点/位置。RGBA 预乘颜色转为协议所需
  BGRA，仅转换光标图片；位置更新共享同一张不可变形状，不复制其像素。
- GNOME 的光标专用缓冲像素 chunk 长度为零，立即归还，不能覆盖等待编码的视频帧。
  视频和光标分别读取，光标移动不会消耗视频信用或增加已显示视频帧计数。
- 每个采集会话一个 Unix socket，放在新建的 0700 私有临时目录中；socket 为 0600，
  只接受启动 capture worker 的父进程 PID/UID。外部连接仍沿用既有认证与可靠通道，
  无新账号、密码、端口或配置。只有一个当前光标快照，不保留移动历史。
- Rust 复用现有会话/代次验证和有界光标包解析。视频结束时，即使调用方仍持有
  空闲光标句柄，也取消读取、释放占用并清理临时目录。
- X11 暂时仍使用原来的内嵌光标，**不宣告独立源已完成**。受保护桌面的 DMA
  后备仍走原有采集，独立光标通道明确失败并让操作端清除叠加/恢复本地指针；
  不伪造“光标已内嵌”，DRM cursor plane 支持仍需开发与验收。

解析依据 [PipeWire 光标元数据定义](https://pipewire.pages.freedesktop.org/pipewire/meta_8h_source.html)
与本次对应版本的 [GNOME Mutter 42 实现](https://github.com/GNOME/mutter/blob/gnome-42/src/backends/meta-screen-cast-stream-src.c)、
[显示器流实现](https://github.com/GNOME/mutter/blob/gnome-42/src/backends/meta-screen-cast-monitor-stream-src.c)。
特别处理了只有位置的消息将 hotspot 字段置零、空 sprite 的全零 bitmap、以及
Mutter 的 bitmap 为预乘 RGBA。此适配器不代表所有 Wayland compositor 已兼容。

## 构建与生成数据

- Windows 媒体库单测 22 项通过；Linux 媒体库单测 24 项通过。
- Linux 完整 Desktop 构建通过（4 jobs，嵌套构建 1）；与 Windows 4-job 回归重叠时
  总编译预算为 8，未超过 10。完整 Desktop SHA-256：
  `05647785f6839739ad41bb92af6075a29974f4a5de31176420c766ce5cdfced9`。
- ASan/UBSan 原生生成测试通过：48 个颜色通道逐值校验，113 组截断、偏移、尺寸、
  格式、非预乘颜色异常拒绝；验证行间 padding、负原点/2 倍缩放、未对齐元数据、
  只移动时形状/热点保留、空 sprite 后仅移动不会恢复旧光标。
- 最初编译有两处新增警告（析构缩进、Linux 未使用的 mut），已修正；最终媒体库
  编译没有这两条警告。完整 Desktop 仍有此前第三方库警告，未宣称零警告。

## Ubuntu 实机

Ubuntu 22.04，GNOME Wayland，活动用户 hy / UID 1000，会话 299。通过现有 SSH
在新建临时目录执行，未更新已安装程序、服务、账号或配置。确认用户桌面未锁屏。

第一次独立 worker 检查通过后，又使用完整 Desktop 的 `--media-worker` 重测：

1. 未开始视频前请求光标被拒绝，旧屏幕配置被拒绝。
2. 两个连续视频代次，每代次两次打开光标，均取得实际 24×24 预乘 BGRA 形状，
   热点 (4,4)、位置 (777,430)。编码视频帧请求数为零；PipeWire 初始原始帧仍需
   启动并提供元数据，不能表述成“没有进行屏幕采集”。
3. 单独重开光标可用；保留空闲光标 handle 后销毁视频，等待 backend idle 成功，
   旧 handle 及旧配置再次读取均失败。没有残留 worker、临时 socket 或目录。
4. 启动原有 GNOME 输入通道，只发送三个移动事件，不点击、不发按键。在这段期间
   不读取光标、不请求编码视频帧；随后 sequence=2 直接返回最终 (807,430)，
   shape payload 为 0，没有依次回放三个位置。
5. normalized input 的整数参考为 (806,429)，实际为 (807,430)，符合该检查明确
   允许的一像素舍入差，不能写成逐像素完全一致。恢复指针后读取到 (777,430)，
   与测试前完全相同；进程和临时目录再次清理确认通过。

实机驱动为 `native_cursor_check` 和新增的 `native_cursor_motion_check`；后者明确
要求空闲桌面，含移动和恢复动作。上述是 **NativeBackend + 正式内嵌 worker** 检查，
没有使用网络连接或打开正式远程查看窗口，不能扩大为端到端产品验收。
完整哈希、原始测试输出与清理状态保存在配套 JSON。

## 仍需完成

真实自定义/隐藏光标、视频无双光标的图像比较、正式跨平台窗口、源故障和重开、
慢读/大形状背压边界、CPU/帧率，以及 X11 与锁屏 DMA 光标源。旧版本发行版、
多屏/混合 DPI 和持续运行矩阵保持待验收。此前 GTK/ATK critical 告警也仍待定位。
