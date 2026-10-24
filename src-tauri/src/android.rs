//! Android only: the bridge between the shared service and the platform parts
//! in Kotlin (`gen/android/.../LettertaskNative.kt`): WorkManager runs the
//! periodic background pass, a foreground service keeps push alive, alarms
//! wake both up. Kotlin calls in through the JNI functions below; the service
//! calls out for notifications and when settings change. No mail logic here.
//!
//! One service per process: the window, the worker and the push service share
//! it, whoever starts first creates it.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use async_trait::async_trait;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::{jboolean, jstring, JNI_TRUE};
use jni::{JNIEnv, JavaVM};
use mailcore::{Host, Service};
use serde_json::{json, Value};

static VM: OnceLock<JavaVM> = OnceLock::new();
/// The Kotlin class `LettertaskNative`, for calls from Rust threads (they
/// cannot look up app classes themselves).
static NATIVE: OnceLock<GlobalRef> = OnceLock::new();
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static SERVICE: Mutex<Option<Arc<Service>>> = Mutex::new(None);
static HOST: OnceLock<Arc<AndroidHost>> = OnceLock::new();

/// Where the real data lives (`dataDir/<package>`, like Tauri's app data dir).
pub(crate) fn data_dir() -> Option<PathBuf> {
    DATA_DIR.get().cloned()
}

pub(crate) fn service() -> Option<Arc<Service>> {
    SERVICE.lock().unwrap().clone()
}

pub(crate) fn set_service(svc: Arc<Service>) {
    *SERVICE.lock().unwrap() = Some(svc);
    // The window's service may differ from what Kotlin planned with.
    settings_changed();
}

fn host() -> Arc<AndroidHost> {
    HOST.get_or_init(|| Arc::new(AndroidHost { ui: RwLock::new(None) })).clone()
}

/// The host for the window's service: window events go to the window,
/// notifications always to Android (same channel with or without window).
pub(crate) fn host_with_ui(ui: Arc<dyn Host>) -> Arc<dyn Host> {
    let h = host();
    *h.ui.write().unwrap() = Some(ui);
    h
}

pub(crate) struct AndroidHost {
    ui: RwLock<Option<Arc<dyn Host>>>,
}

impl AndroidHost {
    fn ui(&self) -> Option<Arc<dyn Host>> {
        self.ui.read().unwrap().clone()
    }
}

fn without_window() -> mailcore::Error {
    mailcore::Error::invalid("Nur bei geöffneter App möglich.")
}

#[async_trait]
impl Host for AndroidHost {
    fn emit(&self, event: &str, payload: Value) {
        if matches!(event, "config-changed" | "accounts-changed") {
            settings_changed();
        }
        if let Some(ui) = self.ui() {
            ui.emit(event, payload);
        }
    }
    fn open_url(&self, url: &str) -> mailcore::Result<()> {
        self.ui().ok_or_else(without_window)?.open_url(url)
    }
    fn open_path(&self, path: &std::path::Path) -> mailcore::Result<()> {
        self.ui().ok_or_else(without_window)?.open_path(path)
    }
    async fn save_dialog(&self, suggested_name: &str) -> Option<PathBuf> {
        let ui = self.ui()?;
        ui.save_dialog(suggested_name).await
    }
    fn notify(&self, title: &str, body: &str) {
        call_kotlin("notify", "(Ljava/lang/String;Ljava/lang/String;)V", &[title, body]);
    }
    fn open_window(&self, kind: &str, query: &str) -> mailcore::Result<()> {
        self.ui().ok_or_else(without_window)?.open_window(kind, query)
    }
}

/// Static method of `LettertaskNative` with string arguments, from any thread.
fn call_kotlin(name: &str, sig: &str, args: &[&str]) {
    let (Some(vm), Some(class)) = (VM.get(), NATIVE.get()) else { return };
    let run = || -> jni::errors::Result<()> {
        let mut env = vm.attach_current_thread()?;
        let strings = args.iter().map(|a| env.new_string(a)).collect::<jni::errors::Result<Vec<_>>>()?;
        let values: Vec<JValue> = strings.iter().map(|s| JValue::Object(s.as_ref())).collect();
        let class: &JClass = class.as_obj().into();
        env.call_static_method(class, name, sig, &values)?;
        Ok(())
    };
    if let Err(e) = run() {
        tracing::warn!("Android-Aufruf {name}: {e}");
    }
}

fn settings_changed() {
    call_kotlin("onSettingsChanged", "()V", &[]);
}

/// The process's service; without a window one is started for the real
/// accounts (not in demo builds: the demo world lives only in the window).
fn ensure_service() -> Option<Arc<Service>> {
    let mut slot = SERVICE.lock().unwrap();
    if let Some(s) = slot.as_ref() {
        return Some(s.clone());
    }
    if crate::mobile_demo() {
        return None;
    }
    let dir = data_dir()?;
    match Service::new(crate::real_options(dir), host()) {
        Ok(s) => {
            *slot = Some(s.clone());
            Some(s)
        }
        Err(e) => {
            tracing::error!("Dienst ohne Fenster: {e}");
            None
        }
    }
}

fn to_jstring(env: &mut JNIEnv, v: Value) -> jstring {
    env.new_string(v.to_string()).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

// ---- called from Kotlin (LettertaskNative) ----

#[no_mangle]
pub extern "system" fn Java_de_symbiosika_lettertask_LettertaskNative_nativeInit(mut env: JNIEnv, class: JClass, data_dir: JString) {
    if let Ok(vm) = env.get_java_vm() {
        let _ = VM.set(vm);
    }
    if NATIVE.get().is_none() {
        if let Ok(g) = env.new_global_ref(JObject::from(class)) {
            let _ = NATIVE.set(g);
        }
    }
    if let Ok(dir) = env.get_string(&data_dir) {
        let _ = DATA_DIR.set(PathBuf::from(String::from(dir)));
    }
}

/// What Kotlin keeps running: `{ intervalMins, push }` (see `BackgroundPlan`).
#[no_mangle]
pub extern "system" fn Java_de_symbiosika_lettertask_LettertaskNative_plan(mut env: JNIEnv, _class: JClass) -> jstring {
    let v = match ensure_service() {
        Some(svc) => serde_json::to_value(svc.background_plan()).unwrap_or_default(),
        None => json!({ "intervalMins": 0, "push": false }),
    };
    to_jstring(&mut env, v)
}

/// One background pass (WorkManager thread; blocks until done).
#[no_mangle]
pub extern "system" fn Java_de_symbiosika_lettertask_LettertaskNative_backgroundSync(mut env: JNIEnv, _class: JClass) -> jstring {
    let v = match ensure_service() {
        Some(svc) => {
            let run = tauri::async_runtime::block_on(async move { svc.background_sync().await });
            serde_json::to_value(run).unwrap_or_default()
        }
        None => json!({ "ok": true, "skipped": "demo" }),
    };
    to_jstring(&mut env, v)
}

/// The push service started: listen if push is on.
#[no_mangle]
pub extern "system" fn Java_de_symbiosika_lettertask_LettertaskNative_pushStart(_env: JNIEnv, _class: JClass) {
    if let Some(svc) = ensure_service() {
        tauri::async_runtime::block_on(async move { svc.apply_push() });
    }
}

/// Alarm after Doze, or the network changed: renew IDLE and catch up.
#[no_mangle]
pub extern "system" fn Java_de_symbiosika_lettertask_LettertaskNative_pushRefresh(_env: JNIEnv, _class: JClass) {
    if let Some(svc) = service() {
        tauri::async_runtime::block_on(async move {
            svc.apply_push();
            svc.refresh_push();
        });
    }
}

/// The window became visible or went to the background.
#[no_mangle]
pub extern "system" fn Java_de_symbiosika_lettertask_LettertaskNative_setAppVisible(_env: JNIEnv, _class: JClass, visible: jboolean) {
    if let Some(svc) = service() {
        svc.set_interactive(visible == JNI_TRUE);
    }
}
