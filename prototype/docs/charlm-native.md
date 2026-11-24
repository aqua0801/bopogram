# 字級語言模型（charlm）的自有格式與計算規格

不依賴 ONNX Runtime 執行 charlm。參考實作是 `prototype/bopogram/charlm_native.py`（numpy），
Rust 引擎照本文件實作，用 `prototype/tools/charlm_vectors.py` 產生的測試向量驗收。

## 檔案

| 檔案 | 位置 | 說明 |
|---|---|---|
| `charlm.bin`、`charlm.json` | `~/bopogram-data/neural/export/charlm_native/` | 權重與字表。字表與 ONNX 版的 `charlm.json` 相同 |
| `training/neural/export_charlm_native.py` | repo | 從 PyTorch 檢查點匯出 |
| `charlm_vectors.jsonl` | `~/bopogram-data/golden/` | 測試向量 |

模型：6 層 pre-LN Transformer，`d_model` 384、6 個 head、`d_ff` 1536、字表 9,944、最長 128 個位置，
輸出層與字元 embedding 共用權重。

### `charlm.bin`

```
0   8 bytes   magic  "BPCLM" 00 00 01
8   u32 LE    標頭長度 n
12  n bytes   標頭（UTF-8 JSON）
    資料區從 12 + n 之後第一個 64 的倍數開始；每個張量的 offset 是相對資料區開頭，也對齊 64
```

標頭：`{"format": 1, "d_model", "n_layers", "n_heads", "d_ff", "n_chars", "max_len", "ln_eps": 1e-5,
"step", "tensors": [{"name", "dtype": "i8"|"f32", "shape", "offset"}]}`。張量都是 row-major、little-endian。

| 張量 | 型別 | 形狀 |
|---|---|---|
| `emb.q`、`emb.scale` | i8、f32 | [9944, 384]、[9944] |
| `pos` | f32 | [128, 384] |
| `{i}.norm1.w`、`{i}.norm1.b`、`{i}.norm2.w`、`{i}.norm2.b` | f32 | [384] |
| `{i}.qkv.q`、`.scale`、`.b` | i8、f32、f32 | [1152, 384]、[1152]、[1152] |
| `{i}.proj.q`、`.scale`、`.b` | | [384, 384]、[384]、[384] |
| `{i}.ff1.q`、`.scale`、`.b` | | [1536, 384]、[1536]、[1536] |
| `{i}.ff2.q`、`.scale`、`.b` | | [384, 1536]、[384]、[384] |
| `norm.w`、`norm.b` | f32 | [384] |
| `out_bias` | f32 | [9944] |

`{i}` 是 0 到 5。權重量化（匯出時）：每一列 `scale = max|列| / 127`（全 0 的列為 1），`q = round(w / scale)`，
round 是「0.5 遠離 0」。

## 輸入（與 ONNX 版相同，`charlm.py` 的 `score`）

1. 前文取最後 40 個字。`encode`：連續的數字變成一個 `<n>`，連續的英文字母變成一個 `<e>`，空白略過，
   不在字表的漢字（U+4E00–U+9FFF）是 `<x>`，其他是 `<p>`。
2. 每個候選：`seq = ([<s>] + encode(前文) + encode(候選) + [<s>])` 取最後 128 個；
   `first = max(0, len(seq) - len(encode(候選) + [<s>]) - 1)`。
3. 分數 = Σ ln P(seq[i+1] | seq[0..i])，i 從 `first` 到 `len(seq) - 2`。

## 計算

張量在步驟之間都是 **f32**。每個步驟內部用 **f64** 計算，結果捨入成 f32 一次。
原因：啟動值會量化成 8 位元，f32 最後一位的差異偶爾會讓某個值捨入到另一個整數，誤差一層層放大；
不同的加法順序（不同的實作、不同的批次形狀）在 f32 下會不同，在 f64 下再捨入成 f32 幾乎都相同。

| 步驟 | 定義 |
|---|---|
| embedding | `x[p] = f32(emb.q[id] × emb.scale[id]) + pos[p]`（乘法捨入成 f32 後，f32 相加） |
| LayerNorm | f64：`mean`、`var`（除以 n）、`(x − mean) / sqrt(var + eps) × w + b`，結果捨入成 f32 |
| 量化線性層 | 見下 |
| 注意力（每個 head，dh = 64） | 分數 `f32(f32(q·k) / sqrt(64))`（q·k 以 f64 加總）；因果遮罩（位置 j 只看 0..j）；`e = f32(exp(s − max))`、`p = f32(e / Σe)`（f64）；輸出 `f32(Σ p·v)`（f64 加總） |
| block | `x = x + proj(attn(qkv(LN1(x))))`；`x = x + ff2(GELU(ff1(LN2(x))))`；加法是 f32 |
| GELU | f64：`0.5·x·(1 + erf(x / √2))`，erf 用下面的公式，結果捨入成 f32 |
| 輸出 | `h = LN(x)`；`logits = out(h)`（量化線性層，權重 `emb.q`/`emb.scale`，bias `out_bias`）；f64：`logp = logit[目標] − logsumexp(logits)` |

**量化線性層** `y = W x + b`，W 是 `.q`（[out, in]）與 `.scale`：

1. 對每個位置的輸入向量 x（f32）：`sx = f32(max|x| / 127)`（f64 除法）；`sx` 為 0 時把除數當成 1。
   `qx = clamp(round(x / sx), −127, 127)`，除法用 f64，round 是「0.5 遠離 0」。
2. `acc = Σ qx·q`：整數，用 i32 累加（最大約 2,500 萬，不會溢位），結果完全精確。
3. `y = f32(acc × (sx × scale[j]) + b[j])`，全部以 f64 計算。

**erf**（Abramowitz & Stegun 7.1.26，誤差 < 1.5e-7），f64：

```
t = 1 / (1 + 0.3275911·|x|)
y = ((((1.061405429·t − 1.453152027)·t + 1.421413741)·t − 0.284496736)·t + 0.254829592)·t
erf(x) = sign(x)·(1 − y·exp(−x²))
```

## 前綴快取（建議，不影響結果）

因果模型在一個位置的狀態只取決於它之前的字，所以每個位置每一層的 K、V 與最後的 `h` 可以留著，
給開頭相同的下一個序列使用。參考實作保留最近 32 個序列，新序列找共同前綴最長的那一個，從分歧處開始算；
每個位置的 `ln P(下一個字)` 也一起保留，輸出層只算還不知道的位置。
同一次重排的候選共用前文與大部分組字內容，下一次按鍵又幾乎全部相同。
測試：有快取與完全重算的結果逐位元相同（200 組隨機序列）。

## 驗收

`charlm_vectors.jsonl` 每一行是一次重排呼叫：`context`、`texts`、`seqs`、`firsts`（先驗證編碼）、
`logp`（每個序列從頭計算）。第一行另有 `trace`：第一個序列在 embedding 之後與每一層之後的 `x`，
四捨五入到 6 位小數，用來找出兩邊從哪一步開始不同。

目標：`seqs`、`firsts` 與 `trace` 完全相同，`logp` 每個值差 ≤ 1e-9（輸出層的 logsumexp 可以用向量化的 `exp`）。
f64 的加法順序仍可能在極少數情況讓捨入成 f32 的結果不同；遇到時回報那一行，兩邊一起看。
優化方向與限制見 `charlm-optimization.md`。

## 準確率與速度（2026-11-24，與 ONNX Runtime 版比較）

同一個檢查點（`runs/charlm_fw/last.pt`），量化方式不同（自有格式是逐列、對稱），分數不完全相同。

| | ONNX Runtime int8 | 自有格式（numpy） |
|---|---|---|
| 測試集 513 句 | 93.2% | 93.0%（少 1 句） |
| 開發集 11,657 句 | 91.1% | 91.3% |
| 使用者打字紀錄 49 句 | 81.6% | 83.7% |
| 模型檔 | 18.6 MB | 14.9 MB |
| 每個音節（測試集，含 n-gram） | P50 1.9 ms、P95 3.8 ms | P50 22 ms、P95 43 ms |

與 PyTorch float32 原始模型相比，自有格式的分數比 ONNX Runtime int8 更接近。
numpy 版只是參考實作，速度不代表 Rust 版；Python 原型日常使用仍以 ONNX Runtime 版為預設
（執行包 `config.json` 的 `charlm.model` 指向 `.onnx`；指向 `charlm.bin` 就改用自有格式）。
