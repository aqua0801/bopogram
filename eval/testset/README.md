# 測試句集

`testset_draft.tsv` 是**草稿**，經審閱確認前不作為正式評估依據。

## 格式

UTF-8、Tab 分隔，第一行為欄位名稱：

| 欄位 | 說明 |
|---|---|
| `id` | 編號。字首代表類別：H 同音字、P 破音字、T 台灣與中國用語、L 長句、R 遠距離依賴、D 日常對話 |
| `category` | `homophone`、`polyphone`、`tw_cn`、`long`、`longdist`、`daily` |
| `context` | 上文（已送出的文字），可空白 |
| `text` | 正確的文字，只含中文字，不含標點 |
| `note` | 備註：測試重點、破音字的指定讀音（如 `長=ㄓㄤˇ`）、來源（`MozTW:檔名`） |

注音不存在檔案中，評估時由字典從 `text` 產生；破音字依 `note` 指定的讀音。

`note` 結尾有 `〔語料重複〕` 的句子，表示整句曾原樣出現在訓練語料中（多半是「你知道嗎」這類短句）。
評估時會分別報告「全部」與「排除語料重複」兩種分數。

## 來源

- H、P、T、L、R 類與 D001–D060：為本專案撰寫。
- D061 以後：取自 MozTW 華語句庫（CC0），固定亂數種子抽選。
- 句庫其餘句子在 `eval/devset/moztw_dev.tsv`，只用於開發時調整權重，不用於報告分數，與本測試集不重疊。

重新產生：`python3 eval/tools/build_testset.py --dict <dict.tsv> --moztw <cc0-sentences/zh-TW> --corpus <corpus>`
