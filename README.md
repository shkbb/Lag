<div align="center">

<h1>Lag</h1>
<p>Instant replay and screen recorder for Windows</p>

[![Rust](https://img.shields.io/badge/Rust-stable-B7410E?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![Slint](https://img.shields.io/badge/Slint-1.18-2379F4?style=flat-square)](https://slint.dev/)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows_x64-0078D6?style=flat-square&logo=windows)](../../releases)
[![Releases](https://img.shields.io/github/v/release/shkbb/Lag?style=flat-square&color=success)](../../releases/latest)

<p>
  <a href="#features">Features</a> ·
  <a href="#installation">Installation</a> ·
  <a href="#building-from-source">Build from source</a> ·
  <a href="#license">License</a> ·
  <a href="#українською">Українською</a>
</p>

</div>

Lag keeps the last few minutes of your screen in memory. When something worth keeping happens, press the hotkey and Lag saves the last seconds or minutes as a clip. Nothing goes to disk until you save.

Version 2.0 is a rewrite in Rust; 1.x was written in C# with Avalonia. Your settings, clips and hotkeys carry over. On the same PC with the same settings (recording a 165 fps buffer of the desktop), 2.0 shows its window in 1.7 s instead of 2.7 s, uses about a quarter less CPU and memory, and installs as 27 files instead of 1101.

## Features

| | |
|---|---|
| Replay buffer | A rolling buffer in RAM. A global hotkey saves the last N seconds or minutes. You can pause the buffer with a button or a hotkey, and the paused stretch is left out of saved clips. |
| Capture | Windows Graphics Capture of a game window or a whole monitor, with variable frame rate and a real timestamp on every frame. |
| Encoding | H.264 or HEVC on NVENC (NVIDIA), AMF (AMD) or QSV (Intel). Without a hardware encoder Lag falls back to x264 on the CPU. It probes your GPU and picks the encoder itself. |
| Game detection | Finds the active game by GPU 3D load and Steam, and switches between the game and the desktop without stopping the buffer. |
| Audio | Game or system sound and the microphone, as separate named tracks or one mix. You can record only the apps you choose, use push-to-talk, and clean the mic with RNNoise noise suppression. |
| Presets | Performance, Balanced and Quality, tuned from your GPU, CPU and displays. |
| Overlays | Webcam (choose resolution and 30 or 60 fps) and a keyboard and mouse display, both recorded into the clip. A live FPS / CPU / GPU / RAM monitor stays on screen but never ends up in clips. |
| Library | Clips grouped by date, filtered by game, with search and favourites. Thumbnails keep the clip's own aspect ratio. |
| Player | Built on mpv, with hardware decoding. |
| Editor | Trim, cut out the middle, change speed, crop and reframe (9:16 included), rotate and flip, add text and colour filters, then export to video or GIF. |
| Montage | Put several clips on one strip, edit each one, split them with the blade, drag to reorder, and export them as a single video. The Import button opens any video file, not only Lag clips. |
| Screenshots | A separate hotkey saves a screenshot to the library. |
| Output | Resolution, frame rate, bitrate, codec, container (`mp4`, `mkv`, `mov`) and GPU adapter. |
| Updates | Lag checks GitHub Releases on launch and installs new versions silently. |
| Startup | Can start with Windows and start recording right away. The window keeps the size you gave it. |
| Themes | Dark, Light, System (follows Windows) and GTA VI. The app icon changes with the theme. |
| Languages | 17 |

## Installation

1. Open [Releases](../../releases).
2. Download `Lag-win-Setup.exe` from the latest release.
3. Run it. The setup wizard asks for a folder and a language.

Later updates download and install by themselves. If you are on 1.x, the 2.0 installer removes the old .NET files; settings and clips stay where they are.

A portable build, `Lag-win-Portable.zip`, is attached to each release as well. It does not update itself.

> **Windows SmartScreen:** the installer is not code-signed yet, so on first run Windows may call it an unrecognized app. Click More info, then Run anyway. Every release is built from the source in this repository.

## Building from source

The app lives in `lag-rs/`, a Cargo workspace:

| Crate | Contents |
|---|---|
| `lag-app` | The Slint interface, tray, hotkeys, library, player and editor pages |
| `lag-capture` | Screen capture, encoding and the replay buffer |
| `lag-audio` | Audio capture, mixing and RNNoise |
| `lag-player` | The libmpv player |
| `lag-core` | Settings, presets, languages |
| `lag-ffi` | Bindings to FFmpeg 7.1 and libmpv |

`InstantReplay/` holds the C# 1.x app. It is kept for reference and because it carries runtime files that 2.0 still uses: FFmpeg (`InstantReplay/ffmpeg/`), the RNNoise model (`InstantReplay/rnnoise/`), sounds and the icon.

You need:

- Windows 10 1903 or later, x64.
- [Rust](https://rustup.rs/). `lag-rs/rust-toolchain.toml` selects `stable-x86_64-pc-windows-gnu`, and rustup installs it on the first build.
- [WinLibs](https://winlibs.com/) (MinGW-w64, UCRT) for its `dlltool`. `lag-rs/.cargo/config.toml` points at the maintainer's copy, so change that path to yours. The `dlltool` from llvm-mingw produces import tables that crash the app at startup.
- A libmpv development build, for example `mpv-dev-x86_64` from [shinchiro/mpv-winbuild-cmake](https://github.com/shinchiro/mpv-winbuild-cmake/releases). Put `libmpv-2.dll`, `libmpv.dll.a` and `include/` into `lag-rs/vendor/mpv/`, or set `LAG_MPV_DIR` to the folder that has them.

FFmpeg and the RNNoise model are already in the repository. `LAG_FFMPEG_DIR` points the build at a different FFmpeg 7.1.

Build:

```bash
git clone https://github.com/shkbb/Lag.git
cd Lag/lag-rs
cargo build --release -p lag-app
```

The app ends up in `lag-rs/target/release/Lag.exe`. The build script copies the FFmpeg and libmpv DLLs, `ffmpeg/` and `rnnoise/` next to it, so you can run it from there.

To make the installer and the portable zip, install [Inno Setup 6](https://jrsoftware.org/isinfo.php) and run:

```powershell
powershell -ExecutionPolicy Bypass -File lag-rs\tools\package.ps1 -Version 2.0.0 -Installer -Portable
```

It checks that every runtime file is in place, strips debug info from the shipped `Lag.exe`, and writes `Lag-win-Setup.exe` and `Lag-win-Portable.zip` to `installer/Output/`.

## License

GNU General Public License v3.0. See [`LICENSE`](LICENSE).

---

## Українською

Lag тримає в пам'яті останні хвилини того, що відбувається на екрані. Коли стається щось варте збереження, натискаєте гарячу клавішу, і Lag зберігає останні секунди чи хвилини як кліп. На диск нічого не пишеться, доки ви не збережете.

Версія 2.0 переписана на Rust; 1.x була на C# з Avalonia. Налаштування, кліпи й гарячі клавіші переходять автоматично. На тому самому ПК з тими самими налаштуваннями (запис буфера робочого столу на 165 fps) 2.0 відкриває вікно за 1,7 с замість 2,7 с, бере приблизно на чверть менше процесора й пам'яті і встановлюється як 27 файлів замість 1101.

### Можливості

| | |
|---|---|
| Буфер повтору | Кільцевий буфер в оперативній пам'яті. Глобальна гаряча клавіша зберігає останні N секунд або хвилин. Буфер можна поставити на паузу кнопкою чи клавішею, і пауза не потрапляє в кліпи. |
| Захоплення | Windows Graphics Capture вікна гри або цілого монітора, зі змінною частотою кадрів і справжньою позначкою часу на кожному кадрі. |
| Кодування | H.264 або HEVC на NVENC (NVIDIA), AMF (AMD) чи QSV (Intel). Без апаратного кодека Lag переходить на x264 на процесорі. Відеокарту перевіряє і кодек обирає сам. |
| Визначення гри | Знаходить активну гру за навантаженням 3D на відеокарті та Steam і перемикається між грою й робочим столом, не зупиняючи буфер. |
| Звук | Звук гри чи системи і мікрофон, окремими підписаними доріжками або одним міксом. Можна записувати лише вибрані програми, вмикати push-to-talk і чистити мікрофон шумозаглушенням RNNoise. |
| Пресети | Продуктивність, Баланс і Якість, підібрані під вашу відеокарту, процесор і монітори. |
| Оверлеї | Вебкамера (роздільність і 30 чи 60 fps на вибір) і показ клавіатури з мишею, обидва записуються в кліп. Монітор FPS / CPU / GPU / RAM видно на екрані, але в кліпи він не потрапляє. |
| Бібліотека | Кліпи згруповані за датою, з фільтром за грою, пошуком і улюбленими. Мініатюри зберігають пропорції самого кліпу. |
| Програвач | Побудований на mpv, з апаратним декодуванням. |
| Редактор | Обрізка, вирізання середини, швидкість, кадрування й рефрейм (зокрема 9:16), поворот і віддзеркалення, текст і кольорові фільтри, експорт у відео або GIF. |
| Монтаж | Кілька кліпів на одній смузі: кожен редагується окремо, ріжеться лезом, перетягується, і все експортується одним відео. Кнопка «Імпортувати» відкриває будь-який відеофайл, не лише кліпи Lag. |
| Скріншоти | Окрема гаряча клавіша зберігає знімок екрана в бібліотеку. |
| Вихід | Роздільність, частота кадрів, бітрейт, кодек, контейнер (`mp4`, `mkv`, `mov`) і відеоадаптер. |
| Оновлення | Під час запуску Lag перевіряє GitHub Releases і тихо встановлює нові версії. |
| Запуск | Може стартувати разом з Windows і одразу починати запис. Вікно пам'ятає розмір, який ви йому дали. |
| Теми | Темна, світла, системна (за Windows) і GTA VI. Іконка програми змінюється разом з темою. |
| Мови | 17 |

### Встановлення

1. Відкрийте [Releases](../../releases).
2. Завантажте `Lag-win-Setup.exe` з останнього релізу.
3. Запустіть. Майстер встановлення спитає папку й мову.

Далі оновлення завантажуються й ставляться самі. Якщо у вас 1.x, інсталятор 2.0 прибере старі файли .NET, а налаштування й кліпи залишаться на місці.

До кожного релізу також додано портативну версію `Lag-win-Portable.zip`. Вона сама не оновлюється.

> **Windows SmartScreen:** інсталятор поки без цифрового підпису, тож при першому запуску Windows може назвати його нерозпізнаною програмою. Натисніть «Докладніше», потім «Виконати все одно». Кожен реліз зібраний з коду цього репозиторію.

### Збірка з коду

Програма лежить у `lag-rs/`, це Cargo workspace:

| Крейт | Що всередині |
|---|---|
| `lag-app` | Інтерфейс на Slint, трей, гарячі клавіші, сторінки бібліотеки, програвача й редактора |
| `lag-capture` | Захоплення екрана, кодування, буфер повтору |
| `lag-audio` | Захоплення звуку, змішування, RNNoise |
| `lag-player` | Програвач на libmpv |
| `lag-core` | Налаштування, пресети, мови |
| `lag-ffi` | Прив'язки до FFmpeg 7.1 і libmpv |

У `InstantReplay/` лежить C#-версія 1.x. Її залишено для довідки, а ще тому, що там файли, які 2.0 досі використовує: FFmpeg (`InstantReplay/ffmpeg/`), модель RNNoise (`InstantReplay/rnnoise/`), звуки й іконка.

Потрібно:

- Windows 10 1903 або новіша, x64.
- [Rust](https://rustup.rs/). `lag-rs/rust-toolchain.toml` вибирає `stable-x86_64-pc-windows-gnu`, rustup поставить його під час першої збірки.
- [WinLibs](https://winlibs.com/) (MinGW-w64, UCRT) заради його `dlltool`. У `lag-rs/.cargo/config.toml` прописано шлях на ПК автора, змініть його на свій. `dlltool` з llvm-mingw дає таблиці імпорту, через які програма падає на старті.
- Збірка libmpv для розробки, наприклад `mpv-dev-x86_64` з [shinchiro/mpv-winbuild-cmake](https://github.com/shinchiro/mpv-winbuild-cmake/releases). Покладіть `libmpv-2.dll`, `libmpv.dll.a` і `include/` у `lag-rs/vendor/mpv/` або вкажіть цю папку в `LAG_MPV_DIR`.

FFmpeg і модель RNNoise вже є в репозиторії. Через `LAG_FFMPEG_DIR` можна підставити інший FFmpeg 7.1.

Збірка:

```bash
git clone https://github.com/shkbb/Lag.git
cd Lag/lag-rs
cargo build --release -p lag-app
```

Програма з'явиться в `lag-rs/target/release/Lag.exe`. Скрипт збірки кладе поруч DLL FFmpeg і libmpv, а також `ffmpeg/` і `rnnoise/`, тож запускати можна звідти.

Щоб зробити інсталятор і портативний zip, встановіть [Inno Setup 6](https://jrsoftware.org/isinfo.php) і запустіть:

```powershell
powershell -ExecutionPolicy Bypass -File lag-rs\tools\package.ps1 -Version 2.0.0 -Installer -Portable
```

Скрипт перевіряє, що всі потрібні файли на місці, прибирає налагоджувальну інформацію з `Lag.exe` для релізу і кладе `Lag-win-Setup.exe` та `Lag-win-Portable.zip` в `installer/Output/`.

### Ліцензія

GNU General Public License v3.0. Деталі у файлі [`LICENSE`](LICENSE).
