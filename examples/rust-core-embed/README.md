# 在 Rust 中使用 spoor-core

这个示例不使用 GUI 或 Web 框架，只演示三个步骤：创建 `ParseRequest`、调用 `parse`、读取 `ParseResult`。

```bash
cargo run -p spoor-rust-core-embed-example
```

实际应用应通过 `content.kind` 判断结果是文档还是表格，并检查 `warnings`。默认每次最多处理 64 MiB。

完整桌面应用见 [Tauri 示例](../tauri-desktop/)。
