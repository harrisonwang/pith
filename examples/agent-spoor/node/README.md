# Node.js 智能体示例

需要 Node.js 18 和一个兼容 OpenAI 接口的模型服务。

```bash
npm install
cp .env.example .env

npm run native
npm run mcp
npm run skill

# 一次性提问
npm run native -- "用 data/byd.pdf 第 1 页总结关键财务数据"
```

在 `.env` 中填写 `BASE_URL`、`OPENAI_API_KEY` 和 `OPENAI_MODEL`。使用 Skill 时，还需要确保可以从 `PATH` 中找到 `spoor` CLI。

单独启动 MCP Server：

```bash
npm run mcp:server
```

Claude Desktop 配置示例：

```json
{
  "mcpServers": {
    "spoor": {
      "command": "npx",
      "args": ["tsx", "/绝对路径/examples/agent-spoor/node/src/mcp/spoor-server.ts"],
      "cwd": "/允许读取的文档目录"
    }
  }
}
```

参考实现拒绝直接使用 `../` 访问 `cwd` 以外的文件，但它不是安全沙箱。不要在含有不可信符号链接的目录中运行。三种调用方式的区别见[上级 README](../README.md)。
