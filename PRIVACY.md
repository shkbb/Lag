# Lag privacy policy

*Last updated: 20 November 2026*

Lag is a desktop app for Windows that records replays of your games on your own PC. It has no accounts, no
servers of its own, no analytics and no telemetry. The developer never receives your recordings, your
settings or anything about you.

## What stays on your PC

Recordings, screenshots, thumbnails, settings and logs are stored only on your computer, in the folders shown
in Lag's settings.

## What Lag sends over the internet

- **Update check.** When Lag starts, it asks GitHub (api.github.com) for the latest release of Lag and, if
  there is a newer one, downloads the installer from GitHub. Only the usual request data (your IP address,
  the app's user agent) reaches GitHub, under [GitHub's privacy statement](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement).
- **Cloud storage, only if you connect it.** If you connect Google Drive (or another cloud) in Lag's
  settings, Lag uploads the clips you choose to a "Lag" folder in **your own** cloud account, and reads them
  back to show them in your library.

## Google Drive

When you connect Google Drive, Lag asks for:

- `drive.file`: access **only to files Lag itself created** in your Drive (your clips, their thumbnails and
  Lag's library index). Lag cannot see, read or change any other file in your Drive.
- your e-mail address (`openid`, `userinfo.email`), only to show which account is connected.

The sign-in token is stored on your PC, encrypted with Windows (DPAPI) so that only your Windows user can use
it. It is never sent anywhere except to Google. Disconnecting the account in Lag deletes the token; you can
also revoke Lag's access at any time at [myaccount.google.com/permissions](https://myaccount.google.com/permissions).

Lag's use and transfer of information received from Google APIs adheres to the
[Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy),
including the Limited Use requirements. The data is used only to store and show your own clips; it is not
transferred to anyone, not used for advertising, and not read by people.

## Changes and contact

Changes to this policy are published in this file in the Lag repository. Questions:
[open an issue](https://github.com/shkbb/Lag/issues).

---

# Політика конфіденційності Lag

*Оновлено: 20 листопада 2026*

Lag — програма для Windows, що записує повтори ваших ігор на вашому ПК. У неї немає облікових записів,
власних серверів, аналітики чи телеметрії. Розробник ніколи не отримує ваші записи, налаштування чи будь-які
дані про вас.

**Що лишається на ПК:** записи, скріншоти, мініатюри, налаштування й логи зберігаються лише на вашому
комп'ютері, у папках, указаних у налаштуваннях Lag.

**Що Lag надсилає в інтернет:**

- **Перевірка оновлень.** Під час запуску Lag питає в GitHub (api.github.com) про останній випуск і, якщо він
  новіший, завантажує інсталятор з GitHub.
- **Хмара, лише якщо ви її підключили.** Тоді Lag вивантажує вибрані кліпи в папку «Lag» у **вашому власному**
  хмарному акаунті й читає їх звідти, щоб показати в бібліотеці.

**Google Диск.** Lag просить доступ `drive.file`, тобто **лише до файлів, які створив сам Lag** (кліпи,
мініатюри, індекс бібліотеки). Інших файлів вашого Диска Lag не бачить і не змінює. Також Lag отримує вашу
пошту, щоб показати, який акаунт підключено. Токен входу зберігається на вашому ПК, зашифрований засобами
Windows (DPAPI), і надсилається лише в Google. Відключення акаунта в Lag видаляє токен; відкликати доступ можна
будь-коли на [myaccount.google.com/permissions](https://myaccount.google.com/permissions).

Використання даних, отриманих через API Google, відповідає
[Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy),
зокрема вимогам Limited Use: дані потрібні лише для зберігання й показу ваших кліпів, нікому не передаються, не
використовуються для реклами і не читаються людьми.

Питання: [створіть issue](https://github.com/shkbb/Lag/issues).
