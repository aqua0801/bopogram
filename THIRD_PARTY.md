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

## 待查證（尚未使用）

- 司法院裁判書（https://opendata.judicial.gov.tw/）
- 行政院新聞稿、公視新聞
- 教育部辭典（字典來源，CC BY-ND；採「本機建置、不散布成品」方案）
