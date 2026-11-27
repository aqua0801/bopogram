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
| `charlm.py`、`charlm_native.py` | 字級語言模型重排 | 是，自有格式 `charlm.bin`，不用 ONNX Runtime（見下文） |
| `server.py`、`cli.py` | 協定迴圈、命令列 | 是 |
| `neural.py` | 逐位置神經評分 | 否：目前執行包沒有啟用。之後啟用時再移植 |
| `evaluate.py` | 評估 | 是（`eval` 命令，用來比對正確率與延遲） |
| `prototype/tools/review_log.py` | 打字紀錄審核工具 | 待決定：留 Python（只有開發者使用），或之後另外移植 |

## 架構

- crate `bopogram`（`crates/bopogram/`）：引擎函式庫加 CLI（執行檔 `bopogram.exe`）。
  **不依賴任何 Windows 元件**，在 Linux 上也能編譯與測試（專案規則的要求）。
- 接上前端分兩步：
  1. host 改成啟動 `bopogram.exe serve`，取代現在的 `python -m bopogram serve`，管線協定不變。
     `bopogram-host.json` 設定用哪一個，可以隨時切回 Python 版。
  2. 穩定之後，host 直接連結引擎函式庫，少掉一個行程與一次管線往返（E6 已完成）。

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

## 字級語言模型（charlm）：自有格式（已定，2026-11-24）

ONNX Runtime 上游 repo 被移除（`onnxruntime-handoff.md`）後，WSL 端改出自有格式 `charlm.bin`
與逐步的計算規格（`prototype/docs/charlm-native.md`，參考實作 `charlm_native.py`）。
Rust 照規格實作（`crates/bopogram/src/charlm.rs`），不需要 ONNX Runtime、`ort` 或任何新依賴。
以下為原本的提案，留作紀錄。

- 原建議：`ort` crate（MIT OR Apache-2.0），呼叫 ONNX Runtime（MIT，已登記於 `THIRD_PARTY.md`）。
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

每次評估依專案規則同時報告：首選正確率、只用單字詞頻的對照組、延遲 P50／P95。
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

### E3（2026-11-24）

- 使用者詞庫（`userdict.rs`、`dictops.rs`，字典上的使用者層在 `lexicon.rs`）、個人化學習（`user.rs`）、
  打字紀錄（`typing_log.rs`）。`close` 與結束時強制存 `user.json`，平常最多 30 秒存一次，與原型相同。
- 對照答案（charlm 關閉）五輪全部 **0 差異**，含結束時的 `user.json`、`userdict.tsv`、`excluded.tsv`。
- 打字紀錄不在對照答案裡：以同一串請求（上文含英數、手動選字、只有符號的送出）分別送給兩個引擎，
  紀錄（`t` 以外）與 `user.json` 相同。
- 打字紀錄的時間要本地時間，標準函式庫沒有時區：Windows 上呼叫 kernel32 的 `GetLocalTime`
  （只此一處，非 Windows 平台用 UTC），引擎仍可在其他平台編譯。
- `readings` 的逐字組合，原型以 Python `set` 迭代讀音，分數相同時順序會隨雜湊種子改變。
  實測（種子 0～5、數個多音字組合）因讀音機率不同而沒有同分，Rust 以字典中讀音出現順序為準。

### E5（2026-11-24）

- host 可選引擎：`bopogram-host.json` 有 `engine`（`bopogram.exe`）就用 Rust 引擎，沒有就用 Python。
  `install.ps1 -Engine rust|python`（預設 rust），`switch-engine.ps1` 不必重裝即可切換。
- Rust 引擎的模型檔不寫進執行包：`--models D:\bopogram-data\arpa`。模型檔比執行包的 `.binary` 舊時，
  引擎在 `host.log` 警告，安裝與切換腳本則停止。
- 單一請求出錯（panic）只回錯誤，不讓引擎結束。前端斷線時（寫回應失敗）也會先存 `user.json` 再結束，
  沙盒試用時發現並以「關閉輸出管道」實測確認。
- 沙盒：兩個引擎單獨執行、安裝、打字、詞庫、雙向切換都正常（使用者確認）。對照答案五輪仍 0 差異。

### E4（2026-11-24）

- charlm（`charlm.rs`）：照 `charlm-native.md` 實作。f64 的加總（平均、softmax 與 log-sum-exp 的分母、
  最後的總和）照 numpy 的 pairwise 順序；矩陣乘法以 8 位元整數精確累加（AVX2，沒有時用一般迴圈）。
  前綴快取與 (上文, 文字) 分數快取的大小、淘汰順序都與原型相同，所以快取狀態也一致。
  每個位置的 log-sum-exp 記在快取裡，同一前綴的候選只算一次輸出層。
- `charlm.bin` 在 `--models` 資料夾或 config 指向的模型旁；沒有時警告並只用 n-gram。
- 測試向量（`bopogram charlm-check`）：1,498 個序列編碼全同，1,496 個分數逐位元相同；
  剩下 2 個，Windows 上的 Python 參考實作也算出與 Rust 相同的值，差異來自 WSL 與 Windows 的數學函式庫。
- 92ee5ea 起，參考實作把每個序列的總分改成依位置順序相加（原本是 numpy 的 pairwise），Rust 跟進。
  以新參考實作在 Windows 重新產生測試向量：1,498 個分數全部逐位元相同（含經過前綴快取），
  `trace`（每層之後的 `x`，6 位小數）7 步全同。`charlm-check` 依新的驗收標準：`logp` 差 ≤ 1e-9。
- 優化（`prototype/docs/charlm-optimization.md`），每一步都以錄下的 5,295 次重排呼叫確認分數與優化前逐位元相同：
  - 整數乘法獨立成 `int8.rs`：AVX-512 VNNI（`vpdpbusd`，啟動值加 128、減去 128×權重和），
    權重在載入時重排成每 16 個輸出列交錯、每 4 個輸入一組，每次算 1～4 列輸入 × 2 組輸出；
    沒有 VNNI 時用 AVX2 的 `vpmaddwd`，再沒有就用純量。每個核心都與純量版逐一比對整數結果。
  - 同一次重排的所有候選併成一批，一層一層算；結果等同逐一計算（每個候選照參考實作的規則，
    從較早保留的序列中共同前綴最長的那一個開始，包含同一批中較早的候選）。
  - 前綴快取改為以位置為單位共用（`Rc`），不再複製整段前綴的 K、V；保留規則仍是最近 32 個序列。
  - 量化與反量化以 AVX-512 執行（除法、floor、乘加都是正確捨入，與純量版相同）。
  - 輸出層 2 個執行緒（常駐的輔助執行緒 `pool.rs`）：各算一半輸出；logsumexp 在 numpy pairwise 的
    第一個分割點（4,968）分開加總，結果相同。
  - 四個線性層（GELU 併在 ff1 裡）各算一半輸出、注意力各算一半的列，都用同一個輔助執行緒。
    交接工作後兩邊先忙等最多 200 µs 再睡（Windows 喚醒睡著的執行緒要數十 µs，一次重排有數十次交接）；
    打字的空檔都在睡。
  - 熱路徑的暫存空間重複使用（每個執行緒一份）；載入後先跑一次假的重排再清掉快取（暖機，結果不變）。
- 量測（2026-11-24，Ryzen 9 9900X3D；兩邊都是 2 個執行緒；`charlm-bench` 與 `bench_ort.py` 重播同一份呼叫，
  每個按鍵用 `eval` 逐音節打測試句集；搶快取：同一個 CCD 上另一個行程不斷讀寫 24 MB）：

  | 條件 | Rust 每次呼叫 熱 P50／P95 | 冷 P50／P95 | ONNX Runtime 熱 P50／P95 | 冷 P50／P95 |
  |---|---|---|---|---|
  | CCD0（3D V-Cache） | 1.00／1.83 ms | 1.66／2.54 ms | 1.31／4.42 ms | 2.15／5.09 ms |
  | CCD1 | 0.97／1.79 ms | 1.48／2.17 ms | 1.25／4.41 ms | 1.98／5.22 ms |
  | **CCD1＋搶快取** | 1.03／1.84 ms | 1.56／**2.29** ms | 1.34／4.56 ms | 2.24／**5.51** ms |

  | 條件 | 每個按鍵 P50／P95：Rust 引擎 | 原型＋ONNX Runtime |
  |---|---|---|
  | CCD0 | 1.27／2.12 ms | 2.02／5.08 ms |
  | CCD1 | 1.35／2.17 ms | 2.33／5.47 ms |
  | CCD1＋搶快取 | 1.50／2.32 ms | 2.53／5.69 ms |

  記憶體（charlm 載入並跑完全部呼叫後增加的 working set）：Rust +39 MB，ONNX Runtime +67 MB。
  採用標準（CCD1＋搶快取、冷的 P95 不比 ONNX Runtime 慢；記憶體不比它多）兩項都達成，熱與冷的 P50、P95 也都比較快。
  優化前同條件（CCD1）冷 P50／P95 為 3.65／5.85 ms。
- 沒有做：GELU 與注意力的 f64 向量化（加總順序會變，規格要求先證明必要）；改為與線性層一起分給兩個執行緒。
- 發現：serde_json 預設的浮點數解析可能差最後 1 位，改開 `float_roundtrip`（同一個 crate 的功能，不是新依賴）。
- 對照答案：以原型、`charlm` 指向 `charlm.bin` 在本機重新產生（原型 5.2 分鐘），五輪 **0 差異**；
  charlm 關閉的五輪仍 0 差異。
- 效能：每個音節約 3～4 ms（不含 charlm 時 < 0.2 ms）；載入 0.4 秒。
  記憶體：工作集 498 MB、尖峰 509 MB（charlm 約 +20 MB；n-gram 模型本來就有 481 MB），略超過 500 MB 目標。
- `eval` 命令（`eval.rs`）：三種系統（`bopogram`、`word-unigram`、`char-unigram`）、`--style`、
  `--charlm-model`／`--charlm-weight`／`--charlm-top`／`--no-charlm`、`--errors`、`--json`，計分與輸出格式照原型。
  神經模型相關選項（`--neural-*`、`--dump-nbest`、`--beam`、`--nbest`）沒有移植，使用時報錯請改用原型。
  測試句集 513 句，五種設定（charlm 正式／口語、不用 charlm、兩個對照組）與原型的錯誤清單、指標、
  列印的表格完全相同（char-unigram 用測試句集自身的字頻當替代字頻表，只為比對兩邊）。

  | 設定 | 首選句正確率 | 每音節延遲 P50／P95（Rust） | 原型（numpy 版 charlm） |
  |---|---|---|---|
  | charlm，正式 | 92.98% | 4.3／6.7 ms | 23.4／32.4 ms |
  | charlm，口語 | 93.76% | 4.2／6.6 ms | 23.6／32.3 ms |
  | 不用 charlm | 91.62% | 0.10／0.32 ms | 0.38／1.33 ms |
  | word-unigram 對照組 | 55.95% | 0.008／0.021 ms | |

### E6（2026-11-24）

- host 直接連結引擎函式庫：Rust 引擎在 `bopogram-host.exe` 裡的專用執行緒上執行（引擎的資料不跨執行緒共用），
  管線連線把請求經由 channel 交給它，`sid` 的路由不變。不再另開 `bopogram.exe` 行程，少一次管線往返。
- 協定處理抽成 `bopogram::server::Server`（`handle_line`），`bopogram serve` 與 host 共用同一份程式；
  對照答案兩組五輪仍 0 差異。
- Python 原型仍可選（`"engine": "python"`），留作與參考實作比對；打字不再需要 Python。
- 結束：引擎在 host 裡，強制結束 host 會失去上次存檔後的學習。改為 host 建立停止事件
  `Local\bopogram-host-stop-<SID>`（預設安全性：只有該使用者與 SYSTEM 能設定，低完整性程序不能寫），
  安裝、切換、移除腳本設定它，引擎存好 `user.json` 後 host 自行結束；10 秒內沒結束才強制結束。
  沒有停止事件的舊版 host 照舊強制結束（其引擎子行程看到輸入結束會自行存檔）。
- 引擎的警告（模型檔過期、找不到 `charlm.bin`、詞庫略過的行）原本經由子行程的 stderr 寫進 `host.log`；
  host 是 GUI 程式沒有 stderr，改為把自己的 stderr 指到 `host.log`。
- 沙盒：打字、只有 host 一個行程、切換引擎時先存檔再結束、學習在切換後保留，都正常（使用者確認）。
- `bopogram-host.json` 的 `engine` 改為 `"rust"` 或 `"python"`（不再是執行檔路徑）。
  `bopogram.exe` 仍會安裝：安裝與切換前用它確認引擎可用，也提供 `eval`、`convert`、`build-lm`。

### 記憶體低於 500 MB（2026-11-25）

WSL 端移除 general 模型（7cc5777）後，引擎行程載入時 479 MB，打字後長到 551 MB，仍超過目標。
以計數的 allocator 找出持有者（量測程式已移除），三處修改，**結果不變**（對照答案五輪 0 差異、
charlm 測試向量 1,498 個逐位元相同、測試句集 93.0%）：

| 項目 | 修改 | 前 → 後 |
|---|---|---|
| n-gram 混合的分數快取 | 上限 300,000 → 50,000 筆（每筆連同狀態約 130 位元組；每個按鍵只查幾千筆，延遲在雜訊內） | 打字後增加約 40 MB → 數 MB |
| 字典 | 每個讀音原本存成 `Vec<String>` 加標籤字串，改為音節 id（`Box<[u32]>`）與標籤編號，需要時才轉回文字 | 94 → 35 MB |
| charlm 權重 | 區塊內的線性層只用 VNNI 的重排版本時，不再保留原始排列（輸出層與 embedding 共用、需要逐列讀取，仍保留） | 29 → 19 MB |
| 解碼器的候選快取 | 原本只在詞庫改變時清空，改為超過 4,096 筆清空 | 少數 MB |

整體（`serve`，charlm 開啟，測試句集與開發集各打兩遍）：載入後工作集 479 → **422 MB**，
打字後 551 → **441 MB**，之後不再增長。其餘主要是 n-gram 模型（ly 127 MB、fineweb_small 185 MB、web 8 MB）。

### charlm 分級（2026-11-25，`prototype/docs/charlm-tiers.md`）

- 字表：與模型同名的 `.json`（`charlm-lite.bin` → `charlm-lite.json`），沒有才用同資料夾的 `charlm.json`
  （`charlm::table_path`）。字表的 `rerank.weight`／`rerank.top` 是該級的重排設定；`config.json` 的 `charlm`
  寫了 `weight`／`top` 才覆寫，都沒有時用 0.5 與 5。`eval` 的 `--charlm-weight`／`--charlm-top` 同樣可省略。
- `charlm-check`、`charlm-bench` 改為接受模型檔（或含 `charlm.bin` 的資料夾）。
- 維度、層數、head 數本來就從檔頭讀；運算核心不分等級。
- 前綴快取：最多 32 個序列，且保留的位置（共用的前綴只算一次）不超過 20 MB，大的等級保留較少序列；不影響分數。
- 驗證：
  - 輕量級測試向量（Windows 上以參考實作重新產生）1,498 個逐位元相同，經前綴快取也相同。
    WSL 產生的 `charlm_vectors_lite.jsonl` 有 13 個序列差 1e-14 級，與先前標準級一樣來自 WSL 的數學函式庫。
  - 標準級向量逐位元相同；對照答案（`config.json` 不寫權重，由字表提供 0.5／5）五輪 0 差異。
  - 輕量級測試句集 92.8%（權重 0.3，取自字表），錯誤清單與原型逐句相同。
- 量測（同 `charlm-optimization.md` 的方式，Rust 2 個執行緒）：

  | 條件 | 等級 | 每次呼叫 熱 P50／P95 | 冷 P50／P95 | 每個按鍵 P50／P95 |
  |---|---|---|---|---|
  | CCD0 | 輕量 | 0.47／0.75 ms | 0.88／1.40 ms | 0.61／1.02 ms |
  | CCD0 | 標準 | 0.94／1.74 ms | 1.66／2.56 ms | 1.23／2.13 ms |
  | CCD1 | 輕量 | 0.46／0.77 ms | 0.79／1.26 ms | 0.66／1.13 ms |
  | CCD1 | 標準 | 1.07／1.77 ms | 1.55／2.33 ms | 1.44／2.25 ms |
  | CCD1＋搶快取 | 輕量 | 0.46／0.76 ms | 0.89／1.33 ms | 0.71／1.22 ms |
  | CCD1＋搶快取 | 標準 | 1.16／1.99 ms | 1.60／2.33 ms | 1.47／2.30 ms |

  charlm 的記憶體（載入並跑完全部呼叫後增加的工作集）：輕量 +16 MB、標準 +28 MB。
  原型（numpy）每個按鍵 P50：輕量 7.2 ms。

### 微調的標準級、進階級與正式模式的的／地／得（2026-11-26）

- 重排新增規則（同 `engine.py` 的 `rescore`，見 `prototype/docs/charlm-native.md`）：正式模式下，若選中的結果與
  n-gram 第一名長度相同、而且只在兩邊都是「的／地／得」的位置不同，改選 n-gram 第一名（`engine::only_de`）。
- 標準級換成微調版（`charlm.bin`），新增進階級（`charlm-advanced.bin`，31 MB）。兩者的測試向量都是在 Windows 上重新產生的，
  逐位元相同，經過前綴快取也相同；WSL 產生的向量有 1e-14 級的差異（標準 18 個、進階 4 個），原因同前。
- 評估（Rust，與 `charlm-tiers.md` 的表相同）：

  | | 標準 | 進階 |
  |---|---|---|
  | 正式：測試集／開發集 | 93.4%／91.6% | 93.4%／91.8% |
  | 口語：測試集／開發集 | 94.2%／92.2% | 94.2%／92.5% |
  | 每個音節 P50／P95 | 1.4／2.2 ms | 2.5／3.7 ms |
  | 記憶體（載入／跑完測試集） | 422／441 MB | 439／459 MB |

### Linux（2026-11-27）

- 引擎不需修改即可編譯成 Linux 版。在 Windows 上交叉編譯成靜態執行檔，不需要 Linux 工具鏈：
  `rustup target add x86_64-unknown-linux-musl`，再執行
  `CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld cargo build --release -p bopogram --target x86_64-unknown-linux-musl`。
- WSL 上驗證：對照答案全部 0 差異，測試集正式 93.4%、口語 94.2%，與 Windows 相同。
  每個音節 P50 約 2.1 ms，比 Windows 慢約五成，原因是 musl 的記憶體配置器；改用 glibc 編譯應可消除。
- 非 Windows 平台上，工作集量測回傳 0；打字紀錄的時間用 UTC（標準函式庫沒有時區）。

### 對照答案（2026-11-26）

- Windows 上的對照答案（15:25）比新規則早，用它比對會有 99 處不同，列出的部分全部只差在的／地／得；請 WSL 端複製重新產生的版本過來。

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

### build-dict（2026-11-27，`prototype/docs/release-spec.md`）

- `bopogram build-dict MOE_DIR EXTRA_TSV OUT_DIR`：`training/dict/build_dict.py` 的 `build` 步驟（`dictbuild.rs`）。
  讀教育部三部辭典的 `.xlsx`，解壓過的或直接在下載的 `.zip` 裡（同名時取 zip 內的，同 Python），加上 `dict_extra.tsv`。
- Python 的 dict／Counter 保留插入順序，有幾個步驟依賴它（輕聲另讀的嘗試順序、推斷讀音時同票的排名），
  這裡用保留插入順序的 `Ordered` 重現；正規式與 `str.strip()`／`split()` 的空白定義照 Python。
- 驗證：Windows（只有 zip）與 Linux（WSL，解壓過的與 zip 並存）的 `dict.tsv`、`rejected.tsv`、`need_reading.tsv`
  與 Python 版逐位元相同；`stats.json` 數字相同，只有 `len_*` 的順序不同（Python 從集合計算，順序不固定）。
  2.1 秒（Python 9.7 秒）。
- 新的相依套件：calamine（.xlsx）、zip（只開解壓縮），已登記於 `THIRD_PARTY.md`。

### 模型包（2026-11-27，`prototype/docs/release-spec.md`）

- `bopogram pack verify|prepare|switch|install|rollback|add-tier|tier|status [--root DIR]`（`pack.rs`）。
  最後一行輸出是 JSON（`{"ok": ...}`），給設定程式與安裝程式讀。
- 資料夾 ROOT（Windows `%LOCALAPPDATA%\Bopogram\data`，Linux `~/.local/share/bopogram`）：
  `models/`（使用中的模型包，加上在這裡建的 `dict.tsv` 與引擎設定 `engine.json`）、`models-previous/`、`userdata/`。
- `engine.json` = 模型包的 `config.json`（不改，SHA-256 才對得上），個人資料改指 `../userdata/`，
  沿用舊 `engine.json` 的學習、打字紀錄、詞庫與 charlm 等級；新模型包沒有該等級時退回標準級（`tier_fell_back`）。
  打字紀錄預設關閉。輕量／進階級（`bopogram-charlm` 包）裝在 `models/charlm/`，更新模型包時沿用。
- 檢查：`format`、`engine`（`>=X.Y`，與程式版本比較，workspace 版本改為 0.2.0）、每個檔案的大小與 SHA-256、
  zip 內不得有清單外的檔案、路徑不得跳出資料夾。自我檢查：不載入個人資料，`selftest.json` 的 40 句要完全相同。
- `switch` 在引擎執行中也能做（Rust 開檔允許改名）：舊引擎繼續用已移到 `models-previous` 的檔案，重新啟動引擎後載入新的。
- 驗證（Windows 與 Linux）：安裝 3.8 秒（解壓、SHA-256、建字典、自我檢查 40/40），建出的 `dict.tsv` 與 Python 版逐位元相同；
  更新後等級保留、回到上一版、加入輕量級並切換；竄改一個位元組、清單外的檔案、`../` 路徑、需要更新版程式的包都會被拒絕，
  失敗時不留下 `models-new`。
- `make_model_pack.py` 在 Windows 上產生的模型包曾因 `manifest.json` 寫成反斜線而被拒絕，已在 92c11de 修正；
  發布用的模型包由 `installer/release.ps1` 以修正後的版本產生，並經 `pack verify` 檢查。
