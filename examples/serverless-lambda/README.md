# AWS Lambda 示例

把 Linux 版 `spoor` CLI 放进 Lambda Layer 的 `/opt/bin/spoor`，处理函数再通过独立进程解析文件。请求格式：

```json
{ "filename": "report.pdf", "body": "...", "isBase64Encoded": true }
```

本地集成测试：

```bash
cargo build -p spoor-cli
SPOOR_BIN="$PWD/target/debug/spoor" npm --prefix examples/serverless-lambda test
```

子进程使用 CLI 的默认限制：每次最多处理 64 MiB，最多输出 256 KiB。

大文件不应直接放进同步请求的 `body`。可以先把文件放到 S3，再让 Lambda 根据 S3 事件读取，并另外配置 Lambda 的内存、超时和临时磁盘。
