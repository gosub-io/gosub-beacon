using System.Runtime.InteropServices;

namespace Gosub.Beacon.Windows.Interop;

/// <summary>
/// Raw P/Invoke against beacon.dll, transcribed from crates/beacon-ffi/include/beacon.h.
///
/// Three marshalling details that fail at runtime rather than at compile time:
///
///   1. C's `bool` is one byte; .NET's default marshalling is the 4-byte Win32 BOOL. Every
///      bool here is UnmanagedType.U1.
///
///   2. Functions returning `char*` are declared as IntPtr. The caller frees them with
///      beacon_string_free; marshalling them as string would free them with CoTaskMemFree.
///      <see cref="BeaconStrings.Take"/> reads and frees in one step.
///
///   3. `size_t` is pointer-width: nuint.
///
/// Callers must also follow the header's rules: keep no state, poll for events, and call
/// everything from the UI thread.
/// </summary>
internal static partial class BeaconNative
{
    private const string Lib = "beacon";

    // ── types ──────────────────────────────────────────────────────────────────

    public enum EventKind
    {
        Redraw = 0,
        TabsChanged = 1,
        ActiveTabChanged = 2,
        TitleChanged = 3,
        UrlChanged = 4,
        LoadingChanged = 5,
        Progress = 6,
        FaviconChanged = 7,
        NavStateChanged = 8,
        HoverUrl = 9,
        CursorChanged = 10,
        DownloadOffered = 11,
        TabCrashed = 12,
        Log = 13,
        DownloadChanged = 14,
        NavigationFailed = 15,
        HitTest = 16,
    }

    public enum Button
    {
        Left = 0,
        Middle = 1,
        Right = 2,
    }

    [Flags]
    public enum Modifiers : uint
    {
        None = 0,
        Shift = 1,
        Control = 2,
        Alt = 4,
        Meta = 8,
    }

    /// <summary>Mirrors BeaconEvent. `Text` is borrowed and dies at the next poll.</summary>
    [StructLayout(LayoutKind.Sequential)]
    public struct Event
    {
        public EventKind Kind;
        public ulong Tab;
        public IntPtr Text;
        public double Number;
    }

    /// <summary>
    /// Mirrors BeaconFrame. Pixels are BGRA with premultiplied alpha - which is exactly
    /// WPF's PixelFormats.Pbgra32, so the page blits with no conversion at all. Borrowed
    /// until beacon_release_frame.
    /// </summary>
    [StructLayout(LayoutKind.Sequential)]
    public struct Frame
    {
        public IntPtr Pixels;
        public uint Width;   // device pixels, dpr already applied
        public uint Height;
        public uint Stride;  // bytes per row, not necessarily Width * 4
        public uint Dpr;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct Config
    {
        public IntPtr UserDataDir;                       // UTF-8, or NULL for the default
        [MarshalAs(UnmanagedType.U1)] public bool PrivateMode;
    }

    // ── lifecycle ──────────────────────────────────────────────────────────────

    [DllImport(Lib, EntryPoint = "beacon_new")]
    public static extern IntPtr New(ref Config config);

    [DllImport(Lib, EntryPoint = "beacon_new")]
    public static extern IntPtr NewDefault(IntPtr nullConfig);

    [DllImport(Lib, EntryPoint = "beacon_free")]
    public static extern void Free(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_string_free")]
    public static extern void StringFree(IntPtr s);

    [DllImport(Lib, EntryPoint = "beacon_is_private")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool IsPrivate(IntPtr browser);

    // ── tabs ───────────────────────────────────────────────────────────────────

    [DllImport(Lib, EntryPoint = "beacon_open_tab")]
    public static extern ulong OpenTab(IntPtr browser, [MarshalAs(UnmanagedType.LPUTF8Str)] string url);

    [DllImport(Lib, EntryPoint = "beacon_open_tab_after")]
    public static extern ulong OpenTabAfter(IntPtr browser, [MarshalAs(UnmanagedType.LPUTF8Str)] string url, ulong after);

    [DllImport(Lib, EntryPoint = "beacon_close_tab")]
    public static extern void CloseTab(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_activate_tab")]
    public static extern void ActivateTab(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_reopen_closed_tab")]
    public static extern ulong ReopenClosedTab(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_tab_count")]
    public static extern nuint TabCount(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_tab_at")]
    public static extern ulong TabAt(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_active_tab")]
    public static extern ulong ActiveTab(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_tab_title")]
    public static extern IntPtr TabTitle(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_tab_url")]
    public static extern IntPtr TabUrl(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_tab_is_loading")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool TabIsLoading(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_tab_can_go_back")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool TabCanGoBack(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_tab_can_go_forward")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool TabCanGoForward(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_tab_progress")]
    public static extern double TabProgress(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_move_tab")]
    public static extern void MoveTab(IntPtr browser, ulong tab, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_tab_is_pinned")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool TabIsPinned(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_set_tab_pinned")]
    public static extern void SetTabPinned(IntPtr browser, ulong tab, [MarshalAs(UnmanagedType.U1)] bool pinned);

    // ── navigation ─────────────────────────────────────────────────────────────

    [DllImport(Lib, EntryPoint = "beacon_navigate")]
    public static extern void Navigate(IntPtr browser, ulong tab, [MarshalAs(UnmanagedType.LPUTF8Str)] string url);

    [DllImport(Lib, EntryPoint = "beacon_back")]
    public static extern void Back(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_forward")]
    public static extern void Forward(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_reload")]
    public static extern void Reload(IntPtr browser, [MarshalAs(UnmanagedType.U1)] bool ignoreCache);

    [DllImport(Lib, EntryPoint = "beacon_stop")]
    public static extern void Stop(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_homepage")]
    public static extern IntPtr Homepage(IntPtr browser);

    // ── view and input ─────────────────────────────────────────────────────────

    [DllImport(Lib, EntryPoint = "beacon_set_viewport")]
    public static extern void SetViewport(IntPtr browser, ulong tab, uint width, uint height, float scale);

    [DllImport(Lib, EntryPoint = "beacon_mouse_move")]
    public static extern void MouseMove(IntPtr browser, ulong tab, float x, float y);

    [DllImport(Lib, EntryPoint = "beacon_mouse_down")]
    public static extern void MouseDown(IntPtr browser, ulong tab, float x, float y, Button button);

    [DllImport(Lib, EntryPoint = "beacon_mouse_up")]
    public static extern void MouseUp(IntPtr browser, ulong tab, float x, float y, Button button);

    [DllImport(Lib, EntryPoint = "beacon_scroll")]
    public static extern void Scroll(IntPtr browser, ulong tab, float deltaX, float deltaY);

    [DllImport(Lib, EntryPoint = "beacon_set_zoom")]
    public static extern void SetZoom(IntPtr browser, ulong tab, float zoom);

    [DllImport(Lib, EntryPoint = "beacon_zoom")]
    public static extern float Zoom(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_key_down")]
    public static extern void KeyDown(IntPtr browser, ulong tab,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string key,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string code, Modifiers modifiers);

    [DllImport(Lib, EntryPoint = "beacon_key_up")]
    public static extern void KeyUp(IntPtr browser, ulong tab,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string key,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string code, Modifiers modifiers);

    [DllImport(Lib, EntryPoint = "beacon_text_input")]
    public static extern void TextInput(IntPtr browser, ulong tab, [MarshalAs(UnmanagedType.LPUTF8Str)] string text);

    // ── events and frames ──────────────────────────────────────────────────────

    [DllImport(Lib, EntryPoint = "beacon_poll_events")]
    public static extern nuint PollEvents(IntPtr browser, [Out] Event[] outEvents, nuint max);

    [DllImport(Lib, EntryPoint = "beacon_acquire_frame")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool AcquireFrame(IntPtr browser, ulong tab, out Frame frame);

    [DllImport(Lib, EntryPoint = "beacon_release_frame")]
    public static extern void ReleaseFrame(IntPtr browser, ulong tab);

    // The GPU path: hand Beacon an HWND and it draws into it directly. Declared for
    // completeness; this shell uses AcquireFrame, which needs no graphics adapter.
    [DllImport(Lib, EntryPoint = "beacon_attach_view")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool AttachView(IntPtr browser, ulong tab, IntPtr hwnd, uint width, uint height);

    [DllImport(Lib, EntryPoint = "beacon_detach_view")]
    public static extern void DetachView(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_resize_view")]
    public static extern void ResizeView(IntPtr browser, ulong tab, uint width, uint height);

    [DllImport(Lib, EntryPoint = "beacon_draw_view")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool DrawView(IntPtr browser, ulong tab);

    // ── bookmarks ──────────────────────────────────────────────────────────────

    [DllImport(Lib, EntryPoint = "beacon_tab_is_bookmarked")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool TabIsBookmarked(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_toggle_bookmark")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool ToggleBookmark(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_bookmark_count")]
    public static extern nuint BookmarkCount(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_bookmark_url")]
    public static extern IntPtr BookmarkUrl(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_bookmark_title")]
    public static extern IntPtr BookmarkTitle(IntPtr browser, nuint index);

    // ── history ────────────────────────────────────────────────────────────────
    //
    // Snapshot-then-read-by-index, the pattern beacon.h uses for every list: one call takes
    // a snapshot and returns a count, then accessors read rows out of it.

    [DllImport(Lib, EntryPoint = "beacon_history_search")]
    public static extern nuint HistorySearch(IntPtr browser, [MarshalAs(UnmanagedType.LPUTF8Str)] string query, nuint limit);

    [DllImport(Lib, EntryPoint = "beacon_history_url")]
    public static extern IntPtr HistoryUrl(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_history_title")]
    public static extern IntPtr HistoryTitle(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_history_visit_count")]
    public static extern ulong HistoryVisitCount(IntPtr browser, nuint index);

    // ── what is under the pointer ──────────────────────────────────────────────
    //
    // Asynchronous: HitTest returns a token and the answer arrives later as a HitTest event
    // carrying the same token. The accessors below are only valid after that event.

    [DllImport(Lib, EntryPoint = "beacon_hit_test")]
    public static extern ulong HitTest(IntPtr browser, ulong tab, float x, float y);

    [DllImport(Lib, EntryPoint = "beacon_hit_link")]
    public static extern IntPtr HitLink(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_hit_image")]
    public static extern IntPtr HitImage(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_hit_text")]
    public static extern IntPtr HitText(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_hit_selection")]
    public static extern IntPtr HitSelection(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_hit_is_editable")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool HitIsEditable(IntPtr browser);

    // ── favicons ───────────────────────────────────────────────────────────────

    /// <summary>
    /// The icon as the site served it, usually PNG or ICO. Borrowed until the next call to
    /// this function, so copy the bytes out before asking about another tab.
    /// </summary>
    [DllImport(Lib, EntryPoint = "beacon_tab_favicon")]
    public static extern IntPtr TabFavicon(IntPtr browser, ulong tab, out nuint outLen);

    // ── view-source ────────────────────────────────────────────────────────────

    [DllImport(Lib, EntryPoint = "beacon_view_source")]
    public static extern ulong ViewSource(IntPtr browser, ulong tab, [MarshalAs(UnmanagedType.U1)] bool raw);

    // ── a tab whose worker died ────────────────────────────────────────────────

    [DllImport(Lib, EntryPoint = "beacon_tab_crash_reason")]
    public static extern IntPtr TabCrashReason(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_revive_tab")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool ReviveTab(IntPtr browser, ulong tab);

    // ── where forward leads ────────────────────────────────────────────────────
    //
    // Usually one entry. More than one means the history forked: you went back and then
    // somewhere else.

    [DllImport(Lib, EntryPoint = "beacon_forward_snapshot")]
    public static extern nuint ForwardSnapshot(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_forward_url")]
    public static extern IntPtr ForwardUrl(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_forward_go")]
    public static extern void ForwardGo(IntPtr browser, nuint index);

    // ── the previous session ───────────────────────────────────────────────────

    [DllImport(Lib, EntryPoint = "beacon_session_snapshot")]
    public static extern nuint SessionSnapshot(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_session_url")]
    public static extern IntPtr SessionUrl(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_session_pinned")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool SessionPinned(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_session_active")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool SessionActive(IntPtr browser, nuint index);

    // ── developer panel: console log ───────────────────────────────────────────

    /// <summary>Levels as the Rust `log` crate orders them: 1 is the loudest.</summary>
    public enum LogLevel : uint
    {
        Error = 1,
        Warn = 2,
        Info = 3,
        Debug = 4,
        Trace = 5,
    }

    [DllImport(Lib, EntryPoint = "beacon_log_snapshot")]
    public static extern nuint LogSnapshot(IntPtr browser, nuint max);

    [DllImport(Lib, EntryPoint = "beacon_log_message")]
    public static extern IntPtr LogMessage(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_log_target")]
    public static extern IntPtr LogTarget(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_log_level")]
    public static extern uint LogLevelAt(IntPtr browser, nuint index);

    /// <summary>Milliseconds since the Unix epoch; formatting is the shell's business.</summary>
    [DllImport(Lib, EntryPoint = "beacon_log_timestamp")]
    public static extern ulong LogTimestamp(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_log_clear")]
    public static extern void LogClear(IntPtr browser);

    // ── developer panel: timing ────────────────────────────────────────────────

    /// <summary>One row of the engine's timing table, in microseconds.</summary>
    [StructLayout(LayoutKind.Sequential)]
    public struct Timing
    {
        public ulong Count;
        public ulong TotalUs;
        public ulong MinUs;
        public ulong MaxUs;
        public ulong AvgUs;
        public ulong P50Us;
        public ulong P75Us;
        public ulong P95Us;
        public ulong P99Us;
    }

    /// <summary>Slowest namespace first. Zero when the engine was built without its `timing`
    /// feature, which compiles the subsystem out.</summary>
    [DllImport(Lib, EntryPoint = "beacon_timing_snapshot")]
    public static extern nuint TimingSnapshot(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_timing_namespace")]
    public static extern IntPtr TimingNamespace(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_timing_at")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool TimingAt(IntPtr browser, nuint index, out Timing outTiming);

    [DllImport(Lib, EntryPoint = "beacon_timing_describes")]
    public static extern IntPtr TimingDescribes(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_timing_reset")]
    public static extern void TimingReset(IntPtr browser);

    // ── developer panel: network ───────────────────────────────────────────────

    /// <summary>
    /// Marks a field the engine never reported, as distinct from one that is really zero: a
    /// request on a pooled connection resolves nothing rather than resolving instantly.
    /// Render it as "-", not 0.
    /// </summary>
    public const ulong Absent = ulong.MaxValue;

    public enum RequestState : uint
    {
        Queued = 0,
        Running = 1,
        Finished = 2,
        Failed = 3,
        Cancelled = 4,
    }

    /// <summary>How far an unfinished request got.</summary>
    public enum RequestPhase : uint
    {
        Queued = 0,
        Opening = 1,
        Connecting = 2,
        Waiting = 3,
        Receiving = 4,
        Done = 5,
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct Request
    {
        public ulong StartedMs;      // first seen, ms since epoch; rows are in this order
        public ulong ReceivedBytes;
        public ulong ContentLength;  // what the server said it would send; Absent if not
        public ulong ElapsedUs;      // Absent while still in flight
        public ulong DnsUs;          // Absent on a reused connection
        public ulong ConnectUs;      // encloses DnsUs rather than following it
        public ulong HeadersMs;      // when the response headers landed, same clock as started
        public uint Status;          // 0 until a response line arrives
        public RequestState State;
        public RequestPhase Phase;
        public nuint RequestHeaderCount;
        public nuint ResponseHeaderCount;
        public nuint RedirectCount;
        [MarshalAs(UnmanagedType.U1)] public bool HasBody;
        [MarshalAs(UnmanagedType.U1)] public bool BodyTruncated;
        [MarshalAs(UnmanagedType.U1)] public bool BodyEvicted;
    }

    /// <summary>Copy the requests for `tab`, or every tab when it is 0.</summary>
    [DllImport(Lib, EntryPoint = "beacon_net_snapshot")]
    public static extern nuint NetSnapshot(IntPtr browser, ulong tab);

    [DllImport(Lib, EntryPoint = "beacon_net_at")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool NetAt(IntPtr browser, nuint index, out Request outRequest);

    [DllImport(Lib, EntryPoint = "beacon_net_clear")]
    public static extern void NetClear(IntPtr browser);

    /// <summary>Off by default. Follow the panel's visibility so bodies are not held in
    /// memory while it is closed.</summary>
    [DllImport(Lib, EntryPoint = "beacon_net_set_capture_bodies")]
    public static extern void NetSetCaptureBodies(IntPtr browser, [MarshalAs(UnmanagedType.U1)] bool enabled);

    [DllImport(Lib, EntryPoint = "beacon_net_set_show_sensitive_headers")]
    public static extern void NetSetShowSensitiveHeaders(IntPtr browser, [MarshalAs(UnmanagedType.U1)] bool enabled);

    [DllImport(Lib, EntryPoint = "beacon_net_captured_body_bytes")]
    public static extern nuint NetCapturedBodyBytes(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_net_url")]
    public static extern IntPtr NetUrl(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_kind")]
    public static extern IntPtr NetKind(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_initiator")]
    public static extern IntPtr NetInitiator(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_method")]
    public static extern IntPtr NetMethod(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_content_type")]
    public static extern IntPtr NetContentType(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_error")]
    public static extern IntPtr NetError(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_state_label")]
    public static extern IntPtr NetStateLabel(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_phase_label")]
    public static extern IntPtr NetPhaseLabel(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_phase_hint")]
    public static extern IntPtr NetPhaseHint(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_failure_label")]
    public static extern IntPtr NetFailureLabel(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_failure_hint")]
    public static extern IntPtr NetFailureHint(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_body_text")]
    public static extern IntPtr NetBodyText(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_net_request_header_name")]
    public static extern IntPtr NetRequestHeaderName(IntPtr browser, nuint index, nuint header);

    [DllImport(Lib, EntryPoint = "beacon_net_request_header_value")]
    public static extern IntPtr NetRequestHeaderValue(IntPtr browser, nuint index, nuint header);

    [DllImport(Lib, EntryPoint = "beacon_net_response_header_name")]
    public static extern IntPtr NetResponseHeaderName(IntPtr browser, nuint index, nuint header);

    [DllImport(Lib, EntryPoint = "beacon_net_response_header_value")]
    public static extern IntPtr NetResponseHeaderValue(IntPtr browser, nuint index, nuint header);

    [DllImport(Lib, EntryPoint = "beacon_net_redirect_status")]
    public static extern uint NetRedirectStatus(IntPtr browser, nuint index, nuint hop);

    [DllImport(Lib, EntryPoint = "beacon_net_redirect_url")]
    public static extern IntPtr NetRedirectUrl(IntPtr browser, nuint index, nuint hop);

    // ── settings ───────────────────────────────────────────────────────────────
    //
    // The ABI hands over rows -- key, description, type, value, default, constraint -- and
    // the shell decides what a boolean or a bounded number looks like on its platform.

    public enum SettingType
    {
        Bool = 0,
        Int = 1,
        UInt = 2,
        Float = 3,
        String = 4,
        Map = 5, // a comma-separated list, edited as text
    }

    /// <summary>NULL or "" for all; a `*` makes it a wildcard, anything else is a
    /// case-insensitive substring. Sorted.</summary>
    [DllImport(Lib, EntryPoint = "beacon_settings_snapshot")]
    public static extern nuint SettingsSnapshot(IntPtr browser, [MarshalAs(UnmanagedType.LPUTF8Str)] string? filter);

    [DllImport(Lib, EntryPoint = "beacon_setting_key")]
    public static extern IntPtr SettingKey(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_setting_description")]
    public static extern IntPtr SettingDescription(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_setting_value")]
    public static extern IntPtr SettingValue(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_setting_default")]
    public static extern IntPtr SettingDefault(IntPtr browser, nuint index);

    /// <summary>The accepted values in one line ("left | right", "-1 | 0-9999"), or NULL.</summary>
    [DllImport(Lib, EntryPoint = "beacon_setting_constraint")]
    public static extern IntPtr SettingConstraint(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_setting_type")]
    public static extern SettingType SettingTypeAt(IntPtr browser, nuint index);

    /// <summary>Whether it differs from the default.</summary>
    [DllImport(Lib, EntryPoint = "beacon_setting_is_modified")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool SettingIsModified(IntPtr browser, nuint index);

    /// <summary>Non-zero when the setting is restricted to literal choices.</summary>
    [DllImport(Lib, EntryPoint = "beacon_setting_choice_count")]
    public static extern nuint SettingChoiceCount(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_setting_choice")]
    public static extern IntPtr SettingChoice(IntPtr browser, nuint index, nuint choice);

    [DllImport(Lib, EntryPoint = "beacon_setting_range")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool SettingRange(IntPtr browser, nuint index, out long lo, out long hi);

    /// <summary>
    /// Write a value, typed by the key's own schema. False when the key is unknown or the
    /// value is outside its constraint; revert the editor when that happens. Writing the
    /// default removes the override.
    /// </summary>
    [DllImport(Lib, EntryPoint = "beacon_setting_set")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool SettingSet(IntPtr browser,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string key,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string value);

    [DllImport(Lib, EntryPoint = "beacon_setting_reset")]
    [return: MarshalAs(UnmanagedType.U1)]
    public static extern bool SettingReset(IntPtr browser, [MarshalAs(UnmanagedType.LPUTF8Str)] string key);

    // ── downloads ──────────────────────────────────────────────────────────────

    public enum DownloadState
    {
        Running = 0,
        Finished = 1,
        Failed = 2,
    }

    /// <summary>The URL behind an offer, for a save dialog that wants to show it.</summary>
    [DllImport(Lib, EntryPoint = "beacon_download_offer_url")]
    public static extern IntPtr DownloadOfferUrl(IntPtr browser, ulong offer);

    /// <summary>
    /// Answer an offer with a chosen path. Returns a download id, or 0. The offer id keeps
    /// the answer attached to the right offer if a second download arrives while a save
    /// dialog is open.
    /// </summary>
    [DllImport(Lib, EntryPoint = "beacon_download_accept")]
    public static extern ulong DownloadAccept(IntPtr browser, ulong offer,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string path);

    [DllImport(Lib, EntryPoint = "beacon_download_reject")]
    public static extern void DownloadReject(IntPtr browser, ulong offer);

    [DllImport(Lib, EntryPoint = "beacon_download_count")]
    public static extern nuint DownloadCount(IntPtr browser);

    [DllImport(Lib, EntryPoint = "beacon_download_at")]
    public static extern ulong DownloadAt(IntPtr browser, nuint index);

    [DllImport(Lib, EntryPoint = "beacon_download_filename")]
    public static extern IntPtr DownloadFilename(IntPtr browser, ulong id);

    [DllImport(Lib, EntryPoint = "beacon_download_path")]
    public static extern IntPtr DownloadPath(IntPtr browser, ulong id);

    /// <summary>0..1, or -1 when the server sent no length.</summary>
    [DllImport(Lib, EntryPoint = "beacon_download_progress")]
    public static extern double DownloadProgress(IntPtr browser, ulong id);

    [DllImport(Lib, EntryPoint = "beacon_download_received")]
    public static extern ulong DownloadReceived(IntPtr browser, ulong id);

    [DllImport(Lib, EntryPoint = "beacon_download_state")]
    public static extern DownloadState DownloadStateAt(IntPtr browser, ulong id);

    /// <summary>Hand a finished download to the desktop's default application.</summary>
    [DllImport(Lib, EntryPoint = "beacon_download_open")]
    public static extern void DownloadOpen(IntPtr browser, ulong id);
}

/// <summary>Reading owned strings back across the boundary without leaking them.</summary>
internal static class BeaconStrings
{
    /// <summary>
    /// Read a `char*` the library handed us and free it with the library's own allocator.
    /// Null becomes null; the caller never sees the pointer.
    /// </summary>
    public static string? Take(IntPtr owned)
    {
        if (owned == IntPtr.Zero)
        {
            return null;
        }

        try
        {
            return Marshal.PtrToStringUTF8(owned);
        }
        finally
        {
            BeaconNative.StringFree(owned);
        }
    }

    /// <summary>Read a borrowed `const char*` - an event's text - without freeing it.</summary>
    public static string? Borrow(IntPtr borrowed) =>
        borrowed == IntPtr.Zero ? null : Marshal.PtrToStringUTF8(borrowed);
}
