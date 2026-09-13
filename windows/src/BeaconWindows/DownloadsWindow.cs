using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Threading;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// <summary>
/// Downloads, running and finished.
///
/// Progress is 0..1, or -1 when the server sent no length, which is common with chunked
/// responses. Those rows show an indeterminate bar and the bytes received; treating -1 as
/// 0% would leave an empty bar beside a working download.
/// </summary>
internal sealed class DownloadsWindow : Window
{
    private readonly BeaconBrowser _browser;
    private readonly StackPanel _rows = new();
    private readonly TextBlock _status = new() { Margin = new Thickness(10, 4, 10, 6), Foreground = Brushes.Gray, FontSize = 11 };
    private readonly DispatcherTimer _timer;

    public DownloadsWindow(BeaconBrowser browser)
    {
        _browser = browser;
        Title = "Downloads - Gosub Beacon";
        Width = 720;
        Height = 460;

        var root = new DockPanel();
        DockPanel.SetDock(_status, Dock.Bottom);
        root.Children.Add(_status);
        root.Children.Add(new ScrollViewer
        {
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            Content = _rows,
            Margin = new Thickness(10),
        });
        Content = root;

        // The engine emits DOWNLOAD_CHANGED, but polling keeps the bar moving between events.
        _timer = new DispatcherTimer(DispatcherPriority.Background) { Interval = TimeSpan.FromMilliseconds(500) };
        _timer.Tick += (_, _) => Reload();
        Loaded += (_, _) => { _timer.Start(); Reload(); };
        Closed += (_, _) => _timer.Stop();
    }

    public void Reload()
    {
        var downloads = _browser.Downloads();
        _rows.Children.Clear();

        foreach (var download in downloads)
        {
            _rows.Children.Add(BuildRow(download));
        }

        var running = downloads.Count(d => d.State == BeaconNative.DownloadState.Running);
        _status.Text = downloads.Count == 0
            ? "No downloads yet."
            : $"{downloads.Count} download{(downloads.Count == 1 ? "" : "s")}, {running} in progress";
    }

    private FrameworkElement BuildRow(DownloadRow download)
    {
        var panel = new StackPanel { Margin = new Thickness(0, 0, 0, 12) };

        panel.Children.Add(new TextBlock
        {
            Text = download.Filename,
            FontWeight = FontWeights.SemiBold,
            TextTrimming = TextTrimming.CharacterEllipsis,
        });

        if (download.Path is { Length: > 0 } path)
        {
            panel.Children.Add(new TextBlock
            {
                Text = path,
                Foreground = Brushes.Gray,
                FontSize = 11,
                TextTrimming = TextTrimming.CharacterEllipsis,
            });
        }

        var line = new DockPanel { Margin = new Thickness(0, 4, 0, 0) };

        if (download.State == BeaconNative.DownloadState.Running)
        {
            var bar = new ProgressBar
            {
                Height = 14,
                Width = 320,
                // No length from the server means no meaningful fraction.
                IsIndeterminate = download.Indeterminate,
                Minimum = 0,
                Maximum = 1,
                Value = download.Indeterminate ? 0 : download.Progress,
                VerticalAlignment = VerticalAlignment.Center,
            };
            line.Children.Add(bar);
        }

        var detail = new TextBlock
        {
            Margin = new Thickness(10, 0, 0, 0),
            VerticalAlignment = VerticalAlignment.Center,
            FontSize = 11,
            Foreground = download.State == BeaconNative.DownloadState.Failed ? Brushes.DarkRed : Brushes.DimGray,
            Text = download.State switch
            {
                BeaconNative.DownloadState.Finished => $"Finished · {DeveloperPanel.FormatBytes(download.Received)}",
                BeaconNative.DownloadState.Failed => "Failed",
                _ when download.Indeterminate => $"{DeveloperPanel.FormatBytes(download.Received)} received",
                _ => $"{download.Progress * 100:0}% · {DeveloperPanel.FormatBytes(download.Received)}",
            },
        };
        line.Children.Add(detail);

        if (download.State == BeaconNative.DownloadState.Finished)
        {
            var open = new Button
            {
                Content = "Open",
                Width = 70,
                Margin = new Thickness(10, 0, 0, 0),
                ToolTip = "Hand it to the desktop's default application",
            };
            open.Click += (_, _) => _browser.OpenDownload(download.Id);
            DockPanel.SetDock(open, Dock.Right);
            line.Children.Add(open);
        }

        panel.Children.Add(line);
        return panel;
    }
}
