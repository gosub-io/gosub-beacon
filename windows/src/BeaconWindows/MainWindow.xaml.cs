using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

public partial class MainWindow : Window
{
    private readonly BeaconBrowser _browser;
    private readonly PageView _page = new();

    /// <summary>Set while writing the address bar ourselves, so the engine's URL does not
    /// overwrite what the user is typing.</summary>
    private bool _updatingAddress;

    private HistoryWindow? _historyWindow;
    private BookmarksWindow? _bookmarksWindow;
    private DeveloperPanel? _devPanel;
    private SettingsWindow? _settingsWindow;
    private DownloadsWindow? _downloadsWindow;

    internal MainWindow(string[] startupUrls)
    {
        InitializeComponent();

        _browser = BeaconBrowser.Create();
        _page.Attach(_browser);
        _page.ContextRequested += ShowPageContextMenu;
        PageHost.Child = _page;

        Tabs.Attach(_browser);
        Tabs.TabActivated += tab => { _browser.ActivateTab(tab); SyncActiveTab(); Tabs.Rebuild(); };
        Tabs.TabClosed += tab => { _browser.CloseTab(tab); Tabs.Rebuild(); };
        Tabs.NewTabRequested += () => OpenTab(Homepage());
        Tabs.ViewSourceRequested += tab => Activate(_browser.ViewSource(tab));
        Tabs.DuplicateRequested += tab =>
        {
            if (_browser.TabUrl(tab) is { Length: > 0 } url)
            {
                OpenTab(url);
            }
        };

        Bookmarks.Attach(_browser);
        Bookmarks.Navigate += url => Navigate(url);
        Bookmarks.NavigateNewTab += OpenTab;

        BackButton.Click += (_, _) => _browser.Back();
        ForwardButton.Click += (_, _) => _browser.Forward();
        ForwardButton.MouseRightButtonUp += (_, _) => ShowForwardBranches();
        ReloadButton.Click += (_, _) => _browser.Reload();
        BookmarkButton.Click += (_, _) => ToggleBookmark();
        AddressBar.KeyDown += OnAddressBarKeyDown;

        BuildMenus();

        // Events are pulled, not pushed. CompositionTarget.Rendering is WPF's per-frame tick
        // and runs on the UI thread, which the ABI requires.
        CompositionTarget.Rendering += OnFrame;

        Closed += (_, _) =>
        {
            CompositionTarget.Rendering -= OnFrame;
            _historyWindow?.Close();
            _bookmarksWindow?.Close();
            _devPanel?.Close();
            _settingsWindow?.Close();
            _downloadsWindow?.Close();
            _browser.Dispose();
        };

        if (startupUrls.Length > 0)
        {
            foreach (var url in startupUrls)
            {
                OpenTab(url);
            }
        }
        else
        {
            RestoreSessionOrHome();
        }

        Bookmarks.Rebuild();

        // Opens the panel at launch, like Chrome's --auto-open-devtools-for-tabs. Also the
        // only way to exercise its marshalling without a desktop: over ssh a GUI lands in
        // session 0, where a window can be created but not seen.
        //
        // Deferred to Loaded: opening a second window from this constructor runs before the
        // main window has had a layout pass, so the page view reports a zero size and never
        // sends its viewport.
        if (Environment.GetEnvironmentVariable("BEACON_DEVTOOLS") is { Length: > 0 } devtools)
        {
            Loaded += (_, _) =>
            {
                ShowDeveloperPanel();

                // "all" also raises the settings and downloads windows, to exercise their
                // marshalling.
                if (devtools.Equals("all", StringComparison.OrdinalIgnoreCase))
                {
                    ShowSettings();
                    ShowDownloads();

                    // Show(), not ShowDialog(): a modal dialog would block this session.
                    new AboutWindow { Owner = this }.Show();
                }
            };
        }
    }

    private string Homepage() => _browser.Homepage() ?? "gosub://home";

    /// <summary>
    /// Reopen what the last session had, as the GTK shell does when started with no URL.
    /// Falls back to the homepage when there is nothing to restore.
    /// </summary>
    private void RestoreSessionOrHome()
    {
        var session = _browser.SessionEntries();
        if (session.Count == 0)
        {
            OpenTab(Homepage());
            return;
        }

        ulong active = 0;
        foreach (var entry in session)
        {
            var tab = _browser.OpenTab(entry.Url);
            if (tab == 0)
            {
                continue;
            }

            if (entry.Pinned)
            {
                _browser.SetTabPinned(tab, true);
            }

            if (entry.Active)
            {
                active = tab;
            }
        }

        if (active != 0)
        {
            _browser.ActivateTab(active);
        }

        SyncActiveTab();
        Tabs.Rebuild();
        StatusLine.Text = $"Restored {session.Count} tab{(session.Count == 1 ? "" : "s")} from the last session";
    }

    private void OpenTab(string url)
    {
        var tab = _browser.OpenTab(url);
        if (tab == 0)
        {
            StatusLine.Text = $"Could not parse URL: {url}";
            return;
        }

        Activate(tab);
    }

    private void Activate(ulong tab)
    {
        if (tab == 0)
        {
            return;
        }

        _browser.ActivateTab(tab);
        SyncActiveTab();
        Tabs.Rebuild();
    }

    private void Navigate(string url)
    {
        if (_page.Tab == 0)
        {
            OpenTab(url);
        }
        else
        {
            _browser.Navigate(_page.Tab, url);
        }
    }

    // ── the pump ───────────────────────────────────────────────────────────────

    private void OnFrame(object? sender, EventArgs e)
    {
        var chromeDirty = false;

        foreach (var ev in _browser.PollEvents())
        {
            switch (ev.Kind)
            {
                case BeaconNative.EventKind.Redraw:
                    // Unconditional: Redraw does not name a tab. The FFI emits it with tab 0,
                    // meaning a new frame exists, not that a particular tab changed. Gating it
                    // on ev.Tab == _page.Tab never fires and the page stays blank. The Swift
                    // shell exempts it from its tab filter for the same reason.
                    _page.Refresh();
                    break;

                case BeaconNative.EventKind.TabsChanged:
                    chromeDirty = true;
                    break;

                case BeaconNative.EventKind.ActiveTabChanged:
                    SyncActiveTab();
                    chromeDirty = true;
                    break;

                case BeaconNative.EventKind.TitleChanged:
                    chromeDirty = true;
                    if (ev.Tab == _page.Tab)
                    {
                        Title = string.IsNullOrEmpty(ev.Text) ? "Gosub Beacon" : $"{ev.Text} - Gosub Beacon";
                    }

                    break;

                case BeaconNative.EventKind.UrlChanged:
                    if (ev.Tab == _page.Tab && !AddressBar.IsKeyboardFocused)
                    {
                        SetAddress(ev.Text ?? string.Empty);
                        UpdateBookmarkButton();
                    }

                    break;

                case BeaconNative.EventKind.FaviconChanged:
                    Tabs.InvalidateFavicon(ev.Tab);
                    chromeDirty = true;
                    break;

                case BeaconNative.EventKind.LoadingChanged:
                    StatusLine.Text = ev.Number != 0 ? "Loading..." : "Done";
                    UpdateNavButtons();
                    break;

                case BeaconNative.EventKind.Progress:
                    StatusLine.Text = ev.Number < 0 ? "Loading..." : $"Loading... {ev.Number * 100:0}%";
                    break;

                case BeaconNative.EventKind.NavStateChanged:
                    UpdateNavButtons();
                    break;

                case BeaconNative.EventKind.HoverUrl:
                    StatusLine.Text = ev.Text ?? string.Empty;
                    break;

                case BeaconNative.EventKind.NavigationFailed:
                    StatusLine.Text = ev.Text ?? string.Empty;
                    break;

                case BeaconNative.EventKind.TabCrashed:
                    StatusLine.Text = ev.Text is { Length: > 0 } reason
                        ? $"Tab crashed: {reason} - right-click the tab to reload it"
                        : "Tab crashed";
                    chromeDirty = true;
                    break;

                case BeaconNative.EventKind.HitTest:
                    // The token comes back as `number`. The accessors are only valid until
                    // another hit test replaces the answer.
                    _page.OnHitTestAnswered((ulong)ev.Number, _browser.ReadHit());
                    break;

                case BeaconNative.EventKind.CursorChanged:
                    _page.Cursor = ev.Number switch
                    {
                        1 => Cursors.Hand,
                        2 => Cursors.IBeam,
                        _ => Cursors.Arrow,
                    };
                    break;

                case BeaconNative.EventKind.DownloadOffered:
                {
                    // A modal save dialog runs its own message loop, which ticks
                    // CompositionTarget.Rendering and re-enters this pump. Hand it to the
                    // dispatcher so the batch finishes first.
                    var offer = (ulong)ev.Number;
                    var suggested = ev.Text ?? "download";
                    Dispatcher.BeginInvoke(new Action(() => AnswerDownloadOffer(offer, suggested)));
                    break;
                }

                case BeaconNative.EventKind.DownloadChanged:
                    _downloadsWindow?.Reload();
                    break;

                case BeaconNative.EventKind.Log:
                    // The developer panel reads the log on its own timer.
                    break;
            }
        }

        // Several events in one pump can each ask for a rebuild; do it once, after the batch.
        if (chromeDirty)
        {
            Tabs.Rebuild();
        }
    }

    // ── chrome ─────────────────────────────────────────────────────────────────

    private void SyncActiveTab()
    {
        _page.Tab = _browser.ActiveTab;
        if (_devPanel is not null)
        {
            _devPanel.Tab = _page.Tab;
        }

        _page.SyncViewport();
        _page.Refresh();
        SetAddress(_browser.TabUrl(_page.Tab) ?? string.Empty);
        UpdateNavButtons();
        UpdateBookmarkButton();
        UpdateZoomLabel();
    }

    private void UpdateNavButtons()
    {
        var tab = _page.Tab;
        BackButton.IsEnabled = tab != 0 && _browser.CanGoBack(tab);
        ForwardButton.IsEnabled = tab != 0 && _browser.CanGoForward(tab);
    }

    private void UpdateBookmarkButton()
    {
        var on = _page.Tab != 0 && _browser.IsBookmarked(_page.Tab);
        BookmarkButton.Content = on ? "★" : "☆";
        BookmarkButton.ToolTip = on ? "Remove bookmark (Ctrl+D)" : "Bookmark this page (Ctrl+D)";
    }

    private void UpdateZoomLabel()
    {
        if (_page.Tab == 0)
        {
            ZoomLabel.Text = string.Empty;
            return;
        }

        var zoom = _browser.Zoom(_page.Tab);
        ZoomLabel.Text = Math.Abs(zoom - 1f) < 0.01f ? string.Empty : $"{zoom * 100:0}%";
    }

    private void SetAddress(string url)
    {
        _updatingAddress = true;
        AddressBar.Text = url;
        _updatingAddress = false;
    }

    private void ToggleBookmark()
    {
        if (_page.Tab == 0)
        {
            return;
        }

        var on = _browser.ToggleBookmark(_page.Tab);
        StatusLine.Text = on ? "Bookmarked" : "Bookmark removed";
        UpdateBookmarkButton();
        Bookmarks.Rebuild();
        _bookmarksWindow?.Reload();
    }

    private void SetZoom(float zoom)
    {
        if (_page.Tab == 0)
        {
            return;
        }

        // The engine clamps to 0.25..5.0; clamp here too so the label matches.
        _browser.SetZoom(_page.Tab, Math.Clamp(zoom, 0.25f, 5f));
        UpdateZoomLabel();
    }

    private void OnAddressBarKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key != Key.Return || _updatingAddress)
        {
            return;
        }

        var url = AddressBar.Text.Trim();
        if (url.Length == 0)
        {
            return;
        }

        // The engine accepts what a user would type ("example.com", "gosub://home", a
        // filesystem path), so no URL fixing is needed here.
        Navigate(url);
        _page.Focus();
        e.Handled = true;
    }

    // ── context menu on the page ───────────────────────────────────────────────

    private void ShowPageContextMenu(Point at, HitResult hit)
    {
        var menu = new ContextMenu();

        if (hit.Link is { Length: > 0 } link)
        {
            menu.Items.Add(MenuItemFor("Open Link in New Tab", () => OpenTab(link)));
            menu.Items.Add(MenuItemFor("Copy Link Address", () => ClipboardSafe.SetText(link)));
            menu.Items.Add(new Separator());
        }

        if (hit.Image is { Length: > 0 } image)
        {
            menu.Items.Add(MenuItemFor("Open Image in New Tab", () => OpenTab(image)));
            menu.Items.Add(MenuItemFor("Copy Image Address", () => ClipboardSafe.SetText(image)));
            menu.Items.Add(new Separator());
        }

        if (hit.Selection is { Length: > 0 } selection)
        {
            menu.Items.Add(MenuItemFor("Copy", () => ClipboardSafe.SetText(selection)));
            menu.Items.Add(new Separator());
        }

        menu.Items.Add(MenuItemFor("Back", () => _browser.Back(), _page.Tab != 0 && _browser.CanGoBack(_page.Tab)));
        menu.Items.Add(MenuItemFor("Forward", () => _browser.Forward(), _page.Tab != 0 && _browser.CanGoForward(_page.Tab)));
        menu.Items.Add(MenuItemFor("Reload", () => _browser.Reload()));
        menu.Items.Add(new Separator());
        menu.Items.Add(MenuItemFor("View Source", () => Activate(_browser.ViewSource(_page.Tab))));

        menu.PlacementTarget = _page;
        menu.Placement = System.Windows.Controls.Primitives.PlacementMode.Relative;
        menu.HorizontalOffset = at.X;
        menu.VerticalOffset = at.Y;
        menu.IsOpen = true;
    }

    /// <summary>
    /// Where Forward leads when the history has forked. The engine picks a branch by
    /// default; this offers the others.
    /// </summary>
    private void ShowForwardBranches()
    {
        if (_page.Tab == 0)
        {
            return;
        }

        var entries = _browser.ForwardEntries(_page.Tab);
        if (entries.Count == 0)
        {
            return;
        }

        var menu = new ContextMenu();
        for (var i = 0; i < entries.Count; i++)
        {
            var index = i;
            menu.Items.Add(MenuItemFor(entries[i], () => _browser.ForwardGo(index)));
        }

        menu.PlacementTarget = ForwardButton;
        menu.IsOpen = true;
    }

    private static MenuItem MenuItemFor(string header, Action action, bool enabled = true)
    {
        var item = new MenuItem { Header = header, IsEnabled = enabled };
        item.Click += (_, _) => action();
        return item;
    }

    // ── menu bar ───────────────────────────────────────────────────────────────

    private void BuildMenus()
    {
        var file = new MenuItem { Header = "_File" };
        file.Items.Add(MenuItemFor("New _Tab\tCtrl+T", () => OpenTab(Homepage())));
        file.Items.Add(MenuItemFor("_Close Tab\tCtrl+W", () =>
        {
            if (_page.Tab != 0)
            {
                _browser.CloseTab(_page.Tab);
                Tabs.Rebuild();
            }
        }));
        file.Items.Add(MenuItemFor("Reopen Closed Tab\tCtrl+Shift+T", () => Activate(_browser.ReopenClosedTab())));
        file.Items.Add(new Separator());
        file.Items.Add(MenuItemFor("View _Source\tCtrl+U", () => Activate(_browser.ViewSource(_page.Tab))));
        file.Items.Add(new Separator());
        file.Items.Add(MenuItemFor("E_xit", Close));

        var edit = new MenuItem { Header = "_Edit" };
        edit.Items.Add(MenuItemFor("Copy _Address", () => ClipboardSafe.SetText(_browser.TabUrl(_page.Tab))));
        edit.Items.Add(MenuItemFor("Focus Address Bar\tCtrl+L", () =>
        {
            AddressBar.Focus();
            AddressBar.SelectAll();
        }));

        var view = new MenuItem { Header = "_View" };
        view.Items.Add(MenuItemFor("_Reload\tF5", () => _browser.Reload()));
        view.Items.Add(MenuItemFor("Reload Ignoring Cache\tCtrl+F5", () => _browser.Reload(_page.Tab, true)));
        view.Items.Add(MenuItemFor("_Stop\tEsc", () => _browser.Stop()));
        view.Items.Add(new Separator());
        view.Items.Add(MenuItemFor("Zoom _In\tCtrl++", () => SetZoom(_browser.Zoom(_page.Tab) + 0.1f)));
        view.Items.Add(MenuItemFor("Zoom _Out\tCtrl+-", () => SetZoom(_browser.Zoom(_page.Tab) - 0.1f)));
        view.Items.Add(MenuItemFor("_Actual Size\tCtrl+0", () => SetZoom(1f)));
        view.Items.Add(new Separator());
        var showBar = new MenuItem { Header = "_Bookmarks Bar", IsCheckable = true, IsChecked = true };
        showBar.Click += (_, _) => BookmarksBarHost.Visibility = showBar.IsChecked ? Visibility.Visible : Visibility.Collapsed;
        view.Items.Add(showBar);

        // History and Bookmarks are filled when they open: the engine owns both lists, and a
        // menu built once at startup would go stale.
        var history = new MenuItem { Header = "Hi_story" };
        history.SubmenuOpened += (_, _) => FillHistoryMenu(history);

        var bookmarks = new MenuItem { Header = "_Bookmarks" };
        bookmarks.SubmenuOpened += (_, _) => FillBookmarksMenu(bookmarks);

        var tools = new MenuItem { Header = "_Tools" };
        tools.Items.Add(MenuItemFor("_Developer Tools\tF12", ShowDeveloperPanel));
        tools.Items.Add(new Separator());
        tools.Items.Add(MenuItemFor("Clear Network Log", () => _browser.ClearNetwork()));
        tools.Items.Add(MenuItemFor("Clear Console", () => _browser.ClearLog()));
        tools.Items.Add(MenuItemFor("Reset Timing", () => _browser.ResetTiming()));
        tools.Items.Add(new Separator());
        tools.Items.Add(MenuItemFor("_Downloads\tCtrl+J", ShowDownloads));
        tools.Items.Add(MenuItemFor("_Settings", ShowSettings));

        var help = new MenuItem { Header = "_Help" };
        help.Items.Add(MenuItemFor("_About Gosub Beacon", ShowAbout));

        MainMenu.Items.Add(file);
        MainMenu.Items.Add(edit);
        MainMenu.Items.Add(view);
        MainMenu.Items.Add(history);
        MainMenu.Items.Add(bookmarks);
        MainMenu.Items.Add(tools);
        MainMenu.Items.Add(help);

        FillHistoryMenu(history);
        FillBookmarksMenu(bookmarks);
    }

    private void FillHistoryMenu(MenuItem menu)
    {
        menu.Items.Clear();
        menu.Items.Add(MenuItemFor("_Back\tAlt+Left", () => _browser.Back(),
            _page.Tab != 0 && _browser.CanGoBack(_page.Tab)));
        menu.Items.Add(MenuItemFor("_Forward\tAlt+Right", () => _browser.Forward(),
            _page.Tab != 0 && _browser.CanGoForward(_page.Tab)));
        menu.Items.Add(new Separator());
        menu.Items.Add(MenuItemFor("Show All _History\tCtrl+H", ShowHistoryWindow));
    }

    private void FillBookmarksMenu(MenuItem menu)
    {
        menu.Items.Clear();
        var on = _page.Tab != 0 && _browser.IsBookmarked(_page.Tab);
        menu.Items.Add(MenuItemFor(on ? "_Remove Bookmark\tCtrl+D" : "_Bookmark This Page\tCtrl+D", ToggleBookmark));
        menu.Items.Add(MenuItemFor("Show All Bookmarks\tCtrl+Shift+O", ShowBookmarksWindow));

        var saved = _browser.Bookmarks();
        if (saved.Count > 0)
        {
            menu.Items.Add(new Separator());
            foreach (var bookmark in saved.Take(30))
            {
                var label = string.IsNullOrWhiteSpace(bookmark.Title) ? bookmark.Url : bookmark.Title;
                menu.Items.Add(MenuItemFor(label, () => Navigate(bookmark.Url)));
            }
        }
    }

    private void ShowHistoryWindow()
    {
        if (_historyWindow is null)
        {
            _historyWindow = new HistoryWindow(_browser) { Owner = this };
            _historyWindow.Navigate += url => OpenTab(url);
            _historyWindow.Closed += (_, _) => _historyWindow = null;
        }

        _historyWindow.Show();
        _historyWindow.Activate();
    }

    private void ShowBookmarksWindow()
    {
        if (_bookmarksWindow is null)
        {
            _bookmarksWindow = new BookmarksWindow(_browser) { Owner = this };
            _bookmarksWindow.Navigate += url => OpenTab(url);
            _bookmarksWindow.Closed += (_, _) => _bookmarksWindow = null;
        }

        _bookmarksWindow.Show();
        _bookmarksWindow.Activate();
    }

    private void ShowDeveloperPanel()
    {
        if (_devPanel is null)
        {
            _devPanel = new DeveloperPanel(_browser) { Owner = this, Tab = _page.Tab };
            _devPanel.Closed += (_, _) => _devPanel = null;
        }

        _devPanel.Tab = _page.Tab;
        _devPanel.Show();
        _devPanel.Activate();
    }

    /// <summary>
    /// Answer a download offer with a Windows save dialog. The offer id keeps the answer
    /// attached to the right offer if a second download arrives while the dialog is open.
    /// </summary>
    private void AnswerDownloadOffer(ulong offer, string suggestedName)
    {
        var dialog = new Microsoft.Win32.SaveFileDialog
        {
            FileName = suggestedName,
            Title = "Save file",
            OverwritePrompt = true,
        };

        if (_browser.DownloadOfferUrl(offer) is { Length: > 0 } url)
        {
            dialog.Title = $"Save {url}";
        }

        if (dialog.ShowDialog(this) == true)
        {
            var id = _browser.AcceptDownload(offer, dialog.FileName);
            if (id == 0)
            {
                StatusLine.Text = "The download could not be started.";
                return;
            }

            StatusLine.Text = $"Downloading {System.IO.Path.GetFileName(dialog.FileName)}...";
            ShowDownloads();
        }
        else
        {
            _browser.RejectDownload(offer);
            StatusLine.Text = "Download cancelled";
        }
    }

    private void ShowDownloads()
    {
        if (_downloadsWindow is null)
        {
            _downloadsWindow = new DownloadsWindow(_browser) { Owner = this };
            _downloadsWindow.Closed += (_, _) => _downloadsWindow = null;
        }

        _downloadsWindow.Show();
        _downloadsWindow.Activate();
    }

    private void ShowSettings()
    {
        if (_settingsWindow is null)
        {
            _settingsWindow = new SettingsWindow(_browser) { Owner = this };
            _settingsWindow.Closed += (_, _) => _settingsWindow = null;
        }

        _settingsWindow.Show();
        _settingsWindow.Activate();
    }

    private void ShowAbout() => new AboutWindow { Owner = this }.ShowDialog();

    protected override void OnPreviewKeyDown(KeyEventArgs e)
    {
        base.OnPreviewKeyDown(e);
        var ctrl = (Keyboard.Modifiers & ModifierKeys.Control) != 0;
        var shift = (Keyboard.Modifiers & ModifierKeys.Shift) != 0;
        var alt = (Keyboard.Modifiers & ModifierKeys.Alt) != 0;

        switch (e.Key)
        {
            case Key.T when ctrl && shift:
                Activate(_browser.ReopenClosedTab());
                break;
            case Key.T when ctrl:
                OpenTab(Homepage());
                break;
            case Key.W when ctrl:
                if (_page.Tab != 0)
                {
                    _browser.CloseTab(_page.Tab);
                    Tabs.Rebuild();
                }

                break;
            case Key.L when ctrl:
                AddressBar.Focus();
                AddressBar.SelectAll();
                break;
            case Key.D when ctrl:
                ToggleBookmark();
                break;
            case Key.H when ctrl:
                ShowHistoryWindow();
                break;
            case Key.F12:
                ShowDeveloperPanel();
                break;
            case Key.J when ctrl:
                ShowDownloads();
                break;
            case Key.O when ctrl && shift:
                ShowBookmarksWindow();
                break;
            case Key.U when ctrl:
                Activate(_browser.ViewSource(_page.Tab));
                break;
            case Key.F5 when ctrl:
                _browser.Reload(_page.Tab, true);
                break;
            case Key.F5:
                _browser.Reload();
                break;
            case Key.Escape:
                _browser.Stop();
                break;
            case Key.OemPlus when ctrl:
            case Key.Add when ctrl:
                SetZoom(_browser.Zoom(_page.Tab) + 0.1f);
                break;
            case Key.OemMinus when ctrl:
            case Key.Subtract when ctrl:
                SetZoom(_browser.Zoom(_page.Tab) - 0.1f);
                break;
            case Key.D0 when ctrl:
                SetZoom(1f);
                break;
            case Key.Left when alt:
                _browser.Back();
                break;
            case Key.Right when alt:
                _browser.Forward();
                break;
            default:
                return;
        }

        e.Handled = true;
    }
}
