using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

/// <summary>
/// The page: frames blitted into a WriteableBitmap, plus mouse and scroll input.
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

    /// <summary>Last viewport handed to the engine, to avoid re-sending it every layout pass.</summary>
    private (uint Width, uint Height, float Scale) _sentViewport;

    /// <summary>The tab being shown. Set by the window when the active tab changes.</summary>
    public ulong Tab { get; set; }

    public PageView()
    {
        Focusable = true;

        // The engine rasterizes at device resolution; smoothing here would blur it.
        RenderOptions.SetBitmapScalingMode(this, BitmapScalingMode.NearestNeighbor);
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

        if ((width, height, scale) == _sentViewport)
        {
            return;
        }

        _sentViewport = (width, height, scale);
        _browser.SetViewport(Tab, width, height, scale);
        Console.Error.WriteLine($"[beacon] viewport tab={Tab} {width}x{height} @{scale}");
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
            // a frame rendered before the last viewport change landed. Follow the frame.
            if (_bitmap is null || _bitmap.PixelWidth != frame.Width || _bitmap.PixelHeight != frame.Height)
            {
                // A bitmap declared at 96*dpr DPI lays out at the right logical size.
                var dpi = 96.0 * Math.Max(1u, frame.Dpr);
                _bitmap = new WriteableBitmap((int)frame.Width, (int)frame.Height, dpi, dpi,
                    PixelFormats.Pbgra32, palette: null);
                Console.Error.WriteLine($"[beacon] first frame {frame.Width}x{frame.Height} dpr={frame.Dpr} stride={frame.Stride}");
            }

            var rect = new Int32Rect(0, 0, (int)frame.Width, (int)frame.Height);
            _bitmap.WritePixels(rect, frame.Pixels, (int)(frame.Stride * frame.Height), (int)frame.Stride);
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
            // Width/Height are in DIPs and account for the bitmap's DPI, so device pixels
            // land 1:1.
            dc.DrawImage(_bitmap, new Rect(0, 0, _bitmap.Width, _bitmap.Height));
        }
    }

    protected override void OnRenderSizeChanged(SizeChangedInfo info)
    {
        base.OnRenderSizeChanged(info);
        SyncViewport();
    }

    // ── input ──────────────────────────────────────────────────────────────────
    //
    // Positions go to the engine in CSS pixels, and WPF's input coordinates already are
    // CSS pixels, so no DPI conversion here, unlike the viewport above.

    protected override void OnMouseMove(MouseEventArgs e)
    {
        base.OnMouseMove(e);
        if (_browser is not null && Tab != 0)
        {
            var p = e.GetPosition(this);
            _browser.MouseMove(Tab, (float)p.X, (float)p.Y);
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
        var p = e.GetPosition(this);
        _browser.MouseDown(Tab, (float)p.X, (float)p.Y, Map(e.ChangedButton));
    }

    protected override void OnMouseUp(MouseButtonEventArgs e)
    {
        base.OnMouseUp(e);
        if (_browser is not null && Tab != 0)
        {
            var p = e.GetPosition(this);
            _browser.MouseUp(Tab, (float)p.X, (float)p.Y, Map(e.ChangedButton));
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
        _pendingHitToken = _browser.HitTest(Tab, _pendingHitAt.X, _pendingHitAt.Y);
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
        // about 20px a line matches the other shells.
        _browser.Scroll(Tab, 0, -(e.Delta / 120f) * 60f);
        e.Handled = true;
    }

    private static BeaconNative.Button Map(MouseButton button) => button switch
    {
        MouseButton.Right => BeaconNative.Button.Right,
        MouseButton.Middle => BeaconNative.Button.Middle,
        _ => BeaconNative.Button.Left,
    };
}
