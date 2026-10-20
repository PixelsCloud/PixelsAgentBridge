# macOS Desktop 原生视图接入：阶段记录

日期：2026-10-17。Apple M4，macOS 27.0.1，单屏 1920×1080、100% 缩放。
本记录不关闭 W4，也不代表已完成跨机器操作或 macOS 锁屏登录。

## 实现

Desktop 的独立远程桌面窗口可以在 AppKit 主线程创建嵌入式 Metal 视图，
保留顶部工具栏；NSWindow 地址只在进程内由宿主取得，不进入 IPC 或网络。
VideoToolbox/OpenH264 解码在独立串行后台任务上运行；显示和本地事件处理
由 Tauri 主线程调度。复用原来的四帧窗口、输入协议、认证及可靠传输。

输入默认关闭，通过现有“启用操作”流程开启。原生视图按实际 backing 像素、
黑边和旋转后的显示区域归一化坐标；拒绝黑边点击，按住鼠标拖出画面时钳制
坐标并保留松键事件。失焦、最小化、禁用和销毁释放输入。左右修饰键分别映射，
高频移动合并，按键与模式确认队列溢出时结束输入，不静默丢弃松键。

停止代次后已排队的 UI 工作不再访问视图，持有的 CVPixelBuffer 自动释放。
软件表面池允许四个在途帧加一个静帧重绘表面，上限五个。同步 native 解码若
被驱动永久阻塞，当前进程内实现还不能强制终止该调用；此项不能视为已验收。

## 当前验证

`experiments/remote-desktop/macos_view_input_check.mm` 在 Apple 上编译运行，
36 项检查通过：默认只查看、启用/禁用确认、四个旋转方向的中心与相对角落、
黑边、越界拖动松键、字母键、左右 Shift 同时按住后分别释放、失焦释放、
关闭输入后的事件拒绝、事件接收端失效后的关闭。

这是向自有 NSView 的事件处理方法投递合成 NSEvent 的检查，没有 CGEventPost，
没有向系统或远端发送测试输入。实际屏幕缩放为 100%；不能替代 Retina、
系统快捷键分派、输入法、真实远端键鼠或混合 DPI 的验收。

测试源码 SHA-256：`3ea8c8cd655e1c72cfb8688d4736ace1721e94572421922da4dbc8d86020e402`。
在 macOS 源码目录中可用下列命令复现本地事件检查（输出二进制置于自有临时目录）：

```sh
xcrun clang++ -std=c++17 -O2 -fobjc-arc -fblocks \
  -I crates/desktop-media/native/macos \
  experiments/remote-desktop/macos_view_input_check.mm \
  -framework Foundation -framework AppKit -framework ApplicationServices \
  -o "$TEST_DIRECTORY/macos-view-input-check"
"$TEST_DIRECTORY/macos-view-input-check"
```

共用 Rust 媒体模块的 Windows 和 macOS Release 测试各 11 项通过；新增测试覆盖高频移动
后松键/模式确认不丢失及 native 错误唤醒挂起帧。
Windows 完整 Desktop 的 `cargo check --locked --bin pab-desktop -j 2` 通过，
确认共用输入桥和平台分支保持可编译。Windows 新缓存同样缺少 OCR 模型，
补入已校验 SHA-256 的同版本 `eng.traineddata` 后继续，未修改依赖版本。

完整 Mac Desktop 的首轮构建遇到 OCR 上游源码下载失败，使用已核对版本和
SHA-256 的同版本缓存解决，未改产品依赖。下一轮链接暴露了 `@available`
生成的 `__isPlatformVersionAtLeast` 缺失；构建脚本现从当前编译器查询并链接
Clang macOS 运行库，避免固定 Xcode 路径。原因与 Clang 的
[运行库说明](https://clang.llvm.org/docs/Toolchain.html)一致。修正后完整 Mac Desktop
Release 构建通过（Rust 1.95.0，`tauri/custom-protocol`，2 jobs，4 分 42 秒）。
可执行文件 23,427,792 字节，SHA-256：
`9545920c09d4a1444c4335ded413ff64d0db0abab116a6f7524a9789d2795e0a`。
最终可执行文件的 ScreenCaptureKit 仍为 `LC_LOAD_WEAK_DYLIB`；检查结束时
自有 native 测试进程为空。结构化证据见同目录的 `.json` 记录。

同一构建使用全新的隔离 TOML/数据目录、关闭的回环端口作为服务地址启动，
WindowServer 确认其 PID 有且只有一个大于 900×600 的可见普通窗口；测试结束
后自有进程已退出。没有复制本地 IPC token，没有注册为安装版后台服务的 helper。
该结果证明正式程序可启动窗口，不等于远程桌面产品窗口已完成跨机器验收。

修改后的 native worker 又通过 8 组生命周期检查（四个旋转方向 × 硬件/软件
解码）：尺寸变化、最小化、恢复、关闭，静帧仍只记一次，每组 16 次颜色检查，
最大 RGB 误差 1。worker SHA-256：
`6defa4fc148a986a283792fc27a11e66ce8bd7d107e481bbe22a4ddaf42acfac`。

这轮首次生命周期复测在枚举阶段全部失败：显示器休眠时 active display 数量
为 0，旧版探针也出现相同结果。通过有限时长的系统显示唤醒断言，屏幕恢复为
1920×1080 后重测通过；GUI 测试还显式选择当前用户的图形 bootstrap 会话。
未修改睡眠配置、登录状态或 TCC 权限。这不是睡眠恢复或锁屏登录的完整验收，
被控端在屏幕休眠时的行为仍需补齐。

## 实机前置与未完成项

- XRServer 本轮重新枚举为四块 1920×1080、100% 缩放横向屏幕，x 原点为
  0、1920、3840、5760。90 仍为单屏。XRServer 原服务正常运行，自有临时
  媒体进程为空，未为本轮检查重启或替换它的服务。
- Apple 的现有 Desktop 只记住 Company，尚无 90/XRServer 本地保存的凭据；
  已请求通过正常 Desktop 连接保存。没有读取或输出设备密码。
- 完整 Desktop 窗口的真实跨机器画面、键鼠、窗口生命周期仍待验收。
  macOS 被控端权限、真实采集、锁屏登录、输入法及跨平台组合仍未完成。
- 本轮使用独立源码、缓存和输出目录，不更新本机或 Apple 的安装版。
  Cargo 各用 2 jobs，native 嵌套构建 1 job，总并发维持在 10 以内。
