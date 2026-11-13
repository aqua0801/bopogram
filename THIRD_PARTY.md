# 第三方程式、資料與模型

所有第三方項目都記錄於此：名稱、來源網址、授權、用途。
語料只用於本機訓練，原文不隨專案散布。

## 程式庫與工具

| 名稱 | 來源 | 授權 | 用途 |
|---|---|---|---|
| OpenCC | https://github.com/BYVoid/OpenCC | Apache-2.0 | 訓練階段：語料簡轉繁、轉台灣用語（`training/`） |
| mwparserfromhell | https://github.com/earwig/mwparserfromhell | MIT | 訓練階段：解析維基標記（`training/`） |
| antiword | http://www.winfield.demon.nl/ | GPL-2.0-or-later | 訓練階段：以外部命令列工具將 `.doc` 轉純文字，不連結、不散布 |
| unar（The Unarchiver） | https://theunarchiver.com/command-line-tools | LGPL-2.1-or-later | 訓練階段：以外部命令列工具解壓 RAR，不連結、不散布 |

## 語料

| 名稱 | 來源 | 授權 | 用途 |
|---|---|---|---|
| 中文維基百科 dump（2026-11-01） | https://dumps.wikimedia.org/zhwiki/ | CC BY-SA 4.0、GFDL | 語言模型訓練語料（低權重） |
| 中文維基新聞 dump（2026-11-02） | https://dumps.wikimedia.org/zhwikinews/ | CC BY 2.5 | 語言模型訓練語料 |
| 立法院公報原始檔案（資料集 41） | https://data.ly.gov.tw/getds.action?id=41 | 立法院網站資料開放宣告（可重製、改作，須註明出處） | 語言模型訓練語料（口語逐字稿）。資料來源：立法院開放資料服務平台 |
| 司法院裁判書（資料集分類 051） | https://opendata.judicial.gov.tw/ | 司法院網站資料開放宣告（可重製、改作，須註明出處） | 語言模型訓練語料（法律文書，佔比上限 15%）。資料來源：司法院資料開放平台。依「Delete-Infor」清單剔除已撤下的裁判書 |
| 行政院開放資料集與本院新聞（即時新聞、重要政策、院會決議、新聞稿） | https://www.ey.gov.tw/opendata/ 、 https://www.ey.gov.tw/Page/6485009ABEC1CB9C | 政府資料開放授權條款－第1版（須註明出處） | 語言模型訓練語料（新聞體）。資料來源：行政院全球資訊網 |
| twngrams（臺灣華語 1–4 詞組網站數統計，v0.1.0） | https://huggingface.co/datasets/taiwan-corpora/twngrams | CC0 1.0 | 第 2 階段：詞表來源與驗證（只有統計數字，不含原文） |
| MozTW 華語句庫（cc0-sentences，`zh-TW/` 內的句子檔） | https://github.com/moztw/cc0-sentences | CC0 1.0 | 不進訓練語料；保留作為測試句集草稿的素材與開發用驗證集。不使用 `gpt-4.txt`（模型生成）與 repo 內的輸入法表格檔 |

## 待查證（尚未使用）

- 教育部辭典（字典來源，CC BY-ND；採「本機建置、不散布成品」方案）
