# ONNX Runtime：E4 暫停的原因與待決定事項（給 WSL 端）

Windows 端 2026-11-24 整理。E4（Rust 引擎接上 charlm）需要 ONNX Runtime，準備過程中遇到兩個問題：
遙測，以及上游原始碼庫被下架。這份文件說明查到的事實、Windows 端做過的檢查，以及需要兩邊一起決定的事。

## 使用者的決定（2026-11-24）

1. charlm **照常使用 1.32.0**，並關閉遙測。
2. 原型在 `charlm.py`、`neural.py` 加上 `ort.disable_telemetry_events()`（**WSL 端處理**）。
3. Rust 引擎先做 E5（純 n-gram 接上前端），E4 之後再做：用 1.32.0，啟動時呼叫 `DisableTelemetryEvents`。
4. neural 比照 charlm 處理。

## 目前進度（Rust 引擎）

| 里程碑 | 狀態 |
|---|---|
| E1 字典、自有格式 n-gram、解碼器 | 完成，首選與原型相同 |
| E2 組字區、`serve` 協定 | 完成 |
| E3 使用者詞庫、個人化學習、打字紀錄 | 完成 |
| E4 charlm | **暫停**（本文件） |

對照答案以原型、`charlm: null` 在 Windows 重新產生後，五輪（testset、devset、editing、userdict、learning）
共 46,778 個請求 **0 差異**，含各輪結束時的使用者檔案。細節見 `engine-spec.md` 的「進度與發現」。

## 問題一：官方版本預設開啟遙測

ONNX Runtime 套件附的 `Privacy.md` 寫明：官方版本**預設開啟**遙測，會把使用資料送給 Microsoft。
專案規則是「輸入法不得包含任何網路連線、遙測或自動上傳功能」，所以要先處理。

Windows 端檢查了兩份 1.32.0 的 `onnxruntime.dll`：

| | NuGet `Microsoft.ML.OnnxRuntime` 1.32.0 | 原型 `prototype/.venv`（pip） |
|---|---|---|
| 來源 | api.nuget.org，SHA-512 與 NuGet 公布值相同 | PyPI，**2026-11-18 12:46 安裝** |
| 版本資訊 | 1.32.0.20261112.3.fab30a4 | 1.32.0.20261112.6.fab30a4 |
| SHA-256 | `348c3d29d6995eee37e3451825eaddb7ea7400be3afda6ca2286b12bc775f042` | `443c0065fcdba17618ddef05050308f4b471893c5c7da0337d6957c9073ad8a7` |
| 簽章 | Microsoft Corporation，有效 | Microsoft Corporation，有效 |
| 網路函式庫（winhttp、wininet、ws2_32） | 沒有引用 | 沒有引用 |
| 走 HTTPS 上傳的那套遙測（1DS） | 沒有 | 沒有 |
| Windows 事件紀錄（ETW：`EventRegister`、`EventWriteTransfer`） | **有** | **有** |

結論：DLL 本身不會連網。但它會寫 ETW 事件，依 `Privacy.md` 的說法，這些事件會不會被送到 Microsoft，
取決於 Windows 本身的診斷資料設定。

- 可以用 API 關掉：Python 是 `onnxruntime.disable_telemetry_events()`，C API 是 `DisableTelemetryEvents`。
  但文件說，關掉之前可能已經寫出一個初始化事件。
- **原型目前沒有關。** `prototype/bopogram/charlm.py` 和 `neural.py` 建立 `InferenceSession` 前都沒有呼叫。
  **建議：** 在兩處 `import onnxruntime as ort` 之後、建立任何 session 之前，呼叫一次
  `ort.disable_telemetry_events()`。不影響分數，對照答案不必重新產生。
- 完全不含遙測程式的做法，是用官方提供的 `--no_telemetry` 選項從原始碼編譯。使用者原本選了這個做法，但碰到了問題二。

## 問題二：上游原始碼庫被下架

- 大約 2026-11-17 到 11-18，`github.com/microsoft/onnxruntime` 連同 release 檔案一起消失，回 404。
  `onnxruntime-genai`、`onnxruntime-extensions`、`olive` 等相關原始碼庫也一樣。
- Microsoft 發言人的說法是：「已將部分儲存庫設為私有，以調查惡意內容。」除此之外沒有正式說明，
  **沒有公布哪些版本受影響**。
- 網路上有人推測和同一時期另一個 Microsoft PyPI 套件被植入竊取帳密的程式有關，但沒有證據把兩件事連起來。
- PyPI、NuGet、Maven 上的套件仍可下載。
- 被拿出來討論的替代原始碼都不可信：第三方 fork 無法確認是原封不動的副本，
  也有人提醒 SourceForge 鏡像的壓縮檔可能含惡意程式。

對本專案的影響：

1. 現在**沒有可信的原始碼**，所以無法自己編一份不含遙測的版本。
2. 1.32.0 的兩份 DLL 都是 11/12 從同一個 commit 編出來的，就在事件前幾天。兩份都有 Microsoft 的有效簽章，
   但在調查結果公布前，無法確定沒有問題。
3. 原型環境裡的那一份是 11/18 安裝的。Python 引擎開著 charlm（或 neural）時就會載入 ONNX Runtime。

來源：
[Hacker News 討論](https://news.ycombinator.com/item?id=50539610)、
[StableBuild 部落格](https://www.stablebuild.com/blog/onnx-runtime-disappeared-from-github-without-warning)、
[ScopeOne issue #30](https://github.com/Experimental-Microscopy-Lab/ScopeOne/issues/30)。

## Windows 端做了什麼、沒做什麼

- **有下載：** NuGet 的 `Microsoft.ML.OnnxRuntime` 1.32.0，只解出 x64 DLL、LICENSE、ThirdPartyNotices、Privacy.md，
  放在 `D:\bopogram-data\onnxruntime\1.32.0\`（不進 git）。套件本身已刪除。
- **沒有：**
  - 用這個 DLL 執行過任何東西；
  - 在 Rust 引擎加入 `ort`，也沒有改 `THIRD_PARTY.md`；
  - 改過使用者的執行包設定，或停掉任何程序。

## 需要決定的事

1. **charlm 是否繼續使用？** 在 Microsoft 公布結論前，選項有：
   - (a) 照常使用 1.32.0，加上 `disable_telemetry_events()`；
   - (b) 改用事件前較早發布的版本。但沒有原始碼庫可以比對，也無法確認哪些版本安全；
   - (c) 暫時關閉 charlm（執行包 `config.json` 的 `"charlm": null`），回到純 n-gram 的準確率。
     請 WSL 端提供關閉前後的測試集與開發集分數差，讓使用者判斷代價。
2. **原型要不要先加上 `disable_telemetry_events()`？** 建議要，不論問題一怎麼決定。
3. **Rust 引擎的方向：** Windows 端建議 E4 暫緩，先做 E5：把純 n-gram 的 Rust 引擎接上前端，
   在沙盒測試後交給使用者試用，並保留一鍵切回 Python 版。charlm 等上游有結論後再接。
   如果決定長期不用 charlm，E5 的 Rust 引擎就是正式版的選字核心。
4. **neural（第 5 階段的神經評分層）也用 ONNX Runtime，** 同樣受這次事件影響，一併考慮。

## 給 E4 恢復時的備註

- Rust 端預定用 `ort` 套件，並在執行時才載入 `onnxruntime.dll`，所以編譯時不下載任何東西。
  兩者使用前都會登記在 `THIRD_PARTY.md`。
- 啟動時呼叫 `DisableTelemetryEvents`，執行緒數和原型相同，讓分數盡量逐位一致。
- 驗收方式：用正式對照答案（含 charlm）比對五輪。目前純 n-gram 的 Rust 引擎有 3,133 個差異，
  全部來自 charlm 重新排序。
