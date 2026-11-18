# Виміри й знахідки (фаза 0)

## Тулчейн
- MSVC/Visual Studio на ПК немає → `stable-x86_64-pc-windows-gnu` (закріплено в `rust-toolchain.toml`).
- FFmpeg-біндинги: bindgen 0.72 + libclang 18 (PyPI wheel у `vendor/libclang`), заголовки `n7.1.5`
  (sparse-clone у `vendor/ffmpeg-src`). `hwcontext_d3d11va.h` НЕ йде через bindgen (тягне `windows.h` →
  intrinsics clang 23 ламають libclang 18); три структури описані вручну в `lag-ffi/src/d3d11va.rs`.
- Лінкування: GNU ld лінкує прямо з `.dll` (FFmpeg з `InstantReplay/ffmpeg`), mpv через `libmpv.dll.a`.
- `vendor/` у `.gitignore` (бінарники/заголовки, відтворювані). Згенеровані `*_sys.rs` комітяться.

## Slint (спайк, 2026-11-16)
- femtovg/OpenGL: рендер до **~163 fps** на 165 Гц моніторі без жодних хаків таймера; RAM ~79 МБ, CPU ~18% одного ядра
  при безперервній анімації. (Avalonia потребувала рефлексію в `DefaultRenderTimer(120)`.)
- software-рендер не підтримує rendering notifier → лишаємось на femtovg (потрібен і для mpv-текстури).

## libmpv (shinchiro 20261111, baseline x86-64, без AVX2-вимоги)
- `mpv_initialize`: **~2 мс** (libVLC: ~10 с холодний старт — регресія 1.8.0 зникає сама собою).
- Відкриття реального кліпу 688 МБ / 120 с / 2580×1080 H.264 / 3×AAC: **200–440 мс** до `file-loaded`.
- HW-декод: `hwdec=d3d11va-copy` працює headless. У вікні: `d3d11va` + OpenGL interop (`dxinterop`, WGL_NV_DX_interop2) — перевірити в спайку з текстурою.
- Exact seek: 0,15–0,5 с (довгий GOP). Для скрабу в редакторі: keyframe-seek під час перетягування, exact — на відпусканні.

## FFmpeg, що постачається з 1.7/1.8 (n7.1.5 BtbN, GPL shared)
- Є: `h264_nvenc`, `hevc_nvenc`, `aac`, `libx264`. **Немає `av1_nvenc`** → AV1 опційний, як і в C#.

## Особливості існуючих кліпів, які треба зберегти
- Аудіотреки в MP4 **без title** (тільки `handler_name=SoundHandler`, `language=und`), хоча C# ставить `title`
  через `av_dict_set` — MP4-муксер його не пише. Плеєр/редактор розрізняють треки за ПОРЯДКОМ. Rust-муксер
  має давати той самий порядок; додати назви можна, але не покладатися на них при читанні старих кліпів.
- Приклад реального кліпу: відео ~45 Мбіт/с, container fps 73.9 (VFR), треки: 198k / 165k / 160k.
