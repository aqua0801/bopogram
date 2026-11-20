# Bopogram 注音（試用版）：安裝、移除與回報

這是第 4 階段的試用版：Python 原型引擎加上最小的 TSF 前端，目的是實際打字評估選字。
它不是正式版，沒有數位簽章，只供自己的機器使用，不可散布（執行包內的字典為 CC BY-ND）。

## 安裝前的條件

- 64 位元 Windows，Smart App Control 為關閉（開啟時會擋下未簽章的 DLL）。
- 已在 `crates\` 執行過 `cargo build --release` 與
  `cargo build --release -p bopogram-ime --target i686-pc-windows-msvc`（32 位元版）。
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
| 2. 結束 host（等引擎存好個人化資料），複製 `bopogram_ime.dll`、`bopogram-host.exe`、`bopogram-settings.exe`、`bopogram.ico`、`uninstall.ps1`（含 `common.ps1`），32 位元 DLL 放進 `x86\`，寫入 `bopogram-host.json` | `C:\Program Files\Bopogram\`；使用中的舊檔排入重開機刪除（`HKLM\SYSTEM\CurrentControlSet\Control\Session Manager` 的 `PendingFileRenameOperations`） |
| 3. `regsvr32` 註冊文字服務（64 與 32 位元各一次） | `HKLM\SOFTWARE\Classes\CLSID\{B6B6D901-0BBF-4287-A302-F265DBE9A46C}`、其 32 位元版 `HKLM\SOFTWARE\WOW6432Node\Classes\CLSID\{…}`、`HKLM\SOFTWARE\Microsoft\CTF\TIP\{B6B6D901-0BBF-4287-A302-F265DBE9A46C}` |
| 4. 把輸入法加進目前使用者的「中文（繁體，台灣）」鍵盤 | 目前使用者的語言清單（`Set-WinUserLanguageList`，存在 `HKCU`） |
| 5. 開始功能表捷徑「Bopogram 設定」 | `C:\ProgramData\Microsoft\Windows\Start Menu\Programs\` |
| 6. 登入時啟動引擎 host（`-NoAutostart` 可略過），並立即以一般權限啟動一次 | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 的 `Bopogram` 值 |

安裝在 `Program Files` 是因為 DLL 會被載入到所有程式（包含以系統管理員執行的程式），
所在資料夾必須只有系統管理員能寫入。

裝好後用 Win+空白鍵切到「Bopogram 注音（試用版）」。安裝前就開著的程式要重開才會用到新版。

### 更新

重新建置後再執行一次 `install.ps1` 即可，不必登出：

- 正在使用中的舊 DLL 會被改名為 `*.old-<時間>`，並排定在下次重開機時刪除（下次安裝也會清）。
- 已開著的程式繼續用舊版直到重開，新開的程式用新版；腳本最後會列出還在用舊版的程式。
  檔案總管與開始功能表搜尋（`SearchHost.exe`）也一樣：在工作管理員重新啟動檔案總管、
  結束 `SearchHost.exe`（會自動再起來）即換成新版。
- 引擎 host 會重新啟動；舊版 DLL 與新版 host 使用同一份協定，可以一起運作。
- 輸入法清單（Win+空白鍵）裡的圖示由 Windows 快取，換圖示時可能要等下次登入才更新。

## 移除

以系統管理員身分開啟 PowerShell，執行：

```
powershell -ExecutionPolicy Bypass -File D:\bopogram\crates\install\uninstall.ps1
```

安裝資料夾裡也有一份：`C:\Program Files\Bopogram\uninstall.ps1`，repo 不在時可用它移除。不必登出。

它依序：

1. 把輸入法從目前使用者所有語言的鍵盤清單拿掉。
2. 刪除登入時啟動（`Run` 的 `Bopogram`）與開始功能表捷徑。
3. `regsvr32 /u` 解除註冊（64 與 32 位元），登錄裡若還有殘留的本輸入法機碼則直接刪除。
4. 結束 host（等引擎存檔）與設定程式，刪除 `C:\Program Files\Bopogram`。
   還開著的程式會佔住 DLL：這些檔案與資料夾排定在下次重開機時刪除，腳本會列出是哪些程式。
5. 安裝時若用了 `-Trace`，刪除該紀錄檔（它記錄了所有打的字）。
6. 刪除 `%LOCALAPPDATA%\Bopogram\`：設定（`keys.json`、`style.txt` 等）、`host.log`（只記啟動與錯誤）、
   計時紀錄。加 `-KeepSettings` 可保留。
7. 加 `-RemovePersonalData` 時，刪除引擎 `config.json` 指定的個人化資料（`user.json`）、打字紀錄
   （`typing_log.jsonl`）與使用者詞庫（`userdict.tsv`、`excluded.tsv`）。預設保留。
8. 最後檢查登錄中是否還有本輸入法的 CLSID，有就列出來。

預設不刪的只有執行包（`D:\bopogram-data\bopogram-runtime\`，含個人化資料）與引擎程式（repo）。

## 程式與資料在哪裡

| 項目 | 位置 |
|---|---|
| 前端 DLL、host、設定程式 | `C:\Program Files\Bopogram\` |
| 引擎程式 | `D:\bopogram\prototype`（`git pull` 後，結束 `bopogram-host.exe` 即使用新版） |
| 字典、模型、設定 | `D:\bopogram-data\bopogram-runtime\` |
| 個人化資料、打字紀錄 | 同上的 `userdata\` |

全部在本機，沒有任何網路連線。引擎由 `bopogram-host.exe` 在登入時啟動（沒有設定時則在第一次打字時），常駐約 470 MB 記憶體；
要重新啟動引擎，在工作管理員結束 `bopogram-host.exe` 即可，下次打字會自動再起來。

## 工具列圖示與設定

切到本輸入法時，工作列的輸入指示區會出現「中」或「英」的圖示，顏色跟隨工作列的淺色／深色；
全形模式時字下方多一條短橫線。輸入法清單（Win+空白鍵）裡的 Bopogram 圖示是藍底的「ㄅ」
（`bopogram.ico`，由 `bopogram-ime\src\icon.rs` 自行繪製產生）。

- 左鍵：切換中／英（同單按 Shift）。
- 右鍵：選單，可切換口語／正式，或開啟「設定…」。

設定視窗（`bopogram-settings.exe`，也可以直接執行）的選項：

| 選項 | 存在哪裡 | 何時生效 |
|---|---|---|
| 選字風格：口語／正式 | `%LOCALAPPDATA%\Bopogram\style.txt` | 切換到任一程式時 |
| 所有程式共用中／英模式 | `%LOCALAPPDATA%\Bopogram\mode-scope.txt`（共用的模式存在 `mode.txt`） | 切換到任一程式時 |
| 新開的程式以中文或英文開始 | `%LOCALAPPDATA%\Bopogram\keys.json` 的 `start_mode` | 之後開啟的程式 |
| 單按 Shift 切換中／英 | `keys.json` 的 `shift_toggle` | 約 5 秒內 |
| 選字鍵：1 2 3… 或 A S D F… | `keys.json` 的 `select_keys` | 約 5 秒內 |
| 每頁候選數：5、7、9、10 | `keys.json` 的 `page_size` | 約 5 秒內 |
| 候選視窗文字大小 | `%LOCALAPPDATA%\Bopogram\candidate-font.txt` | 約 5 秒內 |
| 個人化學習 | 執行包 `config.json` 的 `user_model` | 套用時重新啟動引擎 |
| 打字紀錄 | 執行包 `config.json` 的 `typing_log` | 套用時重新啟動引擎 |
| 計時紀錄 | `%LOCALAPPDATA%\Bopogram\timing-until.txt`（到期時間） | 約 10 秒內；24 小時後自動關閉 |

另有「審核打字紀錄…」（開啟 `review_log.py`）、「開啟資料夾」（`userdata\`）、「重新啟動引擎」。
關閉個人化學習或打字紀錄不會刪除已存的資料；要刪除個人化學習資料，按「清除個人化資料…」：
會先停止引擎、刪除 `user_model` 指向的檔案（預設 `userdata\user.json`），再重新啟動引擎。打字紀錄不受影響。

## 候選視窗

- 按 ↓ 開啟。滑鼠可直接點選候選字；滾輪或底下的 ‹ › 換頁。
- 跟隨 Windows 的淺色／深色模式（「設定 → 個人化 → 色彩 → 選擇應用程式模式」），換了之後下次開啟生效。
- 出現在游標所在那個字的下方，下方空間不夠時改到上方。
- 自己繪製輸入法介面的程式（例如部分全螢幕遊戲）會改用它們自己的候選清單，本輸入法的視窗不會出現。

## 按鍵

與微軟新注音相同的部分見 `prototype\docs\frontend-handoff.md` 的按鍵表。與它不同或需要知道的：

- Shift 單按：中／英切換。Ctrl+`` ` ``：口語／正式切換。
- 中／英模式預設每個程式各自記住（同微軟注音）。設定裡勾選「所有程式共用」後，
  在任一程式切換，其他程式在下次切到它時跟著變。
- Shift 加符號鍵輸入半形符號（`:` `"` `<` `>` `?` `!` 等）：組字中插入組字區，沒在組字時直接交給程式。
  全形標點用 Ctrl 加符號、單按 `=` `[` `]` `\` `'`，或先按 `` ` `` 再按符號鍵。
- Ctrl 加符號只接 `,` `.` `'` `;` `[` `]`（，。、；【】），以及 Ctrl+Shift 加 `;` `/` `1`（：？！）；
  其他 Ctrl 組合一律交給程式。游標在符號旁時，↓／↑ 可換成相關符號（例如 【 → 「）。
- Shift+空白鍵（中文模式）：全形／半形切換，切換時游標旁會短暫顯示「全形」或「半形」。
  全形時，沒在組字時打的空白、Shift 或 Caps Lock 的英文字母、數字鍵台與 Shift 符號都會變成全形；
  注音鍵照常組字，Ctrl 快速鍵照常交給程式。英文模式一律是一般鍵盤（Shift+空白鍵也不攔），
  以免打大寫字母接空白時誤觸。
- 選字鍵改成 A S D F… 後，候選視窗開著時這些鍵用來選字，數字鍵則照常是注音。

## 已知限制

- AppContainer 程式（Windows 搜尋、開始功能表、市集 App）可以打字，但在裡面打的字一律不學習、不記錄。
- 以系統管理員身分執行的程式不會自己啟動引擎；登入時已啟動 host 就能正常使用。
- 引擎啟動要載入模型數秒，這段時間打的字先以注音符號顯示，引擎就緒後自動換成選字結果；
  等超過 20 秒仍未就緒時，畫面上的注音符號會原樣送出，之後 30 秒內按鍵直接交給程式。
- 引擎回應慢（單次超過 200 ms）時組字會保留，畫面最多再等 200 ms 後補上，再不行就等下一次按鍵。
  只有引擎不在了（管線斷線，或 host 等了 5 秒仍沒回應）才放棄，組字區的內容原樣送進文件。
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

**按鍵反應慢**：在設定視窗勾選「計時紀錄」，照常打字到感覺慢的情況出現，
再把 `%LOCALAPPDATA%\Bopogram\timing.log` 交回來。每行是時間、程式名稱、請求種類與各步驟的耗時
（`password` 檢查密碼欄位、`context` 讀上文、`engine` 等引擎、`write` 寫入文件、`popup` 候選視窗），
不含任何打的字。24 小時後自動停止。

**文字被吞、莫名送出**：開啟引擎的追蹤記錄後重現問題，再把檔案交回來。
追蹤檔會記下**所有打的字**（密碼欄位除外），查完務必關掉並刪除檔案。

```
# 開啟（以系統管理員身分）
powershell -ExecutionPolicy Bypass -File D:\bopogram\crates\install\install.ps1 -Trace D:\bopogram-trace.txt
# 關閉：不加 -Trace 再裝一次，然後刪除 D:\bopogram-trace.txt
```

開啟期間 `host.log` 會有一行 `TRACE IS ON`。

**緊急狀況**（某個程式因輸入法無法使用）：Win+空白鍵切回微軟注音即可；仍有問題就執行移除腳本。
