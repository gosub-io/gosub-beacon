using System.IO;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// <summary>
/// The tab strip: favicon, title, close button, pinning, drag-to-reorder and a context menu.
///
/// Rebuilt wholesale from the engine whenever the tabs change, so the shell keeps no tab
/// list of its own. Rebuilding a dozen buttons is cheap next to laying out a page.
/// </summary>
internal sealed class TabStrip : UserControl
{
    private readonly StackPanel _panel = new() { Orientation = Orientation.Horizontal };
    private BeaconBrowser? _browser;

    /// <summary>Favicon bytes are borrowed until the next call, so decoded images are cached
    /// by tab rather than re-fetched on every rebuild.</summary>
    private readonly Dictionary<ulong, ImageSource?> _icons = new();

    private ulong _dragging;

    public event Action<ulong>? TabActivated;
    public event Action<ulong>? TabClosed;
    public event Action? NewTabRequested;
    public event Action<ulong>? ViewSourceRequested;
    public event Action<ulong>? DuplicateRequested;

    public TabStrip()
    {
        var scroller = new ScrollViewer
        {
            HorizontalScrollBarVisibility = ScrollBarVisibility.Auto,
            VerticalScrollBarVisibility = ScrollBarVisibility.Disabled,
            Content = _panel,
        };
        Content = scroller;
        AllowDrop = true;
    }

    public void Attach(BeaconBrowser browser) => _browser = browser;

    /// <summary>Drop a cached icon so the next rebuild re-reads it.</summary>
    public void InvalidateFavicon(ulong tab) => _icons.Remove(tab);

    public void Rebuild()
    {
        if (_browser is null)
        {
            return;
        }

        var active = _browser.ActiveTab;
        var tabs = _browser.Tabs();

        // Forget icons for closed tabs, or the dictionary grows without bound.
        foreach (var stale in _icons.Keys.Where(k => !tabs.Contains(k)).ToList())
        {
            _icons.Remove(stale);
        }

        _panel.Children.Clear();
        foreach (var tab in tabs)
        {
            _panel.Children.Add(BuildTab(tab, tab == active));
        }

        _panel.Children.Add(new Button
        {
            Content = "+",
            Width = 28,
            Height = 24,
            Margin = new Thickness(2, 2, 0, 0),
            BorderThickness = new Thickness(0),
            Background = Brushes.Transparent,
            ToolTip = "New tab (Ctrl+T)",
            Command = new Relay(() => NewTabRequested?.Invoke()),
        });
    }

    private FrameworkElement BuildTab(ulong tab, bool isActive)
    {
        var browser = _browser!;
        var pinned = browser.TabIsPinned(tab);
        var url = browser.TabUrl(tab);
        var title = browser.TabTitle(tab);
        if (string.IsNullOrWhiteSpace(title))
        {
            title = url ?? "New tab";
        }

        var row = new StackPanel { Orientation = Orientation.Horizontal, VerticalAlignment = VerticalAlignment.Center };

        if (Favicon(tab) is { } icon)
        {
            row.Children.Add(new Image
            {
                Source = icon,
                Width = 16,
                Height = 16,
                Margin = new Thickness(0, 0, 6, 0),
                VerticalAlignment = VerticalAlignment.Center,
            });
        }

        // A pinned tab shows only its icon, but still needs something to click when the
        // site served none.
        if (!pinned || Favicon(tab) is null)
        {
            row.Children.Add(new TextBlock
            {
                Text = pinned ? "•" : title,
                MaxWidth = pinned ? 12 : 170,
                TextTrimming = TextTrimming.CharacterEllipsis,
                VerticalAlignment = VerticalAlignment.Center,
            });
        }

        if (!pinned)
        {
            var close = new Button
            {
                Content = "×",
                Width = 16,
                Height = 16,
                Padding = new Thickness(0),
                Margin = new Thickness(6, 0, 0, 0),
                FontSize = 12,
                BorderThickness = new Thickness(0),
                Background = Brushes.Transparent,
                ToolTip = "Close tab (Ctrl+W)",
                Command = new Relay(() => TabClosed?.Invoke(tab)),
            };
            row.Children.Add(close);
        }

        var button = new Button
        {
            Content = row,
            Padding = new Thickness(8, 4, 8, 4),
            Margin = new Thickness(0, 2, 1, 0),
            MaxWidth = pinned ? 46 : 240,
            Background = isActive ? Brushes.White : Brushes.Transparent,
            BorderThickness = new Thickness(0),
            ToolTip = url,
            Command = new Relay(() => TabActivated?.Invoke(tab)),
            ContextMenu = BuildContextMenu(tab, pinned),
            Tag = tab,
        };

        // Middle-click closes. Not offered on pinned tabs, which the engine refuses anyway.
        button.MouseDown += (_, e) =>
        {
            if (e.ChangedButton == MouseButton.Middle && !pinned)
            {
                TabClosed?.Invoke(tab);
            }
        };

        button.PreviewMouseMove += (s, e) =>
        {
            if (e.LeftButton == MouseButtonState.Pressed && _dragging == 0)
            {
                _dragging = tab;
                DragDrop.DoDragDrop((DependencyObject)s, tab, DragDropEffects.Move);
                _dragging = 0;
            }
        };

        button.AllowDrop = true;
        button.Drop += (_, e) =>
        {
            if (e.Data.GetData(typeof(ulong)) is ulong moved && moved != tab)
            {
                var index = _browser!.Tabs().ToList().IndexOf(tab);
                if (index >= 0)
                {
                    _browser.MoveTab(moved, index);
                    Rebuild();
                }
            }
        };

        return button;
    }

    private ContextMenu BuildContextMenu(ulong tab, bool pinned)
    {
        var menu = new ContextMenu();

        menu.Items.Add(Item("New Tab to the Right", () => NewTabRequested?.Invoke()));
        menu.Items.Add(Item("Duplicate Tab", () => DuplicateRequested?.Invoke(tab)));
        menu.Items.Add(new Separator());
        menu.Items.Add(Item("Reload", () => _browser?.Reload(tab, false)));
        menu.Items.Add(Item("View Source", () => ViewSourceRequested?.Invoke(tab)));
        menu.Items.Add(new Separator());
        menu.Items.Add(Item(pinned ? "Unpin Tab" : "Pin Tab", () =>
        {
            _browser?.SetTabPinned(tab, !pinned);
            Rebuild();
        }));
        menu.Items.Add(Item("Close Tab", () => TabClosed?.Invoke(tab), enabled: !pinned));
        menu.Items.Add(Item("Close Other Tabs", () =>
        {
            if (_browser is null)
            {
                return;
            }

            foreach (var other in _browser.Tabs().Where(t => t != tab && !_browser.TabIsPinned(t)).ToList())
            {
                TabClosed?.Invoke(other);
            }
        }));

        return menu;
    }

    private static MenuItem Item(string header, Action action, bool enabled = true)
    {
        var item = new MenuItem { Header = header, IsEnabled = enabled };
        item.Click += (_, _) => action();
        return item;
    }

    /// <summary>
    /// Decode the tab's icon, cached. The bytes arrive as the site served them, so a decode
    /// failure is normal and means "no icon".
    /// </summary>
    private ImageSource? Favicon(ulong tab)
    {
        if (_icons.TryGetValue(tab, out var cached))
        {
            return cached;
        }

        ImageSource? decoded = null;
        if (_browser?.TabFavicon(tab) is { Length: > 0 } bytes)
        {
            try
            {
                var image = new BitmapImage();
                image.BeginInit();
                image.StreamSource = new MemoryStream(bytes);
                image.CacheOption = BitmapCacheOption.OnLoad;
                image.EndInit();
                image.Freeze();
                decoded = image;
            }
            catch
            {
                // Something WPF cannot read; show no icon.
                decoded = null;
            }
        }

        _icons[tab] = decoded;
        return decoded;
    }
}

/// <summary>A command that runs an action, for menu items and tab buttons.</summary>
internal sealed class Relay(Action action) : ICommand
{
    public event EventHandler? CanExecuteChanged
    {
        add { }
        remove { }
    }

    public bool CanExecute(object? parameter) => true;

    public void Execute(object? parameter) => action();
}
