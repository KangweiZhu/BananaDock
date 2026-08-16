#include "screenedge.h"

#include <QGuiApplication>
#include <QQuickWindow>
#include <qpa/qplatformnativeinterface.h>

#include <wayland-client.h>

ScreenEdge::ScreenEdge()
    : QWaylandClientExtensionTemplate<ScreenEdge>(1)
{
    initialize();
}

ScreenEdge::~ScreenEdge()
{
    if (m_edge) {
        kde_auto_hide_screen_edge_v1_destroy(m_edge);
    }
}

bool ScreenEdge::isAvailable() const
{
    return isActive();
}

void ScreenEdge::attach(QQuickWindow *window)
{
    if (m_edge || !window || !isActive()) {
        return;
    }

    auto *native = QGuiApplication::platformNativeInterface();
    if (!native) {
        return;
    }
    auto *surface = static_cast<wl_surface *>(native->nativeResourceForWindow("surface", window));
    if (!surface) {
        return;
    }

    m_edge = get_auto_hide_screen_edge(QtWayland::kde_screen_edge_manager_v1::border_bottom, surface);
}

void ScreenEdge::setHidden(bool hidden)
{
    if (!m_edge) {
        return;
    }
    // No early-out on an unchanged value: when the compositor reveals the
    // surface after an edge trigger it drops the activation on its side without
    // telling us, so re-arming has to go through even if nothing looks changed.
    m_hidden = hidden;

    if (hidden) {
        kde_auto_hide_screen_edge_v1_activate(m_edge);
    } else {
        kde_auto_hide_screen_edge_v1_deactivate(m_edge);
    }
}
