package ma.burgeros.app

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothSocket
import android.content.Context
import android.os.Build
import android.util.Base64
import app.tauri.PermissionState
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.util.UUID
import kotlin.concurrent.thread

@InvokeArg
class WriteArgs {
    lateinit var target: String
    lateinit var data: String
}

/**
 * Bluetooth ESC/POS printing over RFCOMM (SPP). Also used for the SUNMI built-in printer,
 * which SUNMI exposes as a paired Bluetooth device named "InnerPrinter".
 * Called from Rust (print worker thread); socket I/O runs on its own thread.
 */
@TauriPlugin(
    permissions = [
        Permission(strings = [Manifest.permission.BLUETOOTH_CONNECT], alias = "bluetooth")
    ]
)
class PrinterPlugin(private val activity: Activity) : Plugin(activity) {
    private val spp: UUID = UUID.fromString("00001101-0000-1000-8000-00805F9B34FB")
    private val lock = Any()

    private fun adapter() =
        (activity.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager?)?.adapter

    private fun allowed(): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.S || getPermissionState("bluetooth") == PermissionState.GRANTED

    @SuppressLint("MissingPermission")
    @Command
    fun listPaired(invoke: Invoke) {
        if (!allowed()) {
            invoke.reject("Bluetooth permission not granted")
            return
        }
        val a = adapter()
        if (a == null) {
            invoke.reject("This device has no Bluetooth")
            return
        }
        val list = JSArray()
        for (d in a.bondedDevices) {
            val o = JSObject()
            o.put("name", d.name ?: "")
            o.put("address", d.address)
            list.put(o)
        }
        val ret = JSObject()
        ret.put("devices", list)
        invoke.resolve(ret)
    }

    @SuppressLint("MissingPermission")
    @Command
    fun write(invoke: Invoke) {
        val args = invoke.parseArgs(WriteArgs::class.java)
        thread {
            try {
                if (!allowed()) throw Exception("Bluetooth permission not granted")
                val a = adapter() ?: throw Exception("This device has no Bluetooth")
                if (!a.isEnabled) throw Exception("Bluetooth is off")
                val dev = a.bondedDevices.firstOrNull { it.address.equals(args.target, true) || it.name == args.target }
                    ?: throw Exception("Printer '${args.target}' is not paired")
                val bytes = Base64.decode(args.data, Base64.DEFAULT)
                synchronized(lock) {
                    a.cancelDiscovery()
                    var s: BluetoothSocket = dev.createRfcommSocketToServiceRecord(spp)
                    try {
                        s.connect()
                    } catch (e: Exception) {
                        try { s.close() } catch (_: Exception) {}
                        // Some low-cost printers only answer on RFCOMM channel 1.
                        val m = dev.javaClass.getMethod("createRfcommSocket", Int::class.javaPrimitiveType)
                        s = m.invoke(dev, 1) as BluetoothSocket
                        s.connect()
                    }
                    try {
                        val out = s.outputStream
                        var i = 0
                        while (i < bytes.size) {
                            val n = minOf(1024, bytes.size - i)
                            out.write(bytes, i, n)
                            out.flush()
                            i += n
                        }
                        // Let the printer drain its buffer before the link closes.
                        Thread.sleep(300L + bytes.size / 20)
                    } finally {
                        try { s.close() } catch (_: Exception) {}
                    }
                }
                invoke.resolve()
            } catch (e: Exception) {
                invoke.reject(e.message ?: e.toString())
            }
        }
    }
}
