# 远程桌面开发实验

本目录保存隔离实验的入口和版本清单。生产协议、Cargo 工作区和发布构建尚未接入新功能；完整任务见 [开发方案](../../REMOTE_DESKTOP_IMPLEMENTATION_PLAN.md)。

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

编码器关闭 B 帧及 lookahead，单帧同步等待码流后才解除资源映射、释放 DXGI frame lease。码流字节读至 CPU 是编码输出，不是像素回读。报告中的零复制、零转换计数仅描述本实验代码，驱动内部处理未知。暂不包含硬解码、渲染、网络、软编后备、会话恢复和锁屏支持。
