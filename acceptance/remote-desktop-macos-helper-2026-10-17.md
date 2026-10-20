# macOS 被控端 helper 接入：阶段记录

日期：2026-10-17。Apple M4，macOS 27.0.1，Rust 1.95.0。
本记录属于 W4 的实现与局部验证，07.1/07.2 保持未完成。

## 实现

Executor 在 macOS 启用现有 HelperMediaBackend；Aqua/LoginWindow session helper
在原 AppKit 循环中启动独立媒体连接，沿用现有认证、二进制帧传输和输入连接。
LoginWindow 的 root helper 使用内核读取的进程身份与图形审计会话配对；
普通应用执行路由仍不接受 root，不增加客户端鉴权步骤。

采集调用已有 NativeBackend/ScreenCaptureKit/VideoToolbox，未引入逐帧 CPU
像素复制。枚举前复用已有有限时长显示唤醒逻辑；权限不足通过现有权限错误返回。
每次远程输入代次持有独立输入对象，绑定显示器 ID、原点、逻辑尺寸、像素尺寸
和缩放；布局变化后拒绝旧输入，要求刷新。坐标按所选屏幕逻辑范围映射，支持
负原点及 Retina 比例。实际多屏与 Retina 操作仍需实机验收。

按下时保存翻译后的 native keycode，松开时使用同一个 keycode，避免键盘布局
改变后释放另一按键。输入复用现有按键恢复记录，不保存输入文字。代次销毁先
取消排队操作，再在 AppKit 释放本代次对象；新代次等释放完成后才取得输入槽位。
SIGTERM 路径等待媒体任务取消，再结束 AppKit 进程。旧应用/浏览器输入对象保持独立。

## 验证记录

- Apple 上 `pab-desktop-control` 的 `media_input::tests` 2 项通过：负原点/Retina
  坐标和布局失效；已排队按键在输入对象销毁后不能初始化引擎或发送。
- Apple 上 `macos_display::tests` 6 项通过。以上测试不注入真实系统输入。
- Apple 上 `pab-executor` 的 `local_ipc::media::tests` 3 项通过：使用真实
  回环 WebSocket 与内核身份挑战，验证 200,013 字节帧传输、启动/静帧时取消、
  静帧期间输入、输入或视频断线后的释放和旧 provider 配对拒绝。媒体与输入
  backend 为测试对象，此结果不等于真实屏幕采集或系统松键。
- Apple 上 `helper_identity::media_tests` 2 项通过：进程生命周期、PID/会话
  配对，以及平台 helper 账户规则。上述两组在当前用户图形 bootstrap 中运行。
- Windows 完整 Desktop `cargo check --locked --bin pab-desktop -j 2` 通过。
- 本阶段 Mac 完整 Desktop Release 构建通过。首次从 Desktop 独立 workspace
  选择 Executor 测试被 Cargo
  拒绝（Executor 有 dev-dependencies）；改用根 workspace，不修改产品依赖。

构建均使用独立源码与缓存。Mac 后续测试使用 8 个 Cargo jobs、嵌套 CMake 1 job，
期间不启动其他编译；安装版 Desktop 与服务未被覆盖。

## 实机启动发现与修复

完整 Desktop 构建通过后，隔离 helper 启动检查暴露了原有 AppKit 初始化问题：
它已处于 `NSApplicationActivationPolicyProhibited`，再设同一策略返回 `false`，
旧代码把这个无变化结果当成错误并提前退出。原生最小探针实测 `before=2`、
`setter_returned=false`、`after=2`，确认状态已符合要求。
现在只在当前策略不同的情况下切换。探针保存在
`experiments/remote-desktop/macos_helper_policy_check.m`，不创建窗口、不发送输入。
策略含义及当前状态读取参考 Apple 的
[activationPolicy 文档](https://developer.apple.com/documentation/appkit/nsapplication/activationpolicy())；
同策略设置的具体返回值由本机实测确认，不作为所有系统版本的通用假设。

```sh
xcrun clang -fobjc-arc experiments/remote-desktop/macos_helper_policy_check.m \
  -framework AppKit -o "$TEST_DIRECTORY/helper-policy-check"
"$TEST_DIRECTORY/helper-policy-check"
```

首个 smoke 失败时子进程已退出，安装版未被触碰。修复后完整 Desktop Release
构建通过（8 jobs，4 分 25 秒），23,560,224 字节，SHA-256：
`fcd1ec3630e8f03073daf6cf366a27937cd15f1db1c43586228edeb1f5b70da4`。

同一产物在独立数据目录中运行 `--session-helper`：运行 3 秒后仍存活，未出现
大型主窗口，SIGTERM 后在 10 秒内以状态 0 退出，自有进程已回收。
该目录没有安装版 IPC token，进程辅助功能和录屏权限均为 false；没有真实
媒体连接或系统输入。因此这只证明无连接时的 helper 启动、AppKit 循环和退出
路径，不能替代有按键按下时的释放或 TCC 授权后的验收。

当前图形 bootstrap 的 `activeConsole=true`；屏幕休眠时诊断的活动屏幕数为 0。
本次启停未请求画面，没有触发显示唤醒。结构化记录见同目录 `.json` 文件。

清理检查还发现 Aqua 既有 open-event 转发曾启动同一测试二进制的独立 GUI；
已确认路径和 PID 后结束该测试进程。复测为每次启动建立独立进程组，测试结束
时一并核验其子进程清理；未改变产品原有窗口转发行为。最终检查自有 native
进程为空，ScreenCaptureKit 保持弱链接，安装版进程未被结束或替换。

在 Apple checkout 中复现 Executor 检查：

```sh
CARGO_BUILD_JOBS=8 CMAKE_BUILD_PARALLEL_LEVEL=1 cargo +1.95.0 test \
  --manifest-path Cargo.toml --locked --release -j 8 \
  -p pab-executor --lib local_ipc::media::tests
CARGO_BUILD_JOBS=8 CMAKE_BUILD_PARALLEL_LEVEL=1 cargo +1.95.0 test \
  --manifest-path Cargo.toml --locked --release -j 8 \
  -p pab-executor --lib helper_identity::media_tests
```

## 未覆盖项

当前检查不证明跨机器实际采集、系统键鼠、权限授权恢复或 LoginWindow 画面可用。
新构建的独立可执行文件与已安装 bundle 的 TCC 身份不同，前者权限结果不能代表
后者。仍需验证正式 bundle、Windows↔Apple、锁屏登录和会话切换。

XRServer 已有四屏出图与切换记录；本轮复查仍存在 LogonUI，原 Executor 还有
其他任务子进程。未重启其服务，四屏用户桌面的实际点击坐标继续等待可测试状态。
