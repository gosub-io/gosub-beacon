using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// <summary>
/// Visited pages, searchable. The engine returns rows newest-and-most-visited first and
/// keeps them valid until the next search, so this can re-query on every keystroke.
///
/// An empty query matches nothing rather than everything, so the prompt below says so
/// rather than showing an empty list.
/// </summary>
internal sealed class HistoryWindow : Window
{
    private readonly BeaconBrowser _browser;
    private readonly TextBox _query = new() { Padding = new Thickness(4, 3, 4, 3), FontSize = 13 };
    private readonly ListView _list = new();
    private readonly TextBlock _status = new() { Margin = new Thickness(8, 4, 8, 4), Foreground = System.Windows.Media.Brushes.Gray };

    public event Action<string>? Navigate;

    public HistoryWindow(BeaconBrowser browser)
    {
        _browser = browser;
        Title = "History";
        Width = 820;
        Height = 560;

        var grid = new GridView();
        grid.Columns.Add(new GridViewColumn
        {
            Header = "Title",
            Width = 320,
            DisplayMemberBinding = new System.Windows.Data.Binding(nameof(HistoryEntry.Title)),
        });
        grid.Columns.Add(new GridViewColumn
        {
            Header = "Address",
            Width = 400,
            DisplayMemberBinding = new System.Windows.Data.Binding(nameof(HistoryEntry.Url)),
        });
        grid.Columns.Add(new GridViewColumn
        {
            Header = "Visits",
            Width = 60,
            DisplayMemberBinding = new System.Windows.Data.Binding(nameof(HistoryEntry.VisitCount)),
        });
        _list.View = grid;
        _list.MouseDoubleClick += (_, _) => OpenSelected();

        _query.TextChanged += (_, _) => Search();
        _query.KeyDown += (_, e) =>
        {
            if (e.Key == Key.Return)
            {
                OpenSelected();
            }
        };

        var search = new DockPanel { Margin = new Thickness(8) };
        DockPanel.SetDock(_query, Dock.Right);
        var label = new TextBlock
        {
            Text = "Search:",
            VerticalAlignment = VerticalAlignment.Center,
            Margin = new Thickness(0, 0, 6, 0),
        };
        search.Children.Add(label);
        search.Children.Add(_query);

        var root = new DockPanel();
        DockPanel.SetDock(search, Dock.Top);
        DockPanel.SetDock(_status, Dock.Bottom);
        root.Children.Add(search);
        root.Children.Add(_status);
        root.Children.Add(_list);
        Content = root;

        _status.Text = "Type to search visited pages.";
        Loaded += (_, _) => _query.Focus();
    }

    private void Search()
    {
        var text = _query.Text.Trim();
        if (text.Length == 0)
        {
            _list.ItemsSource = null;
            _status.Text = "Type to search visited pages.";
            return;
        }

        var rows = _browser.SearchHistory(text, 200);
        _list.ItemsSource = rows;
        _status.Text = rows.Count switch
        {
            0 => $"No pages matching \"{text}\".",
            1 => "1 page.",
            _ => $"{rows.Count} pages.",
        };
    }

    private void OpenSelected()
    {
        if (_list.SelectedItem is HistoryEntry entry)
        {
            Navigate?.Invoke(entry.Url);
        }
    }
}
