# 使用者詞庫：引擎交接說明（草稿，待使用者審閱）

給 WSL 端（`prototype/`）的工作階段使用，之後的 Rust 引擎照同一份規格實作。
Windows 端（`crates/`）負責設定視窗的詞庫頁、組字中的加詞／刪詞快捷鍵與 host 的權限檢查。

**審閱狀態：** 本文件定義的新檔案格式（`userdict.tsv`、`hidden.tsv`）屬於字典格式的一部分，
新增的協定操作會改變前端與引擎的合約，兩者都要使用者同意後才實作。

## 目標

- 使用者可以加入字典裡沒有的詞（人名、專有名詞、自己的用語），加入後馬上能打出來，並且排在前面。
- 使用者可以刪掉不想看到的詞：不論來自字典還是個人化學習，之後都不再出現在候選裡。
- 加詞時不必自己打注音：引擎提供讀音，破音字列出讓使用者選。
- 組字中可以直接把目前的詞加入或刪除，讀音取自實際打的注音，不會選錯讀音。

## 檔案

兩個檔案都在執行包的 `userdata\`，屬於個人資料：只存在本機，不進 git，不匯出。
格式與 `dict.tsv` 相同（UTF-8 不含 BOM、`\n` 換行、Tab 分隔；空行與 `#` 開頭的行略過），
這樣 Python 與 Rust 可以共用同一個解析程式。

### `userdata/userdict.tsv`：使用者加入的詞

```
詞<Tab>注音<Tab>user
```

- 注音的每個音節以一個半形空白分隔，寫法與 `dict.tsv` 相同（例：`ㄅㄛ ㄆㄛ`，輕聲寫成 `˙ㄇㄜ`）。
- 一個字對一個音節：`len(詞) == len(音節)`。長度 1 到 `MAX_WORD_LEN`（目前是 8）。
- 同一個詞可以有多個讀音，一個讀音一行。完全相同的一行只保留一筆。
- 第三欄固定是 `user`，用來和字典的來源標記區分。

### `userdata/hidden.tsv`：被刪除的詞

```
詞<Tab>注音
詞
```

- 有注音時只隱藏那一個讀音；沒有注音（只有一欄）時隱藏這個詞的所有讀音。
- 被隱藏的詞不進候選格、不出現在候選清單，個人化學習也不再替它加分。

### `config.json`

可選的兩個鍵；沒有寫時用以下預設值，所以現有的執行包不必改：

```json
"user_dict": "userdata/userdict.tsv",
"hidden_words": "userdata/hidden.tsv"
```

設成 `null` 表示關閉這個功能。檔案不存在時視為空的；第一次寫入時才建立。

### 寫入規則

- **只有引擎寫這兩個檔案。** 前端與設定視窗一律透過協定操作修改，避免兩邊同時寫入。
- 寫法與 `user.json` 相同：先寫 `.tmp` 再 `os.replace`，避免檔案寫到一半。
- 每次修改都立即寫入，不延後；加詞與刪詞的頻率很低。
- 讀不了的行（欄位不對、讀音不合法）略過，記一行到 stderr（只記行號，不記內容），不要讓引擎啟動失敗。

## 選字規則

1. 候選格展開時，使用者詞與字典詞一起列入，扣掉被隱藏的詞。
2. 使用者詞要能贏：剛加入的詞，在沒有上文的情況下打它的完整讀音，首選就要是它。
   做法建議比照個人化學習：在語言模型分數之外加一個固定加分（`config.json` 的 `user_dict_bonus`，
   數值由 WSL 端調整）。不在語言模型裡的字會被當作未知詞，加分要足以抵銷這個懲罰。
3. 依 CLAUDE.md 的原則，神經層不得否決使用者詞：神經分數只是加權項之一，而且有下限。
4. 使用者詞之後的選擇與修改照常進個人化學習，跟字典的詞一樣。
5. 一個音節至少要留一個候選。如果隱藏某個詞會讓某個讀音沒有任何候選，就拒絕這次刪除
   （錯誤原因見下方的 `last_candidate`）。

## 協定：新增的操作

格式與現有的協定相同（一行一個 JSON，`id` 原樣帶回）。這些操作作用在整個引擎上，
不分 `sid`：在任一程式加的詞，所有程式立即可用。它們不進打字紀錄，也不是個人化學習的輸入。

| `op` | 其他欄位 | 作用與回應 |
|---|---|---|
| `readings` | `text` | 查一個詞可能的讀音。回應 `readings`：`[{"reading": "ㄓㄤˇ ㄉㄚˋ", "known": true}, ...]`，最可能的在前，最多 16 個。`known` 表示字典或使用者詞庫已經有這個詞與讀音。整個詞在字典裡就用字典的讀音；否則用每個字的讀音組合，依 `reading_prior` 排序 |
| `dict_add` | `word`、`reading` | 把詞加入使用者詞庫。這個詞與讀音若在 `hidden.tsv` 裡，一併移除（等於恢復） |
| `dict_add` | `start`、`length`（不帶 `word`） | 把目前組字區的一段加入詞庫，讀音取自這個 `sid` 實際打的音節。`start` 與 `length` 的意義與 `candidates` 的回應相同 |
| `dict_remove` | `word`，可加 `reading` | 刪除一個詞。使用者詞直接從 `userdict.tsv` 移除；字典的詞寫進 `hidden.tsv`。兩種情況都一併清掉 `user.json` 裡含這個詞的單詞與雙詞紀錄。回應 `removed`：`"user"` 或 `"hidden"` |
| `dict_remove` | `start`、`length`、`word` | 刪除候選視窗裡選中的詞，讀音取自這個 `sid` 的音節。帶上 `word` 是為了確認：組字區已經變了就回 `not_found` |
| `dict_restore` | `word`，可加 `reading` | 從 `hidden.tsv` 移除，恢復成字典裡的狀態 |
| `dict_list` | | 回應 `entries`：`[{"word", "reading", "kind": "user" 或 "hidden"}]`，給設定視窗顯示 |
| `dict_reload` | | 重新讀取兩個檔案（例如使用者手動編輯過之後） |

加入或刪除之後，回應照常帶這個 `sid` 的完整組字狀態（`preedit` 等欄位）。組字區不重新計算，
免得畫面突然變動；下一個按鍵時才用新的詞庫。

失敗時回應 `ok: false`，除了 `error`（給人看的說明）之外，另帶 `reason` 讓前端判斷：

| `reason` | 情況 |
|---|---|
| `bad_reading` | 讀音含有不合法的音節 |
| `length_mismatch` | 字數與音節數不同 |
| `too_long` | 超過 `MAX_WORD_LEN` |
| `duplicate` | 使用者詞庫已有同一筆（`dict_add`）。前端當作成功處理 |
| `not_found` | 要刪除或恢復的詞不存在，或組字區已經改變 |
| `last_candidate` | 隱藏後某個讀音會沒有任何候選 |
| `disabled` | `config.json` 把這個功能設成 `null` |

### 範例

```
→ {"op":"readings","text":"長大","id":1}
← {"ok":true,"readings":[{"reading":"ㄓㄤˇ ㄉㄚˋ","known":true},{"reading":"ㄔㄤˊ ㄉㄚˋ","known":true}],"id":1}
→ {"op":"dict_add","word":"波波","reading":"ㄅㄛ ㄅㄛ","id":2}
← {"ok":true,"preedit":"","pending":"","cursor":0,"syllables":0,"segments":[],"id":2}
→ {"op":"dict_remove","word":"視頻","id":3}
← {"ok":true,"removed":"hidden","preedit":"","pending":"","cursor":0,"syllables":0,"segments":[],"id":3}
```

## 前端負責的部分（Windows 端，供參考）

- 設定視窗新增「詞庫」頁：列出使用者詞與已刪除的詞，可以搜尋、加入（打字後讓使用者從 `readings` 的結果選讀音）、
  刪除、恢復。設定視窗自己連上 host 的管線，host 沒在執行時就先啟動它。
- 組字中的快捷鍵（預設鍵與使用者討論後決定，可在設定裡改）：
  - 把游標處的詞或選取的範圍加入詞庫（`dict_add` 帶 `start`／`length`）。
  - 在候選視窗裡刪除選中的候選（`dict_remove` 帶 `start`／`length`／`word`），刪除前先確認。
- host 拒絕來自 AppContainer 的 `dict_*` 修改操作，與「AppContainer 一律關閉學習」的規則一致。
  `readings` 與 `dict_list` 只讀，不在此限。
- 密碼欄位裡輸入法不處理按鍵，所以快捷鍵也不作用。
- `uninstall.ps1 -RemovePersonalData` 與設定的「清除學習」：前者要一併刪除這兩個檔案；
  後者只清除個人化學習，不動詞庫（兩者在設定裡分開說明）。

## 測試（WSL 端）

不需要模型的單元測試，可以和 `tests/test_engine.py` 放在一起：

1. 解析：註解、空行、壞行（壞行略過，引擎照常啟動）、BOM、只有一欄的 `hidden.tsv`。
2. 加詞：字典沒有的詞加入後，打它的完整讀音首選就是它；另開一個 `sid` 也能打出來。
3. 加詞的範圍形式：組字區「ㄅㄛ ㄅㄛ」的一段，讀音取自實際的音節（破音字用打的那個讀音）。
4. 刪除字典的詞：不再出現在候選；`dict_restore` 後恢復；刪除單一讀音時其他讀音不受影響。
5. 刪除時一併清除 `user.json` 中含這個詞的紀錄。
6. `last_candidate`：不能把某個讀音的最後一個候選刪掉。
7. 錯誤原因：`bad_reading`、`length_mismatch`、`too_long`、`duplicate`、`not_found`、`disabled`。
8. 寫入：中途失敗時原本的檔案不受影響（`.tmp` 加上 `os.replace`）。
9. `readings`：字典詞用字典的讀音；字典沒有的詞用逐字組合並依 `reading_prior` 排序；最多 16 個。

另外在 `eval/testset` 的評估上確認，空的詞庫不改變任何分數（加這個功能不能讓現有結果變差）。
