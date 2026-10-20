# Linux DRM：原生导出、用户态编码与锁屏边界

日期：2026-10-17；基线 `1d1b700`。本轮将上一轮单帧实验推进为 `desktop-media`
中的原生组件，**尚未连接 supervisor 与普通 NativeBackend 的自动选择，不能
从产品窗口使用此后备，也不代表完成登录或 W5。** 详细构建、原始结果及恢复
证据见 [JSON](remote-desktop-linux-dma-native-2026-10-17.json)。

## 实现

- `drm_export.h`：root 导出选定 connector 的主平面 DMA-BUF。通过自身独立的
  DRM 观察 fd 开启 atomic/universal 属性查询；不修改借用的合成器文件的
  capability，不申请/释放 master，不设置显示模式。借用路径限定 vmwgfx。
- `dma_frame.h`：私有继承式 `SOCK_SEQPACKET` 通道，通过 SCM_RIGHTS 仅传递
  图像和对应 render-node fd；不传递合成器控制 fd 或 CPU 像素。固定大小版本、
  序号、格式、尺寸、步长、文件长度和 ancillary 数量检查，接收 fd 为 CLOEXEC。
- `dma_image.h`：普通用户进程复用 GBM/EGL context、texture、FBO 和像素数组，
  逐帧导入图像。支持时直接读取 BGRA，避免额外 RGBA→BGRA 转换；不支持时才
  原地交换通道。VM 当前使用软件编码，GPU→CPU 读回与编码器的 YUV 转换仍需要。
- `dma_capture.h`：接入原有压缩 worker 管道和软件编码器，按需求取一帧，无原始
  帧积压。GNOME 电源、锁屏状态与布局在读取/编码前后检查；熄屏或状态变化终止
  当前代次，防止把先前桌面当作锁屏图继续发出。
- 增加内部 `export-dma` / `capture-dma` 入口，前者要求 root，后者拒绝 root。
  正常 `capture` 和网络认证/传输未修改。还没有可供任意进程连接的 broker 或
  新监听端口；通道仅由本轮隔离测试父进程显式建立和传递。
- Linux 构建显式依赖 `gbm` / `libdrm`，打包依赖补充 `libdrm2` / `libgles2`。
  未创建或发布安装包。

## 实机结果

设备为 Ubuntu 22.04.5 VM、内核 6.8.0-138、GNOME Wayland，会话 23，1280×800，
2 vCPU。原安装版服务一直运行；测试产物放在唯一临时目录，未覆盖安装目录。
本地构建总并发最多 8，原生边界测试使用 1 CPU、一个编译进程。

最终 worker SHA-256：
`eb7857d15df224b3fff83d555824729513c5b41c9030444e5afd00322a8c914e`。

| 最终构建检查 | 结果 |
| --- | --- |
| 新 DMA 动态链路 | root 导出，UID 1000 转换/编码/解码；90 帧、89 次采样变化，预热后约 15.006 FPS，`direct_bgra=true` |
| 原 PipeWire 路径回归 | 同一产物与动态测试；90 帧、89 次变化，预热后约 13.754 FPS |
| 采集进程 SIGTERM | 先完成 3 帧，终止采集后 root 导出进程沿 EOF 以 0 退出，无残留 |
| 锁屏代次边界 | 先完成 3 帧，确认 LockedHint=yes 后再请求；采集以“Display is asleep; refusing stale scanout”退出，边界之后输出 0 字节；root 导出正常退出 |
| 原生 descriptor 边界 | ASan/UBSan 下 28 项通过，包括截断/超长数据、少/多/截断 ancillary fd、错误代次、尺寸越界、文件长度不足；拒绝前后 fd 数量相同 |

动态数字包含本地采集、编码和解码，不包含网络、渲染或真实合成器呈现；分辨率
是 1280×800，不能冒充 1080p 指标。测试没有保存屏幕像素；变化检测不代替完整
色彩精度、光标、多屏或输入验收。15.006 是有限采样窗的测量结果，不表示配置
超过 15 FPS。

锁屏测试沿用用户已有授权，每轮先设置独立 45 秒自动解锁定时器，finally 中
恢复原会话并停止定时器。没有发送键鼠或密码。本轮只证明旧代次停止，未测试
新代次在锁屏界面启动，也未通过新路径完成登录。

## 发现和修正

初次实机测试被主平面布局校验拒绝，失败原始报告保留。进一步只读查询确认，
没有开启 atomic capability 的观察 fd 只能看到 `type`、`IN_FORMATS`，无法
取得 SRC/CRTC 属性。改用独立观察 fd 查询，保留合成器原有 capability 后通过。
同时改为从主平面取得 framebuffer，避免依赖旧式 CRTC buffer 字段；GETFB2
遇到 ENOENT/EAGAIN 时最多重新观察三次，每次释放自己的句柄，不重新发送请求。
其他错误不重试。成功导出前显式检查新 GEM handle 的释放结果。

最终四项实机检查都绑定上述同一个 worker；此前成功构建及失败构建的结果也
单独保留，没有混为同一产物的证明。

## 恢复和后续

最终会话 23 active/unlocked，PowerSaveMode=0；安装版 Executor、TOML 和 service
unit 的哈希与此前基线一致。临时目录、传输文件、进程、定时器和媒体 runtime
均为零。未更新本机 Windows 安装版、未更改主工作树。

下一步需要把此组件接入 root supervisor：通过已有活动会话/进程身份观察创建
继承通道，保持 root 控制 fd 只在导出进程内，按 session/PID 代次监督退出；再
接入 NativeBackend 的选择与恢复。还需唤醒和登录输入、GDM、新会话及真实窗口
验收。目前裁剪/缩放/硬件旋转的 DRM 主平面会明确拒绝，避免错屏；这些能力、
光标和多屏仍必须补齐，不能据此降低 W2/W5/W7 的要求。
