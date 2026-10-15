# 依赖与工具链清单

本文件由 `python tools/dependency_inventory.py` 依据 `Cargo.lock` 生成；文档验证器会比对两者，手工修改会被拒绝。

- Rust 工具链：`1.99.0`（见 [rust-toolchain.toml](../rust-toolchain.toml)），workspace 声明的最低版本见根 `Cargo.toml` 的 `rust-version`。
- 文档验证依赖：[requirements-docs.txt](../requirements-docs.txt)（`jsonschema`，MIT），只安装到被忽略的 `.research/python-deps`。
- 不捆绑第三方二进制：构建只引用 workspace 内 crate 与下表 crates.io 包；`.research/` 中的 WizTree 与 Mole 研究副本被 Git 忽略，任何 `Cargo.toml` 都不得引用它们（验证器检查）。
- 本项目自身许可证尚未决定，crate 暂标 `UNLICENSED`；发布前需在 [分发与许可](distribution-and-licensing.md) 中确定并生成 notices。

| Crate | 版本 | 许可 | 来源 | 用途 |
| --- | --- | --- | --- | --- |
| `base64` | 0.22.1 | MIT OR Apache-2.0 | crates.io | Lossless raw path encoding (UTF-16LE / raw bytes) in protocol records |
| `bitflags` | 2.13.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by digest |
| `cc` | 1.8.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by libsqlite3-sys |
| `cfg-if` | 1.0.5 | MIT OR Apache-2.0 | crates.io | Transitive: required by sha2 |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 | crates.io | Transitive: required by sha2 |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by digest |
| `digest` | 0.10.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by sha2 |
| `fallible-iterator` | 0.3.0 | MIT/Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `fallible-streaming-iterator` | 0.1.9 | MIT/Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `find-msvc-tools` | 0.1.14 | MIT OR Apache-2.0 | crates.io | Transitive: required by cc |
| `foldhash` | 0.1.5 | Zlib | crates.io | Transitive: required by hashbrown |
| `generic-array` | 0.14.7 | MIT | crates.io | Transitive: required by block-buffer, crypto-common |
| `hashbrown` | 0.15.5 | MIT OR Apache-2.0 | crates.io | Transitive: required by hashlink |
| `hashlink` | 0.10.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde_json |
| `libc` | 0.2.190 | MIT OR Apache-2.0 | crates.io | macOS system calls (process liveness via kill/proc_pidinfo, later POSIX scan backend) |
| `libsqlite3-sys` | 0.35.0 | MIT | crates.io | Transitive: required by rusqlite |
| `memchr` | 2.8.3 | Unlicense OR MIT | crates.io | Transitive: required by serde_json |
| `pkg-config` | 0.3.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by libsqlite3-sys |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 | crates.io | Transitive: required by quote, serde_derive, syn, thiserror-impl |
| `quote` | 1.0.47 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde_derive, syn, thiserror-impl |
| `rusqlite` | 0.37.0 | MIT | crates.io | SQLite access for the rebuildable scan index and the separate audit store (bundled SQLite, no system library) |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | crates.io | Typed (de)serialization of protocol records, manifests and rules |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde, serde_json |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde, serde_core |
| `serde_json` | 1.0.154 | MIT OR Apache-2.0 | crates.io | Strict JSON input/output for the machine protocol (CLI-03/04) |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | crates.io | SHA-256 for plan digests (CM-C14N-v1) and fixture/sentinel hashes |
| `shlex` | 2.0.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by cc |
| `smallvec` | 1.16.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `syn` | 3.0.9 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde_derive, thiserror-impl |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | crates.io | Typed error enums in cm-core and cm-store |
| `thiserror-impl` | 2.0.21 | MIT OR Apache-2.0 | crates.io | Transitive: required by thiserror |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by crypto-common, generic-array |
| `unicode-ident` | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | crates.io | Transitive: required by proc-macro2, syn |
| `vcpkg` | 0.2.15 | MIT/Apache-2.0 | crates.io | Transitive: required by libsqlite3-sys |
| `version_check` | 0.9.5 | MIT/Apache-2.0 | crates.io | Transitive: required by generic-array |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-sys |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | crates.io | Windows API bindings for process liveness and, later, native scan/execution backends |
| `zmij` | 1.0.23 | MIT | crates.io | Transitive: required by serde_json |
