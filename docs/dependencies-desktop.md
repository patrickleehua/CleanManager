# 桌面端依赖清单

本文件由 `python tools/dependency_inventory.py` 依据 `apps/desktop/src-tauri/Cargo.lock` 与 `apps/desktop/package-lock.json` 生成；文档验证器会比对三者，手工修改会被拒绝。核心与 CLI 的依赖见 [依赖与工具链清单](dependencies.md)。

- Node.js 与 npm 只用于构建 webview 资源与运行前端测试；`dev` 范围的包不进入安装包。
- 桌面端通过路径依赖复用 workspace 内的 `cm-core`、`cm-app`、`cm-store`、`cm-platform`，下表不重复列出它们及其 crates.io 依赖以外的本地 crate。
- 不捆绑第三方二进制，`.research/` 中的研究副本不被任何清单引用（验证器检查）。

## Rust crates（439）

| Crate | 版本 | 许可 | 来源 | 用途 |
| --- | --- | --- | --- | --- |
| `adler2` | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | crates.io | Transitive: required by miniz_oxide |
| `aho-corasick` | 1.1.5 | Unlicense OR MIT | crates.io | Transitive: required by regex, regex-automata |
| `alloc-no-stdlib` | 3.0.0 | BSD-3-Clause | crates.io | Transitive: required by alloc-stdlib, brotli, brotli-decompressor |
| `alloc-stdlib` | 0.3.0 | BSD-3-Clause | crates.io | Transitive: required by brotli, brotli-decompressor |
| `android_system_properties` | 0.1.6 | MIT OR Apache-2.0 | crates.io | Transitive: required by iana-time-zone |
| `anyhow` | 1.0.104 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri, tauri-build, tauri-plugin, tauri-plugin-fs, tauri-utils |
| `atk` | 0.18.2 | MIT | crates.io | Transitive: required by gtk |
| `atk-sys` | 0.18.2 | MIT | crates.io | Transitive: required by atk, gtk-sys |
| `atomic-waker` | 1.1.2 | Apache-2.0 OR MIT | crates.io | Transitive: required by hyper |
| `autocfg` | 1.5.1 | Apache-2.0 OR MIT | crates.io | Transitive: required by indexmap, memoffset, num-traits |
| `base64` | 0.21.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by swift-rs |
| `base64` | 0.22.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by cm-core, cm-store, plist, serde_with, wry |
| `base64` | 0.23.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by hyper-util, reqwest, tauri-codegen, ureq, ureq-proto |
| `base64ct` | 1.8.3 | Apache-2.0 OR MIT | crates.io | Transitive: required by pem-rfc7468 |
| `bit-set` | 0.8.0 | Apache-2.0 OR MIT | crates.io | Transitive: required by dom_query |
| `bit-vec` | 0.8.0 | Apache-2.0 OR MIT | crates.io | Transitive: required by bit-set |
| `bitflags` | 1.3.2 | MIT/Apache-2.0 | crates.io | Transitive: required by javascriptcore-rs, png, webkit2gtk, webkit2gtk-sys |
| `bitflags` | 2.13.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by cairo-rs, core-graphics, core-graphics-types, dispatch2, glib, keyboard-types, ndk, objc2-app-kit, objc2-cloud-kit, objc2-core-foundation, objc2-core-graphics, objc2-core-text, objc2-foundation, objc2-io-surface, objc2-quartz-core, objc2-ui-kit, objc2-web-kit, openssl, png, redox_syscall, rusqlite, rustix, security-framework, selectors, tao, tower-http |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by digest |
| `block2` | 0.6.2 | MIT | crates.io | Transitive: required by dispatch2, objc2-app-kit, objc2-foundation, objc2-ui-kit, objc2-web-kit, rfd, tao, wry |
| `brotli` | 9.0.0 | BSD-3-Clause AND MIT | crates.io | Transitive: required by tauri-codegen, tauri-utils |
| `brotli-decompressor` | 6.0.1 | BSD-3-Clause/MIT | crates.io | Transitive: required by brotli |
| `bumpalo` | 3.20.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by wasm-bindgen-macro-support |
| `bytemuck` | 1.25.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by softbuffer |
| `byteorder` | 1.5.0 | Unlicense OR MIT | crates.io | Transitive: required by ico |
| `bytes` | 1.12.1 | MIT | crates.io | Transitive: required by combine, http, http-body, http-body-util, hyper, hyper-util, reqwest, tauri, tokio, tokio-util, tower-http |
| `cairo-rs` | 0.18.5 | MIT | crates.io | Transitive: required by gdk, gtk, webkit2gtk |
| `cairo-sys-rs` | 0.18.2 | MIT | crates.io | Transitive: required by cairo-rs, gdk-sys, gtk-sys, webkit2gtk-sys |
| `camino` | 1.2.6 | MIT OR Apache-2.0 | crates.io | Transitive: required by cargo_metadata |
| `cargo-platform` | 0.1.9 | MIT OR Apache-2.0 | crates.io | Transitive: required by cargo_metadata |
| `cargo_metadata` | 0.19.2 | MIT | crates.io | Transitive: required by tauri-utils |
| `cargo_toml` | 1.0.1 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri-build |
| `cc` | 1.8.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by embed-resource, iana-time-zone-haiku, libsqlite3-sys, objc2-exception-helper, openssl-sys, vswhom-sys |
| `cesu8` | 1.1.0 | Apache-2.0/MIT | crates.io | Transitive: required by jni |
| `cfb` | 0.14.0 | MIT | crates.io | Transitive: required by infer |
| `cfg-expr` | 0.15.8 | MIT OR Apache-2.0 | crates.io | Transitive: required by system-deps |
| `cfg-if` | 1.0.5 | MIT OR Apache-2.0 | crates.io | Transitive: required by crc32fast, getrandom, jni, js-sys, libloading, openssl, parking_lot_core, sha2, wasm-bindgen, winreg |
| `chrono` | 0.4.45 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde_with |
| `combine` | 4.6.8 | MIT | crates.io | Transitive: required by jni |
| `cookie` | 0.18.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri, tauri-runtime, wry |
| `core-foundation` | 0.10.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by core-graphics, core-graphics-types, security-framework, tao |
| `core-foundation-sys` | 0.8.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by core-foundation, iana-time-zone, security-framework, security-framework-sys |
| `core-graphics` | 0.25.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by tao |
| `core-graphics-types` | 0.2.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by core-graphics |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 | crates.io | Transitive: required by sha2 |
| `crc32fast` | 1.5.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by flate2, png |
| `crossbeam-channel` | 0.5.17 | MIT OR Apache-2.0 | crates.io | Transitive: required by muda, tao, tray-icon, wry |
| `crossbeam-utils` | 0.8.23 | MIT OR Apache-2.0 | crates.io | Transitive: required by crossbeam-channel |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by digest |
| `cssparser` | 0.37.0 | MPL-2.0 | crates.io | Transitive: required by dom_query, selectors |
| `cssparser-macros` | 0.7.1 | MPL-2.0 | crates.io | Transitive: required by cssparser |
| `ctor` | 1.0.13 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri-utils |
| `darling` | 0.21.3 | MIT | crates.io | Transitive: required by serde_with_macros |
| `darling_core` | 0.21.3 | MIT | crates.io | Transitive: required by darling, darling_macro |
| `darling_macro` | 0.21.3 | MIT | crates.io | Transitive: required by darling |
| `dbus` | 0.9.12 | Apache-2.0/MIT | crates.io | Transitive: required by tao |
| `der` | 0.8.2 | Apache-2.0 OR MIT | crates.io | Transitive: required by ureq |
| `deranged` | 0.5.9 | MIT OR Apache-2.0 | crates.io | Transitive: required by time |
| `derive_more` | 2.1.1 | MIT | crates.io | Transitive: required by selectors |
| `derive_more-impl` | 2.1.1 | MIT | crates.io | Transitive: required by derive_more |
| `digest` | 0.10.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by sha2 |
| `dirs` | 7.0.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri, tauri-build, tray-icon, wry |
| `dirs-sys` | 0.5.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by dirs |
| `dispatch2` | 0.3.1 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by objc2-core-foundation, objc2-core-graphics, rfd, tao |
| `displaydoc` | 0.2.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by icu_collections, icu_locale_core, icu_provider, tinystr, zerotrie |
| `dlopen2` | 0.8.2 | MIT | crates.io | Transitive: required by tao |
| `dlopen2_derive` | 0.4.3 | MIT | crates.io | Transitive: required by dlopen2 |
| `dom_query` | 0.28.0 | MIT | crates.io | Transitive: required by tauri-utils, wry |
| `dpi` | 0.1.2 | Apache-2.0 AND MIT | crates.io | Transitive: required by muda, tao, tauri-runtime, wry |
| `dtoa` | 1.0.11 | MIT OR Apache-2.0 | crates.io | Transitive: required by dtoa-short |
| `dtoa-short` | 0.3.5 | MPL-2.0 | crates.io | Transitive: required by cssparser |
| `dunce` | 1.0.5 | CC0-1.0 OR MIT-0 OR Apache-2.0 | crates.io | Transitive: required by tauri, tauri-plugin-fs, tauri-utils, tauri-winres, wry |
| `dyn-clone` | 1.0.20 | MIT OR Apache-2.0 | crates.io | Transitive: required by schemars |
| `embed-resource` | 3.0.12 | MIT | crates.io | Transitive: required by tauri-winres |
| `embed_plist` | 1.2.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri |
| `equivalent` | 1.0.3 | Apache-2.0 OR MIT | crates.io | Transitive: required by indexmap |
| `erased-serde` | 0.4.10 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde-untagged |
| `errno` | 0.3.14 | MIT OR Apache-2.0 | crates.io | Transitive: required by rustix |
| `fallible-iterator` | 0.3.0 | MIT/Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `fallible-streaming-iterator` | 0.1.9 | MIT/Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `fastrand` | 2.5.0 | Apache-2.0 OR MIT | crates.io | Transitive: required by phf_generator, tempfile |
| `fdeflate` | 0.3.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by png |
| `field-offset` | 0.3.6 | MIT OR Apache-2.0 | crates.io | Transitive: required by gtk |
| `find-msvc-tools` | 0.1.14 | MIT OR Apache-2.0 | crates.io | Transitive: required by cc |
| `flate2` | 1.1.10 | MIT OR Apache-2.0 | crates.io | Transitive: required by png |
| `fnv` | 1.0.7 | Apache-2.0 / MIT | crates.io | Transitive: required by cfb, darling_core |
| `foldhash` | 0.1.5 | Zlib | crates.io | Transitive: required by hashbrown |
| `foldhash` | 0.2.0 | Zlib | crates.io | Transitive: required by dom_query |
| `foreign-types` | 0.3.2 | MIT/Apache-2.0 | crates.io | Transitive: required by openssl |
| `foreign-types` | 0.5.0 | MIT/Apache-2.0 | crates.io | Transitive: required by core-graphics |
| `foreign-types-macros` | 0.2.4 | MIT/Apache-2.0 | crates.io | Transitive: required by foreign-types |
| `foreign-types-shared` | 0.1.1 | MIT/Apache-2.0 | crates.io | Transitive: required by foreign-types |
| `foreign-types-shared` | 0.3.1 | MIT/Apache-2.0 | crates.io | Transitive: required by foreign-types |
| `form_urlencoded` | 1.2.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by url |
| `futures-channel` | 0.3.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by gio, glib, gtk, hyper-util, soup3 |
| `futures-core` | 0.3.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by futures-channel, futures-executor, futures-util, gio, glib, http-body-util, hyper, reqwest, sync_wrapper, tokio-util, tower |
| `futures-executor` | 0.3.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by glib |
| `futures-io` | 0.3.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by futures-util, gio |
| `futures-macro` | 0.3.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by futures-util |
| `futures-sink` | 0.3.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by futures-util, tokio-util |
| `futures-task` | 0.3.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by futures-executor, futures-util, glib |
| `futures-util` | 0.3.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by futures-executor, gio, glib, hyper-util, js-sys, reqwest, tower, tower-http, wasm-streams |
| `gdk` | 0.18.2 | MIT | crates.io | Transitive: required by gdkx11, gtk, webkit2gtk |
| `gdk-pixbuf` | 0.18.5 | MIT | crates.io | Transitive: required by gdk, gtk |
| `gdk-pixbuf-sys` | 0.18.0 | MIT | crates.io | Transitive: required by gdk-pixbuf, gdk-sys, gtk-sys |
| `gdk-sys` | 0.18.2 | MIT | crates.io | Transitive: required by gdk, gdkwayland-sys, gdkx11-sys, gtk-sys, webkit2gtk, webkit2gtk-sys |
| `gdkwayland-sys` | 0.18.2 | MIT | crates.io | Transitive: required by tao |
| `gdkx11` | 0.18.2 | MIT | crates.io | Transitive: required by wry |
| `gdkx11-sys` | 0.18.2 | MIT | crates.io | Transitive: required by gdkx11, tao |
| `generic-array` | 0.14.7 | MIT | crates.io | Transitive: required by block-buffer, crypto-common |
| `getrandom` | 0.3.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri |
| `getrandom` | 0.4.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by tempfile, uuid |
| `gio` | 0.18.4 | MIT | crates.io | Transitive: required by gdk, gdk-pixbuf, gdkx11, gtk, pango, soup3, webkit2gtk |
| `gio-sys` | 0.18.1 | MIT | crates.io | Transitive: required by gdk-pixbuf-sys, gdk-sys, gio, glib, gtk-sys, soup3-sys, webkit2gtk, webkit2gtk-sys |
| `glib` | 0.18.5 | MIT | crates.io | Transitive: required by atk, cairo-rs, gdk, gdk-pixbuf, gdkx11, gio, gtk, javascriptcore-rs, libappindicator, pango, soup3, webkit2gtk |
| `glib-macros` | 0.18.5 | MIT | crates.io | Transitive: required by glib |
| `glib-sys` | 0.18.1 | MIT | crates.io | Transitive: required by atk-sys, cairo-sys-rs, gdk-pixbuf-sys, gdk-sys, gdkwayland-sys, gdkx11-sys, gio-sys, glib, gobject-sys, gtk-sys, javascriptcore-rs-sys, pango-sys, rfd, soup3-sys, webkit2gtk, webkit2gtk-sys |
| `glob` | 0.3.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri, tauri-build, tauri-plugin, tauri-plugin-fs, tauri-utils |
| `gobject-sys` | 0.18.0 | MIT | crates.io | Transitive: required by atk-sys, gdk-pixbuf-sys, gdk-sys, gdkwayland-sys, gio-sys, glib, gtk-sys, javascriptcore-rs-sys, pango-sys, rfd, soup3-sys, webkit2gtk, webkit2gtk-sys |
| `gtk` | 0.18.2 | MIT | crates.io | Transitive: required by libappindicator, muda, tao, tauri, tauri-runtime, tauri-runtime-wry, webkit2gtk, wry |
| `gtk-sys` | 0.18.2 | MIT | crates.io | Transitive: required by gtk, libappindicator, libappindicator-sys, rfd, webkit2gtk, webkit2gtk-sys |
| `gtk3-macros` | 0.18.2 | MIT | crates.io | Transitive: required by gtk |
| `hashbrown` | 0.12.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by indexmap |
| `hashbrown` | 0.15.5 | MIT OR Apache-2.0 | crates.io | Transitive: required by hashlink |
| `hashbrown` | 0.17.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by indexmap |
| `hashlink` | 0.10.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by rusqlite |
| `heck` | 0.4.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by glib-macros |
| `heck` | 0.5.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by system-deps, tauri, tauri-build, tauri-macros |
| `hex` | 0.4.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde_with |
| `html5ever` | 0.39.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by dom_query |
| `http` | 1.5.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by http-body, http-body-util, hyper, hyper-util, reqwest, tauri, tauri-runtime, tauri-runtime-wry, tauri-utils, tower-http, ureq-proto, wry |
| `http-body` | 1.1.0 | MIT | crates.io | Transitive: required by http-body-util, hyper, hyper-util, reqwest, tower-http |
| `http-body-util` | 0.1.5 | MIT | crates.io | Transitive: required by reqwest |
| `httparse` | 1.10.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by hyper, hyper-util, ureq-proto |
| `hyper` | 1.12.0 | MIT | crates.io | Transitive: required by hyper-util, reqwest |
| `hyper-util` | 0.1.21 | MIT | crates.io | Transitive: required by reqwest |
| `iana-time-zone` | 0.1.65 | MIT OR Apache-2.0 | crates.io | Transitive: required by chrono |
| `iana-time-zone-haiku` | 0.1.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by iana-time-zone |
| `ico` | 0.5.0 | MIT | crates.io | Transitive: required by tauri-codegen |
| `icu_collections` | 2.1.1 | Unicode-3.0 | crates.io | Transitive: required by icu_normalizer, icu_properties |
| `icu_locale_core` | 2.1.1 | Unicode-3.0 | crates.io | Transitive: required by icu_properties, icu_provider |
| `icu_normalizer` | 2.1.1 | Unicode-3.0 | crates.io | Transitive: required by idna_adapter |
| `icu_normalizer_data` | 2.1.1 | Unicode-3.0 | crates.io | Transitive: required by icu_normalizer |
| `icu_properties` | 2.1.2 | Unicode-3.0 | crates.io | Transitive: required by icu_normalizer, idna_adapter, urlpattern |
| `icu_properties_data` | 2.1.2 | Unicode-3.0 | crates.io | Transitive: required by icu_properties |
| `icu_provider` | 2.1.1 | Unicode-3.0 | crates.io | Transitive: required by icu_normalizer, icu_properties |
| `ident_case` | 1.0.1 | MIT/Apache-2.0 | crates.io | Transitive: required by darling_core |
| `idna` | 1.1.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by url |
| `idna_adapter` | 1.2.1 | Apache-2.0 OR MIT | crates.io | Transitive: required by idna |
| `indexmap` | 1.9.3 | Apache-2.0 OR MIT | crates.io | Transitive: required by schemars, serde_with |
| `indexmap` | 2.14.2 | Apache-2.0 OR MIT | crates.io | Transitive: required by plist, serde_with, toml, toml_edit |
| `infer` | 0.22.0 | MIT | crates.io | Transitive: required by tauri-utils |
| `ipnet` | 2.12.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by hyper-util |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 | crates.io | Transitive: required by cssparser, http, hyper, serde_json, time |
| `javascriptcore-rs` | 1.1.2 | MIT | crates.io | Transitive: required by webkit2gtk, wry |
| `javascriptcore-rs-sys` | 1.1.1 | MIT | crates.io | Transitive: required by javascriptcore-rs, webkit2gtk-sys |
| `jni` | 0.21.1 | MIT/Apache-2.0 | crates.io | Transitive: required by tao, tauri, tauri-runtime, tauri-runtime-wry, wry |
| `jni-sys` | 0.3.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by jni, ndk, ndk-sys |
| `jni-sys` | 0.4.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by jni-sys |
| `jni-sys-macros` | 0.4.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by jni-sys |
| `js-sys` | 0.3.106 | MIT OR Apache-2.0 | crates.io | Transitive: required by iana-time-zone, reqwest, rfd, softbuffer, uuid, wasm-bindgen-futures, wasm-streams, web-sys, web-time |
| `json-patch` | 4.2.0 | MIT/Apache-2.0 | crates.io | Transitive: required by tauri-build, tauri-codegen, tauri-utils |
| `jsonptr` | 0.7.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by json-patch |
| `keyboard-types` | 0.8.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by muda |
| `libappindicator` | 0.9.0 | Apache-2.0 OR MIT | crates.io | Transitive: required by tray-icon |
| `libappindicator-sys` | 0.9.0 | Apache-2.0 OR MIT | crates.io | Transitive: required by libappindicator |
| `libc` | 0.2.190 | MIT OR Apache-2.0 | crates.io | Transitive: required by android_system_properties, atk, atk-sys, cairo-rs, cairo-sys-rs, cm-platform, core-foundation, core-graphics, core-graphics-types, cpufeatures, dbus, dirs-sys, dispatch2, dlopen2, errno, gdk, gdk-pixbuf, gdk-pixbuf-sys, gdk-sys, gdkwayland-sys, gdkx11, gdkx11-sys, getrandom, gio, gio-sys, glib, glib-sys, gobject-sys, gtk, gtk-sys, hyper-util, javascriptcore-rs-sys, libredox, mio, native-tls, objc2-foundation, openssl, openssl-sys, pango, pango-sys, parking_lot_core, rustix, security-framework, security-framework-sys, socket2, soup3, soup3-sys, tao, tauri, tokio, vswhom, vswhom-sys, webkit2gtk, webkit2gtk-sys, wry, x11, x11-dl |
| `libdbus-sys` | 0.2.7 | Apache-2.0/MIT | crates.io | Transitive: required by dbus |
| `libloading` | 0.7.4 | ISC | crates.io | Transitive: required by libappindicator-sys |
| `libredox` | 0.1.25 | MIT | crates.io | Transitive: required by redox_users |
| `libsqlite3-sys` | 0.35.0 | MIT | crates.io | Transitive: required by rusqlite |
| `linux-raw-sys` | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | crates.io | Transitive: required by rustix |
| `litemap` | 0.8.3 | Unicode-3.0 | crates.io | Transitive: required by icu_locale_core |
| `lock_api` | 0.4.14 | MIT OR Apache-2.0 | crates.io | Transitive: required by parking_lot |
| `log` | 0.4.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by html5ever, iana-time-zone, jni, libappindicator, markup5ever, native-tls, ndk, reqwest, rfd, selectors, tao, tauri, tauri-plugin-dialog, tauri-plugin-fs, tauri-runtime-wry, tauri-utils, ureq, ureq-proto |
| `markup5ever` | 0.39.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by html5ever |
| `memchr` | 2.8.3 | Unlicense OR MIT | crates.io | Transitive: required by aho-corasick, combine, embed-resource, futures-util, glib, quick-xml, regex, regex-automata, serde_json, tauri-utils, winnow |
| `memoffset` | 0.9.1 | MIT | crates.io | Transitive: required by field-offset |
| `mime` | 0.3.17 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri |
| `miniz_oxide` | 0.8.9 | MIT OR Zlib OR Apache-2.0 | crates.io | Transitive: required by png |
| `miniz_oxide` | 0.9.1 | MIT OR Zlib OR Apache-2.0 | crates.io | Transitive: required by flate2 |
| `mio` | 1.2.4 | MIT | crates.io | Transitive: required by tokio |
| `muda` | 0.20.0 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri, tray-icon |
| `native-tls` | 0.2.18 | MIT OR Apache-2.0 | crates.io | Transitive: required by ureq |
| `ndk` | 0.9.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by softbuffer, tao, wry |
| `ndk-context` | 0.1.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by tao |
| `ndk-sys` | 0.6.0+11769913 | MIT OR Apache-2.0 | crates.io | Transitive: required by ndk, tao |
| `new_debug_unreachable` | 1.0.6 | MIT | crates.io | Transitive: required by selectors, string_cache, tendril |
| `num-conv` | 0.1.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by time, time-macros |
| `num-traits` | 0.2.19 | MIT OR Apache-2.0 | crates.io | Transitive: required by chrono |
| `num_enum` | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | crates.io | Transitive: required by ndk |
| `num_enum_derive` | 0.7.6 | BSD-3-Clause OR MIT OR Apache-2.0 | crates.io | Transitive: required by num_enum |
| `objc2` | 0.6.5 | MIT | crates.io | Transitive: required by block2, dispatch2, muda, objc2-app-kit, objc2-cloud-kit, objc2-core-data, objc2-core-foundation, objc2-core-graphics, objc2-core-image, objc2-core-location, objc2-core-text, objc2-foundation, objc2-io-surface, objc2-quartz-core, objc2-ui-kit, objc2-user-notifications, objc2-web-kit, rfd, softbuffer, tao, tauri, tauri-runtime, tauri-runtime-wry, tray-icon, window-vibrancy, wry |
| `objc2-app-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by muda, objc2-web-kit, rfd, tao, tauri, tauri-runtime-wry, tray-icon, window-vibrancy, wry |
| `objc2-cloud-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by objc2-ui-kit |
| `objc2-core-data` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by objc2-ui-kit |
| `objc2-core-foundation` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by muda, objc2-app-kit, objc2-core-graphics, objc2-core-text, objc2-foundation, objc2-io-surface, objc2-quartz-core, objc2-ui-kit, objc2-web-kit, rfd, softbuffer, tray-icon, window-vibrancy, wry |
| `objc2-core-graphics` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by objc2-core-text, objc2-ui-kit, softbuffer, tray-icon |
| `objc2-core-image` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by objc2-ui-kit |
| `objc2-core-location` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by objc2-ui-kit |
| `objc2-core-text` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by objc2-ui-kit |
| `objc2-encode` | 4.1.0 | MIT | crates.io | Transitive: required by objc2 |
| `objc2-exception-helper` | 0.1.1 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by objc2 |
| `objc2-foundation` | 0.3.2 | MIT | crates.io | Transitive: required by muda, objc2-app-kit, objc2-cloud-kit, objc2-core-data, objc2-core-image, objc2-core-location, objc2-quartz-core, objc2-ui-kit, objc2-user-notifications, objc2-web-kit, rfd, softbuffer, tao, tauri, tauri-plugin-fs, tray-icon, window-vibrancy, wry |
| `objc2-io-surface` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by objc2-core-graphics |
| `objc2-quartz-core` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by objc2-app-kit, objc2-ui-kit, softbuffer, window-vibrancy |
| `objc2-ui-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by tao, tauri, tauri-runtime, wry |
| `objc2-user-notifications` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by objc2-ui-kit |
| `objc2-web-kit` | 0.3.2 | Zlib OR Apache-2.0 OR MIT | crates.io | Transitive: required by tauri, tauri-runtime, wry |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by cairo-rs, dlopen2, gdk-pixbuf, gio, glib, libappindicator-sys, muda, pango, proc-macro-crate, tauri-runtime-wry, tempfile, tracing-core, tray-icon, wasm-bindgen, webkit2gtk, wry, x11-dl |
| `openssl` | 0.10.81 | Apache-2.0 | crates.io | Transitive: required by native-tls |
| `openssl-macros` | 0.1.1 | MIT/Apache-2.0 | crates.io | Transitive: required by openssl |
| `openssl-probe` | 0.2.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by native-tls |
| `openssl-sys` | 0.9.117 | MIT | crates.io | Transitive: required by native-tls, openssl |
| `option-ext` | 0.2.0 | MPL-2.0 | crates.io | Transitive: required by dirs-sys |
| `pango` | 0.18.3 | MIT | crates.io | Transitive: required by gdk, gtk |
| `pango-sys` | 0.18.0 | MIT | crates.io | Transitive: required by gdk-sys, gtk-sys, pango |
| `parking_lot` | 0.12.5 | MIT OR Apache-2.0 | crates.io | Transitive: required by string_cache, tao |
| `parking_lot_core` | 0.9.12 | MIT OR Apache-2.0 | crates.io | Transitive: required by parking_lot |
| `pem-rfc7468` | 1.0.0 | Apache-2.0 OR MIT | crates.io | Transitive: required by der |
| `percent-encoding` | 2.3.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by form_urlencoded, hyper-util, reqwest, tao, tauri, tauri-plugin-fs, tauri-runtime-wry, ureq, url, wry |
| `phf` | 0.13.1 | MIT | crates.io | Transitive: required by cssparser, selectors, tauri-utils, web_atoms |
| `phf_codegen` | 0.13.1 | MIT | crates.io | Transitive: required by selectors, web_atoms |
| `phf_generator` | 0.13.1 | MIT | crates.io | Transitive: required by phf_codegen, phf_macros, string_cache_codegen |
| `phf_macros` | 0.13.1 | MIT | crates.io | Transitive: required by phf |
| `phf_shared` | 0.13.1 | MIT | crates.io | Transitive: required by phf, phf_codegen, phf_generator, phf_macros, string_cache, string_cache_codegen |
| `pin-project-lite` | 0.2.17 | Apache-2.0 OR MIT | crates.io | Transitive: required by futures-util, gio, http-body-util, hyper, hyper-util, reqwest, tokio, tokio-util, tower, tower-http, tracing |
| `pkg-config` | 0.3.34 | MIT OR Apache-2.0 | crates.io | Transitive: required by gdk-sys, gdkwayland-sys, gtk, libdbus-sys, libsqlite3-sys, openssl-sys, system-deps, webkit2gtk-sys, x11, x11-dl |
| `plist` | 1.8.0 | MIT | crates.io | Transitive: required by tauri, tauri-codegen, tauri-plugin, tauri-utils |
| `png` | 0.17.16 | MIT OR Apache-2.0 | crates.io | Transitive: required by ico |
| `png` | 0.18.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by muda, tauri-codegen, tray-icon |
| `potential_utf` | 0.1.6 | Unicode-3.0 | crates.io | Transitive: required by icu_collections |
| `powerfmt` | 0.2.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by deranged, time |
| `precomputed-hash` | 0.1.1 | MIT | crates.io | Transitive: required by dom_query, selectors, string_cache |
| `proc-macro-crate` | 1.3.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by gtk3-macros |
| `proc-macro-crate` | 2.0.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by glib-macros |
| `proc-macro-crate` | 3.5.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by num_enum_derive |
| `proc-macro-error` | 1.0.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by glib-macros, gtk3-macros |
| `proc-macro-error-attr` | 1.0.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by proc-macro-error |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 | crates.io | Transitive: required by darling_core, derive_more-impl, displaydoc, dlopen2_derive, foreign-types-macros, futures-macro, glib-macros, gtk3-macros, num_enum_derive, openssl-macros, phf_macros, proc-macro-error, proc-macro-error-attr, quote, ref-cast-impl, schemars_derive, serde_derive, serde_derive_internals, serde_repr, serde_with_macros, serialize-to-javascript-impl, string_cache_codegen, syn, synstructure, tao-macros, tauri-codegen, tauri-macros, tauri-utils, thiserror-impl, wasm-bindgen-macro-support, webview2-com-macros, windows-implement, windows-interface, yoke-derive, zerofrom-derive, zerovec-derive |
| `quick-xml` | 0.38.4 | MIT | crates.io | Transitive: required by plist |
| `quote` | 1.0.47 | MIT OR Apache-2.0 | crates.io | Transitive: required by cssparser-macros, darling_core, darling_macro, derive_more-impl, displaydoc, dlopen2_derive, foreign-types-macros, futures-macro, glib-macros, gtk3-macros, jni-sys-macros, num_enum_derive, openssl-macros, phf_macros, proc-macro-error, proc-macro-error-attr, ref-cast-impl, schemars_derive, serde_derive, serde_derive_internals, serde_repr, serde_with_macros, serialize-to-javascript-impl, string_cache_codegen, syn, synstructure, tao-macros, tauri-codegen, tauri-macros, tauri-utils, thiserror-impl, wasm-bindgen-macro, wasm-bindgen-macro-support, webview2-com-macros, windows-implement, windows-interface, yoke-derive, zerofrom-derive, zerovec-derive |
| `r-efi` | 5.3.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | crates.io | Transitive: required by getrandom |
| `r-efi` | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | crates.io | Transitive: required by getrandom |
| `raw-window-handle` | 0.6.2 | MIT OR Apache-2.0 OR Zlib | crates.io | Transitive: required by ndk, rfd, softbuffer, tao, tauri, tauri-plugin-dialog, tauri-runtime, tauri-runtime-wry, window-vibrancy, wry |
| `redox_syscall` | 0.5.18 | MIT | crates.io | Transitive: required by parking_lot_core, softbuffer |
| `redox_users` | 0.5.3 | MIT | crates.io | Transitive: required by dirs-sys |
| `ref-cast` | 1.0.27 | MIT OR Apache-2.0 | crates.io | Transitive: required by schemars |
| `ref-cast-impl` | 1.0.27 | MIT OR Apache-2.0 | crates.io | Transitive: required by ref-cast |
| `regex` | 1.13.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri-utils, urlpattern |
| `regex-automata` | 0.4.18 | MIT OR Apache-2.0 | crates.io | Transitive: required by regex |
| `regex-syntax` | 0.8.11 | MIT OR Apache-2.0 | crates.io | Transitive: required by regex, regex-automata |
| `reqwest` | 0.13.5 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri |
| `rfd` | 0.16.0 | MIT | crates.io | Transitive: required by tauri-plugin-dialog |
| `rusqlite` | 0.37.0 | MIT | crates.io | Transitive: required by cm-store |
| `rustc-hash` | 2.1.3 | Apache-2.0 OR MIT | crates.io | Transitive: required by selectors |
| `rustc_version` | 0.4.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by derive_more-impl, embed-resource, field-offset |
| `rustix` | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | crates.io | Transitive: required by tempfile |
| `rustls-pki-types` | 1.15.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by ureq |
| `rustversion` | 1.0.23 | MIT OR Apache-2.0 | crates.io | Transitive: required by num_enum, wasm-bindgen |
| `same-file` | 1.0.6 | Unlicense/MIT | crates.io | Transitive: required by walkdir |
| `schannel` | 0.1.29 | MIT | crates.io | Transitive: required by native-tls |
| `schemars` | 0.8.22 | MIT | crates.io | Transitive: required by tauri-build, tauri-plugin, tauri-plugin-fs, tauri-utils |
| `schemars` | 0.9.0 | MIT | crates.io | Transitive: required by serde_with |
| `schemars` | 1.2.3 | MIT | crates.io | Transitive: required by serde_with |
| `schemars_derive` | 0.8.22 | MIT | crates.io | Transitive: required by schemars |
| `scopeguard` | 1.2.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by lock_api |
| `security-framework` | 3.7.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by native-tls |
| `security-framework-sys` | 2.17.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by native-tls, security-framework |
| `selectors` | 0.38.0 | MPL-2.0 | crates.io | Transitive: required by dom_query |
| `semver` | 1.0.28 | MIT OR Apache-2.0 | crates.io | Transitive: required by cargo_metadata, cargo_toml, rustc_version, tauri-build, tauri-codegen, tauri-utils |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | crates.io | Typed (de)serialization of IPC payloads |
| `serde-untagged` | 0.1.9 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri-utils |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 | crates.io | Transitive: required by bitflags, camino, deranged, erased-serde, indexmap, semver, serde, serde-untagged, serde_json, serde_spanned, serde_with, time, toml, toml_datetime, uuid |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde, serde_core, url |
| `serde_derive_internals` | 0.29.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by schemars_derive |
| `serde_json` | 1.0.154 | MIT OR Apache-2.0 | crates.io | JSON values passed between the core and the webview |
| `serde_repr` | 0.1.21 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri, tauri-plugin-fs |
| `serde_spanned` | 0.6.9 | MIT OR Apache-2.0 | crates.io | Transitive: required by toml, toml_edit |
| `serde_spanned` | 1.1.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by toml |
| `serde_with` | 3.17.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri-utils |
| `serde_with_macros` | 3.17.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by serde_with |
| `serialize-to-javascript` | 0.1.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri |
| `serialize-to-javascript-impl` | 0.1.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by serialize-to-javascript |
| `servo_arc` | 0.4.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by selectors |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | crates.io | Transitive: required by cm-core, cm-store, tauri-codegen, wry |
| `shlex` | 2.0.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by cc |
| `simd-adler32` | 0.3.10 | MIT | crates.io | Transitive: required by fdeflate, miniz_oxide |
| `siphasher` | 1.0.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by phf_shared |
| `slab` | 0.4.12 | MIT | crates.io | Transitive: required by futures-util |
| `smallvec` | 1.16.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by cfg-expr, cssparser, gio, glib, hyper, icu_normalizer, idna, parking_lot_core, rusqlite, selectors |
| `socket2` | 0.6.5 | MIT OR Apache-2.0 | crates.io | Transitive: required by hyper-util, tokio |
| `softbuffer` | 0.4.8 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri-runtime-wry |
| `soup3` | 0.5.0 | MIT | crates.io | Transitive: required by webkit2gtk, wry |
| `soup3-sys` | 0.5.0 | MIT | crates.io | Transitive: required by soup3, webkit2gtk-sys |
| `stable_deref_trait` | 1.2.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by servo_arc, yoke |
| `static_vcruntime` | 3.0.0 | MIT OR Apache-2.0 OR Zlib | crates.io | Transitive: required by tauri-build |
| `string_cache` | 0.9.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by web_atoms |
| `string_cache_codegen` | 0.6.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by web_atoms |
| `strsim` | 0.11.1 | MIT | crates.io | Transitive: required by darling_core, tauri-build |
| `swift-rs` | 1.0.9 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri, tauri-utils |
| `syn` | 1.0.109 | MIT OR Apache-2.0 | crates.io | Transitive: required by proc-macro-error |
| `syn` | 2.0.119 | MIT OR Apache-2.0 | crates.io | Transitive: required by darling_core, darling_macro, derive_more-impl, dlopen2_derive, glib-macros, gtk3-macros, jni-sys-macros, num_enum_derive, openssl-macros, phf_macros, schemars_derive, serde_derive_internals, serde_with_macros, serialize-to-javascript-impl, tao-macros, tauri-codegen, tauri-macros, thiserror-impl, webview2-com-macros, windows-implement, windows-interface |
| `syn` | 3.0.9 | MIT OR Apache-2.0 | crates.io | Transitive: required by cssparser-macros, displaydoc, foreign-types-macros, futures-macro, ref-cast-impl, serde_derive, serde_repr, synstructure, thiserror-impl, wasm-bindgen-macro-support, yoke-derive, zerofrom-derive, zerovec-derive |
| `sync_wrapper` | 1.0.2 | Apache-2.0 | crates.io | Transitive: required by reqwest, tower |
| `synstructure` | 0.14.0 | MIT | crates.io | Transitive: required by yoke-derive, zerofrom-derive |
| `system-deps` | 6.2.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by atk-sys, cairo-sys-rs, gdk-pixbuf-sys, gdk-sys, gdkwayland-sys, gdkx11-sys, gio-sys, glib-sys, gobject-sys, gtk-sys, javascriptcore-rs-sys, pango-sys, soup3-sys, webkit2gtk-sys |
| `tao` | 0.37.1 | Apache-2.0 | crates.io | Transitive: required by tauri-runtime-wry |
| `tao-macros` | 0.1.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by tao, wry |
| `target-lexicon` | 0.12.16 | Apache-2.0 WITH LLVM-exception | crates.io | Transitive: required by cfg-expr |
| `tauri` | 2.12.4 | Apache-2.0 OR MIT | crates.io | Desktop shell: native window, IPC commands to the shared Rust core, capability-scoped permissions |
| `tauri-build` | 2.7.3 | Apache-2.0 OR MIT | crates.io | Build-time generation of the Tauri context, permissions and Windows resources |
| `tauri-codegen` | 2.7.3 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri-macros |
| `tauri-macros` | 2.7.3 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri |
| `tauri-plugin` | 2.7.2 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri-plugin-dialog, tauri-plugin-fs |
| `tauri-plugin-dialog` | 2.8.3 | Apache-2.0 OR MIT | crates.io | Native folder picker for choosing scan scopes (no file contents read) |
| `tauri-plugin-fs` | 2.7.1 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri-plugin-dialog |
| `tauri-runtime` | 2.12.2 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri, tauri-runtime-wry |
| `tauri-runtime-wry` | 2.12.2 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri |
| `tauri-utils` | 2.10.2 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri, tauri-build, tauri-codegen, tauri-macros, tauri-plugin, tauri-plugin-fs, tauri-runtime, tauri-runtime-wry |
| `tauri-winres` | 0.3.6 | MIT | crates.io | Transitive: required by tauri-build |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by native-tls |
| `tendril` | 0.5.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by dom_query, markup5ever |
| `thiserror` | 1.0.69 | MIT OR Apache-2.0 | crates.io | Transitive: required by cairo-rs, gio, glib, jni, ndk |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | crates.io | Transitive: required by cargo_metadata, cm-app, cm-core, cm-store, json-patch, muda, redox_users, tauri, tauri-codegen, tauri-plugin-dialog, tauri-plugin-fs, tauri-runtime, tauri-utils, tray-icon, webview2-com-sys, wry |
| `thiserror-impl` | 1.0.69 | MIT OR Apache-2.0 | crates.io | Transitive: required by thiserror |
| `thiserror-impl` | 2.0.21 | MIT OR Apache-2.0 | crates.io | Transitive: required by thiserror |
| `time` | 0.3.45 | MIT OR Apache-2.0 | crates.io | Transitive: required by cookie, plist, serde_with, tauri-codegen |
| `time-core` | 0.1.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by time, time-macros |
| `time-macros` | 0.2.25 | MIT OR Apache-2.0 | crates.io | Transitive: required by time |
| `tinystr` | 0.8.4 | Unicode-3.0 | crates.io | Transitive: required by icu_locale_core |
| `tokio` | 1.53.3 | MIT | crates.io | Transitive: required by hyper, hyper-util, reqwest, tauri, tokio-util, tower, wasm-bindgen-futures |
| `tokio-util` | 0.7.20 | MIT | crates.io | Transitive: required by reqwest |
| `toml` | 0.8.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by system-deps |
| `toml` | 1.1.8+spec-1.1.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by cargo_toml, embed-resource, tauri-plugin-fs, tauri-utils, tauri-winres |
| `toml_datetime` | 0.6.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by proc-macro-crate, toml, toml_edit |
| `toml_datetime` | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by toml, toml_edit |
| `toml_edit` | 0.19.15 | MIT OR Apache-2.0 | crates.io | Transitive: required by proc-macro-crate |
| `toml_edit` | 0.20.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by proc-macro-crate, toml |
| `toml_edit` | 0.25.17+spec-1.1.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by proc-macro-crate |
| `toml_parser` | 1.1.5+spec-1.1.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by toml, toml_edit |
| `toml_writer` | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by toml |
| `tower` | 0.5.3 | MIT | crates.io | Transitive: required by reqwest, tower-http |
| `tower-http` | 0.6.11 | MIT | crates.io | Transitive: required by reqwest |
| `tower-layer` | 0.3.3 | MIT | crates.io | Transitive: required by tower, tower-http |
| `tower-service` | 0.3.3 | MIT | crates.io | Transitive: required by hyper-util, reqwest, tower, tower-http |
| `tracing` | 0.1.44 | MIT | crates.io | Transitive: required by hyper-util, softbuffer |
| `tracing-core` | 0.1.36 | MIT | crates.io | Transitive: required by tracing |
| `tray-icon` | 0.25.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by tauri |
| `try-lock` | 0.2.5 | MIT | crates.io | Transitive: required by want |
| `typeid` | 1.0.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by erased-serde, serde-untagged |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by crypto-common, generic-array |
| `unicode-ident` | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | crates.io | Transitive: required by proc-macro2, syn, wasm-bindgen-shared |
| `unicode-segmentation` | 1.13.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by tao |
| `ureq` | 3.4.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by cm-app |
| `ureq-proto` | 0.6.4 | MIT OR Apache-2.0 | crates.io | Transitive: required by ureq |
| `url` | 2.5.8 | MIT OR Apache-2.0 | crates.io | Transitive: required by reqwest, schemars, tao, tauri, tauri-codegen, tauri-plugin-dialog, tauri-plugin-fs, tauri-runtime, tauri-runtime-wry, tauri-utils, tower-http, urlpattern, wry |
| `urlpattern` | 0.6.0 | MIT | crates.io | Transitive: required by tauri-utils |
| `utf8-zero` | 0.8.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by ureq |
| `utf8_iter` | 1.0.4 | Apache-2.0 OR MIT | crates.io | Transitive: required by idna |
| `uuid` | 1.26.1 | Apache-2.0 OR MIT | crates.io | Transitive: required by cfb, schemars, tauri-codegen, tauri-utils |
| `vcpkg` | 0.2.15 | MIT/Apache-2.0 | crates.io | Transitive: required by libsqlite3-sys, openssl-sys |
| `version-compare` | 0.2.1 | MIT | crates.io | Transitive: required by system-deps |
| `version_check` | 0.9.5 | MIT/Apache-2.0 | crates.io | Transitive: required by cookie, generic-array, proc-macro-error, proc-macro-error-attr |
| `vswhom` | 0.1.0 | MIT | crates.io | Transitive: required by embed-resource |
| `vswhom-sys` | 0.1.3 | MIT | crates.io | Transitive: required by vswhom |
| `walkdir` | 2.5.0 | Unlicense/MIT | crates.io | Transitive: required by jni, tauri-build, tauri-codegen, tauri-plugin, tauri-utils |
| `want` | 0.3.2 | MIT | crates.io | Transitive: required by hyper |
| `wasi` | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | crates.io | Transitive: required by mio |
| `wasip2` | 1.0.1+wasi-0.2.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | crates.io | Transitive: required by getrandom |
| `wasm-bindgen` | 0.2.129 | MIT OR Apache-2.0 | crates.io | Transitive: required by iana-time-zone, js-sys, reqwest, rfd, softbuffer, uuid, wasm-bindgen-futures, wasm-streams, web-sys, web-time |
| `wasm-bindgen-futures` | 0.4.79 | MIT OR Apache-2.0 | crates.io | Transitive: required by reqwest, rfd, wasm-streams |
| `wasm-bindgen-macro` | 0.2.129 | MIT OR Apache-2.0 | crates.io | Transitive: required by wasm-bindgen |
| `wasm-bindgen-macro-support` | 0.2.129 | MIT OR Apache-2.0 | crates.io | Transitive: required by wasm-bindgen-macro |
| `wasm-bindgen-shared` | 0.2.129 | MIT OR Apache-2.0 | crates.io | Transitive: required by wasm-bindgen, wasm-bindgen-macro-support |
| `wasm-streams` | 0.5.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by reqwest |
| `web-sys` | 0.3.106 | MIT OR Apache-2.0 | crates.io | Transitive: required by reqwest, rfd, softbuffer, wasm-streams |
| `web-time` | 1.1.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by cfb |
| `web_atoms` | 0.2.6 | MIT OR Apache-2.0 | crates.io | Transitive: required by markup5ever |
| `webkit2gtk` | 2.0.2 | MIT | crates.io | Transitive: required by tauri, tauri-runtime, tauri-runtime-wry, wry |
| `webkit2gtk-sys` | 2.0.2 | MIT | crates.io | Transitive: required by webkit2gtk, wry |
| `webview2-com` | 0.39.1 | MIT | crates.io | Transitive: required by tauri, tauri-runtime, tauri-runtime-wry, wry |
| `webview2-com-macros` | 0.8.1 | MIT | crates.io | Transitive: required by webview2-com |
| `webview2-com-sys` | 0.39.1 | MIT | crates.io | Transitive: required by webview2-com |
| `winapi` | 0.3.9 | MIT/Apache-2.0 | crates.io | Transitive: required by dlopen2, gio-sys, libloading |
| `winapi-i686-pc-windows-gnu` | 0.4.0 | MIT/Apache-2.0 | crates.io | Transitive: required by winapi |
| `winapi-util` | 0.1.11 | Unlicense OR MIT | crates.io | Transitive: required by same-file, walkdir |
| `winapi-x86_64-pc-windows-gnu` | 0.4.0 | MIT/Apache-2.0 | crates.io | Transitive: required by winapi |
| `window-vibrancy` | 0.8.1 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri |
| `windows` | 0.62.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by cm-platform, tao, tauri, tauri-runtime, tauri-runtime-wry, webview2-com, webview2-com-sys, wry |
| `windows-collections` | 0.3.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows |
| `windows-core` | 0.62.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by cm-platform, iana-time-zone, tao, webview2-com, webview2-com-sys, windows, windows-collections, windows-future, windows-numerics, wry |
| `windows-future` | 0.3.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows |
| `windows-implement` | 0.60.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-core |
| `windows-interface` | 0.59.3 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-core |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by chrono, parking_lot_core, windows-core, windows-future, windows-numerics, windows-result, windows-strings, windows-sys, windows-targets, windows-threading, windows-version |
| `windows-numerics` | 0.3.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows |
| `windows-result` | 0.4.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-core |
| `windows-strings` | 0.5.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-core |
| `windows-sys` | 0.45.0 | MIT OR Apache-2.0 | crates.io | Transitive: required by jni |
| `windows-sys` | 0.60.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by rfd |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by cm-platform, dbus, dirs-sys, errno, mio, muda, rustix, schannel, socket2, softbuffer, tempfile, tokio, tray-icon, winapi-util, window-vibrancy, winreg |
| `windows-targets` | 0.42.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-sys |
| `windows-targets` | 0.53.5 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-sys |
| `windows-threading` | 0.2.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-future |
| `windows-version` | 0.1.7 | MIT OR Apache-2.0 | crates.io | Transitive: required by tao, window-vibrancy, wry |
| `windows_aarch64_gnullvm` | 0.42.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_aarch64_gnullvm` | 0.53.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_aarch64_msvc` | 0.42.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_aarch64_msvc` | 0.53.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_i686_gnu` | 0.42.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_i686_gnu` | 0.53.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_i686_gnullvm` | 0.53.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_i686_msvc` | 0.42.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_i686_msvc` | 0.53.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_x86_64_gnu` | 0.42.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_x86_64_gnu` | 0.53.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_x86_64_gnullvm` | 0.42.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_x86_64_gnullvm` | 0.53.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_x86_64_msvc` | 0.42.2 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `windows_x86_64_msvc` | 0.53.1 | MIT OR Apache-2.0 | crates.io | Transitive: required by windows-targets |
| `winnow` | 0.5.40 | MIT | crates.io | Transitive: required by toml_edit |
| `winnow` | 1.0.4 | MIT | crates.io | Transitive: required by toml, toml_edit, toml_parser |
| `winreg` | 0.56.0 | MIT | crates.io | Transitive: required by embed-resource |
| `wit-bindgen` | 0.46.0 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | crates.io | Transitive: required by wasip2 |
| `writeable` | 0.6.4 | Unicode-3.0 | crates.io | Transitive: required by icu_locale_core, icu_provider |
| `wry` | 0.57.0 | Apache-2.0 OR MIT | crates.io | Transitive: required by tauri-runtime-wry |
| `x11` | 2.21.0 | MIT | crates.io | Transitive: required by gdkx11, gdkx11-sys |
| `x11-dl` | 2.21.0 | MIT | crates.io | Transitive: required by tao, wry |
| `yoke` | 0.8.3 | Unicode-3.0 | crates.io | Transitive: required by icu_collections, icu_provider, zerotrie, zerovec |
| `yoke-derive` | 0.8.4 | Unicode-3.0 | crates.io | Transitive: required by yoke |
| `zerofrom` | 0.1.8 | Unicode-3.0 | crates.io | Transitive: required by icu_collections, icu_provider, yoke, zerotrie, zerovec |
| `zerofrom-derive` | 0.1.8 | Unicode-3.0 | crates.io | Transitive: required by zerofrom |
| `zeroize` | 1.9.1 | Apache-2.0 OR MIT | crates.io | Transitive: required by der, rustls-pki-types |
| `zerotrie` | 0.2.5 | Unicode-3.0 | crates.io | Transitive: required by icu_properties, icu_provider |
| `zerovec` | 0.11.8 | Unicode-3.0 | crates.io | Transitive: required by icu_collections, icu_locale_core, icu_normalizer, icu_properties, icu_provider, potential_utf, tinystr |
| `zerovec-derive` | 0.11.6 | Unicode-3.0 | crates.io | Transitive: required by zerovec |
| `zlib-rs` | 0.6.8 | Zlib | crates.io | Transitive: required by flate2 |
| `zmij` | 1.0.23 | MIT | crates.io | Transitive: required by serde_json |

## npm 包（97）

| 包 | 版本 | 许可 | 范围 | 用途 |
| --- | --- | --- | --- | --- |
| @esbuild/aix-ppc64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/android-arm | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/android-arm64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/android-x64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/darwin-arm64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/darwin-x64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/freebsd-arm64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/freebsd-x64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/linux-arm | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/linux-arm64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/linux-ia32 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/linux-loong64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/linux-mips64el | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/linux-ppc64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/linux-riscv64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/linux-s390x | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/linux-x64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/netbsd-arm64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/netbsd-x64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/openbsd-arm64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/openbsd-x64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/openharmony-arm64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/sunos-x64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/win32-arm64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/win32-ia32 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @esbuild/win32-x64 | 0.28.2 | MIT | dev (optional, platform-specific) | Transitive |
| @jridgewell/resolve-uri | 3.1.2 | MIT | dev | Transitive |
| @jridgewell/sourcemap-codec | 1.6.0 | MIT | dev | Transitive |
| @jridgewell/trace-mapping | 0.3.31 | MIT | dev | Transitive |
| @napi-rs/lzma-linux-x64-gnu | 1.5.1 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-android-arm-eabi | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-android-arm64 | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-darwin-arm64 | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-darwin-x64 | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-freebsd-arm64 | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-freebsd-x64 | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-arm-gnueabihf | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-arm-musleabihf | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-arm64-gnu | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-arm64-musl | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-loong64-gnu | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-loong64-musl | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-ppc64-gnu | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-ppc64-musl | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-riscv64-gnu | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-riscv64-musl | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-s390x-gnu | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-x64-gnu | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-linux-x64-musl | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-openbsd-x64 | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-openharmony-arm64 | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-win32-arm64-msvc | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-win32-ia32-msvc | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-win32-x64-gnu | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @rollup/rollup-win32-x64-msvc | 4.64.7 | MIT | dev (optional, platform-specific) | Transitive |
| @tauri-apps/api | 2.12.2 | Apache-2.0 OR MIT | runtime | Webview side of the Tauri IPC bridge |
| @tauri-apps/cli | 2.12.2 | Apache-2.0 OR MIT | dev | Development/build CLI for the Tauri app (not shipped) |
| @tauri-apps/cli-darwin-arm64 | 2.12.2 | Apache-2.0 OR MIT | dev (optional, platform-specific) | Transitive |
| @tauri-apps/cli-darwin-x64 | 2.12.2 | Apache-2.0 OR MIT | dev (optional, platform-specific) | Transitive |
| @tauri-apps/cli-linux-arm-gnueabihf | 2.12.2 | Apache-2.0 OR MIT | dev (optional, platform-specific) | Transitive |
| @tauri-apps/cli-linux-arm64-gnu | 2.12.2 | Apache-2.0 OR MIT | dev (optional, platform-specific) | Transitive |
| @tauri-apps/cli-linux-arm64-musl | 2.12.2 | Apache-2.0 OR MIT | dev (optional, platform-specific) | Transitive |
| @tauri-apps/cli-linux-riscv64-gnu | 2.12.2 | Apache-2.0 OR MIT | dev (optional, platform-specific) | Transitive |
| @tauri-apps/cli-linux-x64-gnu | 2.12.2 | Apache-2.0 OR MIT | dev (optional, platform-specific) | Transitive |
| @tauri-apps/cli-linux-x64-musl | 2.12.2 | Apache-2.0 OR MIT | dev (optional, platform-specific) | Transitive |
| @tauri-apps/cli-win32-arm64-msvc | 2.12.2 | Apache-2.0 OR MIT | dev (optional, platform-specific) | Transitive |
| @tauri-apps/cli-win32-ia32-msvc | 2.12.2 | Apache-2.0 OR MIT | dev (optional, platform-specific) | Transitive |
| @tauri-apps/cli-win32-x64-msvc | 2.12.2 | Apache-2.0 OR MIT | dev (optional, platform-specific) | Transitive |
| @types/chai | 5.2.3 | MIT | dev | Transitive |
| @types/deep-eql | 4.0.2 | MIT | dev | Transitive |
| @types/estree | 1.0.9 | MIT | dev | Transitive |
| @vitest/mocker | 5.0.3 | MIT | dev | Transitive |
| @vitest/spy | 5.0.3 | MIT | dev | Transitive |
| assertion-error | 2.0.1 | MIT | dev | Transitive |
| chai | 6.3.0 | MIT | dev | Transitive |
| es-module-lexer | 2.3.2 | MIT | dev | Transitive |
| esbuild | 0.28.2 | MIT | dev | Transitive |
| estree-walker | 3.0.3 | MIT | dev | Transitive |
| expect-type | 1.4.0 | Apache-2.0 | dev | Transitive |
| fdir | 6.5.0 | MIT | dev | Transitive |
| fsevents | 2.3.3 | MIT | dev (optional, platform-specific) | Transitive |
| magic-string | 1.4.3 | MIT | dev | Transitive |
| nanoid | 3.3.20 | MIT | dev | Transitive |
| obug | 2.2.1 | MIT | dev | Transitive |
| picocolors | 1.1.1 | ISC | dev | Transitive |
| picomatch | 4.0.7 | MIT | dev | Transitive |
| postcss | 8.5.30 | MIT | dev | Transitive |
| rollup | 4.64.7 | MIT | dev | Transitive |
| source-map-js | 1.2.2 | BSD-3-Clause | dev | Transitive |
| std-env | 4.3.0 | MIT | dev | Transitive |
| tinybench | 6.2.2 | MIT | dev | Transitive |
| tinyexec | 1.3.1 | MIT | dev | Transitive |
| tinyglobby | 0.2.17 | MIT | dev | Transitive |
| typescript | 6.0.3 | Apache-2.0 | dev | Type checking of the webview sources (not shipped) |
| vite | 7.3.7 | MIT | dev | Bundler for the webview assets (not shipped) |
| vitest | 5.0.3 | MIT | dev | Unit tests for the webview logic (not shipped) |
| why-is-node-running | 3.2.1 | MIT | dev | Transitive |
