# Python 智能体示例

需要 Python 3.11、[`uv`](https://docs.astral.sh/uv/) 和一个兼容 OpenAI 接口的模型服务。

```bash
uv sync
cp .env.example .env

uv run python -m app --mode native
uv run python -m app --mode mcp
uv run python -m app --mode skill

# 一次性提问
uv run python -m app --mode native \
  "用 data/byd.pdf 第 1 页总结关键财务数据"
```

在 `.env` 中填写 `BASE_URL`、`OPENAI_API_KEY` 和 `OPENAI_MODEL`。使用 Skill 时，还需要确保可以从 `PATH` 中找到 `spoor` CLI，也可以设置 `SPOOR_BIN`。

Claude Desktop 配置示例：

```json
{
  "mcpServers": {
    "spoor": {
      "command": "uv",
      "args": ["run", "python", "-m", "app.mcp_server.spoor_server"],
      "cwd": "/绝对路径/examples/agent-spoor/python"
    }
  }
}
```

MCP Server 只接受工作目录中的文件路径。它不是安全沙箱，不应在含有不可信符号链接的目录中运行。三种调用方式的区别见[上级 README](../README.md)。
