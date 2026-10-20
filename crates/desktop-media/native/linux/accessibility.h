#pragma once
#include <gtk/gtk.h>
#include <gtk/gtk-a11y.h>

namespace pab_linux {
namespace accessibility {
inline AtkStateSet* (*button_ref_state_set)(AtkObject*) = nullptr;

inline AtkStateSet* button_states(AtkObject* object) {
    // GTK 3.24 GtkButtonAccessible returns NULL after its widget is destroyed.
    // AT-SPI can still hold this valid accessible while serializing its cache.
    // Preserve the live-widget implementation; only provide the defunct state
    // which GtkWidgetAccessible already supplies for a destroyed widget.
    if (gtk_accessible_get_widget(GTK_ACCESSIBLE(object)))
        return button_ref_state_set(object);
    auto* states = atk_state_set_new();
    atk_state_set_add_state(states, ATK_STATE_DEFUNCT);
    return states;
}

// Call once on the GUI main thread before creating any GTK widgets. This is a
// process-local compatibility override of a public ATK virtual method; no
// system library, log handler, accessibility setting or input behavior changes.
inline bool initialize() {
    if (button_ref_state_set) return true;
    auto* klass = ATK_OBJECT_CLASS(g_type_class_ref(GTK_TYPE_BUTTON_ACCESSIBLE));
    if (!klass || !klass->ref_state_set) return false;
    // Check behavior, not distribution version strings. A detached accessible
    // is never exposed as a widget or registered in the application tree.
    auto* probe = ATK_OBJECT(g_object_new(GTK_TYPE_BUTTON_ACCESSIBLE, nullptr));
    auto* states = klass->ref_state_set(probe);
    g_object_unref(probe);
    if (states) {
        g_object_unref(states);
        g_type_class_unref(klass);
        return false; // Runtime already returns a state set for this case.
    }
    button_ref_state_set = klass->ref_state_set;
    klass->ref_state_set = button_states;
    // Keep this class reference for the process lifetime along with the saved
    // callback. Installation precedes subclass initialization and widget use.
    return true;
}
}
}
