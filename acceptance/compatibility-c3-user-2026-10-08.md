# C3 Windows 普通用户安装态基线

原生 pixels 工具连接 90（211399447），安装版本 1.2.32，WTS 会话 2，账户
pxrdp_2c3153bd5fff41，SID 尾号 1099。没有创建、提权或注销用户。

- whoami /all：Users、Remote Desktop Users，没有 Administrators；Medium Mandatory Level。
  命令任务 a7c210e4-98d3-4672-9c09-1de282ffd978。中文系统输出在旧 MCP 中乱码，C1 新参数验收待宿主更新。
- 用户目录 UTF-8 写入/回读一致（28 bytes），SHA-256
  f96a0c7a5cf7aff9de37e3366423b0dade5e92ee95f750f8c843b33947e3f834。
  写操作 ea442d6d-1975-4b70-a555-009c9e3fbf5c，读取 e11efb7b-81e4-462a-a7e7-82db2b0799c0。
- Get-Acl 所有者和命令实际 SID 均为该普通用户。安装目录写入
  ce190b3b-76a5-45e7-afa2-a0819a4e78e4 返回 access_denied；只读复查文件不存在，没有 SYSTEM 回退。
- PTY 791997d0-2346-40ab-a068-947e39c915ef 返回同一 SID，随后正常关闭。
- 将 user 引用误用于 desktop_user 的应用查询 bf16aac0-f07f-4d75-beb1-1dcc62aec3b7
  明确拒绝 execution mode does not match，不借用 Administrator 桌面。

当前普通账户没有可用 desktop_user helper；未冒充通过普通用户应用启动、前台焦点或 UAC 交互。
这些需要可用普通用户交互桌面，不能通过干预日常会话来伪造。Release 安装后的命令/文件/PTY
重验仍待执行。测试文件位于该账户目录，待本轮升级回读后清理。
