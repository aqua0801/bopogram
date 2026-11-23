# 語言模型的 ARPA 檔（給 Rust 引擎）

## 位置

| 檔案 | 位置 | 說明 |
|---|---|---|
| `ly.arpa`、`general.arpa`、`web.arpa`、`fineweb_small.arpa` | `~/bopogram-data/dist/arpa/`（Windows 端複製到 `D:\bopogram-data\arpa\`） | 執行包 `config.json` 的 `models` 四個模型，檔名對應同名的 `.binary` |
| `training/lm/export_arpa.py` | repo | 產生上面的檔案 |

語料授權的關係，這些檔案和執行包一樣不進 git、不散布。

## 為什麼不能直接用訓練時的 ARPA

執行包的 `.binary` 是用 `build_binary -q 8 -b 8 trie` 建的：2-gram 與 3-gram 的機率、2-gram 的 back-off
都量化成每個表 256 個值。Python 引擎讀的是量化後的值，跟原始 ARPA 每個字最多差 0.19（log10），
用原始 ARPA 的引擎選字會不同，標準答案（`golden.md`）比對不會過。

`dist/arpa/` 的檔案是從 `.binary` 讀回來的值（做法見 `export_arpa.py` 開頭的說明），
與 `.binary` 的分數差在 5e-6 以內（float 加法順序造成），格式是標準 ARPA，n-gram 與原始檔完全相同、順序相同。

## 載入時可以注意的地方

- unigram 沒有量化，照檔案的值存成 f32。
- 2-gram、3-gram 的機率，每一階不同的值最多 256 個；2-gram 的 back-off 最多 256 個。
  載入時建立「值 → 索引」表，存 8 位元索引就完全不失真，記憶體與 KenLM 量化版相近。
  若自己再做一次量化（重新分組），值會改變，標準答案可能不過。
- 計分規則與 KenLM 相同：找得到最長的 n-gram 就用它的機率，找不到就加上 context 的 back-off 退一階，
  完全不在詞彙裡的詞用 `<unk>`。log10。
- 四個模型怎麼合併、權重、`<s>`／`</s>`、符號與英數字如何對應成模型的 token，以 Python 原型
  （`prototype/bopogram/lm.py`、`decoder.py`）為準。
