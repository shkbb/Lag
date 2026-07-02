<div align="center">

<h1>Lag</h1>
<p><strong>Instant Replay & Screen Recorder</strong></p>

[![.NET 8](https://img.shields.io/badge/.NET-8.0-512BD4?style=flat-square&logo=dotnet)](https://dotnet.microsoft.com/)
[![Avalonia UI](https://img.shields.io/badge/Avalonia-11.2-8B5CF6?style=flat-square)](https://avaloniaui.net/)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows_x64-0078D6?style=flat-square&logo=windows)](../../releases)
[![Releases](https://img.shields.io/github/v/release/shkbb/Lag?style=flat-square&color=success)](../../releases/latest)

<p>
  <strong>Lag</strong> is a sleek, lightweight instant replay tool powered by a custom<br>
  <strong>native VFR capture engine</strong> (Windows Graphics Capture + hardware NVENC/AMF/QSV)<br>
  and a modern <strong>Avalonia UI</strong> with Glassmorphism design. Never miss an epic highlight again.
</p>

<p>
  <a href="#features">Features</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#installation">Installation</a> ·
  <a href="#building-from-source">Build from Source</a> ·
  <a href="#license">License</a> ·
  <a href="#lag---запис-миттєвих-повторів">Українська</a>
</p>

</div>

---

## Features

| Feature | Description |
|---|---|
| **Native VFR engine** | Smooth, high-FPS variable-frame-rate capture via Windows Graphics Capture (WGC) — true per-frame timestamps, no forced-duplicate stutter |
| **Smart Hardware Encoding** | Auto-picks the best encoder by probing your GPU — `H.264` / `HEVC` / `AV1` on `NVENC` (NVIDIA), `AMF` (AMD), `QSV` (Intel), or `x264` CPU fallback |
| **Automatic Game Detection** | Detects the active game by GPU 3D-engine usage + Steam (no denylist, no curated DB), and auto-switches between game and full-desktop capture without stopping the buffer |
| **Multi-track Audio** | Captures system/game sound **and** microphone — as separate tracks or a single mix. Per-app audio capture (record only chosen programs), selectable output device, push-to-talk, mono/stereo mic |
| **Mic Noise Suppression** | Built-in **RNNoise** neural noise suppression cleans your microphone live — background hum, fans, and keyboard clatter are filtered out of recordings and mic monitoring |
| **Hardware Presets** | One-click **Performance / Balanced / Quality** presets tuned to *your* machine — Lag probes your GPU, CPU, and displays and picks matching capture settings |
| **Built-in Clip Editor** | Trim, cut out the middle, change speed, crop & reframe, rotate/flip, add text captions, and apply colour filters — then export to video or **GIF**, all inside the app |
| **Global Hotkeys** | Save the last X seconds/minutes of gameplay with a single customizable keystroke |
| **Pause & Resume** | Pause the rolling buffer — by button or a configurable global hotkey — then resume; the paused stretch is seamlessly absent from saved replays |
| **Instant Screenshots** | Capture your screen to the library with a dedicated global hotkey |
| **Built-in Video Player** | Review your highlights directly inside the app — no external player needed |
| **Configurable Output** | Pick resolution, frame rate, bitrate, codec/vendor, container (`mp4` / `mkv` / `mov`), and GPU adapter |
| **Auto-Updater** | Velopack integration: checks GitHub Releases on launch and updates automatically |
| **Set and Forget** | Start with Windows, auto-start recording — configure once and forget |
| **Light & Dark Themes** | A sleek dark theme and a warm cream light theme, plus a **System** mode that follows Windows live — with smoothly animated controls throughout |
| **17 Languages** | Fully localized in 17 languages |

---

## How it works

Lag records into a rolling in-memory **replay buffer**, so when something epic happens you hit the hotkey and the last X seconds are saved instantly — nothing is written to disk until you ask for it.

The recorder is a **custom native VFR engine** (`Services/VfrCapture/`): it captures the game window (or a monitor) through **Windows Graphics Capture**, converts on the GPU, and encodes with hardware **NVENC / AMF / QSV** (or **x264** on the CPU) via the bundled **FFmpeg 7.1** libraries — writing true variable-frame-rate timestamps for buttery-smooth playback.

---

## Installation

> For gamers — no technical knowledge required.

1. Open the [**Releases**](../../releases) tab
2. Download the latest **`Lag-win-Setup.exe`**
3. Run the installer — the app installs instantly and launches
4. Future updates are downloaded and applied automatically

---

## Building from Source

> For developers who want to build or contribute to the project.

**Prerequisites:**
- [.NET 8 SDK](https://dotnet.microsoft.com/download)
- **Windows SDK 10.0.26100** (the project targets `net8.0-windows10.0.26100.0` to reach the newer WGC knobs; it still *runs* on Windows 10 1903 / 19041+)

> **Note:** The native **FFmpeg 7.1** runtime (`avcodec-61`, `avformat-61`, `avutil-59`, `swresample-5`, plus `ffmpeg`/`ffprobe`) used by the capture engine ships in the repository under `InstantReplay/ffmpeg/` and is bundled into the build automatically. No external binaries or extra downloads are required.

**Step 1 — Clone the repository:**

```bash
git clone https://github.com/shkbb/Lag.git
cd Lag
```

**Step 2 — Build & run:**

```bash
dotnet build InstantReplay/InstantReplay.csproj -c Release
dotnet run --project InstantReplay/InstantReplay.csproj -c Release
```

The build output (with the `ffmpeg/` runtime bundled) lands in:

```
InstantReplay/bin/Release/net8.0-windows10.0.26100.0/Lag.exe
```

---

## License

Distributed under the **GNU General Public License v3.0**.
See [`LICENSE`](LICENSE) for full details.

---

<div align="center">

<h1>Lag</h1>
<p><strong>Запис миттєвих повторів</strong></p>

</div>

**Lag** — це стильна та легка програма для запису ігрових моментів. Вона працює на власному **нативному VFR-рушії захоплення** (Windows Graphics Capture + апаратний NVENC/AMF/QSV) із сучасним **Avalonia UI** у стилі скломорфізму. Більше жоден епічний момент не буде втрачено.

---

## Можливості

| Функція | Опис |
|---|---|
| **Нативний VFR-рушій** | Плавний запис із високим FPS і змінною частотою кадрів через Windows Graphics Capture (WGC) — справжні часові мітки кожного кадру, без заїкань від дубльованих кадрів |
| **Апаратне прискорення** | Сам обирає найкращий кодек, перевіряючи вашу відеокарту — `H.264` / `HEVC` / `AV1` на `NVENC` (NVIDIA), `AMF` (AMD), `QSV` (Intel) або `x264` на процесорі |
| **Автовизначення гри** | Визначає активну гру за завантаженням 3D-рушія GPU + Steam (без чорних списків і баз даних) і автоматично перемикається між грою та записом усього робочого столу, не зупиняючи буфер |
| **Багатодоріжкове аудіо** | Записує звук системи/гри **та** мікрофон — окремими доріжками або одним міксом. Захоплення звуку обраних програм, вибір пристрою виводу, push-to-talk, моно/стерео мікрофон |
| **Шумозаглушення мікрофона** | Вбудоване нейромережеве шумозаглушення **RNNoise** очищує мікрофон наживо — фоновий гул, вентилятори та стукіт клавіатури фільтруються із запису та прослуховування мікрофона |
| **Апаратні пресети** | Пресети **Продуктивність / Баланс / Якість** в один клік, підібрані під *ваш* комп'ютер — Lag аналізує відеокарту, процесор і монітори та обирає відповідні налаштування запису |
| **Вбудований редактор** | Обрізка, вирізання середини, зміна швидкості, кадрування та рефрейм, поворот/віддзеркалення, текстові підписи й кольорові фільтри — з експортом у відео або **GIF**, прямо в програмі |
| **Глобальні гарячі клавіші** | Збережіть останні секунди або хвилини гри одним натисканням налаштованої клавіші |
| **Пауза та відновлення** | Призупиніть кільцевий буфер — кнопкою або налаштовуваною глобальною гарячою клавішею — і відновіть; паузований проміжок безшовно відсутній у збережених повторах |
| **Миттєві скріншоти** | Збережіть знімок екрана в бібліотеку окремою глобальною гарячою клавішею |
| **Вбудований програвач** | Переглядайте моменти прямо в програмі — без сторонніх плеєрів |
| **Гнучкий вихід** | Вибір роздільної здатності, частоти кадрів, бітрейту, кодека/виробника, контейнера (`mp4` / `mkv` / `mov`) та відеоадаптера |
| **Автооновлення** | Velopack перевіряє GitHub при запуску й оновлює програму автоматично |
| **Автоматизація** | Автозапуск разом з Windows та автоматичний початок запису |
| **Світла і темна теми** | Стильна темна тема й тепла кремова світла, плюс режим **«Системна»**, що слідує за Windows наживо — з плавно анімованими елементами інтерфейсу |
| **17 мов** | Повністю локалізовано 17 мовами |

---

## Як це працює

Lag записує у кільцевий **буфер повтору** в пам'яті, тож коли стається щось епічне — ви натискаєте гарячу клавішу, і останні X секунд зберігаються миттєво. На диск нічого не пишеться, доки ви самі не попросите.

Рекордер — це власний **нативний VFR-рушій** (`Services/VfrCapture/`): він захоплює вікно гри (або монітор) через **Windows Graphics Capture**, конвертує кадри на GPU й кодує апаратно через **NVENC / AMF / QSV** (або **x264** на процесорі) за допомогою вбудованих бібліотек **FFmpeg 7.1**, записуючи справжні часові мітки зі змінною частотою кадрів для ідеально плавного відтворення.

---

## Встановлення

> Для користувачів — технічні знання не потрібні.

1. Перейдіть на вкладку [**Releases**](../../releases)
2. Завантажте найновіший файл **`Lag-win-Setup.exe`**
3. Запустіть його — програма встановиться за секунду й запуститься
4. Усі майбутні оновлення завантажуватимуться автоматично

---

## Збірка з вихідного коду

> Для розробників, які хочуть зібрати або долучитися до проєкту.

**Вимоги:**
- [.NET 8 SDK](https://dotnet.microsoft.com/download)
- **Windows SDK 10.0.26100** (проєкт націлений на `net8.0-windows10.0.26100.0` заради новіших можливостей WGC; але *працює* на Windows 10 1903 / 19041+)

> **Примітка:** Нативний рантайм **FFmpeg 7.1** (`avcodec-61`, `avformat-61`, `avutil-59`, `swresample-5` та `ffmpeg`/`ffprobe`), який використовує рушій захоплення, лежить у репозиторії в папці `InstantReplay/ffmpeg/` і автоматично копіюється у вихідну директорію під час збірки. Жодних зовнішніх бінарників чи додаткових завантажень не потрібно.

**Крок 1 — Клонуйте репозиторій:**

```bash
git clone https://github.com/shkbb/Lag.git
cd Lag
```

**Крок 2 — Зберіть і запустіть:**

```bash
dotnet build InstantReplay/InstantReplay.csproj -c Release
dotnet run --project InstantReplay/InstantReplay.csproj -c Release
```

Зібраний застосунок (разом із рантаймом `ffmpeg/`) опиниться тут:

```
InstantReplay/bin/Release/net8.0-windows10.0.26100.0/Lag.exe
```

---

## Ліцензія

Поширюється за ліцензією **GNU General Public License v3.0**.
Детальніше — у файлі [`LICENSE`](LICENSE).
