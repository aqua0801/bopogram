# Bopogram

繁體中文注音輸入法，依前後文選字，而不是只看單字詞頻。

*A Traditional Chinese bopomofo (Zhuyin) input method that picks characters from context with an n-gram
language model and a small character-level re-ranker. Runs entirely offline.*

- 「我**再**看看」與「我**在**家」、「**請坐**不用客氣」這類同音字，依上下文自動分辨。
- 以台灣用語為準，中國用語（視頻、質量、信息……）降低權重。
- 全部在本機執行，**沒有任何網路功能**、遙測或自動上傳。個人化學習資料只存在本機，密碼欄位不記錄、不學習。
- 不需要 GPU。目標：一般 x64 CPU，預設設定下記憶體與硬碟各 500 MB 以內。

## 狀態

| 平台 | 引擎 | 前端 | 狀態 |
|---|---|---|---|
| Windows 10／11（x64） | Rust | TSF（`bopogram-ime`） | 可用；安裝檔未簽章（SmartScreen 會警告） |
| Linux（x64） | Rust | IBus（`bopogram-ibus`） | 可用（Ubuntu 24.04 測試）；`.deb` 與 `.tar.gz` |

## 安裝

從 GitHub Releases 下載。程式與模型分開發布，各有版本；更新模型不用重裝程式。輸入法與安裝程式都不會連網，
更新一律由你自己下載檔案。

- **Windows**：`bopogram-setup-<版本>-full.exe`（程式＋模型）。之後的模型更新，在「Bopogram 設定」選擇下載的模型包。
- **Linux**：`.deb` 或 `.tar.gz`，再安裝模型包（`bopogram pack install bopogram-models-<版本>.zip`，然後 `ibus restart`）。
  詳見 `crates/docs/linux.md`。
- **模型包** `bopogram-models-<版本>.zip`：n-gram 模型、標準級 charlm、教育部辭典的原始檔與使用說明。
  安裝時會在你的電腦上從辭典原始檔建出注音字典（見下方「資料與授權」）。

## 準確率與速度

首選整句正確率（兩種寫法都標準的字，例如臺／台、記錄／紀錄，都算對）：

| | 測試句集（513 句） | 開發集（MozTW，11,657 句） |
|---|---|---|
| Bopogram，口語模式 | 94.2% | 92.2% |
| Bopogram，正式模式 | 93.4% | 91.6% |
| 只用 n-gram（不重排） | 91.6% | 90.4% |
| 對照組：只用詞頻（不看上下文） | 55.9% | — |
| 對照組：只用單字頻率 | 8.0% | — |

正式模式依標準寫出「地／得」；口語模式把三者都寫成「的」。

延遲：Rust 引擎每個按鍵 P50 約 1.5 ms、P95 約 2.3 ms（含重排；Ryzen 9 9900X3D、一般 CCD、模擬快取被占用）。

## 運作方式

1. **候選格**：依注音序列，從字典展開所有可能的字詞組合。
2. **n-gram**：三個 3-gram 模型（KenLM 訓練）線性混合，beam search 找出前幾名。
3. **字級語言模型重排（charlm）**：小型因果 Transformer 對前 5 名重新評分。以 8 位元整數在 CPU 上執行，
   不依賴其他推論函式庫。分四級（關閉／輕量 6 MB／標準 15 MB／進階 31 MB）。
4. **個人化**：本機的使用者 n-gram，只學手動改選的詞；另有使用者詞庫。

詳細規格在 `prototype/docs/` 與 `crates/docs/`。

## 專案結構

| 目錄 | 內容 |
|---|---|
| `crates/` | Rust：引擎 `bopogram`（含命令列工具）、Windows 前端 `bopogram-ime`、Linux 前端 `bopogram-ibus`、host、設定程式、安裝腳本 |
| `prototype/` | Python 原型引擎：參考實作、評估、產生對照答案（`golden.md`） |
| `training/` | 語料處理、字典建置、n-gram 與 charlm 訓練 |
| `eval/` | 測試句集、開發集、評估與分析工具 |

## 建置

一般使用請直接安裝 Releases 的安裝檔與模型包。以下是從原始碼建置；repo 不含字典與模型，要自己訓練需下載來源資料（見 `THIRD_PARTY.md`）。

- 引擎：`cd crates && cargo build --release -p bopogram`
- Windows 前端與安裝：見 `crates/docs/install.md`（需 MSVC 工具鏈；DLL 請先在 Windows Sandbox 測試）。
- Python 原型：`cd prototype && uv sync`，執行 `uv run python -m bopogram.cli --help`。
- 字典與模型：`training/` 的腳本（`dict/build_dict.py`、`pipeline/`、`lm/train.py`、`neural/`），
  來源資料清單與授權見 `THIRD_PARTY.md`。

## 資料與授權

- **程式碼**：MIT（`LICENSE`）。Rust 相依套件的授權清單在 `crates/docs/licenses.md`（全部為寬鬆授權）。
- **字典**：由教育部《重編國語辭典修訂本》《國語辭典簡編本》《成語典》（CC BY-ND 3.0 臺灣）等資料建置。
  授權不允許散布改作的成品，所以模型包附的是**未經修改的原始檔與其公眾授權使用說明**，字典在你的電腦上建置。
- **模型**：模型包內的字級語言模型（charlm）以 CC BY-SA 4.0 授權；n-gram 模型與其他資料的出處見模型包的 `NOTICE.txt`。
  語料本身不散布；來源與授權記錄在 `THIRD_PARTY.md`。
- **Windows 安裝檔**以 Inno Setup 製作，內含其安裝與解除安裝程式（Inno Setup 6.7 起限非商業用途；本專案為非商業開源）。
- **測試資料**：`eval/testset` 為本專案自行撰寫；`eval/devset` 取自 MozTW cc0-sentences（CC0）。

## 原創性

本專案沒有閱讀、引用或參考其他輸入法專案的原始碼，也沒有使用其他輸入法的詞庫、模型或測試集。
前端直接以 Windows TSF API 撰寫，不使用第三方輸入法框架。
唯一的例外是評估：以新酷音（libchewing，LGPL）的公開 C API 跑測試句集作為對照組，未讀其原始碼，也未取用其詞庫或輸出（見 `THIRD_PARTY.md`）。
