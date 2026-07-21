# 示例目录

第一次了解 spoor，先看这两个：

- [`answer-trace`](answer-trace/)：查找模型引用的出处，并在原 PDF 中标出位置。
- [`cloudflare-pages`](cloudflare-pages/)：在浏览器本地或 Cloudflare Pages 中解析文档。

其他示例可以按运行环境选择：

| 示例 | 场景 |
| --- | --- |
| [`agent-spoor`](agent-spoor/) | 直接调用、MCP 和 Skill 三种用法；正式 MCP Server 尚未发布 |
| [`local-corpus-explorer`](local-corpus-explorer/) | 在浏览器中批量解析、搜索和导出 JSONL |
| [`cloudflare-worker`](cloudflare-worker/) | 简单的 Worker API |
| [`rust-core-embed`](rust-core-embed/) | 在 Rust 程序中直接使用 `spoor-core` |
| [`tauri-desktop`](tauri-desktop/) | Tauri 2 桌面端 |
| [`electron-desktop`](electron-desktop/) | Electron + Node.js 包 |
| [`serverless-lambda`](serverless-lambda/) | AWS Lambda |

`wasm/demo` 是仓库内部测试页。公开部署前，请按[安全说明](../SECURITY.md)自行加入身份认证、限流、进程隔离和超时控制，并避免在日志中记录敏感内容。
