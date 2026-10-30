package com.pixels.agent.core

data class TextInsertionObservation(val text: String?, val start: Int, val end: Int)

/** Confirm the text separately from the caret. Never replay a text mutation. */
fun confirmTextInsertion(
    expected: String,
    caret: Int,
    read: () -> TextInsertionObservation?,
    moveCaret: () -> Unit,
): AutomationResult {
    val observed = read()
    if (observed == null || observed.text != expected)
        return AutomationResult.Failed(AutomationError.UNCONFIRMED)
    if (observed.start == caret && observed.end == caret) return AutomationResult.Dispatched(true)
    val selected =
        try {
            moveCaret()
            read()
        } catch (_: RuntimeException) {
            null
        }
    if (selected != null && selected.text != expected)
        return AutomationResult.Failed(AutomationError.UNCONFIRMED)
    return AutomationResult.Dispatched(
        true,
        warnings =
            if (selected?.start == caret && selected.end == caret) emptyList()
            else
                listOf(
                    "Text was inserted, but the caret position could not be confirmed. Do not repeat the text insertion."
                ),
    )
}
