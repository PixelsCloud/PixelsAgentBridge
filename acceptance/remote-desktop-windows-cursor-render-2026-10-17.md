# Windows 光标叠加与 Desktop 接入

接续 [Windows 光标源](remote-desktop-windows-cursor-source-2026-10-17.md)。
本阶段把独立光标流接到 Windows 原生窗口；Mac/Linux 叠加和实机跨机器验收
仍未完成，LT-03.5 保持未完成。

## 实现

- Windows Desktop 的每个视频代次分别运行视频、输入和光标任务，静止视频
  读取不阻塞光标。无能力或光标打开失败仍可查看/输入；光标源退出后清除最后
  一张光标，保持视频。切屏/关闭仍销毁旧原生窗口，不保留旧代次图像。
- 新增独立 `NativeCursorRenderer`，沿用已有私有管道互斥写入，保证光标、
  输入模式和压缩视频命令不串包。一次只提交一个光标；独立序号与消费确认
  不改变视频队列、视频 ACK 或显示帧统计。部分本地写入失败后禁止复用该
  handle，由 Desktop 关闭此原生代次，避免把半包当成新视频。
- 彩色光标使用 GPU 预乘透明混合；AND/XOR 图像仅复制光标覆盖区域到 GPU
  纹理后运算，不回读整帧或光标背景到 CPU。形状只在变化时上传，移动和隐藏
  复用现有图像。没有新增用户配置文件或鉴权流程。
- 在可见画面的 viewport 内计算桌面坐标比例、图像缩放与热点，不重复旋转
  光标。先用宽范围数值裁剪，再转为窗口坐标，避免负原点/极端位置溢出。
  不绘制到黑边；隐藏/清除后使用保留的视频帧重绘，不留下旧位置。
- 光标静态重绘不增加视频 `submitted/displayed`。启用操作、指针位于画面内
  且光标流有效时隐藏本地箭头；只查看、黑边、停止操作和光标流清除恢复箭头。
  未调用全局 ShowCursor 计数器，也没有全局鼠标钩子。
- 私有 worker 协议增加光标命令及输出事件 6，视频与输入原格式保持。
  Desktop 使用当前同一可执行文件启动 renderer；网络协议没有增加新要求。

## 已验证

GPU 离屏测试使用生成的 BGRA 背景与光标，只回读测试纹理，不读取用户桌面。
30 个组合覆盖彩色半透明、单色反色/黑白/透明、1 倍与 1.5 倍显示、桌面与
像素尺寸比例不同、竖向可见尺寸、非零热点、左边裁剪、极端屏外位置、隐藏和
清除。比较 1,966,080 个像素，最大通道误差为 0。

实际 Windows 原生窗口测试使用生成的 H.264 视频与光标。硬件解码和软件
解码分别只提交一帧视频，然后连续提交 8 个光标位置/隐藏更新并清除；没有
额外视频 presentation 通知，两次最终显示视频帧数均为 1。窗口结束后保留
光标 renderer handle 也不能继续提交。测试没有注入系统输入或替换安装版。

并发窗口测试同时提交 16 帧生成视频、16 次光标更新及清除，第一张为
512×512、2 MiB AND/XOR 图像，覆盖负屏幕原点与 2 倍桌面/像素比例。
在 20 秒期限内收到第 16 帧，视频与光标确认均完成，没有管道串包或阻塞。

Rust 媒体单元 22 项和原生形状检查通过。新增确认测试证明光标消费不产生
视频 ACK，不允许重复绑定或未提交的光标确认。3 项 GPU/窗口测试默认忽略，
已在本机显式执行通过。相关记录在 `.build/remote-desktop/windows-cursor-render-tests.log`
和 `windows-cursor-render-live.log`。

复现命令（Windows，先配置 NASM 和 Visual Studio，构建顺序执行）：

```powershell
$env:CARGO_BUILD_JOBS = '6'
$env:CMAKE_BUILD_PARALLEL_LEVEL = '1'
$env:_CL_ = '/MP1'
cargo test --locked -p pab-desktop-media --lib --test windows_cursor --test windows_cursor_render -j6
cargo test --locked -p pab-desktop-media --test windows_cursor_render -j6 -- --ignored --nocapture --test-threads=1
cargo build --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --features tauri/custom-protocol -j6
```

并行存在其他构建时，以上 6 个作业与其他构建总和仍不得超过 10。

包含最终原生窗口改动的完整 Windows Desktop 构建通过（`tauri/custom-protocol`，
6 个作业，增量构建 2 分 46 秒）；格式检查与 `git diff --check` 通过。
该结果只证明候选程序构建成功，不代替安装包与真实远端会话验收。

## 仍待验收

真实远端画面、Desktop 完整会话中的光标显示与输入模式切换，多屏/混合 DPI/
旋转/热插拔、系统登录场景，和动态视频同时更新光标时的稳定帧率与资源占用。
生成的像素与本机原生窗口结果不能代替上述实机验收。Mac 仍按用户确认的
单屏条件推进。
