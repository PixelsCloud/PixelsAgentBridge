package com.pixels.agent.fixture

import android.annotation.SuppressLint
import android.app.Activity
import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.os.Bundle
import android.text.InputType
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import com.pixels.agent.R
import com.pixels.agent.platform.capture.ProjectionCaptureService
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** Debug-only deterministic target. Its optional probe operates only this fixture. */
// Opaque test identifiers and test values deliberately stay identical across device locales.
@SuppressLint("SetTextI18n")
class AutomationFixtureActivity : Activity() {
    private val probeScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    var saveCount = 0
        private set

    lateinit var field: EditText
        private set

    lateinit var status: TextView
        private set

    private lateinit var firstPage: LinearLayout
    private lateinit var secondPage: LinearLayout
    private lateinit var secondField: EditText
    private lateinit var counter: TextView
    private lateinit var list: ScrollView
    private var ordinaryInputType = 0

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (intent.getBooleanExtra("finish_fixture", false)) {
            finish()
            return
        }
        ProductionProbe.targetCreated()
        field = EditText(this).apply { contentDescription = getString(R.string.fixture_input) }
        ordinaryInputType = field.inputType
        configureSecureTarget(intent)
        status = TextView(this).apply { setText(R.string.fixture_idle) }
        val save =
            Button(this).apply {
                setText(R.string.fixture_save)
                contentDescription = getString(R.string.fixture_save)
                setOnClickListener {
                    saveCount++
                    counter.text = "fixture_save_count:$saveCount"
                    status.text = getString(R.string.fixture_saved, field.text.toString())
                    if (intent.getBooleanExtra("run_remote_fixture", false)) {
                        val count = saveCount
                        probeScope.launch(Dispatchers.IO) {
                            File(filesDir, "remote-fixture-effects.json")
                                .writeText(
                                    JSONObject()
                                        .put(
                                            "request_id",
                                            intent.getStringExtra("remote_request_id"),
                                        )
                                        .put("save_count", count)
                                        .toString()
                                )
                        }
                    }
                }
            }
        counter = TextView(this).apply { text = "fixture_save_count:0" }
        secondField = EditText(this).apply { contentDescription = "fixture_second_input" }
        firstPage =
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                addView(field)
                addView(save)
                addView(
                    Button(this@AutomationFixtureActivity).apply {
                        text = "fixture_next_page"
                        contentDescription = text
                        setOnClickListener {
                            firstPage.visibility = View.GONE
                            secondPage.visibility = View.VISIBLE
                        }
                    }
                )
            }
        secondPage =
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                visibility = View.GONE
                addView(secondField)
                addView(
                    Button(this@AutomationFixtureActivity).apply {
                        text = "fixture_submit_form"
                        contentDescription = text
                        setOnClickListener {
                            saveCount++
                            counter.text = "fixture_save_count:$saveCount"
                            status.text = "fixture_form_saved:${field.text}:${secondField.text}"
                        }
                    }
                )
                addView(
                    Button(this@AutomationFixtureActivity).apply {
                        text = "fixture_previous_page"
                        contentDescription = text
                        setOnClickListener {
                            secondPage.visibility = View.GONE
                            firstPage.visibility = View.VISIBLE
                        }
                    }
                )
            }
        val items =
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                for (index in 1..50) {
                    addView(
                        Button(this@AutomationFixtureActivity).apply {
                            text = "fixture_item_$index"
                            contentDescription = text
                            setOnClickListener { status.text = "fixture_selected:$index" }
                        }
                    )
                }
            }
        list =
            ScrollView(this).apply {
                contentDescription = "fixture_scroll_list"
                addView(items)
            }
        setContentView(
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                addView(firstPage)
                addView(secondPage)
                addView(status)
                addView(counter)
                addView(
                    Button(this@AutomationFixtureActivity).apply {
                        text = "fixture_long_click"
                        contentDescription = text
                        setOnLongClickListener {
                            status.text = "fixture_long_clicked"
                            true
                        }
                    }
                )
                addView(
                    list,
                    LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f),
                )
            }
        )
        if (intent.getBooleanExtra("run_smoke_probe", false)) {
            probeScope.launch {
                val requestId = intent.getStringExtra("smoke_request_id")?.take(64).orEmpty()
                withContext(Dispatchers.IO) {
                    File(filesDir, "automation-smoke.json")
                        .writeText(
                            JSONObject()
                                .put("status", "running")
                                .put("request_id", requestId)
                                .toString()
                        )
                }
                val result =
                    try {
                        AutomationSmokeProbe.run(this@AutomationFixtureActivity)
                    } catch (cancelled: CancellationException) {
                        throw cancelled
                    } catch (error: IllegalStateException) {
                        JSONObject().put("status", "failed").put("reason", error.message)
                    } catch (_: NoSuchElementException) {
                        JSONObject()
                            .put("status", "failed")
                            .put("reason", "fixture_element_missing")
                    }
                withContext(Dispatchers.IO) {
                    File(filesDir, "automation-smoke.json")
                        .writeText(result.put("request_id", requestId).toString())
                }
            }
        }
        if (intent.getBooleanExtra("run_remote_fixture", false)) {
            probeScope.launch {
                try {
                    RemoteFixture.run(
                        this@AutomationFixtureActivity,
                        intent.getStringExtra("remote_request_id").orEmpty(),
                    )
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (_: Exception) {
                    withContext(Dispatchers.IO) {
                        File(filesDir, "remote-fixture-access.json")
                            .writeText(
                                JSONObject()
                                    .put("request_id", intent.getStringExtra("remote_request_id"))
                                    .put("ready", false)
                                    .put("error", "remote_fixture_failed")
                                    .toString()
                            )
                    }
                }
            }
        }
    }

    override fun onDestroy() {
        ProductionProbe.targetDestroyed()
        probeScope.cancel()
        super.onDestroy()
    }

    override fun onNewIntent(incoming: Intent) {
        super.onNewIntent(incoming)
        configureSecureTarget(incoming)
        if (incoming.getBooleanExtra("finish_fixture", false)) {
            finish()
            return
        }
        if (incoming.getBooleanExtra("reset_fixture", false)) {
            field.setText("")
            secondField.setText("")
            secondPage.visibility = View.GONE
            firstPage.visibility = View.VISIBLE
            status.setText(R.string.fixture_idle)
            counter.text = "fixture_save_count:0"
            saveCount = 0
            list.scrollTo(0, 0)
            return
        }
        // Test orchestration can request the real system prompt; it cannot grant consent.
        if (
            intent.getBooleanExtra("run_remote_fixture", false) &&
                incoming.getStringExtra("remote_request_id") ==
                    intent.getStringExtra("remote_request_id") &&
                incoming.getBooleanExtra("request_capture_consent", false) &&
                android.os.Build.VERSION.SDK_INT < 30
        ) {
            startActivityForResult(
                getSystemService(MediaProjectionManager::class.java).createScreenCaptureIntent(),
                20,
            )
        }
    }

    private fun configureSecureTarget(request: Intent) {
        if (!request.hasExtra("secure_fixture")) return
        if (request.getBooleanExtra("secure_fixture", false)) {
            window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
            field.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
            field.setText("fixture-only-secret")
        } else {
            window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
            field.inputType = ordinaryInputType
            field.setText("")
        }
    }

    @Deprecated("Debug fixture supports the Android 9 activity result contract")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == 20 && resultCode == RESULT_OK && data != null) {
            ProjectionCaptureService.start(this, resultCode, data)
        }
    }
}
