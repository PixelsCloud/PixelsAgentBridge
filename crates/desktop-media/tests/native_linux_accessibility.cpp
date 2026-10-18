// Opt-in GTK runtime check. Creates no window and sends no input.
#include "../native/linux/accessibility.h"
#include <cstdio>
#include <initializer_list>
#include <string>

int main(int argc, char** argv) {
    if (argc != 2 || std::string(argv[1]) != "check") return 2;
    const bool installed = pab_linux::accessibility::initialize();
    if (!gtk_init_check(nullptr, nullptr)) return 3;
    auto* widget = gtk_button_new_with_label("Owned accessibility state test");
    g_object_ref_sink(widget);
    auto* object = ATK_OBJECT(g_object_ref(gtk_widget_get_accessible(widget)));
    bool finalized = false;
    g_object_weak_ref(G_OBJECT(object), +[](gpointer data, GObject*) { *static_cast<bool*>(data) = true; }, &finalized);
    bool live_equal = true;
    for (bool enabled : {true, false, true}) {
        gtk_widget_set_sensitive(widget, enabled);
        auto* states = atk_object_ref_state_set(object);
        if (!states || atk_state_set_contains_state(states, ATK_STATE_DEFUNCT)) return 4;
        if (installed) {
            auto* original = pab_linux::accessibility::button_ref_state_set(object);
            auto* delta = atk_state_set_xor_sets(states, original);
            // ATK returns NULL, rather than an empty object, for equal sets.
            live_equal = live_equal && (!delta || atk_state_set_is_empty(delta));
            if (delta) g_object_unref(delta);
            g_object_unref(original);
        }
        g_object_unref(states);
    }
    const bool action_present = ATK_IS_ACTION(object) && atk_action_get_n_actions(ATK_ACTION(object)) == 1;
    gtk_widget_destroy(widget); g_object_unref(widget);
    auto* retired = atk_object_ref_state_set(object);
    const bool defunct = retired && atk_state_set_contains_state(retired, ATK_STATE_DEFUNCT);
    const bool no_action = !atk_action_do_action(ATK_ACTION(object), 0);
    if (retired) g_object_unref(retired);
    g_object_unref(object);
    const bool idempotent = pab_linux::accessibility::initialize() == installed;
    const bool passed = live_equal && action_present && defunct && no_action && finalized && idempotent;
    printf("{\"installed\":%s,\"live_states_equal\":%s,\"live_action_preserved\":%s,\"retired_defunct\":%s,\"retired_action_refused\":%s,\"object_finalized\":%s,\"idempotent\":%s,\"passed\":%s}\n",
        installed ? "true":"false", live_equal ? "true":"false", action_present ? "true":"false", defunct ? "true":"false",
        no_action ? "true":"false", finalized ? "true":"false", idempotent ? "true":"false", passed ? "true":"false");
    return passed ? 0 : 1;
}
