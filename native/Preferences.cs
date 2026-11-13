using System.Linq;
using System.Text.Json;
using System.Threading.Tasks;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;

namespace AirSend;

public sealed partial class MainWindow
{
    private string themePreference = "system";

    private async Task LoadPreferences()
    {
        var language = await NativeEngine.Invoke(new { command = "get_language" });
        if (language.ValueKind == JsonValueKind.String)
            english = language.GetString() == "en";

        var theme = await NativeEngine.Invoke(new { command = "get_theme" });
        if (theme.ValueKind == JsonValueKind.String && theme.GetString() is "light" or "dark")
            themePreference = theme.GetString()!;

        ApplyTheme();
    }

    private void LocalizePreferences()
    {
        var settingsLabel = L("Ajustes", "Settings");
        SettingsButton.Content = settingsLabel;
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(SettingsButton, settingsLabel);
        ThemeChoice.Header = L("Tema", "Theme");
        ThemeChoice.SelectedIndex = -1;
        SystemThemeItem.Content = L("Del sistema", "System default");
        LightThemeItem.Content = L("Claro", "Light");
        DarkThemeItem.Content = L("Oscuro", "Dark");
        LanguageChoice.Header = L("Idioma", "Language");
        Updates.Content = L("Buscar actualizaciones", "Check for updates");
        SelectPreference(ThemeChoice, themePreference);
        SelectPreference(LanguageChoice, english ? "en" : "es");
    }

    private void OnNavigationItemInvoked(NavigationView sender, NavigationViewItemInvokedEventArgs args)
    {
        if (args.InvokedItemContainer == SettingsButton)
            FlyoutBase.ShowAttachedFlyout(SettingsButton);
    }

    private static void SelectPreference(ComboBox box, string value) =>
        box.SelectedItem = box.Items.OfType<ComboBoxItem>().First(item => (string)item.Tag == value);

    private async void OnThemeChanged(object sender, SelectionChangedEventArgs args)
    {
        if (!initialized || applyingSettings || ThemeChoice.SelectedItem is not ComboBoxItem { Tag: string theme }
            || theme == themePreference) return;
        if (busy) { SelectPreference(ThemeChoice, themePreference); return; }

        await Run(async () =>
        {
            try
            {
                await NativeEngine.Invoke(new { command = "save_theme", theme });
                themePreference = theme;
                ApplyTheme();
            }
            finally { SelectPreference(ThemeChoice, themePreference); }
        });
    }

    private async void OnLanguageChanged(object sender, SelectionChangedEventArgs args)
    {
        var current = english ? "en" : "es";
        if (!initialized || applyingSettings || LanguageChoice.SelectedItem is not ComboBoxItem { Tag: string language }
            || language == current) return;
        if (busy) { SelectPreference(LanguageChoice, current); return; }

        await Run(async () =>
        {
            try
            {
                await NativeEngine.Invoke(new { command = "save_language", language });
                english = language == "en";
                Localize();
                await ReadAudioOutputs();
            }
            finally { SelectPreference(LanguageChoice, english ? "en" : "es"); }
        });
    }

    private void ApplyTheme()
    {
        Root.RequestedTheme = themePreference switch
        {
            "light" => ElementTheme.Light,
            "dark" => ElementTheme.Dark,
            _ => ElementTheme.Default
        };
        UpdateCaptionTheme();
    }

    private void UpdateCaptionTheme()
    {
        if (!AppWindowTitleBar.IsCustomizationSupported()) return;
        var titleBar = AppWindow.TitleBar;
        var highContrast = new Windows.UI.ViewManagement.AccessibilitySettings().HighContrast;
        var foreground = Root.ActualTheme == ElementTheme.Dark ? Colors.White : Colors.Black;
        titleBar.ButtonForegroundColor = highContrast ? null : foreground;
        titleBar.ButtonInactiveForegroundColor = highContrast ? null : foreground;
    }
}
