# Mac 独立光标采集阶段记录（2026-10-18）

后续更新：用户解锁后，真实光标七阶段及固定 App 的视频/光标共存已有通过证据。
测试 FIFO 转接不能证明父进程取消，正式 helper 生命周期仍未验收。
见[后续实机记录](remote-desktop-assistance-mac-boundary-2026-10-18.md)；以下保留早期轮次原始状态。

状态：实现已接入开发分支，**实机功能验收未通过，不能关闭 W2/W4/W7**。
没有替换安装版或此前已授权的固定测试 App；没有执行锁屏、注销或修改 TCC。

## 实现

- 将 Windows 私有光标管道适配器移至 `cursor_source.rs`，Mac 复用其会话/代次
  检查、单光标进程、按需读取、有界包解析和视频销毁时取消/回收机制。
- Mac session helper 透传现有光标能力；视频使用 `desktop-separate-cursor`，
  ScreenCaptureKit 关闭内嵌光标。直接诊断的 `desktop` 模式仍包含光标。
- 独立 worker 读取系统形状、热点、位置和可见性；只转换最多 512×512 的形状，
  不获取视频 IOSurface、不增加视频整帧 CPU 读取或转换。位置变化不重发形状。
- 形状按 NSImage 逻辑尺寸和选定屏幕的采集缩放转换为从上到下的预乘 BGRA；
  不能直接拿 TIFF 表示的像素尺寸当屏幕尺寸。Apple 实测箭头逻辑尺寸 28×40，
  TIFF 表示为 280×400，热点为 (5,5)。
- 每 250 ms 校验选中屏幕的原点、范围及采集像素尺寸；变化后终止旧源，由既有
  会话恢复路径处理。保留单个最新状态，没有移动历史队列。

参考本地 RustDesk `src/platform/macos.rs` 的系统光标及 seed 获取方式，未复制
它的像素转换实现。Apple 将
[currentSystemCursor](https://developer.apple.com/documentation/appkit/nscursor/currentsystem?language=objc)
与 [CGCursorIsVisible](https://developer.apple.com/documentation/coregraphics/cgcursorisvisible())
标记为弃用；后者无替代 API。可见性符号运行时解析，缺失则明确失败；
`CGSCurrentCursorSeed` 仅作优化，缺失时比较形状像素。seed 读取期间有变化时
不丢弃已经取得的完整形状，只取消下一次缓存复用，避免频繁设置光标导致饥饿。
[showsCursor](https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/showscursor?language=objc)
控制视频是否带光标。旧版本系统兼容性仍待实测。

## 已有证据

- Windows `pab-desktop-media --lib`：22 项通过（4 jobs），共享适配器改名后行为回归通过。
- Apple 媒体库：23 项通过（4 jobs），包含原有光标包顺序、长度和取消测试。
- Apple 完整 Release Desktop、worker 和 `native_cursor_check` 编译通过；最终
  源版本与产物哈希见配套 JSON（以最终构建记录为准）。
- 原生生成形状检查：48 个颜色通道与独立 sRGB 参考比较（允许量化误差 2），
  验证上下方向、BGRA、预乘透明度；3 组逻辑尺寸/缩放检查；8 组非法几何拒绝。
- 新增 `native_cursor_check`，用于验证无视频请求时光标独立工作、旧配置拒绝、
  光标单独重开、保留空闲光标 handle 时视频销毁仍能退出进程。**此例程目前仅
  编译通过，不能写作真实生命周期通过。**
- 实际 worker 的只读管道检查通过：旧屏幕几何拒绝；当前隐藏状态首包有效；
  未请求时无推送；关闭父输入管道后约 13 ms 退出，显式停止约 31 ms 退出。
  未请求视频帧或发送 OS 输入；这不是完整视频租约回收检查。
- 最终构建任务 `d0a63503-81ed-4e1f-a5c0-559a96600cf9` 成功，Desktop SHA-256
  `3d0e963a9923e4ae7f0bc0dac998c3a5f10122c0a17e4b946dced44482119b22`。
  新建并签名 `Pixels Cursor Source Test.app`，仅准备待测，没有启动它或替换旧包。
  最后只读检查没有遗留本轮 worker/fixture/测试 App 进程。

## 未通过轮次与环境

实际光标测试初始收到了合法隐藏包（sequence 1、image_id 0），随后等待可见包
超时。之后只读检查确认 `CGSSessionScreenIsLocked=true`；前台用户仍为 501。
因此普通 Aqua 会话的箭头/自定义光标 fixture 无法作为前台桌面验证。
这些轮次保留为失败，不能归因于已证明的 seed 竞争，也不能据此认定隐藏切换通过。
后续驱动已增加活动用户且未锁屏预检查。已请求用户解锁，继续其他可独立工作。

测试 fixture 的编译曾发现测试代码的 float/double 初始化列表不匹配，已修正；
另一次命令行未正确引用部署版本参数，编译器把 `.0` 当文件名，修正调用后通过。
这些不是产品运行错误。首次最终构建调用早于上传结束，脚本缺失后终止；确认上传
完成才重新启动构建，没有同时运行两个构建。并发峰值为 Apple 4 jobs 加独立
单文件编译 1 job；Windows 回归 4 jobs 运行时尚未启动 Apple Cargo 构建。

管道测试的首次停止断言也曾失败：Python `communicate(input=stop)` 同时关闭
父管道，worker 可能先观察到 EOF 并以取消错误退出。测试改为保持管道打开，
等待 stop 被处理并退出，再关闭；EOF 与正常 stop 分别验证。未因此修改产品协议。

## 继续验收

解锁后先运行有边界的自有窗口 fixture，观察箭头、自定义颜色/热点、隐藏/显示、
位置变化与不重传形状，再验证真实视频无双光标及生命周期。随后覆盖正式独立窗口、
源故障与重开、性能和系统登录。Mac 单屏按用户确认范围执行；其他平台组合和
旧 macOS API 兼容性保持待验收。此记录不证明 FPS、登录、网络矩阵或长时间稳定性。

可复用驱动为 `experiments/remote-desktop/check_macos_cursor_source.py WORKER FIXTURE`；
fixture 源为 `crates/desktop-media/tests/native_macos_cursor_source.mm`。
驱动会拒绝锁屏环境，限定读取/退出时限，只作用于自有窗口。真实形状检查尚未通过，
不能把驱动本身当作验收证据。
