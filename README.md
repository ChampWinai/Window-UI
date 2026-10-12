# Fix UI

ปรับแต่ง desktop ของ Windows แบบ Seelen UI ทำด้วย Tauri 2 + Rust (windows-rs) + HTML/JS ล้วน

- **แถบ** แยกทุกจอ เลือกได้ 2 แบบ (tray → Bars)
  - *Taskbar* แถบเดียว ตำแหน่งและหน้าตาตามธีม (Windows 11, macOS, GNOME, KDE)
  - *Toolbar + Dock* toolbar ด้านบน (desktop, layout, ชื่อหน้าต่าง, tray, นาฬิกา) + dock ด้านล่าง
- **แอป** จัดกลุ่มตามแอป ปักหมุดได้ (คลิกขวา) คลิกกลางเปิดหน้าต่างใหม่
- **Tiling อัตโนมัติ** 6 layout: dwindle, master-stack, columns, rows, grid, monocle
  maximize หน้าต่างเพื่อให้ลอยออกจาก layout
- **Virtual desktop** สลับ / เพิ่ม (+) / ลบ (คลิกขวา) / เลื่อนล้อเมาส์
- **วอลเปเปอร์** รูปเดียว หรือโฟลเดอร์สไลด์โชว์ (แต่ละจอสุ่มภาพของตัวเอง) ผ่าน tray → Wallpaper
- **System tray** ไอคอนแอปอื่น, ภาษา, Bluetooth, เครือข่าย, เสียง, แบตเตอรี่, การแจ้งเตือน + ปฏิทิน
- ซ่อน taskbar ของ Windows ทุกจอ คืนค่าเมื่อ Quit

## คีย์ลัด

| คีย์ | ทำอะไร |
|---|---|
| `Alt+Space` | launcher ค้นหาหน้าต่าง |
| `Alt+Shift+T` | จัด tiling ใหม่ทุกจอ |
| `Alt+Shift+L` | เปลี่ยน layout |

## ตั้งค่า

เปิดแอป **Settings** จาก tray → Settings… หรือปุ่ม "Fix UI settings" ในแผงตั้งค่าด่วน
มีหน้า Appearance (ธีม, รูปแบบแถบ), Tiling, Wallpaper, Pinned apps (เรียง/ลบ), Shortcuts (กดคีย์ใหม่ได้เลย) และ General
เปลี่ยนแล้วมีผลทันที ค่าผิดหรือคีย์ลัดชนกับโปรแกรมอื่นจะแจ้งเตือน

ทุกค่าเก็บอยู่ที่ `%APPDATA%\com.fixui.desktop\config.yaml` (แก้มือได้ แล้วรีสตาร์ทโปรแกรม)

## วิดเจ็ต

วิดเจ็ตอยู่บน desktop หลังหน้าต่างอื่น แต่ละตัวคือโฟลเดอร์ใน `%APPDATA%\com.fixui.desktop\widgets\<id>\`
(มีตัวอย่าง `clock`, `system`, `media` ให้ลอกไปแก้) เปิด/ปิดได้ใน Settings → Widgets

```yaml
# widget.yaml
name: My widget
description: อธิบายสั้นๆ
width: 300          # px
height: 140
corner: top-right   # top-left | top-right | bottom-left | bottom-right
x: 24               # ระยะจากมุมนั้น
y: 24
```

`index.html` เขียน HTML/CSS/JS ได้เลย (inline ได้) แล้วใส่ `<script src="/_api.js"></script>` เพื่อใช้:

| API | ได้อะไร |
|---|---|
| `FixUI.stats()` | `{ cpu, memory, memory_used, memory_total }` |
| `FixUI.system()` | เสียง, เครือข่าย, แบตเตอรี่, Bluetooth, ภาษา, จำนวนแจ้งเตือน |
| `FixUI.media()` | เพลงที่กำลังเล่น `[{ app, title, artist, playing }]` |
| `FixUI.mediaThumb(app)` | ปกอัลบั้ม (data URL) |
| `FixUI.mediaControl(app, 'prev' \| 'next' \| 'toggle')` | ควบคุมเพลง |

วิดเจ็ตรันใน iframe แบบ sandbox: เรียกคำสั่งอื่นของแอปไม่ได้ และต่อเน็ตไม่ได้ แก้ไฟล์แล้วกด "Reload widgets"

## อัปเดตอัตโนมัติ

1. แก้ `version` ใน `src-tauri/tauri.conf.json` (เช่น 0.2.0) แล้ว commit
2. `npm run release -- https://github.com/ChampWinai/Window-UI/releases/download/v0.2.0`
   → ได้ตัวติดตั้งที่เซ็นแล้ว + `latest.json` ใน `src-tauri/target/release/bundle/nsis/`
3. `gh release create v0.2.0 "src-tauri/target/release/bundle/nsis/Fix-UI_0.2.0_x64-setup.exe" src-tauri/target/release/bundle/nsis/latest.json`
4. URL อัปเดต (ตั้งไว้ใน Settings → Updates ครั้งเดียวพอ):
   `https://github.com/ChampWinai/Window-UI/releases/latest/download/latest.json`

แอปจะเช็กตอนเปิด ถามก่อนติดตั้ง และตรวจลายเซ็นทุกครั้ง
กุญแจเซ็นอยู่ที่ `~/.tauri/fix-ui.key` (นอกโปรเจค) **ห้ามทำหาย และห้าม commit** ถ้าหาย เวอร์ชันที่ติดตั้งไปแล้วจะรับอัปเดตไม่ได้

## สิ่งที่ต้องติดตั้งก่อน

- Rust (rustup) ใช้ toolchain MSVC
- Visual Studio Build Tools เลือก workload "Desktop development with C++" (มี Windows SDK รวมอยู่แล้ว)
- Node.js

## รัน

```
npm install
npm run dev      # โหมดพัฒนา
npm run build    # ได้ตัวติดตั้ง NSIS ที่ src-tauri/target/release/bundle/nsis
cd src-tauri && cargo test   # เทสต์ layout ของ tiling
```

ปิดโปรแกรมด้วย tray → Quit เพื่อให้ taskbar ของ Windows กลับมาทันที
