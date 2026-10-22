package com.pixels.agent.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.pixels.agent.ActivityHistory
import com.pixels.agent.ActivityKind
import com.pixels.agent.ActivityOutcome
import com.pixels.agent.ActivityRecord
import com.pixels.agent.R
import java.text.DateFormat
import java.util.Date

@Composable
internal fun ActivityPage(history: ActivityHistory, onRefresh: () -> Unit) {
    var filter by rememberSaveable { mutableIntStateOf(0) }
    val listState = rememberLazyListState()
    LaunchedEffect(filter) { listState.scrollToItem(0) }
    val rows =
        history.records.filter {
            filter == 0 || (it.outcome == ActivityOutcome.COMPLETED) == (filter == 1)
        }
    Column(Modifier.fillMaxSize().padding(horizontal = 20.dp)) {
        ActivityFilters(filter) { filter = it }
        LazyColumn(
            Modifier.weight(1f).fillMaxWidth().testTag("activity_list"),
            state = listState,
            contentPadding = PaddingValues(bottom = 24.dp),
        ) {
            if (rows.isEmpty())
                item { ProductCard { EmptyActivity(filtered = history.records.isNotEmpty()) } }
            val groups = rows.groupBy { record ->
                record.timestamp?.let { DateFormat.getDateInstance().format(Date(it)) }
            }
            groups.forEach { (date, records) ->
                item(key = "date:$date") {
                    Box(Modifier.padding(top = 16.dp, bottom = 12.dp)) {
                        SectionHeading(date ?: stringResource(R.string.product_activity_history))
                    }
                }
                items(records.size, key = { records[it].id }) { index ->
                    Surface(
                        color = MaterialTheme.colorScheme.surface,
                        shape =
                            androidx.compose.foundation.shape.RoundedCornerShape(
                                topStart = if (index == 0) 16.dp else 0.dp,
                                topEnd = if (index == 0) 16.dp else 0.dp,
                                bottomStart = if (index == records.lastIndex) 16.dp else 0.dp,
                                bottomEnd = if (index == records.lastIndex) 16.dp else 0.dp,
                            ),
                    ) {
                        Column {
                            ActivityRow(records[index])
                            if (index < records.lastIndex) CardDivider()
                        }
                    }
                }
            }
            item {
                if (history.unavailable)
                    Text(
                        stringResource(R.string.product_activity_unavailable),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                TextButton(onClick = onRefresh) { Text(stringResource(R.string.product_refresh)) }
            }
        }
    }
}

@Composable
private fun ActivityFilters(selected: Int, onSelected: (Int) -> Unit) {
    val labels =
        listOf(R.string.product_all, R.string.product_completed, R.string.product_incomplete)
    BoxWithConstraints(
        Modifier.fillMaxWidth().padding(top = 4.dp, bottom = 8.dp).testTag("activity_filters")
    ) {
        val tabWidth = maxWidth / labels.size
        val indicatorOffset by
            animateDpAsState(
                targetValue = tabWidth * selected + (tabWidth - 28.dp) / 2,
                animationSpec = tween(180),
                label = "activityFilterIndicator",
            )
        Column {
            Row(Modifier.fillMaxWidth().height(48.dp).selectableGroup()) {
                labels.forEachIndexed { index, label ->
                    val active = selected == index
                    val interactions = remember { MutableInteractionSource() }
                    val pressed by interactions.collectIsPressedAsState()
                    val scale by
                        animateFloatAsState(
                            targetValue = if (pressed) .94f else 1f,
                            animationSpec = tween(100),
                            label = "activityFilterPress",
                        )
                    val color by
                        animateColorAsState(
                            targetValue =
                                if (active) MaterialTheme.colorScheme.primary
                                else MaterialTheme.colorScheme.onSurfaceVariant,
                            animationSpec = tween(180),
                            label = "activityFilterColor",
                        )
                    Box(
                        Modifier.weight(1f)
                            .fillMaxHeight()
                            .testTag("activity_filter_$index")
                            .selectable(
                                selected = active,
                                interactionSource = interactions,
                                indication = null,
                                role = Role.Tab,
                                onClick = { onSelected(index) },
                            ),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            stringResource(label),
                            Modifier.graphicsLayer {
                                scaleX = scale
                                scaleY = scale
                            },
                            color = color,
                            style = MaterialTheme.typography.titleSmall,
                            fontWeight = if (active) FontWeight.Bold else FontWeight.Medium,
                        )
                    }
                }
            }
            HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        }
        Box(
            Modifier.align(Alignment.BottomStart)
                .offset(x = indicatorOffset)
                .width(28.dp)
                .height(3.dp)
                .background(MaterialTheme.colorScheme.primary, RoundedCornerShape(3.dp))
        )
    }
}

@Composable
internal fun ActivityRow(record: ActivityRecord, compact: Boolean = false) {
    val title =
        when (record.kind) {
            ActivityKind.INTERFACE -> R.string.product_activity_interface
            ActivityKind.NOTIFICATION -> R.string.product_activity_notification
            ActivityKind.SYSTEM -> R.string.product_activity_system
            ActivityKind.FILE -> R.string.product_activity_file
            ActivityKind.DOCUMENT -> R.string.product_activity_document
            ActivityKind.EXECUTION -> R.string.product_activity_execution
        }
    val icon =
        when (record.kind) {
            ActivityKind.INTERFACE -> Glyph.POINTER
            ActivityKind.NOTIFICATION -> Glyph.BELL
            ActivityKind.FILE -> Glyph.UPLOAD
            ActivityKind.DOCUMENT -> Glyph.DOCUMENT
            else -> Glyph.SCREEN
        }
    val status =
        when (record.outcome) {
            ActivityOutcome.COMPLETED -> R.string.product_completed
            ActivityOutcome.INCOMPLETE -> R.string.product_activity_failed
            ActivityOutcome.UNSUPPORTED -> R.string.product_unsupported
            ActivityOutcome.PENDING -> R.string.product_activity_pending
            ActivityOutcome.DISPATCHED -> R.string.product_activity_dispatched
        }
    Row(
        Modifier.fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = if (compact) 8.dp else 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Surface(
            shape = androidx.compose.foundation.shape.CircleShape,
            color = MaterialTheme.colorScheme.surfaceVariant,
        ) {
            PixelsIcon(icon, Modifier.padding(8.dp), MaterialTheme.colorScheme.primary)
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(
                stringResource(title),
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.SemiBold,
            )
            if (!compact)
                Text(
                    stringResource(
                        when (record.outcome) {
                            ActivityOutcome.UNSUPPORTED -> R.string.product_unsupported_description
                            ActivityOutcome.INCOMPLETE -> R.string.product_activity_failed_hint
                            ActivityOutcome.DISPATCHED -> R.string.product_activity_dispatched_hint
                            ActivityOutcome.PENDING -> R.string.product_activity_pending
                            else -> R.string.product_activity_completed_hint
                        }
                    ),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
        }
        Column(
            horizontalAlignment = Alignment.End,
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                record.timestamp?.let {
                    DateFormat.getTimeInstance(DateFormat.SHORT).format(Date(it))
                } ?: "—",
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Surface(
                shape = androidx.compose.foundation.shape.RoundedCornerShape(6.dp),
                color =
                    if (record.outcome == ActivityOutcome.COMPLETED)
                        MaterialTheme.colorScheme.primaryContainer
                    else MaterialTheme.colorScheme.background,
            ) {
                Text(
                    stringResource(status),
                    Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                    style = MaterialTheme.typography.labelSmall,
                    color =
                        if (record.outcome == ActivityOutcome.COMPLETED)
                            MaterialTheme.colorScheme.primary
                        else MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
internal fun EmptyActivity(compact: Boolean = false, filtered: Boolean = false) {
    Column(
        Modifier.fillMaxWidth().padding(if (compact) 20.dp else 32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        PixelsIcon(Glyph.ACTIVITY, Modifier.size(28.dp), MaterialTheme.colorScheme.onSurfaceVariant)
        Text(
            stringResource(
                if (filtered) R.string.product_activity_filter_empty
                else R.string.product_activity_empty
            ),
            style = MaterialTheme.typography.bodyMedium,
        )
        if (!compact && !filtered)
            Text(
                stringResource(R.string.product_activity_empty_hint),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
    }
}
