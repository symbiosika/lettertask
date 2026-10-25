package de.symbiosika.lettertask

import android.app.AlarmManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.os.Build
import android.os.IBinder
import android.os.SystemClock
import androidx.core.content.ContextCompat

/**
 * Stufe 2 ("Sofort benachrichtigen"): keeps the process alive so the Rust
 * service can hold IMAP IDLE connections, like Thunderbird's PushService.
 * Timers do not run while the device sleeps (Doze), so an alarm renews IDLE
 * regularly, and a network change reconnects at once.
 */
class PushService : Service() {
    private var networkCallback: ConnectivityManager.NetworkCallback? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        Notifications.ensureChannels(this)
        val n = Notifications.pushService(this)
        when {
            Build.VERSION.SDK_INT >= 34 -> startForeground(Notifications.PUSH_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
            Build.VERSION.SDK_INT >= 29 -> startForeground(Notifications.PUSH_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
            else -> startForeground(Notifications.PUSH_ID, n)
        }
        LettertaskNative.init(this)
        Thread { LettertaskNative.pushStart() }.start()
        scheduleRenew(this)
        watchNetwork()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
        cancelRenew(this)
        networkCallback?.let { cb ->
            try { getSystemService(ConnectivityManager::class.java)?.unregisterNetworkCallback(cb) } catch (_: Exception) {}
        }
        super.onDestroy()
    }

    private fun watchNetwork() {
        if (Build.VERSION.SDK_INT < 24) return
        val cm = getSystemService(ConnectivityManager::class.java) ?: return
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                Thread { LettertaskNative.pushRefresh() }.start()
            }
        }
        try {
            cm.registerDefaultNetworkCallback(cb)
            networkCallback = cb
        } catch (_: Exception) {}
    }

    companion object {
        /** Renew before servers and NAT routers drop a silent connection. */
        private const val RENEW_MS = 24 * 60 * 1000L

        fun start(ctx: Context) {
            try {
                ContextCompat.startForegroundService(ctx, Intent(ctx, PushService::class.java))
            } catch (_: Exception) {
                // Not allowed from the background right now; the next app start retries.
            }
        }

        fun stop(ctx: Context) {
            ctx.stopService(Intent(ctx, PushService::class.java))
        }

        private fun renewIntent(ctx: Context) = PendingIntent.getBroadcast(
            ctx, 1, Intent(ctx, PushRenewReceiver::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

        /** Exact when permitted, otherwise inexact (still allowed in Doze). */
        fun scheduleRenew(ctx: Context) {
            val am = ctx.getSystemService(AlarmManager::class.java) ?: return
            val at = SystemClock.elapsedRealtime() + RENEW_MS
            val pi = renewIntent(ctx)
            val exact = Build.VERSION.SDK_INT < 31 || am.canScheduleExactAlarms()
            when {
                Build.VERSION.SDK_INT < 23 -> am.set(AlarmManager.ELAPSED_REALTIME_WAKEUP, at, pi)
                exact -> am.setExactAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, at, pi)
                else -> am.setAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, at, pi)
            }
        }

        fun cancelRenew(ctx: Context) {
            ctx.getSystemService(AlarmManager::class.java)?.cancel(renewIntent(ctx))
        }
    }
}
