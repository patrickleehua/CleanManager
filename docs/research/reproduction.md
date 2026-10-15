# 复现与验证工具

全部命令在项目根目录运行。它们验证研究或文档，不构成生产清理操作，也不会启动 WizTree。研究下载与临时依赖位于被忽略的 `.research/`；不能把此目录整体作为产品包发布。

## 1. 文档契约

```powershell
python -m pip install --target .research/python-deps -r requirements-docs.txt
python tools/validate_docs.py --output docs/evidence/document-validation.json
```

验证器检查 Draft 2020-12 schema、正例、字节数边界、路径编码、计划摘要、状态与空间汇总，以及错误样例拒绝和本地 Markdown 链接。依赖版本固定在 requirements-docs.txt。结果只针对文档契约；不能证明 Rust/API/权限/文件系统操作正确。

Schema 和合成样例的来源是 [generate_contracts.py](../../tools/generate_contracts.py)。需要修改结构时先修改生成器，然后执行 `python tools/generate_contracts.py` 再验证。不要只改生成文件导致下次覆盖。

## 2. disk-cleaner 分类探针

取得 [固定 commit](https://github.com/iziqing/-disk-cleaner/tree/fe16728994e4599242b3fd74b2f1ebff702063a9) 的 `analyze.py`，检查源码后使用以下命令。参数路径只是例子，换为本机研究副本。

```powershell
python tools/probe_reference_analyzer.py --source .research/disk-cleaner/analyze.py --output docs/evidence/disk-cleaner-reproduced.json
```

探针先校验经过 CRLF→LF 归一化的 SHA-256，再导入已检查模块，只调用 `analyze_csv`。源码哈希不同则拒绝，不自动执行未来版本。临时 CSV 包含两个虚构路径案例，清理时只删除探针拥有的临时 CSV 目录。不会生成/运行 PowerShell、不调用 scan.py 或 backup.py、不访问 CSV 中指定的目标路径。

本轮研究副本在系统临时研究目录，结果文件保存实际输入源码字节哈希和归一化哈希；复现不依赖相同绝对路径。原始输出里的 safe=true 是被研究程序的判断，不是本工具对那些路径的建议。

## 3. REA 制品检查

1. 从 [官方页](https://diskanalyzer.com/download) 解析当时的 portable 下载链接，不推断最新版本等于 4.33。
2. 若重现本轮，核验 ZIP SHA-256 `38b30aab08710613d8a06a7d3dfb777e43ed436cfd7d1ff8bd33d0b5b6bd2c6d`。不同哈希建立新研究对象，不覆盖旧证据。
3. 按 [调查记录](rea-investigation.md) 的 REA 顺序检查。提取制品后的哈希应与摘要对应；不要执行供应商程序来完成静态步骤。
4. 如 native provider 可用，再取得 functions/imports/xrefs 等证据。不可用时保存结构化诊断并保留 unknown；不能把失败调用计为反编译成功。
5. 导出 evidence bundle，保留工具版本/制品身份/证据 ID/unknown 关系，关闭自己打开的会话。

Windows Authenticode 查询是独立观察：对已核验哈希的 EXE 使用 `Get-AuthenticodeSignature -LiteralPath ...`，版本来自该文件 VersionInfo。结果依赖本机验证环境与观察时间；不要把旧 Valid 状态当未来任何同名文件的签名保证。

## 4. 后续平台验证

遵循 [验证计划](../verification-plan.md) 的独立语料、支持矩阵和结果格式。真实文件系统夹具仅放在明确的测试根/专用测试卷中；任何规模数据都记录生成方法、逻辑与分配口径、权限、冷暖缓存和版本。

当前工作区没有 Rust workspace，也没有可执行的产品测试套件。文档验证通过不允许将测试计划中的 NOT RUN 改为 PASS；必须附运行日志、环境和断言结果。
