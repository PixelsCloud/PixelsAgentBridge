package com.pixels.agent.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.pixels.agent.R

@Composable
internal fun ProductNavigation(selected: Int, onSelected: (Int) -> Unit) {
    val labels =
        listOf(
            R.string.product_tab_home,
            R.string.product_tab_access,
            R.string.product_tab_activity,
            R.string.product_tab_settings,
        )
    val icons = listOf(Glyph.HOME, Glyph.FOLDER, Glyph.ACTIVITY, Glyph.SETTINGS)
    Surface(color = MaterialTheme.colorScheme.surface) {
        Row(
            Modifier.fillMaxWidth()
                .navigationBarsPadding()
                .height(72.dp)
                .padding(horizontal = 16.dp)
                .selectableGroup()
        ) {
            labels.forEachIndexed { index, label ->
                val active = selected == index
                val interactions = remember { MutableInteractionSource() }
                val pressed by interactions.collectIsPressedAsState()
                val itemScale by
                    animateFloatAsState(
                        targetValue = if (pressed) .9f else 1f,
                        animationSpec = tween(100),
                        label = "tabPress",
                    )
                val color =
                    if (active) MaterialTheme.colorScheme.primary
                    else MaterialTheme.colorScheme.onSurfaceVariant
                Box(
                    Modifier.weight(1f)
                        .fillMaxHeight()
                        .selectable(
                            selected = active,
                            interactionSource = interactions,
                            indication = null,
                            role = Role.Tab,
                            onClick = { onSelected(index) },
                        ),
                    contentAlignment = Alignment.Center,
                ) {
                    Column(
                        Modifier.graphicsLayer {
                                scaleX = itemScale
                                scaleY = itemScale
                            }
                            .padding(horizontal = 12.dp, vertical = 6.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        PixelsIcon(icons[index], tint = color, bold = active)
                        Text(
                            stringResource(label),
                            color = color,
                            style = MaterialTheme.typography.labelSmall,
                            fontWeight = if (active) FontWeight.Bold else FontWeight.Normal,
                        )
                    }
                }
            }
        }
    }
}
