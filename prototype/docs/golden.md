# 對照答案：確認其他引擎（Rust 版）與 Python 原型的行為一致

Python 原型對一系列協定請求的回應，全部錄下來當作標準答案。
Rust 引擎實作同一份協定（`prototype/bopogram/server.py`、`frontend-handoff.md`），
重播同樣的請求、得到完全相同的回應，就代表選字、候選清單、編輯規則、詞庫與個人化學習都一致。

## 檔案

| 檔案 | 位置 | 說明 |
|---|---|---|
| `golden.jsonl` | `~/bopogram-data/golden/`（Windows 端複製到 `D:\bopogram-data\golden\`） | 標準答案。含候選清單（來自字典），所以不進 git，與執行包一起複製 |
| `eval/tools/make_golden.py` | repo | 產生標準答案（只在 WSL 端執行） |
| `eval/tools/check_golden.py` | repo | 重播並比對。只用 Python 標準函式庫，Windows 上用一般 Python 3 即可 |

`golden.jsonl` 每一行是一個 JSON：

- `{"run", "setup": {"config", "files"}, "ready"}`：一輪的開始。`files` 是開始前 `userdict.tsv`、`excluded.tsv` 的內容，空字串表示沒有檔案。
- `{"run", "req", "res"}`：一個請求與它的回應（`req` 帶流水號 `id`）。
- `{"run", "end"}`：結束時 `user.json`、`userdict.tsv`、`excluded.tsv` 的內容（`null` 表示沒有檔案）。

每一輪都是重新啟動的引擎，使用者檔案放在新的暫存目錄，打字紀錄關閉。

| 輪 | 請求數 | 內容 |
|---|---|---|
| `testset` | 19,840 | 測試句集 513 句逐鍵輸入（正式模式、帶上文、學習關閉），結尾與開頭各取一次候選清單 |
| `devset` | 26,480 | 開發集前 1,000 句逐鍵輸入 |
| `editing` | 265 | 游標移動、刪除、選字、`cycle`、符號與替代符號、空白、兩種模式、變調、組字區過長自動送出、上文、多個 `sid`、錯誤請求 |
| `userdict` | 123 | 使用者詞庫：查讀音、加詞（兩種形式）、刪除、排除（含跨詞）、恢復、各種錯誤原因、結束時的檔案內容 |
| `learning` | 70 | 個人化學習：手動改選後送出、`learning` 關閉時不學、結束時的 `user.json` |

## 執行

```
python eval/tools/check_golden.py GOLDEN_JSONL RUNTIME_CONFIG --cmd "ENGINE --config {config} serve"
```

- `RUNTIME_CONFIG` 是執行包的 `config.json`。比對程式會在它旁邊寫一份暫時的設定（模型路徑照原本的方式解析），
  只把 `user_model`、`user_dict`、`excluded_words` 換成暫存目錄、`typing_log` 關閉，跑完即刪。
- `{config}` 會被換成那份暫時設定的路徑。引擎需要提供與 Python 版相同的命令列：`bopogram --config X serve`。
- `--runs testset,editing` 只跑部分輪次；`--show N` 每輪最多印出幾個差異。
- 全部一致時印出 `PASS`，結束碼 0；否則印出差異與 `FAIL`，結束碼 1。

比對規則：回應必須是相同的 JSON 物件（欄位與值都一樣，順序不計）；只有 `error` 的文字不比對，
但兩邊要同樣有或沒有 `error`。結束時 `user.json` 以 JSON 比對，兩個 `.tsv` 以文字比對。

## 目標與已知的例外

- 目標是 0 個差異。Python 原型重跑自己的標準答案為 0 個差異（2026-11-21），結果是確定的。
- 唯一可以接受的差異來源是字級模型（charlm）：兩邊用同一個 ONNX 檔，但不同的 ONNX Runtime 版本或執行緒數，
  浮點數可能有極小差異，讓兩個分數幾乎相同的結果互換名次。遇到差異時先關掉 charlm 比對
  （設定裡 `"charlm": null` 後重新產生與比對），差異消失才歸因於此。
- 引擎行為有意修改時（例如調整權重），在 WSL 端重新產生標準答案，並在 commit 訊息註明。

## 重新產生

```
cd ~/bopogram/eval
uv run --project ../prototype python tools/make_golden.py ~/bopogram-data/dist/bopogram-runtime/config.json ~/bopogram-data/golden
```

charlm 用自有格式（numpy 參考實作）時約 15–20 分鐘。每一輪完成就存在 `golden/parts/`，中斷後以同一個指令重跑，
已完成的輪次會略過（設定改變時自動從頭開始）；全部完成才合併成 `golden.jsonl`。
標準答案綁定執行包裡的字典與模型；執行包更新後要重新產生。
