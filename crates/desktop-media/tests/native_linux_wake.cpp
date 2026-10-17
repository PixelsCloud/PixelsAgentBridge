// Exercise the real D-Bus power query and parent-pipe cancellation against an
// isolated bus. No compositor, system power setting or OS input is touched.
#include "../native/linux/dma_capture.h"
#include <cassert>

using namespace pab_linux;

class PowerService {
    GTestDBus* bus_ = g_test_dbus_new(G_TEST_DBUS_NONE);
    GMainContext* context_ = g_main_context_new();
    GMainLoop* loop_ = g_main_loop_new(context_, false);
    std::thread thread_;
    std::atomic<bool> ready_{false};
public:
    std::atomic<int> power{0};
    std::atomic<unsigned> writes{0};
    std::atomic<bool> deny_write{false};
    Connection client;
    PowerService() {
        g_test_dbus_up(bus_);
        const std::string address = g_test_dbus_get_bus_address(bus_);
        const auto flags = GDBusConnectionFlags(G_DBUS_CONNECTION_FLAGS_AUTHENTICATION_CLIENT |
            G_DBUS_CONNECTION_FLAGS_MESSAGE_BUS_CONNECTION);
        thread_ = std::thread([this, address, flags] {
            g_main_context_push_thread_default(context_);
            Connection server(g_dbus_connection_new_for_address_sync(address.c_str(), flags, nullptr, nullptr, nullptr));
            assert(server);
            auto* info = g_dbus_node_info_new_for_xml(
                "<node><interface name='org.gnome.Mutter.DisplayConfig'>"
                "<property name='PowerSaveMode' type='i' access='readwrite'/>"
                "</interface></node>", nullptr);
            assert(info);
            GDBusInterfaceVTable table{};
            table.get_property = [](GDBusConnection*, const gchar*, const gchar*, const gchar*,
                    const gchar*, GError**, gpointer data) -> GVariant* {
                return g_variant_new_int32(static_cast<PowerService*>(data)->power.load());
            };
            table.set_property = [](GDBusConnection*, const gchar*, const gchar*, const gchar*,
                    const gchar*, GVariant* value, GError** error, gpointer data) -> gboolean {
                auto& self = *static_cast<PowerService*>(data);
                if (self.deny_write.load()) {
                    g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED, "test power denied");
                    return false;
                }
                ++self.writes; self.power = g_variant_get_int32(value); return true;
            };
            const auto id = g_dbus_connection_register_object(server.get(),
                "/org/gnome/Mutter/DisplayConfig", info->interfaces[0], &table, this, nullptr, nullptr);
            assert(id);
            call(server.get(), "org.freedesktop.DBus", "/org/freedesktop/DBus",
                "org.freedesktop.DBus", "RequestName",
                g_variant_new("(su)", "org.gnome.Mutter.DisplayConfig", 0u));
            ready_.store(true);
            g_main_loop_run(loop_);
            g_dbus_connection_unregister_object(server.get(), id);
            g_dbus_node_info_unref(info);
            g_dbus_connection_close_sync(server.get(), nullptr, nullptr);
            g_main_context_pop_thread_default(context_);
        });
        const auto deadline = Clock::now() + std::chrono::seconds(3);
        while (!ready_.load()) {
            assert(Clock::now() < deadline);
            std::this_thread::sleep_for(std::chrono::milliseconds(1));
        }
        client.reset(g_dbus_connection_new_for_address_sync(address.c_str(), flags, nullptr, nullptr, nullptr));
        assert(client);
    }
    ~PowerService() {
        g_dbus_connection_close_sync(client.get(), nullptr, nullptr);
        client.reset();
        g_main_loop_quit(loop_);
        thread_.join();
        g_main_loop_unref(loop_);
        g_main_context_unref(context_);
        g_test_dbus_down(bus_);
        g_object_unref(bus_);
    }
};

int main(int argc, char** argv) {
    g_test_init(&argc, &argv, nullptr);
    // Start GTestDBus (including its non-exec watcher child) before creating
    // the synthetic parent pipe, so neither child can retain its writer.
    PowerService service;
    int pipefd[2]; assert(pipe2(pipefd, O_CLOEXEC) == 0);
    dma::Fd read_end(pipefd[0]), write_end(pipefd[1]);
    assert(dup2(read_end.get(), STDIN_FILENO) == STDIN_FILENO);
    unsigned checks = 0;
    // Already awake and unsupported power status never inject another wake.
    for (const int mode : {0, -1}) {
        service.power = mode;
        dma::power_on(service.client.get());
        assert(service.writes == 0);
        dma::await_awake(service.client.get(), [] { assert(false); });
        ++checks;
    }
    for (const int mode : {1, 2, 3}) {
        service.power = mode;
        dma::power_on(service.client.get());
        assert(service.power == 0 && service.writes == unsigned(mode)); ++checks;
    }
    service.power = 3; service.deny_write = true;
    try { dma::power_on(service.client.get()); assert(false); }
    catch (const std::runtime_error& e) { assert(std::string(e.what()).find("test power denied") != std::string::npos); }
    assert(service.power == 3 && service.writes == 3); service.deny_write = false; ++checks;
    for (const unsigned needed : {1u, 2u}) {
        service.power = 3; unsigned attempts = 0;
        dma::await_awake(service.client.get(), [&] {
            if (++attempts == needed) service.power = 0;
        });
        assert(attempts == needed); ++checks;
    }
    service.power = 3; unsigned attempts = 0;
    const auto started = Clock::now();
    try { dma::await_awake(service.client.get(), [&] { ++attempts; }); assert(false); }
    catch (const std::runtime_error& e) { assert(std::string(e.what()) == "Display did not wake"); }
    assert(attempts == 2 && Clock::now() - started < std::chrono::seconds(4)); ++checks;
    attempts = 0;
    try {
        dma::await_awake(service.client.get(), [&] {
            ++attempts; throw std::runtime_error("broker disconnected");
        });
        assert(false);
    } catch (const std::runtime_error& e) { assert(std::string(e.what()) == "broker disconnected"); }
    assert(attempts == 1); ++checks;
    // Parent cancellation wins before the next retry; no wake after close.
    std::thread cancel([&] {
        std::this_thread::sleep_for(std::chrono::milliseconds(60));
        write_end = dma::Fd();
    });
    attempts = 0; const auto cancelled = Clock::now();
    try { dma::await_awake(service.client.get(), [&] { ++attempts; }); assert(false); }
    catch (const std::runtime_error& e) { assert(std::string(e.what()) == "Parent pipe ended"); }
    cancel.join();
    assert(attempts == 0 && Clock::now() - cancelled < std::chrono::milliseconds(500)); ++checks;
    printf("Linux display wake checks passed: %u\n", checks);
}
