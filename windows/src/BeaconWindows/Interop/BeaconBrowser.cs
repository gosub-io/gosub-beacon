using System.Runtime.InteropServices;

namespace Gosub.Beacon.Windows.Interop;

/// <summary>One row of the engine's settings store.</summary>
internal readonly record struct SettingRow(
    string Key,
    string? Description,
    string Value,
    string Default,
    string? Constraint,
    BeaconNative.SettingType Type,
    bool IsModified,
    IReadOnlyList<string> Choices,
    (long Lo, long Hi)? Range);

/// <summary>One download, running or finished.</summary>
internal readonly record struct DownloadRow(
    ulong Id,
    string Filename,
    string? Path,
    double Progress,
    ulong Received,
    BeaconNative.DownloadState State)
{
    /// <summary>True when the server sent no length, so there is no fraction to show.</summary>
    public bool Indeterminate => Progress < 0;
}

/// <summary>One console record.</summary>
internal readonly record struct LogRecord(
    DateTimeOffset When,
    BeaconNative.LogLevel Level,
    string Target,
    string Message);

/// <summary>One row of the engine's timing table.</summary>
internal readonly record struct TimingRow(string Namespace, string? Describes, BeaconNative.Timing Stats);

/// <summary>One HTTP header.</summary>
internal readonly record struct HeaderPair(string Name, string Value);

/// <summary>One hop of a redirect chain.</summary>
internal readonly record struct Redirect(uint Status, string Url);

/// <summary>A network request, as the list shows it.</summary>
internal readonly record struct NetRow(
    int Index,
    BeaconNative.Request Raw,
    string Url,
    string? Method,
    string? Kind,
    string? Initiator,
    string? ContentType,
    string? StateLabel,
    string? PhaseLabel,
    string? FailureLabel);

/// <summary>Everything behind one selected request.</summary>
internal readonly record struct NetDetail(
    IReadOnlyList<HeaderPair> RequestHeaders,
    IReadOnlyList<HeaderPair> ResponseHeaders,
    IReadOnlyList<Redirect> Redirects,
    string? BodyText,
    string? Error,
    string? PhaseHint,
    string? FailureHint);

/// <summary>A visited page.</summary>
internal readonly record struct HistoryEntry(string Url, string Title, ulong VisitCount);

/// <summary>A saved bookmark.</summary>
internal readonly record struct Bookmark(string Url, string Title);

/// <summary>A tab the previous session had open.</summary>
internal readonly record struct SessionEntry(string Url, bool Pinned, bool Active);

/// <summary>What the engine found under the pointer. Every field may be null.</summary>
internal readonly record struct HitResult(
    string? Link,
    string? Image,
    string? Text,
    string? Selection,
    bool IsEditable)
{
    /// <summary>True when there is nothing to put on a context menu.</summary>
    public bool IsEmpty => Link is null && Image is null && Text is null && Selection is null;
}

/// <summary>One event, with its borrowed text already copied into managed memory.</summary>
internal readonly record struct BeaconEvent(
    BeaconNative.EventKind Kind,
    ulong Tab,
    string? Text,
    double Number);

/// <summary>
/// A running browser: owns the native handle and wraps the C ABI so callers do not handle
/// pointers or lifetimes.
///
/// Caches nothing. The header requires that the shell keep no state of its own, so every
/// call here forwards straight through.
/// </summary>
internal sealed class BeaconBrowser : IDisposable
{
    private IntPtr _handle;

    /// <summary>Scratch space for the event pump, reused so the loop allocates nothing.</summary>
    private readonly BeaconNative.Event[] _eventBuffer = new BeaconNative.Event[64];

    private BeaconBrowser(IntPtr handle) => _handle = handle;

    /// <summary>
    /// Start the engine. <paramref name="userDataDir"/> may be null for the platform
    /// default. Throws on failure, since the message is the only diagnostic available.
    /// </summary>
    public static BeaconBrowser Create(string? userDataDir = null, bool privateMode = false)
    {
        var dir = userDataDir is null ? IntPtr.Zero : Marshal.StringToCoTaskMemUTF8(userDataDir);
        try
        {
            var config = new BeaconNative.Config { UserDataDir = dir, PrivateMode = privateMode };
            var handle = BeaconNative.New(ref config);
            if (handle == IntPtr.Zero)
            {
                throw new InvalidOperationException(
                    "beacon_new returned NULL: the engine could not start.");
            }

            return new BeaconBrowser(handle);
        }
        finally
        {
            // The config is copied by beacon_new; our UTF-8 buffer is ours to release.
            if (dir != IntPtr.Zero)
            {
                Marshal.FreeCoTaskMem(dir);
            }
        }
    }

    private IntPtr Handle => _handle != IntPtr.Zero
        ? _handle
        : throw new ObjectDisposedException(nameof(BeaconBrowser));

    public bool IsPrivate => BeaconNative.IsPrivate(Handle);

    // ── tabs ───────────────────────────────────────────────────────────────────

    public ulong OpenTab(string url) => BeaconNative.OpenTab(Handle, url);
    public void CloseTab(ulong tab) => BeaconNative.CloseTab(Handle, tab);
    public void ActivateTab(ulong tab) => BeaconNative.ActivateTab(Handle, tab);
    public ulong ReopenClosedTab() => BeaconNative.ReopenClosedTab(Handle);
    public ulong ActiveTab => BeaconNative.ActiveTab(Handle);
    public int TabCount => (int)BeaconNative.TabCount(Handle);
    public ulong TabAt(int index) => BeaconNative.TabAt(Handle, (nuint)index);

    public string? TabTitle(ulong tab) => BeaconStrings.Take(BeaconNative.TabTitle(Handle, tab));
    public string? TabUrl(ulong tab) => BeaconStrings.Take(BeaconNative.TabUrl(Handle, tab));
    public bool TabIsLoading(ulong tab) => BeaconNative.TabIsLoading(Handle, tab);
    public bool CanGoBack(ulong tab) => BeaconNative.TabCanGoBack(Handle, tab);
    public bool CanGoForward(ulong tab) => BeaconNative.TabCanGoForward(Handle, tab);
    public double TabProgress(ulong tab) => BeaconNative.TabProgress(Handle, tab);

    /// <summary>The tab ids in strip order, read fresh each time.</summary>
    public IReadOnlyList<ulong> Tabs()
    {
        var count = TabCount;
        var tabs = new ulong[count];
        for (var i = 0; i < count; i++)
        {
            tabs[i] = TabAt(i);
        }

        return tabs;
    }

    // ── navigation ─────────────────────────────────────────────────────────────

    public void Navigate(ulong tab, string url) => BeaconNative.Navigate(Handle, tab, url);
    public void Back() => BeaconNative.Back(Handle);
    public void Forward() => BeaconNative.Forward(Handle);
    public void Reload(bool ignoreCache = false) => BeaconNative.Reload(Handle, ignoreCache);
    public void Stop() => BeaconNative.Stop(Handle);
    public string? Homepage() => BeaconStrings.Take(BeaconNative.Homepage(Handle));

    // ── view and input ─────────────────────────────────────────────────────────

    public void SetViewport(ulong tab, uint width, uint height, float scale) =>
        BeaconNative.SetViewport(Handle, tab, width, height, scale);

    public void MouseMove(ulong tab, float x, float y) => BeaconNative.MouseMove(Handle, tab, x, y);

    public void MouseDown(ulong tab, float x, float y, BeaconNative.Button button) =>
        BeaconNative.MouseDown(Handle, tab, x, y, button);

    public void MouseUp(ulong tab, float x, float y, BeaconNative.Button button) =>
        BeaconNative.MouseUp(Handle, tab, x, y, button);

    public void Scroll(ulong tab, float dx, float dy) => BeaconNative.Scroll(Handle, tab, dx, dy);

    public void KeyDown(ulong tab, string key, string code, BeaconNative.Modifiers mods) =>
        BeaconNative.KeyDown(Handle, tab, key, code, mods);

    public void KeyUp(ulong tab, string key, string code, BeaconNative.Modifiers mods) =>
        BeaconNative.KeyUp(Handle, tab, key, code, mods);

    public void TextInput(ulong tab, string text) => BeaconNative.TextInput(Handle, tab, text);

    // ── events ─────────────────────────────────────────────────────────────────

    /// <summary>
    /// Drain the event queue. Text is copied out here while it is still valid: an event's
    /// string is invalidated by the next poll, which may be the next pass of this loop.
    /// </summary>
    public List<BeaconEvent> PollEvents()
    {
        var events = new List<BeaconEvent>();

        // Keep pulling while the queue returns a full buffer, or a burst larger than the
        // buffer is spread over several frames.
        nuint drained;
        do
        {
            drained = BeaconNative.PollEvents(Handle, _eventBuffer, (nuint)_eventBuffer.Length);
            for (nuint i = 0; i < drained; i++)
            {
                var raw = _eventBuffer[i];
                events.Add(new BeaconEvent(raw.Kind, raw.Tab, BeaconStrings.Borrow(raw.Text), raw.Number));
            }
        }
        while (drained == (nuint)_eventBuffer.Length);

        return events;
    }

    /// <summary>
    /// Run <paramref name="use"/> against the tab's latest frame, then release it.
    /// Returns false when nothing has rendered yet, which is normal right after a tab opens.
    /// The pixels are only valid inside the callback.
    /// </summary>
    public bool WithFrame(ulong tab, Action<BeaconNative.Frame> use)
    {
        if (!BeaconNative.AcquireFrame(Handle, tab, out var frame))
        {
            return false;
        }

        try
        {
            use(frame);
            return true;
        }
        finally
        {
            BeaconNative.ReleaseFrame(Handle, tab);
        }
    }

    // ── history ────────────────────────────────────────────────────────────────

    /// <summary>
    /// Search visited pages, newest and most-visited first. An empty query matches nothing
    /// rather than everything, so it returns early without crossing the boundary.
    /// </summary>
    public IReadOnlyList<HistoryEntry> SearchHistory(string query, int limit = 50)
    {
        if (string.IsNullOrWhiteSpace(query))
        {
            return Array.Empty<HistoryEntry>();
        }

        var count = (int)BeaconNative.HistorySearch(Handle, query, (nuint)limit);
        var rows = new List<HistoryEntry>(count);
        for (var i = 0; i < count; i++)
        {
            var idx = (nuint)i;
            rows.Add(new HistoryEntry(
                BeaconStrings.Take(BeaconNative.HistoryUrl(Handle, idx)) ?? string.Empty,
                BeaconStrings.Take(BeaconNative.HistoryTitle(Handle, idx)) ?? string.Empty,
                BeaconNative.HistoryVisitCount(Handle, idx)));
        }

        return rows;
    }

    // ── what is under the pointer ──────────────────────────────────────────────

    /// <summary>Ask what is at (x, y) in CSS pixels. 0 means the request failed.</summary>
    public ulong HitTest(ulong tab, double x, double y) =>
        BeaconNative.HitTest(Handle, tab, (float)x, (float)y);

    /// <summary>Read the answer to the last hit test. Only meaningful after a HitTest event.</summary>
    public HitResult ReadHit() => new(
        BeaconStrings.Take(BeaconNative.HitLink(Handle)),
        BeaconStrings.Take(BeaconNative.HitImage(Handle)),
        BeaconStrings.Take(BeaconNative.HitText(Handle)),
        BeaconStrings.Take(BeaconNative.HitSelection(Handle)),
        BeaconNative.HitIsEditable(Handle));

    // ── favicons ───────────────────────────────────────────────────────────────

    /// <summary>
    /// The tab's icon bytes, or null. Copied immediately: the buffer is borrowed only until
    /// the next call.
    /// </summary>
    public byte[]? TabFavicon(ulong tab)
    {
        var ptr = BeaconNative.TabFavicon(Handle, tab, out var len);
        if (ptr == IntPtr.Zero || len == 0)
        {
            return null;
        }

        var bytes = new byte[(int)len];
        Marshal.Copy(ptr, bytes, 0, bytes.Length);
        return bytes;
    }

    // ── view-source, crashes, forward ──────────────────────────────────────────

    public ulong ViewSource(ulong tab, bool raw = false) => BeaconNative.ViewSource(Handle, tab, raw);

    /// <summary>Why the tab crashed, or null if it did not. A crashed tab keeps its place.</summary>
    public string? TabCrashReason(ulong tab) => BeaconStrings.Take(BeaconNative.TabCrashReason(Handle, tab));

    public bool ReviveTab(ulong tab) => BeaconNative.ReviveTab(Handle, tab);

    /// <summary>
    /// Where Forward leads. More than one entry means the history forked.
    /// </summary>
    public IReadOnlyList<string> ForwardEntries(ulong tab)
    {
        var count = (int)BeaconNative.ForwardSnapshot(Handle, tab);
        var urls = new List<string>(count);
        for (var i = 0; i < count; i++)
        {
            urls.Add(BeaconStrings.Take(BeaconNative.ForwardUrl(Handle, (nuint)i)) ?? string.Empty);
        }

        return urls;
    }

    public void ForwardGo(int index) => BeaconNative.ForwardGo(Handle, (nuint)index);

    // ── the previous session ───────────────────────────────────────────────────

    public IReadOnlyList<SessionEntry> SessionEntries()
    {
        var count = (int)BeaconNative.SessionSnapshot(Handle);
        var rows = new List<SessionEntry>(count);
        for (var i = 0; i < count; i++)
        {
            var idx = (nuint)i;
            rows.Add(new SessionEntry(
                BeaconStrings.Take(BeaconNative.SessionUrl(Handle, idx)) ?? string.Empty,
                BeaconNative.SessionPinned(Handle, idx),
                BeaconNative.SessionActive(Handle, idx)));
        }

        return rows;
    }

    // ── bookmarks ──────────────────────────────────────────────────────────────

    public bool IsBookmarked(ulong tab) => BeaconNative.TabIsBookmarked(Handle, tab);

    /// <summary>Toggle, returning whether the tab is bookmarked afterwards.</summary>
    public bool ToggleBookmark(ulong tab) => BeaconNative.ToggleBookmark(Handle, tab);

    public IReadOnlyList<Bookmark> Bookmarks()
    {
        var count = (int)BeaconNative.BookmarkCount(Handle);
        var rows = new List<Bookmark>(count);
        for (var i = 0; i < count; i++)
        {
            var idx = (nuint)i;
            rows.Add(new Bookmark(
                BeaconStrings.Take(BeaconNative.BookmarkUrl(Handle, idx)) ?? string.Empty,
                BeaconStrings.Take(BeaconNative.BookmarkTitle(Handle, idx)) ?? string.Empty));
        }

        return rows;
    }

    // ── tab arrangement ────────────────────────────────────────────────────────

    public bool TabIsPinned(ulong tab) => BeaconNative.TabIsPinned(Handle, tab);

    /// <summary>Pinned tabs sit left and resist closing; the browser enforces that.</summary>
    public void SetTabPinned(ulong tab, bool pinned) => BeaconNative.SetTabPinned(Handle, tab, pinned);

    public void MoveTab(ulong tab, int index) => BeaconNative.MoveTab(Handle, tab, (nuint)index);

    public float Zoom(ulong tab) => BeaconNative.Zoom(Handle, tab);

    public void SetZoom(ulong tab, float zoom) => BeaconNative.SetZoom(Handle, tab, zoom);

    public void Reload(ulong tab, bool ignoreCache) => BeaconNative.Reload(Handle, ignoreCache);

    // ── developer panel: console ───────────────────────────────────────────────

    /// <summary>
    /// The newest log records, in the order the engine gave them. An empty list usually
    /// means the level filter: capture follows BEACON_LOG / RUST_LOG, which default to
    /// warnings only.
    /// </summary>
    public IReadOnlyList<LogRecord> SnapshotLog(int max = 500)
    {
        var count = (int)BeaconNative.LogSnapshot(Handle, (nuint)max);
        var rows = new List<LogRecord>(count);
        for (var i = 0; i < count; i++)
        {
            var idx = (nuint)i;
            rows.Add(new LogRecord(
                DateTimeOffset.FromUnixTimeMilliseconds((long)BeaconNative.LogTimestamp(Handle, idx)).ToLocalTime(),
                (BeaconNative.LogLevel)BeaconNative.LogLevelAt(Handle, idx),
                BeaconStrings.Take(BeaconNative.LogTarget(Handle, idx)) ?? string.Empty,
                BeaconStrings.Take(BeaconNative.LogMessage(Handle, idx)) ?? string.Empty));
        }

        return rows;
    }

    /// <summary>Process-wide: one logger, so every window's console clears together.</summary>
    public void ClearLog() => BeaconNative.LogClear(Handle);

    // ── developer panel: timing ────────────────────────────────────────────────

    /// <summary>
    /// The engine's timing table, slowest namespace first. Empty when the engine was built
    /// without its `timing` feature, which compiles the subsystem out.
    /// </summary>
    public IReadOnlyList<TimingRow> SnapshotTiming()
    {
        var count = (int)BeaconNative.TimingSnapshot(Handle);
        var rows = new List<TimingRow>(count);
        for (var i = 0; i < count; i++)
        {
            var idx = (nuint)i;
            if (!BeaconNative.TimingAt(Handle, idx, out var stats))
            {
                continue;
            }

            rows.Add(new TimingRow(
                BeaconStrings.Take(BeaconNative.TimingNamespace(Handle, idx)) ?? string.Empty,
                BeaconStrings.Take(BeaconNative.TimingDescribes(Handle, idx)),
                stats));
        }

        return rows;
    }

    /// <summary>Start measuring again, to time a single navigation.</summary>
    public void ResetTiming() => BeaconNative.TimingReset(Handle);

    // ── developer panel: network ───────────────────────────────────────────────

    /// <summary>
    /// Copy the requests for <paramref name="tab"/>, or every tab when it is 0.
    ///
    /// Rows are read by index out of the snapshot this call takes and stay valid only until
    /// the next one, so <see cref="ReadNetDetail"/> must run before snapshotting again.
    /// </summary>
    public IReadOnlyList<NetRow> SnapshotNetwork(ulong tab)
    {
        var count = (int)BeaconNative.NetSnapshot(Handle, tab);
        var rows = new List<NetRow>(count);
        for (var i = 0; i < count; i++)
        {
            var idx = (nuint)i;
            if (!BeaconNative.NetAt(Handle, idx, out var req))
            {
                continue;
            }

            rows.Add(new NetRow(
                i,
                req,
                BeaconStrings.Take(BeaconNative.NetUrl(Handle, idx)) ?? string.Empty,
                BeaconStrings.Take(BeaconNative.NetMethod(Handle, idx)),
                BeaconStrings.Take(BeaconNative.NetKind(Handle, idx)),
                BeaconStrings.Take(BeaconNative.NetInitiator(Handle, idx)),
                BeaconStrings.Take(BeaconNative.NetContentType(Handle, idx)),
                BeaconStrings.Take(BeaconNative.NetStateLabel(Handle, idx)),
                BeaconStrings.Take(BeaconNative.NetPhaseLabel(Handle, idx)),
                BeaconStrings.Take(BeaconNative.NetFailureLabel(Handle, idx))));
        }

        return rows;
    }

    /// <summary>
    /// Headers, body and redirect chain for one row of the snapshot just taken. See the
    /// lifetime note on <see cref="SnapshotNetwork"/>.
    /// </summary>
    public NetDetail ReadNetDetail(int index)
    {
        var idx = (nuint)index;
        if (!BeaconNative.NetAt(Handle, idx, out var req))
        {
            return new NetDetail([], [], [], null, null, null, null);
        }

        var request = ReadHeaders(idx, req.RequestHeaderCount, BeaconNative.NetRequestHeaderName, BeaconNative.NetRequestHeaderValue);
        var response = ReadHeaders(idx, req.ResponseHeaderCount, BeaconNative.NetResponseHeaderName, BeaconNative.NetResponseHeaderValue);

        var redirects = new List<Redirect>((int)req.RedirectCount);
        for (nuint hop = 0; hop < req.RedirectCount; hop++)
        {
            redirects.Add(new Redirect(
                BeaconNative.NetRedirectStatus(Handle, idx, hop),
                BeaconStrings.Take(BeaconNative.NetRedirectUrl(Handle, idx, hop)) ?? string.Empty));
        }

        return new NetDetail(
            request,
            response,
            redirects,
            BeaconStrings.Take(BeaconNative.NetBodyText(Handle, idx)),
            BeaconStrings.Take(BeaconNative.NetError(Handle, idx)),
            BeaconStrings.Take(BeaconNative.NetPhaseHint(Handle, idx)),
            BeaconStrings.Take(BeaconNative.NetFailureHint(Handle, idx)));
    }

    private List<HeaderPair> ReadHeaders(
        nuint index,
        nuint count,
        Func<IntPtr, nuint, nuint, IntPtr> name,
        Func<IntPtr, nuint, nuint, IntPtr> value)
    {
        var headers = new List<HeaderPair>((int)count);
        for (nuint h = 0; h < count; h++)
        {
            headers.Add(new HeaderPair(
                BeaconStrings.Take(name(Handle, index, h)) ?? string.Empty,
                BeaconStrings.Take(value(Handle, index, h)) ?? string.Empty));
        }

        return headers;
    }

    public void ClearNetwork() => BeaconNative.NetClear(Handle);

    /// <summary>Follow the panel's visibility, so bodies are not held while it is closed.</summary>
    public void SetCaptureBodies(bool enabled) => BeaconNative.NetSetCaptureBodies(Handle, enabled);

    public void SetShowSensitiveHeaders(bool enabled) => BeaconNative.NetSetShowSensitiveHeaders(Handle, enabled);

    public long CapturedBodyBytes => (long)BeaconNative.NetCapturedBodyBytes(Handle);

    // ── settings ───────────────────────────────────────────────────────────────

    /// <summary>
    /// Snapshot settings matching <paramref name="filter"/> - null or "" for all, a `*` for
    /// a wildcard pattern, anything else a case-insensitive substring.
    /// </summary>
    public IReadOnlyList<SettingRow> SnapshotSettings(string? filter = null)
    {
        var count = (int)BeaconNative.SettingsSnapshot(Handle, filter);
        var rows = new List<SettingRow>(count);
        for (var i = 0; i < count; i++)
        {
            var idx = (nuint)i;

            var choiceCount = (int)BeaconNative.SettingChoiceCount(Handle, idx);
            var choices = new List<string>(choiceCount);
            for (var c = 0; c < choiceCount; c++)
            {
                choices.Add(BeaconStrings.Take(BeaconNative.SettingChoice(Handle, idx, (nuint)c)) ?? string.Empty);
            }

            (long Lo, long Hi)? range = BeaconNative.SettingRange(Handle, idx, out var lo, out var hi)
                ? (lo, hi)
                : null;

            rows.Add(new SettingRow(
                BeaconStrings.Take(BeaconNative.SettingKey(Handle, idx)) ?? string.Empty,
                BeaconStrings.Take(BeaconNative.SettingDescription(Handle, idx)),
                BeaconStrings.Take(BeaconNative.SettingValue(Handle, idx)) ?? string.Empty,
                BeaconStrings.Take(BeaconNative.SettingDefault(Handle, idx)) ?? string.Empty,
                BeaconStrings.Take(BeaconNative.SettingConstraint(Handle, idx)),
                BeaconNative.SettingTypeAt(Handle, idx),
                BeaconNative.SettingIsModified(Handle, idx),
                choices,
                range));
        }

        return rows;
    }

    /// <summary>
    /// Write a value. False means the store refused it: an unknown key, or a value outside
    /// its constraint. Revert the editor when that happens.
    /// </summary>
    public bool SetSetting(string key, string value) => BeaconNative.SettingSet(Handle, key, value);

    /// <summary>Put a key back to its default, forgetting the override.</summary>
    public bool ResetSetting(string key) => BeaconNative.SettingReset(Handle, key);

    // ── downloads ──────────────────────────────────────────────────────────────

    public string? DownloadOfferUrl(ulong offer) => BeaconStrings.Take(BeaconNative.DownloadOfferUrl(Handle, offer));

    /// <summary>Accept an offer at a path you chose. Returns a download id, or 0.</summary>
    public ulong AcceptDownload(ulong offer, string path) => BeaconNative.DownloadAccept(Handle, offer, path);

    public void RejectDownload(ulong offer) => BeaconNative.DownloadReject(Handle, offer);

    public IReadOnlyList<DownloadRow> Downloads()
    {
        var count = (int)BeaconNative.DownloadCount(Handle);
        var rows = new List<DownloadRow>(count);
        for (var i = 0; i < count; i++)
        {
            var id = BeaconNative.DownloadAt(Handle, (nuint)i);
            rows.Add(new DownloadRow(
                id,
                BeaconStrings.Take(BeaconNative.DownloadFilename(Handle, id)) ?? string.Empty,
                BeaconStrings.Take(BeaconNative.DownloadPath(Handle, id)),
                BeaconNative.DownloadProgress(Handle, id),
                BeaconNative.DownloadReceived(Handle, id),
                BeaconNative.DownloadStateAt(Handle, id)));
        }

        return rows;
    }

    /// <summary>Hand a finished download to the desktop's default application.</summary>
    public void OpenDownload(ulong id) => BeaconNative.DownloadOpen(Handle, id);

    public void Dispose()
    {
        if (_handle == IntPtr.Zero)
        {
            return;
        }

        var handle = _handle;
        _handle = IntPtr.Zero;
        BeaconNative.Free(handle);
    }
}
