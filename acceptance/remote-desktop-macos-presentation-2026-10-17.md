# macOS 权限归因与真实显示确认

日期：2026-10-17。变更基线：`b01cf79`。测试设备：Apple / M4 / macOS 27，
单屏 1920×1080。所有产物位于原隔离测试目录，没有替换安装版。

## 权限结论更正

此前通过 Executor → launchctl asuser → sudo → 测试 App 的 `--macos-check`
得到两项权限 false。用户指出未看到 App 的授权申请后，复核发现这条路径只做
诊断，且 GUI bootstrap 不会自动改变 TCC 的 responsible process：系统日志
仍将诊断进程归因于后台 Executor。这个结果不能用于判定正式 App 缺少授权。

通过 LaunchServices (`open -n`) 正常启动同一个签名 App，确认其主窗口可见。
App 已有启动权限申请和设置页手动申请入口；本次无需新增权限接口或更改 TCC。
随后同样由 LaunchServices 启动诊断，实测 `screenRecording=true`、
`accessibility=true`、`activeConsole=true`、活动显示器 1 个。系统 TCC 日志
对应的主体是 `vip.rgaa.pab.desktop`，与后台诊断归因不同。

测试包原签名二进制 SHA-256：
`252568145762bd183074ae28ab80107191d42df45265fde9bd901373780c2177`。
旧诊断保留在历史 JSON 中并明确标注上下文，新增正常启动复查结果。没有修改
系统授权数据库、使用私有 entitlement、替换用户的安装版或要求用户重复授权。

## 实际采集和发现的问题

使用该签名 App 内嵌 worker，以 LaunchServices 启动，采集**真实桌面**：

- 15 FPS 配置：60 帧，采样时间跨度 4,029,839 微秒。
- 30 FPS 配置：90 帧，采样时间跨度 3,295,103 微秒。
- 两组均为 1920×1080、VideoToolbox 硬件编码；实际采集路径没有逐帧 CPU
  像素映射，压缩包的序号、长度、关键帧和时间顺序通过检查。

这是采集/编码链路观察，没有动态画面唯一性测量，不能用配置 FPS 或包数
宣称网络显示性能达标。原始桌面码流仅保存在隔离私有目录，不加入仓库。

把第一组真实桌面码流交给旧 native renderer 时，接收与“已显示”事件均为
60，但 Metal 正时间戳呈现统计只有 58。旧代码在 `presenter.render()` 返回
后立即发出显示事件；该返回仅表示 GPU 提交，窗口遮挡或丢弃 drawable 时
不等于物理呈现。Rust 内嵌宿主也存在同样的提前确认。

## 修复

Mac 宿主通过 native poll 获取 Metal 的真实呈现计数，仅在正时间戳回调确认
后报告显示事件。解码线程等待当前帧显示，保留原有四包上限，避免隐藏窗口时
继续累积 UI 任务。停止/失败唤醒等待线程；静帧恢复仍由 native 保留帧重绘。
native 独立 worker 也等待真实呈现，关闭输入管道可取消等待。

新增 `macos_host_check`，直接使用生产 `MacNativeViewer`，仅显示仓库中的
生成像素，不采集屏幕、不发送系统输入。Apple 实机已通过默认/软件解码两组：
每组 8 帧依序显示；隐藏窗口期间不发显示确认；恢复后无须新帧即可呈现；
等待隐藏帧时取消，运行时正常结束，无滞留阻塞解码线程。

## 验证范围

媒体库 11 项测试通过；完整 Desktop Release 构建通过，Cargo 4 jobs、嵌套
原生构建 1 job，顺序构建，未与其他编译叠加。Desktop 可执行文件 23,907,264
字节，SHA-256 `8ba4c060344d0b9c3f30056f7129efc0af463db768482498c4afb31fd17cc4ed`。

使用同一发布签名制作新的隔离测试 bundle，内嵌 worker 重放上述实际桌面
码流，15/30 FPS 来源 × 硬件/软件解码四组均通过：分别 60/60、60/60、90/90、
90/90，显示事件与 Metal 实际呈现计数完全一致。硬件解码未逐帧 CPU 映射，
软件解码保留其必要映射。未降低原失败断言，也没有把实际桌面按生成测试色
校验；这些记录中的颜色采样数为 0。

测试结束后关闭隔离 GUI，自有媒体测试进程为零，删除 7 个包含实际桌面码流
的临时文件。只保留元数据和可复用二进制。构建、签名包哈希及运行结果见
[机器记录](remote-desktop-macos-presentation-2026-10-17.json)。

可复现的宿主检查：在 Apple 图形会话运行
`cargo run --locked --release -j 4 -p pab-desktop-media --example macos_host_check`。
其中 AppKit 主线程和生成像素窗口由检查程序独占；实际桌面采集必须另行通过
正常 App/launchd 启动路径验证，不能继续从后台命令的权限结果推断 App 权限。

本项不关闭 Windows↔Apple 产品连接、真实键鼠、LoginWindow、九方向或长期
稳定性验收，也不把 App 的 TCC 授权结果直接套用到后台服务的启动方式。
