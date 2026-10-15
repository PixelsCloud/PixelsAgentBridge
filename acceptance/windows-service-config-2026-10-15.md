# Windows 服务配置升级修复

## 问题与修复

1.3.0 安装器给已有服务的启动命令增加 `--config` 参数时，通过 Windows
PowerShell 5.1 调用 `sc.exe config`。带空格的可执行文件和配置路径中的内嵌
引号未被正确传递，`sc.exe` 返回 1639 和参数用法，安装器最终报退出码 1。

安装器改为 `Invoke-CimMethod Win32_Service.Change`，以字符串参数传递完整命令，
保留路径引号；同时检查 API 返回码，并重新读取服务配置确认启动命令、LocalSystem
账号和自动启动模式全部生效。配置已匹配时不执行修改。新安装仍使用 `New-Service`。

## 已通过的验证

管理员权限下使用实际 Windows PowerShell 5.1 执行：

```powershell
powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File packaging/desktop/windows/test-service-config.ps1
```

- 创建唯一名称的临时 Windows 服务，使用带空格路径复现旧调用，得到退出码 1639。
- 新函数将旧 `--service` 命令正确迁移为带引号的 `--service --config` 命令。
- 连续两次执行成功，服务保持停止状态，不运行测试命令。
- 从 LocalService / 手动启动正确修改为 LocalSystem / 自动启动。
- 不存在的服务明确报错。
- 测试结束删除临时服务，未更新本机已安装的 Pixels 应用。

本次验证覆盖导致用户截图故障的真实服务管理操作。用户机器升级由用户手动执行，
不将临时服务测试记录为 Company 的完整安装成功。
