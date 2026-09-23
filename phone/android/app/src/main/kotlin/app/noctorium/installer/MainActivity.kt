package app.noctorium.installer

import android.content.Intent
import android.net.Uri
import androidx.core.content.FileProvider
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import java.io.File

/**
 * The one thing Dart cannot do: ask Android to install a package.
 *
 * Everything else -- asking GitHub, downloading, hashing -- is Dart. This is a single method that turns a
 * path into a content URI the system package installer is allowed to read, and fires the intent. It
 * installs nothing itself; Android shows its own screen, names the application and asks.
 */
class MainActivity : FlutterActivity() {

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, CHANNEL).setMethodCallHandler { call, result ->
            when (call.method) {
                "install" -> {
                    val path = call.argument<String>("path")
                    if (path.isNullOrBlank()) {
                        // Answered rather than thrown: the Dart side reports a complaint as text and has
                        // nowhere sensible to put an exception.
                        result.success("No file was given to install.")
                    } else {
                        result.success(handOver(File(path)))
                    }
                }
                else -> result.notImplemented()
            }
        }
    }

    /** Null when Android took it, or a sentence saying why it did not. */
    private fun handOver(file: File): String? = runCatching {
        if (!file.isFile) return "The downloaded file is no longer where it was put."
        val uri: Uri = FileProvider.getUriForFile(this, "$packageName.files", file)
        val intent = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(uri, "application/vnd.android.package-archive")
            // The installer runs in its own task, and has to be allowed to read a file belonging to this
            // application -- without the grant it opens on an error rather than on the APK.
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        startActivity(intent)
        null
    }.getOrElse { error ->
        "Could not open Android's installer: ${error.message}. The APK is in this app's files."
    }

    private companion object {
        const val CHANNEL = "app.noctorium.installer/install"
    }
}
