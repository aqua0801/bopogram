# 第三方程式、資料與模型

所有第三方項目都記錄於此：名稱、來源網址、授權、用途。
語料只用於本機訓練，原文不隨專案散布。

## 程式庫與工具

Rust 程式的全部相依套件（含間接相依）及其授權列在 `crates/docs/licenses.md`（由 `cargo metadata` 產生），
由 `cargo deny check licenses`（`crates/deny.toml`）檢查只含允許的寬鬆授權。下表只列直接使用的主要項目。

| 名稱 | 來源 | 授權 | 用途 |
|---|---|---|---|
| OpenCC | https://github.com/BYVoid/OpenCC | Apache-2.0 | 訓練階段：語料簡轉繁、轉台灣用語（`training/`） |
| mwparserfromhell | https://github.com/earwig/mwparserfromhell | MIT | 訓練階段：解析維基標記（`training/`） |
| pyarrow | https://arrow.apache.org/ | Apache-2.0 | 訓練階段：讀取 parquet 資料檔（`training/`） |
| openpyxl | https://openpyxl.readthedocs.io/ | MIT | 字典建置：讀取教育部辭典的 xlsx 檔（`training/`） |
| KenLM | https://github.com/kpu/kenlm | LGPL-2.1 | n-gram 語言模型：訓練工具（`lmplz`、`build_binary`，自行編譯，不進 repo）與原型引擎的查詢函式庫（Python 套件 `kenlm`） |
| antiword | http://www.winfield.demon.nl/ | GPL-2.0-or-later | 訓練階段：以外部命令列工具將 `.doc` 轉純文字，不連結、不散布 |
| unar（The Unarchiver） | https://theunarchiver.com/command-line-tools | LGPL-2.1-or-later | 訓練階段：以外部命令列工具解壓 RAR，不連結、不散布 |
| windows（windows-rs） | https://github.com/microsoft/windows-rs | MIT OR Apache-2.0 | Windows 前端（`crates/`）：呼叫 Win32 API（具名管線、安全描述元、mutex；之後的 TSF） |
| serde_json | https://github.com/serde-rs/json | MIT OR Apache-2.0 | Windows 前端（`crates/`）：解析與產生協定的 JSON 行 |
| zbus（含 zvariant 等相依套件） | https://github.com/z-galaxy/zbus | MIT（相依套件多為 MIT OR Apache-2.0） | Linux 前端（`crates/bopogram-ibus`）：連到 IBus 的本機 D-Bus（unix socket），不連網路 |
| calamine | https://github.com/tafia/calamine | MIT | `bopogram build-dict`：讀教育部辭典的 .xlsx（在使用者電腦上建置字典） |
| zip（zip2） | https://github.com/zip-rs/zip2 | MIT（間接帶進 zlib-rs：Zlib） | `bopogram build-dict` 直接讀下載的 .zip；模型包解壓 |
| sha2（RustCrypto） | https://github.com/RustCrypto/hashes | MIT OR Apache-2.0 | 模型包：檢查每個檔案的 SHA-256 |
| encoding_rs（calamine 的相依套件） | https://github.com/hsivonen/encoding_rs | (Apache-2.0 OR MIT) AND BSD-3-Clause（BSD-3-Clause 為其附帶的 WHATWG 編碼資料） | 讀 .xlsx 時的文字編碼 |
| Inno Setup 6.7 | https://jrsoftware.org/isinfo.php | Inno Setup License；6.7 起標示「Non-commercial use only」，商業用途需另購授權 | Windows 安裝檔的製作工具；安裝檔內含它的安裝與解除安裝程式。本專案為非商業開源使用 |
| ChineseTraditional.isl（自行翻譯） | 由 Inno Setup 的 Default.isl 翻譯（`crates/installer/`） | 依 Inno Setup License：保留原著作權聲明並標明為修改版 | 安裝程式的繁體中文訊息 |
| NumPy | https://numpy.org/ | BSD-3-Clause | 訓練階段：神經模型訓練資料的陣列檔（`training/neural/`） |
| PyTorch（含其 CUDA 執行庫） | https://pytorch.org/ | BSD-3-Clause（CUDA 執行庫為 NVIDIA 專有授權，只在訓練機上使用，不散布） | 第 5 階段：訓練神經逐位置評分模型（`training/neural/`），不隨輸入法散布 |
| ONNX Runtime | https://github.com/microsoft/onnxruntime | MIT | 第 5 階段：在 CPU 上執行神經模型（原型引擎的選用依賴 `onnxruntime`；訓練端用其量化工具） |
| huggingface_hub | https://github.com/huggingface/huggingface_hub | Apache-2.0 | 訓練階段：下載 Hugging Face 上的資料檔（`training/pipeline/fineweb_extract.py`） |
| ONNX、ONNX Script | https://github.com/onnx/onnx 、 https://github.com/microsoft/onnxscript | Apache-2.0、MIT | 第 5 階段：把訓練好的模型匯出成 ONNX 檔（`training/neural/export_onnx.py`） |
| libchewing（新酷音核心，Ubuntu 套件 `libchewing3` 0.6.0 及其內附詞庫） | https://github.com/chewing/libchewing | LGPL-2.1-or-later | 只用於評估：以公開 C API 執行測試句集，當作對照組（`eval/tools/chewing_run.py`）。不讀其原始碼、不取用其詞庫或輸出做為訓練資料，也不隨專案散布 |

## 語料

| 名稱 | 來源 | 授權 | 用途 |
|---|---|---|---|
| 中文維基百科 dump（2026-11-01） | https://dumps.wikimedia.org/zhwiki/ | CC BY-SA 4.0、GFDL | 語言模型訓練語料（低權重） |
| 中文維基新聞 dump（2026-11-02） | https://dumps.wikimedia.org/zhwikinews/ | CC BY 2.5 | 語言模型訓練語料 |
| 立法院公報原始檔案（資料集 41） | https://data.ly.gov.tw/getds.action?id=41 | 立法院網站資料開放宣告（可重製、改作，須註明出處） | 語言模型訓練語料（口語逐字稿）。資料來源：立法院開放資料服務平台 |
| 司法院裁判書（資料集分類 051） | https://opendata.judicial.gov.tw/ | 司法院網站資料開放宣告（可重製、改作，須註明出處） | 語言模型訓練語料（法律文書，佔比上限 15%）。資料來源：司法院資料開放平台。依「Delete-Infor」清單剔除已撤下的裁判書 |
| 行政院開放資料集與本院新聞（即時新聞、重要政策、院會決議、新聞稿） | https://www.ey.gov.tw/opendata/ 、 https://www.ey.gov.tw/Page/6485009ABEC1CB9C | 政府資料開放授權條款－第1版（須註明出處） | 語言模型訓練語料（新聞體）。資料來源：行政院全球資訊網 |
| twngrams（臺灣華語 1–4 詞組網站數統計，v0.1.0） | https://huggingface.co/datasets/taiwan-corpora/twngrams | CC0 1.0 | 第 2 階段：詞表來源與驗證；n-gram 模型「web」由其詞組次數建立（`training/lm/twngrams_arpa.py`）。只有統計數字，不含原文 |
| MozTW 華語句庫（cc0-sentences，`zh-TW/` 內的句子檔） | https://github.com/moztw/cc0-sentences | CC0 1.0 | 不進訓練語料；保留作為測試句集草稿的素材與開發用驗證集。不使用 `gpt-4.txt`（模型生成）與 repo 內的輸入法表格檔 |
| TWLLM-Data（yentinglin/twllm-data，20250216） | https://huggingface.co/datasets/yentinglin/twllm-data | Apache-2.0 | 語言模型訓練語料：只取真實使用者的提問；模型生成的回答與合成提問子集（twllm-evol、twllm-synthetic）一律排除 |
| FineWeb2（HuggingFaceFW/fineweb-2，`cmn_Hani` 子集，抽取 10 個檔案） | https://huggingface.co/datasets/HuggingFaceFW/fineweb-2 | ODC-By 1.0，另須遵守 Common Crawl 使用條款（https://commoncrawl.org/terms-of-use）；網頁原文著作權屬原作者 | 字級語言模型訓練語料（一般網頁文字）。只留繁體、非香港、無簡轉繁痕跡的頁面；依 `training/pipeline/data/fineweb_exclude.txt` 排除使用條款或 robots.txt 禁止 AI 訓練的網站（2026-11-18 查核前 40 大網站）、盜版小說站與中國官方媒體；與開發集、測試集重疊的句子已剔除。2026-11-26 另抽取 1 個檔案（`000_00005`，同樣的篩選），只取部落格與論壇網域（`training/pipeline/fineweb_conv.py`），用於 charlm「挑候選」微調資料；n-gram 模型與 charlm 預訓練都不使用這個檔案 |

## 字典來源（只在本機建置，成品不散布）

| 名稱 | 來源 | 授權 | 用途 |
|---|---|---|---|
| 教育部《重編國語辭典修訂本》《國語辭典簡編本》《成語典》（2026-09-29 版） | https://language.moe.gov.tw/001/Upload/Files/site_content/M0001/respub/index.html | CC BY-ND 3.0 臺灣 | 注音字典的詞條與讀音。repo 只含建置腳本，字典成品不進 git、不散布。發布時附上**未改動的原始壓縮檔**與三份「公眾授權使用說明」（使用說明要求完整保留），在使用者電腦上建置字典（`prototype/docs/release-spec.md`）。出處格式依使用說明：中華民國教育部（Ministry of Education, R.O.C.）。《重編國語辭典修訂本》（版本編號：2015_20260929）網址：http://dict.revised.moe.edu.tw/ ；簡編本 http://dict.concised.moe.edu.tw/ 、成語典 http://dict.idioms.moe.edu.tw/ 同格式 |
| CNS11643 中文標準交換碼全字庫（屬性資料 `Properties.zip`、對照表 `MapingTables.zip`，版本 20261028） | https://data.gov.tw/dataset/5961 | 政府資料開放授權條款－第1版（須註明出處；資料集另提供 OFL-1.1 供擇一，本專案採前者） | 注音字典：補上教育部辭典沒有收錄的單字及其注音（來源標記 `cns`）。資料來源：數位發展部「全字庫」。只在本機建置時讀取，原始檔不進 repo |

## 待查證（尚未使用）

