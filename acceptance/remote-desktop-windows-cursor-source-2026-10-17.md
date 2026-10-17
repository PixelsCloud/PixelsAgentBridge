# Windows 原生光标源

接续 [helper 通道](remote-desktop-cursor-helper-2026-10-17.md)。本阶段实现
Windows 系统光标采样与 Rust 原生进程适配；窗口叠加、Mac/Linux 独立光标源、
多屏与锁屏光标验收仍未完成，不关闭 LT-03.5。

## 实现与取舍

- `NativeBackend::cursor` 只接受当前活动视频的完整配置，视频释放会取消该
  光标源。即使调用方保留光标句柄且不再读取，后台任务仍关闭、回收原生进程。
  切换代次前等待旧进程退出；启动或读取取消也不重用不确定的管道半包。
- 单独运行 DXGI 光标 worker，持续更新一份最新形状/位置；按请求返回，
  不积累移动历史。不依赖视频采集请求或展示额度，不修改原有视频编码流程。
  每次 AcquireNextFrame 只检查元数据，释放桌面纹理租约后才写管道，
  不映射、复制或编码这张桌面纹理。此方案新增一个 GPU device/duplication
  client，仍需验证与其他采集软件并行时的资源上限和性能，不能声称零开销。
- 使用 DXGI 的独立光标可见状态，隐藏、位于其他输出或已内嵌于桌面时不
  请求叠加。协议位置是热点，转换时加上显示器原点和 shape hotspot；
  图像不额外旋转，保留已有协议的可见图像坐标约定。
- 彩色图像转换为预乘 BGRA；单色与 masked-color 图像保留 AND/XOR，
  支持反色，不把反色误转换为普通透明度。仅转换有界光标小图像；最大
  512×512、2 MiB，校验 pitch、热点、格式和完整长度后访问数据。
- Rust 接收端在像素分配前验证版本、序号、图像引用、尺寸及热点。
  静止等待允许空闲；首字节之后整份回复有 5 秒期限。失败只终止光标源。

语义依据：微软 [Desktop Duplication 的光标说明](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api)、
[Pointer Shape 类型](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/ne-dxgi1_2-dxgi_outdupl_pointer_shape_type)
及 [Shape Info 的热点定义](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/ns-dxgi1_2-dxgi_outdupl_pointer_shape_info)。
彩色源的 straight-alpha 解释与微软示例的 SRC_ALPHA 混合一致，参见
[OutputManager.cpp](https://github.com/microsoft/Windows-classic-samples/blob/main/Samples/DXGIDesktopDuplication/cpp/OutputManager.cpp)。
RustDesk 的独立形状/位置思路作为参考；此处使用 DXGI 可见性，避免单独依赖
GetCursorInfo 时无法区分已经内嵌的视频光标。

## 验证

```text
cargo test --locked -p pab-desktop-media --lib --test windows_cursor -j6
20 单元测试通过；1 个生成图像检查通过；1 个 GPU 实机测试默认忽略
```

生成图像检查运行真实 C++ 转换函数，共 52 个断言：黑/白/透明/反色与三种
背景色、非整字节宽度、行 padding、masked-color、半透明彩色、负坐标热点、
尺寸/pitch/热点/长度拒绝与整数溢出。新增 3 项 Rust 管道测试覆盖图像后连续
隐藏/移动、头部非法时不等像素、静止空闲与半包超时。

接入后重跑 `cargo test --locked -p pab-executor --lib local_ipc:: -j6`，
39 项全部通过，包括此前独立光标 helper 与原有窗口、截图、输入和凭据测试。
日志在 `.build/remote-desktop/windows-cursor-helper-regression.log`。

随后显式运行上述 GPU 测试，在本机 Windows/RTX 3060、150% DPI 的原生
采集环境完成两个代次，结果通过。两次均取得真实可见光标，图像 20,736 字节；
每代都没有请求视频帧，也没有注入鼠标键盘、打开新窗口或保存桌面截图。
光标关闭后可在同一活动视频配置重新建立源；视频关闭后保留光标句柄，
`backend.idle()` 仍在期限内返回，旧句柄不可再读，旧配置不可重开。
测试结束检查没有残留 `pab-media-worker.exe`。

### DPI 对照中的失败与修正

第一次独立热点对照失败：协议 `(2252,1435)`，测试观察者 `(1501,957)`。
没有给产品坐标乘系数绕过。只读对照发现本机无 DPI awareness 的进程调用
GetCursorPos/GetPhysicalCursorPos 都返回虚拟化坐标；把观察线程临时切到
per-monitor-v2 后，两者均返回物理坐标。对照示例为 `(1361,567)` 与
`(2042,850)`；恢复线程上下文后回到原结果。

测试观察者因此使用与原生采集一致的 per-monitor-v2 上下文，并在调用后
立即恢复。最终两代都在前后系统热点稳定的条件下与协议热点完全相等，
`physical_position_checked=true`，并非跳过坐标断言。
线程上下文恢复方式参见微软
[SetThreadDpiAwarenessContext](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setthreaddpiawarenesscontext)。

私有日志：`.build/remote-desktop/windows-native-cursor-tests.log`、
`windows-native-cursor-live.log`；首次 DPI 对照失败保留于
`windows-native-cursor-dpi-initial.log`，未用通过记录覆盖。

编译 Cargo 6 jobs，CMake 1 job、MSVC /MP1；检测到另一个明确 -j 2 的构建时
合计为 8，最后复跑无其他构建。没有替换本机安装版、修改 TCC 或远端服务。

## 未覆盖

本轮没有操作远端 Windows、没有真实多屏/旋转/热插拔、锁屏或 UAC 光标
验收，也没有证明原生窗口已显示光标。真实动态移动/形状切换及与视频内嵌
光标之间的同步，需要窗口叠加接入后联合验证。Mac 本轮按单屏验收条件执行，
不以 Windows 的通过结果替代 Mac/Linux 实现与运行结果。
