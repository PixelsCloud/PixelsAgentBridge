# Pixels Android

Android 9（API 28）及以上的原生被控客户端，使用 Kotlin、Compose 和共享 Rust 核心。
通过现有控制服务注册设备，并通过 P2P 或 Relay 接收已认证的远程操作。

## 使用与权限

首次打开应用，配置与操作端一致的服务器，按首页提示准备权限并开启连接。
设备身份、密码和设置保存在应用本地；升级必须保留数据与原签名。
配置使用 TOML，设备凭据通过 Android Keystore 加密。

- 远程界面操作需要无障碍权限；截图按 Android 版本使用系统支持的授权方式。
- 文件操作支持应用工作目录 `/workspace` 和系统选择器授权的 `/shared-…` 目录，
  目录的只读权限或授权撤销会约束后续操作。
- 系统与应用信息受 Android 权限和包可见性限制，进程与日志查询限当前应用。
- 命令与终端在应用自身身份下运行；TCP 端口转发使用应用可访问的网络。
- 文档解析与离线 OCR 使用共享协议和隔离解析服务；Office 版面渲染未提供。
- 浏览器自动化尚未接入；不支持的能力会明确返回错误。

开机恢复默认开启，但只恢复用户此前开启的连接。明确停止、暂停和系统后台限制
仍然有效；开机恢复不会自动启动屏幕采集。详见：

- [启动权限与开机恢复](STARTUP_PERMISSIONS.md)
- [统一界面工具](UNIFIED_TOOLS.md)
- [通知管理](NOTIFICATIONS.md)
- [应用包管理](PACKAGE_MANAGEMENT.md)

## 工程结构

| 模块 | 职责 |
|---|---|
| `app` | Compose 界面、ViewModel 和依赖组装 |
| `core` | 平台无关模型、接口和有界指令调度 |
| `platform` | 无障碍节点、手势、系统导航和截图 |
| `transport` | JNI、连接、TOML 设置和加密凭据 |
| `../../crates/android-agent` | 共享协议与传输的 Rust 原生库 |
| `../../crates/device-session` | 设备认证与在线操作方管理 |

`platform` 与 `transport` 只依赖 `core`，由应用组合根组装。
固定工具版本由 Gradle 配置与构建脚本维护；JNI 和 APK 属于生成产物，不提交到 Git。
第三方图标及离线文档依赖保留版本、哈希和许可证。

## 构建与安装

需要 JDK 21、Android SDK/NDK 和对应的 Rust Android target。
通过 `ANDROID_HOME` 或 `ANDROID_SDK_ROOT` 指定 SDK，
通过 `PAB_ANDROID_JAVA_HOME` 指定 JDK。
从仓库根目录运行：

```powershell
python scripts/build_android.py check-shared --abi arm64-v8a
python scripts/build_android.py assemble --abi arm64-v8a
python scripts/build_android.py check
```

`assemble` 依次构建 Rust 与 Gradle。全部编译任务（包括嵌套构建）的总并发不超过 10。
如需构建后自动覆盖安装，在本地 `.build/android/device.toml` 中配置：

```toml
serial = "YOUR_ADB_DEVICE_SERIAL"
```

未配置设备时仅构建。安装使用 `adb install -r`；设备未授权、离线或签名不匹配时
报告错误，保留已构建 APK。不要通过卸载、清数据或更换签名来绕过更新失败。
调试 APK 位于 `apps/android/app/build/outputs/apk/debug/app-debug.apk`。

## 验证

```powershell
python scripts/build_android.py instrument
python scripts/build_android.py regression
python scripts/build_android.py test-native
python scripts/build_android.py test-files --jobs 2
```

`check` 只读检查；`verify` 会先修复格式，再检查、测试和构建。
整体验证入口为 `scripts/accept_android.py`，配置样本见
[acceptance.example.toml](acceptance.example.toml)。本机设备配置、身份凭据、截图和
运行报告留在 `.build/android/`，不提交到 Git。系统权限由用户通过正常界面授权。
实际网络切换、厂商后台限制和长时间运行需要在目标设备上单独验证，
构建通过不代表这些场景已经验收。
