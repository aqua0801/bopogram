# Bopogram 注音（試用版）：安裝、移除與回報

這是第 4 階段的試用版：Python 原型引擎加上最小的 TSF 前端，目的是實際打字評估選字。
它不是正式版，沒有數位簽章，只供自己的機器使用，不可散布（執行包內的字典為 CC BY-ND）。

## 安裝前的條件

- 64 位元 Windows，Smart App Control 為關閉（開啟時會擋下未簽章的 DLL）。
- 已在 `crates\` 執行過 `cargo build --release`。
- 引擎可在本機執行：`prototype\.venv` 已建立，執行包在 `D:\bopogram-data\bopogram-runtime`。
- 已在 Windows Sandbox 測過同一版（`crates\sandbox\prepare.ps1`）。

## 安裝

以系統管理員身分開啟 PowerShell，執行：

```
powershell -ExecutionPolicy Bypass -File D:\bopogram\crates\install\install.ps1
```

Python 或執行包不在預設位置時，加上 `-Python <python.exe>`、`-Config <config.json>`。
不想讓腳本動語言清單時加 `-NoKeyboard`，之後到「設定 → 時間與語言 → 語言與地區 →
中文（繁體，台灣）→ 語言選項 → 新增鍵盤」自行加入。

腳本依序做這些事，任何一步失敗就停止：

| 步驟 | 動到的地方 |
|---|---|
| 1. 先單獨執行引擎確認可用 | 無 |
| 2. 複製 `bopogram_ime.dll`、`bopogram-host.exe`、`bopogram-settings.exe`，寫入 `bopogram-host.json` | `C:\Program Files\Bopogram\` |
| 3. `regsvr32` 註冊文字服務 | `HKLM\SOFTWARE\Classes\CLSID\{B6B6D901-0BBF-4287-A302-F265DBE9A46C}`、`HKLM\SOFTWARE\Microsoft\CTF\TIP\{B6B6D901-0BBF-4287-A302-F265DBE9A46C}` |
| 4. 把輸入法加進目前使用者的「中文（繁體，台灣）」鍵盤 | 目前使用者的語言清單（`Set-WinUserLanguageList`，存在 `HKCU`） |

安裝在 `Program Files` 是因為 DLL 會被載入到所有程式（包含以系統管理員執行的程式），
所在資料夾必須只有系統管理員能寫入。

裝好後用 Win+空白鍵切到「Bopogram 注音（試用版）」。安裝前就開著的程式要重開才會用到新版。

### 更新

重新建置後再執行一次 `install.ps1` 即可。正在使用中的舊 DLL 會被改名為 `*.old-<時間>`，
下次安裝或移除時清掉；已開著的程式重開後才換成新版。

## 移除

以系統管理員身分開啟 PowerShell，執行：

```
powershell -ExecutionPolicy Bypass -File D:\bopogram\crates\install\uninstall.ps1
```

它會把輸入法從語言清單拿掉、`regsvr32 /u` 解除註冊（刪除上表兩個登錄機碼）、結束 host、
刪除 `C:\Program Files\Bopogram`。DLL 載入後不會卸載，所以還開著的程式會佔住檔案：
腳本刪不掉時，請登出再登入後手動刪除該資料夾。

不會被刪除的東西（要清乾淨請手動刪）：

- `D:\bopogram-data\bopogram-runtime\userdata\`：個人化資料 `user.json`、打字紀錄 `typing_log.jsonl`。
- `%LOCALAPPDATA%\Bopogram\`：`host.log`（只記啟動與錯誤，不記打字內容）、`style.txt`（口語／正式）。

## 程式與資料在哪裡

| 項目 | 位置 |
|---|---|
| 前端 DLL、host、設定程式 | `C:\Program Files\Bopogram\` |
| 引擎程式 | `D:\bopogram\prototype`（`git pull` 後，結束 `bopogram-host.exe` 即使用新版） |
| 字典、模型、設定 | `D:\bopogram-data\bopogram-runtime\` |
| 個人化資料、打字紀錄 | 同上的 `userdata\` |

全部在本機，沒有任何網路連線。引擎由 `bopogram-host.exe` 在第一次打字時啟動，常駐約 360 MB 記憶體；
要重新啟動引擎，在工作管理員結束 `bopogram-host.exe` 即可，下次打字會自動再起來。

## 工具列圖示與設定

切到本輸入法時，工作列的輸入指示區會出現「中」或「英」的圖示：

- 左鍵：切換中／英（同單按 Shift）。
- 右鍵：選單，可切換口語／正式，或開啟「設定…」。

設定視窗（`bopogram-settings.exe`，也可以直接執行）的選項：

| 選項 | 存在哪裡 | 何時生效 |
|---|---|---|
| 選字風格：口語／正式 | `%LOCALAPPDATA%\Bopogram\style.txt` | 切換到任一程式時 |
| 個人化學習 | 執行包 `config.json` 的 `user_model` | 套用時重新啟動引擎 |
| 打字紀錄 | 執行包 `config.json` 的 `typing_log` | 套用時重新啟動引擎 |

另有「審核打字紀錄…」（開啟 `review_log.py`）、「開啟資料夾」（`userdata\`）、「重新啟動引擎」。
關閉個人化學習或打字紀錄不會刪除已存的資料。

## 按鍵

與微軟新注音相同的部分見 `prototype\docs\frontend-handoff.md` 的按鍵表。與它不同或需要知道的：

- Shift 單按：中／英切換。Ctrl+`` ` ``：口語／正式切換。
- Shift 加符號鍵輸入半形符號（`:` `"` `<` `>` `?` `!` 等）：組字中插入組字區，沒在組字時直接交給程式。
  全形標點用 Ctrl 加符號、單按 `=` `[` `]` `\` `'`，或先按 `` ` `` 再按符號鍵。
- Ctrl 加符號只接 `,` `.` `'` `;` `[` `]`（，。、；【】），以及 Ctrl+Shift 加 `;` `/` `1`（：？！）；
  其他 Ctrl 組合一律交給程式。游標在符號旁時，↓／↑ 可換成相關符號（例如 【 → 「）。
- 沒有全形／半形切換（Shift+空白鍵）。

## 已知限制

- 只支援 64 位元程式；32 位元程式裡切到本輸入法會沒有作用。
- AppContainer 程式（Windows 搜尋、開始功能表、市集 App）連不到引擎，在裡面只能打英文，請切回微軟注音。
- 以系統管理員身分執行的程式不會自己啟動引擎；引擎已在執行時可正常使用。
- 引擎啟動要載入模型約 1 至 2 秒，這段時間的按鍵會直接變成英文。
- 引擎沒回應（超過 200 ms）或當掉時，按鍵直接交給程式，已顯示的組字會原樣送出。
- 候選字只能用鍵盤選，不能用滑鼠點。
- 密碼欄位會自動停用（不組字、不學習、不記錄）；判斷依賴程式的宣告，
  遇到沒被認出的密碼欄位請先切到英文模式，並回報是哪個程式。
- 候選視窗位置在少數程式可能不準（程式不回報游標位置時改用系統游標或滑鼠位置）。
- 移除後仍開著的程式會繼續佔住 DLL，直到程式結束。

## 回報問題

**選字錯誤**：不用另外記。打字紀錄會存下注音、引擎的首選與實際送出的文字；定期執行審核工具，
核准的紀錄匯出後交給 WSL 那邊：

```
python D:\bopogram\prototype\tools\review_log.py D:\bopogram-data\bopogram-runtime\userdata
```

**前端問題**（打不出字、組字區或候選視窗顯示不對、按鍵行為不對、程式當掉），請提供：

1. 哪個程式、哪種輸入框。
2. 按了哪些鍵，預期看到什麼，實際看到什麼。
3. `%LOCALAPPDATA%\Bopogram\host.log` 最後幾行。
4. 重開該程式後是否還會發生。

**緊急狀況**（某個程式因輸入法無法使用）：Win+空白鍵切回微軟注音即可；仍有問題就執行移除腳本。
