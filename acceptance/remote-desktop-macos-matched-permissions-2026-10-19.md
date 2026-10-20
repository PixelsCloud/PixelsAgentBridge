# Apple 当前测试包与既有系统授权

日期：2026-10-19。接续[同版本构建记录](remote-desktop-macos-matched-build-2026-10-19.md)，
本次将已构建的 Desktop 更新到用户之前授权的固定测试 App。

固定路径为 `/private/tmp/pab-mac-media-99f879354739/Pixels Remote Desktop Test.app`。
更新前完整备份原 App，保留 bundle identifier 和签名身份，更新后验证签名。
源码构建产物 SHA-256 为 `7956bbdf49e70a7fe7929e0b5bfaa43201595214c75b5de35d56749ded860385`，
签名后 SHA-256 为 `fde65cc7a99899b6a0a4714a8af3f060c1dd8c8495204baf9262777cea6d1954`。
没有更新 `/Applications` 安装版或系统服务。

从当前图形会话经 LaunchServices 启动同一 App 的 `--macos-check`，结果为：

- 当前控制台会话有效，单屏枚举成功。
- 屏幕录制、辅助功能权限均已授予。
- 本地访问配置存在。
- 没有重新申请权限或修改系统权限数据库。

这是当前包的授权检查，不代替锁屏/注销后的实际采集、系统密码登录和恢复验收。
用户已允许锁屏和注销测试；系统登录凭据路径或手动恢复方式仍待提供。
之前构建记录中的“未更新测试包”描述保留为当时状态，本记录记载后续更新。

更新回执、检查结果与任务记录哈希见[结构化证据](remote-desktop-macos-matched-permissions-2026-10-19.json)。
