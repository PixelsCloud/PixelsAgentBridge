# 远程桌面卡顿诊断验收

两端分别记录候选版本、系统、连接路径、会话及 generation。使用专用账号和合成内容；证据存放在忽略的 `smoke/runs/`。不保存真实桌面、密码、剪贴板或按键内容。

## 自动检查

遵守整机总构建并发不超过 10，准备 BUILDING.md 要求的原生工具链。Windows 需要 NASM；CMake 必须支持实际安装的 Visual Studio，嵌套 CMake/MSVC 使用单作业。已有统一 smoke Cargo 适配器通过 `native_env.py` 准备环境。

```sh
cargo test -p pab-remote-desktop --lib --test reliable_sessions -j 4 -- --test-threads=4
cargo test -p pab-executor --lib -j 4 -- local_ipc::media:: --test-threads=1
cargo test -p pab-desktop-media --lib -j 4
cargo build -p pab-desktop-media --bin pab-media-worker -j 4
```

在 Windows/macOS 的交互桌面执行（macOS 的 worker 路径没有 `.exe`）：

```sh
python smoke/remote_desktop_worker.py --worker target/debug/pab-media-worker.exe --software --output smoke/runs/rd-metrics/software.json
python smoke/remote_desktop_worker.py --worker target/debug/pab-media-worker.exe --output smoke/runs/rd-metrics/automatic.json
```

检查同一编码器交替返回旧帧／带耗时帧、调码率后的帧序列、H.264 起始码、PTS 单调性、等待及编码耗时。自动选择编码器可能回退软件，不能据此声称硬件编码通过。脚本只生成合成像素，不采集或保存桌面内容。

验证已解包的 Windows 安装包时，将 `--worker` 指向其 `pab-desktop.exe` 并增加 `--worker-arg=--media-worker`，直接测试安装包内嵌的原生 worker；不需要安装或启动正常桌面界面。

前端在 `apps/desktop` 运行 `npx tsc --noEmit` 和 `npx playwright test tests/remote-desktop.spec.ts`。夹具验证中英文统计、过期采样、旧 generation 隔离、2048 字节原生面板限制及刷新统计不重启视频；不替代原生显示验收。

## 两端实测

| 场景 | 必须观察到的结果 |
|---|---|
| 静止桌面 60 秒 | 接收／显示 FPS 可以为 0；无待显示帧时属于等待变化，不判定解码卡死；输入后恢复画面。 |
| 连续播放合成动画 60 秒 | 接收、提交、显示帧号持续前进；有实际 FPS、压缩视频码率、QUIC RTT；被控端有等待与编码耗时。 |
| P2P 与强制 Relay 分别连接 | 路径和选中路径的 RTT 对应当前连接；统计不改变连接模式。 |
| 最小化／恢复窗口 | 隐藏期间标记窗口隐藏；不把有意的显示背压当成故障；恢复后继续前进。 |
| 切换显示器／FPS／重连 | 新 generation 从新计数开始；旧采样不会覆盖新状态；无旧输入重放。 |
| 限速、短时断网 | RTT、发送／确认积压和帧号可关联；超过原有故障预算按原有规则恢复，日志保留具体错误。 |
| 老被控端／老 helper | 不支持的统计显示未上报；原有视频、输入、剪贴板和质量查询继续工作。 |
| 复现卡住 | 保留两端同一会话的 pipeline progress / pipeline ended / producer failed / control channel failed 日志，区分媒体、心跳、显示确认、输入和原生渲染错误。 |

## 指标含义

- 采集等待包括桌面静止和采样调度；编码耗时来自同一原生进程的实际 encode 调用，可能包括转换／读回和硬件输出等待。Windows/macOS 支持，其他来源允许未上报。
- QUIC RTT 是选中路径的往返估计。视频写入是交给传输层的耗时；均不能当成单向到达时间。
- 解码提交是接收完帧到原生解码器接受的耗时；接收到显示包含本地 IPC、排队、解码和渲染，不单独声称为纯解码或 GPU 耗时。
- 显示确认周期从发送端开始发送到收到对应的累计显示 ACK，包含网络及观看端处理。所有耗时均使用单端单调时钟，不能用双方 PTS 或系统时钟相减。
- 最新阶段、发送／确认帧号、最老未确认帧和源调用耗时在远端快照中。查询走独立流，每 2 秒取样；超过 6 秒的远端采样不继续当作实时值显示。
- 日志每 30 秒记录一次进度，在可见窗口待显示超过 2 秒的状态变化和流结束时追加记录。只保留最多四个待显示帧的元数据；不增加图像队列、不记录输入内容、不改变原有超时或恢复策略。

新增测量不能单独证明卡顿已修复。必须在原故障设备对上复现并关联两端日志，才能给出具体根因和修复结论。
