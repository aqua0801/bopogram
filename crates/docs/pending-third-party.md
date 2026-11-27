# 待登記於 THIRD_PARTY.md 的項目

發布功能（`prototype/docs/release-spec.md`）全部完成後，一次交給 WSL 端登記（2026-11-27 使用者決定）。
`crates/docs/licenses.md` 由 `cargo metadata` 重新產生，`cargo deny check licenses` 必須通過。

| 名稱 | 來源網址 | 授權 | 用途 | 加入時間 |
|---|---|---|---|---|
| calamine | https://github.com/tafia/calamine | MIT | `bopogram build-dict`：讀教育部辭典的 .xlsx | 2026-11-27 |
| zip | https://github.com/zip-rs/zip2 | MIT | `bopogram build-dict`：直接讀下載的 .zip；模型包解壓 | 2026-11-27 |
| sha2 | https://github.com/RustCrypto/hashes | MIT OR Apache-2.0 | 模型包：檢查每個檔案的 SHA-256 | 2026-11-27 |
| Inno Setup | https://jrsoftware.org/isinfo.php | Inno Setup License（寬鬆授權） | Windows 安裝檔的製作工具（不隨安裝檔散布程式碼，只用來產生安裝檔） | 2026-11-27 |

calamine 與 zip 另外帶進的授權（已加入 `crates/deny.toml` 的允許清單）：
encoding_rs 的 `(Apache-2.0 OR MIT) AND BSD-3-Clause`（BSD-3-Clause 是它附的 WHATWG 編碼資料），
zlib-rs 的 `Zlib`。其餘相依套件為 MIT 或 MIT／Apache-2.0，完整清單見 `licenses.md`。
