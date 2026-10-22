package com.pixels.agent.platform.system

import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import com.pixels.agent.core.StorageFailure
import java.io.File
import org.json.JSONObject

/** Runtime facts, not preferences. The OS callback can persist without starting transport. */
internal class PackageChangeStore(context: Context) :
    SQLiteOpenHelper(context, File(context.noBackupFilesDir, "package-changes.db").path, null, 1) {
    override fun onConfigure(db: SQLiteDatabase) {
        db.execSQL("PRAGMA synchronous=FULL")
    }

    override fun onCreate(db: SQLiteDatabase) {
        db.execSQL(
            "CREATE TABLE changes(id TEXT PRIMARY KEY,owner TEXT NOT NULL,nonce TEXT NOT NULL,record TEXT NOT NULL,confirmation TEXT,session INTEGER)"
        )
    }

    override fun onUpgrade(db: SQLiteDatabase, oldVersion: Int, newVersion: Int) {
        error("unsupported_package_journal_version")
    }

    fun read(id: String, owner: String? = null): JSONObject {
        readableDatabase
            .rawQuery(
                "SELECT owner,nonce,record,confirmation,session FROM changes WHERE id=?",
                arrayOf(id),
            )
            .use { row ->
                if (!row.moveToFirst() || (owner != null && owner != row.getString(0)))
                    throw StorageFailure("app_change_not_found")
                return obj(
                    "owner" to row.getString(0),
                    "nonce" to row.getString(1),
                    "result" to JSONObject(row.getString(2)),
                    "confirmation" to row.getString(3),
                    "session" to if (row.isNull(4)) null else row.getInt(4),
                )
            }
    }

    fun create(id: String, owner: String, result: JSONObject) {
        val db = writableDatabase
        db.beginTransaction()
        try {
            db.rawQuery("SELECT record FROM changes", null).use { rows ->
                if (rows.count >= 512) throw StorageFailure("app_change_journal_full")
                while (rows.moveToNext()) {
                    if (
                        JSONObject(rows.getString(0)).getString("state") in
                            listOf("submitted", "waiting_user_action")
                    )
                        throw StorageFailure("app_change_busy")
                }
            }
            db.insertOrThrow(
                "changes",
                null,
                ContentValues().apply {
                    put("id", id)
                    put("owner", owner)
                    put("nonce", java.util.UUID.randomUUID().toString())
                    put("record", result.toString())
                },
            )
            db.setTransactionSuccessful()
        } finally {
            db.endTransaction()
        }
    }

    fun session(id: String, value: Int) {
        writableDatabase.update(
            "changes",
            ContentValues().apply { put("session", value) },
            "id=?",
            arrayOf(id),
        )
    }

    /** Terminal callback facts never regress when an older pending callback arrives late. */
    fun transition(
        id: String,
        state: String,
        error: String? = null,
        confirmation: String? = null,
        note: String? = null,
    ) {
        val db = writableDatabase
        db.beginTransaction()
        try {
            val value = read(id).getJSONObject("result")
            if (
                value.getString("state") in
                    listOf("submitted", "waiting_user_action", "unconfirmed")
            ) {
                value.put("state", state).put("error", error ?: JSONObject.NULL)
                if (note != null) {
                    val notes = value.getJSONArray("notes")
                    if (notes.length() < 8) notes.put(note.take(256))
                    else notes.put(7, note.take(256))
                }
                db.update(
                    "changes",
                    ContentValues().apply {
                        put("record", value.toString())
                        put("confirmation", confirmation)
                    },
                    "id=?",
                    arrayOf(id),
                )
            }
            db.setTransactionSuccessful()
        } finally {
            db.endTransaction()
        }
    }

    /** Claim the one external activity dispatch before starting Android's installer. */
    fun claimActivity(id: String, taskId: Int): Boolean {
        val db = writableDatabase
        db.beginTransaction()
        try {
            val value = read(id).getJSONObject("result")
            if (
                value.optBoolean("activity_dispatched") ||
                    value.getString("state") !in listOf("submitted", "waiting_user_action")
            )
                return false
            value
                .put("activity_dispatched", true)
                .put("activity_task", taskId)
                .put("state", "waiting_user_action")
            db.update(
                "changes",
                ContentValues().apply { put("record", value.toString()) },
                "id=?",
                arrayOf(id),
            )
            db.setTransactionSuccessful()
            return true
        } finally {
            db.endTransaction()
        }
    }
}
