# 性能与文件大小

运行：

```bash
./benchmarks/run.sh
./benchmarks/wasm-size.sh
SPOOR_BIN="$PWD/target/release/spoor" .venv/bin/python benchmarks/python.py
```

以下数据于 2026-06-11 在 Apple Silicon 上测得：

| 项目 | 结果 |
| --- | ---: |
| `spoor-core` 最小输入重复调用 | 245 ns/次 |
| Python 包重复调用 | 2.94 µs/次 |
| Python 常驻子进程通信 | 36.6 µs/次 |
| Python 每个文件启动一次 CLI | 13.0 ms/次 |
| CLI 循环 100 次 | 总计 0.20 s，约 2.0 ms/次 |
| 8 并发执行 100 次 CLI | 总计 0.03 s，约 3,333 次/秒 |
| 单个 CLI 最大常驻内存 | 约 2.56 MiB |
| `spoor-core` crate | 小于 140 KiB |
| macOS arm64 CLI | 约 4.68 MiB |
| 核心格式 WASM | 约 1.36 MiB，gzip 后约 575 KiB |
| 全格式 WASM | 约 2.13 MiB，gzip 后约 838 KiB |
| macOS arm64 `pyspoor` wheel | 约 1.32 MiB |
| macOS arm64 Node.js 扩展 | 约 2.78 MiB |

这些数字用于发现版本更新后是否变慢或变大，不代表其他机器也会得到相同结果。WASM 大小以尚未运行 `wasm-opt` 的发布文件为准；脚本还会检查 gzip 后是否超过 Cloudflare 免费版的 3 MiB 上限。
