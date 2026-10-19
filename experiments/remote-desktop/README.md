# 远程桌面开发实验

本目录保存隔离实验的入口和版本清单。P2 协议与会话 crate 已进入工作区，原生媒体尚未注册到安装版启动路径；完整任务见 [开发方案](../../REMOTE_DESKTOP_IMPLEMENTATION_PLAN.md)。Windows 原生媒体接入真实可靠流的最新实验见 [说明与复现](RELIABLE_MEDIA.md)。

## 准备检查

使用 Python 3.11 或更新版本，在仓库根目录运行：

```powershell
python experiments/remote-desktop/preflight.py --rustdesk D:/GoCloud/rustdesk --hwcodec .build/remote-desktop/upstream/hwcodec
```

脚本只读取指定源码、工具版本和构建路径，输出 JSON 运行报告，不下载、编译、修改配置、采集屏幕或发送输入。退出码 0 只表示参考源码匹配，不表示 native 编译依赖齐全或硬编/零拷贝已经通过。`native_build_ready` 在完成开发库验证前保持 false。

## 首个实验

参考固定 hwcodec 的 `examples/pipeline_vram.rs` 及 `dev/capture`、`dev/render`，验证 Windows GPU 纹理采集、硬编、硬解与显示。当前上游 build.rs 默认需要 FFmpeg/vcpkg；先整理最小 native 依赖和 SDK 头文件，不直接构建整个 RustDesk。

实验至少记录每段耗时、有效显示帧率、编码格式、GPU/适配器身份、CPU 回读、纹理复制、格式转换、码率和显存。首个动态画面使用合成图案或专用测试窗口，避免保存真实桌面内容。

## 已实现的采集前置探针

```powershell
python experiments/remote-desktop/build_dxgi_probe.py
& .build/remote-desktop/dxgi/dxgi-probe.exe
& .build/remote-desktop/dxgi/dxgi-probe.exe --capture --display 0 --seconds 5
```

默认仅枚举当前图形会话的 DXGI 输出；`--capture` 在指定输出上采集 1–60 秒，只读取 GPU 纹理描述和帧时间，不保存、传输或 CPU 回读屏幕图像。显示器索引只对本次枚举有效，不是生产协议的稳定屏幕 ID。遇到桌面切换、输出失效或权限错误时明确退出；恢复和登录桌面适配在后续阶段实现。

`changed_capture_fps` 按不同的非零 `LastPresentTime` 计数，仅代表桌面更新采样，不证明每帧像素不同，也不等于最终有效显示帧率。静止桌面不能用于验收 12 FPS 指标。`probe_*` 复制计数仅描述本探针代码，不声称驱动、DWM 或后续编解码内部没有复制。

构建脚本使用 Windows SDK/MSVC，单个翻译单元、一个编译任务，设置最多 10 个逻辑处理器的继承亲和性；发现其他编译器活动则退出。不依赖 FFmpeg/vcpkg，不构建生产应用。`baseline-windows.ps1`、`baseline-linux.py` 为只读硬件/会话前置查询；PATH 中未找到工具不表示机器上绝对没有安装。

`mcp_client.py` 提供独立实验用的 stdio 客户端，启动指定的现有 MCP 程序，沿用其服务与本地登录配置；不接受或复制设备密码，不改安装文件。串行请求，有限等待，超时不自动重放操作；结束时只关闭自己启动的 MCP 进程。该连接自身会产生正常的 MCP 端点和任务运行记录。

实测结果和限制见 [2026-10-16 记录](RESULTS-2026-10-16.md)。原始报告保存在忽略目录 `.build/remote-desktop/`。

## 构建约束

持续观看/混合测试使用 `verify_windows_linux_mixed.py --seconds`，单次最长
28800 秒。Linux 动态画面 fixture 可限时运行至 32400 秒，给部署和回收留出
时间；远端服务回滚期限必须覆盖所选测试时长，不能只延长本机 driver。
`windows_gpu_resources.py --profile <私有测试目录> --seconds <时长>` 每约
30 秒读取该 Desktop 及子进程的 GPU 内存计数，独占创建结果文件，结束后退出。
应在观看开始时启动；缺少计数、仅观察到 WebView 或关闭后的数据均不能证明
活动视频显存稳定。现有控制台会话、真实呈现采样和资源清理要求仍然适用。

Windows 呈现采样遇到帧编号倒退或采样循环异常时，先保存失败原因、已有帧编号
及触发倒退的生成图案像素，再以失败退出；不保存完整桌面。每段报告独占创建，
不会覆盖原失败样本，帧序与 >12 FPS 验收条件不变。

Mac 实际产品窗口的合成画面观察工具及只含生成帧编号的可复核样本见
[操作端显示验收](../../acceptance/remote-desktop-macos-operator-presentation-2026-10-17.md)。
`analyze_macos_presentation.py` 可在本机离线复核；不会重写原始测试的通过/失败。
`macos_sample_presentation.py` 必须配合已有授权的固定 App 和当前产品窗口使用，
会增加一次屏幕采集/编解码负载。`windows_display_fixture.cpp` 会在前台覆盖主屏
显示生成动画 120 秒，仅限已授权测试机器；不得当作无界后台工具启动。

编译前检查机器是否已有其他会话在构建。单次 Cargo 最多 `-j 10`，嵌套 native 构建限制为 1；所有同时运行的编译合计不能超过 10。Windows 构建进程及其子进程限制在至多 10 个可用逻辑处理器。不要覆盖通用构建脚本、全局环境变量或已安装 Pixels。

源码下载与构建产物放在 `.build/remote-desktop/`，不提交到仓库。引用第三方配置保留其规定格式，新增项目自有配置用 TOML。后续移植源码需固定版本并保留来源和许可。

## 直接纹理 H.264 编码

新增 `nvenc_probe.cpp`，为项目自行编写的隔离实验；参考固定版本 hwcodec 的 NVENC 配置，但直接注册调用方纹理，不使用其 SDK 包装器中的 `CopyResource`。依赖仅为 Windows SDK、固定 externals 中带 NVIDIA MIT 许可声明的 `nvEncodeAPI.h` 和系统 NVIDIA 驱动。头文件不复制到本仓库，构建前核对提交及内容哈希；不运行上游构建脚本。API 参考见 [NVIDIA 编程指南](https://docs.nvidia.com/video-technologies/video-codec-sdk/12.2/nvenc-video-encoder-api-prog-guide/index.html)。

```powershell
git -C .build/remote-desktop/upstream/hwcodec submodule update --init --depth 1 externals
python experiments/remote-desktop/build_nvenc_probe.py
python experiments/remote-desktop/verify_nvenc_probe.py
# 可选：会在所选屏幕短暂显示测试动画，5 秒后关闭，不抢焦点、不发送输入。
python experiments/remote-desktop/verify_nvenc_probe.py --capture --animate --display 0
```

默认验证 GPU 合成纹理的 15/30 FPS 编码及离线软件解码；FFmpeg/ffprobe 仅为验证工具，不作为产品运行依赖。检查分辨率、无 B 帧、输入输出帧数、逐帧哈希不同、参数拒绝及输出文件保护。所有生成物保存在唯一的 `.build/remote-desktop/nvenc/verify-*` 目录。

`--capture` 可直接编码 DXGI 返回的 BGRA8 纹理，禁止与 `--output` 同用，不保存真实桌面码流。`--animate` 只能与采集一起启用，创建不激活的合成颜色窗口，为静止桌面提供更新；它不是解码显示器。显示器编号仅在本次实验枚举中有效。

编码器关闭 B 帧及 lookahead，单帧同步等待码流后才解除资源映射、释放 DXGI frame lease。码流字节读至 CPU 是编码输出，不是像素回读。报告中的零复制、零转换计数仅描述本实验代码，驱动内部处理未知。该编码探针不包含解码和显示，完整本地实验见下文。

## 硬解码与原生显示闭环

```powershell
python experiments/remote-desktop/build_loopback_probe.py
python experiments/remote-desktop/verify_loopback_probe.py
# 增加实际桌面采集：短暂显示动画与查看窗口，不抢焦点、不保存画面。
python experiments/remote-desktop/verify_loopback_probe.py --capture --display 0
```

独立 `loopback-probe.exe` 复用 `nvenc_encoder.h`，新增 `mf_decoder.h`、`d3d_presenter.h`。Windows SDK 系统 H.264 Media Foundation 解码器通过 DXGI device manager 使用同一个 D3D11 device，输出必须为带 `D3D11_BIND_DECODER` 的 NV12 GPU 纹理；缺少组件或硬件支持时明确失败，不把隐式软件后备计为硬解成功。参考 hwcodec 固定版本的 D3D11 视频处理器路径，实验实现自行编写，未引入上游 FFmpeg 构建或二进制依赖。

解码 sample 持有纹理数组槽位直到 GPU 处理完成。视频处理器直接读取该槽位，进行一次必要的 NV12→BGRA 转换及缩放，写入 flip swapchain，不先复制到中间 RGB 纹理。码流声明 SDR BT.709、有限视频范围，输出为全范围 RGB；1080p 对齐到 1088 行时按解码器可见 aperture 裁剪。测试窗口固定 960×540，当前验收视频尺寸为 1920×1080。

窗口使用单帧呈现队列，每次 `Present` 后等待 DXGI frame statistics 确认对应 present ID，再提交下一帧。`effective_display_fps` 是确认显示的不同帧数 / 实测墙钟时长，含首帧启动和末帧 drain；不以编码或 Present 调用次数代替。`source_to_display_observed_*` 是本机开始生成/获取一帧到观察到系统显示确认的耗时，包含轮询误差，**不是网络延迟、输入响应时间或光学测得的屏幕延迟**。`decode_output_*` 是 MFT 交付 GPU sample 的耗时；实际 GPU 完成等待计入后续显示阶段。

默认验证包括：变化四色图案的四角 RGB、帧时间戳顺序、编码/解码/显示数量一致、15/30 FPS 性能、有限待解码队列、非法参数拒绝，以及关闭本脚本创建的查看窗口后进程退出。`--verify-pixels` 每帧只回读四个合成图像像素，单独计量；性能场景不启用它，真实桌面禁止启用。不会保存码流或屏幕图片。采集时排除本地查看窗口，避免递归画面。

报告包含进程 CPU 时间、峰值工作集、结束时 private bytes、DXGI 采样的进程本地显存峰值；不包含 DWM/驱动的全部系统开销，未测 GPU 引擎利用率。应用显式复制次数为零不代表驱动和系统内部零复制。

本机 RTX 3060 与 90 RTX 4090 的 1080p 本地闭环已通过，证据见 [实验记录](RESULTS-2026-10-16.md) 和 [验收数据](../../acceptance/remote-desktop-loopback-2026-10-16.json)。这一阶段尚未接入网络、Desktop 产品入口、多屏切换/旋转、输入、锁屏、软编后备或 Mac/Linux 媒体路径。固定尺寸和颜色契约仅适用于本实验，不能作为生产协议省略 generation、色彩与屏幕布局协商的依据。

系统接口参考：[H.264 解码器](https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-decoder)、[D3D11 解码资源交接](https://learn.microsoft.com/en-us/windows/win32/medfound/supporting-direct3d-11-video-decoding-in-media-foundation)、[可见区域](https://learn.microsoft.com/en-us/windows/win32/medfound/picture-aspect-ratio)、[显示统计](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_frame_statistics)。

## Windows 活动协助的真实进程验收

`assistance_active_acceptance.py` 使用已有的隔离测试资料与已暂存的被控端，覆盖
active 工具取消、请求方崩溃、Desktop 崩溃重启及过期；不会安装程序或发送远端输入。
需要 Python `psutil`、本目录 `mcp_client.py` 及 Desktop 的 Playwright 依赖。

```powershell
python experiments/remote-desktop/assistance_active_acceptance.py --profile .build/remote-desktop/<isolated-profile> --code <device-code>
```

profile 必须位于本工作区 `.build/remote-desktop/`，包含自己的 `config.toml`、
`pab-desktop.exe`、`pab-mcp.exe` 及已有授权数据；不能传安装目录。39100 端口必须空闲。
Desktop 必须是启用正式内置前端的候选，被控端也须已有对应媒体 helper；这些准备不由
脚本代办。脚本只结束自己启动的 Desktop/MCP 及其专属媒体/WebView 子进程，报告保存
在新的私有 `.build` 目录；不读取、保存实际画面或用户输入。

Windows 凭据槽属于系统凭据库：仅复制账号文件不能隔离实际退出/服务切换时的令牌
删除。做该类测试须先建立独立测试 slot，保持原用户的 slot/令牌不变。服务器切换后
恢复私有资料也必须维持 revision 单调增长，不能把旧 revision 写回后计为产品故障。
本轮实际结果与精确范围见[active、服务切换与混合 DPI 验收](../../acceptance/remote-desktop-active-switch-mixed-dpi-2026-10-19.md)。
