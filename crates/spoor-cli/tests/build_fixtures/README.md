# 生成测试文件

`tests/fixtures/` 中的文件已经提交到 git，测试时直接使用。只有新增测试场景或修复生成脚本时，才需要重新生成。

## 依赖安装

```bash
pip install python-docx openpyxl python-pptx reportlab
```

## 重新生成全部测试文件

```bash
cd crates/spoor-cli/tests/build_fixtures
python3 make_docx.py
python3 make_docx_lists.py
python3 make_xlsx.py
python3 make_pptx.py
python3 make_csv.py
python3 make_ipynb.py
python3 make_html.py
python3 make_misc.py
```

## 新增测试文件

1. 在对应的 `make_*.py` 脚本中添加一个 `build_NN_描述性名字()` 函数。
2. 运行脚本生成新文件。
3. 在 `tests/<format>.rs` 添加对应的 `#[test]`。
4. 如果对外支持范围发生变化，更新 `docs/FORMATS_AND_LIMITS.md`。
5. 执行 `cargo test`，首次运行会生成快照。
6. 检查 `tests/snapshots/` 中新生成的 `.snap` 文件，确认正确后再提交。

## 命名规范与注意事项

- 文件命名格式：`NN_描述性名字.ext`（如 `01_basic.docx`）。
- 每个文件只测试一个重点，不要把多个功能放在一起。
- 如需测试特殊结构，例如自定义 XML 命名空间、不规范或少见的 XML，以及极端输入，建议直接编写 XML。python-docx、openpyxl 等库可能自动改写底层结构。
- 文件名、生成函数名和测试名应直接说明测试目的，不再维护重复的逐文件说明表。

支持范围见 `docs/FORMATS_AND_LIMITS.md`，设计说明见 `docs/DESIGN_NOTES.md`。测试代码和 `tests/snapshots/` 中的快照才是最终判断依据。
