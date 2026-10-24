package de.symbiosika.lettertask

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import java.time.OffsetDateTime
import java.util.concurrent.TimeUnit
import org.json.JSONObject

/**
 * Stufe 1: a periodic WorkManager pass (network required, like Thunderbird's
 * MailSyncWorker), plus an alarm for the next due Wiedervorlage.
 * Stufe 2: the push service, only while push is on and an account can use it.
 * What to run comes from Rust (`BackgroundPlan`).
 */
object Background {
    private const val PERIODIC = "mail-sync"
    private const val WAKEUP = "mail-sync-wakeup"

    private val network = Constraints.Builder()
        .setRequiredNetworkType(NetworkType.CONNECTED)
        .setRequiresStorageNotLow(true)
        .build()

    fun apply(ctx: Context) {
        LettertaskNative.init(ctx)
        val plan = try { JSONObject(LettertaskNative.plan()) } catch (_: Exception) { return }
        val interval = plan.optLong("intervalMins", 0)
        val wm = WorkManager.getInstance(ctx)
        if (interval > 0) {
            val req = PeriodicWorkRequestBuilder<MailSyncWorker>(interval.coerceAtLeast(15), TimeUnit.MINUTES)
                .setConstraints(network)
                .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 5, TimeUnit.MINUTES)
                .build()
            wm.enqueueUniquePeriodicWork(PERIODIC, ExistingPeriodicWorkPolicy.UPDATE, req)
        } else {
            wm.cancelUniqueWork(PERIODIC)
        }
        if (plan.optBoolean("push")) PushService.start(ctx) else PushService.stop(ctx)
    }

    /** One pass soon, e.g. when a Wiedervorlage is due. */
    fun syncNow(ctx: Context) {
        val req = OneTimeWorkRequestBuilder<MailSyncWorker>().setConstraints(network).build()
        WorkManager.getInstance(ctx).enqueueUniqueWork(WAKEUP, ExistingWorkPolicy.REPLACE, req)
    }

    /**
     * Wakes up for the next due Wiedervorlage. Inexact but allowed in Doze:
     * no special permission, at most a few minutes late.
     */
    fun scheduleWakeup(ctx: Context, nextWakeup: String?) {
        val am = ctx.getSystemService(AlarmManager::class.java) ?: return
        val pi = PendingIntent.getBroadcast(
            ctx, 0, Intent(ctx, WakeupReceiver::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        am.cancel(pi)
        if (nextWakeup.isNullOrEmpty() || Build.VERSION.SDK_INT < 26) return
        val at = try { OffsetDateTime.parse(nextWakeup).toInstant().toEpochMilli() } catch (_: Exception) { return }
        if (Build.VERSION.SDK_INT >= 23) {
            am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pi)
        } else {
            am.set(AlarmManager.RTC_WAKEUP, at, pi)
        }
    }
}
