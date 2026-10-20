# Linux GTK 宿主与 Rust 显示桥接阶段

本阶段接续 `01a2340`，为正式 Linux 独立窗口准备进程内显示能力。已有 Rust
`LinuxNativeViewer` 与 GTK 私有宿主 ABI，尚未接到 Tauri 的产品入口；操作端
键鼠事件也未接入，不能把这次测试称为完整远控或关闭 W5 / LT-07.5。

## 实现

原生视图借用宿主 GtkWindow，把 GL 区域放在现有 WebKit 内容上方，保留顶部
72 逻辑像素工具栏。退役只移除本代次视图并恢复原内容，不关闭宿主窗口。
GTK 指针只在进程内、主线程使用，没有加入 IPC 或远程消息。

`submit` 只上传 NV12 并请求绘制，不循环派发 GTK 事件，不等待主线程绘制。
后续 `after-paint` 更新计数，Rust 在主线程观察计数后才发送显示确认。
后台线程串行解码，一帧上传后等待显示完成才处理下一帧；压缩包队列最多四帧，
最多一个等待执行的 UI poll。软件解码 Vec 的所有权交给 UI，上传后归还并复用，
无需第二份 NV12 复制。固定样本逐字节比较与地址检查确认跨帧复用同一分配。

取消与解码/窗口错误会停止后台工作，唤醒显示等待并废弃排队 UI 操作。最后的
宿主析构调度回 GTK 线程。原有独立 worker 保留同步入口；没有改变既有认证、
可靠传输、设备数量或单观看者规则。

## 实机检查

环境：Ubuntu 22.04.5 GNOME Wayland VM `556688188`，仅创建自有测试窗口。

- C++ GTK/WebKit 宿主测试通过 270 项检查，包括 20 次创建/销毁、四方向复用、
  工具栏留空、JS 页面计数状态连续保留、重复/短帧拒绝、静态重绘不重复计帧、
  隐藏时不确认、恢复后继续及关闭宿主。20 个 GL widget 的 weak-ref 析构通知
  均收到。使用 ASan/UBSan 和 `G_DEBUG=fatal-warnings`；因 GTK/WebKit 全局
  分配未启用 LeakSanitizer，不能把它写成全面内存/显存泄漏验收。
- Rust Release 图形集成测试分别在原生 Wayland 和 XWayland 通过。真实 Apple
  编码的 8 帧生成画面经过后台软件解码、GTK 上传和绘制确认；同时验证旧代次
  拒绝、隐藏后恢复、排队上传前取消、解码失败、原宿主内容恢复，以及画面
  等待显示时关闭宿主。Tokio runtime 正常退出，无阻塞解码线程挂起。
- Linux 媒体库 12 项、Windows 媒体库 11 项测试通过；Linux 原生 76 项
  ASan/UBSan 边界检查通过。完整 Linux Desktop Release 编译通过。
- 最终自有 native 测试进程为 0；没有更新 VM 或本机安装版、替换服务。

任务 ID、产物与源码哈希见 [结构化证据](remote-desktop-linux-host-2026-10-17.json)。
所有编译串行调度，Cargo 最大 8 jobs，嵌套 CMake 1 job，MSVC `/MP1`。

在已授权的图形会话中复现 Rust 测试（默认标为 ignored，不自动打开窗口）：

```sh
CARGO_BUILD_JOBS=8 CMAKE_BUILD_PARALLEL_LEVEL=1 cargo test --locked --release \
  -p pab-desktop-media --test linux_host -j 8 -- --ignored --nocapture --test-threads=1
```

C++ 宿主检查源码为 `crates/desktop-media/tests/native_linux_host.cpp`，需要
`gtk+-3.0 epoxy webkit2gtk-4.1` 的 pkg-config 依赖，使用 C++17 单任务编译。
测试传入 `--gtk-baseline` 可单独复现纯 GTK 的驱动释放诊断。图形会话的
DISPLAY / XAUTHORITY / Wayland runtime 环境应来自该用户会话，不能硬编码为其他用户。

## 仍需处理

VM 的 SVGA 驱动在多次 GLArea 销毁时输出 `context mismatch in svga_surface_destroy`。
同一测试程序的 `--gtk-baseline` 不使用本项目 Presenter、纹理、shader、overlay
或 reparent，仅创建/销毁 GTK GLArea，仍复现相同诊断。开启现有 VMware WebKit
DMABUF workaround 也不消失。此证据将问题缩小到 GTK/驱动交互，但不证明没有
显存泄漏或设备风险；后续 W7 仍需持续资源观察，未通过禁用 GPU 来掩盖问题。

下一步将宿主接到 Tauri 主线程窗口创建，并完成原生操作端输入采集、模式确认、
焦点丢失释放和旋转/黑边坐标映射，再通过现有产品会话测试 Windows↔Linux。
正式入口仍维持原来的 Linux 未开放状态，避免暴露只有显示而无完整交互的窗口。
当前实机检查不是 Tauri 网络验收，也不是系统登录或长时间性能测试。
