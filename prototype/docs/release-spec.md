# 發布規格：安裝檔、模型包與字典（草稿）

2026-11-27 起草，使用者已同意方向：做安裝檔；模型以手動下載的「模型包」更新；
字典附教育部原始檔、在使用者電腦上建置；charlm 以 CC BY-SA 4.0 發布；接受 FineWeb2 網頁文字的風險。

## 原則

- **輸入法與安裝程式都沒有網路功能。** 不自動檢查、不自動下載更新。使用者自己從 GitHub Releases 下載，
  再交給安裝程式或設定程式。
- 程式與模型分開發布，各有版本號；更新其中一個不用重裝另一個。
- 不改動使用者資料（`user.json`、`userdict.tsv`、`excluded.tsv`、打字紀錄）。
- 散布的每個檔案都要有出處與授權聲明（`NOTICE`），見最後一節。

## 發布的東西

| 包 | 內容 | 平台 | 大小（約） | 由誰產生 |
|---|---|---|---|---|
| 程式安裝檔 | 引擎、前端、host、設定程式、`bopogram` 命令列；Rust 相依套件的授權全文（`cargo-about`） | Windows x64 安裝檔；Linux x64 `.deb` 與 `.tar.gz` | 數 MB | Windows 端 |
| 模型包 `bopogram-models-<版本>.zip` | n-gram（`.bglm`）、charlm 標準級、讀音機率表、字典建置用的資料（見下）、`manifest.json`、`NOTICE` | 共用 | 壓縮前約 400 MB | WSL 端產生內容，Windows 端打包工具可共用 |
| charlm 額外等級 `bopogram-charlm-<等級>-<版本>.zip` | 輕量或進階級的 `.bin`／`.json`、`manifest.json`、`NOTICE` | 共用 | 6／31 MB | 同上 |

安裝檔可以把程式與模型包合成一個「完整安裝檔」方便第一次安裝；之後的更新各自獨立。

## 字典：附原始檔，在使用者電腦上建置

教育部《重編國語辭典修訂本》《國語辭典簡編本》《成語典》都是 CC BY-ND 3.0 臺灣，另附「公眾授權使用說明」：
允許重製、散布（含商業利用），但**不得修改個別條目**的詞目、音讀、釋義等內容；
**散布時必須完整保留使用說明**並確認版本。整理過的 `dict.tsv` 不能散布，所以：

- 模型包附上教育部的**原始壓縮檔，一個位元組都不改**，以及三份使用說明 PDF：

  | 檔案 | 使用說明 |
  |---|---|
  | `dict_revised_2015_20260929.zip` | `reviseddict_10312.pdf` |
  | `dict_concised_2014_20260929.zip` | `conciseddict_10312.pdf` |
  | `dict_idioms_2020_20260929.zip` | `idiomsdict_10409.pdf` |

  （WSL：`~/bopogram-data/raw/moe/`；來源 https://language.moe.gov.tw/001/Upload/Files/site_content/M0001/respub/index.html ）
- 其他來源允許改作，由 WSL 端**預先處理成一個檔案** `dict_extra.tsv` 放進模型包：
  CNS 11643 全字庫補的單字（政府資料開放授權，須註明出處）、twngrams 補的常用詞（CC0）與其讀音規則、
  本專案的手動讀音表、注音符號本身。這些都不是教育部的內容。
- **安裝時**（以及更新模型包時）在使用者電腦上執行 `bopogram build-dict`：讀三個原始壓縮檔與 `dict_extra.tsv`，
  離線產生 `dict.tsv`。教育部內容的整理只發生在使用者自己的電腦上。
- `build-dict` 是 `training/dict/build_dict.py` 的 Rust 移植。驗收：同樣的輸入，輸出的 `dict.tsv` 與 Python 版**逐位元相同**。
  需要讀 `.xlsx`：建議 `calamine`（MIT），採用前登記於 `THIRD_PARTY.md`。
  WSL 端先把 `build_dict.py` 拆成「讀教育部檔案」與「其他來源 → `dict_extra.tsv`」兩段，讓 Rust 只需要移植前者。

## 模型包格式

```
bopogram-models-2026.12/
  manifest.json
  NOTICE                      出處與授權（見最後一節）
  LICENSE-models              CC BY-SA 4.0 全文（charlm）
  config.json                 執行設定（模型路徑、權重等）
  models/ly.bglm  web.bglm  fineweb_small.bglm
  charlm/charlm.bin  charlm.json
  reading_prior.tsv
  dict-source/                教育部原始壓縮檔與三份使用說明（不改動）、dict_extra.tsv
```

`manifest.json`：

```json
{"format": 1, "name": "bopogram-models", "version": "2026.12",
 "engine": ">=0.2", "created": "2026-12-01",
 "files": [{"path": "models/ly.bglm", "size": 128727061, "sha256": "..."}, ...]}
```

`engine` 是需要的最低程式版本（模型格式變了才提高）。

## 更新流程（設定程式或安裝程式）

1. 使用者選擇下載好的 `.zip`。
2. 解壓到暫存資料夾，檢查 `manifest.json` 的 `format`、`engine` 版本、每個檔案的大小與 SHA-256。不符就停止並說明原因。
3. 執行 `bopogram build-dict` 產生 `dict.tsv`。
4. 自我檢查：用新模型載入引擎，轉換幾句固定的測試句（例如「我再看看好了」），結果要和模型包附的預期結果相同。
5. 通過才切換：舊版本整個保留（`models-previous/`），新版本換上，host 重新載入引擎。
6. 設定程式可以「回到上一版」。

使用者資料與等級設定在切換前後不變；若新模型包沒有使用者選的 charlm 等級，退回標準級並告知。

## NOTICE（WSL 端產生，模型包與安裝檔都附上）

| 項目 | 授權 | 必須寫的出處 |
|---|---|---|
| 教育部三部辭典（原始檔，未改動） | CC BY-ND 3.0 臺灣＋使用說明 | 依各使用說明，例如：中華民國教育部（Ministry of Education, R.O.C.）。《重編國語辭典修訂本》（版本編號：2015_20260929）網址：http://dict.revised.moe.edu.tw/ （簡編本、成語典同格式，網址見使用說明） |
| CNS 11643 全字庫 | 政府資料開放授權條款－第 1 版 | 資料來源：數位發展部「全字庫」 |
| 立法院公報 | 立法院網站資料開放宣告 | 資料來源：立法院開放資料服務平台 |
| 司法院裁判書 | 司法院網站資料開放宣告 | 資料來源：司法院資料開放平台 |
| 行政院開放資料與新聞 | 政府資料開放授權條款－第 1 版 | 資料來源：行政院全球資訊網 |
| FineWeb2 | ODC-By 1.0（另須遵守 Common Crawl 使用條款） | HuggingFaceFW/fineweb-2；網頁原文著作權屬原作者 |
| 中文維基百科、維基新聞 | CC BY-SA 4.0、CC BY 2.5 | 維基百科、維基新聞的貢獻者 |
| TWLLM-Data | Apache-2.0 | yentinglin/twllm-data |
| twngrams | CC0 | taiwan-corpora/twngrams |

- **charlm 模型檔**（各等級）以 **CC BY-SA 4.0** 授權發布，因為訓練語料包含 CC BY-SA 的維基百科；附授權全文與上表的出處。
- **n-gram 模型**與讀音機率表：附上表中對應來源的出處（立法院、FineWeb2、twngrams）。
- 程式碼：MIT（`LICENSE`）；Rust 相依套件的授權全文由安裝檔附上。

## 已知的灰色地帶（使用者已知悉）

- FineWeb2 是網頁文字，原文著作權屬各網站作者；已排除宣告禁止 AI 訓練的網站。
- n-gram 模型的詞彙表與讀音機率表，用到了字典中的詞目與讀音（統計數字，不含釋義、不改動條目內容）。

## 分工與順序

1. WSL：拆分 `build_dict.py`、產生 `dict_extra.tsv`、`NOTICE`、模型包內容與 `manifest.json`、自我檢查用的預期結果。
2. Windows：`bopogram build-dict`（Rust 移植，與 Python 版逐位元相同）、模型包的驗證與切換、回到上一版、安裝檔（Windows、Linux）、
   Rust 相依套件授權全文、設定程式的介面。
3. 程式碼簽章（Windows SmartScreen 會警告未簽章的安裝檔）另外決定。
