package io.gosub.beacon;

import android.app.Activity;
import android.app.DatePickerDialog;
import android.app.TimePickerDialog;
import android.content.ActivityNotFoundException;
import android.content.ContentResolver;
import android.content.ContentValues;
import android.content.Context;
import android.content.DialogInterface;
import android.content.Intent;
import android.net.Uri;
import android.os.Environment;
import android.provider.MediaStore;
import android.util.Log;
import android.webkit.MimeTypeMap;
import android.widget.DatePicker;
import android.widget.TimePicker;

import java.io.FileInputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.Calendar;
import java.util.Locale;
import java.util.TimeZone;

/**
 * What Beacon needs from Android that Rust cannot do on its own: a dialog needs a listener
 * class, and a finished download has to go through MediaStore to land in the Downloads
 * folder other apps can see. Static methods only, called over JNI from beacon-egui's
 * android_helper module. Values go in and out as plain ints and strings; formatting and
 * parsing stay on the Rust side.
 */
public final class Helper {
    private static final String TAG = "beacon";

    /** The answer of the picker that is open: null while it is, "" when it was cancelled. */
    private static String pickerResult;
    /** Counts pickers, so a dialog that closes after a newer one opened cannot answer for it. */
    private static int pickerSerial;

    private Helper() {}

    /**
     * Open a date picker at year, month (0-based, as Android counts) and day, or at today for
     * a negative year. The bounds are epoch milliseconds of a day's noon UTC, Long.MIN_VALUE
     * for none. The answer is "y m d" with a 0-based month.
     */
    public static void showDatePicker(final Activity activity, int y0, int m0, int d0,
            final long minMillis, final long maxMillis) {
        if (y0 < 0) {
            Calendar now = Calendar.getInstance();
            y0 = now.get(Calendar.YEAR);
            m0 = now.get(Calendar.MONTH);
            d0 = now.get(Calendar.DAY_OF_MONTH);
        }
        final int year = y0;
        final int month = m0;
        final int day = d0;
        final int serial = openPicker();
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                DatePickerDialog dialog = new DatePickerDialog(activity, new DatePickerDialog.OnDateSetListener() {
                    @Override
                    public void onDateSet(DatePicker view, int y, int m, int d) {
                        answer(serial, y + " " + m + " " + d);
                    }
                }, year, month, day);
                if (minMillis != Long.MIN_VALUE) {
                    dialog.getDatePicker().setMinDate(localNoon(minMillis));
                }
                if (maxMillis != Long.MIN_VALUE) {
                    dialog.getDatePicker().setMaxDate(localNoon(maxMillis));
                }
                dialog.setOnDismissListener(cancelOnDismiss(serial));
                dialog.show();
            }
        });
    }

    /**
     * The same calendar date as a noon UTC, at noon in the phone's own zone: the dialog reads
     * its bounds in that zone, where noon UTC is the next day from UTC+12 on.
     */
    private static long localNoon(long utcNoonMillis) {
        Calendar utc = Calendar.getInstance(TimeZone.getTimeZone("UTC"));
        utc.setTimeInMillis(utcNoonMillis);
        Calendar local = Calendar.getInstance();
        local.clear();
        local.set(utc.get(Calendar.YEAR), utc.get(Calendar.MONTH), utc.get(Calendar.DAY_OF_MONTH), 12, 0);
        return local.getTimeInMillis();
    }

    /** Open a time picker at hour and minute, or at now for a negative hour. The answer is "h m". */
    public static void showTimePicker(final Activity activity, int h0, int m0) {
        if (h0 < 0) {
            Calendar now = Calendar.getInstance();
            h0 = now.get(Calendar.HOUR_OF_DAY);
            m0 = now.get(Calendar.MINUTE);
        }
        final int hour = h0;
        final int minute = m0;
        final int serial = openPicker();
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                TimePickerDialog dialog = new TimePickerDialog(activity, new TimePickerDialog.OnTimeSetListener() {
                    @Override
                    public void onTimeSet(TimePicker view, int h, int m) {
                        answer(serial, h + " " + m);
                    }
                }, hour, minute, android.text.format.DateFormat.is24HourFormat(activity));
                dialog.setOnDismissListener(cancelOnDismiss(serial));
                dialog.show();
            }
        });
    }

    /**
     * The open picker's answer, taken: null while it is still open (or none was opened), ""
     * when it was cancelled, otherwise what showDatePicker or showTimePicker describe.
     */
    public static synchronized String takePickerResult() {
        String result = pickerResult;
        pickerResult = null;
        return result;
    }

    private static synchronized int openPicker() {
        pickerResult = null;
        return ++pickerSerial;
    }

    /** The first answer a picker gives counts; a set is followed by a dismiss, which is not a cancel. */
    private static synchronized void answer(int serial, String result) {
        if (serial == pickerSerial && pickerResult == null) {
            pickerResult = result;
            pickerSerial++;
        }
    }

    private static DialogInterface.OnDismissListener cancelOnDismiss(final int serial) {
        return new DialogInterface.OnDismissListener() {
            @Override
            public void onDismiss(DialogInterface dialog) {
                answer(serial, "");
            }
        };
    }

    /**
     * Copy the file at srcPath into the shared Downloads folder as displayName, and return
     * its content:// URI, or null when that failed.
     */
    public static String publishDownload(Context context, String srcPath, String displayName) {
        ContentResolver resolver = context.getContentResolver();
        ContentValues values = new ContentValues();
        values.put(MediaStore.MediaColumns.DISPLAY_NAME, displayName);
        values.put(MediaStore.MediaColumns.MIME_TYPE, mimeTypeOf(displayName));
        values.put(MediaStore.MediaColumns.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS);
        values.put(MediaStore.MediaColumns.IS_PENDING, 1);
        Uri uri = null;
        try {
            uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values);
            if (uri == null) {
                return null;
            }
            InputStream in = new FileInputStream(srcPath);
            try {
                OutputStream out = resolver.openOutputStream(uri);
                try {
                    byte[] buffer = new byte[64 * 1024];
                    int n;
                    while ((n = in.read(buffer)) > 0) {
                        out.write(buffer, 0, n);
                    }
                } finally {
                    out.close();
                }
            } finally {
                in.close();
            }
            values.clear();
            values.put(MediaStore.MediaColumns.IS_PENDING, 0);
            resolver.update(uri, values, null, null);
            return uri.toString();
        } catch (Exception e) {
            Log.w(TAG, "cannot publish download " + displayName, e);
            if (uri != null) {
                resolver.delete(uri, null, null);
            }
            return null;
        }
    }

    /** Open a content:// URI in whatever app handles its type. Returns false when none does. */
    public static boolean viewUri(Activity activity, String uri, String displayName) {
        Intent intent = new Intent(Intent.ACTION_VIEW);
        intent.setDataAndType(Uri.parse(uri), mimeTypeOf(displayName));
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_ACTIVITY_NEW_TASK);
        try {
            activity.startActivity(intent);
            return true;
        } catch (ActivityNotFoundException e) {
            Log.w(TAG, "no app opens " + displayName, e);
            return false;
        }
    }

    private static String mimeTypeOf(String name) {
        int dot = name.lastIndexOf('.');
        if (dot >= 0) {
            String type = MimeTypeMap.getSingleton().getMimeTypeFromExtension(name.substring(dot + 1).toLowerCase(Locale.ROOT));
            if (type != null) {
                return type;
            }
        }
        return "application/octet-stream";
    }
}
