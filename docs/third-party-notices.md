# 第三方声明（Third-party notices）

本文件由 `python -I tools/generate_notices.py` 依据锁文件生成（`tools/dependency_inventory.py` 也会重新生成），手工修改会被文档验证器拒绝。它列出**随发行包分发**的第三方组件：CLI 包是 `cm-cli` 的普通依赖闭包（不含仅测试/仅构建的 crate）；桌面包是桌面宿主的普通依赖闭包加运行时 npm 包（`dev` 包不进入安装包）。完整依赖清单（含开发与构建依赖）见 [依赖与工具链清单](dependencies.md) 与 [桌面端依赖清单](dependencies-desktop.md)。

发行 zip 中的 `THIRD-PARTY-NOTICES.txt` 由 `tools/package_release.py` 以 `--full` 生成，逐个附上各 crate 自带的许可/声明文件原文。本项目自身许可证尚未决定（见 [分发与许可](distribution-and-licensing.md)），本文件不构成对项目自身许可的声明。

锁文件摘要（SHA-256）：

- `Cargo.lock`：`0e577a8aa0476a481002b879ce1cd2f2812981026cb31d81658e439d1f9246b2`
- `apps/desktop/src-tauri/Cargo.lock`：`d5f226f5f1558f06686373b340ddcef723739ad748f974432b2210a7af256f62`
- `apps/desktop/package-lock.json`：`6255597f6fc0cea30b63690a3fd185394b8cc95747cf02450e460a3a093c8c7c`

## 许可汇总

| 许可表达式 | 组件数 |
| --- | --- |
| MIT OR Apache-2.0 | 253 |
| MIT | 103 |
| Apache-2.0 OR MIT | 38 |
| MIT/Apache-2.0 | 21 |
| Unicode-3.0 | 18 |
| Zlib OR Apache-2.0 OR MIT | 16 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 7 |
| BSD-3-Clause | 5 |
| MPL-2.0 | 5 |
| Unlicense OR MIT | 5 |
| Apache-2.0 | 4 |
| Zlib | 4 |
| Apache-2.0/MIT | 3 |
| MIT OR Apache-2.0 OR LGPL-2.1-or-later | 3 |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 2 |
| BSD-3-Clause OR MIT OR Apache-2.0 | 2 |
| MIT OR Zlib OR Apache-2.0 | 2 |
| Unlicense/MIT | 2 |
| 0BSD OR MIT OR Apache-2.0 | 1 |
| Apache-2.0 / MIT | 1 |
| Apache-2.0 AND MIT | 1 |
| BSD-3-Clause AND MIT | 1 |
| BSD-3-Clause/MIT | 1 |
| CC0-1.0 OR MIT-0 OR Apache-2.0 | 1 |
| ISC | 1 |
| MIT OR Apache-2.0 OR BSD-1-Clause | 1 |
| MIT OR Apache-2.0 OR Zlib | 1 |

## CLI 发行包（86 个 crate）

| Crate | 版本 | 许可 | 来源 |
| --- | --- | --- | --- |
| `base64` | 0.22.1 | MIT OR Apache-2.0 | https://github.com/marshallpierce/rust-base64 |
| `base64` | 0.23.1 | MIT OR Apache-2.0 | https://github.com/marshallpierce/rust-base64 |
| `base64ct` | 1.8.3 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats |
| `bitflags` | 2.13.2 | MIT OR Apache-2.0 | https://github.com/bitflags/bitflags |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| `bytes` | 1.12.1 | MIT | https://github.com/tokio-rs/bytes |
| `cfg-if` | 1.0.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/cfg-if |
| `core-foundation` | 0.10.1 | MIT OR Apache-2.0 | https://github.com/servo/core-foundation-rs |
| `core-foundation-sys` | 0.8.7 | MIT OR Apache-2.0 | https://github.com/servo/core-foundation-rs |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| `curve25519-dalek` | 4.1.3 | BSD-3-Clause | https://github.com/dalek-cryptography/curve25519-dalek/tree/main/curve25519-dalek |
| `curve25519-dalek-derive` | 0.1.1 | MIT/Apache-2.0 | https://github.com/dalek-cryptography/curve25519-dalek |
| `der` | 0.8.2 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats |
| `digest` | 0.10.7 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| `ed25519` | 2.2.3 | Apache-2.0 OR MIT | https://github.com/RustCrypto/signatures/tree/master/ed25519 |
| `ed25519-dalek` | 2.2.0 | BSD-3-Clause | https://github.com/dalek-cryptography/curve25519-dalek/tree/main/ed25519-dalek |
| `errno` | 0.3.14 | MIT OR Apache-2.0 | https://github.com/lambda-fairy/rust-errno |
| `fallible-iterator` | 0.3.0 | MIT/Apache-2.0 | https://github.com/sfackler/rust-fallible-iterator |
| `fallible-streaming-iterator` | 0.1.9 | MIT/Apache-2.0 | https://github.com/sfackler/fallible-streaming-iterator |
| `fastrand` | 2.5.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/fastrand |
| `fiat-crypto` | 0.2.9 | MIT OR Apache-2.0 OR BSD-1-Clause | https://github.com/mit-plv/fiat-crypto |
| `foldhash` | 0.1.5 | Zlib | https://github.com/orlp/foldhash |
| `foreign-types` | 0.3.2 | MIT/Apache-2.0 | https://github.com/sfackler/foreign-types |
| `foreign-types-shared` | 0.1.1 | MIT/Apache-2.0 | https://github.com/sfackler/foreign-types |
| `generic-array` | 0.14.7 | MIT | https://github.com/fizyk20/generic-array.git |
| `getrandom` | 0.4.3 | MIT OR Apache-2.0 | https://github.com/rust-random/getrandom |
| `hashbrown` | 0.15.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| `hashlink` | 0.10.0 | MIT OR Apache-2.0 | https://github.com/kyren/hashlink |
| `http` | 1.5.0 | MIT OR Apache-2.0 | https://github.com/hyperium/http |
| `httparse` | 1.10.1 | MIT OR Apache-2.0 | https://github.com/seanmonstar/httparse |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 | https://github.com/dtolnay/itoa |
| `libc` | 0.2.190 | MIT OR Apache-2.0 | https://github.com/rust-lang/libc |
| `libsqlite3-sys` | 0.35.0 | MIT | https://github.com/rusqlite/rusqlite |
| `linux-raw-sys` | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/sunfishcode/linux-raw-sys |
| `log` | 0.4.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/log |
| `memchr` | 2.8.3 | Unlicense OR MIT | https://github.com/BurntSushi/memchr |
| `native-tls` | 0.2.18 | MIT OR Apache-2.0 | https://github.com/rust-native-tls/rust-native-tls |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 | https://github.com/matklad/once_cell |
| `openssl` | 0.10.81 | Apache-2.0 | https://github.com/rust-openssl/rust-openssl |
| `openssl-macros` | 0.1.1 | MIT/Apache-2.0 | crates.io |
| `openssl-probe` | 0.2.1 | MIT OR Apache-2.0 | https://github.com/rustls/openssl-probe |
| `openssl-sys` | 0.9.117 | MIT | https://github.com/rust-openssl/rust-openssl |
| `pem-rfc7468` | 1.0.0 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats |
| `percent-encoding` | 2.3.2 | MIT OR Apache-2.0 | https://github.com/servo/rust-url/ |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 | https://github.com/dtolnay/proc-macro2 |
| `quote` | 1.0.47 | MIT OR Apache-2.0 | https://github.com/dtolnay/quote |
| `r-efi` | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | https://github.com/r-efi/r-efi |
| `rusqlite` | 0.37.0 | MIT | https://github.com/rusqlite/rusqlite |
| `rustix` | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/bytecodealliance/rustix |
| `rustls-pki-types` | 1.15.1 | MIT OR Apache-2.0 | https://github.com/rustls/pki-types |
| `schannel` | 0.1.29 | MIT | https://github.com/steffengy/schannel-rs |
| `security-framework` | 3.7.0 | MIT OR Apache-2.0 | https://github.com/kornelski/rust-security-framework |
| `security-framework-sys` | 2.17.0 | MIT OR Apache-2.0 | https://github.com/kornelski/rust-security-framework |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| `serde_json` | 1.0.154 | MIT OR Apache-2.0 | https://github.com/serde-rs/json |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hashes |
| `signature` | 2.2.0 | Apache-2.0 OR MIT | https://github.com/RustCrypto/traits/tree/master/signature |
| `smallvec` | 1.16.3 | MIT OR Apache-2.0 | https://github.com/servo/rust-smallvec |
| `subtle` | 2.6.1 | BSD-3-Clause | https://github.com/dalek-cryptography/subtle |
| `syn` | 2.0.119 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| `syn` | 3.0.9 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | https://github.com/Stebalien/tempfile |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| `thiserror-impl` | 2.0.21 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 | https://github.com/paholg/typenum |
| `unicode-ident` | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | https://github.com/dtolnay/unicode-ident |
| `ureq` | 3.4.2 | MIT OR Apache-2.0 | https://github.com/algesten/ureq |
| `ureq-proto` | 0.6.4 | MIT OR Apache-2.0 | https://github.com/algesten/ureq-proto |
| `utf8-zero` | 0.8.1 | MIT OR Apache-2.0 | https://github.com/algesten/utf8-zero |
| `windows` | 0.62.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-collections` | 0.3.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-core` | 0.62.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-future` | 0.3.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-implement` | 0.60.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-interface` | 0.59.3 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-numerics` | 0.3.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-result` | 0.4.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-strings` | 0.5.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-threading` | 0.2.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `zeroize` | 1.9.1 | Apache-2.0 OR MIT | https://github.com/RustCrypto/utils |
| `zmij` | 1.0.23 | MIT | https://github.com/dtolnay/zmij |

## 桌面发行包：Rust crate（415）

| Crate | 版本 | 许可 | 来源 |
| --- | --- | --- | --- |
| `adler2` | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | https://github.com/oyvindln/adler2 |
| `aho-corasick` | 1.1.5 | Unlicense OR MIT | https://github.com/BurntSushi/aho-corasick |
| `alloc-no-stdlib` | 3.0.0 | BSD-3-Clause | https://github.com/dropbox/rust-alloc-no-stdlib |
| `alloc-stdlib` | 0.3.0 | BSD-3-Clause | https://github.com/dropbox/rust-alloc-no-stdlib |
| `android_system_properties` | 0.1.6 | MIT OR Apache-2.0 | https://github.com/nical/android_system_properties |
| `anyhow` | 1.0.104 | MIT OR Apache-2.0 | https://github.com/dtolnay/anyhow |
| `atk` | 0.18.2 | MIT | https://github.com/gtk-rs/gtk3-rs |
| `atk-sys` | 0.18.2 | MIT | https://github.com/gtk-rs/gtk3-rs |
| `atomic-waker` | 1.1.2 | Apache-2.0 OR MIT | https://github.com/smol-rs/atomic-waker |
| `base64` | 0.21.7 | MIT OR Apache-2.0 | https://github.com/marshallpierce/rust-base64 |
| `base64` | 0.22.1 | MIT OR Apache-2.0 | https://github.com/marshallpierce/rust-base64 |
| `base64` | 0.23.1 | MIT OR Apache-2.0 | https://github.com/marshallpierce/rust-base64 |
| `base64ct` | 1.8.3 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats |
| `bit-set` | 0.8.0 | Apache-2.0 OR MIT | https://github.com/contain-rs/bit-set |
| `bit-vec` | 0.8.0 | Apache-2.0 OR MIT | https://github.com/contain-rs/bit-vec |
| `bitflags` | 1.3.2 | MIT/Apache-2.0 | https://github.com/bitflags/bitflags |
| `bitflags` | 2.13.2 | MIT OR Apache-2.0 | https://github.com/bitflags/bitflags |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| `block2` | 0.6.2 | MIT | https://github.com/madsmtm/objc2 |
| `brotli` | 9.0.0 | BSD-3-Clause AND MIT | https://github.com/dropbox/rust-brotli |
| `brotli-decompressor` | 6.0.1 | BSD-3-Clause/MIT | https://github.com/dropbox/rust-brotli-decompressor |
| `bumpalo` | 3.20.3 | MIT OR Apache-2.0 | https://github.com/fitzgen/bumpalo |
| `bytemuck` | 1.25.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/Lokathor/bytemuck |
| `byteorder` | 1.5.0 | Unlicense OR MIT | https://github.com/BurntSushi/byteorder |
| `bytes` | 1.12.1 | MIT | https://github.com/tokio-rs/bytes |
| `cairo-rs` | 0.18.5 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `cairo-sys-rs` | 0.18.2 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `camino` | 1.2.6 | MIT OR Apache-2.0 | https://github.com/camino-rs/camino |
| `cargo-platform` | 0.1.9 | MIT OR Apache-2.0 | https://github.com/rust-lang/cargo |
| `cargo_metadata` | 0.19.2 | MIT | https://github.com/oli-obk/cargo_metadata |
| `cesu8` | 1.1.0 | Apache-2.0/MIT | https://github.com/emk/cesu8-rs |
| `cfb` | 0.14.0 | MIT | https://github.com/mdsteele/rust-cfb |
| `cfg-if` | 1.0.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/cfg-if |
| `chrono` | 0.4.45 | MIT OR Apache-2.0 | https://github.com/chronotope/chrono |
| `combine` | 4.6.8 | MIT | https://github.com/Marwes/combine |
| `cookie` | 0.18.2 | MIT OR Apache-2.0 | https://github.com/SergioBenitez/cookie-rs |
| `core-foundation` | 0.10.1 | MIT OR Apache-2.0 | https://github.com/servo/core-foundation-rs |
| `core-foundation-sys` | 0.8.7 | MIT OR Apache-2.0 | https://github.com/servo/core-foundation-rs |
| `core-graphics` | 0.25.0 | MIT OR Apache-2.0 | https://github.com/servo/core-foundation-rs |
| `core-graphics-types` | 0.2.0 | MIT OR Apache-2.0 | https://github.com/servo/core-foundation-rs |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 | https://github.com/RustCrypto/utils |
| `crc32fast` | 1.5.2 | MIT OR Apache-2.0 | https://github.com/srijs/rust-crc32fast |
| `crossbeam-channel` | 0.5.17 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| `crossbeam-utils` | 0.8.23 | MIT OR Apache-2.0 | https://github.com/crossbeam-rs/crossbeam |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| `cssparser` | 0.37.0 | MPL-2.0 | https://github.com/servo/rust-cssparser |
| `cssparser-macros` | 0.7.1 | MPL-2.0 | https://github.com/servo/rust-cssparser |
| `ctor` | 1.0.13 | Apache-2.0 OR MIT | https://github.com/mmastrac/linktime |
| `darling` | 0.21.3 | MIT | https://github.com/TedDriggs/darling |
| `darling_core` | 0.21.3 | MIT | https://github.com/TedDriggs/darling |
| `darling_macro` | 0.21.3 | MIT | https://github.com/TedDriggs/darling |
| `dbus` | 0.9.12 | Apache-2.0/MIT | https://github.com/diwic/dbus-rs |
| `der` | 0.8.2 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats |
| `deranged` | 0.5.9 | MIT OR Apache-2.0 | https://github.com/jhpratt/deranged |
| `derive_more` | 2.1.1 | MIT | https://github.com/JelteF/derive_more |
| `derive_more-impl` | 2.1.1 | MIT | https://github.com/JelteF/derive_more |
| `digest` | 0.10.7 | MIT OR Apache-2.0 | https://github.com/RustCrypto/traits |
| `dirs` | 7.0.0 | MIT OR Apache-2.0 | https://codeberg.org/dirs/dirs-rs |
| `dirs-sys` | 0.5.0 | MIT OR Apache-2.0 | https://github.com/dirs-dev/dirs-sys-rs |
| `dispatch2` | 0.3.1 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `displaydoc` | 0.2.7 | MIT OR Apache-2.0 | https://github.com/yaahc/displaydoc |
| `dlopen2` | 0.8.2 | MIT | https://github.com/OpenByteDev/dlopen2 |
| `dlopen2_derive` | 0.4.3 | MIT | https://github.com/OpenByteDev/dlopen2 |
| `dom_query` | 0.28.0 | MIT | https://github.com/niklak/dom_query |
| `dpi` | 0.1.2 | Apache-2.0 AND MIT | https://github.com/rust-windowing/winit |
| `dtoa` | 1.0.11 | MIT OR Apache-2.0 | https://github.com/dtolnay/dtoa |
| `dtoa-short` | 0.3.5 | MPL-2.0 | https://github.com/upsuper/dtoa-short |
| `dunce` | 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 | https://gitlab.com/kornelski/dunce |
| `dyn-clone` | 1.0.20 | MIT OR Apache-2.0 | https://github.com/dtolnay/dyn-clone |
| `embed_plist` | 1.2.2 | MIT OR Apache-2.0 | https://github.com/nvzqz/embed-plist-rs |
| `equivalent` | 1.0.3 | Apache-2.0 OR MIT | https://github.com/indexmap-rs/equivalent |
| `erased-serde` | 0.4.10 | MIT OR Apache-2.0 | https://github.com/dtolnay/erased-serde |
| `errno` | 0.3.14 | MIT OR Apache-2.0 | https://github.com/lambda-fairy/rust-errno |
| `fallible-iterator` | 0.3.0 | MIT/Apache-2.0 | https://github.com/sfackler/rust-fallible-iterator |
| `fallible-streaming-iterator` | 0.1.9 | MIT/Apache-2.0 | https://github.com/sfackler/fallible-streaming-iterator |
| `fastrand` | 2.5.0 | Apache-2.0 OR MIT | https://github.com/smol-rs/fastrand |
| `fdeflate` | 0.3.7 | MIT OR Apache-2.0 | https://github.com/image-rs/fdeflate |
| `field-offset` | 0.3.6 | MIT OR Apache-2.0 | https://github.com/Diggsey/rust-field-offset |
| `flate2` | 1.1.10 | MIT OR Apache-2.0 | https://github.com/rust-lang/flate2-rs |
| `fnv` | 1.0.7 | Apache-2.0 / MIT | https://github.com/servo/rust-fnv |
| `foldhash` | 0.1.5 | Zlib | https://github.com/orlp/foldhash |
| `foldhash` | 0.2.0 | Zlib | https://github.com/orlp/foldhash |
| `foreign-types` | 0.3.2 | MIT/Apache-2.0 | https://github.com/sfackler/foreign-types |
| `foreign-types` | 0.5.0 | MIT/Apache-2.0 | https://github.com/sfackler/foreign-types |
| `foreign-types-macros` | 0.2.4 | MIT/Apache-2.0 | https://github.com/sfackler/foreign-types |
| `foreign-types-shared` | 0.1.1 | MIT/Apache-2.0 | https://github.com/sfackler/foreign-types |
| `foreign-types-shared` | 0.3.1 | MIT/Apache-2.0 | https://github.com/sfackler/foreign-types |
| `form_urlencoded` | 1.2.2 | MIT OR Apache-2.0 | https://github.com/servo/rust-url |
| `futures-channel` | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| `futures-core` | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| `futures-executor` | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| `futures-io` | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| `futures-macro` | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| `futures-sink` | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| `futures-task` | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| `futures-util` | 0.3.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/futures-rs |
| `gdk` | 0.18.2 | MIT | https://github.com/gtk-rs/gtk3-rs |
| `gdk-pixbuf` | 0.18.5 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `gdk-pixbuf-sys` | 0.18.0 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `gdk-sys` | 0.18.2 | MIT | https://github.com/gtk-rs/gtk3-rs |
| `gdkwayland-sys` | 0.18.2 | MIT | https://github.com/gtk-rs/gtk3-rs |
| `gdkx11` | 0.18.2 | MIT | https://github.com/gtk-rs/gtk3-rs |
| `gdkx11-sys` | 0.18.2 | MIT | https://github.com/gtk-rs/gtk3-rs |
| `generic-array` | 0.14.7 | MIT | https://github.com/fizyk20/generic-array.git |
| `getrandom` | 0.3.4 | MIT OR Apache-2.0 | https://github.com/rust-random/getrandom |
| `getrandom` | 0.4.3 | MIT OR Apache-2.0 | https://github.com/rust-random/getrandom |
| `gio` | 0.18.4 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `gio-sys` | 0.18.1 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `glib` | 0.18.5 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `glib-macros` | 0.18.5 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `glib-sys` | 0.18.1 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `glob` | 0.3.4 | MIT OR Apache-2.0 | https://github.com/rust-lang/glob |
| `gobject-sys` | 0.18.0 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `gtk` | 0.18.2 | MIT | https://github.com/gtk-rs/gtk3-rs |
| `gtk-sys` | 0.18.2 | MIT | https://github.com/gtk-rs/gtk3-rs |
| `gtk3-macros` | 0.18.2 | MIT | https://github.com/gtk-rs/gtk3-rs |
| `hashbrown` | 0.12.3 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| `hashbrown` | 0.15.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| `hashbrown` | 0.17.1 | MIT OR Apache-2.0 | https://github.com/rust-lang/hashbrown |
| `hashlink` | 0.10.0 | MIT OR Apache-2.0 | https://github.com/kyren/hashlink |
| `heck` | 0.4.1 | MIT OR Apache-2.0 | https://github.com/withoutboats/heck |
| `heck` | 0.5.0 | MIT OR Apache-2.0 | https://github.com/withoutboats/heck |
| `hex` | 0.4.3 | MIT OR Apache-2.0 | https://github.com/KokaKiwi/rust-hex |
| `html5ever` | 0.39.0 | MIT OR Apache-2.0 | https://github.com/servo/html5ever |
| `http` | 1.5.0 | MIT OR Apache-2.0 | https://github.com/hyperium/http |
| `http-body` | 1.1.0 | MIT | https://github.com/hyperium/http-body |
| `http-body-util` | 0.1.5 | MIT | https://github.com/hyperium/http-body |
| `httparse` | 1.10.1 | MIT OR Apache-2.0 | https://github.com/seanmonstar/httparse |
| `hyper` | 1.12.0 | MIT | https://github.com/hyperium/hyper |
| `hyper-util` | 0.1.21 | MIT | https://github.com/hyperium/hyper-util |
| `iana-time-zone` | 0.1.65 | MIT OR Apache-2.0 | https://github.com/strawlab/iana-time-zone |
| `iana-time-zone-haiku` | 0.1.2 | MIT OR Apache-2.0 | https://github.com/strawlab/iana-time-zone |
| `ico` | 0.5.0 | MIT | https://github.com/mdsteele/rust-ico |
| `icu_collections` | 2.1.1 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `icu_locale_core` | 2.1.1 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `icu_normalizer` | 2.1.1 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `icu_normalizer_data` | 2.1.1 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `icu_properties` | 2.1.2 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `icu_properties_data` | 2.1.2 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `icu_provider` | 2.1.1 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `ident_case` | 1.0.1 | MIT/Apache-2.0 | https://github.com/TedDriggs/ident_case |
| `idna` | 1.1.0 | MIT OR Apache-2.0 | https://github.com/servo/rust-url/ |
| `idna_adapter` | 1.2.1 | Apache-2.0 OR MIT | https://github.com/hsivonen/idna_adapter |
| `indexmap` | 1.9.3 | Apache-2.0 OR MIT | https://github.com/bluss/indexmap |
| `indexmap` | 2.14.2 | Apache-2.0 OR MIT | https://github.com/indexmap-rs/indexmap |
| `infer` | 0.22.0 | MIT | https://github.com/bojand/infer |
| `ipnet` | 2.12.2 | MIT OR Apache-2.0 | https://github.com/krisprice/ipnet |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 | https://github.com/dtolnay/itoa |
| `javascriptcore-rs` | 1.1.2 | MIT | https://github.com/tauri-apps/javascriptcore-rs |
| `javascriptcore-rs-sys` | 1.1.1 | MIT | https://github.com/tauri-apps/javascriptcore-rs |
| `jni` | 0.21.1 | MIT/Apache-2.0 | https://github.com/jni-rs/jni-rs |
| `jni-sys` | 0.3.1 | MIT OR Apache-2.0 | https://github.com/jni-rs/jni-sys |
| `jni-sys` | 0.4.1 | MIT OR Apache-2.0 | https://github.com/jni-rs/jni-sys |
| `jni-sys-macros` | 0.4.1 | MIT OR Apache-2.0 | https://github.com/jni-rs/jni-sys |
| `js-sys` | 0.3.106 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/js-sys |
| `json-patch` | 4.2.0 | MIT/Apache-2.0 | https://github.com/idubrov/json-patch |
| `jsonptr` | 0.7.1 | MIT OR Apache-2.0 | https://github.com/chanced/jsonptr |
| `keyboard-types` | 0.8.3 | MIT OR Apache-2.0 | https://github.com/rust-windowing/keyboard-types |
| `libappindicator` | 0.9.0 | Apache-2.0 OR MIT | crates.io |
| `libappindicator-sys` | 0.9.0 | Apache-2.0 OR MIT | crates.io |
| `libc` | 0.2.190 | MIT OR Apache-2.0 | https://github.com/rust-lang/libc |
| `libdbus-sys` | 0.2.7 | Apache-2.0/MIT | https://github.com/diwic/dbus-rs |
| `libloading` | 0.7.4 | ISC | https://github.com/nagisa/rust_libloading/ |
| `libredox` | 0.1.25 | MIT | https://gitlab.redox-os.org/redox-os/libredox.git |
| `libsqlite3-sys` | 0.35.0 | MIT | https://github.com/rusqlite/rusqlite |
| `linux-raw-sys` | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/sunfishcode/linux-raw-sys |
| `litemap` | 0.8.3 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `lock_api` | 0.4.14 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |
| `log` | 0.4.34 | MIT OR Apache-2.0 | https://github.com/rust-lang/log |
| `markup5ever` | 0.39.0 | MIT OR Apache-2.0 | https://github.com/servo/html5ever |
| `memchr` | 2.8.3 | Unlicense OR MIT | https://github.com/BurntSushi/memchr |
| `memoffset` | 0.9.1 | MIT | https://github.com/Gilnaa/memoffset |
| `mime` | 0.3.17 | MIT OR Apache-2.0 | https://github.com/hyperium/mime |
| `miniz_oxide` | 0.8.9 | MIT OR Zlib OR Apache-2.0 | https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide |
| `miniz_oxide` | 0.9.1 | MIT OR Zlib OR Apache-2.0 | https://github.com/Frommi/miniz_oxide/tree/master/miniz_oxide |
| `mio` | 1.2.4 | MIT | https://github.com/tokio-rs/mio |
| `muda` | 0.20.0 | Apache-2.0 OR MIT | https://github.com/tauri-apps/muda |
| `native-tls` | 0.2.18 | MIT OR Apache-2.0 | https://github.com/rust-native-tls/rust-native-tls |
| `ndk` | 0.9.0 | MIT OR Apache-2.0 | https://github.com/rust-mobile/ndk |
| `ndk-context` | 0.1.1 | MIT OR Apache-2.0 | https://github.com/rust-windowing/android-ndk-rs |
| `ndk-sys` | 0.6.0+11769913 | MIT OR Apache-2.0 | https://github.com/rust-mobile/ndk |
| `new_debug_unreachable` | 1.0.6 | MIT | https://github.com/mbrubeck/rust-debug-unreachable |
| `num-conv` | 0.1.0 | MIT OR Apache-2.0 | https://github.com/jhpratt/num-conv |
| `num-traits` | 0.2.19 | MIT OR Apache-2.0 | https://github.com/rust-num/num-traits |
| `num_enum` | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | https://github.com/illicitonion/num_enum |
| `num_enum_derive` | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | https://github.com/illicitonion/num_enum |
| `objc2` | 0.6.5 | MIT | https://github.com/madsmtm/objc2 |
| `objc2-app-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-cloud-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-core-data` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-core-foundation` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-core-graphics` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-core-image` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-core-location` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-core-text` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-encode` | 4.1.0 | MIT | https://github.com/madsmtm/objc2 |
| `objc2-exception-helper` | 0.1.1 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-foundation` | 0.3.2 | MIT | https://github.com/madsmtm/objc2 |
| `objc2-io-surface` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-quartz-core` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-ui-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-user-notifications` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `objc2-web-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | https://github.com/madsmtm/objc2 |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 | https://github.com/matklad/once_cell |
| `openssl` | 0.10.81 | Apache-2.0 | https://github.com/rust-openssl/rust-openssl |
| `openssl-macros` | 0.1.1 | MIT/Apache-2.0 | crates.io |
| `openssl-probe` | 0.2.1 | MIT OR Apache-2.0 | https://github.com/rustls/openssl-probe |
| `openssl-sys` | 0.9.117 | MIT | https://github.com/rust-openssl/rust-openssl |
| `option-ext` | 0.2.0 | MPL-2.0 | https://github.com/soc/option-ext.git |
| `pango` | 0.18.3 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `pango-sys` | 0.18.0 | MIT | https://github.com/gtk-rs/gtk-rs-core |
| `parking_lot` | 0.12.5 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |
| `parking_lot_core` | 0.9.12 | MIT OR Apache-2.0 | https://github.com/Amanieu/parking_lot |
| `pem-rfc7468` | 1.0.0 | Apache-2.0 OR MIT | https://github.com/RustCrypto/formats |
| `percent-encoding` | 2.3.2 | MIT OR Apache-2.0 | https://github.com/servo/rust-url/ |
| `phf` | 0.13.1 | MIT | https://github.com/rust-phf/rust-phf |
| `phf_generator` | 0.13.1 | MIT | https://github.com/rust-phf/rust-phf |
| `phf_macros` | 0.13.1 | MIT | https://github.com/rust-phf/rust-phf |
| `phf_shared` | 0.13.1 | MIT | https://github.com/rust-phf/rust-phf |
| `pin-project-lite` | 0.2.17 | Apache-2.0 OR MIT | https://github.com/taiki-e/pin-project-lite |
| `plist` | 1.8.0 | MIT | https://github.com/ebarnard/rust-plist/ |
| `png` | 0.17.16 | MIT OR Apache-2.0 | https://github.com/image-rs/image-png |
| `png` | 0.18.1 | MIT OR Apache-2.0 | https://github.com/image-rs/image-png |
| `potential_utf` | 0.1.6 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `powerfmt` | 0.2.1 | MIT OR Apache-2.0 | https://github.com/jhpratt/powerfmt |
| `precomputed-hash` | 0.1.1 | MIT | https://github.com/emilio/precomputed-hash |
| `proc-macro-crate` | 1.3.1 | MIT OR Apache-2.0 | https://github.com/bkchr/proc-macro-crate |
| `proc-macro-crate` | 2.0.2 | MIT OR Apache-2.0 | https://github.com/bkchr/proc-macro-crate |
| `proc-macro-crate` | 3.5.0 | MIT OR Apache-2.0 | https://github.com/bkchr/proc-macro-crate |
| `proc-macro-error` | 1.0.4 | MIT OR Apache-2.0 | https://gitlab.com/CreepySkeleton/proc-macro-error |
| `proc-macro-error-attr` | 1.0.4 | MIT OR Apache-2.0 | https://gitlab.com/CreepySkeleton/proc-macro-error |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 | https://github.com/dtolnay/proc-macro2 |
| `quick-xml` | 0.38.4 | MIT | https://github.com/tafia/quick-xml |
| `quote` | 1.0.47 | MIT OR Apache-2.0 | https://github.com/dtolnay/quote |
| `r-efi` | 5.3.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | https://github.com/r-efi/r-efi |
| `r-efi` | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | https://github.com/r-efi/r-efi |
| `raw-window-handle` | 0.6.2 | MIT OR Apache-2.0 OR Zlib | https://github.com/rust-windowing/raw-window-handle |
| `redox_syscall` | 0.5.18 | MIT | https://gitlab.redox-os.org/redox-os/syscall |
| `redox_users` | 0.5.3 | MIT | https://gitlab.redox-os.org/redox-os/users |
| `ref-cast` | 1.0.27 | MIT OR Apache-2.0 | https://github.com/dtolnay/ref-cast |
| `ref-cast-impl` | 1.0.27 | MIT OR Apache-2.0 | https://github.com/dtolnay/ref-cast |
| `regex` | 1.13.1 | MIT OR Apache-2.0 | https://github.com/rust-lang/regex |
| `regex-automata` | 0.4.18 | MIT OR Apache-2.0 | https://github.com/rust-lang/regex |
| `regex-syntax` | 0.8.11 | MIT OR Apache-2.0 | https://github.com/rust-lang/regex |
| `reqwest` | 0.13.5 | MIT OR Apache-2.0 | https://github.com/seanmonstar/reqwest |
| `rfd` | 0.16.0 | MIT | https://github.com/PolyMeilex/rfd |
| `rusqlite` | 0.37.0 | MIT | https://github.com/rusqlite/rusqlite |
| `rustc-hash` | 2.1.3 | Apache-2.0 OR MIT | https://github.com/rust-lang/rustc-hash |
| `rustix` | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/bytecodealliance/rustix |
| `rustls-pki-types` | 1.15.1 | MIT OR Apache-2.0 | https://github.com/rustls/pki-types |
| `rustversion` | 1.0.23 | MIT OR Apache-2.0 | https://github.com/dtolnay/rustversion |
| `same-file` | 1.0.6 | Unlicense/MIT | https://github.com/BurntSushi/same-file |
| `schannel` | 0.1.29 | MIT | https://github.com/steffengy/schannel-rs |
| `schemars` | 0.8.22 | MIT | https://github.com/GREsau/schemars |
| `schemars` | 0.9.0 | MIT | https://github.com/GREsau/schemars |
| `schemars` | 1.2.3 | MIT | https://github.com/GREsau/schemars |
| `schemars_derive` | 0.8.22 | MIT | https://github.com/GREsau/schemars |
| `scopeguard` | 1.2.0 | MIT OR Apache-2.0 | https://github.com/bluss/scopeguard |
| `security-framework` | 3.7.0 | MIT OR Apache-2.0 | https://github.com/kornelski/rust-security-framework |
| `security-framework-sys` | 2.17.0 | MIT OR Apache-2.0 | https://github.com/kornelski/rust-security-framework |
| `selectors` | 0.38.0 | MPL-2.0 | https://github.com/servo/stylo |
| `semver` | 1.0.28 | MIT OR Apache-2.0 | https://github.com/dtolnay/semver |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| `serde-untagged` | 0.1.9 | MIT OR Apache-2.0 | https://github.com/dtolnay/serde-untagged |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| `serde_derive_internals` | 0.29.1 | MIT OR Apache-2.0 | https://github.com/serde-rs/serde |
| `serde_json` | 1.0.154 | MIT OR Apache-2.0 | https://github.com/serde-rs/json |
| `serde_repr` | 0.1.21 | MIT OR Apache-2.0 | https://github.com/dtolnay/serde-repr |
| `serde_spanned` | 0.6.9 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| `serde_spanned` | 1.1.2 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| `serde_with` | 3.17.0 | MIT OR Apache-2.0 | https://github.com/jonasbb/serde_with/ |
| `serde_with_macros` | 3.17.0 | MIT OR Apache-2.0 | https://github.com/jonasbb/serde_with/ |
| `serialize-to-javascript` | 0.1.2 | MIT OR Apache-2.0 | https://github.com/chippers/serialize-to-javascript |
| `serialize-to-javascript-impl` | 0.1.2 | MIT OR Apache-2.0 | https://github.com/chippers/serialize-to-javascript |
| `servo_arc` | 0.4.3 | MIT OR Apache-2.0 | https://github.com/servo/stylo |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | https://github.com/RustCrypto/hashes |
| `simd-adler32` | 0.3.10 | MIT | https://github.com/mcountryman/simd-adler32 |
| `siphasher` | 1.0.4 | MIT OR Apache-2.0 | https://github.com/jedisct1/rust-siphash |
| `slab` | 0.4.12 | MIT | https://github.com/tokio-rs/slab |
| `smallvec` | 1.16.3 | MIT OR Apache-2.0 | https://github.com/servo/rust-smallvec |
| `socket2` | 0.6.5 | MIT OR Apache-2.0 | https://github.com/rust-lang/socket2 |
| `softbuffer` | 0.4.8 | MIT OR Apache-2.0 | https://github.com/rust-windowing/softbuffer |
| `soup3` | 0.5.0 | MIT | https://gitlab.gnome.org/World/Rust/soup3-rs |
| `soup3-sys` | 0.5.0 | MIT | https://gitlab.gnome.org/World/Rust/soup3-rs |
| `stable_deref_trait` | 1.2.1 | MIT OR Apache-2.0 | https://github.com/storyyeller/stable_deref_trait |
| `string_cache` | 0.9.0 | MIT OR Apache-2.0 | https://github.com/servo/string-cache |
| `strsim` | 0.11.1 | MIT | https://github.com/rapidfuzz/strsim-rs |
| `swift-rs` | 1.0.9 | MIT OR Apache-2.0 | https://github.com/Brendonovich/swift-rs |
| `syn` | 1.0.109 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| `syn` | 2.0.119 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| `syn` | 3.0.9 | MIT OR Apache-2.0 | https://github.com/dtolnay/syn |
| `sync_wrapper` | 1.0.2 | Apache-2.0 | https://github.com/Actyx/sync_wrapper |
| `synstructure` | 0.14.0 | MIT | https://github.com/mystor/synstructure |
| `tao` | 0.37.1 | Apache-2.0 | https://github.com/tauri-apps/tao |
| `tao-macros` | 0.1.4 | MIT OR Apache-2.0 | https://github.com/tauri-apps/tao |
| `tauri` | 2.12.4 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| `tauri-codegen` | 2.7.3 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| `tauri-macros` | 2.7.3 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| `tauri-plugin-dialog` | 2.8.3 | Apache-2.0 OR MIT | https://github.com/tauri-apps/plugins-workspace |
| `tauri-plugin-fs` | 2.7.1 | Apache-2.0 OR MIT | https://github.com/tauri-apps/plugins-workspace |
| `tauri-runtime` | 2.12.2 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| `tauri-runtime-wry` | 2.12.2 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| `tauri-utils` | 2.10.2 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | https://github.com/Stebalien/tempfile |
| `tendril` | 0.5.1 | MIT OR Apache-2.0 | https://github.com/servo/html5ever |
| `thiserror` | 1.0.69 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| `thiserror-impl` | 1.0.69 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| `thiserror-impl` | 2.0.21 | MIT OR Apache-2.0 | https://github.com/dtolnay/thiserror |
| `time` | 0.3.45 | MIT OR Apache-2.0 | https://github.com/time-rs/time |
| `time-core` | 0.1.7 | MIT OR Apache-2.0 | https://github.com/time-rs/time |
| `time-macros` | 0.2.25 | MIT OR Apache-2.0 | https://github.com/time-rs/time |
| `tinystr` | 0.8.4 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `tokio` | 1.53.3 | MIT | https://github.com/tokio-rs/tokio |
| `tokio-util` | 0.7.20 | MIT | https://github.com/tokio-rs/tokio |
| `toml` | 1.1.8+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| `toml_datetime` | 0.6.3 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| `toml_datetime` | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| `toml_edit` | 0.19.15 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| `toml_edit` | 0.20.2 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| `toml_edit` | 0.25.17+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| `toml_parser` | 1.1.5+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| `toml_writer` | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | https://github.com/toml-rs/toml |
| `tower` | 0.5.3 | MIT | https://github.com/tower-rs/tower |
| `tower-http` | 0.6.11 | MIT | https://github.com/tower-rs/tower-http |
| `tower-layer` | 0.3.3 | MIT | https://github.com/tower-rs/tower |
| `tower-service` | 0.3.3 | MIT | https://github.com/tower-rs/tower |
| `tracing` | 0.1.44 | MIT | https://github.com/tokio-rs/tracing |
| `tracing-core` | 0.1.36 | MIT | https://github.com/tokio-rs/tracing |
| `tray-icon` | 0.25.1 | MIT OR Apache-2.0 | https://github.com/tauri-apps/tray-icon |
| `try-lock` | 0.2.5 | MIT | https://github.com/seanmonstar/try-lock |
| `typeid` | 1.0.3 | MIT OR Apache-2.0 | https://github.com/dtolnay/typeid |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 | https://github.com/paholg/typenum |
| `unicode-ident` | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | https://github.com/dtolnay/unicode-ident |
| `unicode-segmentation` | 1.13.3 | MIT OR Apache-2.0 | https://github.com/unicode-rs/unicode-segmentation |
| `ureq` | 3.4.2 | MIT OR Apache-2.0 | https://github.com/algesten/ureq |
| `ureq-proto` | 0.6.4 | MIT OR Apache-2.0 | https://github.com/algesten/ureq-proto |
| `url` | 2.5.8 | MIT OR Apache-2.0 | https://github.com/servo/rust-url |
| `urlpattern` | 0.6.0 | MIT | https://github.com/denoland/rust-urlpattern |
| `utf8-zero` | 0.8.1 | MIT OR Apache-2.0 | https://github.com/algesten/utf8-zero |
| `utf8_iter` | 1.0.4 | Apache-2.0 OR MIT | https://github.com/hsivonen/utf8_iter |
| `uuid` | 1.26.1 | Apache-2.0 OR MIT | https://github.com/uuid-rs/uuid |
| `walkdir` | 2.5.0 | Unlicense/MIT | https://github.com/BurntSushi/walkdir |
| `want` | 0.3.2 | MIT | https://github.com/seanmonstar/want |
| `wasi` | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/bytecodealliance/wasi |
| `wasip2` | 1.0.1+wasi-0.2.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/bytecodealliance/wasi-rs |
| `wasm-bindgen` | 0.2.129 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen |
| `wasm-bindgen-futures` | 0.4.79 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/futures |
| `wasm-bindgen-macro` | 0.2.129 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/macro |
| `wasm-bindgen-macro-support` | 0.2.129 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/main/crates/macro-support |
| `wasm-bindgen-shared` | 0.2.129 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/shared |
| `wasm-streams` | 0.5.0 | MIT OR Apache-2.0 | https://github.com/MattiasBuelens/wasm-streams/ |
| `web-sys` | 0.3.106 | MIT OR Apache-2.0 | https://github.com/wasm-bindgen/wasm-bindgen/tree/master/crates/web-sys |
| `web-time` | 1.1.0 | MIT OR Apache-2.0 | https://github.com/daxpedda/web-time |
| `web_atoms` | 0.2.6 | MIT OR Apache-2.0 | https://github.com/servo/html5ever |
| `webkit2gtk` | 2.0.2 | MIT | https://github.com/tauri-apps/webkit2gtk-rs |
| `webkit2gtk-sys` | 2.0.2 | MIT | https://github.com/tauri-apps/webkit2gtk-rs |
| `webview2-com` | 0.39.1 | MIT | https://github.com/wravery/webview2-rs |
| `webview2-com-macros` | 0.8.1 | MIT | https://github.com/wravery/webview2-rs |
| `webview2-com-sys` | 0.39.1 | MIT | https://github.com/wravery/webview2-rs |
| `winapi` | 0.3.9 | MIT/Apache-2.0 | https://github.com/retep998/winapi-rs |
| `winapi-i686-pc-windows-gnu` | 0.4.0 | MIT/Apache-2.0 | https://github.com/retep998/winapi-rs |
| `winapi-util` | 0.1.11 | Unlicense OR MIT | https://github.com/BurntSushi/winapi-util |
| `winapi-x86_64-pc-windows-gnu` | 0.4.0 | MIT/Apache-2.0 | https://github.com/retep998/winapi-rs |
| `window-vibrancy` | 0.8.1 | Apache-2.0 OR MIT | https://github.com/tauri-apps/tauri-plugin-vibrancy |
| `windows` | 0.62.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-collections` | 0.3.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-core` | 0.62.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-future` | 0.3.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-implement` | 0.60.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-interface` | 0.59.3 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-numerics` | 0.3.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-result` | 0.4.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-strings` | 0.5.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-sys` | 0.45.0 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-sys` | 0.60.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-targets` | 0.42.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-targets` | 0.53.5 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-threading` | 0.2.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows-version` | 0.1.7 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_aarch64_gnullvm` | 0.42.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_aarch64_gnullvm` | 0.53.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_aarch64_msvc` | 0.42.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_aarch64_msvc` | 0.53.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_i686_gnu` | 0.42.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_i686_gnu` | 0.53.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_i686_gnullvm` | 0.53.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_i686_msvc` | 0.42.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_i686_msvc` | 0.53.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_x86_64_gnu` | 0.42.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_x86_64_gnu` | 0.53.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_x86_64_gnullvm` | 0.42.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_x86_64_gnullvm` | 0.53.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_x86_64_msvc` | 0.42.2 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `windows_x86_64_msvc` | 0.53.1 | MIT OR Apache-2.0 | https://github.com/microsoft/windows-rs |
| `winnow` | 0.5.40 | MIT | https://github.com/winnow-rs/winnow |
| `winnow` | 1.0.4 | MIT | https://github.com/winnow-rs/winnow |
| `wit-bindgen` | 0.46.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | https://github.com/bytecodealliance/wit-bindgen |
| `writeable` | 0.6.4 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `wry` | 0.57.0 | Apache-2.0 OR MIT | https://github.com/tauri-apps/wry |
| `x11` | 2.21.0 | MIT | https://github.com/AltF02/x11-rs.git |
| `x11-dl` | 2.21.0 | MIT | https://github.com/AltF02/x11-rs.git |
| `yoke` | 0.8.3 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `yoke-derive` | 0.8.4 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `zerofrom` | 0.1.8 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `zerofrom-derive` | 0.1.8 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `zeroize` | 1.9.1 | Apache-2.0 OR MIT | https://github.com/RustCrypto/utils |
| `zerotrie` | 0.2.5 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `zerovec` | 0.11.8 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `zerovec-derive` | 0.11.6 | Unicode-3.0 | https://github.com/unicode-org/icu4x |
| `zlib-rs` | 0.6.8 | Zlib | https://github.com/trifectatechfoundation/zlib-rs |
| `zmij` | 1.0.23 | MIT | https://github.com/dtolnay/zmij |

## 桌面发行包：运行时 npm 包（1）

| 包 | 版本 | 许可 |
| --- | --- | --- |
| @tauri-apps/api | 2.12.2 | Apache-2.0 OR MIT |
