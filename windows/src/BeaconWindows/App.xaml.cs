using System.Windows;
using System.Windows.Threading;
using Gosub.Beacon.Windows.Interop;

namespace Gosub.Beacon.Windows;

public partial class App : Application
{
    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        // A failure to load beacon.dll arrives as a DllNotFoundException from the first
        // P/Invoke. Unhandled, that is a silent exit with only a WER entry, so say so.
        DispatcherUnhandledException += OnUnhandledException;

        var browser = StartBrowser();
        if (browser is null)
        {
            Shutdown(1);
            return;
        }

        // URLs on the command line become the startup tabs, as in the other shells.
        new MainWindow(browser, e.Args).Show();
    }

    /// <summary>
    /// The browser, or null once the user gives up. A failed start says why and offers
    /// another try: a profile held by a Beacon that is still quitting is free a moment later.
    /// </summary>
    private static BeaconBrowser? StartBrowser()
    {
        while (true)
        {
            try
            {
                return BeaconBrowser.Create();
            }
            catch (BeaconStartException ex)
            {
                // Ctrl+C in a MessageBox copies its text, which is how a hint's command
                // gets out of it.
                var message = ex.Message;
                if (ex.Hint is { } hint)
                {
                    message += "\n\n" + hint;
                }
                if (ex.Detail.Length > 0)
                {
                    message += "\n\nDetails: " + ex.Detail;
                }
                message += "\n\nTry again?";
                var answer = MessageBox.Show(
                    message, "Gosub Beacon", MessageBoxButton.YesNo, MessageBoxImage.Error);
                if (answer != MessageBoxResult.Yes)
                {
                    return null;
                }
            }
        }
    }

    private static void OnUnhandledException(object sender, DispatcherUnhandledExceptionEventArgs e)
    {
        var message = e.Exception is DllNotFoundException
            ? "Gosub Beacon could not load beacon.dll.\n\n" +
              "Reinstall Gosub Beacon. If that does not help, install the Microsoft Visual C++ " +
              "Redistributable (x64), which beacon.dll needs.\n\n" +
              "Details: " + e.Exception.Message
            : e.Exception.ToString();

        MessageBox.Show(message, "Gosub Beacon", MessageBoxButton.OK, MessageBoxImage.Error);
        e.Handled = true;
        Current.Shutdown(1);
    }
}
