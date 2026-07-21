# 让智能体调用 spoor

同一个简单智能体，用三种方式调用 spoor：

| 调用方式 | spoor 在哪里运行 | 适合场景 |
| --- | --- | --- |
| 直接调用 | 和智能体在同一进程；使用 Rust、Python 或 Node.js | 需要较低延迟，或要直接读取警告、页码和坐标 |
| MCP Server | 独立进程，通过标准输入输出通信 | 供 Claude Desktop 等 MCP 客户端调用 |
| Skill | 智能体按照 `SKILL.md` 调用 CLI | 智能体可以执行命令，但不能直接调用新的程序接口 |

三个示例回答相同的问题，只是调用 spoor 的方法不同。目录按语言分开：

- [Node.js](node/)：`npm run native|mcp|skill`
- [Python](python/)：`uv run python -m app --mode native|mcp|skill`
- [Rust](rust/)：`cargo run -- --mode native|mcp|skill`

每套实现都附带 PDF、CSV 和含图片的 DOCX，用来检查页码筛选、表格读取、警告和图片提取。

> 这些 MCP Server 只是参考实现，不是已经发布的官方 `spoor-mcp` 包。

## 安全限制

- 参考实现拒绝直接使用 `../` 访问工作目录以外的文件，但它不是安全沙箱。不要在含有不可信符号链接的目录中运行。
- 使用 Skill 时只能运行 `spoor` 命令，不允许管道、重定向或其他命令。
- 单次最多返回 96 KiB，超出后会提示模型只读取相关页、行或列。
- 示例会调用外部模型。即使文件在本地解析，解析后的文本仍可能发送给模型服务。
