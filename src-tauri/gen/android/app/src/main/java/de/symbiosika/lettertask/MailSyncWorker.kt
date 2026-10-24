package de.symbiosika.lettertask

import android.content.Context
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONObject

/** One background pass of the shared Rust service. */
class MailSyncWorker(ctx: Context, params: WorkerParameters) : Worker(ctx, params) {
    override fun doWork(): Result {
        LettertaskNative.init(applicationContext)
        val run = try { JSONObject(LettertaskNative.backgroundSync()) } catch (_: Exception) { return Result.retry() }
        Background.scheduleWakeup(applicationContext, run.optString("nextWakeup").takeIf { it.isNotEmpty() && it != "null" })
        return if (run.optBoolean("ok", true)) Result.success() else Result.retry()
    }
}
