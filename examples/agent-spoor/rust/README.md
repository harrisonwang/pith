# Rust 智能体示例

这是一个独立的 Cargo 项目，不在主仓库的 workspace 中。它直接使用仓库内的 `spoor-core`，并通过 `rmcp` 实现 MCP 客户端和服务端。

```bash
cp .env.example .env

cargo run -- --mode native
cargo run -- --mode mcp
cargo run -- --mode skill

# 一次性提问
cargo run -- --mode native \
  "用 data/byd.pdf 第 1 页总结关键财务数据"

# 不调用模型的基本测试
cargo test
```

使用 Skill 时，需要确保可以从 `PATH` 中找到 `spoor` CLI，也可以设置 `SPOOR_BIN`。

构建并单独运行 MCP Server：

```bash
cargo build --release --bin spoor-mcp-server
./target/release/spoor-mcp-server
```

Claude Desktop 配置示例：

```json
{
  "mcpServers": {
    "spoor": {
      "command": "/绝对路径/examples/agent-spoor/rust/target/release/spoor-mcp-server",
      "cwd": "/允许读取的文档目录"
    }
  }
}
```

参考实现拒绝直接使用 `../` 访问工作目录以外的文件，但它不是安全沙箱。不要在含有不可信符号链接的目录中运行。三种调用方式的区别见[上级 README](../README.md)。
