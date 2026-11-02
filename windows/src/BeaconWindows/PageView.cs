using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// <summary>
/// The page: frames blitted into a WriteableBitmap, plus mouse, scroll and keyboard input.
///
/// Uses the CPU path: beacon_acquire_frame returns BGRA pixels which are copied in. The GPU
/// path (beacon_attach_view with an HWND) is faster on real hardware but needs a graphics
/// adapter; where the display is a basic driver, wgpu falls back to WARP and Vello's compute
/// shaders crash in d3d10warp.dll.
///
/// The frames are premultiplied BGRA, which is PixelFormats.Pbgra32, so nothing is converted.
///
/// A bare FrameworkElement rather than an Image: an Image sizes itself to its source, so
/// before the first frame it measures 0x0, reports a zero viewport, and the engine never
/// renders. This takes the size its container gives it.
/// </summary>
internal sealed class PageView : FrameworkElement
{
    private BeaconBrowser? _browser;
    private WriteableBitmap? _bitmap;

    /// <summary>Last viewport handed to the engine, to avoid re-sending it every layout pass.
    /// Keyed by tab too: each tab has its own viewport, and one shown in a same-sized view
    /// still needs to be told.</summary>
    private (ulong Tab, uint Width, uint Height, float Scale) _sentViewport;

    private ulong _tab;

    /// <summary>The tab being shown. Set by the window when the active tab changes.</summary>
    public ulong Tab
    {
        get => _tab;
        set
        {
            if (_tab != value)
            {
                // A hit test asked on the old tab answers about the old page; a menu built
                // from it would act on the new one.
                _tab = value;
                _pendingHitToken = 0;

                // Nor may the old page stay up: a new tab (view source) has no frame yet.
                _bitmap = null;
                InvalidateVisual();
            }
        }
    }

    /// <summary>DIPs per frame pixel: zoom over the frame's DPR.</summary>
    private double _frameScale = 1;

    public PageView()
    {
        Focusable = true;
        RenderOptions.SetEdgeMode(this, EdgeMode.Aliased);
    }

    public void Attach(BeaconBrowser browser) => _browser = browser;

    /// <summary>
    /// Tell the engine how much room the page has.
    /// </summary>
    public void SyncViewport()
    {
        if (_browser is null || Tab == 0 || ActualWidth <= 0 || ActualHeight <= 0)
        {
            return;
        }

        // width/height are CSS pixels and `scale` is device pixels per CSS pixel. WPF's
        // ActualWidth is already in DIPs, the same unit, so it goes across unscaled;
        // multiplying by the DPI scale lays the page out too wide above 100%.
        var scale = (float)VisualTreeHelper.GetDpi(this).DpiScaleX;
        var width = (uint)Math.Max(1, Math.Round(ActualWidth));
        var height = (uint)Math.Max(1, Math.Round(ActualHeight));

        if ((Tab, width, height, scale) == _sentViewport)
        {
            return;
        }

        _sentViewport = (Tab, width, height, scale);
        _browser.SetViewport(Tab, width, height, scale);
        Log.Debug($"viewport tab={Tab} {width}x{height} @{scale}");
    }

    /// <summary>Pull the latest frame and paint it. Cheap to call on every BEACON_REDRAW.</summary>
    public void Refresh()
    {
        if (_browser is null || Tab == 0)
        {
            return;
        }

        // The engine needs a viewport before it can draw; on the first redraw the layout
        // pass may only just have given us a size.
        SyncViewport();

        var got = _browser.WithFrame(Tab, frame =>
        {
            if (frame.Width == 0 || frame.Height == 0)
            {
                return;
            }

            // The engine may return a different size than requested: a resize in flight, or
            // a frame rendered before the last viewport change landed. Follow the frame, its
            // DPR included: the same pixels at another DPR lay out at another logical size.
            if (_bitmap is null || _bitmap.PixelWidth != frame.Width || _bitmap.PixelHeight != frame.Height)
            {
                _bitmap = new WriteableBitmap((int)frame.Width, (int)frame.Height, 96, 96,
                    PixelFormats.Pbgra32, palette: null);
                Log.Debug($"first frame {frame.Width}x{frame.Height} dpr={frame.Dpr} stride={frame.Stride}");
            }

            var rect = new Int32Rect(0, 0, (int)frame.Width, (int)frame.Height);
            _bitmap.WritePixels(rect, frame.Pixels, (int)(frame.Stride * frame.Height), (int)frame.Stride);

            // A zoomed page is laid out smaller and rasterized at ceil(scale * zoom), so the
            // frame is drawn zoom / dpr DIPs a pixel, not 1 / dpr. Only where its pixels land
            // 1:1 on the screen's is nearest-neighbour right; at 125% or a zoom they would
            // alias, so those get filtered.
            _frameScale = _browser.Zoom(Tab) / Math.Max(1u, frame.Dpr);
            var onScreen = _frameScale * VisualTreeHelper.GetDpi(this).DpiScaleX;
            RenderOptions.SetBitmapScalingMode(this, Math.Abs(onScreen - 1) < 0.001
                ? BitmapScalingMode.NearestNeighbor
                : BitmapScalingMode.HighQuality);
        });

        if (got)
        {
            InvalidateVisual();
        }
    }

    protected override void OnRender(DrawingContext dc)
    {
        base.OnRender(dc);

        // Paint the background: an unpainted FrameworkElement is not hit-testable.
        dc.DrawRectangle(Brushes.White, null, new Rect(0, 0, ActualWidth, ActualHeight));

        if (_bitmap is not null)
        {
            dc.DrawImage(_bitmap, new Rect(0, 0, _bitmap.PixelWidth * _frameScale, _bitmap.PixelHeight * _frameScale));
        }
    }

    protected override void OnRenderSizeChanged(SizeChangedInfo info)
    {
        base.OnRenderSizeChanged(info);
        SyncViewport();
    }

    // Moved to a monitor at another scale: the engine rasterizes at the new DPR.
    protected override void OnDpiChanged(DpiScale oldDpi, DpiScale newDpi)
    {
        base.OnDpiChanged(oldDpi, newDpi);
        Refresh();
    }

    // ── input ──────────────────────────────────────────────────────────────────
    //
    // Positions go to the engine in CSS pixels, and WPF's input coordinates already are
    // CSS pixels, so no DPI conversion here, unlike the viewport above. A zoomed page is
    // laid out smaller, so they are divided by the zoom, as on the Mac.

    private (float X, float Y) PagePoint(MouseEventArgs e)
    {
        var p = e.GetPosition(this);
        var zoom = _browser!.Zoom(Tab);
        return ((float)(p.X / zoom), (float)(p.Y / zoom));
    }

    protected override void OnMouseMove(MouseEventArgs e)
    {
        base.OnMouseMove(e);
        if (_browser is not null && Tab != 0)
        {
            var (x, y) = PagePoint(e);
            _browser.MouseMove(Tab, x, y);
        }
    }

    protected override void OnMouseDown(MouseButtonEventArgs e)
    {
        base.OnMouseDown(e);
        if (_browser is null || Tab == 0)
        {
            return;
        }

        Focus();
        if (Map(e.ChangedButton) is { } button)
        {
            var (x, y) = PagePoint(e);
            _browser.MouseDown(Tab, x, y, button);
        }
    }

    protected override void OnMouseUp(MouseButtonEventArgs e)
    {
        base.OnMouseUp(e);
        if (_browser is not null && Tab != 0 && Map(e.ChangedButton) is { } button)
        {
            var (x, y) = PagePoint(e);
            _browser.MouseUp(Tab, x, y, button);
        }
    }

    /// <summary>
    /// Raised when the engine answers a right-click. The window shows the menu.
    /// </summary>
    public event Action<Point, HitResult>? ContextRequested;

    /// <summary>Where the pending hit test was asked, so the menu opens under the pointer.</summary>
    private Point _pendingHitAt;

    private ulong _pendingHitToken;

    /// <summary>
    /// Deliver the answer to a hit test, called when a HitTest event arrives. Ignores
    /// tokens from other requests.
    /// </summary>
    public void OnHitTestAnswered(ulong token, HitResult hit)
    {
        if (token == 0 || token != _pendingHitToken)
        {
            return;
        }

        _pendingHitToken = 0;
        ContextRequested?.Invoke(_pendingHitAt, hit);
    }

    protected override void OnMouseRightButtonDown(MouseButtonEventArgs e)
    {
        base.OnMouseRightButtonDown(e);
        if (_browser is null || Tab == 0)
        {
            return;
        }

        // The engine answers from the layout tree on its own thread, so the menu cannot be
        // built here. Remember where we asked and wait for the event.
        _pendingHitAt = e.GetPosition(this);
        var (x, y) = PagePoint(e);
        _pendingHitToken = _browser.HitTest(Tab, x, y);
        e.Handled = true;
    }

    protected override void OnMouseWheel(MouseWheelEventArgs e)
    {
        base.OnMouseWheel(e);
        if (_browser is null || Tab == 0)
        {
            return;
        }

        // WPF reports 120 units per notch; the engine takes pixels. Three lines a notch at
        // about 20px a line matches the other shells. A precision touchpad reports finer
        // steps than a notch; those are applied as they come, notches are animated.
        bool precise = e.Delta % 120 != 0;
        _browser.Scroll(Tab, 0, -(e.Delta / 120f) * 60f / _browser.Zoom(Tab), precise);
        e.Handled = true;
    }

    // ── keyboard ───────────────────────────────────────────────────────────────
    //
    // The engine's contract, as on GTK and the Mac: a press that types a character is one
    // KeyDown carrying that character, a press that types nothing is a KeyDown with the key's
    // DOM name, and TextInput is for text that is not one press, such as an IME commit. WPF
    // reports a press (KeyDown) and what it typed (TextInput, from WM_CHAR) separately, so a
    // press that may type is held back until its character arrives.

    /// <summary>The last press that may type, for its TextInput to answer as.</summary>
    private Key _pressInFlight = Key.None;

    /// <summary>What each held key was sent as, so its release names the same key.</summary>
    private readonly Dictionary<Key, string> _held = new();

    protected override void OnKeyDown(KeyEventArgs e)
    {
        base.OnKeyDown(e);
        if (_browser is null || Tab == 0)
        {
            return;
        }

        // With Alt held WPF reports Key.System and puts the key in SystemKey.
        var key = e.Key == Key.System ? e.SystemKey : e.Key;
        var keys = Keyboard.Modifiers;
        var name = KeyMap.Named(key);
        // Control without Alt types nothing (Ctrl+A is U+0001), so the key is the press.
        // Ctrl+Alt is AltGr on many layouts, which does type; its TextInput sends it.
        if (name is null && (keys & (ModifierKeys.Control | ModifierKeys.Alt)) == ModifierKeys.Control)
        {
            name = KeyMap.Character(key, (keys & ModifierKeys.Shift) != 0);
        }

        if (name is null)
        {
            _pressInFlight = key;
            return;
        }

        _browser.KeyDown(Tab, name, KeyMap.Code(key), KeyMap.Modifiers(keys));
        _held[key] = name;
        // Handled keeps WPF's focus navigation off Tab and the arrows, and drops the control
        // character (\b, \t, \r) the press would type. Not with Alt: a handled system key
        // never reaches DefWindowProc, and Alt+F4 and Alt+Space would stop working.
        e.Handled = e.Key != Key.System;
    }

    protected override void OnTextInput(TextCompositionEventArgs e)
    {
        base.OnTextInput(e);
        var press = _pressInFlight;
        _pressInFlight = Key.None;
        var text = e.Text;
        // Control characters are not text; OnKeyDown reports those keys.
        if (_browser is null || Tab == 0 || text.Length == 0 || text.Any(c => c < ' ' || c == '\x7f'))
        {
            return;
        }

        if (press != Key.None && (text.Length == 1 || (text.Length == 2 && char.IsSurrogatePair(text, 0))))
        {
            // One character is the press itself, as the layout resolved it: "a", "!", "é".
            _browser.KeyDown(Tab, text, KeyMap.Code(press), KeyMap.Modifiers(Keyboard.Modifiers));
            _held[press] = text;
        }
        else
        {
            _browser.TextInput(Tab, text);
        }

        e.Handled = true;
    }

    protected override void OnKeyUp(KeyEventArgs e)
    {
        base.OnKeyUp(e);
        var key = e.Key == Key.System ? e.SystemKey : e.Key;
        // A press that typed nothing (a bare Shift) is over; a later commit is not its text.
        if (key == _pressInFlight)
        {
            _pressInFlight = Key.None;
        }

        if (_browser is not null && Tab != 0 && _held.Remove(key, out var name))
        {
            _browser.KeyUp(Tab, name, KeyMap.Code(key), KeyMap.Modifiers(Keyboard.Modifiers));
            e.Handled = true;
        }
    }

    protected override void OnLostKeyboardFocus(KeyboardFocusChangedEventArgs e)
    {
        base.OnLostKeyboardFocus(e);
        // Releases now go to whatever has focus.
        _held.Clear();
        _pressInFlight = Key.None;
    }

    /// <summary>The engine knows three buttons. The side buttons (XButton1/2) are not page
    /// clicks, so they map to nothing rather than to Left.</summary>
    private static BeaconNative.Button? Map(MouseButton button) => button switch
    {
        MouseButton.Left => BeaconNative.Button.Left,
        MouseButton.Right => BeaconNative.Button.Right,
        MouseButton.Middle => BeaconNative.Button.Middle,
        _ => null,
    };
}
