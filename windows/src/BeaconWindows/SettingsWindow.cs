using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// <summary>
/// The engine's settings store, which GTK draws as gosub://config.
///
/// The ABI hands over rows (key, description, type, value, default, constraint) and leaves
/// the presentation to the shell: a checkbox for a bool, a popup where the schema restricts
/// the value to literal choices, a text field otherwise with the accepted range beside it.
///
/// A write can be refused for an unknown key or a value outside its constraint, so every
/// commit checks the return and reverts the editor on false.
/// </summary>
internal sealed class SettingsWindow : Window
{
    private readonly BeaconBrowser _browser;
    private readonly TextBox _filter = new() { Padding = new Thickness(4, 3, 4, 3), FontSize = 13 };
    private readonly StackPanel _rows = new();
    private readonly TextBlock _status = new() { Margin = new Thickness(10, 4, 10, 6), Foreground = Brushes.Gray, FontSize = 11 };

    public SettingsWindow(BeaconBrowser browser)
    {
        _browser = browser;
        Title = "Settings - Gosub Beacon";
        Width = 900;
        Height = 680;

        _filter.TextChanged += (_, _) => Reload();

        var search = new DockPanel { Margin = new Thickness(10, 10, 10, 6) };
        var label = new TextBlock
        {
            Text = "Filter:",
            VerticalAlignment = VerticalAlignment.Center,
            Margin = new Thickness(0, 0, 6, 0),
        };
        DockPanel.SetDock(label, Dock.Left);
        search.Children.Add(label);
        search.Children.Add(_filter);

        var note = new TextBlock
        {
            // The engine reads net.* once at startup, so say so rather than leaving the
            // editor looking broken.
            Text = "Settings under net.* are read when the engine starts, so changes to them take effect next launch.",
            TextWrapping = TextWrapping.Wrap,
            Foreground = Brushes.DimGray,
            FontSize = 11,
            Margin = new Thickness(10, 0, 10, 6),
        };

        var root = new DockPanel();
        DockPanel.SetDock(search, Dock.Top);
        DockPanel.SetDock(note, Dock.Top);
        DockPanel.SetDock(_status, Dock.Bottom);
        root.Children.Add(search);
        root.Children.Add(note);
        root.Children.Add(_status);
        root.Children.Add(new ScrollViewer
        {
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            Content = _rows,
            Margin = new Thickness(10, 0, 10, 0),
        });
        Content = root;

        Reload();
    }

    private void Reload()
    {
        var filter = _filter.Text.Trim();
        var rows = _browser.SnapshotSettings(filter.Length == 0 ? null : filter);

        _rows.Children.Clear();
        foreach (var row in rows)
        {
            _rows.Children.Add(BuildRow(row));
        }

        var modified = rows.Count(r => r.IsModified);
        _status.Text = rows.Count == 0
            ? "No settings match that filter."
            : $"{rows.Count} setting{(rows.Count == 1 ? "" : "s")}, {modified} changed from default";
    }

    private FrameworkElement BuildRow(SettingRow row)
    {
        var grid = new Grid { Margin = new Thickness(0, 0, 0, 10) };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(360) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });

        var left = new StackPanel();
        left.Children.Add(new TextBlock
        {
            Text = row.Key,
            FontFamily = new FontFamily("Consolas"),
            FontWeight = row.IsModified ? FontWeights.Bold : FontWeights.Normal,
            TextWrapping = TextWrapping.Wrap,
        });

        if (row.Description is { Length: > 0 } description)
        {
            left.Children.Add(new TextBlock
            {
                Text = description,
                Foreground = Brushes.Gray,
                FontSize = 11,
                TextWrapping = TextWrapping.Wrap,
                Margin = new Thickness(0, 1, 8, 0),
            });
        }

        // The engine already gives the constraint as one line ("left | right", "-1 | 0-9999"),
        // so use it directly rather than rebuilding it from Range/Choices.
        var hint = row.Constraint ?? (row.Range is { } r ? $"{r.Lo}-{r.Hi}" : null);
        if (hint is { Length: > 0 })
        {
            left.Children.Add(new TextBlock
            {
                Text = $"accepts: {hint}",
                Foreground = Brushes.DarkGray,
                FontSize = 10,
                TextWrapping = TextWrapping.Wrap,
                Margin = new Thickness(0, 1, 8, 0),
            });
        }

        Grid.SetColumn(left, 0);
        grid.Children.Add(left);

        var editor = BuildEditor(row);
        Grid.SetColumn(editor, 1);
        grid.Children.Add(editor);

        var reset = new Button
        {
            Content = "Reset",
            Width = 64,
            Height = 24,
            Margin = new Thickness(8, 0, 0, 0),
            VerticalAlignment = VerticalAlignment.Top,
            IsEnabled = row.IsModified,
            ToolTip = row.IsModified ? $"Back to the default ({row.Default})" : "Already at its default",
        };
        reset.Click += (_, _) =>
        {
            if (_browser.ResetSetting(row.Key))
            {
                _status.Text = $"{row.Key} reset to {row.Default}";
                Reload();
            }
            else
            {
                _status.Text = $"The store refused to reset {row.Key}.";
            }
        };
        Grid.SetColumn(reset, 2);
        grid.Children.Add(reset);

        return grid;
    }

    private FrameworkElement BuildEditor(SettingRow row)
    {
        // A restricted set of literal choices means a popup, whatever the underlying type.
        if (row.Choices.Count > 0)
        {
            var combo = new ComboBox { VerticalAlignment = VerticalAlignment.Top, MinWidth = 160 };
            foreach (var choice in row.Choices)
            {
                combo.Items.Add(choice);
            }

            combo.SelectedItem = row.Value;
            combo.SelectionChanged += (_, _) =>
            {
                if (combo.SelectedItem is string chosen && chosen != row.Value)
                {
                    Commit(row, chosen, () => combo.SelectedItem = row.Value);
                }
            };
            return combo;
        }

        if (row.Type == BeaconNative.SettingType.Bool)
        {
            var check = new CheckBox
            {
                IsChecked = row.Value.Equals("true", StringComparison.OrdinalIgnoreCase),
                VerticalAlignment = VerticalAlignment.Top,
                Margin = new Thickness(0, 4, 0, 0),
            };
            check.Click += (_, _) =>
            {
                var wanted = check.IsChecked == true ? "true" : "false";
                Commit(row, wanted, () => check.IsChecked = !check.IsChecked);
            };
            return check;
        }

        var box = new TextBox
        {
            Text = row.Value,
            VerticalAlignment = VerticalAlignment.Top,
            Padding = new Thickness(3, 2, 3, 2),
            FontFamily = row.Type == BeaconNative.SettingType.Map ? new FontFamily("Consolas") : SystemFonts.MessageFontFamily,
        };

        // Commit on Enter or focus loss, not per keystroke: writes are typed by the key's
        // schema and a half-typed number is not a valid value.
        void CommitBox()
        {
            if (box.Text != row.Value)
            {
                Commit(row, box.Text, () => box.Text = row.Value);
            }
        }

        box.LostFocus += (_, _) => CommitBox();
        box.KeyDown += (_, e) =>
        {
            if (e.Key == System.Windows.Input.Key.Return)
            {
                CommitBox();
            }
        };
        return box;
    }

    /// <summary>
    /// Write, reverting the editor if the store refused. Reloads on success: a write changes
    /// the modified flag, and writing the default removes the override.
    /// </summary>
    private void Commit(SettingRow row, string value, Action revert)
    {
        if (_browser.SetSetting(row.Key, value))
        {
            _status.Text = $"{row.Key} = {value}";
            Reload();
            return;
        }

        revert();
        _status.Text = row.Constraint is { Length: > 0 } constraint
            ? $"\"{value}\" was refused for {row.Key}. Accepts: {constraint}"
            : $"\"{value}\" was refused for {row.Key}.";
    }
}
