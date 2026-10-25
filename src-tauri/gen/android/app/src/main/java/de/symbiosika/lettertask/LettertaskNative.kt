package de.symbiosika.lettertask

import android.content.Context
import android.os.Handler
import android.os.Looper
import java.io.File

/**
 * Bridge to the Rust service (src-tauri/src/android.rs). Kotlin only plans
 * when to run (WorkManager, push service, alarms); all mail logic is in Rust.
 */
object LettertaskNative {
    @Volatile private var context: Context? = null

    init {
        System.loadLibrary("lettertask_lib")
    }

    /** Call before anything else; safe to call again. */
    fun init(ctx: Context) {
        if (context != null) return
        val app = ctx.applicationContext
        context = app
        // Same place as Tauri's app data dir: dataDir/<package>.
        val base = if (android.os.Build.VERSION.SDK_INT >= 24) app.dataDir else File(app.applicationInfo.dataDir)
        nativeInit(File(base, app.packageName).absolutePath)
    }

    @JvmStatic private external fun nativeInit(dataDir: String)

    /** `{ intervalMins, push }`: what to keep running. */
    @JvmStatic external fun plan(): String

    /** One background pass; blocks. `{ ok, skipped, nextWakeup }`. */
    @JvmStatic external fun backgroundSync(): String

    @JvmStatic external fun pushStart()

    @JvmStatic external fun pushRefresh()

    @JvmStatic external fun setAppVisible(visible: Boolean)

    // ---- called from Rust ----

    @JvmStatic
    fun notify(title: String, body: String) {
        context?.let { Notifications.showMail(it, title, body) }
    }

    /** Settings or accounts changed: re-plan worker and push service. */
    @JvmStatic
    fun onSettingsChanged() {
        val ctx = context ?: return
        Handler(Looper.getMainLooper()).post { Background.apply(ctx) }
    }
}
