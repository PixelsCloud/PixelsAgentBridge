# 安装包发布与在线升级

官网负责版本分配和发布记录，COS 保存安装包。Windows x86_64 与 macOS Apple Silicon 分别递增安装包版本；Rust、npm、Tauri 内部版本保持原值。官网地址默认为 `https://agent.rgaa.vip`。

首次使用时，给构建机配置私有环境变量 `PAB_RELEASE_API_TOKEN`（官网发布令牌）和 `PAB_RELEASE_PRIVATE_KEY_FILE`（32 字节 Ed25519 发布私钥的路径）。密钥、令牌、COS 凭据不能提交到 Git。官网部署机单独保存 COSCLI 配置，构建机不需要 COS 长期密钥。当前不购买 Apple Developer ID，也不做 macOS 安装包签名或公证；发布清单仍必须使用 Ed25519 签名。

从对应系统的源码目录执行：

```sh
python scripts/release.py windows --notes-zh "本次更新内容" --notes-en "Release notes"
python scripts/release.py macos --notes-zh "本次更新内容" --notes-en "Release notes"
```

每次只选对应系统的一条命令。脚本会预留版本、增量构建完整 EXE/PKG、核对 SHA-256 清单、签名、直传 COS、请求官网校验并发布，最后查询公网更新接口。若中断，原地使用同一命令加 `--resume`；`.build/release-<平台>-<架构>.json` 保存这次发布的 `build_id` 和阶段，成功后自动删除。不要删除未完成状态后再重新发布，否则会浪费版本号。

管理员在 `https://agent.rgaa.vip/admin/releases` 可查看发布历史、未完成上传及最近一次校验错误，并回滚当前指针。手工补发时先在页面预留版本，使用与该版本完全一致的安装包文件名，再在本机运行：

```sh
python scripts/sign_release.py .build/packages/pixels-agent-bridge-windows-x86_64-release-1.2.75-setup.exe
```

将输出的 Base64 签名粘贴到页面，选包并填写中英文更新说明。页面和脚本使用相同的上传与校验 API。回滚仅改变官网当前发布，不强制已升级客户端降级；目标对象必须仍在 COS 且与数据库哈希一致。

客户端每天检查一次新版本，也可在“设置 → 关于”手动检查。客户端下载后核对平台、版本、来源、文件长度、SHA-256 和 Ed25519 签名，再启动完整系统安装器。支持升级的首版仍需手工安装一次；旧版没有升级入口。升级会短暂中断控制和 MCP 会话，但用户数据与设备身份保留。macOS 图形安装器会要求管理员密码，未签名包可能显示系统提示。Windows 测试请优先在测试机进行，不重装正在被其他 session 使用的本机。

签名私钥应离线备份。轮换时先发布一个内置新旧两把公钥的客户端，再把官网切到新签名；待旧客户端过渡后删除旧公钥。直接替换当前单公钥会使旧客户端拒绝新版本。
