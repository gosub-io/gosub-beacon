using System.Windows;
using System.Windows.Threading;

namespace Gosub.Beacon.Windows;

public partial class App : Application
{
    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        // A failure to load beacon.dll arrives as a DllNotFoundException from the first
        // P/Invoke. Unhandled, that is a silent exit with only a WER entry, so report what
        // is wrong and how to fix it.
        DispatcherUnhandledException += OnUnhandledException;

        // URLs on the command line become the startup tabs, as in the other shells.
        new MainWindow(e.Args).Show();
    }

    private static void OnUnhandledException(object sender, DispatcherUnhandledExceptionEventArgs e)
    {
        var message = e.Exception is DllNotFoundException
            ? "beacon.dll could not be loaded.\n\n" +
              "Build it first:\n" +
              "    cargo xwin build -p beacon-ffi --target x86_64-pc-windows-msvc\n\n" +
              "and make sure it sits next to GosubBeacon.exe. The MSVC build also needs the " +
              "Visual C++ redistributable (it imports VCRUNTIME140.dll).\n\n" +
              e.Exception.Message
            : e.Exception.ToString();

        MessageBox.Show(message, "Gosub Beacon", MessageBoxButton.OK, MessageBoxImage.Error);
        e.Handled = true;
        Current.Shutdown(1);
    }
}
