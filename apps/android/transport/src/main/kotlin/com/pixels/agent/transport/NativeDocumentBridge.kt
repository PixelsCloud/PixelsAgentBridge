package com.pixels.agent.transport

/** Loaded only by the isolated parser service. Descriptors are its only file access. */
class NativeDocumentBridge {
    init {
        System.loadLibrary("pab_android_agent")
    }

    external fun limitResources(): String

    external fun extract(
        input: Int,
        extension: String,
        mode: String,
        output: Int,
        recognizer: PageRecognizer,
    ): String

    external fun recognize(gray: ByteArray, width: Int, height: Int, model: ByteArray): String
}

/** Called synchronously in the disposable parser process, using the same input descriptor. */
interface PageRecognizer {
    fun recognize(page: Int): String
}
