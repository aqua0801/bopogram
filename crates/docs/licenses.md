# Rust 相依套件授權清單（`crates/`）

由 `cargo metadata` 產生（只算 Windows x64／x86 與 Linux musl 的一般與建置相依，不含只在測試用的）；
`cargo deny check licenses`（`crates/deny.toml`）檢查全部都在允許清單內：MIT、Apache-2.0、
Apache-2.0 WITH LLVM-exception、Unicode-3.0、Unlicense。表示式中有 OR 的取其中允許的一項。

共 82 個套件。

| 套件 | 版本 | 授權 | 平台 | 來源 |
|---|---|---|---|---|
| async-broadcast | 0.7.2 | MIT OR Apache-2.0 | Linux | https://github.com/smol-rs/async-broadcast |
| async-channel | 2.5.0 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/async-channel |
| async-executor | 1.14.1 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/async-executor |
| async-io | 2.6.0 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/async-io |
| async-lock | 3.5.0 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/async-lock |
| async-process | 2.5.0 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/async-process |
| async-recursion | 1.2.0 | MIT OR Apache-2.0 | Linux | https://github.com/dcchut/async-recursion |
| async-signal | 0.2.14 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/async-signal |
| async-task | 4.7.1 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/async-task |
| async-trait | 0.1.92 | MIT OR Apache-2.0 | Linux | https://github.com/dtolnay/async-trait |
| atomic-waker | 1.1.2 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/atomic-waker |
| autocfg | 1.5.1 | Apache-2.0 OR MIT | Linux | https://github.com/cuviper/autocfg |
| bitflags | 2.13.2 | MIT OR Apache-2.0 | Linux | https://github.com/bitflags/bitflags |
| blocking | 1.7.0 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/blocking |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | Linux | https://github.com/rust-lang/cfg-if |
| concurrent-queue | 2.5.0 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/concurrent-queue |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 | Linux | https://github.com/crossbeam-rs/crossbeam |
| endi | 1.1.1 | MIT | Linux | https://github.com/zeenix/endi |
| enumflags2 | 0.7.12 | MIT OR Apache-2.0 | Linux | https://github.com/meithecatte/enumflags2 |
| enumflags2_derive | 0.7.12 | MIT OR Apache-2.0 | Linux | https://github.com/meithecatte/enumflags2 |
| equivalent | 1.0.3 | Apache-2.0 OR MIT | Linux | https://github.com/indexmap-rs/equivalent |
| errno | 0.3.15 | MIT OR Apache-2.0 | Linux | https://github.com/lambda-fairy/rust-errno |
| event-listener | 5.4.2 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/event-listener |
| event-listener-strategy | 0.5.4 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/event-listener-strategy |
| fastrand | 2.5.0 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/fastrand |
| futures-core | 0.3.34 | MIT OR Apache-2.0 | Linux | https://github.com/rust-lang/futures-rs |
| futures-io | 0.3.34 | MIT OR Apache-2.0 | Linux | https://github.com/rust-lang/futures-rs |
| futures-lite | 2.6.1 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/futures-lite |
| hashbrown | 0.17.2 | MIT OR Apache-2.0 | Linux | https://github.com/rust-lang/hashbrown |
| hex | 0.4.3 | MIT OR Apache-2.0 | Linux | https://github.com/KokaKiwi/rust-hex |
| indexmap | 2.14.2 | Apache-2.0 OR MIT | Linux | https://github.com/indexmap-rs/indexmap |
| itoa | 1.0.19 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/dtolnay/itoa |
| libc | 0.2.191 | MIT OR Apache-2.0 | Linux | https://github.com/rust-lang/libc |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Linux | https://github.com/sunfishcode/linux-raw-sys |
| memchr | 2.8.3 | Unlicense OR MIT | Windows、Linux | https://github.com/BurntSushi/memchr |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | Linux | https://github.com/matklad/once_cell |
| ordered-stream | 0.2.0 | MIT OR Apache-2.0 | Linux | https://github.com/danieldg/ordered-stream |
| parking | 2.2.1 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/parking |
| pin-project-lite | 0.2.18 | Apache-2.0 OR MIT | Linux | https://github.com/taiki-e/pin-project-lite |
| piper | 0.2.5 | MIT OR Apache-2.0 | Linux | https://github.com/smol-rs/piper |
| polling | 3.11.0 | Apache-2.0 OR MIT | Linux | https://github.com/smol-rs/polling |
| proc-macro-crate | 3.5.0 | MIT OR Apache-2.0 | Linux | https://github.com/bkchr/proc-macro-crate |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/dtolnay/proc-macro2 |
| quote | 1.0.47 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/dtolnay/quote |
| rustix | 1.1.6 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | Linux | https://github.com/bytecodealliance/rustix |
| serde | 1.0.229 | MIT OR Apache-2.0 | Linux | https://github.com/serde-rs/serde |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/serde-rs/serde |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 | Linux | https://github.com/serde-rs/serde |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/serde-rs/json |
| serde_repr | 0.1.21 | MIT OR Apache-2.0 | Linux | https://github.com/dtolnay/serde-repr |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 | Linux | https://github.com/vorner/signal-hook |
| slab | 0.4.12 | MIT | Linux | https://github.com/tokio-rs/slab |
| syn | 2.0.119 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/dtolnay/syn |
| syn | 3.0.10 | MIT OR Apache-2.0 | Linux | https://github.com/dtolnay/syn |
| toml_datetime | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | Linux | https://github.com/toml-rs/toml |
| toml_edit | 0.25.21+spec-1.1.0 | MIT OR Apache-2.0 | Linux | https://github.com/toml-rs/toml |
| toml_parser | 1.1.7+spec-1.1.0 | MIT OR Apache-2.0 | Linux | https://github.com/toml-rs/toml |
| tracing | 0.1.44 | MIT | Linux | https://github.com/tokio-rs/tracing |
| tracing-attributes | 0.1.31 | MIT | Linux | https://github.com/tokio-rs/tracing |
| tracing-core | 0.1.36 | MIT | Linux | https://github.com/tokio-rs/tracing |
| unicode-ident | 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | Windows、Linux | https://github.com/dtolnay/unicode-ident |
| uuid | 1.29.3 | Apache-2.0 OR MIT | Linux | https://github.com/uuid-rs/uuid |
| windows | 0.62.2 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/microsoft/windows-rs |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/microsoft/windows-rs |
| windows-core | 0.62.2 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/microsoft/windows-rs |
| windows-future | 0.3.2 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/microsoft/windows-rs |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/microsoft/windows-rs |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/microsoft/windows-rs |
| windows-link | 0.2.1 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/microsoft/windows-rs |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/microsoft/windows-rs |
| windows-result | 0.4.1 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/microsoft/windows-rs |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/microsoft/windows-rs |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 | Windows、Linux | https://github.com/microsoft/windows-rs |
| winnow | 1.0.5 | MIT | Linux | https://github.com/winnow-rs/winnow |
| zbus | 5.19.0 | MIT | Linux | https://github.com/z-galaxy/zbus/ |
| zbus_macros | 5.19.0 | MIT | Linux | https://github.com/z-galaxy/zbus/ |
| zbus_names | 4.3.5 | MIT | Linux | https://github.com/z-galaxy/zbus/ |
| zcheapstr | 1.4.0 | MIT | Linux | https://github.com/z-galaxy/zcheapstr/ |
| zmij | 1.0.23 | MIT | Windows、Linux | https://github.com/dtolnay/zmij |
| zvariant | 5.15.1 | MIT | Linux | https://github.com/z-galaxy/zbus/ |
| zvariant_derive | 5.15.1 | MIT | Linux | https://github.com/z-galaxy/zbus/ |
| zvariant_utils | 4.2.0 | MIT | Linux | https://github.com/z-galaxy/zbus/ |
