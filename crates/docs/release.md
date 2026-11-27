# 發布：安裝檔與模型包（Windows 端，2026-11-27）

規格：`prototype/docs/release-spec.md`。這裡記 Windows 端做的部分與怎麼產生。

## 產生

| 產物 | 指令 | 產出（預設 `D:\bopogram-data\dist\`） |
|---|---|---|
| Windows 安裝檔 | `powershell -ExecutionPolicy Bypass -File crates\installer\build.ps1 [-Pack <模型包.zip>]` | `bopogram-setup-<版本>.exe`；有 `-Pack` 時為 `bopogram-setup-<版本>-full.exe` |
| Linux 安裝包 | 先在 Windows：`cargo build --release -p bopogram -p bopogram-ibus --target x86_64-unknown-linux-musl`（連結器 `rust-lld`，見 `linux.md`）與 `cargo about generate --workspace --offline --fail about.hbs -o target\THIRD-PARTY-LICENSES.html`；再在 WSL：`sh crates/bopogram-ibus/dist/make-packages.sh <輸出資料夾>` | `bopogram-<版本>-linux-x86_64.tar.gz`、`bopogram_<版本>_amd64.deb` |
| 模型包 | WSL 的 `prototype/tools/make_model_pack.py`（在 WSL 執行；Windows 上執行時 charlm 的路徑會寫成反斜線，見下） | `bopogram-models-<版本>.zip` |

`build.ps1` 會依序：建置 64 與 32 位元、`cargo deny check licenses`、產生授權全文、執行 Inno Setup（`ISCC.exe`）。
不會在這台電腦安裝任何東西。

## Windows 安裝檔做的事（`installer/bopogram.iss`）

1. 請使用者的 host 結束（`bopogram-host.exe --stop`，它會先存好個人化資料）。
2. 程式放 `C:\Program Files\Bopogram\`（32 位元 DLL 在 `x86\`）。應用程式正在使用的檔案先改名（`*.old-<時間>`），
   重新開機時刪除，之後啟動的程式就用新版。清掉腳本安裝留下的 `uninstall.ps1` 等。
3. 註冊文字服務（regsvr32，64 與 32 位元；解除安裝時自動取消註冊）。
4. `bopogram-host.json`：`{"engine": "rust", "config": "%LOCALAPPDATA%\\Bopogram\\data\\models\\engine.json"}`，
   每個使用者各自的模型與資料（host 與設定程式會展開環境變數）。
5. 以登入的使用者身分（不是系統管理員）：
   - 舊的腳本安裝指向執行包時，把它的 `userdata\` 複製到新資料夾（`bopogram pack adopt-userdata`，只補沒有的檔案，不覆蓋）；
   - 完整安裝檔：安裝模型包（`bopogram pack install`，失敗時提示改用設定程式）；
   - 把鍵盤加入「中文（台灣）」（`keyboard.ps1 add`；語言清單沒有中文（台灣）時提示自行加入）；
   - 啟動 host。
6. 開始功能表「Bopogram 設定」；登入時啟動 host（`HKCU\...\Run`）。

解除安裝：結束 host、從所有語言移除鍵盤、取消註冊、刪除程式（使用中的檔案重新開機時刪除），
並詢問是否一併刪除 `%LOCALAPPDATA%\Bopogram`（模型、設定、個人化資料；預設保留）。

## Linux

- `.tar.gz`：`./install.sh [模型包.zip]`，程式放 `/usr/local/lib/bopogram/`（`/usr/local/bin/bopogram`）、
  IBus 元件檔放 `/usr/share/ibus/component/`（這兩步用 sudo），模型包以使用者身分安裝到 `~/.local/share/bopogram/`。
- `.deb`：程式在 `/usr/lib/bopogram/`、`/usr/bin/bopogram`，授權在 `/usr/share/doc/bopogram/`；模型包由每個使用者
  `bopogram pack install <zip>` 後 `ibus restart`。
- IBus 引擎優先讀 `~/.local/share/bopogram/models/engine.json`（模型包），沒有時讀 `config.json`（測試包）。

## 待確認

- Inno Setup 的繁體中文訊息檔（`compiler:Languages\ChineseTraditional.isl`）需在安裝 Inno Setup 後確認存在。
- Windows 安裝檔、設定程式的模型區塊、`bopogram-host.exe --stop` 待沙盒測試；`.deb` 待 VM 測試。
- 程式碼簽章另外決定（`release-spec.md`）。
