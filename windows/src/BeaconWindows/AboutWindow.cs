using System.Diagnostics;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Documents;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Media.Effects;
using System.Windows.Media.Imaging;

namespace Gosub.Beacon.Windows;

/// <summary>
/// The About window: the branded artwork, with a version block and a credits page.
///
/// A port of the macOS dialog: same artwork, same two pages, same crossfade, framed as a
/// plain Windows dialog.
///
/// The artwork is a fixed light image, so text drawn over it uses explicit colours rather
/// than theme ones; a label that turned white on a dark theme would vanish into the
/// picture's pale left half.
/// </summary>
internal sealed class AboutWindow : Window
{
    /// <summary>The artwork is 16:9; this keeps that ratio so nothing is cropped or stretched.</summary>
    private const double ArtWidth = 660;

    private const double ArtHeight = 371;
    private const double BarHeight = 44;

    private readonly Image _artPage = new();
    private readonly Grid _creditsPage = new();
    private readonly Button _toggle = new();
    private bool _showingCredits;

    /// <summary>Same sections and names as the macOS shell.</summary>
    private static readonly (string Section, string[] Names)[] Credits =
    [
        ("Gosub Beacon", ["Gosub Team", "Joshua Thijssen", "SharkTheOne"]),
        ("Networking", ["Gosub Team"]),
        ("HTML5 parser", ["Gosub Team"]),
        ("CSS3 parser", ["Gosub Team"]),
        ("Renderer", ["Gosub Team"]),
        ("Javascript engine", ["Gosub Team"]),
        ("UI", ["Gosub Team"]),
        ("Win32 integration", ["Gosub Team"]),
        ("Rust integration", ["Gosub Team"]),
        ("Translations", ["Gosub Team"]),
    ];

    public AboutWindow()
    {
        Title = "About Gosub Beacon";
        Width = ArtWidth;
        Height = ArtHeight + BarHeight;

        // Not resizable: the artwork has one size and letterboxing it would look worse.
        ResizeMode = ResizeMode.NoResize;
        WindowStartupLocation = WindowStartupLocation.CenterOwner;
        SizeToContent = SizeToContent.Manual;
        Background = Brushes.Black;

        BuildArtPage();
        BuildCreditsPage();
        _creditsPage.Opacity = 0;

        // Both pages occupy the same rectangle; only opacity tells them apart.
        var pages = new Grid { Height = ArtHeight, ClipToBounds = true };
        pages.Children.Add(_artPage);
        pages.Children.Add(_creditsPage);

        var bar = BuildBar();
        var root = new DockPanel();
        DockPanel.SetDock(bar, Dock.Bottom);
        root.Children.Add(bar);
        root.Children.Add(pages);
        Content = root;

        // Escape closes, matching the GTK dialog.
        PreviewKeyDown += (_, e) =>
        {
            if (e.Key == Key.Escape)
            {
                Close();
            }
        };
    }

    private void BuildArtPage()
    {
        // Only the picture. The artwork is a finished composition down to the bottom edge,
        // so the version block lives in the bar rather than on top of it.
        _artPage.Source = Artwork("about.jpg");
        _artPage.Stretch = Stretch.Uniform;
    }

    private void BuildCreditsPage()
    {
        // The credits artwork keeps its left half clear; the scrolling column sits right.
        _creditsPage.Children.Add(new Image
        {
            Source = Artwork("about-credits.jpg"),
            Stretch = Stretch.Uniform,
        });

        var list = new StackPanel { Margin = new Thickness(0, 0, 8, 0) };
        foreach (var (section, names) in Credits)
        {
            list.Children.Add(new TextBlock
            {
                Text = section,
                FontSize = 11,
                FontWeight = FontWeights.SemiBold,
                Foreground = Brushes.White,
                Effect = Legibility(),
                Margin = new Thickness(0, 0, 0, 4),
            });

            foreach (var name in names)
            {
                list.Children.Add(new TextBlock
                {
                    Text = "    " + name,
                    FontSize = 11,
                    Foreground = new SolidColorBrush(Color.FromArgb(224, 255, 255, 255)),
                    Effect = Legibility(),
                    Margin = new Thickness(0, 0, 0, 2),
                });
            }

            list.Children.Add(new Border { Height = 10 });
        }

        var scroller = new ScrollViewer
        {
            Content = list,
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto,
            HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
            Background = Brushes.Transparent,
            BorderThickness = new Thickness(0),
            HorizontalAlignment = HorizontalAlignment.Left,
            // Spanning the right of the picture, matching the Mac layout, so the scrollbar
            // lands at the edge of the artwork rather than down the middle.
            Margin = new Thickness(ArtWidth * 0.56, 22, 20, 22),
            Width = ArtWidth - (ArtWidth * 0.56) - 20,
        };

        _creditsPage.Children.Add(scroller);
    }

    private FrameworkElement BuildBar()
    {
        _toggle.Content = "Credits";
        _toggle.Width = 80;
        _toggle.Height = 24;
        _toggle.HorizontalAlignment = HorizontalAlignment.Left;
        _toggle.VerticalAlignment = VerticalAlignment.Center;
        _toggle.Margin = new Thickness(12, 0, 0, 0);
        _toggle.Click += (_, _) => TogglePage();

        var close = new Button
        {
            Content = "Close",
            Width = 80,
            Height = 24,
            HorizontalAlignment = HorizontalAlignment.Right,
            VerticalAlignment = VerticalAlignment.Center,
            Margin = new Thickness(0, 0, 12, 0),
            IsCancel = true,
        };
        close.Click += (_, _) => Close();

        var version = typeof(AboutWindow).Assembly.GetName().Version is { } v
            ? $"{v.Major}.{v.Minor}.{v.Build}"
            : "0.1.0";

        var info = new TextBlock
        {
            FontSize = 10,
            Foreground = Brushes.DimGray,
            VerticalAlignment = VerticalAlignment.Center,
            Text = $"Gosub Beacon {version} · Powered by the Gosub Engine · © 2026 Gosub Project  ",
        };

        var link = new Hyperlink(new Run("https://gosub.io")) { NavigateUri = new Uri("https://gosub.io") };
        link.RequestNavigate += (_, e) =>
        {
            // UseShellExecute hands it to the default browser; without it .NET tries to
            // execute the URL as a program.
            try
            {
                Process.Start(new ProcessStartInfo(e.Uri.AbsoluteUri) { UseShellExecute = true });
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"[beacon] could not open {e.Uri}: {ex.Message}");
            }

            e.Handled = true;
        };

        var linkBlock = new TextBlock { FontSize = 10, VerticalAlignment = VerticalAlignment.Center };
        linkBlock.Inlines.Add(link);

        var centre = new StackPanel
        {
            Orientation = Orientation.Horizontal,
            HorizontalAlignment = HorizontalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Center,
        };
        centre.Children.Add(info);
        centre.Children.Add(linkBlock);

        var bar = new Grid { Height = BarHeight, Background = SystemColors.ControlBrush };
        bar.Children.Add(_toggle);
        bar.Children.Add(centre);
        bar.Children.Add(close);
        return bar;
    }

    private void TogglePage()
    {
        _showingCredits = !_showingCredits;
        _toggle.Content = _showingCredits ? "About" : "Credits";

        var duration = new Duration(TimeSpan.FromMilliseconds(250));
        _artPage.BeginAnimation(OpacityProperty,
            new DoubleAnimation(_showingCredits ? 0 : 1, duration));
        _creditsPage.BeginAnimation(OpacityProperty,
            new DoubleAnimation(_showingCredits ? 1 : 0, duration));
    }

    /// <summary>Keeps white text legible where the lighthouse beam passes behind it.</summary>
    private static DropShadowEffect Legibility() => new()
    {
        Color = Colors.Black,
        BlurRadius = 3,
        ShadowDepth = 1,
        Direction = 270,
        Opacity = 0.85,
    };

    /// <summary>
    /// Load the packed artwork. A missing file yields no image rather than an exception, so
    /// a packaging mistake costs the artwork and not the dialog. The macOS shell crashed on
    /// exactly this in its first packaged build.
    /// </summary>
    private static BitmapImage? Artwork(string name)
    {
        try
        {
            var image = new BitmapImage();
            image.BeginInit();
            image.UriSource = new Uri($"pack://application:,,,/Resources/{name}", UriKind.Absolute);
            image.CacheOption = BitmapCacheOption.OnLoad;
            image.EndInit();
            image.Freeze();
            return image;
        }
        catch (Exception e)
        {
            Console.Error.WriteLine($"[beacon] About artwork '{name}' is not packed: {e.Message}");
            return null;
        }
    }
}
