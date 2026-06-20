using System.Text.Json;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Lag.Core;
using Lag.Services;
using Lag.Services.ObsIntegration;
using Microsoft.Win32;
using SharpHook.Native;
using Velopack;
using Velopack.Sources;

namespace Lag.ViewModels;

/// <summary>
/// ViewModel for the Settings view. Manages all user-configurable options
/// and persists them to a JSON file.
/// </summary>
public partial class SettingsViewModel : ViewModelBase
{
    private readonly Lag.Services.IReplayRecorder _engine;   // the active recorder (VFR, or OBS fallback)
    private readonly GlobalHotkeyManager _hotkeyManager;
    private readonly GlobalHotkeyService _hotkeyService;

    private static readonly string SettingsFilePath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "Lag", "settings.json");

    [ObservableProperty]
    private bool _hasPendingChanges;

    /// <summary>Active settings tab (0 = Video, 1 = Audio, 2 = General). UI state only.</summary>
    [ObservableProperty]
    private int _selectedSettingsTab;

    /// <summary>
    /// True while the constructor is populating collections and loading persisted settings.
    /// SaveSettings() is suppressed during this window — otherwise RefreshMonitors()/
    /// RefreshMicrophones() (which assign SelectedMonitor/SelectedMicrophone) would trigger a save
    /// that overwrites settings.json with defaults BEFORE LoadSettings() can read it. That was the
    /// root cause of "settings reset on restart".
    /// </summary>
    private bool _isInitializing;

    // ───────────── Buffer Duration ─────────────

    /// <summary>
    /// Replay buffer duration options. Labels are LOCALIZED, so the list is (re)built by
    /// <see cref="RebuildLocalizedOptions"/> at startup and on every language switch.
    /// </summary>
    public System.Collections.ObjectModel.ObservableCollection<BufferOption> BufferOptions { get; } = new();

    [ObservableProperty]
    private BufferOption _selectedBuffer = null!;

    partial void OnSelectedBufferChanged(BufferOption value)
    {
        SaveSettingsRestartRequired();
    }

    /// <summary>
    /// Selected buffer length in seconds. Duration is the authoritative value — labels are
    /// localized display-only strings (the old label-parsing approach broke on translation).
    /// </summary>
    public int BufferSeconds => (int)SelectedBuffer.Duration.TotalSeconds;

    // ───────────── Monitors ─────────────
    
    private readonly HardwareDetector _hardwareDetector;

    public System.Collections.ObjectModel.ObservableCollection<HardwareDetector.MonitorInfo> Monitors { get; } = new();

    [ObservableProperty]
    private HardwareDetector.MonitorInfo? _selectedMonitor;

    partial void OnSelectedMonitorChanged(HardwareDetector.MonitorInfo? value)
    {
        // The capture FPS ceiling follows the monitor's refresh rate; the resolution ceiling follows
        // its native height. Both are rebuilt when the selected monitor changes.
        RebuildFpsOptions();
        RebuildResolutionOptions();
        SaveSettingsRestartRequired();
    }

    private void RefreshMonitors()
    {
        Monitors.Clear();
        var screens = _hardwareDetector.GetAvailableMonitors();
        foreach (var s in screens) Monitors.Add(s);

        SelectedMonitor = Monitors.FirstOrDefault(m => m.IsPrimary) ?? Monitors.FirstOrDefault();
    }

    // ───────────── Microphones ─────────────

    public System.Collections.ObjectModel.ObservableCollection<MicrophoneInfo> Microphones { get; } = new();

    [ObservableProperty]
    private MicrophoneInfo? _selectedMicrophone;

    partial void OnSelectedMicrophoneChanged(MicrophoneInfo? value)
    {
        SaveSettingsRestartRequired();
    }

    private void RefreshMicrophones()
    {
        Microphones.Clear();
        var mics = _hardwareDetector.GetMicrophones();
        foreach (var m in mics) Microphones.Add(m);

        SelectedMicrophone = Microphones.FirstOrDefault();
    }

    // ───────────── Hotkey ─────────────

    [ObservableProperty]
    private string _hotkeyDisplayText = "Alt + F10";

    partial void OnHotkeyDisplayTextChanged(string value)
    {
        OnPropertyChanged(nameof(HotkeyParts));
    }

    /// <summary>Hotkey split into kbd-chip parts for the Figma design ("Ctrl","Shift","S").</summary>
    public IReadOnlyList<string> HotkeyParts =>
        HotkeyDisplayText.Split('+', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries);

    [ObservableProperty]
    private bool _isCapturingHotkey;

    // ───────────── Screenshot hotkey ─────────────

    private KeyCode _screenshotKey = KeyCode.VcF9;
    private ModifierMask _screenshotModifiers = ModifierMask.LeftAlt;

    [ObservableProperty]
    private string _screenshotHotkeyDisplayText = "Alt + F9";

    partial void OnScreenshotHotkeyDisplayTextChanged(string value) =>
        OnPropertyChanged(nameof(ScreenshotHotkeyParts));

    /// <summary>Screenshot hotkey split into kbd-chip parts ("Alt","F9").</summary>
    public IReadOnlyList<string> ScreenshotHotkeyParts =>
        ScreenshotHotkeyDisplayText.Split('+', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries);

    [ObservableProperty]
    private bool _isCapturingScreenshotKey;

    private bool _screenshotCaptureMode;

    [RelayCommand]
    private void CaptureScreenshotHotkey()
    {
        _screenshotCaptureMode = true;
        IsCapturingScreenshotKey = true;
        _hotkeyManager.IsCapturing = true;
    }

    private void ApplyScreenshotHotkeyToService()
    {
        try
        {
            _hotkeyService.UpdateScreenshotHotkey(_screenshotModifiers, _screenshotKey);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to apply screenshot hotkey: {ex.Message}");
        }
    }

    // ───────────── Paths ─────────────

    [ObservableProperty]
    private string _libraryPath;

    partial void OnLibraryPathChanged(string value)
    {
        SaveSettingsRestartRequired();
    }

    // ───────────── Frame Rate ─────────────

    /// <summary>FPS presets; Value = 0 means "Custom" (label localized → rebuilt on language switch).</summary>
    public System.Collections.ObjectModel.ObservableCollection<FpsOption> FpsOptions { get; } = new();

    [ObservableProperty]
    private FpsOption _selectedFps = null!;

    partial void OnSelectedFpsChanged(FpsOption value)
    {
        OnPropertyChanged(nameof(IsCustomFps));
        SaveSettingsRestartRequired();
        MaybeWarnIntensive();
    }

    /// <summary>Custom frame rate (used when the "Custom" preset is selected).</summary>
    [ObservableProperty]
    private int _customFps = 60;

    partial void OnCustomFpsChanged(int value)
    {
        // Clamp to the monitor's refresh — a higher value can't be captured and would only
        // starve the CBR bitrate (the fps-hint splits bit_rate across it). Re-enters once.
        int cap = CurrentRefreshCap();
        if (value > cap) { CustomFps = cap; return; }
        SaveSettingsRestartRequired();
        MaybeWarnIntensive();
    }

    public bool IsCustomFps => SelectedFps.Value == 0;

    /// <summary>The frame rate that actually goes to the engine.</summary>
    public int EffectiveFps =>
        SelectedFps.Value > 0 ? SelectedFps.Value : Math.Clamp(CustomFps, 1, CurrentRefreshCap());

    /// <summary>FPS ceiling = the selected monitor's refresh rate. WGC captures at most the
    /// monitor's composition rate, so offering more is dishonest and (via the CBR fps-hint)
    /// hurts quality. Unknown/implausible refresh → 360 (don't restrict).</summary>
    private int CurrentRefreshCap()
    {
        uint hz = SelectedMonitor?.RefreshRate
                  ?? Monitors.FirstOrDefault(m => m.IsPrimary)?.RefreshRate
                  ?? 0;
        if (hz < 24) return 360;
        return Math.Max(SnapRefresh((int)hz), 60);
    }

    /// <summary>Panels often report 59 / 143 / 164 etc. — snap to the nearest common rate.</summary>
    private static int SnapRefresh(int hz)
    {
        int[] known = { 60, 75, 90, 100, 120, 144, 160, 165, 180, 200, 240, 280, 300, 360, 390, 480, 500 };
        foreach (int k in known)
            if (Math.Abs(hz - k) <= 2) return k;
        return hz;
    }

    /// <summary>Rebuilds the FPS preset list capped to the selected monitor's refresh rate — no
    /// point offering 240/360 on a 144 Hz panel. On a 360 Hz panel the high rungs reappear
    /// automatically. Keeps the prior selection, snapping it down if it now exceeds the cap.</summary>
    private void RebuildFpsOptions()
    {
        bool wasInitializing = _isInitializing;
        _isInitializing = true;
        try
        {
            int prev = SelectedFps?.Value ?? 30;
            int cap = CurrentRefreshCap();

            var rungs = new List<int>();
            foreach (int fps in new[] { 24, 30, 60, 120, 240, 360 })
                if (fps <= cap) rungs.Add(fps);
            if (rungs.Count == 0) rungs.Add(60);
            if (cap > rungs[^1]) rungs.Add(cap);   // the panel's native top (e.g. 144, 165)

            FpsOptions.Clear();
            foreach (int fps in rungs) FpsOptions.Add(new FpsOption(fps.ToString(), fps, FpsTier(fps)));
            FpsOptions.Add(new FpsOption(Localizer.Get("Option_Custom"), 0));

            FpsOption? pick = FpsOptions.FirstOrDefault(f => f.Value == prev);
            if (pick == null && prev > 0)
                pick = FpsOptions.Where(f => f.Value > 0)
                                 .OrderBy(f => Math.Abs(f.Value - Math.Min(prev, cap)))
                                 .First();
            SelectedFps = pick ?? FpsOptions.FirstOrDefault(f => f.Value == 30) ?? FpsOptions[0];

            if (CustomFps > cap) CustomFps = cap;
        }
        finally
        {
            _isInitializing = wasInitializing;
        }
    }

    // ───────────── Intensive-quality tiers + warning ─────────────
    // We DON'T block high settings — we colour them by "intensity" and show a one-time disclaimer.
    // Tier 0 = normal, 1 = caution (amber), 2 = extreme (red). Resolution above 1080p
    // and bitrate above ~25 Mbps / fps above 60 are the thresholds we call out.

    private static int FpsTier(int fps) => fps > 120 ? 2 : fps > 60 ? 1 : 0;
    private static int BitrateTier(int mbps) => mbps > 50 ? 2 : mbps > 30 ? 1 : 0;
    private static int ResolutionTier(int height) => height > 1440 ? 2 : height > 1080 ? 1 : 0;

    /// <summary>True when the current selection is heavy enough to warrant the disclaimer.</summary>
    private bool IsIntensiveSelection
    {
        get
        {
            if (EffectiveFps > 60 || EffectiveBitrateKbps > 25_000) return true;
            int h = SelectedResolution?.TargetHeight ?? 0;
            if (h == 0) h = (int)(SelectedMonitor?.Height ?? 0);   // Native → the monitor's own height
            return h > 1080;
        }
    }

    /// <summary>Shows the intensive-quality disclaimer overlay (once per session) when a heavy option
    /// is chosen — bound to <c>IsVisible</c> in the view.</summary>
    [ObservableProperty]
    private bool _showIntensiveWarning;

    private bool _intensiveAcknowledged;

    private void MaybeWarnIntensive()
    {
        if (_isInitializing || _intensiveAcknowledged) return;
        if (IsIntensiveSelection) ShowIntensiveWarning = true;
    }

    /// <summary>"I understand" — dismiss and don't nag again this session.</summary>
    public void AcknowledgeIntensive()
    {
        _intensiveAcknowledged = true;
        ShowIntensiveWarning = false;
    }

    // Intensive-quality disclaimer text — kept here (not in the language .axaml dicts) so all UI
    // languages live in one place. Falls back to English for any code not listed.
    private (string Title, string Summary, string Body, string Ok) IntensiveT =>
        IntensiveTexts.TryGetValue(SelectedLanguage?.Code ?? "en", out var t) ? t : IntensiveTexts["en"];

    public string IntensiveWarnTitle => IntensiveT.Title;
    public string IntensiveWarnSummary => IntensiveT.Summary;
    public string IntensiveWarnBody => IntensiveT.Body;
    public string IntensiveWarnOk => IntensiveT.Ok;

    private static readonly Dictionary<string, (string Title, string Summary, string Body, string Ok)> IntensiveTexts = new()
    {
        ["en"] = ("Warning: Intensive Quality Options",
                  "Your selected quality options exceed: 60 FPS / 25 Mbps / 1080p.",
                  "• Unless your machine is powerful, your clips may be choppy.\n• Clips are compressed when you publish them — the original stays on your PC.\n• Higher settings don't always mean better clips: a smooth Full HD clip beats a choppy QHD one.\n\nUse these only if your graphics card can handle them and you know what you're doing.",
                  "I understand"),
        ["uk"] = ("Увага: інтенсивні налаштування якості",
                  "Обрані налаштування перевищують: 60 FPS / 25 Мбіт/с / 1080p.",
                  "• Якщо твоя система не потужна, кліпи можуть бути смикані (choppy).\n• При публікації відео все одно стискається — оригінал лишається на ПК.\n• Вищі налаштування ≠ кращі кліпи: плавний Full HD кращий за смиканий QHD.\n\nРекомендовано вмикати це лише якщо твоя відеокарта це тягне і ти знаєш, що робиш.",
                  "Зрозуміло"),
        ["de"] = ("Warnung: Intensive Qualitätseinstellungen",
                  "Deine gewählten Einstellungen überschreiten: 60 FPS / 25 Mbps / 1080p.",
                  "• Wenn dein PC nicht leistungsstark ist, können die Clips ruckeln.\n• Beim Veröffentlichen werden Videos ohnehin komprimiert — das Original bleibt auf deinem PC.\n• Höhere Einstellungen bedeuten nicht immer bessere Clips: ein flüssiger Full-HD-Clip ist besser als ein ruckelnder QHD-Clip.\n\nNutze diese Einstellungen nur, wenn deine Grafikkarte das schafft und du weißt, was du tust.",
                  "Verstanden"),
        ["fr"] = ("Attention : options de qualité intensives",
                  "Les options choisies dépassent : 60 FPS / 25 Mbps / 1080p.",
                  "• Si ton PC n'est pas puissant, tes clips risquent de saccader.\n• Les vidéos sont de toute façon compressées à la publication — l'original reste sur ton PC.\n• Des réglages plus élevés ne donnent pas toujours de meilleurs clips : un clip Full HD fluide vaut mieux qu'un QHD saccadé.\n\nN'utilise ces réglages que si ta carte graphique le permet et si tu sais ce que tu fais.",
                  "J'ai compris"),
        ["be"] = ("Увага: інтэнсіўныя налады якасці",
                  "Абраныя налады перавышаюць: 60 FPS / 25 Мбіт/с / 1080p.",
                  "• Калі твая сістэма не магутная, кліпы могуць тузацца.\n• Пры публікацыі відэа ўсё роўна сціскаецца — арыгінал застаецца на ПК.\n• Вышэйшыя налады ≠ лепшыя кліпы: плыўны Full HD лепшы за тузаны QHD.\n\nРэкамендуецца ўключаць гэта толькі калі твая відэакарта гэта цягне і ты ведаеш, што робіш.",
                  "Зразумела"),
        ["lt"] = ("Įspėjimas: intensyvūs kokybės nustatymai",
                  "Pasirinkti nustatymai viršija: 60 FPS / 25 Mbps / 1080p.",
                  "• Jei tavo kompiuteris nėra galingas, klipai gali strigti.\n• Skelbiant vaizdo įrašai vis tiek suspaudžiami — originalas lieka tavo kompiuteryje.\n• Aukštesni nustatymai ne visada reiškia geresnius klipus: sklandus Full HD geriau nei strigantis QHD.\n\nNaudok šiuos nustatymus tik jei tavo vaizdo plokštė juos pajėgia ir žinai, ką darai.",
                  "Supratau"),
        ["et"] = ("Hoiatus: intensiivsed kvaliteediseaded",
                  "Valitud seaded ületavad: 60 FPS / 25 Mbps / 1080p.",
                  "• Kui su arvuti pole võimas, võivad klipid hakkida.\n• Avaldamisel videod niikuinii pakitakse — originaal jääb su arvutisse.\n• Kõrgemad seaded ei tähenda alati paremaid klippe: sujuv Full HD on parem kui hakkiv QHD.\n\nKasuta neid ainult siis, kui su graafikakaart seda võimaldab ja tead, mida teed.",
                  "Sain aru"),
        ["lv"] = ("Brīdinājums: intensīvi kvalitātes iestatījumi",
                  "Izvēlētie iestatījumi pārsniedz: 60 FPS / 25 Mbps / 1080p.",
                  "• Ja tavs dators nav jaudīgs, klipi var raustīties.\n• Publicējot video tāpat tiek saspiests — oriģināls paliek tavā datorā.\n• Augstāki iestatījumi ne vienmēr nozīmē labākus klipus: vienmērīgs Full HD ir labāks par raustīgu QHD.\n\nLieto tos tikai tad, ja tava videokarte to spēj un tu zini, ko dari.",
                  "Sapratu"),
        ["fi"] = ("Varoitus: raskaat laatuasetukset",
                  "Valitut asetukset ylittävät: 60 FPS / 25 Mbps / 1080p.",
                  "• Jos koneesi ei ole tehokas, leikkeet voivat nykiä.\n• Videot pakataan joka tapauksessa julkaistaessa — alkuperäinen säilyy koneellasi.\n• Korkeammat asetukset eivät aina tarkoita parempia leikkeitä: sujuva Full HD on parempi kuin nykivä QHD.\n\nKäytä näitä vain, jos näytönohjaimesi pystyy siihen ja tiedät mitä teet.",
                  "Selvä"),
        ["sv"] = ("Varning: intensiva kvalitetsinställningar",
                  "Dina valda inställningar överstiger: 60 FPS / 25 Mbps / 1080p.",
                  "• Om din dator inte är kraftfull kan klippen hacka.\n• Videor komprimeras ändå när du publicerar — originalet stannar på din dator.\n• Högre inställningar betyder inte alltid bättre klipp: ett mjukt Full HD-klipp slår ett hackigt QHD.\n\nAnvänd dessa bara om ditt grafikkort klarar det och du vet vad du gör.",
                  "Jag förstår"),
        ["no"] = ("Advarsel: intensive kvalitetsinnstillinger",
                  "De valgte innstillingene overstiger: 60 FPS / 25 Mbps / 1080p.",
                  "• Hvis maskinen din ikke er kraftig, kan klippene hakke.\n• Videoer komprimeres uansett ved publisering — originalen blir igjen på PC-en.\n• Høyere innstillinger betyr ikke alltid bedre klipp: et jevnt Full HD-klipp er bedre enn et hakkete QHD.\n\nBruk disse bare hvis grafikkortet ditt takler det og du vet hva du gjør.",
                  "Jeg forstår"),
        ["da"] = ("Advarsel: intensive kvalitetsindstillinger",
                  "De valgte indstillinger overstiger: 60 FPS / 25 Mbps / 1080p.",
                  "• Hvis din computer ikke er kraftig, kan klippene hakke.\n• Videoer komprimeres alligevel ved udgivelse — originalen bliver på din pc.\n• Højere indstillinger betyder ikke altid bedre klip: et jævnt Full HD-klip er bedre end et hakkende QHD.\n\nBrug kun disse, hvis dit grafikkort kan klare det, og du ved, hvad du laver.",
                  "Jeg forstår"),
        ["nl"] = ("Waarschuwing: intensieve kwaliteitsinstellingen",
                  "Je gekozen instellingen overschrijden: 60 FPS / 25 Mbps / 1080p.",
                  "• Als je pc niet krachtig is, kunnen je clips schokkerig zijn.\n• Video's worden bij het publiceren toch gecomprimeerd — het origineel blijft op je pc.\n• Hogere instellingen betekenen niet altijd betere clips: een vloeiende Full HD-clip is beter dan een schokkerige QHD.\n\nGebruik deze alleen als je videokaart het aankan en je weet wat je doet.",
                  "Begrepen"),
        ["it"] = ("Attenzione: opzioni di qualità intensive",
                  "Le opzioni selezionate superano: 60 FPS / 25 Mbps / 1080p.",
                  "• Se il tuo PC non è potente, le clip potrebbero scattare.\n• I video vengono comunque compressi alla pubblicazione — l'originale resta sul tuo PC.\n• Impostazioni più alte non significano sempre clip migliori: una clip Full HD fluida è meglio di una QHD a scatti.\n\nUsa queste impostazioni solo se la tua scheda video le regge e sai cosa stai facendo.",
                  "Ho capito"),
        ["es"] = ("Advertencia: opciones de calidad intensivas",
                  "Las opciones elegidas superan: 60 FPS / 25 Mbps / 1080p.",
                  "• Si tu PC no es potente, los clips pueden ir a tirones.\n• Los vídeos se comprimen igualmente al publicarlos — el original se queda en tu PC.\n• Ajustes más altos no siempre significan mejores clips: un clip Full HD fluido es mejor que uno QHD a tirones.\n\nUsa estos ajustes solo si tu tarjeta gráfica puede con ellos y sabes lo que haces.",
                  "Entendido"),
        ["pt"] = ("Aviso: opções de qualidade intensivas",
                  "As opções escolhidas excedem: 60 FPS / 25 Mbps / 1080p.",
                  "• Se o teu PC não for potente, os clipes podem ficar com falhas.\n• Os vídeos são comprimidos na publicação de qualquer forma — o original fica no teu PC.\n• Definições mais altas nem sempre significam clipes melhores: um clipe Full HD fluido é melhor que um QHD travado.\n\nUsa estas definições apenas se a tua placa gráfica aguentar e souberes o que estás a fazer.",
                  "Entendi"),
        ["ja"] = ("警告：高負荷の画質設定",
                  "選択した設定は次を超えています：60 FPS / 25 Mbps / 1080p。",
                  "• PCが高性能でない場合、クリップがカクつくことがあります。\n• 公開時に動画はいずれにせよ圧縮されます。オリジナルはPCに残ります。\n• 設定が高いほど良いクリップとは限りません。滑らかなフルHDはカクついたQHDより優れています。\n\nこれらはグラフィックカードが対応でき、操作を理解している場合のみ使用してください。",
                  "了解"),
    };

    /// <summary>Rebuilds the resolution list capped to the selected monitor's native height — you can't
    /// capture above native (the engine only downscales), the same honesty as the FPS cap — with the
    /// high outputs (1440p/4K) coloured by tier. They only appear on a monitor that can supply them.</summary>
    private void RebuildResolutionOptions()
    {
        bool wasInitializing = _isInitializing;
        _isInitializing = true;
        try
        {
            int prev = SelectedResolution?.TargetHeight ?? 0;
            int native = (int)(SelectedMonitor?.Height
                               ?? Monitors.FirstOrDefault(m => m.IsPrimary)?.Height ?? 1080);

            ResolutionOptions.Clear();
            ResolutionOptions.Add(new ResolutionOption(Localizer.Get("Option_Native"), 0, ResolutionTier(native)));
            foreach (int h in new[] { 2160, 1440, 1080, 720 })
                if (h <= native)
                    ResolutionOptions.Add(new ResolutionOption(ResolutionLabel(h), h, ResolutionTier(h)));

            SelectedResolution = ResolutionOptions.FirstOrDefault(r => r.TargetHeight == prev)
                                 ?? ResolutionOptions[0];
        }
        finally
        {
            _isInitializing = wasInitializing;
        }
    }

    private static string ResolutionLabel(int h) => h switch
    {
        2160 => "2160p (4K)",
        1440 => "1440p (QHD)",
        _ => $"{h}p",
    };

    // ───────────── Output File Format ─────────────

    /// <summary>Container formats for saved replays.</summary>
    // mp4 + mov + mkv are verified clean with our H.264/HEVC/AV1 + AAC streams (decode + stream-copy).
    // avi is omitted: it can't carry HEVC/AV1, so offering it would produce broken files.
    public IReadOnlyList<string> FormatOptions { get; } = new[] { "mp4", "mov", "mkv" };

    [ObservableProperty]
    private string _selectedFormat = "mp4";

    partial void OnSelectedFormatChanged(string value)
    {
        SaveSettingsRestartRequired();
    }

    // ───────────── Output Resolution (render downscale) ─────────────

    /// <summary>
    /// Output (render/encode) resolution presets. Capture always stays at the native screen
    /// resolution; this only downscales the encoded output (TargetHeight = 0 means "Native").
    /// The "Native" label is localized → list is rebuilt by <see cref="RebuildLocalizedOptions"/>.
    /// </summary>
    public System.Collections.ObjectModel.ObservableCollection<ResolutionOption> ResolutionOptions { get; } = new();

    [ObservableProperty]
    private ResolutionOption _selectedResolution = null!;

    partial void OnSelectedResolutionChanged(ResolutionOption value)
    {
        SaveSettingsRestartRequired();
        MaybeWarnIntensive();
    }

    // ───────────── Video Codec ─────────────

    /// <summary>
    /// Encoder choice. "Auto" (empty id, default) keeps the automatic hardware-fallback chain
    /// (NVENC → AMF → QSV → x264). Picking a specific codec makes it the preferred encoder —
    /// the engine tries it first and only falls back to the chain if it can't be created.
    /// </summary>
    public System.Collections.ObjectModel.ObservableCollection<CodecOption> CodecOptions { get; } = new();

    [ObservableProperty]
    private CodecOption _selectedCodec = null!;

    partial void OnSelectedCodecChanged(CodecOption value)
    {
        SaveSettingsRestartRequired();
    }

    // ───────────── Library Auto-Cleanup (opt-in) ─────────────

    /// <summary>When enabled, the oldest clips are auto-deleted once the library exceeds the limit.</summary>
    [ObservableProperty]
    private bool _autoCleanupEnabled;

    partial void OnAutoCleanupEnabledChanged(bool value)
    {
        SaveSettings();
    }

    /// <summary>Available library size limits for the auto-cleanup feature.</summary>
    public IReadOnlyList<StorageLimitOption> StorageLimitOptions { get; } = new[]
    {
        new StorageLimitOption(10),
        new StorageLimitOption(25),
        new StorageLimitOption(50),
        new StorageLimitOption(100),
        new StorageLimitOption(200),
        new StorageLimitOption(500)
    };

    [ObservableProperty]
    private StorageLimitOption _selectedStorageLimit;

    partial void OnSelectedStorageLimitChanged(StorageLimitOption value)
    {
        SaveSettings();
    }

    // ───────────── Video Bitrate ─────────────

    /// <summary>Bitrate presets; Kbps = 0 means "Custom" (label localized → rebuilt on language switch).</summary>
    public System.Collections.ObjectModel.ObservableCollection<BitrateOption> BitrateOptions { get; } = new();

    [ObservableProperty]
    private BitrateOption _selectedBitrate = null!;

    partial void OnSelectedBitrateChanged(BitrateOption value)
    {
        OnPropertyChanged(nameof(IsCustomBitrate));
        SaveSettingsRestartRequired();
        MaybeWarnIntensive();
    }

    /// <summary>Custom bitrate in Mbps (used when the "Custom" preset is selected).</summary>
    [ObservableProperty]
    private int _customBitrateMbps = 20;

    partial void OnCustomBitrateMbpsChanged(int value)
    {
        SaveSettingsRestartRequired();
        MaybeWarnIntensive();
    }

    public bool IsCustomBitrate => SelectedBitrate.Kbps == 0;

    // Bitrate range. High values are ALLOWED (warn, don't block) but coloured by tier
    // and gated behind the intensive-quality dialog — the rolling buffer is in RAM, so RAM ≈ bitrate
    // × bufferSeconds / 8 (100 Mbps × 5 min ≈ 3.75 GB). These are just absolute sanity bounds so a
    // custom value can't go truly insane (the old code allowed 300).
    private const int MinBitrateMbps = 3;
    private const int MaxBitrateMbps = 100;

    /// <summary>The bitrate that actually goes to the encoder, in kbps.</summary>
    public int EffectiveBitrateKbps =>
        SelectedBitrate.Kbps > 0 ? SelectedBitrate.Kbps : Math.Clamp(CustomBitrateMbps, MinBitrateMbps, MaxBitrateMbps) * 1000;

    /// <summary>
    /// Figma-style bitrate slider (Mbps). Reads the effective bitrate; writing snaps the
    /// selection to "Custom" with the chosen value, so the engine always gets exactly it.
    /// </summary>
    public double BitrateSliderValue
    {
        get => EffectiveBitrateKbps / 1000.0;
        set
        {
            int mbps = Math.Clamp((int)Math.Round(value), MinBitrateMbps, MaxBitrateMbps);
            CustomBitrateMbps = mbps;
            var preset = BitrateOptions.FirstOrDefault(b => b.Kbps == mbps * 1000);
            SelectedBitrate = preset ?? BitrateOptions.First(b => b.Kbps == 0);
            OnPropertyChanged(nameof(BitrateSliderValue));
            OnPropertyChanged(nameof(BitrateDisplayMbps));
            MaybeWarnIntensive();
        }
    }

    /// <summary>Right-side readout for the bitrate row ("50").</summary>
    public int BitrateDisplayMbps => EffectiveBitrateKbps / 1000;

    // ───────────── Recording GPU ─────────────

    /// <summary>Available GPU adapters: "Auto (name of primary)" + each physical adapter.</summary>
    public IReadOnlyList<GpuOption> GpuOptions { get; private set; } = [];

    [ObservableProperty]
    private GpuOption _selectedGpu;

    partial void OnSelectedGpuChanged(GpuOption value)
    {
        SaveSettingsRestartRequired();
    }

    private IReadOnlyList<GpuOption> BuildGpuOptions()
    {
        var gpus = _hardwareDetector.GetGpuAdapters();
        var list = new List<GpuOption> { new(-1, $"Auto ({gpus[0].Name})") };
        foreach (var gpu in gpus)
            list.Add(new GpuOption(gpu.Index, $"GPU {gpu.Index}: {gpu.Name}"));
        return list;
    }

    // ───────────── System Audio Capture (all / specific apps) ─────────────

    /// <summary>0 = all PC audio, 1 = specific apps only (drives the ComboBox).</summary>
    [ObservableProperty]
    private int _audioModeIndex;

    partial void OnAudioModeIndexChanged(int value)
    {
        OnPropertyChanged(nameof(IsAppsMode));
        SaveSettingsRestartRequired();
    }

    public bool IsAppsMode => AudioModeIndex == 1;

    /// <summary>Live list of apps playing audio (auto-refreshed) merged with saved selections.</summary>
    public System.Collections.ObjectModel.ObservableCollection<AppAudioItem> AudioApps { get; } = new();

    private Avalonia.Threading.DispatcherTimer? _audioAppsTimer;

    // Reuses the recorder's exact game classification to keep games out of the per-app list.
    private readonly Lag.Services.VfrCapture.GameDetector _audioGameDetector = new();

    /// <summary>Kicks off a background scan of apps currently playing audio and merges the result.
    /// Any process currently detected as a GAME is dropped — its sound is the dedicated "game audio"
    /// row, so listing it again as a separate app (cs2, PUBG, …) was double/confusing.</summary>
    private void RefreshAudioAppsNow()
    {
        string? monitorId = SelectedMonitor?.DeviceName;   // read on the UI thread before the scan
        _ = Task.Run(() =>
        {
            var apps = AudioSessionService.GetActiveAudioApps();
            try
            {
                var games = _audioGameDetector.RunningGameExes(monitorId);
                if (games.Count > 0) apps.RemoveAll(a => games.Contains(a.ExeName));
            }
            catch { /* detection is best-effort — never break the picker over it */ }
            return apps;
        }).ContinueWith(t =>
        {
            if (t.IsCompletedSuccessfully)
                Avalonia.Threading.Dispatcher.UIThread.Post(() => MergeAudioApps(t.Result));
        });
    }

    /// <summary>
    /// Merges the freshly scanned RUNNING apps into the visible list: new apps are appended (with
    /// their icon), apps that have closed are dropped UNLESS the user has them enabled (so a
    /// selection survives the app being closed temporarily).
    /// </summary>
    private void MergeAudioApps(List<AudioSessionService.AudioApp> live)
    {
        foreach (var app in live)
        {
            var existing = AudioApps.FirstOrDefault(a => a.Exe.Equals(app.ExeName, StringComparison.OrdinalIgnoreCase));
            if (existing == null)
                AudioApps.Add(new AppAudioItem(app.ExeName, app.DisplayName, OnAppAudioChanged, app.IconPng));
            else
                existing.UpdateMeta(app.DisplayName, app.IconPng);   // fill icon/name on a stale/restored row
        }

        for (int i = AudioApps.Count - 1; i >= 0; i--)
        {
            var item = AudioApps[i];
            bool stillRunning = live.Any(l => l.ExeName.Equals(item.Exe, StringComparison.OrdinalIgnoreCase));
            if (!stillRunning && !item.IsEnabled)
                AudioApps.RemoveAt(i);
        }
    }

    private void OnAppAudioChanged() => SaveSettingsRestartRequired();

    // ───────────── System audio source ("Звук системи" row: enable + volume) ─────────────

    /// <summary>Capture the full-system loopback (used in "Весь звук ПК" mode). Its row checkbox.</summary>
    [ObservableProperty]
    private bool _systemAudioEnabled = true;
    partial void OnSystemAudioEnabledChanged(bool value) => SaveSettingsRestartRequired();

    /// <summary>System-audio volume in percent (0–100), applied as a 0.0–1.0 gain to the loopback.</summary>
    [ObservableProperty]
    private int _systemAudioVolume = 100;
    partial void OnSystemAudioVolumeChanged(int value) => SaveSettingsRestartRequired();

    /// <summary>Microphone row checkbox — off = no mic captured at all.</summary>
    [ObservableProperty]
    private bool _micEnabled = true;
    partial void OnMicEnabledChanged(bool value) => SaveSettingsRestartRequired();

    /// <summary>"Звук гри" row (specific-apps mode): capture the detected game's own audio so it's
    /// always recorded even if it isn't in the picked-apps list, with its own volume.</summary>
    [ObservableProperty]
    private bool _gameAudioEnabled = true;
    partial void OnGameAudioEnabledChanged(bool value) => SaveSettingsRestartRequired();

    [ObservableProperty]
    private int _gameAudioVolume = 100;
    partial void OnGameAudioVolumeChanged(int value) => SaveSettingsRestartRequired();

    // ───────────── Microphone Volume ─────────────

    /// <summary>Microphone volume in percent (0–100). Applied to the OBS mic source as a 0.0–1.0 gain.</summary>
    [ObservableProperty]
    private int _micVolume = 100;

    partial void OnMicVolumeChanged(int value)
    {
        SaveSettingsRestartRequired();
    }

    // ───────────── Microphone Channels (stereo / mono) ─────────────

    /// <summary>0 = stereo, 1 = mono (drives the ComboBox).</summary>
    [ObservableProperty]
    private int _micChannelIndex;

    partial void OnMicChannelIndexChanged(int value)
    {
        SaveSettingsRestartRequired();
    }

    public bool MicMono => MicChannelIndex == 1;

    // ───────────── Push-to-talk ─────────────

    /// <summary>When on, the mic is muted and live only while the PTT key is held.</summary>
    [ObservableProperty]
    private bool _pushToTalkEnabled;

    partial void OnPushToTalkEnabledChanged(bool value)
    {
        ApplyPttToManager();
        SaveSettings();
    }

    private KeyCode _pttKey = KeyCode.VcV;

    [ObservableProperty]
    private string _pttKeyDisplayText = "V";

    [ObservableProperty]
    private bool _isCapturingPttKey;

    /// <summary>True while the next captured key should be bound as the PTT key (not the save hotkey).</summary>
    private bool _pttCaptureMode;

    [RelayCommand]
    private void CapturePttKey()
    {
        _pttCaptureMode = true;
        IsCapturingPttKey = true;
        _hotkeyManager.IsCapturing = true;
    }

    private void ApplyPttToManager()
    {
        _hotkeyManager.PttEnabled = PushToTalkEnabled;
        _hotkeyManager.PttKey = _pttKey;
    }

    // ───────────── Separate Audio Tracks ─────────────

    /// <summary>Save system audio (track 1) and mic (track 2) as separate tracks in the file.</summary>
    [ObservableProperty]
    private bool _separateAudioTracks;

    partial void OnSeparateAudioTracksChanged(bool value)
    {
        SaveSettingsRestartRequired();
    }

    // ───────────── Automation ─────────────

    /// <summary>The Run-key registry value name used for "Start with Windows".</summary>
    private const string AutoRunKeyName = "Lag";
    private const string AutoRunRegistryPath = @"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";

    /// <summary>Launch the app automatically when Windows starts (HKCU ...\Run registry entry).</summary>
    [ObservableProperty]
    private bool _startWithWindows;

    partial void OnStartWithWindowsChanged(bool value)
    {
        // Don't touch the registry while loading persisted state — only on a real user toggle.
        if (!_isInitializing)
            ApplyStartWithWindows(value);
        SaveSettings();
    }

    /// <summary>Automatically begin recording into the replay buffer as soon as the app launches.</summary>
    [ObservableProperty]
    private bool _autoStartRecording;

    partial void OnAutoStartRecordingChanged(bool value)
    {
        SaveSettings();
    }

    /// <summary>Launch hidden in the system tray instead of showing the main window.</summary>
    [ObservableProperty]
    private bool _startMinimized;

    partial void OnStartMinimizedChanged(bool value)
    {
        SaveSettings();
    }

    /// <summary>
    /// Disable Windows Game Mode. Game Mode deprioritizes background apps
    /// while a game is focused, which throttles the capture pipeline to a few FPS and
    /// delays hotkey handling. Default ON — strongly recommended.
    /// </summary>
    [ObservableProperty]
    private bool _disableGameMode = true;

    partial void OnDisableGameModeChanged(bool value)
    {
        if (!_isInitializing)
            ApplyDisableGameMode(value);
        SaveSettings();
    }

    /// <summary>
    /// Toggles Windows Game Mode via the HKCU GameBar keys.
    /// disable=true → Game Mode off; false → restored to the Windows default (on).
    /// </summary>
    private static void ApplyDisableGameMode(bool disable)
    {
        try
        {
            using RegistryKey key = Registry.CurrentUser.CreateSubKey(@"SOFTWARE\Microsoft\GameBar");
            int value = disable ? 0 : 1;
            key.SetValue("AllowAutoGameMode", value, RegistryValueKind.DWord);
            key.SetValue("AutoGameModeEnabled", value, RegistryValueKind.DWord);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to toggle Windows Game Mode: {ex.Message}");
        }
    }

    /// <summary>
    /// Adds or removes the application from the Windows startup (Run) registry key so it launches
    /// automatically at logon. Uses <see cref="Environment.ProcessPath"/> as the target executable.
    /// </summary>
    private static void ApplyStartWithWindows(bool enable)
    {
        try
        {
            using RegistryKey? runKey = Registry.CurrentUser.OpenSubKey(AutoRunRegistryPath, writable: true);
            if (runKey == null) return;

            if (enable)
            {
                string? exePath = Environment.ProcessPath;
                if (!string.IsNullOrEmpty(exePath))
                    runKey.SetValue(AutoRunKeyName, $"\"{exePath}\"");
            }
            else
            {
                if (runKey.GetValue(AutoRunKeyName) != null)
                    runKey.DeleteValue(AutoRunKeyName, throwOnMissingValue: false);
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to update 'Start with Windows' registry entry: {ex.Message}");
        }
    }

    // ───────────── Language ─────────────

    /// <summary>Available UI languages (each name shown in its own language). English is the default.</summary>
    public IReadOnlyList<LanguageOption> LanguageOptions { get; } = new[]
    {
        new LanguageOption("en", "English"),
        new LanguageOption("uk", "Українська"),
        new LanguageOption("de", "Deutsch"),
        new LanguageOption("fr", "Français"),
        new LanguageOption("be", "Беларуская"),
        new LanguageOption("lt", "Lietuvių"),
        new LanguageOption("et", "Eesti"),
        new LanguageOption("lv", "Latviešu"),
        new LanguageOption("fi", "Suomi"),
        new LanguageOption("sv", "Svenska"),
        new LanguageOption("no", "Norsk"),
        new LanguageOption("da", "Dansk"),
        new LanguageOption("nl", "Nederlands"),
        new LanguageOption("it", "Italiano"),
        new LanguageOption("es", "Español"),
        new LanguageOption("pt", "Português"),
        new LanguageOption("ja", "日本語")
    };

    [ObservableProperty]
    private LanguageOption _selectedLanguage;

    partial void OnSelectedLanguageChanged(LanguageOption value)
    {
        // Switch the live UI language immediately (applies even while loading the persisted value),
        // then rebuild every dropdown whose item labels are localized strings.
        Lag.App.SetLanguage(value.Code);
        RebuildLocalizedOptions();
        SaveSettings();
        // The intensive-quality disclaimer text is built in-VM (en/uk), so refresh it on a switch.
        OnPropertyChanged(nameof(IntensiveWarnTitle));
        OnPropertyChanged(nameof(IntensiveWarnSummary));
        OnPropertyChanged(nameof(IntensiveWarnBody));
        OnPropertyChanged(nameof(IntensiveWarnOk));
    }

    /// <summary>
    /// (Re)builds all option lists whose item LABELS are localized ("5 хв" / "5 min", "Auto",
    /// "Native", "Custom"). XAML {DynamicResource} can't reach inside data items, so on a language
    /// switch we regenerate the items and re-select the entry with the same underlying value.
    /// SaveSettings is suppressed during the churn — selections are value-identical.
    /// </summary>
    /// <summary>Codec FORMATS this machine can actually encode (probed by EncoderSelector):
    /// the user picks the format (H.264 / HEVC / AV1) and the engine auto-selects the best encoder
    /// (NVENC / AMF / QSV, x264 fallback) for it. Formats with no usable encoder here never appear, so
    /// e.g. AV1 is hidden on a GPU that can't do it — no confusing dead options.</summary>
    private static IEnumerable<(string Label, string Value)> AvailableCodecFormats()
    {
        var tiers = new HashSet<Lag.Services.VfrCapture.CodecTier>();
        try
        {
            foreach (var e in Lag.Services.VfrCapture.EncoderSelector.Available)
                tiers.Add(e.Tier);
        }
        catch { /* probe unavailable (e.g. FFmpeg not loaded) — fall back to H.264 below */ }

        bool any = false;
        if (tiers.Contains(Lag.Services.VfrCapture.CodecTier.H264)) { yield return ("H.264", "h264"); any = true; }
        if (tiers.Contains(Lag.Services.VfrCapture.CodecTier.Hevc)) { yield return ("H.265 (HEVC)", "hevc"); any = true; }
        if (tiers.Contains(Lag.Services.VfrCapture.CodecTier.Av1)) { yield return ("AV1", "av1"); any = true; }
        if (!any) yield return ("H.264", "h264");   // safety net — H.264 always encodes (libx264)
    }

    private void RebuildLocalizedOptions()
    {
        bool wasInitializing = _isInitializing;
        _isInitializing = true;
        try
        {
            // ── Buffer durations (default: 5 min) ──
            int selBufSec = (int)(SelectedBuffer?.Duration.TotalSeconds ?? 300);
            BufferOptions.Clear();
            foreach (var d in new[]
                     {
                         TimeSpan.FromSeconds(30), TimeSpan.FromMinutes(1), TimeSpan.FromMinutes(2),
                         TimeSpan.FromMinutes(5), TimeSpan.FromMinutes(10), TimeSpan.FromMinutes(15)
                     })
            {
                string label = d.TotalSeconds < 60
                    ? Localizer.Format("Time_SecShort", (int)d.TotalSeconds)
                    : Localizer.Format("Time_MinShort", (int)d.TotalMinutes);
                BufferOptions.Add(new BufferOption(label, d));
            }
            SelectedBuffer = BufferOptions.FirstOrDefault(b => (int)b.Duration.TotalSeconds == selBufSec)
                             ?? BufferOptions[3];

            // ── Output resolution (default: Native; capped to the monitor, coloured by tier) ──
            RebuildResolutionOptions();

            // ── Codec (default: Auto; FORMAT picker, only what this machine can encode) ──
            string selCodec = SelectedCodec?.EncoderId ?? "";
            CodecOptions.Clear();
            CodecOptions.Add(new CodecOption(Localizer.Get("Option_Auto"), ""));
            foreach (var (label, value) in AvailableCodecFormats())
                CodecOptions.Add(new CodecOption(label, value));
            SelectedCodec = CodecOptions.FirstOrDefault(c => c.EncoderId == selCodec) ?? CodecOptions[0];

            // ── Bitrate (default: 20 Mbps; range with high values coloured by tier) ──
            int selKbps = SelectedBitrate?.Kbps ?? 20000;
            BitrateOptions.Clear();
            foreach (int kbps in new[] { 3000, 5000, 7000, 10000, 15000, 20000, 25000, 30000, 50000, 70000, 100000 })
                BitrateOptions.Add(new BitrateOption($"{kbps / 1000} Mbps", kbps, BitrateTier(kbps / 1000)));
            BitrateOptions.Add(new BitrateOption(Localizer.Get("Option_Custom"), 0));
            SelectedBitrate = BitrateOptions.FirstOrDefault(b => b.Kbps == selKbps)
                              ?? BitrateOptions.FirstOrDefault(b => b.Kbps == 20000) ?? BitrateOptions[0];

            // ── FPS (default: 30; rungs capped to the selected monitor's refresh rate) ──
            RebuildFpsOptions();
        }
        finally
        {
            _isInitializing = wasInitializing;
        }
    }

    // ───────────── About / Auto-Update (Velopack) ─────────────

    // GitHub repository hosting the published Velopack releases.
    private const string UpdateRepoUrl = "https://github.com/shkbb/Lag";

    /// <summary>Currently installed app version (or "Debug/Dev" when not installed via Velopack).</summary>
    [ObservableProperty]
    private string _appVersion = "Debug/Dev";

    /// <summary>True while an update check/download is in progress (drives the UI loading state).</summary>
    [ObservableProperty]
    private bool _isCheckingForUpdate;

    /// <summary>Human-readable result of the last update check.</summary>
    [ObservableProperty]
    private string _updateStatus = string.Empty;

    /// <summary>Resolves the installed version via Velopack (local metadata, no network).</summary>
    private static string ResolveAppVersion()
    {
        try
        {
            var mgr = new UpdateManager(new GithubSource(UpdateRepoUrl, string.Empty, false));
            return mgr.IsInstalled && mgr.CurrentVersion != null
                ? mgr.CurrentVersion.ToString()
                : "Debug/Dev";
        }
        catch
        {
            return "Debug/Dev";
        }
    }

    /// <summary>
    /// Checks GitHub releases for a newer version; if found, downloads it and restarts into it.
    /// Does nothing in local/dev mode (not installed via Velopack).
    /// </summary>
    [RelayCommand]
    private async Task CheckForUpdatesAsync()
    {
        if (IsCheckingForUpdate) return;

        IsCheckingForUpdate = true;
        UpdateStatus = Lag.Core.Localizer.Get("Update_Checking");
        try
        {
            var mgr = new UpdateManager(new GithubSource(UpdateRepoUrl, string.Empty, false));

            // Don't check for updates in local debug mode (not installed via Velopack).
            if (!mgr.IsInstalled)
            {
                UpdateStatus = Lag.Core.Localizer.Get("Update_DevMode");
                return;
            }

            var newVersion = await mgr.CheckForUpdatesAsync();
            if (newVersion == null)
            {
                UpdateStatus = Lag.Core.Localizer.Get("Update_UpToDate");
                return;
            }

            // Download and restart into the new version.
            UpdateStatus = Lag.Core.Localizer.Get("Update_Downloading");
            await mgr.DownloadUpdatesAsync(newVersion);
            mgr.ApplyUpdatesAndRestart(newVersion);
        }
        catch (Exception ex)
        {
            Console.WriteLine($"Update failed: {ex.Message}");
            UpdateStatus = Lag.Core.Localizer.Format("Update_Failed", ex.Message);
        }
        finally
        {
            IsCheckingForUpdate = false;
        }
    }

    /// <summary>Silent auto-update run once at startup (there was no automatic check before — only the
    /// manual button). Checks GitHub, downloads a newer version in the background, and STAGES it to
    /// apply the next time the app fully exits — no mid-session restart, no UI. The manual button
    /// still does an immediate download+restart.</summary>
    public async Task AutoUpdateOnStartupAsync()
    {
        try
        {
            var mgr = new UpdateManager(new GithubSource(UpdateRepoUrl, string.Empty, false));
            if (!mgr.IsInstalled) return;                 // dev / portable build — nothing to update

            var newVersion = await mgr.CheckForUpdatesAsync();
            if (newVersion == null) return;               // already up to date

            await mgr.DownloadUpdatesAsync(newVersion);
            // Apply seamlessly when the app next exits; don't relaunch (the user closed it on purpose).
            mgr.WaitExitThenApplyUpdates(newVersion.TargetFullRelease, silent: true, restart: false);
            Console.WriteLine($"[AutoUpdate] {newVersion.TargetFullRelease.Version} downloaded — applies on next exit.");
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[AutoUpdate] check failed: {ex.Message}");
        }
    }

    public SettingsViewModel(
        Lag.Services.IReplayRecorder engine,
        GlobalHotkeyManager hotkeyManager,
        GlobalHotkeyService hotkeyService,
        HardwareDetector hardwareDetector)
    {
        Title = "Налаштування";
        _engine = engine;
        _hotkeyManager = hotkeyManager;
        _hotkeyService = hotkeyService;
        _hardwareDetector = hardwareDetector;

        // Suppress saves while we build the initial state, so device enumeration can't clobber
        // the persisted settings file before LoadSettings() reads it.
        _isInitializing = true;
        try
        {
            // Establish safe defaults FIRST (saves are suppressed by _isInitializing).
            _selectedLanguage = LanguageOptions[0]; // English by default
            _selectedStorageLimit = StorageLimitOptions[2]; // 50 GB (used only when cleanup is enabled)
            GpuOptions = BuildGpuOptions();
            _selectedGpu = GpuOptions[0];               // Auto by default

            // Build the localized dropdown lists (buffer 5 min, Native, Auto, 20 Mbps, 30 fps).
            RebuildLocalizedOptions();

            _appVersion = ResolveAppVersion();
            _libraryPath = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyVideos), "Lag");

            // Populate device lists BEFORE LoadSettings so the persisted monitor/mic can be matched.
            RefreshMonitors();
            RefreshMicrophones();

            // Override defaults + device selections with persisted values when settings.json exists.
            LoadSettings();
        }
        finally
        {
            _isInitializing = false;
        }

        // Register the persisted (or default) hotkey with the active Win32 service at startup.
        ApplyHotkeyToGlobalService();
        ApplyScreenshotHotkeyToService();

        // Re-assert "Game Mode off" each launch when enabled (Windows updates and the
        // user toggling it back in Windows Settings would otherwise silently undo it).
        if (DisableGameMode)
            ApplyDisableGameMode(true);

        // Mirror the persisted push-to-talk state onto the global hook.
        ApplyPttToManager();

        // Listen for hotkey capture events
        _hotkeyManager.HotkeyCaptured += OnHotkeyCaptured;

        // Live "apps playing audio" scanner: scan now, then every 4 s.
        RefreshAudioAppsNow();
        _audioAppsTimer = new Avalonia.Threading.DispatcherTimer { Interval = TimeSpan.FromSeconds(4) };
        _audioAppsTimer.Tick += (_, _) => RefreshAudioAppsNow();
        _audioAppsTimer.Start();
    }

    /// <summary>
    /// Mirrors the currently bound combination (modifiers + key) onto the active Win32 global
    /// hotkey so a rebind takes effect immediately and persists across restarts.
    /// </summary>
    private void ApplyHotkeyToGlobalService()
    {
        try
        {
            _hotkeyService.UpdateHotkey(_hotkeyManager.RequiredModifiers, _hotkeyManager.RequiredKey);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to apply hotkey to global service: {ex.Message}");
        }
    }

    /// <summary>
    /// Enters hotkey capture mode. The next key press will be bound as the new hotkey.
    /// </summary>
    [RelayCommand]
    private void CaptureHotkey()
    {
        IsCapturingHotkey = true;
        HotkeyDisplayText = Lag.Core.Localizer.Get("Hotkey_PressCombo");
        _hotkeyManager.IsCapturing = true;
    }

    /// <summary>
    /// Hotkey ACTIONS are ignored while a capture is in progress and for a short grace
    /// period right after it. Without this, the combo being typed fires immediately:
    /// the old Win32 registration is still live during capture, and re-registering the
    /// new combo while its keys are physically held lets keyboard auto-repeat trigger it.
    /// Checked by MainViewModel and the Win32 hotkey handler in App.
    /// </summary>
    public bool AreHotkeysSuppressed =>
        IsCapturingHotkey || IsCapturingPttKey || IsCapturingScreenshotKey ||
        DateTime.UtcNow < _hotkeySuppressedUntil;

    private DateTime _hotkeySuppressedUntil = DateTime.MinValue;

    /// <summary>
    /// Handles the captured hotkey event from the GlobalHotkeyManager.
    /// Updates the display and saves the new binding.
    /// </summary>
    private void OnHotkeyCaptured(object? sender, HotkeyCapturedEventArgs e)
    {
        // Swallow the keys the user is still holding (auto-repeat, late releases).
        _hotkeySuppressedUntil = DateTime.UtcNow.AddMilliseconds(800);

        if (_screenshotCaptureMode)
        {
            _screenshotCaptureMode = false;
            _screenshotKey = e.Key;
            _screenshotModifiers = e.Modifiers;
            ScreenshotHotkeyDisplayText = FormatHotkey(e.Modifiers, e.Key);
            IsCapturingScreenshotKey = false;

            ApplyScreenshotHotkeyToService();
            SaveSettings();
            return;
        }

        // Push-to-talk capture takes the SINGLE key only (modifiers ignored — PTT is a held key).
        if (_pttCaptureMode)
        {
            _pttCaptureMode = false;
            _pttKey = e.Key;
            PttKeyDisplayText = e.Key.ToString().Replace("Vc", "");
            IsCapturingPttKey = false;

            ApplyPttToManager();
            SaveSettings();
            return;
        }

        _hotkeyManager.RequiredKey = e.Key;
        _hotkeyManager.RequiredModifiers = e.Modifiers;

        HotkeyDisplayText = FormatHotkey(e.Modifiers, e.Key);
        IsCapturingHotkey = false;

        // Apply the new combination to the active Win32 hotkey, then persist it.
        ApplyHotkeyToGlobalService();
        SaveSettings();
    }

    /// <summary>
    /// Formats modifier + key into a human-readable string (e.g., "Alt + F10").
    /// </summary>
    private static string FormatHotkey(ModifierMask modifiers, KeyCode key)
    {
        var parts = new List<string>();

        if (modifiers.HasFlag(ModifierMask.LeftCtrl) || modifiers.HasFlag(ModifierMask.RightCtrl))
            parts.Add("Ctrl");
        if (modifiers.HasFlag(ModifierMask.LeftAlt) || modifiers.HasFlag(ModifierMask.RightAlt))
            parts.Add("Alt");
        if (modifiers.HasFlag(ModifierMask.LeftShift) || modifiers.HasFlag(ModifierMask.RightShift))
            parts.Add("Shift");

        // Convert KeyCode enum name to readable format (e.g., VcF10 → F10)
        string keyName = key.ToString().Replace("Vc", "");
        parts.Add(keyName);

        return string.Join(" + ", parts);
    }

    [RelayCommand]
    private async Task BrowseFolderAsync()
    {
        if (Avalonia.Application.Current?.ApplicationLifetime is Avalonia.Controls.ApplicationLifetimes.IClassicDesktopStyleApplicationLifetime desktop && desktop.MainWindow != null)
        {
            var storageProvider = Avalonia.Controls.TopLevel.GetTopLevel(desktop.MainWindow)?.StorageProvider;
            if (storageProvider != null)
            {
                var result = await storageProvider.OpenFolderPickerAsync(new Avalonia.Platform.Storage.FolderPickerOpenOptions
                {
                    Title = Lag.Core.Localizer.Get("Browse_Title"),
                    AllowMultiple = false
                });

                if (result != null && result.Count > 0)
                {
                    LibraryPath = result[0].Path.LocalPath;
                    SaveSettings();
                }
            }
        }
    }

    // ───────────── Settings Persistence ─────────────

    /// <summary>Persists settings AND, while a recording is live, flags that the change only takes
    /// effect after the recording is restarted. Use ONLY for settings baked into the capture
    /// pipeline snapshot (everything in <see cref="Lag.Services.ObsIntegration.RecorderOptions"/>:
    /// monitor, fps, resolution, codec, bitrate, GPU, buffer, container, library folder, and all
    /// audio routing/levels). Settings that apply live — language, hotkeys, push-to-talk, Game
    /// Mode, autostart, library quota — call <see cref="SaveSettings"/> so they don't nag a
    /// pointless restart (the original bug: changing the language showed a "restart recording" hint
    /// even though it switches instantly).</summary>
    private void SaveSettingsRestartRequired()
    {
        // Honour the SAME suppression as SaveSettings: during construction and localized-option
        // rebuilds (a language switch reassigns the FPS/bitrate/resolution selections) the property
        // changes aren't real user edits — without this guard, switching language while recording
        // would raise the restart flag via that rebuild side-effect (the very bug we're fixing).
        if (_isInitializing) return;
        if (_engine.IsRecording) HasPendingChanges = true;
        SaveSettings();
    }

    private void SaveSettings()
    {
        // Never persist while constructing/loading — see _isInitializing.
        if (_isInitializing) return;

        try
        {
            var settings = new SettingsData
            {
                // Persist seconds (not minutes) so sub-minute buffers like "30 секунд" survive restart.
                BufferSeconds = BufferSeconds,
                HotkeyKey = _hotkeyManager.RequiredKey.ToString(),
                HotkeyModifiers = _hotkeyManager.RequiredModifiers.ToString(),
                ScreenshotKey = _screenshotKey.ToString(),
                ScreenshotModifiers = _screenshotModifiers.ToString(),
                LibraryPath = LibraryPath,
                FrameRate = EffectiveFps,
                FileFormat = SelectedFormat,
                MonitorDeviceName = SelectedMonitor?.DeviceName ?? string.Empty,
                MicrophoneId = SelectedMicrophone?.Id ?? string.Empty,
                StartWithWindows = StartWithWindows,
                AutoStartRecording = AutoStartRecording,
                StartMinimized = StartMinimized,
                DisableGameMode = DisableGameMode,
                Language = SelectedLanguage.Code,
                MicVolume = MicVolume,
                OutputResolutionHeight = SelectedResolution.TargetHeight,
                CodecName = SelectedCodec.EncoderId,
                AutoCleanupEnabled = AutoCleanupEnabled,
                MaxLibrarySizeGb = SelectedStorageLimit.Gb,
                BitrateKbps = EffectiveBitrateKbps,
                GpuIndex = SelectedGpu.Index,
                AudioCaptureMode = IsAppsMode ? "apps" : "all",
                AudioApps = AudioApps
                    .Where(a => a.IsEnabled || a.Volume != 100)
                    .Select(a => new AppAudioSetting { Exe = a.Exe, Enabled = a.IsEnabled, Volume = a.Volume })
                    .ToList(),
                PttEnabled = PushToTalkEnabled,
                PttKey = _pttKey.ToString(),
                MicMono = MicMono,
                SeparateAudioTracks = SeparateAudioTracks,
                SystemAudioEnabled = SystemAudioEnabled,
                SystemAudioVolume = SystemAudioVolume,
                MicEnabled = MicEnabled,
                GameAudioEnabled = GameAudioEnabled,
                GameAudioVolume = GameAudioVolume
            };

            string dir = Path.GetDirectoryName(SettingsFilePath)!;
            Directory.CreateDirectory(dir);
            string json = JsonSerializer.Serialize(settings, new JsonSerializerOptions { WriteIndented = true });
            File.WriteAllText(SettingsFilePath, json);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to save settings: {ex.Message}");
        }
    }

    private void LoadSettings()
    {
        try
        {
            if (!File.Exists(SettingsFilePath)) return;

            string json = File.ReadAllText(SettingsFilePath);
            var settings = JsonSerializer.Deserialize<SettingsData>(json);
            if (settings == null) return;

            // Prefer the new seconds field; fall back to the legacy minutes field for old settings files.
            int targetSeconds = settings.BufferSeconds > 0
                ? settings.BufferSeconds
                : settings.BufferMinutes * 60;
            SelectedBuffer = BufferOptions.FirstOrDefault(b =>
                (int)b.Duration.TotalSeconds == targetSeconds) ?? SelectedBuffer;

            if (Enum.TryParse<KeyCode>(settings.HotkeyKey, out var key))
                _hotkeyManager.RequiredKey = key;
            if (Enum.TryParse<ModifierMask>(settings.HotkeyModifiers, out var mod))
                _hotkeyManager.RequiredModifiers = mod;

            HotkeyDisplayText = FormatHotkey(_hotkeyManager.RequiredModifiers, _hotkeyManager.RequiredKey);

            if (Enum.TryParse<KeyCode>(settings.ScreenshotKey, out var shotKey))
                _screenshotKey = shotKey;
            if (Enum.TryParse<ModifierMask>(settings.ScreenshotModifiers, out var shotMod))
                _screenshotModifiers = shotMod;
            ScreenshotHotkeyDisplayText = FormatHotkey(_screenshotModifiers, _screenshotKey);


            LibraryPath = settings.LibraryPath ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyVideos), "Lag");
            // FPS: clamp to this monitor's refresh (the file may come from a faster-panel machine),
            // then match a preset, otherwise restore as "Custom".
            if (settings.FrameRate > 0)
            {
                int clamped = Math.Clamp(settings.FrameRate, 1, CurrentRefreshCap());
                var fpsPreset = FpsOptions.FirstOrDefault(f => f.Value == clamped);
                if (fpsPreset != null)
                {
                    SelectedFps = fpsPreset;
                }
                else
                {
                    CustomFps = clamped;
                    SelectedFps = FpsOptions.First(f => f.Value == 0); // Custom
                }
            }

            SelectedFormat = FormatOptions.Contains(settings.FileFormat) ? settings.FileFormat : "mp4";
            MicVolume = Math.Clamp(settings.MicVolume, 0, 100);
            SelectedResolution = ResolutionOptions.FirstOrDefault(r =>
                r.TargetHeight == settings.OutputResolutionHeight) ?? ResolutionOptions[0];
            SelectedCodec = CodecOptions.FirstOrDefault(c =>
                c.EncoderId == settings.CodecName) ?? CodecOptions[0];
            AutoCleanupEnabled = settings.AutoCleanupEnabled;
            SelectedStorageLimit = StorageLimitOptions.FirstOrDefault(l =>
                l.Gb == settings.MaxLibrarySizeGb) ?? StorageLimitOptions[2];

            // Bitrate: match a preset, otherwise restore as "Custom".
            if (settings.BitrateKbps > 0)
            {
                var preset = BitrateOptions.FirstOrDefault(b => b.Kbps == settings.BitrateKbps);
                if (preset != null)
                {
                    SelectedBitrate = preset;
                }
                else
                {
                    CustomBitrateMbps = Math.Clamp(settings.BitrateKbps / 1000, MinBitrateMbps, MaxBitrateMbps);
                    SelectedBitrate = BitrateOptions.First(b => b.Kbps == 0); // Custom
                }
            }

            // GPU adapter (falls back to Auto when the saved adapter no longer exists).
            SelectedGpu = GpuOptions.FirstOrDefault(g => g.Index == settings.GpuIndex) ?? GpuOptions[0];

            // System audio capture mode + saved per-app selections.
            AudioModeIndex = settings.AudioCaptureMode == "apps" ? 1 : 0;
            SystemAudioEnabled = settings.SystemAudioEnabled;
            SystemAudioVolume = settings.SystemAudioVolume;
            MicEnabled = settings.MicEnabled;
            GameAudioEnabled = settings.GameAudioEnabled;
            GameAudioVolume = settings.GameAudioVolume;
            foreach (var saved in settings.AudioApps)
            {
                if (string.IsNullOrWhiteSpace(saved.Exe)) continue;
                if (AudioApps.Any(a => a.Exe.Equals(saved.Exe, StringComparison.OrdinalIgnoreCase))) continue;

                string display = saved.Exe.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)
                    ? saved.Exe[..^4] : saved.Exe;
                AudioApps.Add(new AppAudioItem(saved.Exe, display, OnAppAudioChanged)
                {
                    IsEnabled = saved.Enabled,
                    Volume = Math.Clamp(saved.Volume, 0, 100)
                });
            }

            // Push-to-talk + mic channels + tracks.
            PushToTalkEnabled = settings.PttEnabled;
            if (Enum.TryParse<KeyCode>(settings.PttKey, out var pttKey))
            {
                _pttKey = pttKey;
                PttKeyDisplayText = pttKey.ToString().Replace("Vc", "");
            }
            MicChannelIndex = settings.MicMono ? 1 : 0;
            SeparateAudioTracks = settings.SeparateAudioTracks;

            // Restore the persisted monitor/microphone by matching against the enumerated devices.
            if (!string.IsNullOrEmpty(settings.MonitorDeviceName))
            {
                var monitor = Monitors.FirstOrDefault(m => m.DeviceName == settings.MonitorDeviceName);
                if (monitor != null) SelectedMonitor = monitor;
            }

            if (!string.IsNullOrEmpty(settings.MicrophoneId))
            {
                var mic = Microphones.FirstOrDefault(m => m.Id == settings.MicrophoneId);
                if (mic != null) SelectedMicrophone = mic;
            }

            // Automation flags (registry side-effects are suppressed during init via _isInitializing).
            StartWithWindows = settings.StartWithWindows;
            AutoStartRecording = settings.AutoStartRecording;
            StartMinimized = settings.StartMinimized;
            DisableGameMode = settings.DisableGameMode;

            // Language (applies the persisted UI language via OnSelectedLanguageChanged).
            SelectedLanguage = LanguageOptions.FirstOrDefault(l => l.Code == settings.Language) ?? LanguageOptions[0];
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Failed to load settings: {ex.Message}");
        }
    }

    private class SettingsData
    {
        /// <summary>Buffer length in seconds (authoritative). Supports sub-minute values.</summary>
        public int BufferSeconds { get; set; }

        /// <summary>Legacy minutes field, kept only for reading older settings files.</summary>
        public int BufferMinutes { get; set; } = 5;
        public int MonitorIndex { get; set; }
        public string HotkeyKey { get; set; } = "VcF10";
        public string HotkeyModifiers { get; set; } = "LeftAlt";

        /// <summary>Screenshot hotkey (separate from the save-replay one). Default Alt+F9.</summary>
        public string ScreenshotKey { get; set; } = "VcF9";
        public string ScreenshotModifiers { get; set; } = "LeftAlt";
        public string FFmpegPath { get; set; } = "ffmpeg";
        public string LibraryPath { get; set; } = "";
        public int FrameRate { get; set; } = 30;
        public string MonitorDeviceName { get; set; } = "";
        public string MicrophoneId { get; set; } = "";
        public bool StartWithWindows { get; set; }
        public bool AutoStartRecording { get; set; }

        /// <summary>Launch hidden in the system tray (window not shown until requested).</summary>
        public bool StartMinimized { get; set; }

        /// <summary>Keep Windows Game Mode disabled (recommended for stable in-game capture).</summary>
        public bool DisableGameMode { get; set; } = true;

        /// <summary>Opt-in game_capture hook for exclusive-fullscreen games.</summary>
        public bool GameCaptureEnabled { get; set; }

        /// <summary>UI language code: "en" (default) or "uk".</summary>
        public string Language { get; set; } = "en";

        /// <summary>Microphone volume in percent (0–100).</summary>
        public int MicVolume { get; set; } = 100;

        /// <summary>Encoded output height (0 = native, 1080 = 1080p, 720 = 720p).</summary>
        public int OutputResolutionHeight { get; set; }

        /// <summary>Preferred encoder id ("" = automatic hardware fallback chain).</summary>
        public string CodecName { get; set; } = "";

        /// <summary>Opt-in: auto-delete oldest clips when the library exceeds the limit.</summary>
        public bool AutoCleanupEnabled { get; set; }

        /// <summary>Library size limit in GB for the auto-cleanup feature.</summary>
        public int MaxLibrarySizeGb { get; set; } = 50;

        /// <summary>Video encoder bitrate in kbps.</summary>
        public int BitrateKbps { get; set; } = 20000;

        /// <summary>DXGI adapter index for capture/render (-1 = Auto/primary).</summary>
        public int GpuIndex { get; set; } = -1;

        /// <summary>"all" = whole desktop audio; "apps" = selected applications only.</summary>
        public string AudioCaptureMode { get; set; } = "all";

        /// <summary>Per-application audio selections (checked apps and custom volumes).</summary>
        public List<AppAudioSetting> AudioApps { get; set; } = new();

        /// <summary>Push-to-talk enabled + its key.</summary>
        public bool PttEnabled { get; set; }
        public string PttKey { get; set; } = "VcV";

        /// <summary>Downmix microphone to mono.</summary>
        public bool MicMono { get; set; }

        /// <summary>System-audio source row: enabled + volume (default on, 100%).</summary>
        public bool SystemAudioEnabled { get; set; } = true;
        public int SystemAudioVolume { get; set; } = 100;

        /// <summary>Microphone row enabled (default on).</summary>
        public bool MicEnabled { get; set; } = true;

        /// <summary>"Звук гри" row: capture the detected game's audio + its volume (default on, 100%).</summary>
        public bool GameAudioEnabled { get; set; } = true;
        public int GameAudioVolume { get; set; } = 100;

        /// <summary>Save system audio and mic as separate tracks in the file.</summary>
        public bool SeparateAudioTracks { get; set; }


        /// <summary>Output container format: mp4 (default), mkv, mov or avi.</summary>
        public string FileFormat { get; set; } = "mp4";
    }

    /// <summary>Persisted per-application audio selection.</summary>
    public class AppAudioSetting
    {
        public string Exe { get; set; } = "";
        public bool Enabled { get; set; }
        public int Volume { get; set; } = 100;
    }
}

/// <summary>Replay buffer duration option for the Settings dropdown.</summary>
public record BufferOption(string Display, TimeSpan Duration)
{
    public override string ToString() => Display;
}

/// <summary>UI language option for the Settings dropdown.</summary>
public record LanguageOption(string Code, string Display)
{
    public override string ToString() => Display;
}

/// <summary>Output resolution preset (TargetHeight = 0 means native screen resolution).
/// Tier: 0 = normal, 1 = caution (amber), 2 = extreme (red) — drives the tier colouring.</summary>
public record ResolutionOption(string Display, int TargetHeight, int Tier = 0)
{
    public override string ToString() => Display;
}

/// <summary>Video codec option (EncoderId = "" means automatic selection).</summary>
public record CodecOption(string Display, string EncoderId)
{
    public override string ToString() => Display;
}

/// <summary>Library size limit option for auto-cleanup.</summary>
public record StorageLimitOption(int Gb)
{
    public override string ToString() => $"{Gb} GB";
}

/// <summary>Video bitrate preset (Kbps = 0 means "Custom"). Tier drives the colouring.</summary>
public record BitrateOption(string Display, int Kbps, int Tier = 0)
{
    public override string ToString() => Display;
}

/// <summary>Frame-rate preset (Value = 0 means "Custom"). Tier drives the colouring.</summary>
public record FpsOption(string Display, int Value, int Tier = 0)
{
    public override string ToString() => Display;
}

/// <summary>GPU adapter option (Index = -1 means Auto/primary).</summary>
public record GpuOption(int Index, string Display)
{
    public override string ToString() => Display;
}

/// <summary>
/// One row of the "record audio from these apps" list: checkbox + per-app volume.
/// Raises the supplied callback on every change so selections persist immediately.
/// </summary>
public partial class AppAudioItem : ObservableObject
{
    /// <summary>Executable name used to match the OBS application-audio capture (e.g. "Discord.exe").</summary>
    public string Exe { get; }

    /// <summary>Friendly name shown in the UI (app's FileDescription / window title / process name).
    /// Observable so a later scan can upgrade a restored "Process.exe" name to the real one.</summary>
    [ObservableProperty]
    private string _displayName;

    /// <summary>The app's icon for the list row (decoded from the exe), or null. Observable so the
    /// icon can fill in once the app is seen playing audio (restored entries start without one).</summary>
    [ObservableProperty]
    private Avalonia.Media.Imaging.Bitmap? _icon;

    private readonly Action _onChanged;

    [ObservableProperty]
    private bool _isEnabled;

    partial void OnIsEnabledChanged(bool value) => _onChanged();

    /// <summary>Per-application capture volume in percent (0–100).</summary>
    [ObservableProperty]
    private int _volume = 100;

    partial void OnVolumeChanged(int value) => _onChanged();

    public AppAudioItem(string exe, string displayName, Action onChanged, byte[]? iconPng = null)
    {
        Exe = exe;
        _displayName = displayName;
        _onChanged = onChanged;
        Icon = DecodeIcon(iconPng);
    }

    /// <summary>Refreshes a row with fresh scan data — upgrades the friendly name and fills the icon
    /// if it was missing (e.g. a restored selection that started as just the process name).</summary>
    public void UpdateMeta(string displayName, byte[]? iconPng)
    {
        if (!string.IsNullOrWhiteSpace(displayName)) DisplayName = displayName;
        if (Icon == null) { var ic = DecodeIcon(iconPng); if (ic != null) Icon = ic; }
    }

    private static Avalonia.Media.Imaging.Bitmap? DecodeIcon(byte[]? iconPng)
    {
        if (iconPng is not { Length: > 0 }) return null;
        try { return new Avalonia.Media.Imaging.Bitmap(new System.IO.MemoryStream(iconPng)); }
        catch { return null; }
    }
}
