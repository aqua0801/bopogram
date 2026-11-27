# 發布：安裝檔與模型包（Windows 端）

規格：`prototype/docs/release-spec.md`。Windows 端的部分都已完成（2026-11-27）：`build-dict`、模型包、設定程式的模型區塊、
Rust 相依套件授權全文、Windows 安裝檔、Linux 的 `.tar.gz` 與 `.deb`。Windows 安裝檔在沙盒、`.deb` 在 Ubuntu 24.04 VM 測試通過。
程式碼簽章不做。

## 產生一次發布

```
powershell -ExecutionPolicy Bypass -File crates\installer\release.ps1 -ModelsVersion 2026.12
```

全部輸出到**一個資料夾** `D:\bopogram-data\release\bopogram-<程式版本>\`，上傳到 GitHub Releases 的就是這個資料夾裡的全部檔案：

| 檔案 | 內容 |
|---|---|
| `bopogram-setup-<版本>.exe` | Windows 安裝檔：只有程式（模型包在設定程式匯入） |
| `bopogram-setup-<版本>-full.exe` | Windows 安裝檔：程式與模型包（第一次安裝用） |
| `bopogram-<版本>-linux-x86_64.tar.gz` | Linux：`./install.sh [模型包.zip]` |
| `bopogram_<版本>_amd64.deb` | Debian／Ubuntu |
| `bopogram-models-<模型版本>.zip` | 模型包（WSL 的 `prototype/tools/make_model_pack.py`） |
| `bopogram-charlm-lite-<模型版本>.zip`、`bopogram-charlm-advanced-<模型版本>.zip` | charlm 額外等級（`installer/make_tier_pack.py`） |
| `SHA256SUMS.txt` | 以上每個檔案的 SHA-256 |

`release.ps1` 依序：

1. 用目前的打包工具產生模型包與兩個等級包，每個都用 `bopogram pack verify` 檢查（有一個不過就停止，不產生發布資料夾）。
2. Windows 安裝檔兩種（`build.ps1`：建置 64 與 32 位元、`cargo deny check licenses`、`cargo about` 產生授權全文、Inno Setup）。
3. Linux 程式（musl 交叉編譯）與兩種安裝包（在 WSL 執行 `bopogram-ibus/dist/make-packages.sh`）。
4. 把要上傳的檔案移到發布資料夾、寫 `SHA256SUMS.txt`，刪除中間檔。

不會在這台電腦安裝任何東西。輸入：執行包 `D:\bopogram-data\bopogram-runtime`（config、charlm、讀音機率表）、
`.bglm` 在 `D:\bopogram-data\arpa`、教育部原始檔與使用說明及 `dict_extra.tsv` 在 `D:\bopogram-data\dict-source`（皆可用參數改）。

單獨產生某一種：Windows 安裝檔 `installer\build.ps1 [-Pack <模型包>] -Out <資料夾>`；Linux 安裝包見上面第 3 步。

## Windows 安裝檔做的事（`installer/bopogram.iss`）

1. 請使用者的 host 結束（`bopogram-host.exe --stop`，它會先存好個人化資料）。
2. 程式放 `C:\Program Files\Bopogram\`（32 位元 DLL 在 `x86\`）。應用程式正在使用的檔案先改名（`*.old-<時間>`），
   重新開機時刪除，之後啟動的程式就用新版。清掉腳本安裝留下的 `uninstall.ps1` 等。
3. 註冊文字服務（regsvr32，64 與 32 位元；解除安裝時自動取消註冊）。名稱「Bopogram 注音」。
4. `bopogram-host.json`：`{"engine": "rust", "config": "%LOCALAPPDATA%\\Bopogram\\data\\models\\engine.json"}`，
   每個使用者各自的模型與資料（host 與設定程式會展開環境變數）。
5. 以登入的使用者身分（不是系統管理員）：
   - 舊的腳本安裝指向執行包時，把它的 `userdata\` 複製到新資料夾（`bopogram pack adopt-userdata`，只補沒有的檔案，不覆蓋）；
   - 完整安裝檔：安裝模型包（`bopogram pack install`，失敗時提示改用設定程式）；
   - 把鍵盤加入「中文（台灣）」（`keyboard.ps1 add`；語言清單沒有中文（台灣）時提示自行加入）；
   - 啟動 host。
6. 開始功能表「Bopogram 設定」；每個使用者登入時啟動 host（`HKLM\...\Run`；腳本安裝的 `HKCU` 值會移除）。

解除安裝：結束 host、從所有語言移除鍵盤、取消註冊、刪除程式（使用中的檔案重新開機時刪除），
並詢問是否一併刪除 `%LOCALAPPDATA%\Bopogram`（模型、設定、個人化資料；預設保留）。

安裝程式的訊息是繁體中文：Inno Setup 沒有附，`installer/ChineseTraditional.isl` 由它的 Default.isl 自行翻譯
（保留原著作權聲明、標明為修改版）。Inno Setup 6.7 起標示「Non-commercial use only」，本專案為非商業開源使用。

沙盒測試：`sandbox\prepare-installer.ps1 -Pack <模型包>`，再開 `D:\bopogram-data\sandbox\installer.wsb`；
在假造的腳本安裝上靜默安裝，檢查檔案、註冊、鍵盤、登入時啟動、模型包、個人資料移轉、`--stop`、執行中的覆蓋安裝，
結果在 `out-installer\result.txt`。

## Linux

- `.tar.gz`：`./install.sh [模型包.zip]`，程式放 `/usr/local/lib/bopogram/`（`/usr/local/bin/bopogram`）、
  IBus 元件檔放 `/usr/share/ibus/component/`（這兩步用 sudo），模型包以使用者身分安裝到 `~/.local/share/bopogram/`。
- `.deb`：程式在 `/usr/lib/bopogram/`、`/usr/bin/bopogram`，授權在 `/usr/share/doc/bopogram/`；每個使用者
  `bopogram pack install <模型包.zip>` 後 `ibus write-cache; ibus restart`。
- 輸入來源清單裡找不到時：先關掉「設定」再重開（它保留開啟時讀到的清單）。
- IBus 引擎優先讀 `~/.local/share/bopogram/models/engine.json`（模型包），沒有時讀 `config.json`（測試包）。
