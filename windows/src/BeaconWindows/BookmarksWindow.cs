using System.Windows;
using System.Windows.Controls;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// <summary>The bookmark manager: the saved list, with open and copy.</summary>
internal sealed class BookmarksWindow : Window
{
    private readonly BeaconBrowser _browser;
    private readonly ListView _list = new();

    public event Action<string>? Navigate;

    public BookmarksWindow(BeaconBrowser browser)
    {
        _browser = browser;
        Title = "Bookmarks";
        Width = 760;
        Height = 520;

        var grid = new GridView();
        grid.Columns.Add(new GridViewColumn
        {
            Header = "Title",
            Width = 300,
            DisplayMemberBinding = new System.Windows.Data.Binding(nameof(Bookmark.Title)),
        });
        grid.Columns.Add(new GridViewColumn
        {
            Header = "Address",
            Width = 420,
            DisplayMemberBinding = new System.Windows.Data.Binding(nameof(Bookmark.Url)),
        });
        _list.View = grid;
        _list.MouseDoubleClick += (_, _) => OpenSelected();

        var open = new Button { Content = "Open", Width = 90, Margin = new Thickness(0, 0, 6, 0) };
        open.Click += (_, _) => OpenSelected();
        var copy = new Button { Content = "Copy Address", Width = 110, Margin = new Thickness(0, 0, 6, 0) };
        copy.Click += (_, _) =>
        {
            if (_list.SelectedItem is Bookmark b)
            {
                ClipboardSafe.SetText(b.Url);
            }
        };
        var refresh = new Button { Content = "Refresh", Width = 90 };
        refresh.Click += (_, _) => Reload();

        var buttons = new StackPanel
        {
            Orientation = Orientation.Horizontal,
            HorizontalAlignment = HorizontalAlignment.Right,
            Margin = new Thickness(8),
        };
        buttons.Children.Add(open);
        buttons.Children.Add(copy);
        buttons.Children.Add(refresh);

        var root = new DockPanel();
        DockPanel.SetDock(buttons, Dock.Bottom);
        root.Children.Add(buttons);
        root.Children.Add(_list);
        Content = root;

        Reload();
    }

    public void Reload() => _list.ItemsSource = _browser.Bookmarks();

    private void OpenSelected()
    {
        if (_list.SelectedItem is Bookmark b)
        {
            Navigate?.Invoke(b.Url);
        }
    }
}
