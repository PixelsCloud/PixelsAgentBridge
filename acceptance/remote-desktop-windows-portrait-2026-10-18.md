# Windows 竖屏采集误判修复与负坐标验收

日期：2026-10-18；基线 `e714fe0`。本轮使用 90 已安装的 Parsec 驱动，
临时添加左侧副屏，主屏保持 (0,0)、1920×1080，副屏为 (-1080,0)、1080×1920。
不写入显示配置注册表；结束先恢复新副屏原模式，再仅移除本次返回的显示索引。

## 原问题与定位

旧 Desktop 正式窗口可枚举竖屏，选择后进入 switching/reconnecting，最终回到
主屏，未能显示竖屏，因此未执行点击。独立内嵌采集进程同样退出 1，报
`Screen changed; refresh generation`。第二次只查看选择复现相同恢复行为。

新增只读元数据探针 `windows_dxgi_layout_probe.cpp`，取得 90 的实际输出描述、
复制模式和首帧纹理描述，不映射、读取或保存桌面像素：

| 输出 | 桌面尺寸 | 复制模式尺寸 | 纹理尺寸 | DXGI 旋转值 |
| --- | --- | --- | --- | --- |
| 原主屏 | 1920×1080 | 1920×1080 | 1920×1080 | 1（identity） |
| 临时竖屏 | 1080×1920 | 1080×1920 | 1920×1080 | 2（90°） |

原校验把复制模式尺寸与未旋转的编码纹理尺寸比较，稳定竖屏也被拒绝。
现改为复制模式与桌面坐标尺寸比较，并验证复制对象旋转状态与枚举一致。
纹理仍用原始尺寸直接交给现有硬件编码器，实际纹理尺寸检查沿用编码器校验；
没有引入 CPU 像素复制、旋转或格式转换。此处桌面模式与采集表面的区别也符合
[Microsoft Desktop Duplication 文档](https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/desktop-dup-api#rotating-the-desktop-image)。

布局辅助程序以 `--lease-seconds N --layout left-portrait` 显式启用，仅允许原来
一屏且没有活动 Parsec 输出的环境；只选择新增且非主屏的 Parsec 输出。
使用 CDS_TEST 预检，再动态设置模式，独立线程维持显示租约。动态设置与持久化
标志的区别见 [ChangeDisplaySettingsExW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-changedisplaysettingsexw)。
十秒布局预检与二十秒元数据诊断均恢复原拓扑；无参数的辅助程序退出 2。

## 验证状态

完整 Windows Desktop 构建通过（6 分钟，3 jobs），23 项媒体单测通过。
候选 Desktop SHA-256 为 `413f3a9d39835ce004210747449da2e1b3af58f9b4517ba1c94b5fd74ac4eb06`。
同一竖屏的独立采集初始化已从旧版本退出 1 变为退出 0，返回 1920×1080
编码头、`nvenc_gpu readback_per_frame=0`。本机原生旋转 renderer 初始化也通过。

正式窗口再次选择竖屏仍无法出图，持续重连，尚未执行任何点击/按键。因此四象限
颜色/坐标、窗口缩放及切屏释放并未通过。取一个压缩帧的进一步对照中，硬件和
软件编码均返回 `AcquireNextFrame HRESULT=0x887a0026`（ACCESS_LOST）。

无编码器的原生 DXGI 对照进一步缩小范围：普通设备与启用 VIDEO_SUPPORT 的
设备都能获取首个纹理，LastPresentTime=0、AccumulatedFrames=0；对同一复制
对象继续取第二帧即 ACCESS_LOST。失败发生在帧获取层，无需编码器即可复现。
这仍不足以确定驱动内部根因，或宣称其他真实旋转显示器存在相同问题。

两个实验修改——接受首个零呈现时间帧、先创建编码器再创建复制对象——均未修复
该问题，已撤回。最终产品代码仅保留已证实的模式尺寸/旋转校验修正。后续需要
处理该采集后端的兼容性或验证真实物理旋转显示器；不得以改用横屏关闭竖屏要求。

日志首次在写入期间尝试取哈希失败；等待辅助进程结束后重新下载并逐一校验，
原失败保留。完整证据和收尾状态见
[结构化记录](remote-desktop-windows-portrait-2026-10-18.json)。

收尾确认所有租约均恢复原主屏拓扑；90 原服务 Running、恢复标记存在、暂存
进程 0、输入释放标志 0。测试任务均停止后删除，精确暂存目录已清理；本机两份
隔离候选进程已退出，安装版未改动。独立 worker 已重新构建回最终产品源码，
避免后续误用撤回实验的产物。编译总分配未超过 10 jobs。

本轮不覆盖混合 DPI、270° 实机、Linux/Mac 多屏、实际 FPS 或长时稳定性。
也不解决 Windows 不激活窗口线程的首次点击边界。完整交付条目不能据此关闭。
