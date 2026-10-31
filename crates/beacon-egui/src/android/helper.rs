//! Calls into `io.gosub.beacon.Helper`, the Java class `scripts/android-apk.sh` compiles
//! from `packaging/android/java` and dexes into the APK.
//!
//! A native thread cannot find an app class through `FindClass`, so the class is loaded once
//! through the activity's class loader and kept. Every call attaches the calling thread for
//! its own scope: a download is published from a thread of its own, not the UI thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use jni::objects::{Global, JClass, JClassLoader, JObject, JString, JValue};
use jni::{jni_sig, jni_str, Env};

static HELPER: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Each published download's private path, and the content:// URI of its shared copy.
static PUBLISHED: Mutex<Option<HashMap<PathBuf, String>>> = Mutex::new(None);

/// Run `call` with the helper class and the activity. `None` (logged) when Java threw or the
/// activity is not up.
fn with_helper<T>(what: &str, call: impl FnOnce(&mut Env, &JClass, &JObject) -> jni::errors::Result<T>) -> Option<T> {
    let app = super::APP.get()?;
    // SAFETY: both pointers belong to the running NativeActivity and outlive this call, as in
    // `init_certificate_verifier`.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
    let result = vm.attach_current_thread_for_scope(|env| -> jni::errors::Result<T> {
        let activity = unsafe { JObject::from_raw(env, app.activity_as_ptr().cast()) };
        let class = match HELPER.get() {
            Some(class) => class,
            None => {
                let class = load_helper(env, &activity)?;
                HELPER.get_or_init(|| class)
            }
        };
        let result = call(env, class, &activity);
        if result.is_err() && env.exception_check() {
            env.exception_clear();
        }
        result
    });
    result.map_err(|e| log::warn!("{what}: {e}")).ok()
}

fn load_helper(env: &mut Env, activity: &JObject) -> jni::errors::Result<Global<JClass<'static>>> {
    let loader = env
        .call_method(activity, jni_str!("getClassLoader"), jni_sig!(() -> JClassLoader), &[])?
        .l()?;
    let loader = env.cast_local::<JClassLoader>(loader)?;
    let name = env.new_string("io.gosub.beacon.Helper")?;
    let class = loader.load_class(env, name)?;
    env.new_global_ref(class)
}

/// Open a date dialog on `(year, month, day)` with a 1-based month, or on today for `None`.
/// The bounds are epoch milliseconds.
pub fn show_date_picker(date: Option<(i32, u32, u32)>, min: Option<i64>, max: Option<i64>) {
    let (year, month0, day) = match date {
        Some((y, m, d)) => (y, m as i32 - 1, d as i32),
        None => (-1, 0, 0),
    };
    with_helper("cannot open the date picker", |env, class, activity| {
        env.call_static_method(
            class,
            jni_str!("showDatePicker"),
            jni_sig!("(Landroid/app/Activity;IIIJJ)V"),
            &[
                JValue::from(activity),
                JValue::Int(year),
                JValue::Int(month0),
                JValue::Int(day),
                JValue::Long(min.unwrap_or(i64::MIN)),
                JValue::Long(max.unwrap_or(i64::MIN)),
            ],
        )?;
        Ok(())
    });
}

/// Open a time dialog on `(hour, minute)`, or on now for `None`.
pub fn show_time_picker(time: Option<(u32, u32)>) {
    let (hour, minute) = time.map_or((-1, 0), |(h, m)| (h as i32, m as i32));
    with_helper("cannot open the time picker", |env, class, activity| {
        env.call_static_method(
            class,
            jni_str!("showTimePicker"),
            jni_sig!("(Landroid/app/Activity;II)V"),
            &[JValue::from(activity), JValue::Int(hour), JValue::Int(minute)],
        )?;
        Ok(())
    });
}

/// The open dialog's answer, taken: `None` while it is open, `Some("")` when it was cancelled,
/// otherwise `"y m d"` (0-based month) or `"h m"`.
pub fn take_picker_result() -> Option<String> {
    with_helper("cannot read the picker", |env, class, _| {
        let result = env
            .call_static_method(class, jni_str!("takePickerResult"), jni_sig!("()Ljava/lang/String;"), &[])?
            .l()?;
        if result.is_null() {
            return Ok(None);
        }
        let result = env.cast_local::<JString>(result)?;
        result.try_to_string(env).map(Some)
    })
    .flatten()
}

/// Put `text` on the system clipboard.
pub fn copy_text(text: &str) {
    with_helper("cannot copy to the clipboard", |env, class, activity| {
        let text = env.new_string(text)?;
        env.call_static_method(
            class,
            jni_str!("copyText"),
            jni_sig!("(Landroid/content/Context;Ljava/lang/String;)V"),
            &[JValue::from(activity), JValue::from(&text)],
        )?;
        Ok(())
    });
}

/// The text on the system clipboard, `None` when it holds none.
pub fn paste_text() -> Option<String> {
    with_helper("cannot read the clipboard", |env, class, activity| {
        let text = env
            .call_static_method(
                class,
                jni_str!("pasteText"),
                jni_sig!("(Landroid/content/Context;)Ljava/lang/String;"),
                &[JValue::from(activity)],
            )?
            .l()?;
        if text.is_null() {
            return Ok(None);
        }
        let text = env.cast_local::<JString>(text)?;
        text.try_to_string(env).map(Some)
    })
    .flatten()
}

/// Copy the finished download at `path` into the shared Downloads folder as `name`. Blocks
/// for the copy, so call it off the UI thread. Returns whether it worked; Android before 10
/// has no shared Downloads collection to put it in.
pub fn publish_download(path: &Path, name: &str) -> bool {
    let uri = with_helper("cannot publish the download", |env, class, activity| {
        let src = env.new_string(path.to_string_lossy())?;
        let name = env.new_string(name)?;
        let uri = env
            .call_static_method(
                class,
                jni_str!("publishDownload"),
                jni_sig!("(Landroid/content/Context;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
                &[JValue::from(activity), JValue::from(&src), JValue::from(&name)],
            )?
            .l()?;
        if uri.is_null() {
            return Ok(None);
        }
        let uri = env.cast_local::<JString>(uri)?;
        uri.try_to_string(env).map(Some)
    })
    .flatten();
    match uri {
        Some(uri) => {
            PUBLISHED
                .lock()
                .unwrap()
                .get_or_insert_with(HashMap::new)
                .insert(path.to_path_buf(), uri);
            true
        }
        None => false,
    }
}

/// Open a published download in the app that handles its type. A path inside the app's
/// own storage cannot be handed to another app, so only a published one opens.
pub fn open_download(path: &Path) {
    let uri = PUBLISHED.lock().unwrap().as_ref().and_then(|p| p.get(path).cloned());
    let Some(uri) = uri else {
        log::warn!("{} is not in Downloads; nothing to open", path.display());
        return;
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    with_helper("cannot open the download", |env, class, activity| {
        let uri = env.new_string(&uri)?;
        let name = env.new_string(&name)?;
        env.call_static_method(
            class,
            jni_str!("viewUri"),
            jni_sig!("(Landroid/app/Activity;Ljava/lang/String;Ljava/lang/String;)Z"),
            &[JValue::from(activity), JValue::from(&uri), JValue::from(&name)],
        )?;
        Ok(())
    });
}
