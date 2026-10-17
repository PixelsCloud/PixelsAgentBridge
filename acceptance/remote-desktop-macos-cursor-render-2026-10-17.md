# Mac 操作端：独立光标渲染阶段记录

记录跨度：2026-10-17～18（Asia/Shanghai）。基线 `ff34143`。
本轮接入 Mac 操作端的独立光标显示，目标先覆盖已有 Windows
光标源。Mac 被控端的独立光标采集、真实跨机器窗口、系统登录、多屏及稳定性
不由本轮关闭；Mac 当前按用户指定的单屏条件验收。LT-03.5、W2、W4 保持未完成。

## 实现

- Desktop 在 Mac 上复用现有光标会话与可靠传输，认证、单观看者和视频协议不变。
  共用 Windows 原有的私有命令编码，Windows 发送逻辑保持。
- `MacCursorRenderer` 单独持有光标 ACK，最多一个待消费提交。AppKit 主线程上传
  小块形状纹理；移动/隐藏只更新参数。取消或失败后句柄不可继续使用，关闭窗口
  后已排队的旧代次工作跳过，空闲句柄不延长窗口生命周期。
- 原生 Metal 视频 fragment 同时计算光标合成。预乘 BGRA 与 AND/XOR 两种形状
  均支持；热点按可见像素比例缩放，位置按桌面坐标映射，裁剪不画到留白区域。
  不增加整张桌面的 CPU 回读、颜色转换或额外整帧纹理复制。
- 静帧保留原视频图像用于光标重绘，沿用去重的视频呈现计数。光标源失败时清除
  叠加和独立跟踪状态，视频/输入继续；本地渲染失败则结束该 native generation。
- 启用操作且光标通道正常时，通过 AppKit 的 view cursor rect 使用透明本地指针，
  范围仅限远端图像区域。此项已实现，实际窗口内的隐藏/恢复仍待解锁后验收。

## 已有检查与范围

Apple M4 / macOS 27.0.1，原生 Metal 生成纹理检查：

| 检查 | 结果 |
| --- | --- |
| 4 种画面方向 × 2 种光标格式 × 2 种缩放，含中心、边缘和极端离屏位置 | 80 组逐像素检查通过 |
| 隐藏、清除及跟踪状态 | 32 组通过 |
| 比较像素 | 7,864,320（另有隐藏/清除整幅相等检查） |
| 最大通道误差 | 1；仅预乘透明混合允许 1 的量化误差，AND/XOR 必须完全相等 |
| 截断命令拒绝 | 80 次通过 |
| Windows 媒体库单元测试 | 22 项通过 |
| Mac 媒体库单元测试 | 20 项通过，含取消提交和停止后跳过旧 UI 工作 |
| 最终 Mac release 构建 | 完整 Desktop、媒体 worker、宿主检查程序均通过 |

最终源码归档、各源文件与产物 SHA-256、远端任务编号保存在
[结构化记录](remote-desktop-macos-cursor-render-2026-10-17.json)。最终 Desktop
哈希为 `ef0119267c6bf709f279d0d868eaec7d6894225cbbaaa7186d2799fed3f73170`。
独立 GPU 检查进程已完成，临时检查二进制已删除；构建产物留在隔离缓存。

GPU 测试直接编译并运行产品使用的 `video_shader.h` 和 `CursorOverlay`，独立 CPU
参考按像素中心计算位置和颜色。使用生成的 NV12 与光标，不采集屏幕、不注入
系统输入、不申请 TCC 权限。**该结果不是实际产品跨机器显示通过，也不是 FPS 测试。**

测试源码：[`native_macos_cursor.mm`](../crates/desktop-media/tests/native_macos_cursor.mm)。
在 Mac checkout 下使用单个编译任务复现：

```sh
xcrun clang++ -std=c++17 -O2 -fobjc-arc -fblocks -mmacosx-version-min=12.0 \
  -Icrates/desktop-media/native/macos crates/desktop-media/tests/native_macos_cursor.mm \
  -o /tmp/pab-macos-cursor-check \
  -framework Foundation -framework AppKit -framework Metal \
  -framework ApplicationServices -framework VideoToolbox -framework CoreMedia -framework CoreVideo
/tmp/pab-macos-cursor-check
```

## 未通过的窗口检查

`macos_host_check` 已加入硬/软解码、普通/奇数尺寸下的光标 ACK、静帧重绘不增加
视频帧数、关闭窗口后句柄失败等检查。本轮一次 GUI 启动在等待呈现时超时，
之后只读检查确认 `CGSSessionScreenIsLocked=true`，UID 501 仍在控制台。
测试进程已退出，不能把这轮记作窗口检查通过，也尚未排除解锁后的其他问题。
已请用户方便时解锁，等待后复测；没有修改锁屏状态、账号密码或自动登录。

两个已获授权的固定测试 App 的签名、二进制哈希保持，原 Executor 仍加载。
没有替换本机或 Apple 安装版，没有更改 TCC。构建使用独立源码与缓存，Mac 4
任务、Windows 回归 4 任务，同期总额 8；另一次单任务 Metal 检查时总额不超过 5。

2026-10-18 后续：用户恢复 Apple 后，宿主四组检查和真实 Mac→90 窗口已复测
通过；本地指针隐藏、停止操作与光标源退出后的恢复也通过。上述失败保留为
历史记录。详情及 JPEG 证据限制见 [窗口复测记录](remote-desktop-macos-cursor-product-2026-10-18.md)。
下一步补 Linux 操作端及 Mac/Linux 独立光标源、登录桌面与持续性能。

API 依据：[Apple Metal 纹理接口](https://developer.apple.com/documentation/Metal/MTLTexture)、
[AppKit cursor rect 重建](https://developer.apple.com/documentation/appkit/nsview/resetcursorrects())。
