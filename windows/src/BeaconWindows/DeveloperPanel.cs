using System.Windows;
using System.Windows.Controls;
using System.Windows.Data;
using System.Windows.Media;
using System.Windows.Threading;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// <summary>
/// Network, console and timing, over the developer-panel families in beacon.h.
///
/// Every list here is a snapshot: one call copies the records and returns a count, then
/// accessors read them by index. The buffers live in beacon-core and are shared with the GTK
/// and macOS panels, so all three show the same records.
///
/// Body capture and sensitive headers follow this window's visibility, so nothing is held in
/// memory while it is closed.
/// </summary>
internal sealed class DeveloperPanel : Window
{
    private readonly BeaconBrowser _browser;
    private readonly DispatcherTimer _timer;

    private readonly ListView _requests = new();
    private readonly TabControl _detailTabs = new();
    private readonly ListView _requestHeaders = new();
    private readonly ListView _responseHeaders = new();
    private readonly TextBox _body = new() { IsReadOnly = true, TextWrapping = TextWrapping.NoWrap, FontFamily = new FontFamily("Consolas"), VerticalScrollBarVisibility = ScrollBarVisibility.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Auto };
    private readonly ListView _redirects = new();
    private readonly TextBlock _diagnosis = new() { TextWrapping = TextWrapping.Wrap, Margin = new Thickness(8), Foreground = Brushes.DarkRed };
    private readonly TextBlock _netStatus = new() { Margin = new Thickness(8, 4, 8, 4), Foreground = Brushes.Gray, FontSize = 11 };

    private readonly ListView _log = new();
    private readonly ComboBox _levelFilter = new() { Width = 100, Margin = new Thickness(6, 0, 0, 0) };
    private readonly TextBlock _logStatus = new() { Margin = new Thickness(8, 4, 8, 4), Foreground = Brushes.Gray, FontSize = 11 };

    private readonly ListView _timing = new();
    private readonly TextBlock _timingStatus = new() { Margin = new Thickness(8, 4, 8, 4), Foreground = Brushes.Gray, FontSize = 11 };

    private readonly CheckBox _captureBodies = new() { Content = "Capture bodies", Margin = new Thickness(8, 0, 0, 0), VerticalAlignment = VerticalAlignment.Center };
    private readonly CheckBox _showSensitive = new() { Content = "Show Cookie / Authorization", Margin = new Thickness(8, 0, 0, 0), VerticalAlignment = VerticalAlignment.Center };
    private readonly CheckBox _thisTabOnly = new() { Content = "This tab only", IsChecked = true, Margin = new Thickness(8, 0, 0, 0), VerticalAlignment = VerticalAlignment.Center };

    /// <summary>Which tab the network list is scoped to; 0 means every tab.</summary>
    public ulong Tab { get; set; }

    private int _selectedIndex = -1;

    public DeveloperPanel(BeaconBrowser browser)
    {
        _browser = browser;
        Title = "Developer Tools - Gosub Beacon";
        Width = 1100;
        Height = 700;

        var tabs = new TabControl();
        tabs.Items.Add(new TabItem { Header = "Network", Content = BuildNetworkTab() });
        tabs.Items.Add(new TabItem { Header = "Console", Content = BuildConsoleTab() });
        tabs.Items.Add(new TabItem { Header = "Timing", Content = BuildTimingTab() });
        Content = tabs;

        // Twice a second: enough to watch a page load without the copying being noticeable.
        _timer = new DispatcherTimer(DispatcherPriority.Background) { Interval = TimeSpan.FromMilliseconds(500) };
        _timer.Tick += (_, _) => RefreshAll();

        Loaded += (_, _) =>
        {
            // Capture follows visibility.
            _browser.SetCaptureBodies(_captureBodies.IsChecked == true);
            _browser.SetShowSensitiveHeaders(_showSensitive.IsChecked == true);
            _timer.Start();
            RefreshAll();
        };

        Closed += (_, _) =>
        {
            _timer.Stop();
            _browser.SetCaptureBodies(false);
            _browser.SetShowSensitiveHeaders(false);
        };
    }

    // ── network ────────────────────────────────────────────────────────────────

    private FrameworkElement BuildNetworkTab()
    {
        var grid = new GridView();
        Column(grid, "Status", 70, nameof(NetRowView.StatusText));
        Column(grid, "Method", 60, nameof(NetRowView.Method));
        Column(grid, "Type", 80, nameof(NetRowView.Kind));
        Column(grid, "URL", 420, nameof(NetRowView.Url));
        Column(grid, "Size", 80, nameof(NetRowView.Size));
        Column(grid, "Time", 80, nameof(NetRowView.Time));
        Column(grid, "Phase", 90, nameof(NetRowView.Phase));
        Column(grid, "Initiator", 90, nameof(NetRowView.Initiator));
        _requests.View = grid;
        _requests.SelectionChanged += (_, _) =>
        {
            _selectedIndex = _requests.SelectedItem is NetRowView row ? row.Index : -1;
            RefreshNetwork();
        };

        HeaderGrid(_requestHeaders);
        HeaderGrid(_responseHeaders);

        var redirectGrid = new GridView();
        Column(redirectGrid, "Status", 70, nameof(Redirect.Status));
        Column(redirectGrid, "URL", 700, nameof(Redirect.Url));
        _redirects.View = redirectGrid;

        _detailTabs.Items.Add(new TabItem { Header = "Request Headers", Content = _requestHeaders });
        _detailTabs.Items.Add(new TabItem { Header = "Response Headers", Content = _responseHeaders });
        _detailTabs.Items.Add(new TabItem { Header = "Body", Content = _body });
        _detailTabs.Items.Add(new TabItem { Header = "Redirects", Content = _redirects });
        _detailTabs.Items.Add(new TabItem { Header = "Diagnosis", Content = new ScrollViewer { Content = _diagnosis } });

        var clear = new Button { Content = "Clear", Width = 70, Margin = new Thickness(0, 0, 4, 0) };
        clear.Click += (_, _) =>
        {
            _browser.ClearNetwork();
            _selectedIndex = -1;
            RefreshNetwork();
        };

        _captureBodies.Checked += (_, _) => _browser.SetCaptureBodies(true);
        _captureBodies.Unchecked += (_, _) => _browser.SetCaptureBodies(false);
        _showSensitive.Checked += (_, _) => _browser.SetShowSensitiveHeaders(true);
        _showSensitive.Unchecked += (_, _) => _browser.SetShowSensitiveHeaders(false);
        _thisTabOnly.Click += (_, _) => RefreshNetwork();

        var bar = new StackPanel { Orientation = Orientation.Horizontal, Margin = new Thickness(6) };
        bar.Children.Add(clear);
        bar.Children.Add(_thisTabOnly);
        bar.Children.Add(_captureBodies);
        bar.Children.Add(_showSensitive);

        var split = new Grid();
        split.RowDefinitions.Add(new RowDefinition { Height = new GridLength(3, GridUnitType.Star) });
        split.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        split.RowDefinitions.Add(new RowDefinition { Height = new GridLength(2, GridUnitType.Star) });
        Grid.SetRow(_requests, 0);
        var splitter = new GridSplitter { Height = 4, HorizontalAlignment = HorizontalAlignment.Stretch, Background = Brushes.LightGray };
        Grid.SetRow(splitter, 1);
        Grid.SetRow(_detailTabs, 2);
        split.Children.Add(_requests);
        split.Children.Add(splitter);
        split.Children.Add(_detailTabs);

        var root = new DockPanel();
        DockPanel.SetDock(bar, Dock.Top);
        DockPanel.SetDock(_netStatus, Dock.Bottom);
        root.Children.Add(bar);
        root.Children.Add(_netStatus);
        root.Children.Add(split);
        return root;
    }

    private void RefreshNetwork()
    {
        // Snapshot first, then read the selected row's detail from that same snapshot: the
        // accessors are only valid until the next one replaces it.
        var scope = _thisTabOnly.IsChecked == true ? Tab : 0UL;
        var rows = _browser.SnapshotNetwork(scope);

        var keepIndex = _selectedIndex;
        _requests.ItemsSource = rows.Select(NetRowView.From).ToList();
        if (keepIndex >= 0 && keepIndex < rows.Count)
        {
            _requests.SelectedIndex = keepIndex;
            ShowDetail(_browser.ReadNetDetail(keepIndex), rows[keepIndex]);
        }
        else
        {
            ShowDetail(null, null);
        }

        var held = _browser.CapturedBodyBytes;
        _netStatus.Text = rows.Count == 0
            ? "No requests recorded for this scope yet."
            : $"{rows.Count} request{(rows.Count == 1 ? "" : "s")} · {FormatBytes((ulong)held)} of bodies held";
    }

    private void ShowDetail(NetDetail? detail, NetRow? row)
    {
        if (detail is not { } d)
        {
            _requestHeaders.ItemsSource = null;
            _responseHeaders.ItemsSource = null;
            _redirects.ItemsSource = null;
            _body.Text = string.Empty;
            _diagnosis.Text = string.Empty;
            return;
        }

        _requestHeaders.ItemsSource = d.RequestHeaders;
        _responseHeaders.ItemsSource = d.ResponseHeaders;
        _redirects.ItemsSource = d.Redirects;

        // "No body captured" and "captured, then dropped for budget" are different facts.
        _body.Text = d.BodyText ?? (row?.Raw.BodyEvicted == true
            ? "(body was captured, then dropped to stay within the capture budget)"
            : row?.Raw.HasBody == true
                ? "(body not captured - turn on \"Capture bodies\" and reload)"
                : "(no body)");

        if (row?.Raw.BodyTruncated == true && d.BodyText is not null)
        {
            _body.Text += "\n\n(the response continued past the captured preview)";
        }

        var parts = new List<string>();
        if (d.Error is { Length: > 0 } error)
        {
            parts.Add($"Error: {error}");
        }

        if (row?.FailureLabel is { Length: > 0 } failure)
        {
            parts.Add($"Failure kind: {failure}");
        }

        if (d.FailureHint is { Length: > 0 } fh)
        {
            parts.Add(fh);
        }

        if (d.PhaseHint is { Length: > 0 } ph)
        {
            parts.Add($"Stuck in {row?.PhaseLabel ?? "this phase"}: {ph}");
        }

        _diagnosis.Text = parts.Count == 0 ? "Nothing to report for this request." : string.Join("\n\n", parts);
    }

    // ── console ────────────────────────────────────────────────────────────────

    private FrameworkElement BuildConsoleTab()
    {
        var grid = new GridView();
        Column(grid, "Time", 90, nameof(LogRowView.Time));
        Column(grid, "Level", 60, nameof(LogRowView.Level));
        Column(grid, "Target", 200, nameof(LogRowView.Target));
        Column(grid, "Message", 700, nameof(LogRowView.Message));
        _log.View = grid;

        foreach (var level in new[] { "All", "Error", "Warn", "Info", "Debug", "Trace" })
        {
            _levelFilter.Items.Add(level);
        }

        _levelFilter.SelectedIndex = 0;
        _levelFilter.SelectionChanged += (_, _) => RefreshConsole();

        var clear = new Button { Content = "Clear", Width = 70 };
        clear.Click += (_, _) =>
        {
            _browser.ClearLog();
            RefreshConsole();
        };

        var bar = new StackPanel { Orientation = Orientation.Horizontal, Margin = new Thickness(6) };
        bar.Children.Add(clear);
        bar.Children.Add(new TextBlock { Text = "Level:", VerticalAlignment = VerticalAlignment.Center, Margin = new Thickness(12, 0, 0, 0) });
        bar.Children.Add(_levelFilter);

        var root = new DockPanel();
        DockPanel.SetDock(bar, Dock.Top);
        DockPanel.SetDock(_logStatus, Dock.Bottom);
        root.Children.Add(bar);
        root.Children.Add(_logStatus);
        root.Children.Add(_log);
        return root;
    }

    private void RefreshConsole()
    {
        var rows = _browser.SnapshotLog();
        var wanted = _levelFilter.SelectedIndex;
        var filtered = wanted <= 0 ? rows : rows.Where(r => (int)r.Level == wanted).ToList();

        _log.ItemsSource = filtered.Select(LogRowView.From).ToList();

        // An empty console usually means the level filter: capture follows BEACON_LOG /
        // RUST_LOG, which default to warnings only.
        _logStatus.Text = rows.Count == 0
            ? "Nothing captured. Capture follows BEACON_LOG / RUST_LOG, which default to warnings only."
            : $"{filtered.Count} of {rows.Count} record{(rows.Count == 1 ? "" : "s")}";
    }

    // ── timing ─────────────────────────────────────────────────────────────────

    private FrameworkElement BuildTimingTab()
    {
        var grid = new GridView();
        Column(grid, "Namespace", 220, nameof(TimingRowView.Namespace));
        Column(grid, "Count", 70, nameof(TimingRowView.Count));
        Column(grid, "Total", 90, nameof(TimingRowView.Total));
        Column(grid, "Avg", 80, nameof(TimingRowView.Avg));
        Column(grid, "Min", 80, nameof(TimingRowView.Min));
        Column(grid, "Max", 80, nameof(TimingRowView.Max));
        Column(grid, "p50", 80, nameof(TimingRowView.P50));
        Column(grid, "p95", 80, nameof(TimingRowView.P95));
        Column(grid, "p99", 80, nameof(TimingRowView.P99));
        _timing.View = grid;

        // The engine describes what each namespace measures; show it as a tooltip.
        _timing.ItemContainerStyle = new Style(typeof(ListViewItem));
        _timing.ItemContainerStyle.Setters.Add(new Setter(ToolTipService.ToolTipProperty, new Binding(nameof(TimingRowView.Describes))));

        var reset = new Button { Content = "Reset", Width = 70, ToolTip = "Measure one navigation rather than every one since launch" };
        reset.Click += (_, _) =>
        {
            _browser.ResetTiming();
            RefreshTiming();
        };

        var bar = new StackPanel { Orientation = Orientation.Horizontal, Margin = new Thickness(6) };
        bar.Children.Add(reset);

        var root = new DockPanel();
        DockPanel.SetDock(bar, Dock.Top);
        DockPanel.SetDock(_timingStatus, Dock.Bottom);
        root.Children.Add(bar);
        root.Children.Add(_timingStatus);
        root.Children.Add(_timing);
        return root;
    }

    private void RefreshTiming()
    {
        var rows = _browser.SnapshotTiming();
        _timing.ItemsSource = rows.Select(TimingRowView.From).ToList();

        // Zero rows means the engine was built without its `timing` feature, which compiles
        // the subsystem out.
        _timingStatus.Text = rows.Count == 0
            ? "No timing data. The engine may have been built without its `timing` feature, which compiles the subsystem out."
            : $"{rows.Count} namespace{(rows.Count == 1 ? "" : "s")}, slowest first";
    }

    private void RefreshAll()
    {
        RefreshNetwork();
        RefreshConsole();
        RefreshTiming();
    }

    // ── helpers ────────────────────────────────────────────────────────────────

    private static void Column(GridView grid, string header, double width, string path) =>
        grid.Columns.Add(new GridViewColumn { Header = header, Width = width, DisplayMemberBinding = new Binding(path) });

    private static void HeaderGrid(ListView list)
    {
        var grid = new GridView();
        Column(grid, "Name", 240, nameof(HeaderPair.Name));
        Column(grid, "Value", 640, nameof(HeaderPair.Value));
        list.View = grid;
    }

    /// <summary>
    /// Microseconds, or "-" for a field the engine never reported. A request on a pooled
    /// connection resolves nothing rather than resolving instantly.
    /// </summary>
    internal static string FormatUs(ulong us) => us == BeaconNative.Absent
        ? "-"
        : us >= 1_000_000 ? $"{us / 1_000_000.0:0.00} s"
        : us >= 1_000 ? $"{us / 1_000.0:0.0} ms"
        : $"{us} µs";

    internal static string FormatBytes(ulong bytes) => bytes == BeaconNative.Absent
        ? "-"
        : bytes >= 1024 * 1024 ? $"{bytes / (1024.0 * 1024):0.0} MB"
        : bytes >= 1024 ? $"{bytes / 1024.0:0.0} kB"
        : $"{bytes} B";
}

/// <summary>A request as the grid shows it; formatting is kept out of the interop layer.</summary>
internal sealed record NetRowView(
    int Index,
    string StatusText,
    string Method,
    string Kind,
    string Url,
    string Size,
    string Time,
    string Phase,
    string Initiator,
    BeaconNative.Request Raw)
{
    public static NetRowView From(NetRow r) => new(
        r.Index,
        // Show the classified failure rather than a bare error.
        r.Raw.State == BeaconNative.RequestState.Failed
            ? r.FailureLabel ?? "failed"
            : r.Raw.Status == 0 ? r.StateLabel ?? "..." : r.Raw.Status.ToString(),
        r.Method ?? "-",
        r.Kind ?? "-",
        r.Url,
        DeveloperPanel.FormatBytes(r.Raw.ReceivedBytes),
        DeveloperPanel.FormatUs(r.Raw.ElapsedUs),
        r.PhaseLabel ?? "-",
        r.Initiator ?? "-",
        r.Raw);
}

internal sealed record LogRowView(string Time, string Level, string Target, string Message)
{
    public static LogRowView From(LogRecord r) =>
        new(r.When.ToString("HH:mm:ss.fff"), r.Level.ToString(), r.Target, r.Message);
}

internal sealed record TimingRowView(
    string Namespace,
    ulong Count,
    string Total,
    string Avg,
    string Min,
    string Max,
    string P50,
    string P95,
    string P99,
    string Describes)
{
    public static TimingRowView From(TimingRow r) => new(
        r.Namespace,
        r.Stats.Count,
        DeveloperPanel.FormatUs(r.Stats.TotalUs),
        DeveloperPanel.FormatUs(r.Stats.AvgUs),
        DeveloperPanel.FormatUs(r.Stats.MinUs),
        DeveloperPanel.FormatUs(r.Stats.MaxUs),
        DeveloperPanel.FormatUs(r.Stats.P50Us),
        DeveloperPanel.FormatUs(r.Stats.P95Us),
        DeveloperPanel.FormatUs(r.Stats.P99Us),
        r.Describes ?? r.Namespace);
}
