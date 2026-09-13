using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// <summary>
/// The bookmarks bar. Like the tab strip, it holds no list of its own: it asks the engine
/// and rebuilds.
/// </summary>
internal sealed class BookmarksBar : UserControl
{
    private readonly StackPanel _panel = new() { Orientation = Orientation.Horizontal };
    private BeaconBrowser? _browser;

    public event Action<string>? Navigate;
    public event Action<string>? NavigateNewTab;

    public BookmarksBar()
    {
        Content = new ScrollViewer
        {
            HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
            VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
            Content = _panel,
        };
        Padding = new Thickness(4, 2, 4, 2);
    }

    public void Attach(BeaconBrowser browser) => _browser = browser;

    public void Rebuild()
    {
        if (_browser is null)
        {
            return;
        }

        _panel.Children.Clear();
        var bookmarks = _browser.Bookmarks();

        if (bookmarks.Count == 0)
        {
            _panel.Children.Add(new TextBlock
            {
                Text = "No bookmarks yet - Ctrl+D adds this page",
                Foreground = Brushes.Gray,
                FontSize = 11,
                VerticalAlignment = VerticalAlignment.Center,
                Margin = new Thickness(4, 0, 0, 0),
            });
            return;
        }

        foreach (var bookmark in bookmarks)
        {
            var label = string.IsNullOrWhiteSpace(bookmark.Title) ? bookmark.Url : bookmark.Title;
            var button = new Button
            {
                Content = new TextBlock
                {
                    Text = label,
                    MaxWidth = 160,
                    TextTrimming = TextTrimming.CharacterEllipsis,
                },
                ToolTip = bookmark.Url,
                Padding = new Thickness(6, 2, 6, 2),
                Margin = new Thickness(0, 0, 2, 0),
                BorderThickness = new Thickness(0),
                Background = Brushes.Transparent,
                Command = new Relay(() => Navigate?.Invoke(bookmark.Url)),
            };

            // Middle-click opens in a new tab.
            button.MouseDown += (_, e) =>
            {
                if (e.ChangedButton == MouseButton.Middle)
                {
                    NavigateNewTab?.Invoke(bookmark.Url);
                }
            };

            var menu = new ContextMenu();
            var open = new MenuItem { Header = "Open in New Tab" };
            open.Click += (_, _) => NavigateNewTab?.Invoke(bookmark.Url);
            menu.Items.Add(open);
            var copy = new MenuItem { Header = "Copy Address" };
            copy.Click += (_, _) => ClipboardSafe.SetText(bookmark.Url);
            menu.Items.Add(copy);
            button.ContextMenu = menu;

            _panel.Children.Add(button);
        }
    }
}

/// <summary>
/// Clipboard.SetText throws when another process holds the clipboard open. A failed copy
/// should not take the browser down with it.
/// </summary>
internal static class ClipboardSafe
{
    public static void SetText(string? text)
    {
        if (string.IsNullOrEmpty(text))
        {
            return;
        }

        try
        {
            Clipboard.SetText(text);
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[beacon] clipboard unavailable: {e.Message}");
        }
    }
}
