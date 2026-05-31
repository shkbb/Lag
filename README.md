<div align="center">

<h1>Lag</h1>
<p><strong>Instant Replay & Screen Recorder</strong></p>

[![.NET 8](https://img.shields.io/badge/.NET-8.0-512BD4?style=flat-square&logo=dotnet)](https://dotnet.microsoft.com/)
[![Avalonia UI](https://img.shields.io/badge/Avalonia-11.0-8B5CF6?style=flat-square)](https://avaloniaui.net/)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows_x64-0078D6?style=flat-square&logo=windows)](../../releases)
[![Releases](https://img.shields.io/github/v/release/shkbb/Lag?style=flat-square&color=success)](../../releases/latest)

<p>
  <strong>Lag</strong> is a sleek, lightweight instant replay tool powered by the <strong>OBS Studio core</strong> (<code>libobs</code>)<br>
  and a modern <strong>Avalonia UI</strong> with Glassmorphism design. Never miss an epic highlight again.
</p>

<p>
  <a href="#features">Features</a> ·
  <a href="#installation">Installation</a> ·
  <a href="#building-from-source">Build from Source</a> ·
  <a href="#license">License</a> ·
  <a href="#lag---запис-миттєвих-повторів">Українська</a>
</p>

</div>

---

## Screenshots

> *(Add your screenshots here)*

![App Screenshot](https://via.placeholder.com/900x500.png?text=Lag+Settings+UI)

---

## Features

| Feature | Description |
|---|---|
| **Glassmorphism UI** | Windows 11 Mica/Acrylic effects, smooth animations, and Material vector icons |
| **Smart Hardware Encoding** | Auto-detects the best encoder — `NVENC` (NVIDIA), `AMF` (AMD), `QSV` (Intel), or `x264` fallback |
| **Global Hotkeys** | Save the last X seconds/minutes of gameplay with a single customizable keystroke |
| **Built-in Video Player** | Review your highlights directly inside the app — no external player needed |
| **Auto-Updater** | Velopack integration: checks GitHub Releases and updates silently in the background |
| **Set and Forget** | Start with Windows, auto-start recording — configure once and forget |
| **Bilingual** | Full English and Ukrainian localization |

---

## Installation

> For gamers — no technical knowledge required.

1. Open the [**Releases**](../../releases) tab
2. Download the latest **`Lag-Setup.exe`**
3. Run the installer — the app installs instantly, creates a desktop shortcut, and launches
4. Future updates are downloaded and applied automatically

---

## Building from Source

> For developers who want to build or contribute to the project.

**Prerequisites:** [.NET 8 SDK](https://dotnet.microsoft.com/download)

> **Important:** This project requires native `obs-core` binaries (`libobs`, FFmpeg) which are **not included** in this repository due to their size (~400 MB). You can obtain them from an existing OBS Studio installation or a pre-compiled `libobs` package.

**Step 1 — Clone the repository:**

```bash
git clone https://github.com/shkbb/Lag.git
cd Lag
```

**Step 2 — Build the project to generate output directories:**

```bash
dotnet build
```

**Step 3 — Place the `obs-core` binaries into the build output:**

```
Lag/bin/Debug/net8.0-windows/win-x64/obs-core/
```

**Step 4 — Run:**

```bash
dotnet run
```

---

## License

Distributed under the **GNU General Public License v3.0**.
See [`LICENSE`](LICENSE) for full details.

---

---

<div align="center">

<h1>Lag</h1>
<p><strong>Запис миттєвих повторів</strong></p>

</div>

**Lag** — це стильна та легка програма для запису ігрових моментів. Вона поєднує надійність ядра **OBS Studio** (`libobs`) із сучасним **Avalonia UI** у стилі скломорфізму. Більше жоден епічний момент не буде втрачено.

---

## Можливості

| Функція | Опис |
|---|---|
| **Дизайн (Скломорфізм)** | Ефекти Mica/Acrylic з Windows 11, плавні анімації та векторні Material-іконки |
| **Апаратне прискорення** | Автоматично обирає найкращий кодек — `NVENC` (NVIDIA), `AMF` (AMD), `QSV` (Intel) або `x264` |
| **Глобальні гарячі клавіші** | Збережіть останні секунди або хвилини гри одним натисканням налаштованої клавіші |
| **Вбудований програвач** | Переглядайте моменти прямо в програмі — без сторонніх плеєрів |
| **Автооновлення** | Velopack завантажує нові версії з GitHub і оновлює програму у фоновому режимі |
| **Автоматизація** | Автозапуск разом з Windows та автоматичний початок запису |
| **Локалізація** | Повна підтримка англійської та української мов |

---

## Встановлення

> Для користувачів — технічні знання не потрібні.

1. Перейдіть на вкладку [**Releases**](../../releases)
2. Завантажте найновіший файл **`Lag-Setup.exe`**
3. Запустіть його — програма встановиться за секунду, створить ярлик і запуститься
4. Усі майбутні оновлення завантажуватимуться автоматично

---

## Збірка з вихідного коду

> Для розробників, які хочуть зібрати або долучитися до проєкту.

**Вимоги:** [.NET 8 SDK](https://dotnet.microsoft.com/download)

> **Увага:** Для роботи програми потрібні нативні бінарники `obs-core` (`libobs`, FFmpeg), які **не включені** в репозиторій через великий розмір (~400 МБ). Їх можна взяти з папки встановленої OBS Studio або завантажити зібраний пакет libobs.

**Крок 1 — Клонуйте репозиторій:**

```bash
git clone https://github.com/shkbb/Lag.git
cd Lag
```

**Крок 2 — Зберіть проєкт для створення вихідних директорій:**

```bash
dotnet build
```

**Крок 3 — Помістіть бінарники `obs-core` у вихідну директорію:**

```
Lag/bin/Debug/net8.0-windows/win-x64/obs-core/
```

**Крок 4 — Запустіть:**

```bash
dotnet run
```

---

## Ліцензія

Поширюється за ліцензією **GNU General Public License v3.0**.
Детальніше — у файлі [`LICENSE`](LICENSE).
