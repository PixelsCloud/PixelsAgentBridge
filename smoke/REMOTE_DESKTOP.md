# 远程桌面卡顿诊断验收

两端分别记录候选版本、系统、连接路径、会话及 generation。使用专用账号和合成内容；证据存放在忽略的 `smoke/runs/`。不保存真实桌面、密码、剪贴板或按键内容。

## 自动检查

遵守整机总构建并发不超过 10，准备 BUILDING.md 要求的原生工具链。Windows 需要 NASM；CMake 必须支持实际安装的 Visual Studio，嵌套 CMake/MSVC 使用单作业。已有统一 smoke Cargo 适配器通过 `native_env.py` 准备环境。

```sh
cargo test -p pab-remote-desktop --lib --test reliable_sessions -j 4 -- --test-threads=4
cargo test -p pab-executor --lib -j 4 -- local_ipc::media:: --test-threads=1
cargo test -p pab-desktop-media --lib -j 4
cargo build -p pab-desktop-media --bin pab-media-worker -j 4
cargo test --locked -p pab-desktop-media --test windows_media_timing -j 4 -- --nocapture
cargo test --locked -p pab-desktop-media --lib software::conversion_tests -j 4 -- --nocapture
```

在 Windows/macOS 的交互桌面执行（macOS 的 worker 路径没有 `.exe`）：

```sh
python smoke/remote_desktop_worker.py --worker target/debug/pab-media-worker.exe --software --output smoke/runs/rd-metrics/software.json
python smoke/remote_desktop_worker.py --worker target/debug/pab-media-worker.exe --output smoke/runs/rd-metrics/automatic.json
```

检查同一编码器交替返回旧帧／带耗时帧、调码率后的帧序列、H.264 起始码、PTS 单调性、等待及编码耗时。自动选择编码器可能回退软件，不能据此声称硬件编码通过。脚本只生成合成像素，不采集或保存桌面内容。

软件编码的 BGRA 转换遵循原有 BT.709 有限范围和 2×2 色度平均规则。x86/x64 支持 SSE4.1、偶数高度且宽度为四的倍数时使用 SIMD，其他尺寸／CPU 保留标量路径。`AUTO-17` 验证随机输入、行填充、非对齐指针、原色、透明度、截断与整数溢出，优化输出须与标量逐字节一致，不通过降低画质换速度。

只需测转换时，可在独立目录运行 `rustc --edition=2024 -O --test smoke/remote_desktop_conversion.rs -o smoke/runs/conversion-tests.exe`，再执行该程序并加 `--nocapture --test-threads=1`。编译仍计入整机并发限制；不需要桌面权限或第三方原生依赖。报告的转换耗时不等于编解码总耗时或端到端 FPS。

Windows 每帧直接检查当前 DXGI 输出的尺寸、位置、旋转、显示器句柄和 DPI，完整显示器枚举仍由采集协调循环定期执行；编码器继续校验实际纹理尺寸。`windows_media_timing` 的布局测试需要至少一块可枚举显示器，只修改测试副本中的布局元数据，不修改真实屏幕设置。

验证已解包的 Windows 安装包时，将 `--worker` 指向其 `pab-desktop.exe` 并增加 `--worker-arg=--media-worker`，直接测试安装包内嵌的原生 worker；不需要安装或启动正常桌面界面。

前端在 `apps/desktop` 运行 `npx tsc --noEmit` 和 `npx playwright test tests/remote-desktop.spec.ts`。夹具验证中英文统计、过期采样、旧 generation 隔离、2048 字节原生面板限制及刷新统计不重启视频；不替代原生显示验收。

## 原生端到端采样

`remote_desktop_stability.rs` 使用真实账号与设备密码认证、产品视频协议和已安装的原生显示 worker；不使用假视频或截图轮询，也不记录桌面像素。它不覆盖 Desktop 工具栏、重连界面或用户交互验收。先在被控机打开专用合成动画，保证测试窗口可见、动画实际生成速率高于请求帧率，再运行：

```powershell
cargo build --release --locked -p pab-bridge --example remote_desktop_stability -j 4
target/release/examples/remote_desktop_stability.exe --config C:/private/client.toml 123456789 30 300 "C:/Program Files/PixelsAgentBridge/pab-desktop.exe" > smoke/runs/rd-metrics/frames.jsonl
python smoke/remote_desktop_stability.py smoke/runs/rd-metrics/frames.jsonl --output smoke/runs/rd-metrics/summary.json
python smoke/remote_desktop_trace.py smoke/runs/rd-metrics/frames.jsonl --output smoke/runs/rd-metrics/trace.json
```

探针注册临时端点用于测试会话，连接仍受产品正常登录校验；控制端必须先在同一系统用户下正常登录 Pixels，不能通过复制账号库绕过。TOML 使用实际客户端配置；运行历史保存在临时数据库，现有账号、设备身份和已安装程序不被替换。默认只读当前系统用户已有的设备密码库；未保存密码时，在末尾追加仅当前用户可读的临时密码文件路径，测试后删除该文件，禁止在命令行或报告写入密码。所有编译须计入整机 10 作业限制，已有其他构建时应等待。

Windows 被控端可直接运行仓库内的合成画面：`powershell -NoProfile -STA -ExecutionPolicy Bypass -File smoke/remote_desktop_pattern.ps1`。窗口覆盖主显示器，Escape 关闭，12 分钟自动结束；输出只包含自行绘制的参考图和绘制计数，默认放在忽略的 `smoke/runs/rd-pattern/`。`heartbeat.json` 用于核对动画确实在持续生成；若生成帧率不足，应先调整测试负载，不能将它直接归为传输掉帧。需要提前结束时，在该输出目录创建 `stop` 文件；下次测试前删除该文件。

Windows 的 `pab-media-worker.exe media-timing-check`（安装包使用 `pab-desktop.exe --media-worker media-timing-check`）只测线程等待，不访问桌面。核对 2 ms 请求的实际中位耗时、普通定时器回退和句柄释放；高负载下不要用单次最大耗时作为硬性判断。高精度等待使用线程私有定时器，不更改全局计时精度。分别在控制端和被控端运行，避免匿名管道轮询的调度延迟被误判为网络耗时。

同一设备对、相同画面和分辨率先保存基线，再部署候选版本；每轮持续至少 5 分钟。局域网动态 1080p / 30 FPS 的初始门槛为：去除前 5 秒后原生显示平均至少 28.5 FPS，显示间隔 p95 不超过 67 ms、最大不超过 250 ms，无断流或超过 1 秒的停顿。保留平均值、分位数、最差值及两端版本和二进制校验值；记录采样器自身的丢失数。链路完成不等于性能通过。GUI 的工具栏、切换帧率和关闭／重连仍需真实应用单独验收；网络异常下还要检查四帧信用窗口、恢复速度和旧 generation 隔离，不能靠取消背压或放宽故障超时让测试通过。

控制流回归还要验证：延迟显示确认、心跳或输入操作的回复时，后续显示确认仍可发送；包括输入在内最多八条控制消息在途，视频仍最多四帧在途。同一时刻最多一条修改输入／配置的操作等待结果，下一条修改必须等待其确定回复；显示确认可穿过输入的回复等待，不重排已写出的消息。切屏仍阻止旧代次确认，错序回复、超时或中断必须退出且不重放输入。`pab-remote-desktop` 的 `client::control_tests` 使用真实 loopback QUIC 验证这些场景。Windows 媒体测试另外从低优先级父进程启动，检查媒体 worker 使用正常优先级；不能通过把整个应用提高到高优先级掩盖调度问题。

Windows 原生采集使用自己的精确定时器控制 FPS，并在私有 helper 启动回复中声明这一能力；服务层只对未声明能力的来源追加帧率时钟。两种来源均受四帧显示信用窗口限制。验收要同时测直接原生采集与完整链路，检查是否因重复时钟和 IPC 调度损失帧率；确认 helper 能力经过实际 IPC 传递，未知或缺失能力保持服务层限帧。

采样日志通过独立线程、有限队列写入，文件系统阻塞不会阻塞视频接收与显示确认。队列溢出会明确记录 `error` 和丢失数量，并返回失败；这种采样不得记为通过。正常结尾包含 `telemetry` 汇总，用于核对证据是否完整。

报告区分请求上限、采集上限、接收及原生显示确认；独立记录编码、接收到显示、选中路径 RTT、逐帧间隔和异常结束。显示通知可能合并，其计数与原生渲染器汇总分别保留，不能把帧号跳跃直接算成全部显示完成。只有正常关闭才标记采样完成，异常、缺失尾部或未完成的测试不得写成通过。静态画面允许等待变化，评估动态 30 FPS 必须使用持续运动画面。跨机器采集时间戳不能直接相减作为单程网络耗时。

## 逐帧定位与独立候选服务

性能探针和两端候选程序必须使用 release 构建；探针拒绝 debug 构建。debug 的调度、序列化和日志开销不能用来代表发布性能。合成画面与原生显示窗口只在获准占用的测试桌面上运行，不能覆盖正在使用的工作桌面。

逐帧快照每 500 ms 通过独立控制流查询，最多保存最近 64 帧和 8 个慢帧的元数据。它记录信用等待、源调用、原生采集、读回、色彩转换、编码、两层 IPC、QUIC 写入和显示 ACK。接收端另保存最近 64 次显示通知的 ACK 本地排队、写入、控制回复往返和写入前在途数量，以及选中路径的实际端点地址。排队和回复往返分开计时，累计确认允许合并中间帧；未单独写出的帧保留空值。Windows 显示通知另带对应帧的解码等待与呈现调用耗时；解码等待包含排队，呈现调用不等于面板扫描完成。普通界面诊断保持原有字段与刷新频率。

`remote_desktop_trace.py` 以会话内帧号关联发送端和接收端，比较各自单调时钟上的相邻帧间隔，不相减两台机器的时钟。报告保留超过 100 ms 的显示通知间隔和附近同一路径的丢包检测计数变化；丢包检测不等于精确重传次数，相关性也不能单独证明网络根因。缺少帧记录、正常关闭或完整遥测汇总时，`trace_complete` 为 false，不得将结果记为诊断通过。

需要验证未安装的候选服务时，构建 `cargo build --release --locked -p pab-executor --example remote_desktop_host -j 4`。在被控测试机的交互用户会话内运行 `remote_desktop_host.exe --config C:/private/host.toml host 900 34923`，其中 IPC 端口必须空闲且大于 1023。TOML 使用实际控制服务、Relay 与公有 CA 配置，并设置 `[executor]` 下的 `data_dir` 到专用 `smoke` 子目录；整个目录仅允许测试用户和 SYSTEM 读取。禁止复制现有设备身份或账号库。

该程序使用产品 Executor 和经身份验证的媒体 helper，注册独立测试设备。`data_dir/registration.json` 提供设备码与构建类型，`access-password.txt` 仅供正常设备密码认证，通过受保护通道交给控制端；不得写入命令行、报告或 Git。控制端可将同一 release `remote_desktop_host.exe` 作为显示 worker。主进程正常结束会删除密码，提前终止后须另行清理密码、自己的子进程和测试设备记录；不操作现有应用的进程、服务和配置。

在合成画面输出目录创建 `freeze` 文件可暂停重绘，删除后恢复；窗口保留当前合成内容，不露出真实桌面。静止段单独验收等待变化、心跳和恢复，不用动态 30 FPS 的间隔门槛判定卡顿。

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

## Windows 花屏分段校验

使用 `remote_desktop_proof_pattern.ps1 -OutputDirectory C:/private/smoke/pattern` 打开专用测试图案。图案每秒约变化十次，包含独立的 16 位画面标记、反码和上下重复标记，以及 32 × 18 色块。必须先确认图案覆盖被控主显示器，且 `heartbeat.json` 持续更新，再开始连接。禁止在真实工作桌面运行图案校验。

构建上面的两个 release example，在隔离主机 TOML 下以 `host-proof` 替代 `host` 启动 `remote_desktop_host`。`helper-proof` 只由它的父进程启动。控制端仍正常登录，以同一构建的 `remote_desktop_host` 作为显示 worker，在探针命令末尾追加 `--pattern-proof`；可分别追加 `--relay` 或 `--software-decoder` 做路径／解码器对照。

该模式逐帧检查采集 BGRA、解码 NV12、显示 BGRA 三个阶段各 576 个色块中心。采集必须完全一致；编码／渲染允许每通道最多 16 的误差，标记必须与对应采集帧一致。采集原生子进程在隔离数据目录写入 `proof-capture-PID.jsonl`，包含帧号、图案号、检查数和压缩包指纹；不写桌面图片。指纹使用 FNV-1a，仅用于诊断比较，不能用于认证或防篡改。任一阶段不匹配立即失败，并标明阶段和色块位置。

```sh
python smoke/remote_desktop_proof.py receiver.jsonl proof-capture-PID.jsonl --output smoke/runs/rd-proof/result.json
```

分析器要求每个接收帧都有对应的采集、码流和像素校验记录，原生检查总数覆盖全部帧，至少十种动态画面，正常关闭且遥测无丢失。缺记录、错帧、重复记录、没有实际呈现或强制 Relay 路径未被观察到均不能算通过。`--media-worker pattern-proof-check` 是不访问桌面的校验器正反例测试；Python 回归使用 `python -m unittest discover -s smoke/tests -p test_remote_desktop_proof.py`。

此模式增加 GPU 读回并只检查色块中心，不能代替全图像素对照、文字清晰度、人眼观察或实际 GUI／输入验收。它的十帧图案和额外读回也不能用来证明 30 FPS 性能通过。正常采集与显示默认不启用这些检查。

遇到超差时，可在探针末尾附加 `--proof-packets C:/private/smoke/clip.pab`。它仅允许与 `--pattern-proof` 同用，以新文件保存最多 64 MiB 的合成压缩帧，写满即失败，禁止存入 Git。先确认该文件包含的确为专用图案，再用同一版本 worker 重放：

```sh
python smoke/remote_desktop_replay.py --worker C:/private/smoke/remote_desktop_host.exe --archive C:/private/smoke/clip.pab --show-synthetic --output smoke/runs/rd-proof/hardware.json
python smoke/remote_desktop_replay.py --worker C:/private/smoke/remote_desktop_host.exe --archive C:/private/smoke/clip.pab --show-synthetic --software --output smoke/runs/rd-proof/software.json
cargo build --release --locked -p pab-desktop-media --example remote_desktop_color -j 4
target/release/examples/remote_desktop_color.exe replay C:/private/smoke/clip.pab
```

前两项只在可占用的 Windows 测试桌面运行，会打开原生显示窗口；最后一项只做软件解码，无桌面访问。`remote_desktop_color generate smoke/runs/rd-proof/generated.pab` 可独立重建早期动态色块序列并记录逐帧色差。同一码流在不同解码路径上出现相同超差，不能直接归类为硬件解码花屏：应结合实际编码预算，区分有损编码的色差与结构损坏。绝对色差验收仍保持原阈值，不因对照结果而改写为通过。

编码预算回归还需验证：单次延迟和同一信用等待的重复观察不立即降低画质；反复压力跨越 500 ms 后仍能降低预算，稳定后按原十秒窗口恢复。非零码率间的调节保持参考帧并更新实际原生目标；进入或退出码率控制模式仍输出独立 SPS/PPS + IDR。四帧信用窗口与故障截止时间保持原约束。


## Intel hardware encoder qualification (Windows)

Capture tries NVENC, then the Intel Media Foundation encoder on a matching Intel
GPU, then the bundled software codec. Intel uses GPU BGRA-to-NV12 conversion,
limited-range BT.709, quality mode 90, baseline H.264 and zero B frames. It keeps
one frame in flight and checks output timestamps. Unsupported setup or runtime
failure retires the transform before software fallback, whose first frame is an
independent SPS/PPS + IDR. Optional bitrate constraints are currently reported as
unsupported on Intel; they must not silently rebuild or replace the GPU path.

The isolated qualification fixture reads no desktop and opens no window. Build
from a Visual Studio x64 developer shell, with no other build consuming the
machine's job budget:

```powershell
New-Item -ItemType Directory -Force smoke/local/mf-qualification | Out-Null
cl /nologo /O2 /EHsc /std:c++17 smoke/windows_mf_encoder_probe.cpp /Fosmoke/local/mf-qualification/probe.obj /Fesmoke/local/mf-qualification/probe.exe /link mfplat.lib mfuuid.lib ole32.lib d3d11.lib dxgi.lib dxguid.lib wmcodecdspuuid.lib
& smoke/local/mf-qualification/probe.exe (Join-Path $PWD smoke/local/mf-qualification/generated.pab)
cargo build --release --locked -p pab-desktop-media --example remote_desktop_color -j 4
& target/release/examples/remote_desktop_color.exe replay smoke/local/mf-qualification/generated.pab
```

The archive must be a new absolute path containing a `smoke` component. The
fixture generates 240 frames at 1920x1080, caps the archive at 64 MiB, and fails
on missing hardware, reordered output or a two-second event/GPU deadline. Its
reported times exclude generated-pixel construction/upload and transport. The
color oracle must report `within_threshold: true`; check the boolean as well as
the process exit code. Then replay the same archive through both native decoder
paths on an available test desktop. These fixture results do not replace the
five-minute end-to-end 30 FPS gate or full-image/text clarity inspection.

Record the actual capture backend from the helper log (`intel_mf_gpu`,
`nvenc_gpu`, or `openh264_software`). Never attribute an automatic software
fallback's successful frames or timings to the GPU encoder.


For the visible 30 FPS gate, run `remote_desktop_stability.py INPUT --output
REPORT --check-performance`. Besides complete telemetry, it requires sufficient
native presentation observations (at least 90% of consumption notifications),
notification FPS >= 28.5, p95 gap <= 67 ms, maximum gap <= 250 ms, no one-second
gaps, and a direct LAN path. Sparse native observations mean insufficient visible
performance evidence; they do not by themselves prove low scanout FPS. Check
occlusion, display power/lock state and driver statistics before assigning cause.

GPU-completion, async-encoder events and bounded renderer shutdown use the
worker's high-resolution waitable timer, without changing global timer resolution.
On an available Windows test host with a working GPU encoder, also run:

```powershell
python smoke/remote_desktop_worker.py --worker C:/private/smoke/remote_desktop_host.exe --worker-arg=--media-worker --fail-encoder --output smoke/runs/rd-proof/encoder-fallback.json
```

This synthetic-only case fails the third GPU frame deliberately. It requires a
new independent software IDR, actual software readback, a native fallback marker,
continued frame progression, and paced recovery after a deliberate pause. It
must not pass by starting on software when hardware initialization was unavailable.

Hardware qualification must also pass `remote_desktop_proof.py RECEIVER CAPTURE --output REPORT --require-encoder intel_mf_gpu` (or `nvenc_gpu`). Every capture record must name the expected backend, including frames produced just beyond the receiver stop. A successful automatic software fallback is valid recovery evidence, but cannot count as successful hardware qualification.

Add `--show-synthetic` to the encoder-fault command only on an available test desktop. It replays the exact six generated packets through automatic and forced-software native decode/render paths, checks four colored quadrants for every frame (including after the hardware-to-software SPS/PPS + IDR switch), and requires observed native presentation. It opens temporary windows and must not run on a working desktop. The default worker check remains headless.


Keep bitrate-stress and full-quality switch qualifications separate. The worker
check defaults to a 750000 bps budget update to exercise native IPC. That lossy
budget can exceed the full-quality color tolerance. For visible encoder-fault
qualification at full quality, explicitly add `--budget-bps 0 --show-synthetic`:

```powershell
python smoke/remote_desktop_worker.py --worker C:/private/smoke/remote_desktop_host.exe --worker-arg=--media-worker --fail-encoder --budget-bps 0 --show-synthetic --output smoke/runs/rd-proof/encoder-switch-full-quality.json
```

Replay uses the production asynchronous adapter and temporarily raises only its
own generated-content window. Both decoder paths are attempted even if one
fails. Preserve failed bitrate-stress results; do not relax the 12-level quadrant
tolerance or describe a lossy-budget result as full-quality qualification.

## Correlating LAN stalls

Use `--network-probe PRIVATE_IPV4:PORT` on the isolated stability receiver to
record concurrent 64-byte UDP round trips, an independent receiver thread's
50 ms wakeups, and Tokio's 50 ms wakeups. This option is off by default and
does not change video credit, codec settings, transport parameters or priority.
All observations use the receiver's existing monotonic clock.

On the available source test machine, run the bounded responder with explicit
LAN addresses and a new absolute output path containing a `smoke` component:

```powershell
python smoke/remote_desktop_udp_echo.py --bind 192.168.1.10 --peer 192.168.1.20 --ready C:/private/smoke/udp-ready.json --seconds 600
```

Read its chosen UDP port from `udp-ready.json`. If an inbound firewall exception
is needed, scope it to that port, interpreter, local address and receiver address,
and remove it in the launcher's `finally` block. The responder accepts only the
specified peer and fixed 64-byte metadata packets, echoes no arbitrary payload,
and never returns more bytes than it receives. It ends at its deadline or when
`udp-ready.stop` exists, then writes `udp-ready.summary.json`. Existing output or
stop files are rejected, so previous evidence is not replaced.

Add `--network-probe 192.168.1.10:PORT` to the receiver's normal stability command,
then analyze its JSONL with `remote_desktop_trace.py`. Each slow video interval
includes overlapping UDP and scheduling observations. Check the selected video
endpoint against the responder address, sample coverage, failed probes and
telemetry completeness. Keep the normal five-minute FPS gate as a separate
result. A responder's `server_work_us` measures only userspace reply construction;
it excludes time before the receive call returns. Correlated UDP delay can still
include endpoint scheduling, socket dispatch and network delay. It does not prove
physical packet loss or identify a driver. Never subtract peer clock timestamps.

The responder records bounded socket error metadata in
`udp-ready.errors.jsonl` (at most 256 records, total counters in its summary).
Transient local send failures, including Windows 10055, drop that probe without
retrying or exiting the responder. Other socket errors remain fatal and leave a
failed summary. A stopped or failed responder invalidates subsequent UDP failure
observations as evidence of network loss. Pass the matching error file to
`remote_desktop_trace.py --network-errors FILE` to join responder send errors by
the probe sequence; its timestamps use a different origin and are never subtracted
from receiver timestamps. This distinguishes an observed local socket-send error
from an unanswered probe whose failure location is unknown.

## Windows packet-stack correlation

`windows_pktmon.ps1` runs an exclusive, bounded PktMon capture on an available
Windows test machine. Run from an elevated PowerShell session, using the actual
UDP ports of the isolated test host and responder. Obtain ports from the test
process and verify them against the desktop trace's selected endpoint. Capture
only the two explicit LAN endpoints; no desktop access or decryption keys are
needed:

```powershell
& smoke/windows_pktmon.ps1 -Action Start -Directory C:/private/smoke/packet-run -LocalAddress 192.168.1.10 -PeerAddress 192.168.1.20 -Ports @(12345,12346) -Seconds 240
# Run the isolated synthetic desktop/probe on the two available test machines.
& smoke/windows_pktmon.ps1 -Action Stop -Directory C:/private/smoke/packet-run
pktmon etl2txt C:/private/smoke/packet-run/capture.etl --out C:/private/smoke/packet-run/capture.txt --hex
pktmon etl2pcap C:/private/smoke/packet-run/capture.etl --out C:/private/smoke/packet-run/capture.pcapng
```

Start refuses existing captures or filters. A SYSTEM watchdog stops the owned
capture at its deadline, including after caller disconnection. Stop verifies the
active log path before stopping anything. PktMon removes **all** filters rather
than individual named filters: cleanup clears them only if every current filter
belongs to this capture. If foreign filters appeared, it preserves all filters,
removes the owned watchdog and reports the conflict. The wrapper was checked on
Windows 11 and Server 2022, including preservation of a separately added filter.

Each packet snapshot is truncated to 96 bytes. The ETL uses a 128 MiB circular
file; this is a file limit, not an ETW memory limit. Results are private runtime
evidence under ignored `smoke/local/` or `smoke/runs/`. Keep ETL and text: pcapng
conversion omits component and drop-reason details. Use text headers to check
`EventsLost` and `BuffersLost`, and resolve component IDs from that capture's
footer; IDs must not be reused blindly from another boot or machine.

`remote_desktop_packets.py` joins synthetic UDP probes by random session token
and sequence. It compares durations measured locally at each endpoint, never the
two machines' absolute timestamps:

```powershell
python smoke/remote_desktop_packets.py SOURCE.txt RECEIVER.txt --source-ip 192.168.1.10 --receiver-ip 192.168.1.20 --source-nic SOURCE_COMPONENT_ID --receiver-nic RECEIVER_COMPONENT_ID --output smoke/runs/packet-run/intervals.json
```

It currently decodes plain Ethernet/IPv4 UDP probe snapshots. The remainder after
subtracting endpoint-local intervals is not exact wire or one-way latency. Missing
snapshots are not automatically dropped packets. `etl_events_complete` checks ETW
loss reporting; `paired_observations_complete` separately checks whether all
observed probe identities have the required NIC appearances. Check those counts
against application probe completions before interpreting packet coverage. Even
an ETL reporting zero lost events can lack some expected packet observations.

Compare capture-on and capture-off runs with the same duration and configuration.
Short diagnostic success does not replace the five-minute visible performance
gate, and a run in which no stall occurred cannot establish the cause of previous
stalls. For isolated source logging, set `log_level = "info,noq_udp=debug"` in the
test TOML's `[executor]` table. The current UDP dependency logs send errors at most
once per minute; absence of a log entry cannot prove absence of an error. Its
Windows send path treats non-WouldBlock UDP send errors as nonfatal packet loss,
leaving recovery to QUIC loss detection. Changing retry policy needs a reproduced
failure and focused recovery tests rather than assuming that this behavior is
the cause of all stalls.

For long runs, pass a matching deadline to the synthetic source:

```powershell
powershell -NoProfile -STA -File smoke/remote_desktop_pattern.ps1 -OutputDirectory smoke/runs/source-pattern -Seconds 2500
python smoke/remote_desktop_soak.py smoke/runs/soak/frames.jsonl --minimum-seconds 1800 --output smoke/runs/soak/soak.json
```

Run each direction separately on available test desktops. Extend the isolated
host, viewer, scheduled-task and cleanup-watchdog deadlines together; the default
12-minute pattern cannot substantiate a 30-minute run. Keep the unchanged visible
performance thresholds, candidate hashes, actual decoder/path, native observation
ratio and telemetry completeness. The soak report adds minute windows, including
empty windows and gaps crossing boundaries, to expose deterioration hidden by an
overall average. Its window rates are diagnostic, not additional acceptance gates.
Image quality and actual embedded App input/resize remain separate checks.

`windows_desktop_network_fault.ps1` installs temporary block rules scoped to an
isolated smoke desktop binary's owned UDP ports and one private IPv4 peer. It never changes a NIC,
global firewall defaults or an installed app's rules. Run it elevated on the
machine hosting that smoke process; the directory must be fresh and under smoke:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File smoke/windows_desktop_network_fault.ps1 -Directory C:\test\smoke\fault-unique -TargetProcessId SMOKE_PROCESS_ID -PeerAddress 192.168.1.20 -Milliseconds 500
```

The maximum requested hold is three seconds. An independent SYSTEM watchdog
removes the two UUID-owned rules if the caller disappears; collect `owner.json`,
`finished.json`, `watchdog.json` and confirm no owned rules/tasks remain. Rule
installation/removal adds time beyond the requested hold, so do not label the
argument as measured packet blackout duration. This uses the documented
[Windows firewall rule filters](https://learn.microsoft.com/en-us/powershell/module/netsecurity/new-netfirewallrule?view=windowsserver2022-ps).
Confirm the trace actually experienced the impairment before claiming recovery;
an existing flow might not be affected as expected, and Iroh can switch paths.
Record selected paths and sender loss counters, preserve the strict no-fault
baseline separately, and measure post-fault frame cadence/continuity. This tool
does not emulate controlled latency, a percentage of loss, whole-machine network
disconnection or a severed transport connection.

Record viewer monotonic bounds immediately before and after each injection, then
check both transport impact and at least 20 seconds of recovered presentation:

```powershell
python smoke/remote_desktop_recovery.py smoke/runs/fault/frames.jsonl --injections smoke/runs/fault/injections.json --output smoke/runs/fault/recovery.json
```

Each injection record has `requested_ms`, `before_receiver_elapsed_s`,
`after_receiver_elapsed_s`, and the injector's `result` object. The audit rejects
missing faults, unconfirmed cleanup, insufficient transport/presentation impact,
short post-fault observation and persistent poor cadence. Bounds include control
call overhead; they do not measure exact blackout or cross-host recovery latency.
This remains a native pipeline test; exercise the real App's automatic reopening
and input lease handling separately.

For counted loss, delay and blackout testing without changing a machine's NIC,
run the real QUIC integration cases:

```powershell
cargo test --locked -j 4 -p pab-remote-desktop --test reliable_sessions network_recovery:: -- --nocapture --test-threads=1
```

The implementation is `smoke/remote_desktop_network_recovery.rs`. A bounded UDP
proxy adds 20 ms per hop, can discard every 50th datagram, and can drop all
datagrams for 900 ms. Both selected endpoints must remain the proxy throughout;
otherwise the case fails. This matters because Iroh can discover and switch to a
faster direct path, bypassing an ordinary UDP proxy.

Only the opted-in test configuration disables candidate exchange. The supporting
`test-utils` / `smoke-test-utils` features do not change ordinary endpoint defaults
or TLS checks. The cases verify counted drops, no proxy queue overflow, ordered
synthetic payloads, retained control access, same-session/generation recovery and
no persistent cadence penalty. They use synthetic 15 FPS protocol fixtures,
not native H.264, actual 30 FPS display, OS socket-buffer exhaustion or the App's
automatic reopening/input route.
