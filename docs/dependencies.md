# 依赖与工具链清单

本文件由 `python tools/dependency_inventory.py` 依据 `Cargo.lock` 生成；文档验证器会比对两者，手工修改会被拒绝。

- Rust 工具链：`1.99.0`（见 [rust-toolchain.toml](../rust-toolchain.toml)），workspace 声明的最低版本见根 `Cargo.toml` 的 `rust-version`。
- 文档验证依赖：[requirements-docs.txt](../requirements-docs.txt)（`jsonschema`，MIT），只安装到被忽略的 `.research/python-deps`。
- 不捆绑第三方二进制：构建只引用 workspace 内 crate 与下表 crates.io 包；`.research/` 中的 WizTree 与 Mole 研究副本被 Git 忽略，任何 `Cargo.toml` 都不得引用它们（验证器检查）。
- 本项目以 MIT 许可发布（见仓库根目录 `LICENSE`）；第三方许可见 [分发与许可](distribution-and-licensing.md) 与 [第三方 notices](third-party-notices.md)。

| Crate | 版本 | 许可 | 来源 | 用途 |
| --- | --- | --- | --- | --- |
| `base64` | 0.22.1 | MIT OR Apache-2.0 | crates.io | Lossless raw path encoding (UTF-16LE / raw bytes) in protocol records |
| `base64` | 0.23.1 | MIT OR Apache-2.0 | crates.io | Lossless raw path encoding (UTF-16LE / raw bytes) in protocol records |
| `base64ct` | 1.8.3 | Apache-2.0 OR MIT | crates.io | Transitive: required by pem-rfc7468 |
| `bitflags` | 2.13.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by openssl, rusqlite, rustix, security-framework |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by digest |
| `bytes` | 1.12.1 | MIT | crates.io | Transitive: required by http |
| `cc` | 1.8.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by libsqlite3-sys, openssl-sys |
| `cfg-if` | 1.0.5 | MIT OR Apache-2.0 | crates.io | Transitive: required by curve25519-dalek, getrandom, openssl, sha2 |
| `core-foundation` | 0.10.1 | MIT OR Apache-2.0 | crates.io | macOS CoreFoundation types (CFString/CFDictionary/CFURL, property lists) for app bundles, Service Management and power assertions; macOS-only |
| `core-foundation-sys` | 0.8.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by core-foundation, security-framework, security-framework-sys |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 | crates.io | Transitive: required by curve25519-dalek, sha2 |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by digest |
| `curve25519-dalek` | 4.1.3 | BSD-3-Clause | crates.io | Transitive: required by ed25519-dalek |
| `curve25519-dalek-derive` | 0.1.1 | MIT/Apache-2.0 | crates.io | Transitive: required by curve25519-dalek |
| `der` | 0.8.2 | Apache-2.0 OR MIT | crates.io | Transitive: required by ureq |
| `digest` | 0.10.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by curve25519-dalek, sha2 |
| `ed25519` | 2.2.3 | Apache-2.0 OR MIT | crates.io | Transitive: required by ed25519-dalek |
| `ed25519-dalek` | 2.2.0 | BSD-3-Clause | crates.io | Ed25519 signature verification of the CLI self-update manifest with release keys embedded at build time (LIF-01); vetted dalek-cryptography implementation, verification only (no default features, no key generation) |
| `errno` | 0.3.14 | MIT OR Apache-2.0 | crates.io | Transitive: required by rustix |
| `fallible-iterator` | 0.3.0 | MIT/Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `fallible-streaming-iterator` | 0.1.9 | MIT/Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `fastrand` | 2.5.0 | Apache-2.0 OR MIT | crates.io | Transitive: required by tempfile |
| `fiat-crypto` | 0.2.9 | MIT OR Apache-2.0 OR BSD-1-Clause | crates.io | Transitive: required by curve25519-dalek |
| `find-msvc-tools` | 0.1.14 | MIT OR Apache-2.0 | crates.io | Transitive: required by cc |
| `foldhash` | 0.1.5 | Zlib | crates.io | Transitive: required by hashbrown |
| `foreign-types` | 0.3.2 | MIT/Apache-2.0 | crates.io | Transitive: required by openssl |
| `foreign-types-shared` | 0.1.1 | MIT/Apache-2.0 | crates.io | Transitive: required by foreign-types |
| `generic-array` | 0.14.7 | MIT | crates.io | Transitive: required by block-buffer, crypto-common |
| `getrandom` | 0.4.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by tempfile |
| `hashbrown` | 0.15.5 | MIT OR Apache-2.0 | crates.io | Transitive: required by hashlink |
| `hashlink` | 0.10.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `http` | 1.5.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by ureq-proto |
| `httparse` | 1.10.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by ureq-proto |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 | crates.io | Transitive: required by http, serde_json |
| `libc` | 0.2.190 | MIT OR Apache-2.0 | crates.io | macOS system calls (process liveness via kill/proc_pidinfo, POSIX and getattrlistbulk scan backends) |
| `libsqlite3-sys` | 0.35.0 | MIT | crates.io | Transitive: required by rusqlite |
| `linux-raw-sys` | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | crates.io | Transitive: required by rustix |
| `log` | 0.4.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by native-tls, ureq, ureq-proto |
| `memchr` | 2.8.3 | Unlicense OR MIT | crates.io | Transitive: required by serde_json |
| `native-tls` | 0.2.18 | MIT OR Apache-2.0 | crates.io | Transitive: required by ureq |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by tempfile |
| `openssl` | 0.10.81 | Apache-2.0 | crates.io | Transitive: required by native-tls |
| `openssl-macros` | 0.1.1 | MIT/Apache-2.0 | crates.io | Transitive: required by openssl |
| `openssl-probe` | 0.2.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by native-tls |
| `openssl-sys` | 0.9.117 | MIT | crates.io | Transitive: required by native-tls, openssl |
| `pem-rfc7468` | 1.0.0 | Apache-2.0 OR MIT | crates.io | Transitive: required by der |
| `percent-encoding` | 2.3.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by ureq |
| `pkg-config` | 0.3.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by libsqlite3-sys, openssl-sys |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 | crates.io | Transitive: required by curve25519-dalek-derive, openssl-macros, quote, serde_derive, syn, thiserror-impl, windows-implement, windows-interface |
| `quote` | 1.0.47 | MIT OR Apache-2.0 | crates.io | Transitive: required by curve25519-dalek-derive, openssl-macros, serde_derive, syn, thiserror-impl, windows-implement, windows-interface |
| `r-efi` | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | crates.io | Transitive: required by getrandom |
| `rusqlite` | 0.37.0 | MIT | crates.io | SQLite access for the rebuildable scan index and the separate audit store (bundled SQLite, no system library) |
| `rustc_version` | 0.4.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by curve25519-dalek |
| `rustix` | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | crates.io | Transitive: required by tempfile |
| `rustls-pki-types` | 1.15.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by ureq |
| `schannel` | 0.1.29 | MIT | crates.io | Transitive: required by native-tls |
| `security-framework` | 3.7.0 | MIT OR Apache-2.0 | crates.io | macOS code-signature checks (SecStaticCode signer / Team ID of bundles, startup targets and update packages); macOS-only, already in the lock through native-tls |
| `security-framework-sys` | 2.17.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by native-tls, security-framework |
| `semver` | 1.0.28 | MIT OR Apache-2.0 | crates.io | Transitive: required by rustc_version |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | crates.io | Typed (de)serialization of protocol records, manifests and rules |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde, serde_json |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde, serde_core |
| `serde_json` | 1.0.154 | MIT OR Apache-2.0 | crates.io | Strict JSON input/output for the machine protocol (CLI-03/04) |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | crates.io | SHA-256 for plan digests (CM-C14N-v1) and fixture/sentinel hashes |
| `shlex` | 2.0.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by cc |
| `signature` | 2.2.0 | Apache-2.0 OR MIT | crates.io | Transitive: required by ed25519 |
| `smallvec` | 1.16.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `subtle` | 2.6.1 | BSD-3-Clause | crates.io | Transitive: required by curve25519-dalek, ed25519-dalek |
| `syn` | 2.0.119 | MIT OR Apache-2.0 | crates.io | Transitive: required by curve25519-dalek-derive, openssl-macros, windows-implement, windows-interface |
| `syn` | 3.0.9 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde_derive, thiserror-impl |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by native-tls |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | crates.io | Typed error enums in cm-core and cm-store |
| `thiserror-impl` | 2.0.21 | MIT OR Apache-2.0 | crates.io | Transitive: required by thiserror |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by crypto-common, generic-array |
| `unicode-ident` | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | crates.io | Transitive: required by proc-macro2, syn |
| `ureq` | 3.4.2 | MIT OR Apache-2.0 | crates.io | Blocking HTTP client for the optional AI diagnosis API adapters and the person-initiated CLI self-update download (native-tls: OS TLS stack and trust store; diagnosis: no redirects, no retries; self-update: https only, at most 5 redirects, size-bounded) |
| `ureq-proto` | 0.6.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by ureq |
| `utf8-zero` | 0.8.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by ureq |
| `vcpkg` | 0.2.15 | MIT/Apache-2.0 | crates.io | Transitive: required by libsqlite3-sys, openssl-sys |
| `version_check` | 0.9.5 | MIT/Apache-2.0 | crates.io | Transitive: required by generic-array |
| `windows` | 0.62.2 | MIT OR Apache-2.0 | crates.io | COM bindings for the Shell IFileOperation recycle-bin action and its progress sink |
| `windows-collections` | 0.3.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows |
| `windows-core` | 0.62.2 | MIT OR Apache-2.0 | crates.io | Required by the windows crate COM implement macro (referenced by name) |
| `windows-future` | 0.3.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows |
| `windows-implement` | 0.60.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-core |
| `windows-interface` | 0.59.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-core |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-core, windows-future, windows-numerics, windows-result, windows-strings, windows-sys, windows-threading |
| `windows-numerics` | 0.3.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows |
| `windows-result` | 0.4.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-core |
| `windows-strings` | 0.5.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-core |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | crates.io | Windows API bindings for process liveness, scan backends and verified file actions |
| `windows-threading` | 0.2.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-future |
| `zeroize` | 1.9.1 | Apache-2.0 OR MIT | crates.io | Transitive: required by der, rustls-pki-types |
| `zmij` | 1.0.23 | MIT | crates.io | Transitive: required by serde_json |
