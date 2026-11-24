# Bopogram 正式版引擎規格（第 6 階段，草稿）

2026-11-23 起草，**待使用者確認**。標「待決定」的項目需要使用者選擇或同意。

## 目標

把 Python 原型引擎（`prototype/bopogram/`，約 2,600 行）移植成 Rust，行為完全相同，
讓輸入法不再需要 Python 與 WSL 端的執行環境。

- 同一份執行包（`dict.tsv`、模型、`reading_prior.tsv`、`charlm/`、`config.json`）。
- 同一份協定（`prototype/docs/frontend-handoff.md`、`userdict-engine.md`）：前端完全不用改。
- 同一套命令列（`prototype/bopogram/cli.py`）：`convert`、`eval`、`serve`、`lookup`。
- 以對照答案驗收（`prototype/docs/golden.md`）：重播約 4.7 萬個協定請求，**0 個差異**。

不做：選字邏輯的任何改良。要改選字，先在 Python 原型改、重新產生對照答案，再移植。
這樣兩邊隨時能互相驗證，Python 原型繼續當作實驗與評估的地方。

## 範圍

| Python 模組 | 內容 | 移植 |
|---|---|---|
| `lexicon.py` | 字典索引、讀音先驗 | 是 |
| `lm.py` | 多個 n-gram 模型的線性混合、詞彙對應 | 是（模型讀取方式見下文） |
| `decoder.py` | 候選格、Viterbi／beam、增量解碼 | 是 |
| `context.py` | 上文斷詞與語言模型狀態 | 是 |
| `session.py`、`symbols.py`、`layouts/` | 組字區編輯規則、候選清單、符號 | 是 |
| `user.py` | 個人化學習（`user.json`） | 是，檔案格式不變 |
| `userdict.py`、`dictops.py` | 使用者詞庫與 `dict_*` 操作 | 是，檔案格式不變 |
| `typing_log.py` | 打字紀錄 | 是，檔案格式不變 |
| `charlm.py` | 字級語言模型重排（ONNX） | 是（需要 ONNX Runtime，見下文） |
| `server.py`、`cli.py` | 協定迴圈、命令列 | 是 |
| `neural.py` | 逐位置神經評分 | 否：目前執行包沒有啟用。之後啟用時再移植 |
| `evaluate.py` | 評估 | 是（`eval` 命令，用來比對正確率與延遲） |
| `prototype/tools/review_log.py` | 打字紀錄審核工具 | 待決定：留 Python（只有開發者使用），或之後另外移植 |

## 架構

- crate `bopogram`（`crates/bopogram/`）：引擎函式庫加 CLI（執行檔 `bopogram.exe`）。
  **不依賴任何 Windows 元件**，在 Linux 上也能編譯與測試（CLAUDE.md 的要求）。
- 接上前端分兩步：
  1. host 改成啟動 `bopogram.exe serve`，取代現在的 `python -m bopogram serve`，管線協定不變。
     `bopogram-host.json` 設定用哪一個，可以隨時切回 Python 版。
  2. 穩定之後，host 直接連結引擎函式庫，少掉一個行程與一次管線往返。

## n-gram 模型：自有格式（已定）

執行包的模型是 KenLM 的二進位格式（`*.binary`，`build_binary -q 8 -b 8 trie`）。
不連結 KenLM（C++、LGPL-2.1，在 MSVC 上編譯不順），也不照 KenLM 的內部格式自己讀
（格式只能從其原始碼得知，與其實作綁死）。改用 ARPA：

- WSL 端以 `training/lm/export_arpa.py` 從 `.binary` 讀回**量化後**的值，匯出成標準 ARPA
  （說明在 `prototype/docs/lm-arpa.md`；檔案在 `D:\bopogram-data\arpa\`，不進 git）。
  訓練時的原始 ARPA 不能用：與量化值每個字最多差 0.19（log10），選字會不同。
- 本專案的轉換工具（`bopogram build-lm`）把 ARPA 轉成自己的二進位格式，引擎啟動時讀進記憶體。
  - unigram 的機率與 back-off 存 f32。
  - 2-gram、3-gram 的機率與 2-gram 的 back-off，每一階不同的值最多 256 個：存「值的表」加 8 位元索引，
    完全不失真，記憶體與 KenLM 量化版相近。轉換時檢查，超過 256 個就報錯，**絕不自行再量化**，
    否則值會改變，對照答案不會過。
  - 查詢：雜湊表找 n-gram；找到最長的就用它的機率，找不到就加上 context 的 back-off 退一階；
    不在詞彙裡用 `<unk>`。分數是 log10，換算與混合方式照 `lm.py`。
- 匯出的值與 `.binary` 的分數差在 5e-6 以內（float 加法順序造成）。Rust 端照 KenLM 計分時的加法順序實作；
  若對照答案仍因此出現名次互換，逐一確認並與 WSL 端討論。

## 待決定 2：字級語言模型（charlm）用什麼執行

- 建議：`ort` crate（MIT OR Apache-2.0），呼叫 ONNX Runtime（MIT，已登記於 `THIRD_PARTY.md`）。
  以動態載入方式使用官方發行的 `onnxruntime.dll`，放進執行包；建置時不讓 `ort` 自己下載任何東西。
- 替代方案：純 Rust 推論（例如 `tract`），不必帶 DLL，但 int8 量化模型的支援與速度要先驗證。
- 依 `golden.md`，charlm 是唯一可接受的差異來源：浮點數的極小差異可能讓分數幾乎相同的兩個結果互換名次。
  驗收時先關掉 charlm 達成 0 差異，再開 charlm 比對，並逐一確認剩下的差異都屬於這一類。

新增的依賴（`ort`，以及之後可能需要的其他 crate）在採用前先登記於 `THIRD_PARTY.md` 並告知使用者。

## 待決定 3：誰寫、放在哪裡

- 建議：Windows 端（本對話）負責 `crates/bopogram/`。WSL 端繼續負責原型、對照答案、訓練與評估。
  移植時發現原型的行為有疑問，回報給 WSL 端，在原型那邊修正並重新產生對照答案，Rust 端不自行改行為。
- ARPA 匯出由 WSL 端提供（已完成）；轉換工具放在 `crates/bopogram/`，兩邊都能執行。

## 非功能目標

| 項目 | 目標 | 量法 |
|---|---|---|
| 每鍵延遲（引擎內） | P95 ≤ 10 ms（不含 charlm）；含 charlm P95 ≤ 20 ms | `eval` 命令與前端計時紀錄 |
| 啟動到可以打字 | ≤ 2 秒（Python 版目前要數秒） | host 記錄 |
| 記憶體 | ≤ 500 MB（模型約 285 MB） | 工作管理員、host 記錄 |
| 網路 | 無。引擎不開任何連線，依賴也不能有連線功能 | 審查依賴 |
| 個人資料 | 只存在本機，檔案格式與 Python 版相同，可以來回切換 | 對照答案的 `learning`、`userdict` 輪 |

每次評估依 CLAUDE.md 同時報告：首選正確率、只用單字詞頻的對照組、延遲 P50／P95。
Rust 版的首選正確率必須與 Python 版完全相同；不同就是移植有錯。

## 里程碑

| # | 內容 | 驗收 |
|---|---|---|
| E1 | crate 骨架、字典、n-gram 模型與轉換工具、解碼器、`convert` 與 `lookup` | 測試句集逐句的首選與 Python 版完全相同 |
| E2 | 組字區、候選清單、符號、上文、`serve` 協定 | 對照答案的 `testset`、`devset`、`editing` 三輪 0 差異（charlm 關閉） |
| E3 | 個人化學習、使用者詞庫、打字紀錄 | `learning`、`userdict` 兩輪 0 差異；`user.json` 與 `.tsv` 檔內容相同 |
| E4 | charlm（依待決定 2）、`eval` 命令 | 全部輪次通過（charlm 的差異逐一確認）；延遲與記憶體達標 |
| E5 | 接上 host（先以 `bopogram.exe serve` 取代 Python）、安裝腳本 | 沙盒通過後交給使用者試用；可一鍵切回 Python 版 |
| E6 | host 直接連結引擎、移除 Python 依賴 | 試用一段時間沒有問題後再做 |

每個里程碑 commit 一次。E5 起照前端的慣例：先在沙盒測試，再由使用者安裝。

## 進度與發現

### E1（2026-11-23）

- 測試句集 513 句、開發集前 2,000 句，正式與口語兩種模式，首選與 Python 版**完全相同**。
  另抽 150 句比對前 8 名與分數：149 句逐字相同，1 句只差在分數第三位小數的四捨五入（名次相同）。
- 速度：載入 0.3 秒（模型 334 MB）；開發集 2,000 句 3.1 秒，Python 版 10 秒。
- 模型檔比 KenLM 量化版大約兩成（346 MB 對 285 MB），在記憶體上限內。
- 狀態合併：KenLM 只把「後面還接得到更長 n-gram」的詞留在狀態裡，判斷依據是有沒有更長的 n-gram，
  **不是** ARPA 有沒有 back-off 欄（很多 n-gram 有 back-off 欄卻沒有後續）。以 `bopogram lm-probe`
  對 KenLM 的 `.binary` 逐詞比對，狀態完全一致。

### E2（2026-11-24）

- 組字區（`session.rs`）、符號（`symbols.rs`）、`serve` 協定（`server.rs`，含 `--trace`）。
  請求欄位的寬鬆讀法（數字字串、真假值）照原型；讀不懂的請求回 `ok: false` 且不帶 `id`，與原型相同。
- 模型改用 02d8775 的精確 ARPA 重新轉換。
- 對照答案：以原型、`charlm: null` 在本機重新產生（`make_golden.py`，17 秒），
  `testset`、`devset`、`editing` 共 46,585 個請求 **0 差異**，重播 6 秒。
- 參考：對正式對照答案（含 charlm）同三輪有 3,133 個差異，全在 testset／devset，屬 E4 範圍。
- 使用者詞庫、學習（`userdict`、`learning` 兩輪）屬 E3，尚未實作。

### 請 WSL 端處理（兩項都已在 02d8775 處理）

WSL 端的處理結果：ARPA 改為最短的精確 float32 文字，back-off 以 KenLM 的分數解出；web 與 `.binary`
完全相同，ly、general、fineweb_small 各有 7～550 個詞差 1 個 float32 單位（見 `prototype/docs/lm-arpa.md`）。
`nbest()` 已修正，對照答案重新產生，Rust 端同步移除這個行為。

1. **ARPA 的數值要能精確還原 float32。** 目前 `export_arpa.py` 以 `.7g` 輸出，兩個不同的量化值可能印成
   同一個字串，分數因此有最多 5e-6 的差距（上面那 1 句就是這樣來的），對照答案可能因此出現名次互換。
   - 機率：直接以 `repr(float)`（或 `.9g`）輸出 `BaseScore` 的值即可完全精確。
   - 2-gram 的 back-off：以差值推算會有 1e-7 級的誤差。可行的精確作法：對 context (a, b) 找一個
     (b, w) 存在、(a, b, w) 不存在的詞 w，KenLM 回傳 fl32(P(b w) + B)；找出所有滿足此式的 float32 值 B，
     對同一個量化值取多組交集即可唯一決定。Windows 端用 web 模型試過，差異從 20,639 個降到 295 個，
     剩下的就是 `.7g` 把不同值印成同一字串造成的。
2. **原型的小問題（不影響移植，Rust 已照原樣實作）：** `Decoder.nbest()` 在 `eos_weight` 不為 0 時，
   把變數 `n` 改成組字區長度（`w, score_fn, n = ...`），所以結果數量上限變成音節數而不是要求的 n。
   要修的話在原型修正並重新產生對照答案，Rust 端再跟著改。
