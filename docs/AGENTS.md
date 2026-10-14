# 用户文档同步

- `USAGE_AND_SELF_HOSTING.md` 是官网中文使用与自我部署文档的 Markdown 版本。
- 修改这份指南的使用步骤、命令、配置或功能说明时，必须在同一次工作中同步更新相邻官网仓库 `PixelsAgentBridgeSite/web/src/docs.tsx`、`web/src/deployment-guide.tsx` 的对应内容，并核对官网英文说明。不得留下只更新一边的结果。
- 推荐先修改官网内容，再从官网仓库执行 `npm --prefix web run docs:sync` 更新 Markdown；执行 `npm --prefix web run docs:check` 验证一致性。同步脚本支持 `--target` 指定非默认项目路径。
- 官网纯视觉排版变更不要求修改 Markdown；与本指南无关的工程专题不强制同步到官网。
