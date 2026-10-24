package de.symbiosika.lettertask

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat

object Notifications {
    private const val MAIL = "mail"
    private const val PUSH = "push"
    const val PUSH_ID = 1

    fun ensureChannels(ctx: Context) {
        if (Build.VERSION.SDK_INT < 26) return
        val nm = ctx.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(MAIL, "Neue E-Mails und Wiedervorlagen", NotificationManager.IMPORTANCE_DEFAULT))
        nm.createNotificationChannel(
            NotificationChannel(PUSH, "Sofort benachrichtigen (läuft im Hintergrund)", NotificationManager.IMPORTANCE_MIN).apply {
                setShowBadge(false)
            },
        )
    }

    private fun openApp(ctx: Context): PendingIntent {
        val intent = Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        return PendingIntent.getActivity(ctx, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    fun showMail(ctx: Context, title: String, body: String) {
        ensureChannels(ctx)
        val n = NotificationCompat.Builder(ctx, MAIL)
            .setSmallIcon(R.drawable.ic_stat_mail)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setCategory(NotificationCompat.CATEGORY_EMAIL)
            .setContentIntent(openApp(ctx))
            .setAutoCancel(true)
            .build()
        try {
            NotificationManagerCompat.from(ctx).notify((System.currentTimeMillis() and 0x7fffffff).toInt(), n)
        } catch (_: SecurityException) {
            // Notifications not permitted (Android 13+): nothing to show.
        }
    }

    /** The lasting notification Android requires for the push service. */
    fun pushService(ctx: Context) =
        NotificationCompat.Builder(ctx, PUSH)
            .setSmallIcon(R.drawable.ic_stat_mail)
            .setContentTitle("Sofort benachrichtigen ist aktiv")
            .setContentText("Neue E-Mails kommen ohne Verzögerung an.")
            .setContentIntent(openApp(ctx))
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_MIN)
            .build()
}
