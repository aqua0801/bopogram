# 第 4 階段：Windows 前端交接說明

這份文件給在 Windows（`D:\bopogram`）進行的工作階段使用。引擎與協定在 WSL 端已完成並有測試；
Windows 端要寫的是 `crates/` 底下的前端。先讀完 `CLAUDE.md`，尤其是授權、安全與環境分工三節。

## 目標

讓使用者在 Windows 上用 Bopogram 實際打字數天，以評估選字品質。
這是試用版：前端只做「收按鍵、顯示組字區與候選字、送出文字」，所有選字邏輯都在引擎。

## 不可違反的規則（摘自 CLAUDE.md）

- 不得閱讀其他輸入法專案的原始碼。TSF 的知識來源只限微軟官方文件與 `windows-rs` 的文件。
- 不使用 .NET。前端用 Rust（`windows-rs`）。
- 不得有任何網路功能。行程間通訊只用本機具名管線。
- `regsvr32`、修改登錄檔、註冊輸入法：**一律先問使用者**，而且先在 Windows Sandbox 或 VM 測試。
- 密碼欄位必須略過：不處理按鍵、不記錄、不學習。
- 已送出的文字絕對不回頭修改。

## 元件

```
應用程式行程                      使用者工作階段（每位使用者一個）
┌─────────────────────┐          ┌──────────────────────────────────────┐
│ bopogram_ime.dll    │  具名管線 │ bopogram-host.exe                    │
│ （TSF 文字服務）     │◄────────►│  建立管線、轉送訊息                  │
└─────────────────────┘          │      │ 標準輸入／輸出                │
                                 │      ▼                              │
                                 │  python -m bopogram.cli serve       │
                                 │  （原型引擎，載入一次，約 360MB）    │
                                 └──────────────────────────────────────┘
```

### `crates/bopogram-host`（exe）

- 啟動時以子行程執行引擎：`python -m bopogram.cli --config <執行包>\config.json serve`，
  用管線接它的標準輸入與輸出。引擎印出 `{"ok": true, "ready": true}` 後才開始服務。
- 建立具名管線 `\\.\pipe\bopogram-<使用者 SID>`：
  - 必須加 `PIPE_REJECT_REMOTE_CLIENTS`。
  - 安全描述元只允許目前使用者（與 SYSTEM）連線。
  - 每個連線配一個 `sid`（遞增整數），把該連線的每一行請求加上 `sid` 後轉給引擎，
    依回應的 `sid` 送回對應的連線。連線中斷時送 `{"op":"close","sid":N}`。
- 引擎子行程結束時自動重啟一次；再失敗就記錄錯誤並結束。
- 由 DLL 在連不上管線時啟動（單一執行個體，用具名 mutex）。

### `crates/bopogram-ime`（DLL，crate 名稱 `bopogram_ime`）

- TSF 文字輸入處理器：`ITfTextInputProcessorEx`、`ITfKeyEventSink`、`ITfCompositionSink`，
  以及顯示屬性（組字區底線）。
- 按鍵處理（`OnTestKeyDown`／`OnKeyDown`）：
  | 按鍵 | 送給引擎 | 條件 |
  |---|---|---|
  | 大千配置的符號鍵與聲調鍵（含空白鍵） | `key` | 空白鍵只在有未完成音節時當一聲；否則見下 |
  | Backspace | `backspace` | 組字區非空 |
  | ←／→ | `left`／`right` | 組字區非空 |
  | ↓ 或空白鍵（無未完成音節且組字區非空） | `candidates`，並顯示候選視窗 | |
  | 數字鍵 1–9（候選視窗開啟時） | `select`（索引 = 該頁第幾個） | |
  | Enter | `commit`，把回傳的文字送進文件 | 組字區非空 |
  | Esc | 關閉候選視窗；若已關閉則 `escape` | |
  其他按鍵、以及組字區為空時的非注音鍵，一律不處理（回傳未吃掉），讓應用程式自己處理。
- 每次回應都帶完整狀態，前端只要照 `preedit` 重畫組字區，不需自己維護狀態。
- **上文**：開始新的組字前，嘗試用 TSF 讀游標前最多 40 個字，送 `context`。
  讀不到（應用程式不支援）就不送，引擎會自動用它自己最近送出的文字。
- **密碼欄位**：輸入範圍（`IS_PASSWORD`）或 `ES_PASSWORD` 的欄位，DLL 完全不處理按鍵；
  保險起見另送一次 `{"op":"learning","enabled":false}`。離開該欄位後再送 `enabled: true`。
- **口語／正式模式**：提供一個切換方式（建議 Ctrl+\` 或語言列按鈕），切換時對目前的輸入位置送 `style`，
  並在之後每個新連線建立時先送一次目前的模式。預設值寫在執行包 `config.json` 的 `"style"`。
- 候選視窗：試用版用最簡單的 Win32 彈出視窗即可（每頁 9 個，PageUp／PageDown 換頁）。
- DLL 內不得載入模型或做任何重運算；DLL 會被載入到所有程式。
- 管線讀寫要設逾時（建議 200ms）。逾時或斷線時放棄這次按鍵並讓它直接通過，不可卡住應用程式。

## 協定

一行一個 JSON 物件，UTF-8，以 `\n` 結尾。DLL 與 host 之間、host 與引擎之間格式相同
（host 只負責加上 `sid`）。完整說明在 `prototype/bopogram/server.py` 的檔頭註解。

請求欄位：`op`（必填）、`id`（選填，回應會原樣帶回，用來對應請求）、`sid`（host 加上）。

| `op` | 其他欄位 | 作用 |
|---|---|---|
| `key` | `key`：一個字元 | 輸入一個配置按鍵。不屬於配置的鍵回 `ok: false` |
| `backspace` | | 刪除一個注音符號，或一個音節 |
| `left`／`right` | | 移動候選游標 |
| `candidates` | | 回應多一個 `candidates` 陣列 |
| `select` | `index` | 選第幾個候選（對整份清單的索引，從 0 起算） |
| `commit` | | 回應多一個 `commit` 字串，組字區清空 |
| `escape` | | 丟棄組字區 |
| `context` | `text` | 游標前的文字，只在組字區為空時有效 |
| `learning` | `enabled` | 開關個人化學習 |
| `style` | `style`：`casual` 或 `formal` | 口語模式（預設）：輕聲「ㄉㄜ」一律先選「的」；正式模式：由模型判斷的／地／得。只影響送出請求的那個 `sid` |
| `state` | | 只查詢狀態 |
| `close` | | 釋放這個 `sid` |
| `quit` | | 結束引擎（只由 host 使用） |

回應欄位：

| 欄位 | 說明 |
|---|---|
| `ok` | 是否成功 |
| `preedit` | 組字區要顯示的字串（已選的字加上未完成的注音符號） |
| `pending` | 未完成音節的注音符號 |
| `cursor` | 候選游標位置（第幾個字之後） |
| `syllables` | 已完成的音節數 |
| `segments` | 目前的斷詞結果 |
| `style` | 目前的模式（`casual`／`formal`） |
| `candidates` | `[{"word": "在家", "start": 1, "length": 2}, ...]`，長詞在前 |
| `commit` | 送出的文字 |
| `error` | `ok` 為 false 時的原因 |

範例（輸入「我在家」後送出）：

```
→ {"op":"key","key":"j","id":1}
← {"ok":true,"preedit":"ㄨ","pending":"ㄨ","cursor":0,"syllables":0,"segments":[],"id":1}
  ...
→ {"op":"key","key":" ","id":10}
← {"ok":true,"preedit":"我在家","pending":"","cursor":3,"syllables":3,"segments":["我","在家"],"id":10}
→ {"op":"commit","id":11}
← {"ok":true,"commit":"我在家","preedit":"","pending":"","cursor":0,"syllables":0,"segments":[],"id":11}
```

## 在 Windows 上執行引擎

1. 安裝 Python 3.12（64 位元）。
2. 在 repo 的 `prototype\` 下建立虛擬環境並安裝：`pip install -e .`
   這會編譯 KenLM 的 Python 套件，需要 MSVC 建置工具。**這一步在 Windows 上尚未驗證**，
   編譯失敗時先回報，不要改用來路不明的預先編譯檔。
3. 把 WSL 的 `~/bopogram-data/dist/bopogram-runtime/` 整個資料夾複製到 Windows
   （例如 `D:\bopogram-data\bopogram-runtime\`）。內含字典與模型，約 256MB。
   這個資料夾**不得進 git、不得散布**（字典源自教育部資料，CC BY-ND）。
4. 驗證：
   ```
   python -m bopogram.cli --config D:\bopogram-data\bopogram-runtime\config.json convert --keys "ji3y94ru8 "
   ```
   應輸出「我在家」。
5. **尚未驗證**：Linux 產生的 `.binary` 模型檔能否直接在 Windows 讀取。
   若讀取失敗，需在 Windows 上編譯 KenLM 的 `build_binary`，從 WSL 端
   `~/bopogram-data/models/*.arpa` 重新轉換（參數見 `training/lm/train.py`）。
6. 單元測試：`python -m unittest tests.test_layouts tests.test_engine`（14 項，不需要模型）。

個人化資料會寫在執行包的 `userdata\user.json`，只存在本機。

## 測試順序

1. 引擎在 Windows 命令列能跑（上面第 4 步）。
2. `bopogram-host` 單獨執行，用一個小的命令列用戶端連管線，手動送請求。
3. DLL 在 **Windows Sandbox 或 VM** 內註冊並測試：記事本、瀏覽器輸入框、密碼欄位。
4. 以上都通過並向使用者說明後，才由使用者決定是否在主系統安裝。

## 交付給使用者時要附上

- 安裝與解除安裝的步驟（包含會動到哪些登錄機碼）。
- 已知限制清單。
- 如何回報選錯的案例（至少要有：輸入的注音、期望的字、實際出現的字），這些會加進測試句集。
