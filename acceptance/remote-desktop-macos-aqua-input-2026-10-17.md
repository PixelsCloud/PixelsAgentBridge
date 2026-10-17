# macOS 正式 Aqua helper 与实际输入验收

日期：2026-10-17。基线 `63c8193`，Apple M4 / macOS 27 / 单屏 1920×1080。

## 测试路径

独立 Executor 测试进程启动真实本地 IPC 服务，使用新端口和私有 token。
临时 Aqua LaunchAgent 启动候选签名 App 的 `--session-helper`，经过原有
token 认证、内核进程身份验证和捕获/输入配对；调用正式 `HelperMediaBackend`。
没有修改已安装的服务、App、用户配置或 TCC。plist 是 launchd 的强制格式。

第一轮未发送普通键鼠事件的检查已通过：两次打开 1920×1080 采集，各解码
30 帧，通过输入管道确认 ReleaseAll；关闭捕获后旧注册退出，helper 重新注册
后可再次采集，最后 bootout 后注册和 helper 均退出。使用 VideoToolbox 硬编，
实际采集无逐帧 CPU 像素映射。此结果不能代替动态网络显示 FPS。

## 发现与结论更正

加入自有 AppKit 窗口后，旧候选包在采集期间发生前台焦点变化。输入前的焦点断言
失败，因此没有向其他窗口发送测试按键。前台进程记录显示从测试窗口切到
`vip.rgaa.pab.desktop` 子进程，捕获结束后才返回原窗口。只验证 ReleaseAll
或采集出帧无法发现这个问题。

试验性增加 `NSApplicationActivationPolicyProhibited` 后，完整 Desktop 构建
和 11 项媒体库测试通过，但实际输入仍在发送前因失焦失败。这项试验不能证明
修复有效，已从提交代码中撤回；隔离候选包仅作为诊断产物保留。

随后通过现有截图工具查看真实桌面，确认系统有多个叠放的 Pixels Agent Bridge
授权弹窗，要求「绕过系统窗口选择器，直接访问屏幕和音频」。因此前台变化
不能直接归因为 worker 激活错误，`CGPreflightScreenCaptureAccess=true` 也
不能证明这些确认已完成。之前「无需用户进一步确认」的结论仅适用于已检查的
两项权限开关，需要更正；基本采集通过也不等于输入可以在弹窗覆盖下验收。

已停止重复采集并请用户完成当前可见系统提示。未自动点击系统授权、修改 TCC、
添加 entitlement 或关闭系统提示进程。现有测试产生的重复提示及授权恢复流程
仍需继续处理。权限页状态由「正常」改为「已授权」，补充首次共享时的额外系统
确认说明，避免把开关状态描述为实际链路健康。简体、繁体和英文同时更新。

Apple 的 [Persistent Content Capture 文档](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.developer.persistent-content-capture)
说明，VNC 类 App 使用这项 entitlement 前必须先向 Apple 申请并获得许可。
没有直接将该 entitlement 加进签名配置。用户确认后的单次重开、是否再次弹窗、
真实键鼠与失焦恢复继续待验收。

## 可重复运行

新增 `macos_launchagent_native_media_reopens_and_retires` 忽略测试，只能显式
在授权 Apple 图形会话和 `/private/tmp` 下的隔离 profile 运行。测试窗口只观察
自己收到的事件，不安装全局事件钩子，不记录输入正文。

编译 Executor 测试：

```sh
cargo test --locked --release -j 4 -p pab-executor --lib --no-run
clang -fobjc-arc -O2 experiments/remote-desktop/macos_input_fixture.m \
  -framework AppKit -framework ApplicationServices -o /private/tmp/pab-TEST/input-fixture
```

将已签名且有屏幕录制/辅助功能授权的候选 App、测试可执行文件及 fixture
放入同一个 `/private/tmp/pab-*` 目录。以当前图形用户执行
`experiments/remote-desktop/macos_helper_check.py`，提供 `--stage`、`--app`、
`--test`、`--fixture` 四个绝对路径。编译与运行分开，不在测试时自动构建。

协调器通过 LaunchServices 打开自有窗口，确认焦点后启动测试和临时 helper。
每轮采集之后核对鼠标点击位置、单次按下/抬起、A 键、滚轮；按住 Shift，
分别验证 ReleaseAll 与关闭输入管道后的实际释放。关闭 helper 后检查注册
清空、候选进程和窗口退出。结果保存在私有运行目录的 `result.json`。

## 当前结果与范围

基本 capture/IPC/ReleaseAll 两轮通过；带实际按键的测试尚未通过，所有失败
均发生在发送普通输入之前。末次运行后候选 App/worker 进程、测试窗口和临时
helper 均为零；安装版没有替换。测试不将画面码流写入文件，公开记录只含计数、
进程生命周期和哈希，不发布完整桌面截图。前端 `tsc --noEmit` 通过。
基本成功与后续失败的原始结果见 [机器记录](remote-desktop-macos-aqua-input-2026-10-17.json)。

这是 Apple 本机正式 helper/原生输入链路的阶段验收，不替代 Windows↔Apple
产品独立窗口、锁屏/LoginWindow、多屏、九方向矩阵或稳定性验收。W4 保持未完成。
