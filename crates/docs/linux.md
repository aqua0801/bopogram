# Linux 前端（IBus，2026-11-27）

## 架構

- `bopogram-ibus`：IBus 引擎程式。ibus-daemon 依 `/usr/share/ibus/component/bopogram.xml` 以 `--ibus` 啟動它；
  它連上 IBus 自己的 D-Bus（本機 socket），在 `/org/freedesktop/IBus/Factory` 提供 `CreateEngine`，
  每個輸入區建立一個引擎物件（`/org/freedesktop/IBus/Engine/Bopogram/N`）。
- Rust 引擎在同一個程序裡、自己的執行緒上執行（同 Windows host），各輸入區用各自的 `sid`；
  引擎在背景載入（約 2–3 秒），載入中的按鍵由 controller 暫存再送。
- 按鍵規則：`bopogram-controller`（原 `bopogram-ime/src/controller.rs` 拆出），Windows 與 Linux 共用，行為一致。
  `keys.rs` 把 X 的 keysym 與修飾鍵狀態轉成 controller 使用的 Windows 虛擬鍵碼（台灣鍵盤即美式鍵盤）。
- 協定：與 Windows 相同的 JSON 請求（`bopogram-protocol`），只是不經管線，直接交給程序內的 `Server`。
- 沒有網路：唯一的連線是 IBus 的本機 socket。D-Bus 用 zbus（純 Rust）。

## 與 IBus 的對應

| IBus | 做法 |
|---|---|
| `ProcessKeyEvent` | 按下：`key_down`；放開：`key_up`（單按 Shift 切換中英）。按下被吃掉的鍵，放開也吃掉。 |
| 組字區 | `UpdatePreeditText`，整段加底線，模式為 commit：失去焦點或 `Reset` 時由 IBus 自己把顯示的文字送進文件，引擎同時結束組字（學習、保留上下文），不重複送出。 |
| 選字窗 | `UpdateLookupTable` 一次送一頁；`UpdateAuxiliaryText` 顯示頁碼（1/24）與提示（加入詞庫、中英、全半形、口語／正式）。 |
| 滑鼠點選、面板翻頁 | `CandidateClicked`、`PageUp`／`PageDown`、`CursorUp`／`CursorDown`。 |
| 密碼欄位 | `SetContentType`（或 `ContentType` 屬性）的用途為 password（8）或 PIN（9）時停用，不處理、不學習。 |
| 游標前文字 | 應用程式支援 surrounding text 時，開始組字前以 `SetSurroundingText` 的游標前文字為上下文；否則用本輸入法送出過的內容。 |

序列化格式（IBusText、IBusAttrList、IBusAttribute、IBusLookupTable）見 `bopogram-ibus/src/values.rs`。

## 資料與記錄

- 資料夾：`$BOPOGRAM_DATA`，否則 `~/.local/share/bopogram/`（config.json、字典、模型、charlm、userdata）。
  `keys.json`（與 Windows 同格式）放在同一個資料夾可改選字鍵、每頁個數等。
- 使用者模型在 10 秒沒有按鍵後寫入。
- 記錄：`~/.cache/bopogram/ibus.log`，只記啟動、連線、建立與結束引擎、錯誤，不記打的字；超過 1 MB 重新開始。

## 編譯（在 Windows 上）

```
rustup target add x86_64-unknown-linux-musl
set CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld
cargo build --release -p bopogram -p bopogram-ibus --target x86_64-unknown-linux-musl
```

在 Windows 上建置整個 workspace 時，`bopogram-ibus` 只編譯 `--check` 用的部分（D-Bus 只在 unix）。

## 測試

- `bopogram-ibus --check`：不需要 IBus，以 keysym 經 controller 打三句並送出，再試單按 Shift 切英文。
- 以私有 dbus-daemon 模擬 ibus-daemon（WSL）：建立引擎、送按鍵、以 GLib 解析每個訊號的型別；
  組字、送出、選字窗（含翻頁與頁碼）、密碼欄位、失去焦點、`Destroy` 都符合預期。實際的 IBus 與 GNOME 待 VM 測試。

## 安裝（VM）

測試包的 `install.sh`：資料複製到 `~/.local/share/bopogram/`，程式放 `/usr/local/lib/bopogram/`，
元件檔放 `/usr/share/ibus/component/bopogram.xml`（後兩者要 sudo），再 `ibus restart`。
`uninstall.sh` 移除程式與元件檔，資料夾預設保留（`--data` 一併刪除）。
