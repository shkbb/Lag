```markdown
# Lag - Instant Replay & Screen Recorder

![.NET 8](https://img.shields.io/badge/.NET-8.0-512BD4?style=flat-square&logo=dotnet)
![Avalonia UI](https://img.shields.io/badge/Avalonia-11.0-purple?style=flat-square)
![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg?style=flat-square)
![Platform](https://img.shields.io/badge/Platform-Windows_x64-0078D6?style=flat-square&logo=windows)

> **Lag** is a sleek, lightweight, and modern instant replay software powered by the rock-solid **OBS Studio core** (`libobs`) and a beautiful **Avalonia UI** (Glassmorphism). Never miss an epic gaming highlight again.

[Українська версія нижче](#lag---запис-миттєвих-повторів)

---

## Features

* **Premium Glassmorphism UI:** A stunning, modern interface featuring Windows 11 Mica/Acrylic materials, smooth animations, and Material vector icons.
* **Smart Hardware Acceleration:** Automatically detects and uses the best available video encoder on your system (`NVENC` for NVIDIA, `AMF` for AMD, `QSV` for Intel, or `x264` fallback) for zero-lag recording.
* **Global Hotkeys:** Save your last X seconds/minutes of gameplay instantly with a single customizable keystroke.
* **Built-in Video Player:** Review your saved highlights directly inside the app.
* **Auto-Updater:** Integrated Velopack system. The app automatically checks for GitHub releases and updates itself seamlessly in the background.
* **Set and Forget:** Support for "Start with Windows" and "Auto-start recording".
* **Bilingual:** Native English and Ukrainian localization.

## Screenshots

*(Replace these links with actual screenshots of your app later)*
> ![App Screenshot](https://via.placeholder.com/800x450.png?text=Lag+Settings+UI)

## Installation (For Gamers)

1. Go to the [Releases](../../releases) tab.
2. Download the latest **`Lag-Setup.exe`**.
3. Run the installer. The app will install instantly, create a desktop shortcut, and launch.
4. Future updates will be downloaded automatically.

## Building from Source (For Developers)

This project requires the `obs-core` native binaries (`libobs`, `FFmpeg`) to run, which are **not included** in this repository due to their size (~400MB). 

**Step-by-step build guide:**
1. Clone the repository:
```bash
   git clone [https://github.com/shkbb/Lag.git](https://github.com/shkbb/Lag.git)
   cd Lag

```

2. Build the project once to generate the output directories:

```bash
   dotnet build

```

3. Download the necessary `obs-core` binaries (you can extract them from an existing OBS Studio installation or a pre-compiled libobs package).
4. Place the entire `obs-core` folder into your build output directory:
`Lag/bin/Debug/net8.0-windows/win-x64/obs-core/`
5. Run the app:

```bash
   dotnet run

```

## License

This project is licensed under the **GNU General Public License v3.0**. See the `LICENSE` file for details.

---

---

# Lag - Запис миттєвих повторів

> **Lag** — це стильна, легка та сучасна програма для запису ігрових повторів. Вона поєднує в собі надійність ядра **OBS Studio** (`libobs`) та красу **Avalonia UI** (Скломорфізм). Більше жоден епічний ігровий момент не буде втрачено.

## Головні можливості

* **Преміальний дизайн (Скломорфізм):** Сучасний інтерфейс з використанням ефектів напівпрозорості Windows 11 (Mica/Acrylic), плавними анімаціями та векторними Material-іконками.
* **Розумне апаратне прискорення:** Програма сама знаходить найкращий кодек вашої відеокарти (`NVENC` для NVIDIA, `AMF` для AMD, `QSV` для Intel або `x264` для процесора), щоб записувати відео без жодних лагів.
* **Глобальні гарячі клавіші:** Зберігайте останні секунди або хвилини гри одним натисканням налаштованої клавіші.
* **Вбудований програвач:** Переглядайте свої найкращі моменти прямо в програмі, не відкриваючи сторонні плеєри.
* **Система автооновлень:** Інтегрований рушій Velopack автоматично завантажує нові версії програми з GitHub і оновлює її у фоновому режимі.
* **Автоматизація:** Підтримка автозапуску разом з Windows та автоматичного початку запису.
* **Локалізація:** Повна підтримка англійської та української мов.

## Скріншоти

*(Тут будуть скріншоти програми)*

## Встановлення (Для користувачів)

1. Перейдіть на вкладку [Releases](https://www.google.com/search?q=../../releases).
2. Завантажте найновіший файл **`Lag-Setup.exe`**.
3. Запустіть його. Програма встановиться за секунду, створить ярлик і запуститься.
4. Усі майбутні оновлення завантажуватимуться автоматично.

## Збірка з вихідного коду (Для розробників)

Для роботи програми потрібні нативні бінарні файли `obs-core` (`libobs`, `FFmpeg`). Через їхній великий розмір (~400 МБ) вони **не включені** у цей репозиторій.

**Інструкція для збірки:**

1. Клонуйте репозиторій:

```bash
   git clone [https://github.com/shkbb/Lag.git](https://github.com/shkbb/Lag.git)
   cd Lag

```

2. Зберіть проєкт один раз, щоб створилися необхідні папки:

```bash
   dotnet build

```

3. Знайдіть необхідні бінарники `obs-core` (можна взяти з папки встановленої OBS Studio або завантажити зібраний пакет libobs).
4. Помістіть всю папку `obs-core` у вихідну директорію вашої збірки:
`Lag/bin/Debug/net8.0-windows/win-x64/obs-core/`
5. Запустіть програму:

```bash
   dotnet run

```

## Ліцензія

Цей проєкт поширюється за ліцензією **GNU General Public License v3.0**. Детальніше читайте у файлі `LICENSE`.

```

```
