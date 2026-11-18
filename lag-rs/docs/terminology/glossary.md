# Lag UI Terminology & Style Guide

This document defines the English (EN) and Ukrainian (UK) localisation standards for **Lag**, a Windows instant-replay game recorder.

---

## 1. Style Guide

### 1.1 Tone & Voice
- **Plain, confident, Medal-like utility tone**: speak directly to gamers in everyday language.
- **No developer jargon or technical brackets in labels**: avoid internal terms like `(hook)`, `(Alt+Tab)`, `(FPS)`, `(auto)`, `(ESC to exit)`, or `"bake"`. Put technical caveats only where strictly necessary in concise plain text.
- **One short sentence per description**: every setting description or hint should fit cleanly in a single short sentence without multi-sentence warnings or ALL CAPS emphasis.
- **Calm, uniform punctuation**:
  - Use ASCII `...` for in-progress states (`Recording...`, `Saving replay...`, `Exporting... {0}%`).
  - Avoid exclamation marks in empty states or hints.
  - In Ukrainian, use guillemets `«...»` instead of straight quotes `'...'` when quoting UI buttons or modes.

### 1.2 English (EN) Capitalisation Rules
Lag uses a **tiered Title Case + Sentence case** convention (matching Medal.tv and NVIDIA App):

| UI Element | Rule | Examples | Justification |
|---|---|---|---|
| **Dedicated section headers & compact badges** | **ALL CAPS** | `NAVIGATION`, `RECORDING`, `ENCODING`, `HOTKEYS`, `PRESETS`, `CLIPS`, `REPLAY`, `{0} CLIPS` | Matches the UI's uppercase `rgroup` headings and badge chips, creating clear visual hierarchy. |
| **Navigation, tabs, page titles, presets, tray items, and all action buttons** | **Title Case** | `Library`, `Settings`, `Overlay`, `Save Replay`, `Start Recording`, `Open Folder`, `Check for Updates`, `Clear Selection`, `Add Text`, `End Here`, `Re-scan Hardware`, `Export .zip`, `Open Library` | Visually distinguishes clickable actions and top-level destinations from descriptive setting labels, with zero ambiguity between "primary" and "secondary" buttons. |
| **Settings row labels, toggles/checkboxes, multi-word options, descriptions, tooltips, statuses, toasts** | **Sentence case** | `Replay buffer length`, `Frame rate`, `All system audio`, `Keep recording the game`, `Reset trim`, `Recording started`, `Next clip` | Matches Windows 11 Settings and improves scannability in dense two-column setting rows and newly approved option strings. |

**Shared-key exceptions:**
- `Settings_Group_Overlay` is rendered both in the Settings tab strip (`Video | Audio | Overlay | General`) and as a card title, so it uses Title Case (`Overlay`).
- `About_Group` is rendered as a regular setting row inside the `STORAGE & ABOUT` card, so it uses Sentence/Title case (`About`), not `ABOUT`.

### 1.3 Ukrainian (UK) Rules
- **Sentence case everywhere** (except ALL CAPS for dedicated `rgroup` headers and badges like `ЗАПИС`, `КОДУВАННЯ`, `КЛІПИ`, `ПОВТОР`, and standard acronyms like `Windows`, `Steam`, `CS2`, `GPU`, `FPS`, `GIF`). Ukrainian UI never uses English-style Title Case.
- **Address form**: Use polite **`ви`-form** (`ви`, `ваш`, `натисніть`, `перетягніть`, `використовуйте`) wherever the user is addressed directly; eliminate all `ти`-form strings (`додай`, `слухай`, `твоє`, `твою`). Use **infinitive / impersonal** forms for toggle descriptions (`Автоматично видаляти...`, `Показувати...`) and status labels (`Готово`, `Призупинено`, `Зупинено`).
- **Consistent verb forms for buttons & tray actions**: Use **infinitive verbs** for clickable action buttons and tray menu items (`Зберегти повтор`, `Почати запис`, `Зупинити запис`, `Перевірити оновлення`, `Експортувати .zip`, `Вийти`, `Зняти виділення`). Keep standard compact nouns only where established by Windows/media convention (`Пауза`, `Огляд`, `Назад`).
- **No calques, Russianisms, or mixed Latin**:
  - Use `тривалість буфера` (not `довжина буфера`), `роздільна здатність` (not `роздільність`), `оригінальна` (not `нативна`), `пристрій/рівень виведення` (not `виводу`/`виходу`), `екрана` (not `екрану`), `параметри` (not `опції`), `розташування` (not `позиція`), `коліщатко` (not `колесиком`), `обладнання` (not `залізо`), `відеооверлеї` (not `відео-оверлеї`), `скриншот` (2019 orthography, not `скріншот`), `програма` consistently (not mixing `програма` and `застосунок`), `Натисни і говори` (not Latin `Push-to-talk`).
- **Plural-safe count formatting**: Because the localization layer uses single-string `{0}` substitution without ICU plural rules, format counts in Ukrainian as `Кліпів: {0}` / `КЛІПІВ: {0}` rather than `{0} кліпів` (which is ungrammatical for 1, 2–4, 21, etc.).

---

## 2. Terminology Glossary

| Concept | Preferred EN | Preferred UK | Terms to Avoid | Used By |
|---|---|---|---|---|
| **Replay** (instant-replay action & raw buffer capture) | `Replay` | `Повтор` | `Moment` (when referring to saved files), `Повторення` | NVIDIA ShadowPlay, Steam Game Recording, OBS |
| **Replay buffer** (rolling background recording) | `Replay buffer` (compact: `Buffer`) | `Буфер повтору` (compact: `Буфер`) | `Rolling buffer`, `Буфер повторень` | OBS Studio, NVIDIA ShadowPlay |
| **Buffer length** | `Replay buffer length` | `Тривалість буфера повтору` | `Довжина буфера повтору` (calque) | OBS Studio, NVIDIA App |
| **Clip** (saved video file in Library, Player, Editor) | `Clip` / `clips` | `Кліп` / `кліпи` | Mixing `replay` and `moment` in Library/Player (`Your saved moments`, `Next replay` in clip list) | Medal.tv, Steam Game Recording |
| **Library** (collection & folder of saved clips) | `Library` | `Бібліотека` | `Gallery`, `Catalog`, `Архів` | Steam Game Recording, Medal.tv |
| **Recording** (active capture session) | `Recording` | `Запис` | `Capturing` for user-facing transport states; `Зараз записується` (use `Триває запис`) | Medal.tv, OBS Studio, NVIDIA App |
| **Save replay** (trigger saving the buffer) | `Save Replay` (button/tray), `Save replay` (hotkey label) | `Зберегти повтор` | `Capture replay`, `Save moment` | NVIDIA ShadowPlay, OBS Studio |
| **Hotkey** (global keyboard shortcut) | `Hotkey` / `Hotkeys` | `Гаряча клавіша` / `Гарячі клавіші` | `Keybind`, `HOTKEY` (singular for a multi-hotkey group), `Слухаю...` / `Listening...` | OBS Studio, Medal.tv |
| **Encoder / Video codec / GPU** | `Encoding`, `Video codec`, `Recording GPU` | `Кодування`, `Відеокодек`, `GPU для запису` | `Транскодер` | OBS Studio, NVIDIA App |
| **Bitrate** | `Bitrate` (`Mbps`) | `Бітрейт` (`Мбіт/с`) | `Якість потоку` | OBS Studio, Medal.tv |
| **Frame rate** | `Frame rate` | `Частота кадрів` | `Frame rate (FPS)` (brackets in label), lowercase `fps` in prose | Windows 11, Steam, Medal.tv |
| **Resolution** | `Resolution` (option: `Native`) | `Роздільна здатність` (option: `Оригінальна`) | `Output resolution` (inconsistent with Editor), `Роздільність` (calque), `Нативна` | Windows 11, OBS Studio, Steam |
| **Game audio** | `Game audio` | `Звук гри` | `Game sound` | Medal.tv, Steam Game Recording |
| **System audio** | `System audio` / `All system audio` | `Системний звук` / `Увесь системний звук` | `System sound`, `All PC audio`, `PC sound`, `Весь звук ПК` | Windows 11, Medal.tv, Discord |
| **Audio source mode** | `Audio source` (`All system audio` / `Specific apps`) | `Джерело звуку` (`Увесь системний звук` / `Окремі програми`) | `Recording mode` (ambiguous inside Audio settings), `Обрані програми` (for mode name) | Medal.tv, Discord |
| **Microphone** | `Microphone` (compact labels: `Mic level`, `Mic volume`) | `Мікрофон` (`Рівень мікрофона`, `Гучність мікрофона`) | `Input` alone when mic is meant | Discord, Medal.tv, OBS Studio |
| **Output device & level** | `Output device`, `Output level`, `System default` | `Пристрій виведення`, `Рівень виведення`, `За замовчуванням` | `Default (auto)`, `Пристрій виводу`, `Рівень виходу` (calques) | Windows 11, Discord |
| **Push-to-talk** | `Push-to-talk`, `Push-to-talk key` | `Натисни і говори`, `Клавіша «Натисни і говори»` | `PTT`, Latin `Push-to-talk` inside Ukrainian UI, `затиснута` | Discord, Steam |
| **Noise suppression** | `Noise suppression` | `Шумозаглушення` | `Noise cancellation`, `Кулери` (slang; use `вентилятори`) | Discord, OBS Studio |
| **Input sensitivity** | `Input sensitivity`, `Threshold` | `Чутливість мікрофона`, `Поріг` | `Поріг чутливості` (collides with `Threshold`), `Глушить` (slang) | Discord |
| **Mic monitoring** | `Hear yourself` | `Прослуховування мікрофона` | `Чути себе` (literal), `Слухай у навушниках` (`ти`-form) | Medal.tv, Windows |
| **Overlay** | `Overlay` / `Overlays` | `Оверлей` / `Оверлеї` | `Video overlays`, `Відео-оверлеї` (wrong hyphen), `"Bake..."` / `"Запікати..."` (jargon) | NVIDIA App, Discord, Steam |
| **Webcam** | `Webcam`, `Webcam overlay` | `Вебкамера`, `Оверлей вебкамери` | Mixing `Camera` and `Webcam` for the same control | Medal.tv, OBS Studio |
| **Keyboard & mouse overlay** | `Keyboard & mouse`, `Input overlay` | `Клавіатура та миша`, `Показувати натиснуті клавіші` | `Keyboard + mouse` | OBS Studio / input-overlay plugins |
| **Performance stats** | `Performance stats` | `Статистика продуктивності` | `Resource monitor` (sounds like Windows Task Manager) | NVIDIA App, Steam |
| **Desktop** | `Desktop` (`Record the desktop`) | `Робочий стіл` (`Записувати робочий стіл`) | `Screen` when specifically contrasting with a game | Windows 11, OBS Studio |
| **Display** (physical monitor) | `Display` | `Дисплей` | Mixing `Display` and `Monitor` (`Монітор`) / `Екран` | Windows 11, NVIDIA App |
| **Focus** (window focus) | `When the game is out of focus`, `Keep recording the game` | `Коли гра не у фокусі`, `Продовжувати записувати гру` | `(Alt+Tab)` in labels, `"hold the game"` | Steam Game Recording, OBS |
| **Preset** | `PRESETS`, `Performance`, `Balanced`, `Quality` | `ПРЕСЕТИ`, `Продуктивність`, `Баланс`, `Якість` | `"Pick a goal"`, `"Залізо"` (slang; use `обладнання`) | NVIDIA App, Medal.tv |
| **Exclusive fullscreen capture** | `Game capture` | `Захоплення гри` | `Game capture (hook)` / `Захоплення ігор (хук)` | OBS Studio |
| **Screenshot** | `Screenshot` | `Скриншот` | `Скріншот` (pre-2019 spelling) | Steam, Medal.tv |
| **Editor trim & cut** | `Cut`, `End Here`, `Reset trim`, `Set start to current position` | `Вирізати`, `Кінець тут`, `Скинути обрізку`, `Початок кліпу з поточної позиції` | `"to the current moment"` | Medal.tv, Steam Game Recording |
| **Export** | `Export`, `Export options`, `Export as GIF` | `Експортувати`, `Параметри експорту`, `Експортувати як GIF` | `Опції експорту` (calque), `Експорт у GIF` (noun vs verb) | Medal.tv, CapCut |
| **Diagnostics & logs** | `Logs`, `Open Folder`, `Export .zip` | `Логи`, `Відкрити папку`, `Експортувати .zip` | `Експорт .zip` (noun instead of button verb) | Discord, Medal.tv |
