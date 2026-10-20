# macOS helper 启动与实际输入

日期：2026-10-17。基线 `89aa700`。Apple M4、macOS 27.0.1。

用户完成系统确认后，同一个测试包仍在输入前丢失焦点。加入仅针对候选包的
进程角色记录后确认：获得前台的 PID 是无参数的 Desktop GUI，既不是采集
worker，也不是授权窗口。Aqua helper 的 `applicationOpenUntitledFile` 在
启动阶段就被 AppKit 调用，原实现无条件转发为打开 GUI。

独立 AppKit 探针确认了回调顺序：launchd 直接启动也会收到 `oapp` 并调用
open-untitled，随后才是 did-finish-launching；用户显式打开已运行的 App
收到 `rapp`/reopen。修复在 helper 的启动完成前消化 open-untitled，完成后
仍转发显式打开，保留已有 reopen 路径。未修改正常 GUI 的启动入口。

## 实机结果

最终保持同一个隔离包路径与签名身份，只更新该包中的候选二进制；未覆盖
`/Applications` 安装版。完整 Desktop release 构建以 4 jobs 通过，媒体库
11 项测试通过。此前尝试的 native worker 激活策略修改没有解决根因，最终
实现未包含它。

通过 `macos_helper_check.py` 启动真实 Aqua LaunchAgent，与隔离 Executor
测试服务建立认证 IPC，连续两轮执行：

- 每轮 30 帧 1920×1080 实际桌面硬件编码、OpenH264 解码；每帧 CPU 像素
  映射为 0。没有把桌面码流写入文件。
- 每轮一次鼠标按下/抬起、A 键按下/抬起、滚轮和 Shift 按下/释放，测试窗口
  观察到的计数全部精确匹配。目标位置 (100,100)，实际 (99,101)。
- 第一轮显式 ReleaseAll、第二轮关闭输入通道均释放 Shift；关闭采集后旧
  注册退休，停止 helper 后注册清空。
- 测试窗口全程保有焦点，没有额外 GUI 进程；结束后候选进程、helper 和
  fixture 均为零。最终这轮没有观察到新增系统确认。

任务 `b967fae6-55e3-46b5-944e-edd696a11d81`。签名包 SHA-256：
`cca7e91a99b4bca38fb093d0266a9ce9eb5a3b50c9a648cbf4b3d32ec5dc3fbd`。
原始计数、基线失败及候选角色见 [机器记录](remote-desktop-macos-helper-startup-2026-10-17.json)。

另行验证了真实 LaunchServices `open`：helper 启动后只有后台进程；显式打开
候选 App 后出现一个正式窗口，再次打开复用相同 GUI PID，结束后候选进程
清空。使用独立 TOML profile 和关闭的本地服务端口。最初回归脚本误用了
snake_case 的偏好键名导致 GUI 无法启动，改为项目规定的 `controlUrl`/
`relayUrl` 后通过，未为测试修改产品配置读取规则。

## 权限状态的含义

安装版页面的屏幕录制/辅助功能检查已通过，不需要用户反复关闭重开。
该页面的 preflight 不等于检查了额外的系统直接采集确认；文件访问只检查
所列目录，也不代表完整磁盘访问。此前更换隔离包路径导致授权观察混乱，
现在固定候选路径与签名。此记录不承诺 macOS 永不再次要求系统确认。

这次通过的是本机真实 helper 的采集与输入，仍不替代产品网络窗口、动态
显示帧率、系统登录、多屏或长期稳定性验收；W4 继续保持未完成。
