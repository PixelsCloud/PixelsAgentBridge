package com.pixels.agent.platform.system

import org.json.JSONArray
import org.json.JSONObject

internal fun obj(vararg fields: Pair<String, Any?>): JSONObject =
    JSONObject().apply { fields.forEach { (key, value) -> put(key, value ?: JSONObject.NULL) } }

internal fun array(values: Iterable<*>): JSONArray = JSONArray(values.toList())

internal fun response(
    data: JSONObject,
    count: Int = 1,
    warnings: List<String> = emptyList(),
    truncated: Boolean = false,
) = obj("data" to data, "count" to count, "warnings" to array(warnings), "truncated" to truncated)

internal fun JSONObject.optionalString(name: String): String? =
    if (isNull(name)) null else getString(name)
