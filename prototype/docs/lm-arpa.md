# 語言模型的 ARPA 檔（給 Rust 引擎）

## 位置

| 檔案 | 位置 | 說明 |
|---|---|---|
| `ly.arpa`、`web.arpa`、`fineweb_small.arpa` | `~/bopogram-data/dist/arpa/`（Windows 端複製到 `D:\bopogram-data\arpa\`） | 執行包 `config.json` 的 `models` 三個模型（general 已於 2026-11-25 移除，見下），檔名對應同名的 `.binary` |
| `training/lm/export_arpa.py` | repo | 產生上面的檔案 |

語料授權的關係，這些檔案和執行包一樣不進 git、不散布。

## 為什麼不能直接用訓練時的 ARPA

執行包的 `.binary` 是用 `build_binary -q 8 -b 8 trie` 建的：2-gram 與 3-gram 的機率、2-gram 的 back-off
都量化成每個表 256 個值。Python 引擎讀的是量化後的值，跟原始 ARPA 每個字最多差 0.19（log10），
用原始 ARPA 的引擎選字會不同，標準答案（`golden.md`）比對不會過。

`dist/arpa/` 的檔案是從 `.binary` 讀回來的值（做法見 `export_arpa.py` 開頭的說明），格式是標準 ARPA，
n-gram 與原始檔完全相同、順序相同。2 階以上的數值以「讀回 float32 正好是同一個值」的最短文字寫出：
以 f32 讀入（正確捨入），就與 KenLM 用的數值逐位元相同。

驗證方式：用 KenLM 載入匯出的 ARPA，與 `.binary` 對隨機組合的句子逐詞比較分數（每個模型 56 萬個詞）。

| 模型 | 不完全相同的詞 | 最大差 |
|---|---|---|
| web | 0 | 0 |
| ly | 7 | 6e-8 |
| fineweb_small | 33 | 1.2e-7 |
| general | 550 | 6e-8 |

剩下的差異都是 1 個 float32 單位，來自少數 back-off 值：有幾個相鄰的 float32 都符合所有觀察到的分數，
取了最接近估計值的一個。只有兩個候選分數幾乎完全相同時才可能影響名次。
如果對照答案因此出現差異，把那個請求告訴 WSL 端：從那一步的分數可以直接解出正確的值。

## 載入時可以注意的地方

- unigram 沒有量化，照檔案的值存成 f32。
- 2-gram、3-gram 的機率，每一階不同的值最多 256 個；2-gram 的 back-off 最多 256 個。
  載入時建立「值 → 索引」表，存 8 位元索引就完全不失真，記憶體與 KenLM 量化版相近。
  若自己再做一次量化（重新分組），值會改變，標準答案可能不過。
- 計分規則與 KenLM 相同：找得到最長的 n-gram 就用它的機率，找不到就加上 context 的 back-off 退一階，
  完全不在詞彙裡的詞用 `<unk>`。log10。加法以 f32 進行，順序是：先取找到的 n-gram 的機率，
  再從最短的 context 往長的依序加上 back-off（例如 3-gram 模型退到 unigram：`(P(w) + B(b)) + B(a b)`）。
  順序不同，結果可能差 1 個 float32 單位。
- 四個模型怎麼合併、權重、`<s>`／`</s>`、符號與英數字如何對應成模型的 token，以 Python 原型
  （`prototype/bopogram/lm.py`、`decoder.py`）為準。

## 2026-11-25：移除 general

general 的混合權重只有 2.4%，卻占全部 n-gram 約 10%（720 萬／7,340 萬）。
拿掉它、其餘三個權重按比例放大後，測試集 93.2%、開發集 91.1%，與原本完全相同（EM 重新估的權重也一樣），
開發集困惑度 609.6 → 595.7。目的是讓 Rust 引擎的記憶體低於 500 MB。
