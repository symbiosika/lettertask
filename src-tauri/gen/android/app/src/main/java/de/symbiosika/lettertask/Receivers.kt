package de.symbiosika.lettertask

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.PowerManager

/** After a reboot or an app update: plan worker and push service again. */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED && intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
        Background.apply(ctx)
    }
}

/** A Wiedervorlage is due: one pass now. */
class WakeupReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        Background.syncNow(ctx)
    }
}

/** Push renewal alarm: wake the CPU briefly, renew IDLE, plan the next one. */
class PushRenewReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        val pending = goAsync()
        val wake = ctx.getSystemService(PowerManager::class.java)
            ?.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "lettertask:push-renew")
        wake?.acquire(30_000)
        PushService.scheduleRenew(ctx)
        Thread {
            try {
                LettertaskNative.init(ctx)
                LettertaskNative.pushRefresh()
                // The renewal itself runs asynchronously in Rust; keep the CPU up a moment.
                Thread.sleep(10_000)
            } finally {
                if (wake?.isHeld == true) wake.release()
                pending.finish()
            }
        }.start()
    }
}
